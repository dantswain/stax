mod common;

use common::{add_commit, create_branch_with_commit, create_test_repo, git};
use std::collections::{HashMap, HashSet};

// ── shadow branch creation ──────────────────────────────────────────────────

#[test]
fn test_recreate_shadow_branch_basic() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    // Create two feature branches from main
    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    // Create shadow branch merging both
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .expect("should create shadow branch");

    // Shadow branch should exist
    let branches = repo.get_branches().unwrap();
    assert!(branches.contains(&"stax/shadow/consumer".to_string()));

    // Shadow should have both branches' content
    git(path, &["checkout", "stax/shadow/consumer"]);
    assert!(path.join("feat-a.txt").exists());
    assert!(path.join("feat-b.txt").exists());
}

#[test]
fn test_recreate_shadow_branch_replaces_old() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    // Create first version
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();

    let tip1 = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();

    // Add a commit to feat-a
    git(path, &["checkout", "feat-a"]);
    add_commit(path, "feat-a-2.txt", "more a");

    // Recreate — should replace the old shadow
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();

    let tip2 = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();

    assert_ne!(tip1, tip2, "shadow branch should have a new tip");
}

#[test]
fn test_recreate_shadow_branch_conflict_aborts_cleanly() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    // Create two branches that modify the same file
    git(path, &["checkout", "-b", "feat-a"]);
    std::fs::write(path.join("shared.txt"), "version a").unwrap();
    git(path, &["add", "shared.txt"]);
    git(path, &["commit", "-m", "add shared from a"]);

    git(path, &["checkout", "main"]);
    git(path, &["checkout", "-b", "feat-b"]);
    std::fs::write(path.join("shared.txt"), "version b").unwrap();
    git(path, &["add", "shared.txt"]);
    git(path, &["commit", "-m", "add shared from b"]);

    // This should fail due to merge conflict
    let result = repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"]);
    assert!(result.is_err());

    // Working tree should be clean (merge was aborted)
    assert!(repo.is_clean().unwrap());
}

// ── is_shadow_branch ────────────────────────────────────────────────────────

#[test]
fn test_is_shadow_branch() {
    use stax::commands::navigate::is_shadow_branch;

    assert!(is_shadow_branch("stax/shadow/consumer"));
    assert!(is_shadow_branch("stax/shadow/feat-x"));
    assert!(!is_shadow_branch("feat-a"));
    assert!(!is_shadow_branch("main"));
    assert!(!is_shadow_branch("stax-shadow-consumer")); // wrong separator
}

// ── shadow branches excluded from heuristic parent detection ────────────────

#[test]
fn test_shadow_branches_excluded_from_find_parent() {
    use stax::commands::navigate::{build_commit_cache, find_parent};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");

    // Create a child of feat-a
    create_branch_with_commit(path, "feat-child", "feat-a");

    // Create a shadow branch from feat-a (same commit as feat-a)
    git(path, &["checkout", "feat-a"]);
    git(path, &["checkout", "-b", "stax/shadow/consumer"]);

    let branches = repo.get_branches().unwrap();
    let commits = build_commit_cache(&repo, &branches).unwrap();
    let merged = HashSet::new();

    // find_parent for feat-child should find feat-a, not the shadow
    let parent = find_parent(&repo, "feat-child", &branches, &commits, &merged)
        .unwrap()
        .unwrap();
    assert_eq!(parent, "feat-a");
}

// ── shadow branches excluded from find_children ─────────────────────────────

#[test]
fn test_shadow_branches_excluded_from_find_children() {
    use stax::commands::navigate::{build_commit_cache, find_children};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");

    // Create a shadow branch from feat-a
    git(path, &["checkout", "feat-a"]);
    git(path, &["checkout", "-b", "stax/shadow/x"]);
    add_commit(path, "shadow.txt", "shadow");

    let branches = repo.get_branches().unwrap();
    let commits = build_commit_cache(&repo, &branches).unwrap();
    let merged = HashSet::new();

    // Shadow should not appear as a child of main
    let children = find_children(&repo, "main", &branches, &commits, &merged).unwrap();
    assert!(
        !children.contains(&"stax/shadow/x".to_string()),
        "shadow branches should not appear as children"
    );
}

// ── children_from_map resolves shadow to consumer ───────────────────────────

