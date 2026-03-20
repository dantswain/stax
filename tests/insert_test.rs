mod common;

use common::{add_commit, create_branch_with_commit, create_test_repo, git};
use std::collections::HashMap;

// ── helpers ─────────────────────────────────────────────────────────────────

/// Build a cache file with the given branch parent relationships and save it.
fn setup_cache(
    git_dir: &std::path::Path,
    repo: &stax::git::GitRepo,
    parent_map: &[(&str, Option<&str>)],
) {
    use stax::cache::{CacheFile, CachedBranch, StackCache, TrunkInfo};

    let mut branches = HashMap::new();
    for &(name, parent) in parent_map {
        if name == "main" {
            continue;
        }
        let tip = repo.get_commit_hash(&format!("refs/heads/{name}")).unwrap();
        branches.insert(
            name.to_string(),
            CachedBranch {
                tip,
                parent: parent.map(|s| s.to_string()),
                merge_sources: Vec::new(),
            },
        );
    }

    let main_tip = repo.get_commit_hash("refs/heads/main").unwrap();
    let data = CacheFile {
        schema_version: 2,
        trunk: TrunkInfo {
            name: "main".to_string(),
            tip: main_tip,
            merged: Vec::new(),
        },
        branches,
        pull_requests: HashMap::new(),
        shadow_branches: HashMap::new(),
    };
    let cache = StackCache::new(git_dir);
    cache.save(&data);
}

/// Assert a branch's parent in cache.
fn assert_parent(git_dir: &std::path::Path, branch: &str, expected_parent: &str) {
    let mut cache = stax::cache::StackCache::new(git_dir);
    cache.load().expect("cache should load");
    let data = cache.data_ref().unwrap();
    assert_eq!(
        data.branches[branch].parent,
        Some(expected_parent.to_string()),
        "expected '{}' parent to be '{}', got {:?}",
        branch,
        expected_parent,
        data.branches[branch].parent,
    );
}

// ── insert below ────────────────────────────────────────────────────────────

#[test]
fn test_insert_below_basic() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("feat-a")),
        ],
    );

    // On feat-b, insert below
    git(path, &["checkout", "feat-b"]);

    // Perform the insert: create new-branch from feat-a (feat-b's parent)
    repo.create_branch("new-branch", Some("refs/heads/feat-a"))
        .unwrap();

    let new_tip = repo.get_commit_hash("refs/heads/new-branch").unwrap();
    let feat_b_tip = repo.get_commit_hash("refs/heads/feat-b").unwrap();

    let mut cache = stax::cache::StackCache::new(&git_dir);
    cache.load();
    cache.upsert_branch("new-branch", &new_tip, Some("feat-a"));
    cache.upsert_branch("feat-b", &feat_b_tip, Some("new-branch"));

    repo.checkout_branch("new-branch").unwrap();

    // Verify
    assert_eq!(repo.current_branch().unwrap(), "new-branch");
    assert_parent(&git_dir, "new-branch", "feat-a");
    assert_parent(&git_dir, "feat-b", "new-branch");

    // new-branch should be at feat-a's tip (no new commits)
    let feat_a_tip = repo.get_commit_hash("refs/heads/feat-a").unwrap();
    assert_eq!(new_tip, feat_a_tip);
}

#[test]
fn test_insert_above_basic() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("feat-a")),
        ],
    );

    // On feat-a, insert above (between feat-a and feat-b)
    git(path, &["checkout", "feat-a"]);

    repo.create_branch("new-branch", Some("refs/heads/feat-a"))
        .unwrap();

    let new_tip = repo.get_commit_hash("refs/heads/new-branch").unwrap();
    let feat_b_tip = repo.get_commit_hash("refs/heads/feat-b").unwrap();

    let mut cache = stax::cache::StackCache::new(&git_dir);
    cache.load();
    cache.upsert_branch("new-branch", &new_tip, Some("feat-a"));
    cache.upsert_branch("feat-b", &feat_b_tip, Some("new-branch"));

    repo.checkout_branch("new-branch").unwrap();

    // Verify
    assert_eq!(repo.current_branch().unwrap(), "new-branch");
    assert_parent(&git_dir, "new-branch", "feat-a");
    assert_parent(&git_dir, "feat-b", "new-branch");

    // new-branch should be at feat-a's tip
    let feat_a_tip = repo.get_commit_hash("refs/heads/feat-a").unwrap();
    assert_eq!(new_tip, feat_a_tip);
}

