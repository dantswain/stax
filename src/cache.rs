use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// Serialised types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct CacheFile {
    pub schema_version: u32,
    pub trunk: TrunkInfo,
    #[serde(default)]
    pub branches: HashMap<String, CachedBranch>,
    #[serde(default)]
    pub pull_requests: HashMap<String, CachedPullRequest>,
    #[serde(default)]
    pub shadow_branches: HashMap<String, ShadowBranch>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrunkInfo {
    pub name: String,
    pub tip: String,
    #[serde(default)]
    pub merged: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedBranch {
    pub tip: String,
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merge_sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowBranch {
    pub consumer: String,
    pub sources: Vec<String>,
    pub tip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPullRequest {
    pub number: u64,
    pub state: String,
    pub head_ref: String,
    pub base_ref: String,
    pub html_url: String,
    pub draft: bool,
}

// ---------------------------------------------------------------------------
// Restack state (transient, persisted across --continue)
// ---------------------------------------------------------------------------

/// Persisted state for a multi-branch restack operation.
/// Written at the start of a restack and loaded by `--continue` so that
/// the original pre-restack branch tips are preserved across conflict
/// resolution cycles.
#[derive(Debug, Serialize, Deserialize)]
pub struct RestackState {
    /// Branch tips captured BEFORE any rebasing started in this restack run.
    /// Used as the `--onto` base to correctly identify each branch's own commits.
    pub old_tips: HashMap<String, String>,
    /// The branch the user was on when the restack started.
    pub original_branch: String,
}

// ---------------------------------------------------------------------------
// Validation result
// ---------------------------------------------------------------------------

/// Result of comparing cached state against the live branch tips.
pub struct ValidationResult {
    /// Branches whose cached tip still matches — their parent is trustworthy.
    pub valid: HashMap<String, CachedBranch>,
    /// Branches whose tip has changed — parent must be recomputed.
    pub stale: HashSet<String>,
    /// Branches present in git but absent from cache.
    pub new_branches: HashSet<String>,
    /// Branches present in cache but absent from git.
    pub deleted: HashSet<String>,
    /// True when the trunk tip (or name) has changed, invalidating the
    /// merged set.
    pub trunk_changed: bool,
    /// The cached merged set.  Only meaningful when `!trunk_changed`.
    pub cached_merged: HashSet<String>,
}

// ---------------------------------------------------------------------------
// StackCache
// ---------------------------------------------------------------------------

pub struct StackCache {
    cache_path: PathBuf,
    data: Option<CacheFile>,
}

impl StackCache {
    /// Create a new handle.  `git_dir` is the `.git/` directory path.
    pub fn new(git_dir: &Path) -> Self {
        StackCache {
            cache_path: git_dir.join("stax").join("cache.json"),
            data: None,
        }
    }

    /// Borrow the loaded cache data (if any).
    pub fn data_ref(&self) -> Option<&CacheFile> {
        self.data.as_ref()
    }

    /// Load the cache from disk.  Returns `None` on any failure (missing
    /// file, corrupt JSON, wrong schema version).
    pub fn load(&mut self) -> Option<&CacheFile> {
        log::debug!("cache: loading from {}", self.cache_path.display());

        let content = match fs::read_to_string(&self.cache_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::debug!("cache: file not found");
                return None;
            }
            Err(e) => {
                log::debug!("cache: read error: {e}");
                return None;
            }
        };

        let data: CacheFile = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(e) => {
                log::debug!("cache: parse error: {e}");
                return None;
            }
        };

        if data.schema_version != SCHEMA_VERSION {
            log::debug!(
                "cache: schema version mismatch (got {}, expected {SCHEMA_VERSION})",
                data.schema_version
            );
            return None;
        }

        self.data = Some(data);
        self.data.as_ref()
    }

    /// Validate cached state against live branch tips.
    ///
    /// `live_tips` maps branch names to their current commit hashes.
    /// `trunk_name` is the configured trunk branch.
    pub fn validate(
        &self,
        live_tips: &HashMap<String, String>,
        trunk_name: &str,
    ) -> Option<ValidationResult> {
        let data = self.data.as_ref()?;

        // Trunk changed?
        let trunk_tip = live_tips.get(trunk_name).map(|s| s.as_str());
        let trunk_changed =
            data.trunk.name != trunk_name || trunk_tip != Some(data.trunk.tip.as_str());

        let mut valid = HashMap::new();
        let mut stale = HashSet::new();
        let mut new_branches = HashSet::new();
        let mut deleted = HashSet::new();

        // Walk live branches
        for (name, tip) in live_tips {
            if name == trunk_name {
                continue; // trunk is handled separately
            }
            match data.branches.get(name) {
                Some(cached) if cached.tip == *tip => {
                    valid.insert(name.clone(), cached.clone());
                }
                Some(_) => {
                    stale.insert(name.clone());
                }
                None => {
                    new_branches.insert(name.clone());
                }
            }
        }

        // Walk cached branches to detect deletions
        for name in data.branches.keys() {
            if !live_tips.contains_key(name) {
                deleted.insert(name.clone());
            }
        }

        let cached_merged = if trunk_changed {
            HashSet::new()
        } else {
            data.trunk.merged.iter().cloned().collect()
        };

        log::debug!(
            "cache: {} valid, {} stale, {} new, {} deleted, trunk_changed={}",
            valid.len(),
            stale.len(),
            new_branches.len(),
            deleted.len(),
            trunk_changed,
        );
        if !stale.is_empty() {
            let names: Vec<&String> = stale.iter().collect();
            log::debug!("cache: stale branches: {names:?}");
        }
        if !new_branches.is_empty() {
            let names: Vec<&String> = new_branches.iter().collect();
            log::debug!("cache: new branches: {names:?}");
        }
        if !deleted.is_empty() {
            let names: Vec<&String> = deleted.iter().collect();
            log::debug!("cache: deleted branches: {names:?}");
        }

        Some(ValidationResult {
            valid,
            stale,
            new_branches,
            deleted,
            trunk_changed,
            cached_merged,
        })
    }

    /// Load the cache, insert or update a single branch entry, and save.
    /// No-op if no cache file exists yet (let a full recompute initialise it).
    pub fn upsert_branch(&mut self, branch: &str, tip: &str, parent: Option<&str>) {
        if self.load().is_none() {
            log::debug!("cache: upsert_branch skipped — no existing cache");
            return;
        }
        let mut data = self.data.take().unwrap();
        log::debug!(
            "cache: upsert_branch '{}' tip={} parent={:?}",
            branch,
            &tip[..tip.len().min(8)],
            parent
        );
        // Preserve existing merge_sources if present
        let merge_sources = data
            .branches
            .get(branch)
            .map(|b| b.merge_sources.clone())
            .unwrap_or_default();
        data.branches.insert(
            branch.to_string(),
            CachedBranch {
                tip: tip.to_string(),
                parent: parent.map(|s| s.to_string()),
                merge_sources,
            },
        );
        self.save(&data);
        self.data = Some(data);
    }

    /// Insert or update a shadow branch entry, and save.
    pub fn upsert_shadow(&mut self, shadow_name: &str, shadow: ShadowBranch) {
        if self.load().is_none() {
            log::debug!("cache: upsert_shadow skipped — no existing cache");
            return;
        }
        let mut data = self.data.take().unwrap();
        log::debug!(
            "cache: upsert_shadow '{}' consumer='{}' sources={:?}",
            shadow_name,
            shadow.consumer,
            shadow.sources,
        );
        data.shadow_branches.insert(shadow_name.to_string(), shadow);
        self.save(&data);
        self.data = Some(data);
    }

    /// Remove a shadow branch entry.
    pub fn remove_shadow(&mut self, shadow_name: &str) {
        if self.load().is_none() {
            return;
        }
        let mut data = self.data.take().unwrap();
        if data.shadow_branches.remove(shadow_name).is_some() {
            log::debug!("cache: removed shadow '{}'", shadow_name);
            self.save(&data);
        }
        self.data = Some(data);
    }

    /// Look up the shadow branch for a consumer branch.
    pub fn get_shadow_for_consumer(&self, consumer: &str) -> Option<(&String, &ShadowBranch)> {
        let data = self.data.as_ref()?;
        data.shadow_branches
            .iter()
            .find(|(_, sb)| sb.consumer == consumer)
    }

    /// Return the shadow branch name for a consumer.
    pub fn shadow_name_for(consumer: &str) -> String {
        format!("stax/shadow/{consumer}")
    }

    /// Mutably borrow the loaded cache data (if any).
    pub fn data_mut(&mut self) -> Option<&mut CacheFile> {
        self.data.as_mut()
    }

    /// Save the currently loaded data back to disk.
    pub fn save_current(&self) {
        if let Some(data) = &self.data {
            self.save(data);
        }
    }

    /// Persist cache data to disk.  Failures are logged and swallowed.
    pub fn save(&self, data: &CacheFile) {
        if let Some(parent) = self.cache_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                log::warn!("cache: failed to create directory: {e}");
                return;
            }
        }

        let json = match serde_json::to_string_pretty(data) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("cache: serialization failed: {e}");
                return;
            }
        };

        if let Err(e) = fs::write(&self.cache_path, json) {
            log::warn!("cache: save failed: {e}");
        } else {
            log::debug!("cache: saved to {}", self.cache_path.display());
        }
    }

    /// Build a `CacheFile` from freshly-computed results.
    /// Preserves any existing cached PR data for branches that are still present.
    pub fn build_cache_data(
        &self,
        trunk_name: &str,
        trunk_tip: &str,
        parent_map: &HashMap<String, Option<String>>,
        commits: &HashMap<String, String>,
        merged: &HashSet<String>,
    ) -> CacheFile {
        let mut branches = HashMap::new();
        for (name, parent) in parent_map {
            // Skip trunk branches — they don't have parents in the cache
            if let Some(tip) = commits.get(name) {
                // Carry forward merge_sources from existing cache
                let merge_sources = self
                    .data
                    .as_ref()
                    .and_then(|d| d.branches.get(name))
                    .map(|b| b.merge_sources.clone())
                    .unwrap_or_default();
                branches.insert(
                    name.clone(),
                    CachedBranch {
                        tip: tip.clone(),
                        parent: parent.clone(),
                        merge_sources,
                    },
                );
            }
        }

        // Carry forward cached PR data for branches that still exist
        let pull_requests = self
            .data
            .as_ref()
            .map(|d| {
                d.pull_requests
                    .iter()
                    .filter(|(head, _)| branches.contains_key(*head))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();

        // Carry forward shadow branches (they are managed by `include` / `sync`)
        let shadow_branches = self
            .data
            .as_ref()
            .map(|d| d.shadow_branches.clone())
            .unwrap_or_default();

        CacheFile {
            schema_version: SCHEMA_VERSION,
            trunk: TrunkInfo {
                name: trunk_name.to_string(),
                tip: trunk_tip.to_string(),
                merged: merged.iter().cloned().collect(),
            },
            branches,
            pull_requests,
            shadow_branches,
        }
    }

    /// Replace the cached PR data with a fresh set (typically from a bulk
    /// `get_open_pull_requests()` call).  Only PRs whose head_ref exists in the
    /// branch cache are stored.  Requires an existing cache file.
    pub fn save_pull_requests(&mut self, prs: &HashMap<String, CachedPullRequest>) {
        if self.load().is_none() {
            log::debug!("cache: save_pull_requests skipped — no existing cache");
            return;
        }
        let mut data = self.data.take().unwrap();
        data.pull_requests = prs
            .iter()
            .filter(|(head, _)| data.branches.contains_key(*head))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        log::debug!("cache: saved {} pull requests", data.pull_requests.len());
        self.save(&data);
        self.data = Some(data);
    }

    // -----------------------------------------------------------------------
    // Restack state persistence
    // -----------------------------------------------------------------------

    /// Path to the restack state file (`.git/stax/restack-state.json`).
    fn restack_state_path(&self) -> PathBuf {
        self.cache_path
            .parent()
            .expect("cache_path has a parent")
            .join("restack-state.json")
    }

    /// Persist restack state to disk for `--continue` to pick up.
    /// Failures are logged and swallowed.
    pub fn save_restack_state(&self, state: &RestackState) {
        let path = self.restack_state_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                log::warn!("restack-state: failed to create directory: {e}");
                return;
            }
        }
        let json = match serde_json::to_string_pretty(state) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("restack-state: serialization failed: {e}");
                return;
            }
        };
        if let Err(e) = fs::write(&path, json) {
            log::warn!("restack-state: save failed: {e}");
        } else {
            log::debug!(
                "restack-state: saved {} old_tips to {}",
                state.old_tips.len(),
                path.display()
            );
        }
    }

    /// Load persisted restack state.  Returns `None` on any failure.
    pub fn load_restack_state(&self) -> Option<RestackState> {
        let path = self.restack_state_path();
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::debug!("restack-state: file not found");
                return None;
            }
            Err(e) => {
                log::debug!("restack-state: read error: {e}");
                return None;
            }
        };
        match serde_json::from_str(&content) {
            Ok(state) => {
                log::debug!("restack-state: loaded from {}", path.display());
                Some(state)
            }
            Err(e) => {
                log::debug!("restack-state: parse error: {e}");
                None
            }
        }
    }

    /// Delete the restack state file (called on successful completion or fresh start).
    pub fn clear_restack_state(&self) {
        let path = self.restack_state_path();
        match fs::remove_file(&path) {
            Ok(()) => log::debug!("restack-state: cleared {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::debug!("restack-state: remove failed: {e}"),
        }
    }

    /// Convert cached data into the `HashMap<String, Option<String>>` format
    /// used by navigate commands.  Main/trunk branches get `None`.
    pub fn to_parent_map(data: &CacheFile) -> HashMap<String, Option<String>> {
        let mut map = HashMap::new();
        // Trunk entry
        map.insert(data.trunk.name.clone(), None);
        // Branch entries
        for (name, cached) in &data.branches {
            map.insert(name.clone(), cached.parent.clone());
        }
        map
    }

    /// Extract the merged set from cache.
    pub fn to_merged_set(data: &CacheFile) -> HashSet<String> {
        data.trunk.merged.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_git_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("create temp dir")
    }

    fn sample_cache_data() -> CacheFile {
        let mut branches = HashMap::new();
        branches.insert(
            "feature-a".to_string(),
            CachedBranch {
                tip: "aaa111".to_string(),
                parent: Some("main".to_string()),
                merge_sources: Vec::new(),
            },
        );
        branches.insert(
            "feature-b".to_string(),
            CachedBranch {
                tip: "bbb222".to_string(),
                parent: Some("feature-a".to_string()),
                merge_sources: Vec::new(),
            },
        );

        CacheFile {
            schema_version: SCHEMA_VERSION,
            trunk: TrunkInfo {
                name: "main".to_string(),
                tip: "trunk000".to_string(),
                merged: vec!["old-branch".to_string()],
            },
            branches,
            pull_requests: HashMap::new(),
            shadow_branches: HashMap::new(),
        }
    }

    fn live_tips_matching(data: &CacheFile) -> HashMap<String, String> {
        let mut tips = HashMap::new();
        tips.insert(data.trunk.name.clone(), data.trunk.tip.clone());
        for (name, cached) in &data.branches {
            tips.insert(name.clone(), cached.tip.clone());
        }
        tips
    }

    #[test]
    fn test_load_missing_file() {
        let dir = make_git_dir();
        let mut cache = StackCache::new(dir.path());
        assert!(cache.load().is_none());
    }

    #[test]
    fn test_load_corrupt_json() {
        let dir = make_git_dir();
        let stax_dir = dir.path().join("stax");
        fs::create_dir_all(&stax_dir).unwrap();
        fs::write(stax_dir.join("cache.json"), "not json at all {{{").unwrap();

        let mut cache = StackCache::new(dir.path());
        assert!(cache.load().is_none());
    }

    #[test]
    fn test_load_wrong_schema_version() {
        let dir = make_git_dir();
        let stax_dir = dir.path().join("stax");
        fs::create_dir_all(&stax_dir).unwrap();
        fs::write(
            stax_dir.join("cache.json"),
            r#"{"schema_version":999,"trunk":{"name":"main","tip":"x","merged":[]},"branches":{}}"#,
        )
        .unwrap();

        let mut cache = StackCache::new(dir.path());
        assert!(cache.load().is_none());
    }

    #[test]
    fn test_roundtrip() {
        let dir = make_git_dir();
        let data = sample_cache_data();

        let cache = StackCache::new(dir.path());
        cache.save(&data);

        let mut cache2 = StackCache::new(dir.path());
        let loaded = cache2.load().expect("should load");

        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        assert_eq!(loaded.trunk.name, "main");
        assert_eq!(loaded.trunk.tip, "trunk000");
        assert_eq!(loaded.trunk.merged, vec!["old-branch"]);
        assert_eq!(loaded.branches.len(), 2);
        assert_eq!(loaded.branches["feature-a"].tip, "aaa111");
        assert_eq!(
            loaded.branches["feature-a"].parent,
            Some("main".to_string())
        );
        assert_eq!(loaded.branches["feature-b"].tip, "bbb222");
        assert_eq!(
            loaded.branches["feature-b"].parent,
            Some("feature-a".to_string())
        );
    }

    #[test]
    fn test_validate_all_valid() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let tips = live_tips_matching(&data);

        let cache = StackCache::new(dir.path());
        cache.save(&data);

        let mut cache2 = StackCache::new(dir.path());
        cache2.load();

        let result = cache2.validate(&tips, "main").expect("should validate");
        assert_eq!(result.valid.len(), 2);
        assert!(result.stale.is_empty());
        assert!(result.new_branches.is_empty());
        assert!(result.deleted.is_empty());
        assert!(!result.trunk_changed);
        assert!(result.cached_merged.contains("old-branch"));
    }

    #[test]
    fn test_validate_stale_branch() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let mut tips = live_tips_matching(&data);
        tips.insert("feature-a".to_string(), "changed-tip".to_string());

        let cache = StackCache::new(dir.path());
        cache.save(&data);

        let mut cache2 = StackCache::new(dir.path());
        cache2.load();

        let result = cache2.validate(&tips, "main").expect("should validate");
        assert_eq!(result.valid.len(), 1); // feature-b still valid
        assert!(result.stale.contains("feature-a"));
        assert!(result.new_branches.is_empty());
        assert!(result.deleted.is_empty());
    }

    #[test]
    fn test_validate_new_branch() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let mut tips = live_tips_matching(&data);
        tips.insert("feature-c".to_string(), "ccc333".to_string());

        let cache = StackCache::new(dir.path());
        cache.save(&data);

        let mut cache2 = StackCache::new(dir.path());
        cache2.load();

        let result = cache2.validate(&tips, "main").expect("should validate");
        assert_eq!(result.valid.len(), 2);
        assert!(result.new_branches.contains("feature-c"));
        assert!(result.deleted.is_empty());
    }

    #[test]
    fn test_validate_deleted_branch() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let mut tips = live_tips_matching(&data);
        tips.remove("feature-b");

        let cache = StackCache::new(dir.path());
        cache.save(&data);

        let mut cache2 = StackCache::new(dir.path());
        cache2.load();

        let result = cache2.validate(&tips, "main").expect("should validate");
        assert_eq!(result.valid.len(), 1); // feature-a only
        assert!(result.deleted.contains("feature-b"));
    }

    #[test]
    fn test_validate_trunk_changed() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let mut tips = live_tips_matching(&data);
        tips.insert("main".to_string(), "new-trunk-tip".to_string());

        let cache = StackCache::new(dir.path());
        cache.save(&data);

        let mut cache2 = StackCache::new(dir.path());
        cache2.load();

        let result = cache2.validate(&tips, "main").expect("should validate");
        assert!(result.trunk_changed);
        assert!(result.cached_merged.is_empty()); // invalidated
    }

    #[test]
    fn test_validate_trunk_name_changed() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let mut tips = live_tips_matching(&data);
        // Simulate switching from "main" to "master"
        tips.insert("master".to_string(), "trunk000".to_string());

        let cache = StackCache::new(dir.path());
        cache.save(&data);

        let mut cache2 = StackCache::new(dir.path());
        cache2.load();

        let result = cache2.validate(&tips, "master").expect("should validate");
        assert!(result.trunk_changed);
    }

    #[test]
    fn test_to_parent_map() {
        let data = sample_cache_data();
        let map = StackCache::to_parent_map(&data);

        assert_eq!(map.get("main"), Some(&None));
        assert_eq!(map.get("feature-a"), Some(&Some("main".to_string())));
        assert_eq!(map.get("feature-b"), Some(&Some("feature-a".to_string())));
    }

    #[test]
    fn test_to_merged_set() {
        let data = sample_cache_data();
        let merged = StackCache::to_merged_set(&data);
        assert!(merged.contains("old-branch"));
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn test_save_creates_directory() {
        let dir = make_git_dir();
        let stax_dir = dir.path().join("stax");
        assert!(!stax_dir.exists());

        let cache = StackCache::new(dir.path());
        cache.save(&sample_cache_data());

        assert!(stax_dir.exists());
        assert!(stax_dir.join("cache.json").exists());
    }

    #[test]
    fn test_save_overwrites() {
        let dir = make_git_dir();

        let data1 = sample_cache_data();
        let cache = StackCache::new(dir.path());
        cache.save(&data1);

        // Save different data
        let mut data2 = sample_cache_data();
        data2.trunk.tip = "changed-tip".to_string();
        cache.save(&data2);

        let mut cache2 = StackCache::new(dir.path());
        let loaded = cache2.load().expect("should load");
        assert_eq!(loaded.trunk.tip, "changed-tip");
    }

    #[test]
    fn test_build_cache_data() {
        let dir = make_git_dir();
        let cache = StackCache::new(dir.path());

        let mut parent_map = HashMap::new();
        parent_map.insert("main".to_string(), None);
        parent_map.insert("feat".to_string(), Some("main".to_string()));

        let mut commits = HashMap::new();
        commits.insert("main".to_string(), "trunk-tip".to_string());
        commits.insert("feat".to_string(), "feat-tip".to_string());

        let mut merged = HashSet::new();
        merged.insert("old".to_string());

        let data = cache.build_cache_data("main", "trunk-tip", &parent_map, &commits, &merged);

        assert_eq!(data.schema_version, SCHEMA_VERSION);
        assert_eq!(data.trunk.name, "main");
        assert_eq!(data.trunk.tip, "trunk-tip");
        assert!(data.trunk.merged.contains(&"old".to_string()));
        assert_eq!(data.branches["feat"].tip, "feat-tip");
        assert_eq!(data.branches["feat"].parent, Some("main".to_string()));
        assert!(data.pull_requests.is_empty());
    }

    #[test]
    fn test_build_cache_data_preserves_prs() {
        let dir = make_git_dir();

        // Start with a cache that has PR data
        let mut initial = sample_cache_data();
        initial.pull_requests.insert(
            "feature-a".to_string(),
            CachedPullRequest {
                number: 42,
                state: "open".to_string(),
                head_ref: "feature-a".to_string(),
                base_ref: "main".to_string(),
                html_url: "https://github.com/o/r/pull/42".to_string(),
                draft: false,
            },
        );
        let cache = StackCache::new(dir.path());
        cache.save(&initial);

        // Load and rebuild — PR data should carry through
        let mut cache2 = StackCache::new(dir.path());
        cache2.load();

        let mut parent_map = HashMap::new();
        parent_map.insert("feature-a".to_string(), Some("main".to_string()));
        let mut commits = HashMap::new();
        commits.insert("feature-a".to_string(), "aaa111".to_string());
        let merged = HashSet::new();

        let data = cache2.build_cache_data("main", "trunk000", &parent_map, &commits, &merged);
        assert_eq!(data.pull_requests.len(), 1);
        assert_eq!(data.pull_requests["feature-a"].number, 42);
    }

    #[test]
    fn test_save_pull_requests() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let cache = StackCache::new(dir.path());
        cache.save(&data);

        let mut prs = HashMap::new();
        prs.insert(
            "feature-a".to_string(),
            CachedPullRequest {
                number: 10,
                state: "open".to_string(),
                head_ref: "feature-a".to_string(),
                base_ref: "main".to_string(),
                html_url: "https://github.com/o/r/pull/10".to_string(),
                draft: false,
            },
        );
        // This PR's head_ref doesn't exist in branches — should be filtered out
        prs.insert(
            "nonexistent".to_string(),
            CachedPullRequest {
                number: 99,
                state: "open".to_string(),
                head_ref: "nonexistent".to_string(),
                base_ref: "main".to_string(),
                html_url: "https://github.com/o/r/pull/99".to_string(),
                draft: false,
            },
        );

        let mut cache2 = StackCache::new(dir.path());
        cache2.save_pull_requests(&prs);

        let mut cache3 = StackCache::new(dir.path());
        let loaded = cache3.load().expect("should load");
        assert_eq!(loaded.pull_requests.len(), 1);
        assert_eq!(loaded.pull_requests["feature-a"].number, 10);
    }

    #[test]
    fn test_validate_no_data_returns_none() {
        let dir = make_git_dir();
        let cache = StackCache::new(dir.path());
        // Never loaded — data is None
        assert!(cache.validate(&HashMap::new(), "main").is_none());
    }

    #[test]
    fn test_upsert_branch_existing_cache() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let cache = StackCache::new(dir.path());
        cache.save(&data);

        // Upsert a brand-new branch
        let mut cache2 = StackCache::new(dir.path());
        cache2.upsert_branch("feature-c", "ccc333", Some("feature-a"));

        // Reload and verify
        let mut cache3 = StackCache::new(dir.path());
        let loaded = cache3.load().expect("should load");
        assert_eq!(loaded.branches.len(), 3);
        assert_eq!(loaded.branches["feature-c"].tip, "ccc333");
        assert_eq!(
            loaded.branches["feature-c"].parent,
            Some("feature-a".to_string())
        );
        // Existing entries should be untouched
        assert_eq!(loaded.branches["feature-a"].tip, "aaa111");
    }

    #[test]
    fn test_upsert_branch_updates_existing() {
        let dir = make_git_dir();
        let data = sample_cache_data();
        let cache = StackCache::new(dir.path());
        cache.save(&data);

        // Upsert an existing branch with new parent
        let mut cache2 = StackCache::new(dir.path());
        cache2.upsert_branch("feature-b", "bbb222", Some("main"));

        let mut cache3 = StackCache::new(dir.path());
        let loaded = cache3.load().expect("should load");
        assert_eq!(loaded.branches.len(), 2);
        assert_eq!(
            loaded.branches["feature-b"].parent,
            Some("main".to_string())
        );
    }

    #[test]
    fn test_upsert_branch_no_cache_is_noop() {
        let dir = make_git_dir();
        let cache_path = dir.path().join("stax").join("cache.json");
        assert!(!cache_path.exists());

        // Upsert when no cache file exists — should be a no-op
        let mut cache = StackCache::new(dir.path());
        cache.upsert_branch("feature-x", "xxx999", Some("main"));

        // No file should have been created
        assert!(!cache_path.exists());
    }

    // -----------------------------------------------------------------------
    // RestackState tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_restack_state_roundtrip() {
        let dir = make_git_dir();
        let cache = StackCache::new(dir.path());

        let mut old_tips = HashMap::new();
        old_tips.insert("branch-a".to_string(), "aaa111".to_string());
        old_tips.insert("branch-b".to_string(), "bbb222".to_string());
        old_tips.insert("main".to_string(), "mmm000".to_string());

        let state = RestackState {
            old_tips: old_tips.clone(),
            original_branch: "branch-b".to_string(),
        };
        cache.save_restack_state(&state);

        let loaded = cache.load_restack_state().expect("should load");
        assert_eq!(loaded.old_tips, old_tips);
        assert_eq!(loaded.original_branch, "branch-b");
    }

    #[test]
    fn test_restack_state_clear() {
        let dir = make_git_dir();
        let cache = StackCache::new(dir.path());

        let state = RestackState {
            old_tips: HashMap::new(),
            original_branch: "main".to_string(),
        };
        cache.save_restack_state(&state);
        assert!(cache.load_restack_state().is_some());

        cache.clear_restack_state();
        assert!(cache.load_restack_state().is_none());
    }

    #[test]
    fn test_restack_state_missing_file() {
        let dir = make_git_dir();
        let cache = StackCache::new(dir.path());
        assert!(cache.load_restack_state().is_none());
    }
}
