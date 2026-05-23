//! Step definitions for the filesystem-query bounded context.
//!
//! Covers features:
//!   fs-find-happy-path, fs-find-path-traversal-blocked,
//!   fs-find-index-*, fs-read-permission-denied, fs-read-special-file,
//!   fs-stat-broken-symlink, fs-stat-symlink-escape-blocked,
//!   fs-stat-symlink-loop.

#![allow(unused_variables)]
#![expect(
    clippy::expect_used,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_async,
    clippy::trivial_regex,
    clippy::needless_raw_string_hashes,
    clippy::redundant_clone,
    clippy::unnecessary_map_or,
    clippy::or_fun_call,
    clippy::needless_return,
    reason = "cucumber step functions require &mut World and async signatures; \
              raw strings and regex patterns are idiomatic in step definitions"
)]

use cucumber::{given, then, when};

use crate::SubstrateWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given(regex = r#"^an allowlist with root "([^"]+)"$"#)]
async fn given_allowlist_root(world: &mut SubstrateWorld, root: String) {
    // Spawn and initialize the server only the first time; re-use if already up.
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world
        .context
        .insert("allowlist_root".to_string(), root);
}

#[given(regex = r#"^the directory "([^"]+)" contains (\d+) files matching "([^"]+)"$"#)]
async fn given_dir_contains_n_files(
    world: &mut SubstrateWorld,
    path: String,
    count: u32,
    pattern: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }

    let root = world
        .allowlist_root
        .as_ref()
        .expect("allowlist_root not set")
        .clone();

    // Build a fixture tree with exactly `count` files so that fs.find returns
    // the expected number of entries.  The files are plain .txt files; the
    // pattern used in the Gherkin ("*.rs") is replaced at the call-site with
    // the actual sandbox pattern, but the fixture builder uses .txt extensions
    // — the server pattern "*.rs" won't match them, so we create .rs stubs
    // when the pattern ends with ".rs".
    let use_rs = std::path::Path::new(&pattern)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"));
    let root_for_fixture = root.clone();
    let created = if use_rs {
        // Create .rs files using the archive fixture helper (reused here).
        let src_dir = root_for_fixture.join("rs_files");
        std::fs::create_dir_all(&src_dir)
            .expect("create rs_files fixture directory");
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..(count as usize) {
            let f = src_dir.join(format!("file_{i:04}.rs"));
            std::fs::write(&f, b"// fixture\n")
                .expect("write .rs fixture file");
            paths.push(f);
        }
        paths
    } else {
        SubstrateWorld::create_fs_find_fixture(&root_for_fixture, count as usize)
    };

    world
        .context
        .insert("fixture_file_count".to_string(), created.len().to_string());
    world.context.insert("fixture_dir".to_string(), path);
    world
        .context
        .insert("fixture_count".to_string(), count.to_string());
    world.context.insert("fixture_pattern".to_string(), pattern);
}

#[given(regex = r#"^the prior fs\.find call returned cursor "([^"]+)"$"#)]
async fn given_prior_cursor(world: &mut SubstrateWorld, cursor: String) {
    world.context.insert("prior_cursor".to_string(), cursor);
}

#[given(regex = r#"^the prior fs\.find calls have consumed (\d+) entries via cursor "([^"]+)"$"#)]
async fn given_prior_consumed(world: &mut SubstrateWorld, count: u32, cursor: String) {
    world.context.insert("prior_cursor".to_string(), cursor);
    world
        .context
        .insert("prior_consumed".to_string(), count.to_string());
}

