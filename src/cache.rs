use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Serialised types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct CacheFile {
    pub schema_version: u32,
    pub trunk: TrunkInfo,
    #[serde(default)]
    pub branches: HashMap<String, CachedBranch>,
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
    pub fn build_cache_data(
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
                branches.insert(
                    name.clone(),
                    CachedBranch {
                        tip: tip.clone(),
                        parent: parent.clone(),
                    },
                );
            }
        }

        CacheFile {
            schema_version: SCHEMA_VERSION,
            trunk: TrunkInfo {
                name: trunk_name.to_string(),
                tip: trunk_tip.to_string(),
                merged: merged.iter().cloned().collect(),
            },
            branches,
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
            },
        );
        branches.insert(
            "feature-b".to_string(),
            CachedBranch {
                tip: "bbb222".to_string(),
                parent: Some("feature-a".to_string()),
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
        let mut parent_map = HashMap::new();
        parent_map.insert("main".to_string(), None);
        parent_map.insert("feat".to_string(), Some("main".to_string()));

        let mut commits = HashMap::new();
        commits.insert("main".to_string(), "trunk-tip".to_string());
        commits.insert("feat".to_string(), "feat-tip".to_string());

        let mut merged = HashSet::new();
        merged.insert("old".to_string());

        let data =
            StackCache::build_cache_data("main", "trunk-tip", &parent_map, &commits, &merged);

        assert_eq!(data.schema_version, SCHEMA_VERSION);
        assert_eq!(data.trunk.name, "main");
        assert_eq!(data.trunk.tip, "trunk-tip");
        assert!(data.trunk.merged.contains(&"old".to_string()));
        assert_eq!(data.branches["feat"].tip, "feat-tip");
        assert_eq!(data.branches["feat"].parent, Some("main".to_string()));
    }

    #[test]
    fn test_validate_no_data_returns_none() {
        let dir = make_git_dir();
        let cache = StackCache::new(dir.path());
        // Never loaded — data is None
        assert!(cache.validate(&HashMap::new(), "main").is_none());
    }
}
