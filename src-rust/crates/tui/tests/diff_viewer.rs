//! T5-3: Diff viewer tests.

use claurst_tui::diff_viewer::{parse_unified_diff, DiffLineKind};

// ---------------------------------------------------------------------------
// parse_unified_diff
// ---------------------------------------------------------------------------

const SIMPLE_DIFF: &str = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,5 +1,6 @@
 fn main() {
-    println!("hello");
+    println!("hello, world");
+    println!("added line");
 }
"#;

#[test]
fn parse_diff_returns_hunks() {
    let files = parse_unified_diff(SIMPLE_DIFF);
    assert!(!files.is_empty(), "should have at least one file");
    let has_hunks: bool = files.iter().any(|f| !f.hunks.is_empty());
    assert!(has_hunks, "should have at least one hunk");
}

#[test]
fn parse_diff_has_added_lines() {
    let files = parse_unified_diff(SIMPLE_DIFF);
    let added: Vec<_> = files
        .iter()
        .flat_map(|f| f.hunks.iter())
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == DiffLineKind::Added)
        .collect();
    assert!(!added.is_empty(), "should have added lines");
    let texts: Vec<_> = added.iter().map(|l| l.content.as_str()).collect();
    assert!(texts.iter().any(|t| t.contains("hello, world")));
}

#[test]
fn parse_diff_has_removed_lines() {
    let files = parse_unified_diff(SIMPLE_DIFF);
    let removed: Vec<_> = files
        .iter()
        .flat_map(|f| f.hunks.iter())
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == DiffLineKind::Removed)
        .collect();
    assert!(!removed.is_empty(), "should have removed lines");
    let texts: Vec<_> = removed.iter().map(|l| l.content.as_str()).collect();
    assert!(texts.iter().any(|t| t.contains("hello")));
}

#[test]
fn parse_diff_has_context_lines() {
    let files = parse_unified_diff(SIMPLE_DIFF);
    let context: Vec<_> = files
        .iter()
        .flat_map(|f| f.hunks.iter())
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == DiffLineKind::Context)
        .collect();
    assert!(!context.is_empty(), "should have context lines");
}

#[test]
fn parse_empty_diff() {
    let files = parse_unified_diff("");
    assert!(files.is_empty());
}

#[test]
fn parse_diff_hunk_range() {
    let files = parse_unified_diff(SIMPLE_DIFF);
    assert!(!files.is_empty());
    let hunk = &files[0].hunks[0];
    // old_range.0 = old_start, new_range.0 = new_start
    // The @@ -1,5 +1,6 @@ header gives old_start=1, new_start=1
    // But the first line is the Header kind; real data starts in old_range/new_range
    assert!(hunk.old_range.0 > 0 || hunk.new_range.0 > 0);
}

#[test]
fn parse_multifile_diff() {
    let multi = r#"diff --git a/foo.rs b/foo.rs
--- a/foo.rs
+++ b/foo.rs
@@ -1,2 +1,2 @@
-old
+new
diff --git a/bar.rs b/bar.rs
--- a/bar.rs
+++ b/bar.rs
@@ -1,1 +1,1 @@
-x
+y
"#;
    let files = parse_unified_diff(multi);
    assert!(files.len() >= 2);
}

#[test]
fn parse_diff_stats_added_removed() {
    let files = parse_unified_diff(SIMPLE_DIFF);
    // SIMPLE_DIFF has 2 added lines and 1 removed line (not counting Header)
    let total_added: u32 = files.iter().map(|f| f.added).sum();
    let total_removed: u32 = files.iter().map(|f| f.removed).sum();
    assert!(total_added >= 1, "should have counted added lines");
    assert!(total_removed >= 1, "should have counted removed lines");
}