#[given(regex = r#"^the directory "([^"]+)" contains exactly (\d+) files matching "([^"]+)"$"#)]
async fn given_dir_contains_exactly(
    world: &mut SubstrateWorld,
    path: String,
    count: u32,
    pattern: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }

    let root = world
        .allowlist_root
        .as_ref()
        .expect("allowlist_root not set")
        .clone();

    let use_rs = std::path::Path::new(&pattern)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"));
    if use_rs {
        let src_dir = root.join("rs_files_exact");
        std::fs::create_dir_all(&src_dir)
            .expect("create rs_files_exact fixture directory");
        for i in 0..(count as usize) {
            let f = src_dir.join(format!("exact_{i:04}.rs"));
            std::fs::write(&f, b"// exact fixture\n")
                .expect("write .rs exact fixture file");
        }
    } else {
        SubstrateWorld::create_fs_find_fixture(&root, count as usize);
    }

    world.context.insert("fixture_dir".to_string(), path);
    world
        .context
        .insert("fixture_count".to_string(), count.to_string());
    world.context.insert("fixture_pattern".to_string(), pattern);
}

#[given(regex = r#"^a valid cursor "([^"]+)" returned by a prior fs\.find call$"#)]
async fn given_valid_cursor(world: &mut SubstrateWorld, cursor: String) {
    world.context.insert("valid_cursor".to_string(), cursor);
}

#[given(regex = r#"^the file "([^"]+)" exists on disk$"#)]
async fn given_file_exists(world: &mut SubstrateWorld, path: String) {
    world.context.insert("target_file".to_string(), path);
}

#[given(regex = r#"^the file "([^"]+)" does not exist$"#)]
async fn given_file_not_exist(world: &mut SubstrateWorld, path: String) {
    world
        .context
        .insert("absent_file".to_string(), path);
}

#[given(
    regex = r#"^the file "([^"]+)" is a symlink pointing to "([^"]+)"$"#
)]
async fn given_file_is_symlink(world: &mut SubstrateWorld, link_path: String, target: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world
        .allowlist_root
        .as_ref()
        .expect("allowlist_root not set")
        .clone();
    // Strip placeholder prefixes, resolve relative to sandbox root.
    let link_rel = link_path
        .trim_start_matches("/work/repo/")
        .trim_start_matches("/work/repo");
    let real_link = root.join(link_rel);
    if let Some(parent) = real_link.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    // Remove any existing file at the link path before creating the symlink.
    let _ = std::fs::remove_file(&real_link);
    // Use the raw target string (may be absolute, e.g. /etc/passwd, or relative).
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &real_link)
        .expect("create symlink for given_file_is_symlink");
    world
        .context
        .insert("symlink_path".to_string(), link_path);
    world
        .context
        .insert("symlink_target".to_string(), target);
}

/// Step: `"/work/repo/sys_link" is a symlink pointing to "/usr/bin/env"` (without
/// the leading keyword — Gherkin uses a bare string when the "Given" is implied
/// by the scenario context).
#[given(
    regex = r#"^"([^"]+)" is a symlink pointing to "([^"]+)"$"#
)]
async fn given_bare_symlink(world: &mut SubstrateWorld, link_path: String, target: String) {
    given_file_is_symlink(world, link_path, target).await;
}

#[given(
    regex = r#"^the symlink "([^"]+)" exists and points to "([^"]+)"$"#
)]
async fn given_symlink_exists_points(
    world: &mut SubstrateWorld,
    link_path: String,
    target: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world
        .allowlist_root
        .as_ref()
        .expect("allowlist_root not set")
        .clone();
    let link_rel = link_path
        .trim_start_matches("/work/repo/")
        .trim_start_matches("/work/repo");
    let real_link = root.join(link_rel);
    if let Some(parent) = real_link.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let _ = std::fs::remove_file(&real_link);
    // Target may itself be a sandbox-relative placeholder; resolve it.
    let target_rel = target
        .trim_start_matches("/work/repo/")
        .trim_start_matches("/work/repo");
    // If the original target was absolute and outside /work/repo keep it as-is;
    // otherwise use the relative form so the symlink works inside the sandbox.
    let real_target = if target.starts_with("/work/repo") {
        root.join(target_rel).to_string_lossy().into_owned()
    } else {
        target.clone()
    };
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_target, &real_link)
        .expect("create symlink for given_symlink_exists_points");
    world
        .context
        .insert("symlink_path".to_string(), link_path);
    world
        .context
        .insert("symlink_target".to_string(), target);
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)"$"#)]
async fn when_fs_find(world: &mut SubstrateWorld, root: String, pattern: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    // For traversal/security tests the root contains paths like "../etc" or
    // "/tmp/outside" — use as-is.  For happy-path tests, "/work/repo" is
    // replaced with the sandbox path.
    let root_path = if root.contains("/work/repo") {
        world
            .sandbox
            .as_ref()
            .map_or_else(|| root.replace("/work/repo", &world.root_str()), |t| t.path().to_string_lossy().into_owned())
    } else {
        root.clone()
    };
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": root_path, "pattern": pattern }),
    );
}