#[test]
fn test_insert_above_multiple_children() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");
    create_branch_with_commit(path, "feat-c", "feat-a");

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("feat-a")),
            ("feat-c", Some("feat-a")),
        ],
    );

    // On feat-a, insert above — should reparent both feat-b and feat-c
    git(path, &["checkout", "feat-a"]);

    repo.create_branch("new-branch", Some("refs/heads/feat-a"))
        .unwrap();

    let new_tip = repo.get_commit_hash("refs/heads/new-branch").unwrap();
    let feat_b_tip = repo.get_commit_hash("refs/heads/feat-b").unwrap();
    let feat_c_tip = repo.get_commit_hash("refs/heads/feat-c").unwrap();

    let mut cache = stax::cache::StackCache::new(&git_dir);
    cache.load();
    cache.upsert_branch("new-branch", &new_tip, Some("feat-a"));
    cache.upsert_branch("feat-b", &feat_b_tip, Some("new-branch"));
    cache.upsert_branch("feat-c", &feat_c_tip, Some("new-branch"));

    // Verify
    assert_parent(&git_dir, "new-branch", "feat-a");
    assert_parent(&git_dir, "feat-b", "new-branch");
    assert_parent(&git_dir, "feat-c", "new-branch");
}

#[test]
fn test_insert_above_no_children() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");

    let git_dir = repo.git_dir();
    setup_cache(&git_dir, &repo, &[("main", None), ("feat-a", Some("main"))]);

    // On feat-a (leaf), insert above — just creates a branch, no reparenting
    git(path, &["checkout", "feat-a"]);

    repo.create_branch("new-branch", Some("refs/heads/feat-a"))
        .unwrap();

    let new_tip = repo.get_commit_hash("refs/heads/new-branch").unwrap();

    let mut cache = stax::cache::StackCache::new(&git_dir);
    cache.load();
    cache.upsert_branch("new-branch", &new_tip, Some("feat-a"));

    repo.checkout_branch("new-branch").unwrap();

    assert_eq!(repo.current_branch().unwrap(), "new-branch");
    assert_parent(&git_dir, "new-branch", "feat-a");
}

#[test]
fn test_insert_below_diamond_consumer_guard() {
    use stax::cache::{CacheFile, CachedBranch, ShadowBranch, StackCache, TrunkInfo};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    // Create shadow and consumer
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();
    git(path, &["checkout", "stax/shadow/consumer"]);
    git(path, &["checkout", "-b", "consumer"]);
    add_commit(path, "consumer.txt", "consumer work");

    let git_dir = repo.git_dir();
    let shadow_tip = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();
    let data = CacheFile {
        schema_version: 2,
        trunk: TrunkInfo {
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
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // Verify the guard condition: consumer has merge_sources
    let mut cache2 = StackCache::new(&git_dir);
    cache2.load();
    let has_merge_sources = cache2
        .data_ref()
        .and_then(|d| d.branches.get("consumer"))
        .map(|b| !b.merge_sources.is_empty())
        .unwrap_or(false);

    assert!(
        has_merge_sources,
        "consumer should have merge_sources (diamond consumer)"
    );
}

#[test]
fn test_insert_below_shadow_parent_guard() {
    use stax::commands::navigate::is_shadow_branch;

    // The parent of a diamond consumer is a shadow branch
    assert!(is_shadow_branch("stax/shadow/consumer"));
    assert!(!is_shadow_branch("feat-a"));
}

#[test]
fn test_insert_above_diamond_child_updates_sources() {
    use stax::cache::{CacheFile, CachedBranch, ShadowBranch, StackCache, TrunkInfo};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    // Create shadow merging feat-a and feat-b for consumer
    repo.recreate_shadow_branch("stax/shadow/consumer", &["feat-a", "feat-b"])
        .unwrap();
    git(path, &["checkout", "stax/shadow/consumer"]);
    git(path, &["checkout", "-b", "consumer"]);
    add_commit(path, "consumer.txt", "consumer work");
    repo.rebase_onto("consumer", "stax/shadow/consumer")
        .unwrap();

    let git_dir = repo.git_dir();
    let shadow_tip = repo
        .get_commit_hash("refs/heads/stax/shadow/consumer")
        .unwrap();
    let data = CacheFile {
        schema_version: 2,
        trunk: TrunkInfo {
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
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // Insert a new branch above feat-a.
    // Consumer is a diamond child of feat-a (feat-a is in its merge_sources).
    // The insert should update consumer's merge_sources to use new-branch
    // instead of feat-a.
    git(path, &["checkout", "feat-a"]);

    repo.create_branch("new-branch", Some("refs/heads/feat-a"))
        .unwrap();

    let new_tip = repo.get_commit_hash("refs/heads/new-branch").unwrap();

    let mut cache2 = StackCache::new(&git_dir);
    cache2.load();
    cache2.upsert_branch("new-branch", &new_tip, Some("feat-a"));

    // Update diamond child sources
    stax::commands::insert::update_diamond_child_sources(
        &repo,
        &mut cache2,
        "consumer",
        "feat-a",
        "new-branch",
    )
    .unwrap();

    // Verify cache state
    let cache3 = load_cache(&git_dir);
    let data = cache3.data_ref().unwrap();

    // Consumer's merge_sources should now reference new-branch instead of feat-a
    assert_eq!(
        data.branches["consumer"].merge_sources,
        vec!["new-branch".to_string(), "feat-b".to_string()]
    );

    // Shadow should also be updated
    let shadow = &data.shadow_branches["stax/shadow/consumer"];
    assert_eq!(
        shadow.sources,
        vec!["new-branch".to_string(), "feat-b".to_string()]
    );

    // Shadow's cache entry parent should now be new-branch
    assert_eq!(
        data.branches["stax/shadow/consumer"].parent,
        Some("new-branch".to_string())
    );
}

// ── end-to-end via the command function ─────────────────────────────────────

#[tokio::test]
async fn test_insert_below_end_to_end() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("feat-a")),
        ],
    );

    // On feat-b, run insert below
    git(path, &["checkout", "feat-b"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Below, Some("inserted"), true).await;

    std::env::set_current_dir(&original_dir).unwrap();

    result.expect("insert below should succeed");

    // Verify state
    assert_eq!(repo.current_branch().unwrap(), "inserted");
    assert_parent(&git_dir, "inserted", "feat-a");
    assert_parent(&git_dir, "feat-b", "inserted");
}

#[tokio::test]
async fn test_insert_above_end_to_end() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("feat-a")),
        ],
    );

    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Above, Some("inserted"), true).await;

    std::env::set_current_dir(&original_dir).unwrap();

    result.expect("insert above should succeed");

    assert_eq!(repo.current_branch().unwrap(), "inserted");
    assert_parent(&git_dir, "inserted", "feat-a");
    assert_parent(&git_dir, "feat-b", "inserted");
}

