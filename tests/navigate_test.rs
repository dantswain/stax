mod common;

use common::{add_commit, create_branch_with_commit, create_test_repo, git};
use stax::cache::{CachedPullRequest, StackCache};
use stax::commands::navigate::{
    build_commit_cache, build_parent_map, children_from_map, find_children, find_parent,
    get_branches_and_parent_map, get_branches_with_cache, is_active_stack, is_main_branch,
    root_children_from_map, walk_to_top,
};
use std::collections::{HashMap, HashSet};

// ── is_main_branch ───────────────────────────────────────────────────────────

#[test]
fn test_is_main_branch_recognizes_main() {
    assert!(is_main_branch("main"));
}

#[test]
fn test_is_main_branch_recognizes_master() {
    assert!(is_main_branch("master"));
}

#[test]
fn test_is_main_branch_recognizes_develop() {
    assert!(is_main_branch("develop"));
}

#[test]
fn test_is_main_branch_rejects_feature() {
    assert!(!is_main_branch("feature"));
    assert!(!is_main_branch("main-feature"));
    assert!(!is_main_branch("my-main"));
}

// ── find_parent: strict matching ─────────────────────────────────────────────

#[test]
fn test_find_parent_linear_stack() {
    // main → A → B — strict matching should detect parent chain
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    let parent_a = find_parent(&repo, "A", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent_a, "main", "A's parent should be main");

    let parent_b = find_parent(&repo, "B", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent_b, "A", "B's parent should be A");
}

#[test]
fn test_find_parent_picks_closest_ancestor() {
    // main → A → B → C
    // find_parent(C) should pick B (closest), not A or main
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    let parent_c = find_parent(&repo, "C", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent_c, "B", "C's parent should be B (closest ancestor)");
}

#[test]
fn test_find_parent_siblings_both_off_main() {
    // main → A, main → B (siblings — both should parent to main)
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    let parent_a = find_parent(&repo, "A", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent_a, "main");

    let parent_b = find_parent(&repo, "B", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent_b, "main");
}

#[test]
fn test_find_parent_falls_back_to_trunk() {
    // Single branch off main — should return main as parent
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "feature", "main");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    let parent = find_parent(&repo, "feature", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent, "main");
}

// ── find_parent: relaxed matching ────────────────────────────────────────────