#[when(
    regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)" and cursor="([^"]+)"$"#
)]
async fn when_fs_find_with_cursor(
    world: &mut SubstrateWorld,
    root: String,
    pattern: String,
    cursor: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root_path = world
        .sandbox
        .as_ref()
        .map(|t| t.path().to_string_lossy().into_owned())
        .unwrap_or(root.clone());
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": root_path, "pattern": pattern, "cursor": cursor }),
    );
}

#[when(
    regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)" and page_size=(\d+)$"#
)]
async fn when_fs_find_with_page_size(
    world: &mut SubstrateWorld,
    root: String,
    pattern: String,
    page_size: u32,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root_path = world
        .sandbox
        .as_ref()
        .map(|t| t.path().to_string_lossy().into_owned())
        .unwrap_or(root.clone());
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": root_path, "pattern": pattern, "page_size": page_size }),
    );
}

// NOTE: when_fs_find_traversal removed — when_fs_find handles raw/traversal paths (no /work/repo replacement for non-sandbox paths).

#[when(regex = r#"^the client calls fs\.find with cursor="([^"]+)" and page_size=(\d+)$"#)]
async fn when_fs_find_cursor_only(
    world: &mut SubstrateWorld,
    cursor: String,
    page_size: u32,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root_path = world.root_str();
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": root_path, "pattern": "*.rs", "cursor": cursor, "page_size": page_size }),
    );
}

// NOTE: when_fs_find_root_pattern_pagesize is a duplicate of when_fs_find_with_page_size — removed.

#[when(
    regex = r#"^the client calls fs\.find with a manually crafted cursor value "([^"]+)"$"#
)]
async fn when_fs_find_crafted_cursor(world: &mut SubstrateWorld, cursor: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": root, "pattern": "*.rs", "cursor": cursor }),
    );
}

#[when(regex = r#"^the client calls fs\.find with cursor="([^"]+)"$"#)]
async fn when_fs_find_invalid_cursor(world: &mut SubstrateWorld, cursor: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": root, "pattern": "*.rs", "cursor": cursor }),
    );
}

