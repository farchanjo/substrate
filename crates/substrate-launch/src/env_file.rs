//! `.env` file loading for launch Services (ADR-0071).
//!
//! A Service may list `env_file` paths whose `KEY=VALUE` pairs are loaded into the
//! child environment. Paths are resolved relative to the profile directory and MUST
//! stay within it (no absolute paths, no `..` traversal, no symlink escape) — the
//! same containment discipline the path jail applies elsewhere (ADR-0035), enforced
//! here without needing the allowlist because the base is the trusted profile dir.
//!
//! Precedence: files apply in listed order (a later file overrides an earlier one),
//! then the inline `env` map overrides all files. The merged map becomes the child's
//! `env_override`, so it still passes the subprocess banned-key validation (a
//! `.env` that sets `LD_PRELOAD`/`DYLD_*` is rejected exactly like an inline `env`).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use substrate_domain::launch::errors::LaunchError;

/// Merges `env_file` contents under `inline`, returning the child `env_override`.
///
/// Files are read in order (later overrides earlier); `inline` overrides every file.
///
/// # Errors
///
/// Returns [`LaunchError::InvalidProfile`] when an `env_file` path is absolute,
/// contains `..`, escapes the profile directory, or cannot be read.
pub(crate) async fn merge_env_files(
    inline: &BTreeMap<String, String>,
    env_files: &[String],
    profile_dir: &Path,
) -> Result<BTreeMap<String, String>, LaunchError> {
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    for file in env_files {
        // A later file overrides an earlier one.
        merged.extend(load_one(file, profile_dir).await?);
    }
    // Inline env overrides every file.
    for (k, v) in inline {
        merged.insert(k.clone(), v.clone());
    }
    Ok(merged)
}

/// Reads and parses one `.env` file, jailed to `profile_dir`.
async fn load_one(
    rel: &str,
    profile_dir: &Path,
) -> Result<BTreeMap<String, String>, LaunchError> {
    let rel_path = PathBuf::from(rel);
    if rel_path.is_absolute() {
        return Err(LaunchError::InvalidProfile {
            msg: format!("env_file '{rel}' must be relative to the profile directory"),
        });
    }
    if rel_path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(LaunchError::InvalidProfile {
            msg: format!("env_file '{rel}' must not contain '..'"),
        });
    }

    let candidate = profile_dir.join(&rel_path);
    let dir = profile_dir.to_path_buf();
    let rel_owned = rel.to_owned();

    // Canonicalize + containment + read on the blocking pool (async zone B, ADR-0003).
    let content = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let canon = std::fs::canonicalize(&candidate)
            .map_err(|e| format!("cannot resolve env_file: {e}"))?;
        let dir_canon = std::fs::canonicalize(&dir)
            .map_err(|e| format!("cannot resolve profile directory: {e}"))?;
        if !canon.starts_with(&dir_canon) {
            return Err("env_file resolves outside the profile directory".to_owned());
        }
        std::fs::read_to_string(&canon).map_err(|e| format!("cannot read env_file: {e}"))
    })
    .await
    .map_err(|e| LaunchError::InvalidProfile {
        msg: format!("env_file '{rel_owned}' load task failed: {e}"),
    })?
    .map_err(|msg| LaunchError::InvalidProfile {
        msg: format!("env_file '{rel_owned}': {msg}"),
    })?;

    Ok(parse_dotenv(&content))
}

/// Parses dotenv content into key/value pairs.
///
/// Supports `KEY=VALUE`, an optional `export ` prefix, `#` comment lines, blank
/// lines, and single- or double-quoted values (double quotes honour `\n \t \r \" \\`
/// escapes; single quotes are literal). An unquoted value may carry a trailing
/// ` # comment`. Malformed lines (no `=`, empty key) are skipped.
pub(crate) fn parse_dotenv(content: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        map.insert(key.to_owned(), parse_value(value.trim()));
    }
    map
}