#[test]
fn test_children_from_map_resolves_shadow() {
    use stax::commands::navigate::children_from_map;

    let mut parent_map: HashMap<String, Option<String>> = HashMap::new();
    parent_map.insert("main".to_string(), None);
    parent_map.insert("feat-a".to_string(), Some("main".to_string()));
    parent_map.insert(
        "stax/shadow/consumer".to_string(),
        Some("feat-a".to_string()),
    );
    parent_map.insert(
        "consumer".to_string(),
        Some("stax/shadow/consumer".to_string()),
    );

    let merged = HashSet::new();

    // children of feat-a should be [consumer], not [stax/shadow/consumer]
    let children = children_from_map("feat-a", &parent_map, &merged);
    assert_eq!(children, vec!["consumer".to_string()]);
}

// ── cache shadow branch helpers ─────────────────────────────────────────────

#[test]
fn test_cache_shadow_roundtrip() {
    use stax::cache::{ShadowBranch, StackCache};

    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path();

    // Create initial cache
    let cache = StackCache::new(git_dir);
    let mut branches = HashMap::new();
    branches.insert(
        "feat-a".to_string(),
        stax::cache::CachedBranch {
            tip: "aaa".to_string(),
            parent: Some("main".to_string()),
            merge_sources: Vec::new(),
        },
    );
    let data = stax::cache::CacheFile {
        schema_version: 2,
        trunk: stax::cache::TrunkInfo {
            name: "main".to_string(),
            tip: "trunk000".to_string(),
            merged: Vec::new(),
        },
        branches,
        pull_requests: HashMap::new(),
        shadow_branches: HashMap::new(),
    };
    cache.save(&data);

    // Upsert a shadow
    let mut cache2 = StackCache::new(git_dir);
    cache2.upsert_shadow(
        "stax/shadow/consumer",
        ShadowBranch {
            consumer: "consumer".to_string(),
            sources: vec!["feat-a".to_string(), "feat-b".to_string()],
            tip: "shadow-tip".to_string(),
        },
    );

    // Load and verify
    let mut cache3 = StackCache::new(git_dir);
    let loaded = cache3.load().unwrap();
    assert_eq!(loaded.shadow_branches.len(), 1);
    let shadow = &loaded.shadow_branches["stax/shadow/consumer"];
    assert_eq!(shadow.consumer, "consumer");
    assert_eq!(shadow.sources, vec!["feat-a", "feat-b"]);

    // Remove
    cache3.remove_shadow("stax/shadow/consumer");

    let mut cache4 = StackCache::new(git_dir);
    let loaded2 = cache4.load().unwrap();
    assert!(loaded2.shadow_branches.is_empty());
}

#[test]
fn test_cache_merge_sources_serialization() {
    use stax::cache::{CacheFile, CachedBranch, StackCache, TrunkInfo};

    let dir = tempfile::tempdir().unwrap();

    let mut branches = HashMap::new();
    branches.insert(
        "consumer".to_string(),
        CachedBranch {
            tip: "ccc".to_string(),
            parent: Some("stax/shadow/consumer".to_string()),
            merge_sources: vec!["feat-a".to_string(), "feat-b".to_string()],
        },
    );

    let data = CacheFile {
        schema_version: 2,
        trunk: TrunkInfo {
            name: "main".to_string(),
            tip: "trunk".to_string(),
            merged: Vec::new(),
        },
        branches,
        pull_requests: HashMap::new(),
        shadow_branches: HashMap::new(),
    };

    let cache = StackCache::new(dir.path());
    cache.save(&data);

    let mut cache2 = StackCache::new(dir.path());
    let loaded = cache2.load().unwrap();
    assert_eq!(
        loaded.branches["consumer"].merge_sources,
        vec!["feat-a", "feat-b"]
    );
}

// ── build_parent_map skips shadow branches ──────────────────────────────────

#[test]
fn test_build_parent_map_skips_shadows() {
    use stax::commands::navigate::{build_commit_cache, build_parent_map};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");

    // Create a shadow branch
    git(path, &["checkout", "feat-a"]);
    git(path, &["checkout", "-b", "stax/shadow/x"]);
    add_commit(path, "shadow.txt", "shadow");

    let branches = repo.get_branches().unwrap();
    let commits = build_commit_cache(&repo, &branches).unwrap();
    let merged = HashSet::new();

    let parent_map = build_parent_map(&repo, &branches, &commits, &merged).unwrap();

    // Shadow branch should NOT be in the parent map (it's managed by include)
    assert!(
        !parent_map.contains_key("stax/shadow/x"),
        "shadow branch should not be in parent_map from build_parent_map"
    );
}