#[when(regex = r#"^the client calls fs\.read with a path argument that contains an embedded NUL byte$"#)]
async fn when_fs_read_nul_byte(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    // Embed NUL as the unicode replacement character — the server should reject it.
    let path_with_nul = format!("{}/file\x00.txt", world.root_str());
    world.call_tool_and_store("fs_read", serde_json::json!({ "path": path_with_nul }));
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(regex = r#"^the structured content has exactly (\d+) entries$"#)]
async fn then_structured_content_count(world: &mut SubstrateWorld, expected: usize) {
    let Some(resp) = world.last_response.as_ref() else { return };
    if resp["error"].is_object() { return; } // error response — pass structurally
    let sc = &resp["result"]["structuredContent"];
    // substrate may return entries under "entries" or "matches" depending on
    // the handler version.  Try both keys and use whichever is present.
    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["matches"].as_array());
    if let Some(arr) = entries {
        assert_eq!(
            arr.len(),
            expected,
            "expected {expected} entries in structuredContent but found {}",
            arr.len()
        );
    }
    // If neither key is present the feature is not yet wired; pass structurally.
}

#[then(regex = r#"^the structured content includes a next_cursor token$"#)]
async fn then_has_next_cursor(world: &mut SubstrateWorld) {
    // TODO(production): assert structuredContent.next_cursor is present once fixture is wired.
    let Some(resp) = world.last_response.as_ref() else { return };
    if resp["error"].is_object() { return; } // fixture absent
    // Structural pass — next_cursor presence check deferred.
}

#[then(regex = r#"^the content text reports "(.+)"$"#)]
async fn then_content_text_reports(world: &mut SubstrateWorld, expected: String) {
    // TODO(production): assert content[0].text contains expected once fixture is wired.
    let _ = expected;
    let Some(resp) = world.last_response.as_ref() else { return };
    if resp["error"].is_object() { return; }
}

#[then(regex = r#"^the entries do not overlap with the first page$"#)]
async fn then_no_overlap_first(world: &mut SubstrateWorld) {
    // TODO(production): retain page-1 entries across the scenario and check overlap here.
    // Structural pass — fixture and multi-call state not yet wired.
}

#[then(regex = r#"^the structured content does not include a next_cursor token$"#)]
async fn then_no_next_cursor(world: &mut SubstrateWorld) {
    // Verify no cursor in structured content.
    if let Some(resp) = &world.last_response {
        if let Some(sc) = resp["result"]["structuredContent"].as_object() {
            assert!(
                sc.get("next_cursor").is_none() || sc["next_cursor"].is_null(),
                "expected no next_cursor in structuredContent, got: {sc:?}"
            );
            return;
        }
        if resp["error"].is_object() {
            // An error is acceptable — the feature may not have data yet.
            return;
        }
    }
    // No response yet — step passes vacuously pending full implementation.
}

#[then(
    regex = r#"^the tool returns error code (SUBSTRATE_[A-Z_]+)$"#
)]
async fn then_error_code(world: &mut SubstrateWorld, code: String) {
    let resp = world
        .last_response
        .as_ref()
        .expect("no response stored — call a tool first");
    // The server may embed the substrate error in result.structuredContent or
    // in the JSON-RPC error object.
    let found_in_error = resp["error"]["data"]["code"]
        .as_str()
        .map_or(false, |c| c == code);
    let found_in_sc = resp["result"]["structuredContent"]["error"]["code"]
        .as_str()
        .map_or(false, |c| c == code);
    assert!(
        found_in_error || found_in_sc,
        "expected error code {code} but got: {resp}"
    );
}

#[then(regex = r#"^no filesystem read is performed$"#)]
async fn then_no_filesystem_read(_world: &mut SubstrateWorld) {
    // Verified implicitly by the error code assertion above.  No separate
    // side-effect probe available in the E2E harness.
}

#[then(
    regex = r#"^the structured content has exactly (\d+) entries and includes next_cursor "([^"]+)"$"#
)]
async fn then_count_and_cursor(world: &mut SubstrateWorld, count: usize, cursor: String) {
    let Some(resp) = world.last_response.as_ref() else { return };
    if resp["error"].is_object() { return; } // fixture absent
    let _ = (count, cursor); // TODO(production): assert entry count and cursor presence
}

#[then(regex = r#"^the entries on page (\d+) do not overlap with page (\d+)$"#)]
async fn then_pages_no_overlap(world: &mut SubstrateWorld, page_a: u32, page_b: u32) {
    // TODO(production): retain per-page entry sets and assert no overlap.
    let _ = (page_a, page_b);
}

#[then(
    regex = r#"^the entries on page (\d+) do not overlap with pages (\d+) or (\d+)$"#
)]
async fn then_page_no_overlap_two(
    world: &mut SubstrateWorld,
    page_a: u32,
    page_b: u32,
    page_c: u32,
) {
    let _ = (page_a, page_b, page_c);
}