#[test]
fn test_find_parent_relaxed_when_parent_merged_main() {
    // Simulate: main → A → B, then A merges main into itself.
    // After the merge, A's tip is a merge commit, so strict check
    // (merge_base(B, A) == A.tip) fails. Relaxed should still detect A.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // Create A off main with a commit
    create_branch_with_commit(p, "A", "main");
    // Create B off A with a commit
    create_branch_with_commit(p, "B", "A");

    // Now advance main so there's something to merge
    git(p, &["checkout", "main"]);
    add_commit(p, "main_extra.txt", "main extra content");

    // Merge main into A (simulates the real-world scenario where
    // someone merges the base branch into their feature branch)
    git(p, &["checkout", "A"]);
    git(p, &["merge", "main", "-m", "merge main into A"]);

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    // B's parent should still be A even though A merged main
    let parent_b = find_parent(&repo, "B", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(
        parent_b, "A",
        "B's parent should be A even after A merged main (relaxed match)"
    );
}

#[test]
fn test_find_parent_relaxed_deep_stack_with_merge() {
    // main → A → B → C, then A merges main.
    // Both B and C should still be correctly parented.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    // Advance main and merge into A
    git(p, &["checkout", "main"]);
    add_commit(p, "main_extra.txt", "main extra");
    git(p, &["checkout", "A"]);
    git(p, &["merge", "main", "-m", "merge main into A"]);

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    let parent_b = find_parent(&repo, "B", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent_b, "A", "B should still parent to A after merge");

    let parent_c = find_parent(&repo, "C", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent_c, "B", "C should still parent to B");
}

// ── find_parent: merged branch handling ──────────────────────────────────────

#[test]
fn test_find_parent_skips_merged_branches() {
    // main → A → B, then A is merged into main.
    // find_parent(B) should skip A (merged) and fall back to main.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    // Merge A into main (simulating PR merge)
    git(p, &["checkout", "main"]);
    git(p, &["merge", "A", "--no-ff", "-m", "merge A"]);

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    // A should be detected as merged
    assert!(merged.contains("A"), "A should be detected as merged");

    // B's parent should skip A (merged) and fall back to main
    let parent_b = find_parent(&repo, "B", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(
        parent_b, "main",
        "B should fall back to main since A is merged"
    );
}

// ── find_children ────────────────────────────────────────────────────────────

#[test]
fn test_find_children_linear_stack() {
    // main → A → B → C
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    // A's only child should be B (not C — that's a grandchild)
    let children_a = find_children(&repo, "A", &branches, &commits, &merged).unwrap();
    assert_eq!(children_a, vec!["B"], "A's direct child should be B only");

    let children_b = find_children(&repo, "B", &branches, &commits, &merged).unwrap();
    assert_eq!(children_b, vec!["C"], "B's direct child should be C only");

    let children_c = find_children(&repo, "C", &branches, &commits, &merged).unwrap();
    assert!(children_c.is_empty(), "C should have no children (leaf)");
}

#[test]
fn test_find_children_fork() {
    // main → A → B, A → C (fork at A)
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "A");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    let mut children_a = find_children(&repo, "A", &branches, &commits, &merged).unwrap();
    children_a.sort();
    assert_eq!(
        children_a,
        vec!["B", "C"],
        "A should have two children (fork)"
    );
}

#[test]
fn test_find_children_excludes_merged() {
    // main → A → B, merge A into main
    // find_children for main shouldn't include A (it's merged)
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    // Merge A into main
    git(p, &["checkout", "main"]);
    git(p, &["merge", "A", "--no-ff", "-m", "merge A"]);

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    assert!(merged.contains("A"), "A should be merged");

    // find_children on main should not include merged branch A
    let children = find_children(&repo, "main", &branches, &commits, &merged).unwrap();
    assert!(
        !children.contains(&"A".to_string()),
        "merged branch A should not appear as a child"
    );
}

// ── build_parent_map ─────────────────────────────────────────────────────────

#[test]
fn test_build_parent_map_linear_stack() {
    // main → A → B → C
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();
    let map = build_parent_map(&repo, &branches, &commits, &merged).unwrap();

    assert_eq!(
        map.get("main").unwrap(),
        &None,
        "main should have no parent"
    );
    assert_eq!(
        map.get("A").unwrap().as_deref(),
        Some("main"),
        "A's parent should be main"
    );
    assert_eq!(
        map.get("B").unwrap().as_deref(),
        Some("A"),
        "B's parent should be A"
    );
    assert_eq!(
        map.get("C").unwrap().as_deref(),
        Some("B"),
        "C's parent should be B"
    );
}

#[test]
fn test_build_parent_map_skips_merged_branches() {
    // main → A, then merge A into main.
    // Merged branch A should NOT appear in the parent map.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");

    git(p, &["checkout", "main"]);
    git(p, &["merge", "A", "--no-ff", "-m", "merge A"]);

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();
    assert!(merged.contains("A"), "A should be detected as merged");

    let map = build_parent_map(&repo, &branches, &commits, &merged).unwrap();
    assert!(
        !map.contains_key("A"),
        "merged branch A should not be in the parent map"
    );
    assert!(map.contains_key("main"), "main should always be in the map");
}

#[test]
fn test_build_parent_map_multiple_stacks() {
    // main → A → B, main → C (two separate stacks)
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "main");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();
    let map = build_parent_map(&repo, &branches, &commits, &merged).unwrap();

    assert_eq!(map.get("A").unwrap().as_deref(), Some("main"));
    assert_eq!(map.get("B").unwrap().as_deref(), Some("A"));
    assert_eq!(map.get("C").unwrap().as_deref(), Some("main"));
}

// ── children_from_map ────────────────────────────────────────────────────────

#[test]
fn test_children_from_map_basic() {
    // Construct a parent map: main → A → B, main → C
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));
    map.insert("C".to_string(), Some("main".to_string()));

    let merged = HashSet::new();

    let mut children_main = children_from_map("main", &map, &merged);
    children_main.sort();
    assert_eq!(children_main, vec!["A", "C"]);

    let children_a = children_from_map("A", &map, &merged);
    assert_eq!(children_a, vec!["B"]);

    let children_b = children_from_map("B", &map, &merged);
    assert!(children_b.is_empty());
}

#[test]
fn test_children_from_map_skips_merged() {
    // A is a child of main but merged — should be excluded
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("main".to_string()));

    let mut merged = HashSet::new();
    merged.insert("A".to_string());

    let children = children_from_map("main", &map, &merged);
    assert_eq!(
        children,
        vec!["B"],
        "merged branch A should be excluded from children"
    );
}

#[test]
fn test_children_from_map_skips_main_branches() {
    // If "develop" has parent "main", it should be excluded from children
    // because is_main_branch("develop") is true
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("develop".to_string(), Some("main".to_string()));
    map.insert("feature".to_string(), Some("main".to_string()));

    let merged = HashSet::new();

    let children = children_from_map("main", &map, &merged);
    assert_eq!(
        children,
        vec!["feature"],
        "develop should be excluded (is_main_branch)"
    );
}

// ── root_children_from_map ───────────────────────────────────────────────────

#[test]
fn test_root_children_from_map_basic() {
    // main → A → B, main → C
    // Root children of main: A, C
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));
    map.insert("C".to_string(), Some("main".to_string()));

    let merged = HashSet::new();

    let mut roots = root_children_from_map("main", &map, &merged);
    roots.sort();
    assert_eq!(roots, vec!["A", "C"]);
}

#[test]
fn test_root_children_from_map_skips_merged() {
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("main".to_string()));

    let mut merged = HashSet::new();
    merged.insert("A".to_string());

    let roots = root_children_from_map("main", &map, &merged);
    assert_eq!(roots, vec!["B"], "merged branch A should be excluded");
}

