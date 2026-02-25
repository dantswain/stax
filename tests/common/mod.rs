#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

pub fn create_test_repo() -> (TempDir, stax::git::GitRepo) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path();

    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "test@test.com"]);
    git(path, &["config", "user.name", "Test"]);
    add_commit(path, "init.txt", "initial");

    let repo = stax::git::GitRepo::open(path).expect("failed to open repo");
    (dir, repo)
}

/// Create a test repo with a bare "remote" origin for push/tracking tests.
pub fn create_test_repo_with_remote() -> (TempDir, TempDir, stax::git::GitRepo) {
    let bare_dir = TempDir::new().expect("failed to create bare dir");
    git(bare_dir.path(), &["init", "--bare", "-b", "main"]);

    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path();

    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "test@test.com"]);
    git(path, &["config", "user.name", "Test"]);
    git(
        path,
        &["remote", "add", "origin", bare_dir.path().to_str().unwrap()],
    );
    add_commit(path, "init.txt", "initial");
    git(path, &["push", "-u", "origin", "main"]);

    let repo = stax::git::GitRepo::open(path).expect("failed to open repo");
    (dir, bare_dir, repo)
}

pub fn git(dir: &Path, args: &[&str]) -> String {
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

pub fn add_commit(dir: &Path, filename: &str, content: &str) {
    std::fs::write(dir.join(filename), content).expect("failed to write file");
    git(dir, &["add", filename]);
    git(dir, &["commit", "-m", &format!("add {filename}")]);
}

pub fn create_branch_with_commit(dir: &Path, branch: &str, parent: &str) {
    git(dir, &["checkout", parent]);
    git(dir, &["checkout", "-b", branch]);
    add_commit(dir, &format!("{branch}.txt"), &format!("{branch} content"));
}

pub fn commit_is_ancestor(dir: &Path, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(dir)
        .status()
        .expect("merge-base failed")
        .success()
}