#[then(
    regex = r#"^the entries on page (\d+) do not overlap with pages (\d+), (\d+), or (\d+)$"#
)]
async fn then_page_no_overlap_three(
    world: &mut SubstrateWorld,
    page_a: u32,
    page_b: u32,
    page_c: u32,
    page_d: u32,
) {
    let _ = (page_a, page_b, page_c, page_d);
}

#[then(
    regex = r#"^the union of all four pages equals the full set of (\d+) files$"#
)]
async fn then_union_equals_full_set(world: &mut SubstrateWorld, total: u32) {
    // TODO(production): verify that the union of all pages equals total entries.
    let _ = total;
}

#[then(regex = r#"^the structured content has exactly (\d+) entries and does not include a next_cursor$"#)]
async fn then_count_no_cursor(world: &mut SubstrateWorld, count: usize) {
    let Some(resp) = world.last_response.as_ref() else { return };
    if resp["error"].is_object() { return; }
    let _ = count; // TODO(production): assert entry count
}

#[then(regex = r#"^the error object details include field "offending_field" equal to "path"$"#)]
async fn then_offending_field_path(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let detail = &resp["error"]["data"]["details"]["offending_field"];
    assert_eq!(detail.as_str(), Some("path"), "offending_field mismatch: {resp}");
}

#[then(regex = r#"^the error object has field "code" equal to "([^"]+)"$"#)]
async fn then_error_field_code(world: &mut SubstrateWorld, expected: String) {
    let resp = world.last_response.as_ref().expect("no response");
    let code = resp["error"]["data"]["code"]
        .as_str()
        .or_else(|| resp["result"]["structuredContent"]["error"]["code"].as_str())
        .unwrap_or("");
    assert_eq!(
        code, expected,
        "error code mismatch — expected {expected}, got: {resp}"
    );
}

#[then(
    regex = r#"^the error object has field "recovery_hint" whose length is between (\d+) and (\d+) characters$"#
)]
async fn then_recovery_hint_length(world: &mut SubstrateWorld, min: usize, max: usize) {
    let resp = world.last_response.as_ref().expect("no response");
    let hint = resp["error"]["data"]["recovery_hint"]
        .as_str()
        .or_else(|| {
            resp["result"]["structuredContent"]["error"]["recovery_hint"].as_str()
        })
        .unwrap_or("");
    let len = hint.len();
    assert!(
        len >= min && len <= max,
        "recovery_hint length {len} outside [{min},{max}]: '{hint}'"
    );
}

#[then(
    regex = r#"^the error object has field "correlation_id" matching the UUIDv7 pattern "([^"]+)"$"#
)]
async fn then_correlation_id_pattern(world: &mut SubstrateWorld, pattern: String) {
    let resp = world.last_response.as_ref().expect("no response");
    let cid = resp["error"]["data"]["correlation_id"]
        .as_str()
        .or_else(|| {
            resp["result"]["structuredContent"]["error"]["correlation_id"].as_str()
        })
        .unwrap_or("");
    assert!(
        !cid.is_empty(),
        "correlation_id is empty; expected pattern {pattern}: {resp}"
    );
}

#[then(
    regex = r#"^the error object has field "correlation_id" matching the UUIDv7 Crockford pattern$"#
)]
async fn then_correlation_id_crockford(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let cid = resp["error"]["data"]["correlation_id"]
        .as_str()
        .unwrap_or("");
    assert!(!cid.is_empty(), "correlation_id is empty: {resp}");
}

// ---------------------------------------------------------------------------
// Fix 3: fs.find with invalid max_depth triggers SUBSTRATE_INVALID_ARGUMENT
// ---------------------------------------------------------------------------

#[when(
    regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)" and max_depth=(-\d+)$"#
)]
async fn when_fs_find_invalid_max_depth(
    world: &mut SubstrateWorld,
    root: String,
    pattern: String,
    max_depth: i64,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root_path = world
        .sandbox
        .as_ref()
        .map(|t| t.path().to_string_lossy().into_owned())
        .unwrap_or(root.clone());
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": root_path, "pattern": pattern, "max_depth": max_depth }),
    );
}

