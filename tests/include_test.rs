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
fn test_recreate_shadow_branch_conflict_leaves_merge_in_progress() {
    use stax::git::ShadowMergeConflict;

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

    let err = result.unwrap_err();
    let conflict = err.downcast_ref::<ShadowMergeConflict>().unwrap();
    assert_eq!(conflict.shadow_name, "stax/shadow/consumer");
    assert_eq!(conflict.failed_source, "feat-b");

    // Merge should be in progress (not aborted)
    assert!(repo.is_merge_in_progress());

    // We should be on the shadow branch
    assert_eq!(repo.current_branch().unwrap(), "stax/shadow/consumer");
}

#[test]
fn test_shadow_merge_conflict_continue() {
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

    // Trigger conflict
    let result = repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"]);
    assert!(result.is_err());
    assert!(repo.is_merge_in_progress());

    // Resolve the conflict manually
    std::fs::write(path.join("shared.txt"), "resolved").unwrap();
    git(path, &["add", "shared.txt"]);

    // Continue the shadow merge (no remaining sources)
    repo.continue_shadow_merge("stax/shadow/consumer", &[])
        .expect("continue should succeed");

    // Shadow should now have both branches' content
    assert!(!repo.is_merge_in_progress());
    let content = std::fs::read_to_string(path.join("shared.txt")).unwrap();
    assert_eq!(content, "resolved");
}

