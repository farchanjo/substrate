//! Step definitions for the text-processing bounded context.
//!
//! Covers features:
//!   text-search-happy-path-paginated, text-search-binary-file-skipped,
//!   text-search-catastrophic-regex.

#![allow(unused_variables)]

use cucumber::{given, then, when};

use crate::SubstrateWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given(
    regex = r#"^the directory "([^"]+)" contains text files with (\d+) lines matching "([^"]+)"$"#
)]
async fn given_dir_text_lines(
    world: &mut SubstrateWorld,
    path: String,
    count: u32,
    pattern: String,
) {
    world.context.insert("fixture_dir".to_string(), path);
    world
        .context
        .insert("fixture_line_count".to_string(), count.to_string());
    world.context.insert("fixture_pattern".to_string(), pattern);
}

#[given(regex = r#"^a prior text\.search call returned cursor "([^"]+)"$"#)]
async fn given_text_prior_cursor(world: &mut SubstrateWorld, cursor: String) {
    world.context.insert("prior_cursor".to_string(), cursor);
}

#[given(regex = r#"^prior calls have consumed (\d+) matches via cursor "([^"]+)"$"#)]
async fn given_text_prior_consumed(world: &mut SubstrateWorld, count: u32, cursor: String) {
    world.context.insert("prior_cursor".to_string(), cursor);
    world
        .context
        .insert("prior_consumed".to_string(), count.to_string());
}

#[given(
    regex = r#"^the file "([^"]+)" contains "([^"]+)" on line (\d+)$"#
)]
async fn given_file_contains_line(
    world: &mut SubstrateWorld,
    path: String,
    content: String,
    line: u32,
) {
    world.context.insert("fixture_file".to_string(), path);
    world
        .context
        .insert("fixture_line_content".to_string(), content);
    world
        .context
        .insert("fixture_line_number".to_string(), line.to_string());
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(
    regex = r#"^the client calls text\.search with root="([^"]+)" and pattern="([^"]+)"$"#
)]
async fn when_text_search(world: &mut SubstrateWorld, root: String, pattern: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root_path = world
        .sandbox
        .as_ref()
        .map(|t| t.path().to_string_lossy().into_owned())
        .unwrap_or(root.clone());
    world.call_tool_and_store(
        "text_search",
        serde_json::json!({ "root": root_path, "pattern": pattern }),
    );
}

#[when(
    regex = r#"^the client calls text\.search with root="([^"]+)" and pattern="([^"]+)" and cursor="([^"]+)"$"#
)]
async fn when_text_search_cursor(
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
        "text_search",
        serde_json::json!({ "root": root_path, "pattern": pattern, "cursor": cursor }),
    );
}

#[when(
    regex = r#"^the client calls text\.search with root="([^"]+)" and pattern="([^"]+)" and case_insensitive=(true|false)$"#
)]
async fn when_text_search_case_insensitive(
    world: &mut SubstrateWorld,
    root: String,
    pattern: String,
    case_insensitive: bool,
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
        "text_search",
        serde_json::json!({
            "root": root_path,
            "pattern": pattern,
            "case_insensitive": case_insensitive,
        }),
    );
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(regex = r#"^the structured content has exactly (\d+) match entries$"#)]
async fn then_match_entries_count(world: &mut SubstrateWorld, expected: usize) {
    unimplemented!(
        "step pending: text-search — match entry count {expected} requires populated fixture"
    );
}

#[then(
    regex = r#"^each entry contains fields: file_path, line_number, line_text$"#
)]
async fn then_match_entry_fields(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: text-search — match entry field check requires fixture data"
    );
}

#[then(
    regex = r#"^the \(file_path, line_number\) pairs do not overlap with the first page$"#
)]
async fn then_text_no_overlap_first(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: text-search — cursor overlap check requires multi-call state"
    );
}

#[then(
    regex = r#"^at least one match entry has file_path="([^"]+)" and line_number=(\d+)$"#
)]
async fn then_match_at_path_line(
    world: &mut SubstrateWorld,
    file_path: String,
    line_number: u32,
) {
    unimplemented!(
        "step pending: text-search — specific file/line match check for '{file_path}:{line_number}'"
    );
}

#[then(
    regex = r#"^a match entry with file_path="([^"]+)" and line_number=(\d+) is returned$"#
)]
async fn then_match_entry_specific(
    world: &mut SubstrateWorld,
    file_path: String,
    line_number: u32,
) {
    unimplemented!(
        "step pending: text-search — case-insensitive match check for '{file_path}:{line_number}'"
    );
}
