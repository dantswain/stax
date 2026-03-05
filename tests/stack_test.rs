mod common;

use common::{create_branch_with_commit, create_test_repo, git};
use std::collections::HashSet;

// ── Stack::analyze (without GitHub) ──────────────────────────────────────────

#[tokio::test]
async fn test_analyze_single_branch() {
    let (_dir, repo) = create_test_repo();

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    assert_eq!(stack.current_branch, "main");
    assert!(stack.branches.contains_key("main"));
    assert!(stack.roots.contains(&"main".to_string()));
}

#[tokio::test]
async fn test_analyze_linear_stack() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    // A's parent should be main
    let a = stack.branches.get("A").unwrap();
    assert_eq!(a.parent.as_deref(), Some("main"));

    // B's parent should be A
    let b = stack.branches.get("B").unwrap();
    assert_eq!(b.parent.as_deref(), Some("A"));

    // main should have A as child
    let main = stack.branches.get("main").unwrap();
    assert!(main.children.contains(&"A".to_string()));
}

#[tokio::test]
async fn test_analyze_branching_from_main() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A, main → B (siblings)
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    let a = stack.branches.get("A").unwrap();
    assert_eq!(a.parent.as_deref(), Some("main"));

    let b = stack.branches.get("B").unwrap();
    assert_eq!(b.parent.as_deref(), Some("main"));

    let main = stack.branches.get("main").unwrap();
    assert!(main.children.contains(&"A".to_string()));
    assert!(main.children.contains(&"B".to_string()));
}

#[tokio::test]
async fn test_analyze_deep_stack() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B → C → D
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");
    create_branch_with_commit(p, "D", "C");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

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
    assert_eq!(
        stack.branches.get("D").unwrap().parent.as_deref(),
        Some("C")
    );
}

#[tokio::test]
async fn test_analyze_current_branch_tracked() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    create_branch_with_commit(p, "feature", "main");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    assert_eq!(stack.current_branch, "feature");

    let feature = stack.branches.get("feature").unwrap();
    assert!(feature.is_current);

    let main = stack.branches.get("main").unwrap();
    assert!(!main.is_current);
}

// ── get_stack_for_branch (with real repo) ────────────────────────────────────

#[tokio::test]
async fn test_get_stack_for_branch_includes_ancestors_and_descendants() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B → C
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    // Get stack from B's perspective — should include full chain
    let branch_stack = stack.get_stack_for_branch("B");
    let names: Vec<&str> = branch_stack.iter().map(|b| b.name.as_str()).collect();

    assert!(names.contains(&"main"), "should include root ancestor");
    assert!(names.contains(&"A"), "should include parent");
    assert!(names.contains(&"B"), "should include the branch itself");
    assert!(names.contains(&"C"), "should include child");
}

#[tokio::test]
async fn test_get_stack_for_branch_nonexistent() {
    let (_dir, repo) = create_test_repo();

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();
    let result = stack.get_stack_for_branch("nonexistent");
    assert!(result.is_empty());
}

// ── mixed topology ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_analyze_diamond_topology() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → C
    //       ↘ B (also from main)
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main");
    create_branch_with_commit(p, "C", "A");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    assert_eq!(
        stack.branches.get("A").unwrap().parent.as_deref(),
        Some("main")
    );
    assert_eq!(
        stack.branches.get("B").unwrap().parent.as_deref(),
        Some("main")
    );
    assert_eq!(
        stack.branches.get("C").unwrap().parent.as_deref(),
        Some("A")
    );
}

#[tokio::test]
async fn test_analyze_no_prs_without_github() {
    let (dir, repo) = create_test_repo();
    create_branch_with_commit(dir.path(), "feature", "main");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    // Without GitHub client, no PRs should be attached
    for branch in stack.branches.values() {
        assert!(branch.pull_request.is_none());
    }
}

// ── main branch detection ────────────────────────────────────────────────────

#[tokio::test]
async fn test_main_is_root() {
    let (dir, repo) = create_test_repo();
    create_branch_with_commit(dir.path(), "feature", "main");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    let main = stack.branches.get("main").unwrap();
    assert!(main.parent.is_none(), "main should have no parent");
    assert!(stack.roots.contains(&"main".to_string()));
}

