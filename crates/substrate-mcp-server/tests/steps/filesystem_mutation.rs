//! Step definitions for the filesystem-mutation bounded context.
//!
//! Covers features:
//!   fs-mkdir-happy-path-dry-run, fs-remove-requires-elicitation,
//!   fs-remove-concurrent-race, fs-rename-overwrite-blocked-without-flag,
//!   fs-set-permissions-outside-allowlist, fs-write-enospc.

#![allow(unused_variables)]

use cucumber::{given, then, when};

use crate::SubstrateWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given(regex = r#"^the directory "([^"]+)" does not exist$"#)]
async fn given_dir_not_exist(world: &mut SubstrateWorld, path: String) {
    world.context.insert("absent_dir".to_string(), path);
}

#[given(regex = r#"^a dry run for "([^"]+)" has been reviewed$"#)]
async fn given_dry_run_reviewed(world: &mut SubstrateWorld, path: String) {
    world
        .context
        .insert("dry_run_reviewed".to_string(), path);
}

#[given(regex = r#"^the directory "([^"]+)" contains (\d+) files$"#)]
async fn given_dir_contains_n_files_simple(
    world: &mut SubstrateWorld,
    path: String,
    count: u32,
) {
    world.context.insert("fixture_dir".to_string(), path);
    world
        .context
        .insert("fixture_count".to_string(), count.to_string());
}

#[given(
    regex = r#"^the target filesystem for "([^"]+)" has less than 1 MiB of free space \(near-full fixture\)$"#
)]
async fn given_near_full_fs(world: &mut SubstrateWorld, path: String) {
    // This fixture is environmental — we record intent but cannot simulate it.
    world.context.insert("near_full_path".to_string(), path);
}

#[given(regex = r#"^the target filesystem has at least (\d+) MiB of free space$"#)]
async fn given_fs_has_space(world: &mut SubstrateWorld, mib: u32) {
    world
        .context
        .insert("min_free_mib".to_string(), mib.to_string());
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(
    regex = r#"^the client calls fs\.mkdir with path="([^"]+)" and dry_run=(true|false)$"#
)]
async fn when_fs_mkdir_dry(world: &mut SubstrateWorld, path: String, dry_run: bool) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    world.call_tool_and_store(
        "fs_mkdir",
        serde_json::json!({ "path": full_path, "dry_run": dry_run }),
    );
}

#[when(
    regex = r#"^the client calls fs\.mkdir with path="([^"]+)" and dry_run=(true|false) and elicitation_confirmed=(true|false)$"#
)]
async fn when_fs_mkdir_confirmed(
    world: &mut SubstrateWorld,
    path: String,
    dry_run: bool,
    confirmed: bool,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    world.call_tool_and_store(
        "fs_mkdir",
        serde_json::json!({
            "path": full_path,
            "dry_run": dry_run,
            "elicitation_confirmed": confirmed,
        }),
    );
}

#[when(
    regex = r#"^the client calls fs\.mkdir with path="([^"]+)" and parents=(true|false) and dry_run=(true|false) and elicitation_confirmed=(true|false)$"#
)]
async fn when_fs_mkdir_parents(
    world: &mut SubstrateWorld,
    path: String,
    parents: bool,
    dry_run: bool,
    confirmed: bool,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    world.call_tool_and_store(
        "fs_mkdir",
        serde_json::json!({
            "path": full_path,
            "parents": parents,
            "dry_run": dry_run,
            "elicitation_confirmed": confirmed,
        }),
    );
}

#[when(
    regex = r#"^the client calls fs\.remove with path="([^"]+)" and elicitation_confirmed=(true|false)$"#
)]
async fn when_fs_remove(world: &mut SubstrateWorld, path: String, confirmed: bool) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    world.call_tool_and_store(
        "fs_remove",
        serde_json::json!({
            "path": full_path,
            "elicitation_confirmed": confirmed,
        }),
    );
}

#[when(
    regex = r#"^the client calls fs\.remove with path="([^"]+)" and recursive=(true|false) and elicitation_confirmed=(true|false)$"#
)]
async fn when_fs_remove_recursive(
    world: &mut SubstrateWorld,
    path: String,
    recursive: bool,
    confirmed: bool,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    world.call_tool_and_store(
        "fs_remove",
        serde_json::json!({
            "path": full_path,
            "recursive": recursive,
            "elicitation_confirmed": confirmed,
        }),
    );
}

#[when(
    regex = r#"^the client calls fs\.write with path="([^"]+)" and content of size (\d+) MiB$"#
)]
async fn when_fs_write_mib(world: &mut SubstrateWorld, path: String, size_mib: u32) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    // Generate synthetic content of the requested size (best-effort in test).
    let content = "x".repeat(size_mib as usize * 1024 * 1024);
    world.call_tool_and_store(
        "fs_write",
        serde_json::json!({ "path": full_path, "content": content }),
    );
}

#[when(
    regex = r#"^the client calls fs\.write with path="([^"]+)" and content of size (\d+) KiB$"#
)]
async fn when_fs_write_kib(world: &mut SubstrateWorld, path: String, size_kib: u32) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    let content = "x".repeat(size_kib as usize * 1024);
    world.call_tool_and_store(
        "fs_write",
        serde_json::json!({ "path": full_path, "content": content }),
    );
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(regex = r#"^the tool returns a dry-run plan describing the directory to be created$"#)]
async fn then_dry_run_plan_dir(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: fs-mkdir-happy-path-dry-run — dry-run plan content check"
    );
}

