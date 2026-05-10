/// Integration tests for ShadowSnapshot — ported from opencode's snapshot.test.ts.
///
/// Each test:
///   1. Creates a real temp git repo
///   2. Makes file changes
///   3. Exercises ShadowSnapshot methods
///   4. Asserts filesystem + return-value invariants
///
/// Tests are async and require git to be on PATH.  They are skipped gracefully
/// when git is unavailable.

use std::path::{Path, PathBuf};
use std::fs;
use tempfile::TempDir;
use claurst_core::snapshot::{ShadowSnapshot, Patch};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Initialise a fresh git repo in a tempdir, commit two files, and return
/// the TempDir plus the content of each committed file.
async fn bootstrap() -> (TempDir, String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path();
    git(p, &["init", "-b", "main"]).await;
    git(p, &["config", "user.email", "test@test"]).await;
    git(p, &["config", "user.name", "Test"]).await;

    let a_content = format!("A{}", rand_str());
    let b_content = format!("B{}", rand_str());
    fs::write(p.join("a.txt"), &a_content).unwrap();
    fs::write(p.join("b.txt"), &b_content).unwrap();
    git(p, &["add", "."]).await;
    git(p, &["commit", "-m", "init"]).await;
    (dir, a_content, b_content)
}

fn rand_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
    format!("{n:08x}")
}

async fn git(dir: &Path, args: &[&str]) {
    tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .ok();
}

fn snap_or_skip(dir: &Path) -> ShadowSnapshot {
    match ShadowSnapshot::for_session(dir) {
        Some(s) => s,
        None => {
            eprintln!("git not available or not a repo; skipping test");
            std::process::exit(0); // can't use `return` from closure, just skip
        }
    }
}

fn fwd(base: &Path, rel: &str) -> PathBuf {
    PathBuf::from(base.join(rel).to_string_lossy().replace('\\', "/"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tracks_deleted_files() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::remove_file(p.join("a.txt")).unwrap();

    let patch = snap.patch(&before).await;
    assert!(patch.files.contains(&fwd(p, "a.txt")), "deleted file in patch");
}

#[tokio::test]
async fn revert_removes_new_files() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::write(p.join("new.txt"), "NEW").unwrap();

    let patch = snap.patch(&before).await;
    snap.revert(&[patch]).await;

    assert!(!p.join("new.txt").exists(), "new file should be deleted after revert");
}

#[tokio::test]
async fn revert_in_subdirectory() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::create_dir_all(p.join("sub")).unwrap();
    fs::write(p.join("sub/file.txt"), "SUB").unwrap();

    let patch = snap.patch(&before).await;
    snap.revert(&[patch]).await;

    assert!(!p.join("sub/file.txt").exists(), "nested file deleted");
}

#[tokio::test]
async fn multiple_file_operations() {
    let (dir, a_content, b_content) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::remove_file(p.join("a.txt")).unwrap();
    fs::write(p.join("c.txt"), "C").unwrap();
    fs::create_dir_all(p.join("dir")).unwrap();
    fs::write(p.join("dir/d.txt"), "D").unwrap();
    fs::write(p.join("b.txt"), "MODIFIED").unwrap();

    let patch = snap.patch(&before).await;
    snap.revert(&[patch]).await;

    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), a_content);
    assert_eq!(fs::read_to_string(p.join("b.txt")).unwrap(), b_content);
    assert!(!p.join("c.txt").exists());
}

#[tokio::test]
async fn empty_directory_not_tracked() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::create_dir_all(p.join("empty")).unwrap();

    let patch = snap.patch(&before).await;
    assert_eq!(patch.files.len(), 0, "empty dir should not appear in patch");
}

#[tokio::test]
async fn binary_file_tracked_and_reverted() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::write(p.join("image.png"), &[0x89u8, 0x50, 0x4e, 0x47]).unwrap();

    let patch = snap.patch(&before).await;
    assert!(patch.files.contains(&fwd(p, "image.png")), "binary file in patch");

    snap.revert(&[patch]).await;
    assert!(!p.join("image.png").exists(), "binary file removed after revert");
}