#[tokio::test]
async fn test_orphan_branch_falls_back_to_main() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // Create a branch off main with a commit, then add a commit to main
    // so the branch isn't a direct child but still shares merge-base
    create_branch_with_commit(p, "feature", "main");
    git(p, &["checkout", "main"]);

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    let feature = stack.branches.get("feature").unwrap();
    assert_eq!(
        feature.parent.as_deref(),
        Some("main"),
        "branch with no closer parent should fall back to main"
    );
}

// ── PR base_ref overrides merge-base parent (critical for stacked PRs) ───────
//
// After a rebase the merge-base heuristic often puts every branch directly
// under main.  The PR base_ref is the source of truth and must override
// the merge-base parent so the stack renders correctly.

#[tokio::test]
async fn test_pr_base_ref_overrides_merge_base_parent() {
    let (dir, _repo) = create_test_repo();
    let p = dir.path();

    // Build main → A → B → C (linear stack)
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    // Simulate a rebase that makes all branches look like direct children of
    // main from a merge-base perspective: rebase A onto main's tip, then B
    // onto A, etc.  After this the merge-base heuristic correctly links them,
    // but to simulate the broken case we manually set up a parent_map where
    // the merge-base puts B and C under main (which happens in large repos
    // after rebase --onto when the merge-base shifts).
    git(p, &["checkout", "A"]);
    let repo = stax::git::GitRepo::open(p).unwrap();

    let branches = repo.get_branches().unwrap();
    let mut commits = std::collections::HashMap::new();
    for b in &branches {
        if let Ok(hash) = repo.get_commit_hash(&format!("refs/heads/{b}")) {
            commits.insert(b.clone(), hash);
        }
    }
    let merged = std::collections::HashSet::new();

    // Intentionally wrong parent_map (simulates merge-base after rebase)
    let mut parent_map: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    parent_map.insert("main".to_string(), None);
    parent_map.insert("A".to_string(), Some("main".to_string()));
    parent_map.insert("B".to_string(), Some("main".to_string())); // WRONG — should be A
    parent_map.insert("C".to_string(), Some("main".to_string())); // WRONG — should be B

    // Build stack WITHOUT PR overrides — all three branches are under main
    let stack_no_prs = stax::stack::Stack::from_parent_map(
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

    assert_eq!(
        stack_no_prs.branches.get("B").unwrap().parent.as_deref(),
        Some("main"),
        "without PR overrides, merge-base parent should be used"
    );
    assert_eq!(
        stack_no_prs.branches.get("C").unwrap().parent.as_deref(),
        Some("main"),
        "without PR overrides, merge-base parent should be used"
    );

    // Now apply PR base_ref overrides (simulates cached PR data)
    parent_map.insert("B".to_string(), Some("A".to_string())); // PR says B → A
    parent_map.insert("C".to_string(), Some("B".to_string())); // PR says C → B

    let stack_with_prs = stax::stack::Stack::from_parent_map(
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

    // With PR overrides the stack should be correctly linked: main → A → B → C
    assert_eq!(
        stack_with_prs.branches.get("A").unwrap().parent.as_deref(),
        Some("main"),
    );
    assert_eq!(
        stack_with_prs.branches.get("B").unwrap().parent.as_deref(),
        Some("A"),
        "PR base_ref must override merge-base parent"
    );
    assert_eq!(
        stack_with_prs.branches.get("C").unwrap().parent.as_deref(),
        Some("B"),
        "PR base_ref must override merge-base parent"
    );

    // Children should reflect the corrected parents
    let main_children = &stack_with_prs.branches.get("main").unwrap().children;
    assert!(
        main_children.contains(&"A".to_string()),
        "main should have A as child"
    );
    assert!(
        !main_children.contains(&"B".to_string()),
        "main should NOT have B as child when PR says B → A"
    );
    assert!(
        !main_children.contains(&"C".to_string()),
        "main should NOT have C as child when PR says C → B"
    );

    let a_children = &stack_with_prs.branches.get("A").unwrap().children;
    assert!(
        a_children.contains(&"B".to_string()),
        "A should have B as child"
    );

    let b_children = &stack_with_prs.branches.get("B").unwrap().children;
    assert!(
        b_children.contains(&"C".to_string()),
        "B should have C as child"
    );
}

#[tokio::test]
async fn test_cached_prs_override_parent_map_for_full_stack_scope() {
    // Tests the full flow: when get_stack_for_branch is called on the bottom
    // branch, the entire chain should be included — not just the current
    // branch + main (which is what happens when merge-base is wrong).
    let (dir, _repo) = create_test_repo();
    let p = dir.path();

    // main → A → B → C → D → E
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");
    create_branch_with_commit(p, "D", "C");
    create_branch_with_commit(p, "E", "D");
    git(p, &["checkout", "A"]);

    let repo = stax::git::GitRepo::open(p).unwrap();
    let branches = repo.get_branches().unwrap();
    let mut commits = std::collections::HashMap::new();
    for b in &branches {
        if let Ok(hash) = repo.get_commit_hash(&format!("refs/heads/{b}")) {
            commits.insert(b.clone(), hash);
        }
    }
    let merged = std::collections::HashSet::new();

    // Parent map with PR-corrected chain
    let mut parent_map: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    parent_map.insert("main".to_string(), None);
    parent_map.insert("A".to_string(), Some("main".to_string()));
    parent_map.insert("B".to_string(), Some("A".to_string()));
    parent_map.insert("C".to_string(), Some("B".to_string()));
    parent_map.insert("D".to_string(), Some("C".to_string()));
    parent_map.insert("E".to_string(), Some("D".to_string()));

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

    // get_stack_for_branch from A should include the entire chain
    let scope = stack.get_stack_for_branch("A");
    let names: Vec<&str> = scope.iter().map(|b| b.name.as_str()).collect();

    assert!(names.contains(&"main"), "scope should include main");
    assert!(names.contains(&"A"), "scope should include A");
    assert!(names.contains(&"B"), "scope should include B");
    assert!(names.contains(&"C"), "scope should include C");
    assert!(names.contains(&"D"), "scope should include D");
    assert!(names.contains(&"E"), "scope should include E");
    assert_eq!(names.len(), 6, "scope should contain exactly 6 branches");
}

// ── from_parent_map matches analyze_for_branch ──────────────────────────────

#[tokio::test]
async fn test_from_parent_map_matches_analyze_for_branch() {
    let (dir, _repo) = create_test_repo();
    let p = dir.path();

    // Build a non-trivial topology: main → A → B → C, main → D
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");
    create_branch_with_commit(p, "D", "main");
    git(p, &["checkout", "B"]); // current branch = B

    // Reopen repo after checkout
    let repo = stax::git::GitRepo::open(p).unwrap();

    // Get the reference result from analyze_for_branch
    let expected = stax::stack::Stack::analyze_for_branch(&repo, "B", None)
        .await
        .unwrap();

    // Build the same via the cache path
    let (branches, commits, merged, parent_map) =
        stax::commands::navigate::get_branches_and_parent_map(&repo).unwrap();
    let actual = stax::stack::Stack::from_parent_map(
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

    // Compare: same current branch
    assert_eq!(actual.current_branch, expected.current_branch);

    // Compare: same set of branch names discovered
    let actual_names: HashSet<&String> = actual.branches.keys().collect();
    let expected_names: HashSet<&String> = expected.branches.keys().collect();
    assert_eq!(actual_names, expected_names, "discovered branches differ");

    // Compare: same parent for each branch
    for (name, actual_branch) in &actual.branches {
        let expected_branch = expected.branches.get(name).unwrap();
        assert_eq!(
            actual_branch.parent, expected_branch.parent,
            "parent mismatch for branch '{name}'"
        );
    }

    // Compare: same children for each branch (sorted for determinism)
    for (name, actual_branch) in &actual.branches {
        let expected_branch = expected.branches.get(name).unwrap();
        let mut actual_children = actual_branch.children.clone();
        let mut expected_children = expected_branch.children.clone();
        actual_children.sort();
        expected_children.sort();
        assert_eq!(
            actual_children, expected_children,
            "children mismatch for branch '{name}'"
        );
    }

    // Compare: same roots (sorted)
    let mut actual_roots = actual.roots.clone();
    let mut expected_roots = expected.roots.clone();
    actual_roots.sort();
    expected_roots.sort();
    assert_eq!(actual_roots, expected_roots, "roots differ");
}