/// Parses a single dotenv value: quoted (with escapes for double quotes), or an
/// unquoted value with an optional trailing inline comment.
fn parse_value(raw: &str) -> String {
    match raw.as_bytes().first() {
        // Double-quoted: honour escapes, terminate at the first unescaped quote.
        Some(b'"') => unescape_double_quoted(&raw[1..]),
        // Single-quoted: literal up to the next quote.
        Some(b'\'') => {
            let rest = &raw[1..];
            rest.split_once('\'').map_or(rest, |(v, _)| v).to_owned()
        },
        // Unquoted: an inline comment starts at the first " #".
        _ => raw
            .split_once(" #")
            .map_or(raw, |(v, _)| v.trim_end())
            .to_owned(),
    }
}

/// Unescapes a double-quoted dotenv value body (the text after the opening quote),
/// stopping at the first unescaped `"`.
fn unescape_double_quoted(rest: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for c in rest.chars() {
        if escaped {
            out.push(match c {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                other => other,
            });
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            break;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;

    #[test]
    fn parse_dotenv_basic_forms() {
        let content = "\
# a comment
BLANK_BELOW=

KEY=value
export EXPORTED=exp
SPACED = trimmed
DQ=\"hello world\"
SQ='literal $NOPE'
ESC=\"line1\\nline2\"
INLINE=bare # trailing comment
EMPTY_KEY_LINE
=no_key
";
        let m = parse_dotenv(content);
        assert_eq!(m.get("KEY").map(String::as_str), Some("value"));
        assert_eq!(m.get("EXPORTED").map(String::as_str), Some("exp"));
        assert_eq!(m.get("SPACED").map(String::as_str), Some("trimmed"));
        assert_eq!(m.get("DQ").map(String::as_str), Some("hello world"));
        assert_eq!(m.get("SQ").map(String::as_str), Some("literal $NOPE"));
        assert_eq!(m.get("ESC").map(String::as_str), Some("line1\nline2"));
        assert_eq!(m.get("INLINE").map(String::as_str), Some("bare"));
        assert_eq!(m.get("BLANK_BELOW").map(String::as_str), Some(""));
        // Comment line, bare word (no '='), and empty key are all skipped.
        assert!(!m.contains_key("EMPTY_KEY_LINE"));
        assert!(!m.contains_key(""));
    }

    #[tokio::test]
    async fn merge_precedence_inline_over_later_over_earlier() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.env"), "A=1\nB=1\nC=1\n").unwrap();
        std::fs::write(dir.path().join("b.env"), "B=2\nC=2\n").unwrap();
        let inline = BTreeMap::from([("C".to_owned(), "3".to_owned())]);

        let merged = merge_env_files(
            &inline,
            &["a.env".to_owned(), "b.env".to_owned()],
            dir.path(),
        )
        .await
        .unwrap();

        assert_eq!(merged.get("A").map(String::as_str), Some("1")); // only in a
        assert_eq!(merged.get("B").map(String::as_str), Some("2")); // b overrides a
        assert_eq!(merged.get("C").map(String::as_str), Some("3")); // inline overrides both
    }

    #[tokio::test]
    async fn rejects_absolute_and_traversal_and_escape() {
        let dir = tempfile::tempdir().unwrap();
        let inline = BTreeMap::new();

        assert!(matches!(
            merge_env_files(&inline, &["/etc/passwd".to_owned()], dir.path()).await,
            Err(LaunchError::InvalidProfile { .. })
        ));
        assert!(matches!(
            merge_env_files(&inline, &["../secret.env".to_owned()], dir.path()).await,
            Err(LaunchError::InvalidProfile { .. })
        ));
        // Missing file is an error, not a silent empty.
        assert!(matches!(
            merge_env_files(&inline, &["nope.env".to_owned()], dir.path()).await,
            Err(LaunchError::InvalidProfile { .. })
        ));
    }
}
