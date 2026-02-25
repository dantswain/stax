mod common;

use common::{
    add_commit, create_branch_with_commit, create_test_repo, create_test_repo_with_remote, git,
};

// ── current_branch ───────────────────────────────────────────────────────────

#[test]
fn test_current_branch_is_main() {
    let (_dir, repo) = create_test_repo();
    assert_eq!(repo.current_branch().unwrap(), "main");
}

#[test]
fn test_current_branch_after_checkout() {
    let (dir, repo) = create_test_repo();
    create_branch_with_commit(dir.path(), "feature", "main");

    assert_eq!(repo.current_branch().unwrap(), "feature");
}

// ── get_branches ─────────────────────────────────────────────────────────────

#[test]
fn test_get_branches_initial() {
    let (_dir, repo) = create_test_repo();
    let branches = repo.get_branches().unwrap();
    assert_eq!(branches, vec!["main"]);
}

#[test]
fn test_get_branches_multiple() {
    let (dir, repo) = create_test_repo();
    create_branch_with_commit(dir.path(), "A", "main");
    create_branch_with_commit(dir.path(), "B", "main");

    let mut branches = repo.get_branches().unwrap();
    branches.sort();
    assert_eq!(branches, vec!["A", "B", "main"]);
}

// ── create_branch ────────────────────────────────────────────────────────────

#[test]
fn test_create_branch() {
    let (_dir, repo) = create_test_repo();
    repo.create_branch("feature", None).unwrap();

    let branches = repo.get_branches().unwrap();
    assert!(branches.contains(&"feature".to_string()));
}

#[test]
fn test_create_branch_from_ref() {
    let (dir, repo) = create_test_repo();
    create_branch_with_commit(dir.path(), "A", "main");

    // Create B from A's ref
    repo.create_branch("B", Some("refs/heads/A")).unwrap();

    // B should point to the same commit as A
    let a_hash = repo.get_commit_hash("refs/heads/A").unwrap();
    let b_hash = repo.get_commit_hash("refs/heads/B").unwrap();
    assert_eq!(a_hash, b_hash);
}

#[test]
fn test_create_branch_duplicate_fails() {
    let (_dir, repo) = create_test_repo();
    repo.create_branch("feature", None).unwrap();

    let result = repo.create_branch("feature", None);
    assert!(result.is_err(), "creating duplicate branch should fail");
}

// ── checkout_branch ──────────────────────────────────────────────────────────

#[test]
fn test_checkout_branch() {
    let (_dir, repo) = create_test_repo();
    repo.create_branch("feature", None).unwrap();

    repo.checkout_branch("feature").unwrap();
    assert_eq!(repo.current_branch().unwrap(), "feature");

    repo.checkout_branch("main").unwrap();
    assert_eq!(repo.current_branch().unwrap(), "main");
}

#[test]
fn test_checkout_nonexistent_branch_fails() {
    let (_dir, repo) = create_test_repo();
    let result = repo.checkout_branch("nonexistent");
    assert!(result.is_err());
}

// ── get_commit_hash ──────────────────────────────────────────────────────────

#[test]
fn test_get_commit_hash() {
    let (_dir, repo) = create_test_repo();
    let hash = repo.get_commit_hash("refs/heads/main").unwrap();

    // SHA-1 hash is 40 hex chars
    assert_eq!(hash.len(), 40);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_get_commit_hash_changes_after_commit() {
    let (dir, repo) = create_test_repo();
    let hash1 = repo.get_commit_hash("refs/heads/main").unwrap();

    add_commit(dir.path(), "new.txt", "new content");

    let hash2 = repo.get_commit_hash("refs/heads/main").unwrap();
    assert_ne!(hash1, hash2);
}

// ── get_merge_base ───────────────────────────────────────────────────────────

#[test]
fn test_get_merge_base_same_branch() {
    let (_dir, repo) = create_test_repo();
    let merge_base = repo.get_merge_base("main", "main").unwrap();
    let main_hash = repo.get_commit_hash("refs/heads/main").unwrap();
    assert_eq!(merge_base.to_string(), main_hash);
}

#[test]
fn test_get_merge_base_parent_child() {
    let (dir, repo) = create_test_repo();
    let main_hash = repo.get_commit_hash("refs/heads/main").unwrap();

    create_branch_with_commit(dir.path(), "feature", "main");

    let merge_base = repo.get_merge_base("main", "feature").unwrap();
    assert_eq!(merge_base.to_string(), main_hash);
}

#[test]
fn test_get_merge_base_diverged_branches() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();
    let fork_point = repo.get_commit_hash("refs/heads/main").unwrap();

    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "main");

    let merge_base = repo.get_merge_base("A", "B").unwrap();
    assert_eq!(merge_base.to_string(), fork_point);
}