#[test]
fn test_root_children_from_map_includes_orphaned_by_merge() {
    // main → A → B, then A is merged into main.
    // B's parent in the map is A, and A is merged.
    // root_children_from_map should include B because its parent is merged
    // (it's effectively a new root).
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));

    let mut merged = HashSet::new();
    merged.insert("A".to_string());

    let roots = root_children_from_map("main", &map, &merged);
    assert!(
        roots.contains(&"B".to_string()),
        "B should be a root child since its parent A is merged"
    );
}

#[test]
fn test_root_children_from_map_empty_when_no_branches() {
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);

    let merged = HashSet::new();
    let roots = root_children_from_map("main", &map, &merged);
    assert!(roots.is_empty());
}

// ── walk_to_top ──────────────────────────────────────────────────────────────

#[test]
fn test_walk_to_top_linear() {
    // main → A → B → C
    // walk_to_top("A") should reach C
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));
    map.insert("C".to_string(), Some("B".to_string()));

    let merged = HashSet::new();

    let top = walk_to_top("A", &map, &merged);
    assert_eq!(top, "C", "should walk linear chain A → B → C");
}

#[test]
fn test_walk_to_top_single_branch() {
    // main → A (no children beyond A)
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));

    let merged = HashSet::new();

    let top = walk_to_top("A", &map, &merged);
    assert_eq!(top, "A", "single branch should return itself");
}

#[test]
fn test_walk_to_top_stops_at_fork() {
    // main → A → B, A → C (fork at A)
    // walk_to_top("A") should stop at A because it has 2 children
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));
    map.insert("C".to_string(), Some("A".to_string()));

    let merged = HashSet::new();

    let top = walk_to_top("A", &map, &merged);
    assert_eq!(top, "A", "should stop at fork (A has 2 children)");
}

#[test]
fn test_walk_to_top_skips_merged_children() {
    // main → A → B → C, but B is merged.
    // From A, B is in merged set so children_from_map skips it.
    // A should have no visible children, so walk stops at A.
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));
    map.insert("C".to_string(), Some("B".to_string()));

    let mut merged = HashSet::new();
    merged.insert("B".to_string());

    let top = walk_to_top("A", &map, &merged);
    // B is merged, so A has no visible children. C's parent is B which is merged,
    // but C itself is not merged and its parent B is not visible.
    // children_from_map("A") only finds B (parent=A), but B is merged so it's skipped.
    // C's parent is B, not A, so it's not a child of A.
    assert_eq!(
        top, "A",
        "should stop at A because its only child B is merged"
    );
}

#[test]
fn test_walk_to_top_from_middle_of_stack() {
    // main → A → B → C → D
    // walk_to_top("B") should reach D
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));
    map.insert("C".to_string(), Some("B".to_string()));
    map.insert("D".to_string(), Some("C".to_string()));

    let merged = HashSet::new();

    let top = walk_to_top("B", &map, &merged);
    assert_eq!(top, "D", "should walk B → C → D");
}

// ── is_active_stack ──────────────────────────────────────────────────────────

#[test]
fn test_is_active_stack_multi_branch_always_active() {
    // Multi-branch stack (root ≠ top) is always active regardless of PR data
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));
    let merged = HashSet::new();

    assert!(
        is_active_stack("A", "B", None, &map, &merged),
        "multi-branch stack should always be active"
    );

    // Even with empty open PR set
    let empty_prs: HashSet<String> = HashSet::new();
    assert!(
        is_active_stack("A", "B", Some(&empty_prs), &map, &merged),
        "multi-branch stack active even with no open PRs"
    );
}

#[test]
fn test_is_active_stack_single_branch_with_open_pr() {
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    let merged = HashSet::new();

    let mut open = HashSet::new();
    open.insert("A".to_string());

    assert!(
        is_active_stack("A", "A", Some(&open), &map, &merged),
        "single-branch with open PR should be active"
    );
}

#[test]
fn test_is_active_stack_single_branch_without_open_pr() {
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    let merged = HashSet::new();

    let open: HashSet<String> = HashSet::new();

    assert!(
        !is_active_stack("A", "A", Some(&open), &map, &merged),
        "single-branch with no open PR should be inactive"
    );
}

#[test]
fn test_is_active_stack_single_branch_no_pr_data() {
    // When GitHub is unavailable (no PR data), show everything
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    let merged = HashSet::new();

    assert!(
        is_active_stack("A", "A", None, &map, &merged),
        "single-branch with no PR data should default to active"
    );
}

#[test]
fn test_is_active_stack_fork_at_root() {
    // A has two children (fork) — even though root == top, the fork
    // means it's a meaningful branch point
    let mut map = HashMap::new();
    map.insert("main".to_string(), None);
    map.insert("A".to_string(), Some("main".to_string()));
    map.insert("B".to_string(), Some("A".to_string()));
    map.insert("C".to_string(), Some("A".to_string()));
    let merged = HashSet::new();

    let empty_prs: HashSet<String> = HashSet::new();
    assert!(
        is_active_stack("A", "A", Some(&empty_prs), &map, &merged),
        "fork at root should be active even with no open PRs"
    );
}