#[tokio::test]
async fn test_insert_below_main_errors() {
    let (dir, _repo) = create_test_repo();
    let path = dir.path();

    git(path, &["checkout", "main"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Below, Some("new-branch"), true).await;

    std::env::set_current_dir(&original_dir).unwrap();

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Cannot insert below a main branch"),
        "Expected main branch error, got: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_insert_above_end_to_end_multiple_children() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");
    create_branch_with_commit(path, "feat-c", "feat-a");

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("feat-a")),
            ("feat-c", Some("feat-a")),
        ],
    );

    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Above, Some("inserted"), true).await;
    std::env::set_current_dir(&original_dir).unwrap();
    result.expect("insert above should succeed");

    assert_eq!(repo.current_branch().unwrap(), "inserted");
    assert_parent(&git_dir, "inserted", "feat-a");
    assert_parent(&git_dir, "feat-b", "inserted");
    assert_parent(&git_dir, "feat-c", "inserted");
}

// ── self-insert errors ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_insert_self_errors() {
    let (dir, _repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    // Try to insert the current branch relative to itself
    let result =
        stax::commands::insert::run(stax::InsertPosition::Above, Some("feat-a"), true).await;

    std::env::set_current_dir(&original_dir).unwrap();

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("itself"),
        "Expected self-reference error, got: {}",
        err_msg
    );
}

// ── cached PR base_ref update ───────────────────────────────────────────────