// ── count_commits_between ────────────────────────────────────────────────────

#[test]
fn test_count_commits_between_linear() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main (1 commit) → A (1 more) → B (1 more)
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    assert_eq!(
        repo.count_commits_between("refs/heads/main", "refs/heads/A")
            .unwrap(),
        1
    );
    assert_eq!(
        repo.count_commits_between("refs/heads/main", "refs/heads/B")
            .unwrap(),
        2
    );
    assert_eq!(
        repo.count_commits_between("refs/heads/A", "refs/heads/B")
            .unwrap(),
        1
    );
}

#[test]
fn test_count_commits_between_same_ref() {
    let (_dir, repo) = create_test_repo();
    assert_eq!(
        repo.count_commits_between("refs/heads/main", "refs/heads/main")
            .unwrap(),
        0
    );
}

// ── is_clean ─────────────────────────────────────────────────────────────────

#[test]
fn test_is_clean_true() {
    let (_dir, repo) = create_test_repo();
    assert!(repo.is_clean().unwrap());
}

#[test]
fn test_is_clean_modified_file() {
    let (dir, repo) = create_test_repo();
    std::fs::write(dir.path().join("init.txt"), "modified").unwrap();
    assert!(!repo.is_clean().unwrap());
}

#[test]
fn test_is_clean_staged_file() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();
    std::fs::write(p.join("staged.txt"), "new").unwrap();
    git(p, &["add", "staged.txt"]);
    assert!(!repo.is_clean().unwrap());
}

#[test]
fn test_is_clean_ignores_untracked() {
    let (dir, repo) = create_test_repo();
    std::fs::write(dir.path().join("untracked.txt"), "ignored").unwrap();
    // Untracked files should not make the repo dirty
    assert!(repo.is_clean().unwrap());
}

// ── tracking / remote ────────────────────────────────────────────────────────

#[test]
fn test_track_branch() {
    let (dir, _bare, repo) = create_test_repo_with_remote();
    let p = dir.path();

    // Create and push a feature branch
    create_branch_with_commit(p, "feature", "main");
    git(p, &["push", "origin", "feature"]);

    // Set tracking
    repo.track_branch("feature").unwrap();

    // Verify upstream is set
    let upstream = git(p, &["config", "branch.feature.remote"]);
    assert_eq!(upstream, "origin");
    let merge_ref = git(p, &["config", "branch.feature.merge"]);
    assert_eq!(merge_ref, "refs/heads/feature");
}

#[test]
fn test_has_remote_branch() {
    let (dir, _bare, repo) = create_test_repo_with_remote();
    let p = dir.path();

    assert!(repo.has_remote_branch("main").unwrap());
    assert!(!repo.has_remote_branch("nonexistent").unwrap());

    // Push a new branch, then check
    create_branch_with_commit(p, "feature", "main");
    git(p, &["push", "origin", "feature"]);

    // Need to fetch so the remote ref is visible
    git(p, &["fetch", "origin"]);
    assert!(repo.has_remote_branch("feature").unwrap());
}

#[test]
fn test_ensure_tracking_branch() {
    let (dir, _bare, repo) = create_test_repo_with_remote();
    let p = dir.path();

    create_branch_with_commit(p, "feature", "main");
    git(p, &["push", "origin", "feature"]);
    git(p, &["fetch", "origin"]);

    repo.ensure_tracking_branch("feature").unwrap();

    let upstream = git(p, &["config", "branch.feature.remote"]);
    assert_eq!(upstream, "origin");
}

#[test]
fn test_ensure_tracking_branch_no_remote_fails() {
    let (_dir, _bare, repo) = create_test_repo_with_remote();

    // "no-remote" was never pushed, so ensure_tracking should fail
    let result = repo.ensure_tracking_branch("no-remote");
    assert!(result.is_err());
}

#[test]
fn test_get_remote_url() {
    let (_dir, bare, repo) = create_test_repo_with_remote();

    let url = repo.get_remote_url("origin");
    assert!(url.is_some());
    assert_eq!(url.unwrap(), bare.path().to_str().unwrap());
}

#[test]
fn test_get_remote_url_nonexistent() {
    let (_dir, repo) = create_test_repo();
    assert!(repo.get_remote_url("origin").is_none());
}