// ── get_branches_with_cache ──────────────────────────────────────────────────

#[test]
fn test_get_branches_with_cache_returns_all_branches() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();

    assert!(branches.contains(&"main".to_string()));
    assert!(branches.contains(&"A".to_string()));
    assert!(branches.contains(&"B".to_string()));

    // All branches should have commit hashes
    assert!(commits.contains_key("main"));
    assert!(commits.contains_key("A"));
    assert!(commits.contains_key("B"));

    // No branches are merged in this setup
    assert!(merged.is_empty());
}

#[test]
fn test_get_branches_with_cache_detects_merged() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");

    // Merge A into main
    git(p, &["checkout", "main"]);
    git(p, &["merge", "A", "--no-ff", "-m", "merge A"]);

    let (_branches, _commits, merged) = get_branches_with_cache(&repo).unwrap();

    assert!(merged.contains("A"), "A should be in the merged set");
    assert!(!merged.contains("main"), "main should not be merged");
}

#[test]
fn test_get_branches_with_cache_keeps_merged_in_branch_list() {
    // Merged branches should still appear in the branch list
    // (needed for find_parent to detect relationships through them)
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    git(p, &["checkout", "main"]);
    git(p, &["merge", "A", "--no-ff", "-m", "merge A"]);

    let (branches, _commits, merged) = get_branches_with_cache(&repo).unwrap();

    assert!(merged.contains("A"));
    assert!(
        branches.contains(&"A".to_string()),
        "merged branch A should still be in the branch list"
    );
}

// ── build_commit_cache ───────────────────────────────────────────────────────

#[test]
fn test_build_commit_cache_hashes_are_valid() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");

    let branches = repo.get_branches().unwrap();
    let cache = build_commit_cache(&repo, &branches).unwrap();

    // Hashes should be 40-character hex strings
    for (branch, hash) in &cache {
        assert_eq!(
            hash.len(),
            40,
            "commit hash for {branch} should be 40 chars"
        );
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "commit hash for {branch} should be hex"
        );
    }

    // Different branches with different commits should have different hashes
    assert_ne!(
        cache.get("main").unwrap(),
        cache.get("A").unwrap(),
        "main and A should have different commit hashes"
    );
}

// ── Integration: full pipeline ───────────────────────────────────────────────

#[test]
fn test_full_pipeline_linear_stack() {
    // End-to-end: main → A → B → C
    // Verify build_parent_map, children_from_map, walk_to_top all work together
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();
    let map = build_parent_map(&repo, &branches, &commits, &merged).unwrap();

    // Root children of main
    let roots = root_children_from_map("main", &map, &merged);
    assert_eq!(roots, vec!["A"]);

    // Walk A to top should reach C
    let top = walk_to_top("A", &map, &merged);
    assert_eq!(top, "C");

    // Children checks
    assert_eq!(children_from_map("A", &map, &merged), vec!["B"]);
    assert_eq!(children_from_map("B", &map, &merged), vec!["C"]);
    assert!(children_from_map("C", &map, &merged).is_empty());
}

#[test]
fn test_full_pipeline_two_stacks() {
    // main → A → B, main → C → D
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "main");
    create_branch_with_commit(p, "D", "C");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();
    let map = build_parent_map(&repo, &branches, &commits, &merged).unwrap();

    let mut roots = root_children_from_map("main", &map, &merged);
    roots.sort();
    assert_eq!(roots, vec!["A", "C"]);

    assert_eq!(walk_to_top("A", &map, &merged), "B");
    assert_eq!(walk_to_top("C", &map, &merged), "D");
}

#[test]
fn test_full_pipeline_with_merged_base() {
    // main → A → B → C, then merge A into main.
    // After merge: B and C should still form a navigable stack.
    // B's parent falls back to main. C's parent is still B.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    git(p, &["checkout", "main"]);
    git(p, &["merge", "A", "--no-ff", "-m", "merge A"]);

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();
    let map = build_parent_map(&repo, &branches, &commits, &merged).unwrap();

    assert!(merged.contains("A"));
    assert!(!map.contains_key("A"), "A is merged, not in map");

    // B's parent should be main (A is merged/skipped)
    assert_eq!(map.get("B").unwrap().as_deref(), Some("main"));
    // C's parent should be B
    assert_eq!(map.get("C").unwrap().as_deref(), Some("B"));

    // Root children: B (because A is merged, B's parent=main)
    let roots = root_children_from_map("main", &map, &merged);
    assert!(roots.contains(&"B".to_string()));

    // Walk from B should reach C
    assert_eq!(walk_to_top("B", &map, &merged), "C");
}