#[tokio::test]
async fn test_insert_below_updates_cached_pr_base() {
    use stax::cache::{CacheFile, CachedBranch, CachedPullRequest, StackCache, TrunkInfo};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");

    let git_dir = repo.git_dir();
    let main_tip = repo.get_commit_hash("refs/heads/main").unwrap();
    let feat_a_tip = repo.get_commit_hash("refs/heads/feat-a").unwrap();

    // Set up cache with a PR for feat-a targeting main
    let data = CacheFile {
        schema_version: 2,
        trunk: TrunkInfo {
            name: "main".to_string(),
            tip: main_tip,
            merged: Vec::new(),
        },
        branches: [(
            "feat-a".to_string(),
            CachedBranch {
                tip: feat_a_tip,
                parent: Some("main".to_string()),
                merge_sources: Vec::new(),
            },
        )]
        .into_iter()
        .collect(),
        pull_requests: [(
            "feat-a".to_string(),
            CachedPullRequest {
                number: 123,
                state: "open".to_string(),
                head_ref: "feat-a".to_string(),
                base_ref: "main".to_string(),
                html_url: "https://github.com/o/r/pull/123".to_string(),
                draft: false,
            },
        )]
        .into_iter()
        .collect(),
        shadow_branches: HashMap::new(),
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // On feat-a, insert below — should update cached PR base_ref
    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Below, Some("inserted"), true).await;

    std::env::set_current_dir(&original_dir).unwrap();

    result.expect("insert below should succeed");

    // Verify cached PR base_ref was updated
    let cache2 = load_cache(&git_dir);
    let data2 = cache2.data_ref().unwrap();
    assert_eq!(
        data2.pull_requests["feat-a"].base_ref, "inserted",
        "cached PR base_ref should be updated to the inserted branch"
    );
    // PR number should be preserved
    assert_eq!(data2.pull_requests["feat-a"].number, 123);
}

#[tokio::test]
async fn test_insert_above_updates_cached_pr_base_for_children() {
    use stax::cache::{CacheFile, CachedBranch, CachedPullRequest, StackCache, TrunkInfo};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");

    let git_dir = repo.git_dir();
    let main_tip = repo.get_commit_hash("refs/heads/main").unwrap();
    let feat_a_tip = repo.get_commit_hash("refs/heads/feat-a").unwrap();
    let feat_b_tip = repo.get_commit_hash("refs/heads/feat-b").unwrap();

    // Set up cache with PRs
    let data = CacheFile {
        schema_version: 2,
        trunk: TrunkInfo {
            name: "main".to_string(),
            tip: main_tip,
            merged: Vec::new(),
        },
        branches: [
            (
                "feat-a".to_string(),
                CachedBranch {
                    tip: feat_a_tip,
                    parent: Some("main".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
            (
                "feat-b".to_string(),
                CachedBranch {
                    tip: feat_b_tip,
                    parent: Some("feat-a".to_string()),
                    merge_sources: Vec::new(),
                },
            ),
        ]
        .into_iter()
        .collect(),
        pull_requests: [(
            "feat-b".to_string(),
            CachedPullRequest {
                number: 456,
                state: "open".to_string(),
                head_ref: "feat-b".to_string(),
                base_ref: "feat-a".to_string(),
                html_url: "https://github.com/o/r/pull/456".to_string(),
                draft: false,
            },
        )]
        .into_iter()
        .collect(),
        shadow_branches: HashMap::new(),
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // On feat-a, insert above — feat-b should have its PR base updated
    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Above, Some("inserted"), true).await;

    std::env::set_current_dir(&original_dir).unwrap();

    result.expect("insert above should succeed");

    // Verify cached PR base_ref was updated for feat-b
    let cache2 = load_cache(&git_dir);
    let data2 = cache2.data_ref().unwrap();
    assert_eq!(
        data2.pull_requests["feat-b"].base_ref, "inserted",
        "cached PR base_ref for child should be updated to inserted branch"
    );
    assert_eq!(data2.pull_requests["feat-b"].number, 456);
}

#[tokio::test]
async fn test_insert_below_consistent_with_get_branches_and_parent_map() {
    // The key integration test: after insert below, get_branches_and_parent_map
    // should return the inserted parent, NOT the PR's old base_ref.
    use stax::cache::{CacheFile, CachedBranch, CachedPullRequest, StackCache, TrunkInfo};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");

    let git_dir = repo.git_dir();
    let main_tip = repo.get_commit_hash("refs/heads/main").unwrap();
    let feat_a_tip = repo.get_commit_hash("refs/heads/feat-a").unwrap();

    let data = CacheFile {
        schema_version: 2,
        trunk: TrunkInfo {
            name: "main".to_string(),
            tip: main_tip,
            merged: Vec::new(),
        },
        branches: [(
            "feat-a".to_string(),
            CachedBranch {
                tip: feat_a_tip,
                parent: Some("main".to_string()),
                merge_sources: Vec::new(),
            },
        )]
        .into_iter()
        .collect(),
        pull_requests: [(
            "feat-a".to_string(),
            CachedPullRequest {
                number: 99,
                state: "open".to_string(),
                head_ref: "feat-a".to_string(),
                base_ref: "main".to_string(),
                html_url: "https://github.com/o/r/pull/99".to_string(),
                draft: false,
            },
        )]
        .into_iter()
        .collect(),
        shadow_branches: HashMap::new(),
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    stax::commands::insert::run(stax::InsertPosition::Below, Some("inserted"), true)
        .await
        .expect("insert should succeed");

    std::env::set_current_dir(&original_dir).unwrap();

    // Now call get_branches_and_parent_map using the existing repo —
    // it should see feat-a's parent as "inserted", NOT "main" (the stale PR base).
    let (_, _, _, parent_map) =
        stax::commands::navigate::get_branches_and_parent_map(&repo).unwrap();

    assert_eq!(
        parent_map.get("feat-a"),
        Some(&Some("inserted".to_string())),
        "feat-a's parent should be 'inserted', not reverted to 'main' by PR override"
    );
    assert_eq!(
        parent_map.get("inserted"),
        Some(&Some("main".to_string())),
        "inserted's parent should be 'main'"
    );
}

// ── reparent existing branch ────────────────────────────────────────────────

#[tokio::test]
async fn test_insert_above_reparents_existing_branch() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main"); // currently off main

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("main")),
        ],
    );

    // On feat-a, insert above feat-b — should reparent feat-b onto feat-a
    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Above, Some("feat-b"), true).await;
    std::env::set_current_dir(&original_dir).unwrap();
    result.expect("reparent above should succeed");

    assert_parent(&git_dir, "feat-b", "feat-a");
}