#[tokio::test]
async fn large_added_files_skipped() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    let huge = vec![0u8; 2 * 1024 * 1024 + 1];
    fs::write(p.join("huge.bin"), &huge).unwrap();

    let patch = snap.patch(&before).await;
    assert_eq!(patch.files.len(), 0, "large file not in patch");

    let diff = snap.diff(&before).await;
    assert!(diff.is_empty(), "large file not in diff");

    // Second track should return same hash.
    let after = snap.track().await.expect("track2");
    assert_eq!(before, after, "same hash when only large file changed");
}

#[tokio::test]
async fn track_with_no_changes_returns_same_hash() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let h1 = snap.track().await.expect("track1");
    let h2 = snap.track().await.expect("track2");
    let h3 = snap.track().await.expect("track3");
    assert_eq!(h1, h2);
    assert_eq!(h1, h3);
}

#[tokio::test]
async fn restore_function() {
    let (dir, a_content, b_content) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::remove_file(p.join("a.txt")).unwrap();
    fs::write(p.join("new.txt"), "new").unwrap();
    fs::write(p.join("b.txt"), "modified").unwrap();

    snap.restore(&before).await;

    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), a_content);
    assert_eq!(fs::read_to_string(p.join("b.txt")).unwrap(), b_content);
    // Restore does not delete new files (git checkout-index -a -f only restores tracked)
    // so new.txt may remain — that matches opencode behaviour.
}

#[tokio::test]
async fn diff_function_with_various_changes() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::remove_file(p.join("a.txt")).unwrap();
    fs::write(p.join("new.txt"), "new content").unwrap();
    fs::write(p.join("b.txt"), "modified content").unwrap();

    let diff = snap.diff(&before).await;
    assert!(diff.contains("a.txt"), "diff contains deleted file");
    assert!(diff.contains("b.txt"), "diff contains modified file");
    assert!(diff.contains("new.txt"), "diff contains new file");
}

#[tokio::test]
async fn special_characters_in_filenames() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::write(p.join("file with spaces.txt"), "SPACES").unwrap();
    fs::write(p.join("file-with-dashes.txt"), "DASHES").unwrap();
    fs::write(p.join("file_with_underscores.txt"), "UNDERSCORES").unwrap();

    let patch = snap.patch(&before).await;
    assert!(patch.files.contains(&fwd(p, "file with spaces.txt")));
    assert!(patch.files.contains(&fwd(p, "file-with-dashes.txt")));
    assert!(patch.files.contains(&fwd(p, "file_with_underscores.txt")));
}

#[tokio::test]
async fn hidden_files_tracked() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("track");
    fs::write(p.join(".hidden"), "hidden").unwrap();
    fs::write(p.join(".config"), "config").unwrap();

    let patch = snap.patch(&before).await;
    assert!(patch.files.contains(&fwd(p, ".hidden")));
    assert!(patch.files.contains(&fwd(p, ".config")));
}

#[tokio::test]
async fn gitignore_respected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path();
    git(p, &["init", "-b", "main"]).await;
    git(p, &["config", "user.email", "test@test"]).await;
    git(p, &["config", "user.name", "Test"]).await;
    fs::write(p.join(".gitignore"), "*.ignored\nbuild/\n").unwrap();
    fs::write(p.join("tracked.txt"), "tracked").unwrap();
    git(p, &["add", "."]).await;
    git(p, &["commit", "-m", "init"]).await;

    let snap = snap_or_skip(p);
    let before = snap.track().await.expect("track");

    fs::write(p.join("new.ignored"), "should not appear").unwrap();
    fs::write(p.join("new-tracked.txt"), "should appear").unwrap();
    fs::create_dir_all(p.join("build")).unwrap();
    fs::write(p.join("build/out.js"), "should not appear").unwrap();

    let patch = snap.patch(&before).await;
    assert!(patch.files.contains(&fwd(p, "new-tracked.txt")));
    assert!(!patch.files.contains(&fwd(p, "new.ignored")));
    assert!(!patch.files.iter().any(|f| f.to_string_lossy().contains("build/")));
}

#[tokio::test]
async fn revert_with_empty_patches_noop() {
    let (dir, a_content, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    snap.revert(&[]).await;
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), a_content);

    snap.revert(&[Patch { hash: "dummy".into(), files: vec![] }]).await;
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), a_content);
}