#[test]
fn test_full_pipeline_with_fork() {
    // main → A → B, A → C
    // walk_to_top("A") should stop at A (fork)
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "A");

    let (branches, commits, merged) = get_branches_with_cache(&repo).unwrap();
    let map = build_parent_map(&repo, &branches, &commits, &merged).unwrap();

    let roots = root_children_from_map("main", &map, &merged);
    assert_eq!(roots, vec!["A"]);

    // Fork at A — walk stops
    let top = walk_to_top("A", &map, &merged);
    assert_eq!(top, "A", "walk should stop at fork");

    // Both B and C are children of A
    let mut children = children_from_map("A", &map, &merged);
    children.sort();
    assert_eq!(children, vec!["B", "C"]);
}

// ── get_branches_and_parent_map (cache integration) ──────────────────────────

#[test]
fn test_cache_cold_writes_file() {
    // On first call with no cache, the function should write a cache file.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let cache_path = repo.git_dir().join("stax").join("cache.json");
    assert!(
        !cache_path.exists(),
        "cache should not exist before first call"
    );

    let (branches, _commits, merged, parent_map) = get_branches_and_parent_map(&repo).unwrap();

    assert!(
        cache_path.exists(),
        "cache should be written after first call"
    );
    assert!(branches.contains(&"main".to_string()));
    assert!(branches.contains(&"A".to_string()));
    assert!(branches.contains(&"B".to_string()));
    assert_eq!(parent_map.get("A").unwrap().as_deref(), Some("main"));
    assert_eq!(parent_map.get("B").unwrap().as_deref(), Some("A"));
    assert!(merged.is_empty());
}

#[test]
fn test_cache_warm_hit_returns_same_results() {
    // Call twice without changing anything — second call should be a cache hit
    // and return identical results.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    // First call: cold cache
    let (branches1, commits1, merged1, parent_map1) = get_branches_and_parent_map(&repo).unwrap();

    // Second call: should be a full cache hit
    let (branches2, commits2, merged2, parent_map2) = get_branches_and_parent_map(&repo).unwrap();

    assert_eq!(branches1, branches2);
    assert_eq!(commits1, commits2);
    assert_eq!(merged1, merged2);
    assert_eq!(parent_map1, parent_map2);

    // Verify the parent chain
    assert_eq!(parent_map2.get("A").unwrap().as_deref(), Some("main"));
    assert_eq!(parent_map2.get("B").unwrap().as_deref(), Some("A"));
    assert_eq!(parent_map2.get("C").unwrap().as_deref(), Some("B"));
}