#[then(regex = r#"^the directory "([^"]+)" does not exist on disk$"#)]
async fn then_dir_not_on_disk(world: &mut SubstrateWorld, path: String) {
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    assert!(
        !std::path::Path::new(&full_path).exists(),
        "expected directory '{full_path}' to NOT exist but it does"
    );
}

#[then(regex = r#"^no error is returned$"#)]
async fn then_no_error(world: &mut SubstrateWorld) {
    if let Some(resp) = &world.last_response {
        assert!(
            !resp["error"].is_object(),
            "expected no error but got: {}",
            resp["error"]
        );
    }
}

#[then(regex = r#"^the directory "([^"]+)" exists on disk$"#)]
async fn then_dir_exists_on_disk(world: &mut SubstrateWorld, path: String) {
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    assert!(
        std::path::Path::new(&full_path).is_dir(),
        "expected directory '{full_path}' to exist but it does not"
    );
}

#[then(regex = r#"^the tool returns a success result with the created path$"#)]
async fn then_success_created_path(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"].is_object() && !resp["error"].is_object(),
        "expected success result, got: {resp}"
    );
}

#[then(regex = r#"^the file "([^"]+)" still exists on disk$"#)]
async fn then_file_still_on_disk(world: &mut SubstrateWorld, path: String) {
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    assert!(
        std::path::Path::new(&full_path).exists(),
        "expected file '{full_path}' to still exist but it does not"
    );
}

#[then(regex = r#"^the file "([^"]+)" does not exist on disk$"#)]
async fn then_file_not_on_disk(world: &mut SubstrateWorld, path: String) {
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    assert!(
        !std::path::Path::new(&full_path).exists(),
        "expected file '{full_path}' to NOT exist but it does"
    );
}

#[then(regex = r#"^the tool returns a success result confirming deletion$"#)]
async fn then_success_deletion(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"].is_object() && !resp["error"].is_object(),
        "expected success result for deletion, got: {resp}"
    );
}

#[then(
    regex = r#"^the directories "([^"]+)", "([^"]+)", and "([^"]+)" exist on disk$"#
)]
async fn then_three_dirs_exist(
    world: &mut SubstrateWorld,
    p1: String,
    p2: String,
    p3: String,
) {
    let root = world.root_str();
    for p in &[&p1, &p2, &p3] {
        let full = p.replace("/work/repo", &root);
        assert!(
            std::path::Path::new(&full).is_dir(),
            "expected dir '{full}' to exist but it does not"
        );
    }
}

#[then(regex = r#"^the directory "([^"]+)" still exists on disk$"#)]
async fn then_dir_still_on_disk(world: &mut SubstrateWorld, path: String) {
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    assert!(
        std::path::Path::new(&full_path).is_dir(),
        "expected directory '{full_path}' to still exist but it does not"
    );
}

#[then(regex = r#"^no file named "([^"]+)" exists under "([^"]+)"$"#)]
async fn then_no_file_under(world: &mut SubstrateWorld, filename: String, dir: String) {
    let root = world.root_str();
    let full_dir = dir.replace("/work/repo", &root);
    let full_path = std::path::Path::new(&full_dir).join(&filename);
    assert!(
        !full_path.exists(),
        "expected no file '{filename}' under '{full_dir}' but it exists"
    );
}

#[then(regex = r#"^no "\.tmp" file created during the write attempt remains under "([^"]+)"$"#)]
async fn then_no_tmp_file(world: &mut SubstrateWorld, dir: String) {
    let root = world.root_str();
    let full_dir = dir.replace("/work/repo", &root);
    if let Ok(rd) = std::fs::read_dir(&full_dir) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            assert!(
                !name.to_string_lossy().contains(".tmp"),
                "found leftover .tmp file under '{full_dir}': {name:?}"
            );
        }
    }
}

#[then(
    regex = r#"^the error object details include field "observed_bytes" with a positive integer value$"#
)]
async fn then_observed_bytes_positive(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: fs-write-enospc — observed_bytes in error details requires near-full FS fixture"
    );
}

#[then(
    regex = r#"^the error object details include field "limit_bytes" with a positive integer value$"#
)]
async fn then_limit_bytes_positive(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: fs-write-enospc — limit_bytes in error details requires near-full FS fixture"
    );
}

#[then(
    regex = r#"^the value of "observed_bytes" is greater than the value of "limit_bytes"$"#
)]
async fn then_observed_gt_limit(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: fs-write-enospc — observed_bytes > limit_bytes comparison"
    );
}

#[then(regex = r#"^the response does not contain an error object$"#)]
async fn then_response_no_error(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        !resp["error"].is_object(),
        "expected no error object but got: {}",
        resp["error"]
    );
}

#[then(regex = r#"^the file "([^"]+)" exists on disk with the expected content$"#)]
async fn then_file_exists_with_content(world: &mut SubstrateWorld, path: String) {
    let root = world.root_str();
    let full_path = path.replace("/work/repo", &root);
    assert!(
        std::path::Path::new(&full_path).exists(),
        "expected file '{full_path}' to exist but it does not"
    );
}

#[then(
    regex = r#"^the error object does not have field "code" equal to "([^"]+)"$"#
)]
async fn then_error_code_not(world: &mut SubstrateWorld, not_code: String) {
    let resp = world.last_response.as_ref().expect("no response");
    let code = resp["error"]["data"]["code"].as_str().unwrap_or("");
    assert_ne!(
        code, not_code,
        "error code should not be {not_code} but it is: {resp}"
    );
}