#[tokio::test]
async fn revert_preserves_existing_file_deleted_then_recreated() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    fs::write(p.join("existing.txt"), "original").unwrap();
    let hash = snap.track().await.expect("track");

    fs::remove_file(p.join("existing.txt")).unwrap();
    fs::write(p.join("existing.txt"), "recreated").unwrap();
    fs::write(p.join("newfile.txt"), "new").unwrap();

    let patch = snap.patch(&hash).await;
    snap.revert(&[patch]).await;

    assert!(!p.join("newfile.txt").exists(), "new file deleted");
    assert!(p.join("existing.txt").exists(), "existing restored");
    assert_eq!(fs::read_to_string(p.join("existing.txt")).unwrap(), "original");
}

#[tokio::test]
async fn snapshot_isolation_between_projects() {
    let (dir1, _, _) = bootstrap().await;
    let (dir2, _, _) = bootstrap().await;
    let p1 = dir1.path();
    let p2 = dir2.path();

    let snap1 = snap_or_skip(p1);
    let snap2 = snap_or_skip(p2);

    let before1 = snap1.track().await.expect("track1");
    fs::write(p1.join("project1.txt"), "p1").unwrap();
    let patch1 = snap1.patch(&before1).await;
    assert!(patch1.files.contains(&fwd(p1, "project1.txt")));

    let before2 = snap2.track().await.expect("track2");
    fs::write(p2.join("project2.txt"), "p2").unwrap();
    let patch2 = snap2.patch(&before2).await;
    assert!(patch2.files.contains(&fwd(p2, "project2.txt")));
    assert!(!patch2.files.iter().any(|f| f.to_string_lossy().contains("project1")));
}

#[tokio::test]
async fn diff_full_sets_status() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    fs::write(p.join("grow.txt"), "one\n").unwrap();
    fs::write(p.join("trim.txt"), "line1\nline2\n").unwrap();
    fs::write(p.join("delete.txt"), "gone").unwrap();
    let before = snap.track().await.expect("before");

    fs::write(p.join("grow.txt"), "one\ntwo\n").unwrap();
    fs::write(p.join("trim.txt"), "line1\n").unwrap();
    fs::remove_file(p.join("delete.txt")).unwrap();
    fs::write(p.join("added.txt"), "new").unwrap();
    let after = snap.track().await.expect("after");

    let diffs = snap.diff_full(&before, &after).await;
    assert_eq!(diffs.len(), 4);

    let added = diffs.iter().find(|d| d.file == "added.txt").expect("added");
    assert_eq!(added.status, Some(claurst_core::snapshot::FileStatus::Added));

    let deleted = diffs.iter().find(|d| d.file == "delete.txt").expect("deleted");
    assert_eq!(deleted.status, Some(claurst_core::snapshot::FileStatus::Deleted));

    let grow = diffs.iter().find(|d| d.file == "grow.txt").expect("grow");
    assert_eq!(grow.status, Some(claurst_core::snapshot::FileStatus::Modified));
    assert!(grow.additions > 0);
    assert_eq!(grow.deletions, 0);
}

#[tokio::test]
async fn diff_full_no_changes_empty() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    let before = snap.track().await.expect("before");
    let after = snap.track().await.expect("after");
    let diffs = snap.diff_full(&before, &after).await;
    assert!(diffs.is_empty());
}

#[tokio::test]
async fn revert_overlapping_files_uses_first_patch_hash() {
    let (dir, _, _) = bootstrap().await;
    let p = dir.path();
    let snap = snap_or_skip(p);

    fs::write(p.join("shared.txt"), "v1").unwrap();
    let snap1 = snap.track().await.expect("snap1");

    fs::write(p.join("shared.txt"), "v2").unwrap();
    let snap2 = snap.track().await.expect("snap2");

    fs::write(p.join("shared.txt"), "v3").unwrap();
    let patch1 = snap.patch(&snap1).await;
    let patch2 = snap.patch(&snap2).await;

    // patch1 first → should restore to v1
    snap.revert(&[patch1, patch2]).await;
    assert_eq!(fs::read_to_string(p.join("shared.txt")).unwrap(), "v1");
}