#[test]
fn test_shadow_merge_conflict_with_remaining_sources() {
    use stax::git::ShadowMergeConflict;

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    // Create three branches: a, b conflict, c doesn't
    git(path, &["checkout", "-b", "feat-a"]);
    std::fs::write(path.join("shared.txt"), "version a").unwrap();
    git(path, &["add", "shared.txt"]);
    git(path, &["commit", "-m", "add shared from a"]);

    git(path, &["checkout", "main"]);
    git(path, &["checkout", "-b", "feat-b"]);
    std::fs::write(path.join("shared.txt"), "version b").unwrap();
    git(path, &["add", "shared.txt"]);
    git(path, &["commit", "-m", "add shared from b"]);

    git(path, &["checkout", "main"]);
    git(path, &["checkout", "-b", "feat-c"]);
    add_commit(path, "feat-c.txt", "feat c");

    // Trigger conflict (feat-b conflicts with feat-a, feat-c is remaining)
    let result =
        repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b", "feat-c"]);
    let err = result.unwrap_err();
    let conflict = err.downcast_ref::<ShadowMergeConflict>().unwrap();
    assert_eq!(conflict.failed_source, "feat-b");
    assert_eq!(conflict.remaining_sources, vec!["feat-c"]);

    // Resolve and continue
    std::fs::write(path.join("shared.txt"), "resolved").unwrap();
    git(path, &["add", "shared.txt"]);
    repo.continue_shadow_merge("stax/shadow/consumer", &["feat-c"])
        .expect("continue should succeed");

    // Shadow should have all three branches' content
    assert!(path.join("shared.txt").exists());
    assert!(path.join("feat-c.txt").exists());
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
        pr_refreshed_at: None,
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
        pr_refreshed_at: None,
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

// ── ancestor detection (guard against unnecessary diamond merges) ────────────

#[test]
fn test_ancestor_detected_for_sibling_branches() {
    // When A and B are both off main, main is an ancestor of B.
    // The include guard should detect this.
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    // main is ancestor of feat-b: merge-base(main, feat-b) == main's tip
    let main_tip = repo.get_commit_hash("refs/heads/main").unwrap();
    let merge_base = repo.get_merge_base("main", "feat-b").unwrap();
    assert_eq!(
        merge_base.to_string(),
        main_tip,
        "main should be an ancestor of feat-b"
    );

    // feat-a is NOT an ancestor of feat-b (they diverged from main)
    let a_tip = repo.get_commit_hash("refs/heads/feat-a").unwrap();
    let mb_ab = repo.get_merge_base("feat-a", "feat-b").unwrap();
    assert_ne!(
        mb_ab.to_string(),
        a_tip,
        "feat-a should NOT be an ancestor of feat-b"
    );
}

#[test]
fn test_ancestor_detected_for_stacked_branches() {
    // When B is stacked on A, A is an ancestor of B.
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");

    let a_tip = repo.get_commit_hash("refs/heads/feat-a").unwrap();
    let merge_base = repo.get_merge_base("feat-a", "feat-b").unwrap();
    assert_eq!(
        merge_base.to_string(),
        a_tip,
        "feat-a should be an ancestor of feat-b"
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

// ── nth-level diamond merge ─────────────────────────────────────────────────

#[test]
fn test_shadow_branch_at_nth_level() {
    // main → A → B and main → C → D. Shadow merges C and B for D.
    // This tests that shadow branches work when sources are deep in the tree.
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "a", "main");
    create_branch_with_commit(path, "b", "a");
    create_branch_with_commit(path, "c", "main");
    create_branch_with_commit(path, "d", "c");

    // Create shadow merging c and b (d's parent is c, so sources = [c, b])
    repo.recreate_shadow_branch("stax/shadow/d", &["c", "b"])
        .expect("should create shadow from nth-level branches");

    // Verify shadow has content from both c and b
    git(path, &["checkout", "stax/shadow/d"]);
    assert!(path.join("c.txt").exists(), "should have c's content");
    assert!(path.join("b.txt").exists(), "should have b's content");
    // b is based on a, so a's content should also be reachable
    assert!(path.join("a.txt").exists(), "should have a's content");
}

#[test]
fn test_shadow_recreation_after_source_update() {
    // Recreating shadow after a source branch gets new commits.
    // Simulates what happens during restack.
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();
    let tip1 = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();

    // feat-a gets new commits (simulating restack moving it forward)
    git(path, &["checkout", "feat-a"]);
    add_commit(path, "feat-a-update.txt", "updated");

    // Recreate shadow — should incorporate the new commit
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();
    let tip2 = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();

    assert_ne!(tip1, tip2, "shadow should have new tip after source update");

    // Verify new content is present
    git(path, &["checkout", "stax/shadow/consumer"]);
    assert!(path.join("feat-a-update.txt").exists());
    assert!(path.join("feat-b.txt").exists());
}

#[test]
fn test_dissolve_one_source_merged_rebases_consumer() {
    // Bug 1 regression test: dissolution must rebase the consumer onto the
    // remaining source.  Exercises the actual dissolve_shadows_if_needed
    // function with real cache state.
    use common::commit_is_ancestor;
    use stax::cache::{CachedBranch, ShadowBranch, StackCache};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    // Build shadow and consumer
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();
    git(path, &["checkout", "stax/shadow/consumer"]);
    git(path, &["checkout", "-b", "consumer"]);
    add_commit(path, "consumer.txt", "consumer work");
    repo.rebase_onto("consumer", "stax/shadow/consumer")
        .unwrap();
    assert!(commit_is_ancestor(path, "stax/shadow/consumer", "consumer"));

    // Set up cache exactly as `include` would
    let git_dir = repo.git_dir();
    let shadow_tip = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();
    let consumer_tip = repo.get_commit_hash("refs/heads/consumer").unwrap();
    let main_tip = repo.get_commit_hash("refs/heads/main").unwrap();

    let data = stax::cache::CacheFile {
        schema_version: 2,
        trunk: stax::cache::TrunkInfo {
            name: "main".to_string(),
            tip: main_tip,
            merged: Vec::new(),
        },
        branches: [
            (
                "feat-a".to_string(),
                CachedBranch {
                    tip: repo.get_commit_hash("refs/heads/feat-a").unwrap(),
                    parent: Some("main".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
            (
                "feat-b".to_string(),
                CachedBranch {
                    tip: repo.get_commit_hash("refs/heads/feat-b").unwrap(),
                    parent: Some("main".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
            (
                "stax/shadow/consumer".to_string(),
                CachedBranch {
                    tip: shadow_tip.clone(),
                    parent: Some("feat-a".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
            (
                "consumer".to_string(),
                CachedBranch {
                    tip: consumer_tip,
                    parent: Some("stax/shadow/consumer".to_string()),
                    merge_sources: vec!["feat-a".to_string(), "feat-b".to_string()],
                },
            ),
        ]
        .into_iter()
        .collect(),
        pull_requests: HashMap::new(),
        shadow_branches: [(
            "stax/shadow/consumer".to_string(),
            ShadowBranch {
                consumer: "consumer".to_string(),
                sources: vec!["feat-a".to_string(), "feat-b".to_string()],
                tip: shadow_tip,
            },
        )]
        .into_iter()
        .collect(),
        pr_refreshed_at: None,
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // Simulate feat-b being merged — call dissolve_shadows_if_needed
    let merged = vec![("feat-b".to_string(), "PR #2 merged".to_string())];
    git(path, &["checkout", "consumer"]);
    stax::commands::sync::dissolve_shadows_if_needed(&repo, &merged, "main").unwrap();

    // Consumer should now be based on feat-a (the remaining source)
    assert!(
        commit_is_ancestor(path, "feat-a", "consumer"),
        "consumer should be based on feat-a after dissolution"
    );

    // Shadow branch should be deleted
    let branches = repo.get_branches().unwrap();
    assert!(
        !branches.contains(&"stax/shadow/consumer".to_string()),
        "shadow branch should be deleted"
    );

    // Consumer's own work should survive
    git(path, &["checkout", "consumer"]);
    assert!(path.join("consumer.txt").exists());

    // Cache should reflect the new parent
    let mut cache2 = StackCache::new(&git_dir);
    let loaded = cache2.load().unwrap();
    assert_eq!(
        loaded.branches["consumer"].parent,
        Some("feat-a".to_string())
    );
    assert!(loaded.branches["consumer"].merge_sources.is_empty());
    assert!(loaded.shadow_branches.is_empty());
}

#[test]
fn test_dissolve_all_sources_merged_reparents_to_trunk() {
    use common::commit_is_ancestor;
    use stax::cache::{CachedBranch, ShadowBranch, StackCache};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();
    git(path, &["checkout", "stax/shadow/consumer"]);
    git(path, &["checkout", "-b", "consumer"]);
    add_commit(path, "consumer.txt", "consumer work");
    repo.rebase_onto("consumer", "stax/shadow/consumer")
        .unwrap();

    // Set up cache
    let git_dir = repo.git_dir();
    let shadow_tip = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();
    let data = stax::cache::CacheFile {
        schema_version: 2,
        trunk: stax::cache::TrunkInfo {
            name: "main".to_string(),
            tip: repo.get_commit_hash("refs/heads/main").unwrap(),
            merged: Vec::new(),
        },
        branches: [
            (
                "stax/shadow/consumer".to_string(),
                CachedBranch {
                    tip: shadow_tip.clone(),
                    parent: Some("feat-a".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
            (
                "consumer".to_string(),
                CachedBranch {
                    tip: repo.get_commit_hash("refs/heads/consumer").unwrap(),
                    parent: Some("stax/shadow/consumer".to_string()),
                    merge_sources: vec!["feat-a".to_string(), "feat-b".to_string()],
                },
            ),
        ]
        .into_iter()
        .collect(),
        pull_requests: HashMap::new(),
        shadow_branches: [(
            "stax/shadow/consumer".to_string(),
            ShadowBranch {
                consumer: "consumer".to_string(),
                sources: vec!["feat-a".to_string(), "feat-b".to_string()],
                tip: shadow_tip,
            },
        )]
        .into_iter()
        .collect(),
        pr_refreshed_at: None,
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // Both sources merged
    let merged = vec![
        ("feat-a".to_string(), "PR #1 merged".to_string()),
        ("feat-b".to_string(), "PR #2 merged".to_string()),
    ];
    git(path, &["checkout", "consumer"]);
    stax::commands::sync::dissolve_shadows_if_needed(&repo, &merged, "main").unwrap();

    // Consumer should be based on main (trunk)
    assert!(commit_is_ancestor(path, "main", "consumer"));

    // Cache should show consumer → main
    let mut cache2 = StackCache::new(&git_dir);
    let loaded = cache2.load().unwrap();
    assert_eq!(loaded.branches["consumer"].parent, Some("main".to_string()));
}

#[test]
fn test_dissolve_partial_recreates_shadow_with_remaining() {
    use stax::cache::{CachedBranch, ShadowBranch, StackCache};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");
    create_branch_with_commit(path, "feat-c", "main");

    // Shadow merges all three
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b", "feat-c"])
        .unwrap();
    git(path, &["checkout", "stax/shadow/consumer"]);
    git(path, &["checkout", "-b", "consumer"]);
    add_commit(path, "consumer.txt", "consumer work");

    let git_dir = repo.git_dir();
    let shadow_tip = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();
    let data = stax::cache::CacheFile {
        schema_version: 2,
        trunk: stax::cache::TrunkInfo {
            name: "main".to_string(),
            tip: repo.get_commit_hash("refs/heads/main").unwrap(),
            merged: Vec::new(),
        },
        branches: [
            (
                "consumer".to_string(),
                CachedBranch {
                    tip: repo.get_commit_hash("refs/heads/consumer").unwrap(),
                    parent: Some("stax/shadow/consumer".to_string()),
                    merge_sources: vec![
                        "feat-a".to_string(),
                        "feat-b".to_string(),
                        "feat-c".to_string(),
                    ],
                },
            ),
            (
                "stax/shadow/consumer".to_string(),
                CachedBranch {
                    tip: shadow_tip.clone(),
                    parent: Some("feat-a".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
        ]
        .into_iter()
        .collect(),
        pull_requests: HashMap::new(),
        shadow_branches: [(
            "stax/shadow/consumer".to_string(),
            ShadowBranch {
                consumer: "consumer".to_string(),
                sources: vec![
                    "feat-a".to_string(),
                    "feat-b".to_string(),
                    "feat-c".to_string(),
                ],
                tip: shadow_tip,
            },
        )]
        .into_iter()
        .collect(),
        pr_refreshed_at: None,
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // Only feat-b merged — 2 sources remain, shadow should be recreated
    let merged = vec![("feat-b".to_string(), "PR #2 merged".to_string())];
    git(path, &["checkout", "consumer"]);
    stax::commands::sync::dissolve_shadows_if_needed(&repo, &merged, "main").unwrap();

    // Shadow should still exist with remaining sources
    let branches = repo.get_branches().unwrap();
    assert!(branches.contains(&"stax/shadow/consumer".to_string()));

    // Shadow should have content from feat-a and feat-c
    git(path, &["checkout", "stax/shadow/consumer"]);
    assert!(path.join("feat-a.txt").exists());
    assert!(path.join("feat-c.txt").exists());

    // Cache should reflect updated sources
    let mut cache2 = StackCache::new(&git_dir);
    let loaded = cache2.load().unwrap();
    let shadow = &loaded.shadow_branches["stax/shadow/consumer"];
    assert_eq!(shadow.sources, vec!["feat-a", "feat-c"]);
    assert_eq!(
        loaded.branches["consumer"].merge_sources,
        vec!["feat-a", "feat-c"]
    );
}

#[test]
fn test_shadow_injected_into_parent_map() {
    // Verify that get_branches_and_parent_map injects shadow entries from
    // cache so the parent map shows consumer → shadow → first_source.
    use stax::cache::{CachedBranch, ShadowBranch, StackCache};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    // Create the shadow branch and consumer in git
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();
    git(path, &["checkout", "stax/shadow/consumer"]);
    git(path, &["checkout", "-b", "consumer"]);
    add_commit(path, "consumer.txt", "consumer work");
    repo.rebase_onto("consumer", "stax/shadow/consumer")
        .unwrap();

    // Set up cache with shadow data
    let git_dir = repo.git_dir();
    let shadow_tip = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();
    let data = stax::cache::CacheFile {
        schema_version: 2,
        trunk: stax::cache::TrunkInfo {
            name: "main".to_string(),
            tip: repo.get_commit_hash("refs/heads/main").unwrap(),
            merged: Vec::new(),
        },
        branches: [
            (
                "feat-a".to_string(),
                CachedBranch {
                    tip: repo.get_commit_hash("refs/heads/feat-a").unwrap(),
                    parent: Some("main".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
            (
                "feat-b".to_string(),
                CachedBranch {
                    tip: repo.get_commit_hash("refs/heads/feat-b").unwrap(),
                    parent: Some("main".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
            (
                "stax/shadow/consumer".to_string(),
                CachedBranch {
                    tip: shadow_tip.clone(),
                    parent: Some("feat-a".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
            (
                "consumer".to_string(),
                CachedBranch {
                    tip: repo.get_commit_hash("refs/heads/consumer").unwrap(),
                    parent: Some("stax/shadow/consumer".to_string()),
                    merge_sources: vec!["feat-a".to_string(), "feat-b".to_string()],
                },
            ),
        ]
        .into_iter()
        .collect(),
        pull_requests: HashMap::new(),
        shadow_branches: [(
            "stax/shadow/consumer".to_string(),
            ShadowBranch {
                consumer: "consumer".to_string(),
                sources: vec!["feat-a".to_string(), "feat-b".to_string()],
                tip: shadow_tip,
            },
        )]
        .into_iter()
        .collect(),
        pr_refreshed_at: None,
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // Now call get_branches_and_parent_map — it should inject shadow entries
    let (_, _, _, parent_map) =
        stax::commands::navigate::get_branches_and_parent_map(&repo).unwrap();

    // Shadow should be in the parent map with first source as parent
    assert_eq!(
        parent_map.get("stax/shadow/consumer"),
        Some(&Some("feat-a".to_string())),
        "shadow's parent should be first source"
    );
    // Consumer's parent should be the shadow
    assert_eq!(
        parent_map.get("consumer"),
        Some(&Some("stax/shadow/consumer".to_string())),
        "consumer's parent should be the shadow"
    );
}

#[test]
fn test_restack_recreates_shadow_before_consumer() {
    // When restacking, if a branch's parent is a shadow, the shadow must be
    // recreated from its sources before the consumer is rebased onto it.
    use common::commit_is_ancestor;

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    // Build shadow and consumer
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();

    git(path, &["checkout", "stax/shadow/consumer"]);
    git(path, &["checkout", "-b", "consumer"]);
    add_commit(path, "consumer.txt", "consumer work");
    repo.rebase_onto("consumer", "stax/shadow/consumer")
        .unwrap();

    // Now advance feat-a (simulating restack of feat-a)
    git(path, &["checkout", "feat-a"]);
    add_commit(path, "feat-a-v2.txt", "feat-a updated");

    // Recreate shadow (this is what restack does before rebasing consumer)
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();

    // Now rebase consumer onto the new shadow
    git(path, &["checkout", "consumer"]);
    repo.rebase_onto("consumer", "stax/shadow/consumer")
        .unwrap();

    // Consumer should be based on the new shadow
    assert!(commit_is_ancestor(path, "stax/shadow/consumer", "consumer"));

    // Consumer should have access to feat-a's new content
    git(path, &["checkout", "consumer"]);
    assert!(path.join("feat-a-v2.txt").exists());
    assert!(path.join("consumer.txt").exists());
}
