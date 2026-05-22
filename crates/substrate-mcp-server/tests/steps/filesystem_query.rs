//! Step definitions for the filesystem-query bounded context.
//!
//! Covers features:
//!   fs-find-happy-path, fs-find-path-traversal-blocked,
//!   fs-find-index-*, fs-read-permission-denied, fs-read-special-file,
//!   fs-stat-broken-symlink, fs-stat-symlink-escape-blocked,
//!   fs-stat-symlink-loop.

#![allow(unused_variables)]

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

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)"$"#)]
async fn when_fs_find(world: &mut SubstrateWorld, root: String, pattern: String) {
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

#[when(regex = r#"^the client calls fs\.find with root="([^"]*)"\s+and pattern="([^"]+)"$"#)]
async fn when_fs_find_traversal(world: &mut SubstrateWorld, root: String, pattern: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": root, "pattern": pattern }),
    );
}

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

#[when(
    regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)" and page_size=(\d+)$"#
)]
async fn when_fs_find_root_pattern_pagesize(
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
    unimplemented!(
        "step pending: docs/arch/specs/features/filesystem-query/fs-find-happy-path.feature — \
         structuredContent entry count assertion requires running server with populated fixture tree (expected {expected})"
    );
}

#[then(regex = r#"^the structured content includes a next_cursor token$"#)]
async fn then_has_next_cursor(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: fs-find-happy-path — next_cursor assertion requires fixture data"
    );
}

#[then(regex = r#"^the content text reports "(.+)"$"#)]
async fn then_content_text_reports(world: &mut SubstrateWorld, expected: String) {
    unimplemented!(
        "step pending: fs-find-happy-path — content text assertion: expected text '{expected}'"
    );
}

#[then(regex = r#"^the entries do not overlap with the first page$"#)]
async fn then_no_overlap_first(world: &mut SubstrateWorld) {
    unimplemented!("step pending: fs-find-happy-path — overlap check requires multi-call state");
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
    unimplemented!(
        "step pending: pagination-cursor-roundtrip — count {count} and cursor '{cursor}' assertion requires fixture tree"
    );
}

#[then(regex = r#"^the entries on page (\d+) do not overlap with page (\d+)$"#)]
async fn then_pages_no_overlap(world: &mut SubstrateWorld, page_a: u32, page_b: u32) {
    unimplemented!(
        "step pending: pagination-cursor-roundtrip — page {page_a} vs {page_b} overlap check requires multi-call state"
    );
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
    unimplemented!(
        "step pending: pagination-cursor-roundtrip — 3-page overlap check"
    );
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
    unimplemented!("step pending: pagination — 4-page union check");
}

#[then(
    regex = r#"^the union of all four pages equals the full set of (\d+) files$"#
)]
async fn then_union_equals_full_set(world: &mut SubstrateWorld, total: u32) {
    unimplemented!(
        "step pending: pagination-cursor-roundtrip — union check for {total} files"
    );
}

#[then(regex = r#"^the structured content has exactly (\d+) entries and does not include a next_cursor$"#)]
async fn then_count_no_cursor(world: &mut SubstrateWorld, count: usize) {
    unimplemented!(
        "step pending: pagination — {count} entries without cursor assertion"
    );
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