#[test]
fn test_cache_partial_hit_stale_branch() {
    // First call populates cache, then amend a commit on a branch to change
    // its tip — second call should do a partial recompute.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    // First call: cold cache, writes cache file
    let (_b1, _c1, _m1, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(pm1.get("B").unwrap().as_deref(), Some("A"));

    // Amend B's commit to change its tip
    git(p, &["checkout", "B"]);
    add_commit(p, "B_extra.txt", "extra content on B");

    // Second call: partial cache hit (B is stale)
    let (_b2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();

    // B's parent should still be A after recompute
    assert_eq!(pm2.get("B").unwrap().as_deref(), Some("A"));
    assert_eq!(pm2.get("A").unwrap().as_deref(), Some("main"));
}

#[test]
fn test_cache_partial_hit_new_branch() {
    // First call populates cache, then create a new branch — second call
    // should detect the new branch and compute its parent.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");

    // First call: cold cache
    let (_b1, _c1, _m1, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert!(!pm1.contains_key("B"), "B doesn't exist yet");

    // Create a new branch B off A
    create_branch_with_commit(p, "B", "A");

    // Second call: partial hit (B is new)
    let (_b2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(pm2.get("B").unwrap().as_deref(), Some("A"));
}

#[test]
fn test_cache_handles_deleted_branch() {
    // First call populates cache with A and B, then delete B.
    // Second call should still work correctly.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    // First call: cold cache with A and B
    let (_b1, _c1, _m1, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert!(pm1.contains_key("B"));

    // Delete branch B
    git(p, &["checkout", "main"]);
    git(p, &["branch", "-D", "B"]);

    // Second call: B is in cache but deleted from git
    let (branches2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();
    assert!(!branches2.contains(&"B".to_string()));
    assert_eq!(pm2.get("A").unwrap().as_deref(), Some("main"));
}

#[test]
fn test_cache_trunk_tip_changed() {
    // When trunk (main) tip changes, merged set must be recomputed.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    // First call: cold cache
    let (_b1, _c1, merged1, _pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert!(merged1.is_empty());

    // Advance main (changes trunk tip, triggers trunk_changed)
    git(p, &["checkout", "main"]);
    add_commit(p, "main_extra.txt", "extra on main");

    // Second call: trunk changed
    let (_b2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();

    // Parent relationships should still be correct
    assert_eq!(pm2.get("A").unwrap().as_deref(), Some("main"));
    assert_eq!(pm2.get("B").unwrap().as_deref(), Some("A"));
}

#[test]
fn test_cache_corrupt_file_recovers() {
    // If cache file is corrupt, the function should fall back to full recompute.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");

    // Write a corrupt cache file
    let stax_dir = repo.git_dir().join("stax");
    std::fs::create_dir_all(&stax_dir).unwrap();
    std::fs::write(stax_dir.join("cache.json"), "not valid json {{{").unwrap();

    // Should gracefully degrade to full recompute
    let (_branches, _commits, _merged, parent_map) = get_branches_and_parent_map(&repo).unwrap();

    assert_eq!(parent_map.get("A").unwrap().as_deref(), Some("main"));

    // Cache should be repaired (overwritten with valid data)
    let mut cache = StackCache::new(&repo.git_dir());
    assert!(
        cache.load().is_some(),
        "cache should be valid after recovery"
    );
}

#[test]
fn test_cache_with_merged_branch() {
    // Verify cache works when there are merged branches.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    // Merge A into main
    git(p, &["checkout", "main"]);
    git(p, &["merge", "A", "--no-ff", "-m", "merge A"]);

    // First call: cold cache
    let (_b1, _c1, merged1, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert!(merged1.contains("A"), "A should be merged");
    assert!(
        !pm1.contains_key("A"),
        "merged A should not be in parent map"
    );
    assert_eq!(pm1.get("B").unwrap().as_deref(), Some("main"));

    // Second call: warm cache — should return same results
    let (_b2, _c2, merged2, pm2) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(merged1, merged2);
    assert_eq!(pm1, pm2);
}

#[test]
fn test_cache_stale_parent_triggers_child_recompute() {
    // If A's tip changes, B (whose cached parent is A) should also be
    // recomputed because its parent relationship may have changed.
    //
    // When A advances without restacking B, B's closest ancestor branch
    // becomes main (A's tip is no longer in B's ancestry). This is correct
    // behavior — B would need restacking to be back on top of A.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    // First call: cold cache
    let (_b1, _c1, _m1, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(pm1.get("B").unwrap().as_deref(), Some("A"));
    assert_eq!(pm1.get("C").unwrap().as_deref(), Some("B"));

    // Advance A (makes A stale, which should also trigger B recompute)
    git(p, &["checkout", "A"]);
    add_commit(p, "A_extra.txt", "extra commit on A");

    // Second call: partial hit — A is stale, B should be recomputed
    // because its cached parent (A) is stale.
    let (_b2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(pm2.get("A").unwrap().as_deref(), Some("main"));
    // B's parent changes to main because A's tip has moved past B's
    // branch point (A advanced without restacking B)
    assert_eq!(pm2.get("B").unwrap().as_deref(), Some("main"));
    assert_eq!(pm2.get("C").unwrap().as_deref(), Some("B"));
}

// ── PR base_ref overrides in get_branches_and_parent_map ─────────────────────
//
// These tests verify the critical behavior that PR base_ref overrides are
// applied by get_branches_and_parent_map(), ensuring all consumers (navigate
// commands, `stax stack`) see the same parent-child structure.

/// Helper: write PR data into the cache for the given branches.
fn write_pr_cache(repo: &stax::git::GitRepo, prs: Vec<(&str, &str, u64)>) {
    let mut cache = StackCache::new(&repo.git_dir());
    let mut pr_map = HashMap::new();
    for (head, base, number) in prs {
        pr_map.insert(
            head.to_string(),
            CachedPullRequest {
                number,
                state: "open".to_string(),
                head_ref: head.to_string(),
                base_ref: base.to_string(),
                html_url: format!("https://github.com/o/r/pull/{number}"),
                draft: false,
            },
        );
    }
    cache.save_pull_requests(&pr_map);
}

#[test]
fn test_pr_override_corrects_parent_in_navigate() {
    // After rebasing, merge-base may incorrectly assign B→main and C→main.
    // PR base_ref data (B→A, C→B) should override this so navigate commands
    // see the correct stack: main → A → B → C.
    //
    // This is the CRITICAL test for consistent behavior between `stax stack`
    // and navigate commands like `stax top`.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    // First call: populate the cache with correct merge-base parents
    let (_b, _c, _m, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(pm1.get("A").unwrap().as_deref(), Some("main"));
    assert_eq!(pm1.get("B").unwrap().as_deref(), Some("A"));
    assert_eq!(pm1.get("C").unwrap().as_deref(), Some("B"));

    // Now simulate what happens after rebase: advance A so that merge-base
    // of B and A no longer equals A's tip (B would fall back to main).
    git(p, &["checkout", "A"]);
    add_commit(p, "A_rebase.txt", "simulated rebase on A");

    // Without PR overrides, B and C would get wrong parents.
    // Write PR data that says B→A, C→B.
    write_pr_cache(&repo, vec![("B", "A", 10), ("C", "B", 11)]);

    // Second call: cache detects A as stale, recomputes B and C parents.
    // Merge-base would put B→main, but PR override corrects to B→A.
    let (_b2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(
        pm2.get("B").unwrap().as_deref(),
        Some("A"),
        "PR override should correct B's parent to A (not main)"
    );
    assert_eq!(
        pm2.get("C").unwrap().as_deref(),
        Some("B"),
        "PR override should keep C's parent as B"
    );
}

#[test]
fn test_pr_override_on_cache_full_hit() {
    // Even on a full cache hit (no stale branches), PR overrides should
    // be applied.  This tests the fast path.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main"); // B off main, not A

    // First call: cold cache.  Merge-base correctly identifies B→main.
    let (_b, _c, _m, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(pm1.get("B").unwrap().as_deref(), Some("main"));

    // Now write a PR that says B's base is A (user retargeted the PR).
    write_pr_cache(&repo, vec![("B", "A", 42)]);

    // Second call: full cache hit (no tips changed).
    // PR override should correct B→A.
    let (_b2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(
        pm2.get("B").unwrap().as_deref(),
        Some("A"),
        "PR override should correct B's parent to A on full cache hit"
    );
}

#[test]
fn test_pr_override_walk_to_top_sees_correct_chain() {
    // Verify that walk_to_top works correctly when PR overrides change
    // the parent chain.  This directly tests the `stax top` behavior.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main"); // B off main (not A)
    create_branch_with_commit(p, "C", "main"); // C off main (not B)

    // First call: merge-base puts all three off main.
    let (_b, _c, _m, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(pm1.get("A").unwrap().as_deref(), Some("main"));
    assert_eq!(pm1.get("B").unwrap().as_deref(), Some("main"));
    assert_eq!(pm1.get("C").unwrap().as_deref(), Some("main"));
    // walk_to_top from A would stop at A (no children in this structure)
    assert_eq!(walk_to_top("A", &pm1, &HashSet::new()), "A");

    // Write PR data that forms a chain: A→main, B→A, C→B
    write_pr_cache(&repo, vec![("A", "main", 1), ("B", "A", 2), ("C", "B", 3)]);

    // Second call: full cache hit with PR overrides.
    let (_b2, _c2, merged2, pm2) = get_branches_and_parent_map(&repo).unwrap();

    // Parent chain should now be: main → A → B → C
    assert_eq!(pm2.get("A").unwrap().as_deref(), Some("main"));
    assert_eq!(pm2.get("B").unwrap().as_deref(), Some("A"));
    assert_eq!(pm2.get("C").unwrap().as_deref(), Some("B"));

    // walk_to_top from A should now reach C
    assert_eq!(
        walk_to_top("A", &pm2, &merged2),
        "C",
        "walk_to_top should follow PR-corrected chain A → B → C"
    );

    // children_from_map should also reflect the corrected structure
    assert_eq!(children_from_map("A", &pm2, &merged2), vec!["B"]);
    assert_eq!(children_from_map("B", &pm2, &merged2), vec!["C"]);
    assert!(children_from_map("C", &pm2, &merged2).is_empty());

    // root_children should be just A
    assert_eq!(
        root_children_from_map("main", &pm2, &merged2),
        vec!["A"],
        "only A should be a root child of main after PR overrides"
    );
}

#[test]
fn test_pr_override_children_from_map_reflects_override() {
    // After PR overrides, children_from_map should return children
    // based on the corrected parents, not the merge-base parents.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main");

    // First call: both off main
    let (_b, _c, _m, _pm) = get_branches_and_parent_map(&repo).unwrap();

    // PR says B's parent is A
    write_pr_cache(&repo, vec![("B", "A", 10)]);

    let (_b2, _c2, merged, pm2) = get_branches_and_parent_map(&repo).unwrap();

    // A should have B as child
    assert_eq!(
        children_from_map("A", &pm2, &merged),
        vec!["B"],
        "A should have B as child after PR override"
    );
    // main should only have A as root child (B moved under A)
    assert_eq!(
        root_children_from_map("main", &pm2, &merged),
        vec!["A"],
        "main should only have A as root child after B moved under A"
    );
}

#[test]
fn test_pr_override_does_not_apply_for_unknown_base() {
    // If a PR's base_ref doesn't exist as a branch, the override should
    // be silently skipped (the branch was likely deleted).
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");

    // First call: populate cache
    let (_b, _c, _m, _pm) = get_branches_and_parent_map(&repo).unwrap();

    // Write a PR pointing to a non-existent base branch
    write_pr_cache(&repo, vec![("A", "nonexistent-branch", 99)]);

    let (_b2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();

    // A's parent should still be main (override skipped because base doesn't exist)
    assert_eq!(
        pm2.get("A").unwrap().as_deref(),
        Some("main"),
        "override should be skipped when base_ref doesn't exist as a branch"
    );
}

#[test]
fn test_pr_override_on_cache_miss_with_existing_pr_data() {
    // Even on a cache miss (e.g. corrupt cache file), if PR data somehow
    // exists, it should still apply.  In practice, a true miss means no
    // PR data, but this tests the code path robustness.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main");

    // Cold cache: no file exists. PR data can't be written without a cache
    // file, so this call populates the branch cache.
    let (_b, _c, _m, pm1) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(pm1.get("B").unwrap().as_deref(), Some("main"));

    // Write PR data
    write_pr_cache(&repo, vec![("B", "A", 5)]);

    // Corrupt the branch data so it looks like a miss, but PR data survives.
    // Actually, a real cache miss means the file doesn't exist or is corrupt,
    // so there's no PR data to apply.  Instead, test the partial hit path
    // by creating a new branch to trigger partial recompute.
    create_branch_with_commit(p, "C", "main");

    let (_b2, _c2, _m2, pm2) = get_branches_and_parent_map(&repo).unwrap();
    assert_eq!(
        pm2.get("B").unwrap().as_deref(),
        Some("A"),
        "PR override should apply even on partial cache hit"
    );
}

// ── Stack::from_parent_map consistency ─────────────────────────────────────
//
// These tests verify that Stack::from_parent_map (used by status, restack,
// sync) produces the same parent-child relationships as get_branches_and_parent_map
// (used by navigate commands). This ensures all commands see the same stack.

#[tokio::test]
async fn test_from_parent_map_uses_same_parents_as_navigate() {
    // Build a stack: main → A → B → C
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");
    git(p, &["checkout", "C"]);

    // Get parent map from navigate (the source of truth)
    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&repo).unwrap();

    // Build stack using from_parent_map (used by status, restack, sync)
    let stack = stax::stack::Stack::from_parent_map(
        &repo,
        "C",
        None,
        &branches,
        &commits,
        &merged,
        &parent_map,
    )
    .await
    .unwrap();

    // Verify stack matches parent map
    assert_eq!(
        stack.branches.get("A").unwrap().parent.as_deref(),
        Some("main")
    );
    assert_eq!(
        stack.branches.get("B").unwrap().parent.as_deref(),
        Some("A")
    );
    assert_eq!(
        stack.branches.get("C").unwrap().parent.as_deref(),
        Some("B")
    );

    // Verify children
    assert!(stack
        .branches
        .get("A")
        .unwrap()
        .children
        .contains(&"B".to_string()));
    assert!(stack
        .branches
        .get("B")
        .unwrap()
        .children
        .contains(&"C".to_string()));
}

#[tokio::test]
async fn test_from_parent_map_respects_pr_overrides() {
    // When PR data says B→A but merge-base says B→main,
    // Stack::from_parent_map should respect the PR override.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main"); // B is off main, not A
    git(p, &["checkout", "B"]);

    // Cold cache: populate it
    let _ = get_branches_and_parent_map(&repo).unwrap();

    // Write PR data: B→A
    write_pr_cache(&repo, vec![("B", "A", 1)]);

    // Now get the parent map with PR override applied
    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&repo).unwrap();

    // Navigate sees B→A (overridden by PR)
    assert_eq!(parent_map.get("B").unwrap().as_deref(), Some("A"));

    // Build stack from the same parent map — should see the same thing
    let stack = stax::stack::Stack::from_parent_map(
        &repo,
        "B",
        None,
        &branches,
        &commits,
        &merged,
        &parent_map,
    )
    .await
    .unwrap();

    assert_eq!(
        stack.branches.get("B").unwrap().parent.as_deref(),
        Some("A"),
        "Stack::from_parent_map should see PR-overridden parent"
    );
    assert!(
        stack
            .branches
            .get("A")
            .unwrap()
            .children
            .contains(&"B".to_string()),
        "A should list B as a child after PR override"
    );
}

#[tokio::test]
async fn test_from_parent_map_fork_topology() {
    // Verify fork topology: main → A → B, A → C
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "A");
    git(p, &["checkout", "A"]);

    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&repo).unwrap();

    let stack = stax::stack::Stack::from_parent_map(
        &repo,
        "A",
        None,
        &branches,
        &commits,
        &merged,
        &parent_map,
    )
    .await
    .unwrap();

    // A should have 2 children: B and C
    let a_children = &stack.branches.get("A").unwrap().children;
    assert!(a_children.contains(&"B".to_string()));
    assert!(a_children.contains(&"C".to_string()));
    assert_eq!(a_children.len(), 2);

    // B and C both have A as parent
    assert_eq!(
        stack.branches.get("B").unwrap().parent.as_deref(),
        Some("A")
    );
    assert_eq!(
        stack.branches.get("C").unwrap().parent.as_deref(),
        Some("A")
    );
}

#[test]
fn test_cache_updated_after_get_branches_and_parent_map() {
    // Verify the cache file is written after calling get_branches_and_parent_map,
    // which is what restack, sync, and status rely on for persistence.
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let cache_path = repo.git_dir().join("stax").join("cache.json");
    assert!(!cache_path.exists(), "cache should not exist yet");

    let _ = get_branches_and_parent_map(&repo).unwrap();

    assert!(
        cache_path.exists(),
        "cache should be written after first call"
    );

    // Verify cache content is valid
    let mut cache = StackCache::new(&repo.git_dir());
    let data = cache.load().expect("cache should be loadable");
    assert!(
        data.branches.contains_key("A"),
        "cache should contain branch A"
    );
    assert!(
        data.branches.contains_key("B"),
        "cache should contain branch B"
    );
    assert_eq!(
        data.branches.get("A").unwrap().parent.as_deref(),
        Some("main")
    );
    assert_eq!(data.branches.get("B").unwrap().parent.as_deref(), Some("A"));
}