#[tokio::test]
async fn test_insert_below_reparents_existing_branch() {
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "feat-a");

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("feat-a")),
        ],
    );

    // On feat-b, insert below feat-a — should reparent feat-b onto feat-a
    git(path, &["checkout", "feat-b"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Below, Some("feat-a"), true).await;
    std::env::set_current_dir(&original_dir).unwrap();
    result.expect("reparent below should succeed");

    assert_parent(&git_dir, "feat-b", "feat-a");
}

#[tokio::test]
async fn test_insert_above_existing_updates_pr_base() {
    use stax::cache::{CacheFile, CachedBranch, CachedPullRequest, StackCache, TrunkInfo};

    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    let git_dir = repo.git_dir();
    let main_tip = repo.get_commit_hash("refs/heads/main").unwrap();

    // Set up cache with a PR for feat-b targeting main
    let data = CacheFile {
        schema_version: 2,
        trunk: TrunkInfo {
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
        ]
        .into_iter()
        .collect(),
        pull_requests: [(
            "feat-b".to_string(),
            CachedPullRequest {
                number: 42,
                state: "open".to_string(),
                head_ref: "feat-b".to_string(),
                base_ref: "main".to_string(),
                html_url: "https://github.com/o/r/pull/42".to_string(),
                draft: false,
            },
        )]
        .into_iter()
        .collect(),
        shadow_branches: HashMap::new(),
    };
    let cache = StackCache::new(&git_dir);
    cache.save(&data);

    // On feat-a, reparent feat-b above — should update PR base_ref
    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    let result =
        stax::commands::insert::run(stax::InsertPosition::Above, Some("feat-b"), true).await;
    std::env::set_current_dir(&original_dir).unwrap();
    result.expect("reparent above should succeed");

    let cache2 = load_cache(&git_dir);
    let data2 = cache2.data_ref().unwrap();
    assert_eq!(data2.pull_requests["feat-b"].base_ref, "feat-a");
    assert_eq!(data2.branches["feat-b"].parent, Some("feat-a".to_string()));
}

#[tokio::test]
async fn test_insert_above_existing_does_not_checkout() {
    // Reparenting an existing branch should NOT check it out —
    // the user stays on their current branch.
    let (dir, repo) = create_test_repo();
    let path = dir.path();

    create_branch_with_commit(path, "feat-a", "main");
    create_branch_with_commit(path, "feat-b", "main");

    let git_dir = repo.git_dir();
    setup_cache(
        &git_dir,
        &repo,
        &[
            ("main", None),
            ("feat-a", Some("main")),
            ("feat-b", Some("main")),
        ],
    );

    git(path, &["checkout", "feat-a"]);

    let original_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::set_current_dir(path).unwrap();

    stax::commands::insert::run(stax::InsertPosition::Above, Some("feat-b"), true)
        .await
        .unwrap();
    std::env::set_current_dir(&original_dir).unwrap();

    // Should still be on feat-a
    assert_eq!(repo.current_branch().unwrap(), "feat-a");
}

/// Load cache and return a StackCache (caller inspects via data_ref()).
fn load_cache(git_dir: &std::path::Path) -> stax::cache::StackCache {
    let mut cache = stax::cache::StackCache::new(git_dir);
    cache.load().expect("cache should load");
    cache
}
