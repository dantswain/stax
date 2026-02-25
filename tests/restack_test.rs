use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

fn create_test_repo() -> (TempDir, stax::git::GitRepo) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path();

    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "test@test.com"]);
    git(path, &["config", "user.name", "Test"]);
    add_commit(path, "init.txt", "initial");

    let repo = stax::git::GitRepo::open(path).expect("failed to open repo");
    (dir, repo)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} failed to execute: {e}"));
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        panic!("git {args:?} failed: {stderr}");
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn add_commit(dir: &Path, filename: &str, content: &str) {
    std::fs::write(dir.join(filename), content).expect("failed to write file");
    git(dir, &["add", filename]);
    git(dir, &["commit", "-m", &format!("add {filename}")]);
}

fn create_branch_with_commit(dir: &Path, branch: &str, parent: &str) {
    git(dir, &["checkout", parent]);
    git(dir, &["checkout", "-b", branch]);
    add_commit(dir, &format!("{branch}.txt"), &format!("{branch} content"));
}

fn commit_is_ancestor(dir: &Path, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(dir)
        .status()
        .expect("merge-base failed")
        .success()
}

// ── rebase_onto tests ────────────────────────────────────────────────────────

#[test]
fn test_rebase_onto_simple() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    // Add a new commit to A (B is now behind)
    git(p, &["checkout", "A"]);
    add_commit(p, "a_extra.txt", "extra on A");

    let a_tip = git(p, &["rev-parse", "A"]);

    // B should NOT contain A's new commit yet
    assert!(
        !commit_is_ancestor(p, &a_tip, "B"),
        "B should not yet contain A's new commit"
    );

    // Rebase B onto A
    repo.rebase_onto("B", "A").expect("rebase_onto failed");

    // Now B should contain A's new commit
    assert!(
        commit_is_ancestor(p, &a_tip, "B"),
        "after rebase B should contain A's new commit"
    );
}

#[test]
fn test_rebase_onto_noop() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B  (B is already based on A's tip)
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let b_hash_before = git(p, &["rev-parse", "B"]);

    repo.rebase_onto("B", "A")
        .expect("noop rebase should succeed");

    let b_hash_after = git(p, &["rev-parse", "B"]);
    assert_eq!(
        b_hash_before, b_hash_after,
        "noop rebase should not change B"
    );
}

#[test]
fn test_rebase_onto_conflict_aborts() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A, main → B  where both modify the same file
    create_branch_with_commit(p, "A", "main");
    // Overwrite A's file with specific content
    std::fs::write(p.join("shared.txt"), "A's version").unwrap();
    git(p, &["add", "shared.txt"]);
    git(p, &["commit", "-m", "A modifies shared"]);

    git(p, &["checkout", "main"]);
    git(p, &["checkout", "-b", "B"]);
    std::fs::write(p.join("shared.txt"), "B's version").unwrap();
    git(p, &["add", "shared.txt"]);
    git(p, &["commit", "-m", "B modifies shared"]);

    let b_hash_before = git(p, &["rev-parse", "B"]);

    // Should fail with conflict
    let result = repo.rebase_onto("B", "A");
    assert!(result.is_err(), "conflicting rebase should fail");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("failed due to conflicts"),
        "error message should mention conflicts"
    );

    // No leftover rebase state
    assert!(
        !p.join(".git/rebase-merge").exists(),
        "rebase should have been aborted cleanly"
    );

    // B should be unchanged
    let b_hash_after = git(p, &["rev-parse", "B"]);
    assert_eq!(
        b_hash_before, b_hash_after,
        "B should be unchanged after failed rebase"
    );
}

#[test]
fn test_rebase_full_stack() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B → C
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");
    create_branch_with_commit(p, "C", "B");

    // Add a new commit to A
    git(p, &["checkout", "A"]);
    add_commit(p, "a_new.txt", "new on A");
    let a_tip = git(p, &["rev-parse", "A"]);

    // Rebase the whole chain: B onto A, then C onto B
    repo.rebase_onto("B", "A").expect("rebase B onto A failed");
    repo.rebase_onto("C", "B").expect("rebase C onto B failed");

    // Verify the chain: A's new commit should be in B and C
    assert!(
        commit_is_ancestor(p, &a_tip, "B"),
        "B should contain A's new commit"
    );
    assert!(
        commit_is_ancestor(p, &a_tip, "C"),
        "C should contain A's new commit"
    );

    // Verify linear ancestry: A < B < C
    assert!(commit_is_ancestor(p, "A", "B"));
    assert!(commit_is_ancestor(p, "B", "C"));
}

#[test]
fn test_rebase_onto_preserves_branch() {
    let (dir, repo) = create_test_repo();
    let p = dir.path();

    // main → A → B
    create_branch_with_commit(p, "A", "main");
    create_branch_with_commit(p, "B", "A");

    let b_hash_before = git(p, &["rev-parse", "B"]);

    // Add commit to A so rebase is non-trivial
    git(p, &["checkout", "A"]);
    add_commit(p, "a_update.txt", "update on A");

    repo.rebase_onto("B", "A").expect("rebase failed");

    let b_hash_after = git(p, &["rev-parse", "B"]);

    // B's ref should have moved (new commit hash)
    assert_ne!(
        b_hash_before, b_hash_after,
        "branch ref should be updated after rebase"
    );

    // B should still be a valid branch
    let branches = git(p, &["branch", "--list", "B"]);
    assert!(
        branches.contains('B'),
        "branch B should still exist after rebase"
    );
}
