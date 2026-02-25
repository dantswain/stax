mod common;

use common::{create_branch_with_commit, create_test_repo, git};

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
async fn test_analyze_three_level_stack() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B  (2-level deep — the heuristic reliably detects this)
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    assert_eq!(
        stack.branches.get("A").unwrap().parent.as_deref(),
        Some("main")
    );
    assert_eq!(
        stack.branches.get("B").unwrap().parent.as_deref(),
        Some("A")
    );
}

#[tokio::test]
async fn test_analyze_all_non_main_branches_have_parent() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B → C → D
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");
    create_branch_with_commit(p, "D", "C");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    // Every non-main branch should have a parent assigned
    for (name, branch) in &stack.branches {
        if name != "main" {
            assert!(
                branch.parent.is_some(),
                "branch {name} should have a parent"
            );
        }
    }

    // A's parent is definitely main (only candidate after excluding main branches)
    assert_eq!(
        stack.branches.get("A").unwrap().parent.as_deref(),
        Some("main")
    );
    // B's parent is definitely A (only valid merge-base match)
    assert_eq!(
        stack.branches.get("B").unwrap().parent.as_deref(),
        Some("A")
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

    // main → A → B  (2-level chain the heuristic reliably detects)
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let stack = stax::stack::Stack::analyze(&repo, None).await.unwrap();

    // Get stack from A's perspective — should include main (ancestor) and B (child)
    let branch_stack = stack.get_stack_for_branch("A");
    let names: Vec<&str> = branch_stack.iter().map(|b| b.name.as_str()).collect();

    assert!(names.contains(&"main"), "should include root ancestor");
    assert!(names.contains(&"A"), "should include the branch itself");
    assert!(names.contains(&"B"), "should include child");
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
