use crate::cache::{CachedPullRequest, RestackState, StackCache};
use crate::commands::navigate::get_branches_and_parent_map;
use crate::git::GitRepo;
use crate::github::{GitHubClient, PullRequest};
use crate::token_store;
use crate::utils;
use anyhow::{anyhow, Result};
use colored::*;
use dialoguer::{theme::ColorfulTheme, FuzzySelect};
use std::collections::{HashMap, HashSet};

const MAIN_BRANCHES: &[&str] = &["main", "master", "develop"];

pub async fn run(check: bool, continue_repair: bool) -> Result<()> {
    log::debug!("repair: check={}, continue={}", check, continue_repair);
    let git = GitRepo::open(".")?;

    if continue_repair {
        if git.is_rebase_in_progress() {
            utils::print_info("Continuing rebase...");
            git.rebase_continue()?;
            utils::print_success("Rebase continued successfully");
        }
        // State file is preserved — do_repair will load it.
        // Load cached PRs (persisted by fetch_live_prs on the initial run).
        let prs = load_cached_prs(&git)?;
        return do_repair(&git, false, &prs).await;
    }

    // Fresh start — clear any stale state
    StackCache::new(&git.git_dir()).clear_restack_state();

    let prs = fetch_live_prs(&git).await?;
    do_repair(&git, check, &prs).await
}

/// Core repair logic.  Accepts pre-fetched PR data so that tests can
/// supply synthetic PRs without hitting the GitHub API.
pub async fn do_repair(git: &GitRepo, check: bool, prs: &[PullRequest]) -> Result<()> {
    if git.is_rebase_in_progress() {
        return Err(anyhow!(
            "A rebase is currently in progress.\n\
             Resolve conflicts and run 'stax repair --continue', or 'git rebase --abort' to cancel."
        ));
    }
    if !git.is_clean()? {
        return Err(anyhow!(
            "Working directory has uncommitted changes. Please commit or stash them first."
        ));
    }

    let current_branch = git.current_branch()?;

    if prs.is_empty() {
        utils::print_info("No open PRs found — nothing to repair");
        return Ok(());
    }

    let all_branches = git.get_branches()?;
    let branch_set: HashSet<&str> = all_branches.iter().map(|s| s.as_str()).collect();

    // Filter to PRs whose head_ref and base_ref both exist locally
    let local_prs: Vec<&PullRequest> = prs
        .iter()
        .filter(|pr| {
            branch_set.contains(pr.head_ref.as_str()) && branch_set.contains(pr.base_ref.as_str())
        })
        .collect();

    // Step 2: Build expected parent map from PR chain
    let mut expected_parents: HashMap<String, String> = HashMap::new();

    // 2a: Branches with PRs — expected parent is pr.base_ref
    for pr in &local_prs {
        expected_parents.insert(pr.head_ref.clone(), pr.base_ref.clone());
    }

    // 2b: Infer parents for gap branches (PR bases without their own PR)
    let inferred = infer_gap_branch_parents(&local_prs, &branch_set)?;
    for (branch, parent) in &inferred {
        expected_parents.insert(branch.clone(), parent.clone());
    }

    if expected_parents.is_empty() {
        utils::print_info("No branches with PR relationships found — nothing to repair");
        return Ok(());
    }

    // Step 3: Detect mismatches
    //
    // A branch needs repair if its *detected* parent (from the merge-base
    // heuristic + PR overrides) doesn't match the *expected* parent from
    // PR data.  Branches that are merely out-of-date on the correct parent
    // are not topology errors — `stax restack` handles those.
    utils::print_info("Checking branch topology against PR data...");
    println!();

    let (_, _, _, parent_map) = get_branches_and_parent_map(git)?;

    let mut needs_repair: Vec<(String, String)> = Vec::new();

    // Sort branches for consistent output (topological: parents before children)
    let sorted = topological_sort(&expected_parents);

    for (branch, expected_parent) in &sorted {
        // Skip shadow branches — they are managed by `include`
        if crate::commands::navigate::is_shadow_branch(branch) {
            continue;
        }
        let current_parent = parent_map
            .get(branch)
            .and_then(|p| p.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let is_ok = current_parent == *expected_parent;

        if is_ok {
            if check {
                println!(
                    "  {} {}: OK (based on {})",
                    "✓".green(),
                    branch.bold(),
                    expected_parent.cyan()
                );
            }
        } else {
            let source = if inferred.contains_key(branch) {
                "inferred from PR chain"
            } else {
                "from PR base_ref"
            };

            println!("  {} {}: {}", "✗".red(), branch.bold(), "misplaced".red());
            println!("      Current parent: {}", current_parent.yellow());
            println!(
                "      Expected parent: {} ({})",
                expected_parent.green(),
                source
            );

            needs_repair.push((branch.clone(), expected_parent.clone()));
        }
    }

    println!();

    if needs_repair.is_empty() {
        utils::print_success("All branches have correct topology");
        return Ok(());
    }

    if check {
        println!(
            "{} branch(es) need repair. Run '{}' to fix.",
            needs_repair.len(),
            "stax repair".bold()
        );
        return Ok(());
    }

    // Step 4: Repair in topological order
    let cache = StackCache::new(&git.git_dir());

    // Load persisted old_tips (from --continue) or compute fresh
    let old_tips = {
        let persisted = cache.load_restack_state();
        let mut tips = persisted.map(|s| s.old_tips).unwrap_or_default();
        let had_persisted = !tips.is_empty();
        for (branch, parent) in &needs_repair {
            for name in [branch, parent] {
                if !tips.contains_key(name) {
                    if let Ok(hash) = git.get_commit_hash(&format!("refs/heads/{name}")) {
                        tips.insert(name.clone(), hash);
                    }
                }
            }
        }
        if had_persisted {
            log::debug!(
                "repair: loaded persisted old_tips, merged to {} entries",
                tips.len()
            );
        } else {
            log::debug!("repair: computed fresh old_tips ({} entries)", tips.len());
        }
        tips
    };

    // Persist for potential --continue
    cache.save_restack_state(&RestackState {
        old_tips: old_tips.clone(),
        original_branch: current_branch.clone(),
    });

    let mut repaired = Vec::new();

    for (branch, expected_parent) in &needs_repair {
        utils::print_info(&format!("Rebasing '{}' onto '{}'", branch, expected_parent));

        // Use merge-base with the expected parent as the upstream for --onto.
        // git rebase --onto will auto-skip commits whose diffs are already in the target.
        let merge_base = git.get_merge_base(branch, expected_parent)?;
        let old_base = merge_base.to_string();

        git.rebase_onto_with_base(
            branch,
            expected_parent,
            Some(&old_base),
            Some("stax repair --continue"),
        )?;
        repaired.push(branch.as_str());
    }

    // Success — clean up state file
    cache.clear_restack_state();

    // Restore original branch
    git.checkout_branch(&current_branch)?;

    // Refresh cache
    log::debug!("repair: refreshing cache");
    let _ = get_branches_and_parent_map(git);

    println!();
    for branch in &repaired {
        utils::print_success(&format!("Repaired '{}'", branch));
    }
    utils::print_success("All branches repaired successfully");

    Ok(())
}

/// Check cached PR data for topology mismatches without fetching from GitHub.
/// Returns a list of (branch, current_parent, expected_parent) tuples.
/// Used by other commands to warn users about broken topology.
pub fn check_topology_from_cache(
    git: &GitRepo,
    parent_map: &HashMap<String, Option<String>>,
) -> Vec<(String, String, String)> {
    let mut cache = StackCache::new(&git.git_dir());
    let cached_prs = match cache.load() {
        Some(data) => data.pull_requests.clone(),
        None => return Vec::new(),
    };
    if cached_prs.is_empty() {
        return Vec::new();
    }

    let all_branches: Vec<String> = parent_map.keys().cloned().collect();
    let branch_set: HashSet<&str> = all_branches.iter().map(|s| s.as_str()).collect();

    // Convert cached PRs to PullRequest references for reuse of existing logic
    let prs: Vec<PullRequest> = cached_prs
        .values()
        .filter(|cpr| cpr.state == "open")
        .map(|cpr| PullRequest {
            number: cpr.number,
            title: String::new(),
            body: None,
            state: cpr.state.clone(),
            head_ref: cpr.head_ref.clone(),
            base_ref: cpr.base_ref.clone(),
            html_url: cpr.html_url.clone(),
            draft: cpr.draft,
        })
        .collect();

    let local_prs: Vec<&PullRequest> = prs
        .iter()
        .filter(|pr| {
            branch_set.contains(pr.head_ref.as_str()) && branch_set.contains(pr.base_ref.as_str())
        })
        .collect();

    if local_prs.is_empty() {
        return Vec::new();
    }

    // Build expected parents from PR chain (same logic as do_repair)
    let mut expected_parents: HashMap<String, String> = HashMap::new();
    for pr in &local_prs {
        expected_parents.insert(pr.head_ref.clone(), pr.base_ref.clone());
    }

    // Infer gap branch parents (silently — no interactive prompts)
    if let Ok(inferred) = infer_gap_branch_parents_silent(&local_prs, &branch_set) {
        for (branch, parent) in inferred {
            expected_parents.insert(branch, parent);
        }
    }

    // Compare against current parent_map (skip shadow branches)
    let mut mismatches = Vec::new();
    for (branch, expected_parent) in &expected_parents {
        if crate::commands::navigate::is_shadow_branch(branch) {
            continue;
        }
        let current_parent = parent_map
            .get(branch)
            .and_then(|p| p.clone())
            .unwrap_or_else(|| "unknown".to_string());

        if current_parent != *expected_parent {
            mismatches.push((branch.clone(), current_parent, expected_parent.clone()));
        }
    }

    if !mismatches.is_empty() {
        log::debug!("check_topology: found {} mismatch(es)", mismatches.len());
    }

    mismatches
}

/// Fetch live PRs from GitHub and persist to cache.
async fn fetch_live_prs(git: &GitRepo) -> Result<Vec<PullRequest>> {
    let token = token_store::get_token()
        .ok_or_else(|| anyhow!("Not authenticated. Run 'stax auth' to log in."))?;
    let remote_url = git
        .get_remote_url("origin")
        .ok_or_else(|| anyhow!("No 'origin' remote found."))?;
    let github = GitHubClient::new(&token, &remote_url)?;

    utils::print_info("Fetching PR data from GitHub...");
    let prs = github.get_open_pull_requests().await?;

    // Persist to cache
    let cached: HashMap<String, CachedPullRequest> = prs
        .iter()
        .map(|pr| {
            (
                pr.head_ref.clone(),
                CachedPullRequest {
                    number: pr.number,
                    state: pr.state.clone(),
                    head_ref: pr.head_ref.clone(),
                    base_ref: pr.base_ref.clone(),
                    html_url: pr.html_url.clone(),
                    draft: pr.draft,
                },
            )
        })
        .collect();
    let mut cache = StackCache::new(&git.git_dir());
    cache.save_pull_requests(&cached);

    log::debug!("repair: fetched {} open PRs", prs.len());
    Ok(prs)
}

/// Load cached PRs from disk (for --continue after a previous run persisted them).
fn load_cached_prs(git: &GitRepo) -> Result<Vec<PullRequest>> {
    let mut cache = StackCache::new(&git.git_dir());
    let cached_prs = cache
        .load()
        .map(|data| data.pull_requests.clone())
        .unwrap_or_default();

    if cached_prs.is_empty() {
        return Err(anyhow!(
            "No cached PR data found. Run 'stax repair' (without --continue) to fetch fresh data."
        ));
    }

    let prs: Vec<PullRequest> = cached_prs
        .values()
        .filter(|cpr| cpr.state == "open")
        .map(|cpr| PullRequest {
            number: cpr.number,
            title: String::new(),
            body: None,
            state: cpr.state.clone(),
            head_ref: cpr.head_ref.clone(),
            base_ref: cpr.base_ref.clone(),
            html_url: cpr.html_url.clone(),
            draft: cpr.draft,
        })
        .collect();

    log::debug!("repair: loaded {} cached PRs for --continue", prs.len());
    Ok(prs)
}

/// Infer parents for "gap branches" — branches that are used as PR bases
/// but don't have PRs of their own.
///
/// Uses PR chain topology: removes descendant tails to find the unique
/// candidate parent. Falls back to FuzzySelect if ambiguous.
pub(crate) fn infer_gap_branch_parents(
    prs: &[&PullRequest],
    branch_set: &HashSet<&str>,
) -> Result<HashMap<String, String>> {
    let head_refs: HashSet<&str> = prs.iter().map(|pr| pr.head_ref.as_str()).collect();
    let base_refs: HashSet<&str> = prs.iter().map(|pr| pr.base_ref.as_str()).collect();

    // Chain tails: PR branches that no other PR targets as its base
    let chain_tails: HashSet<&str> = head_refs
        .iter()
        .filter(|h| !base_refs.contains(*h))
        .copied()
        .collect();

    // Gap branches: branches used as PR bases that don't have their own PR
    let gap_branches: Vec<&str> = base_refs
        .iter()
        .filter(|b| {
            !head_refs.contains(*b) && !MAIN_BRANCHES.contains(b) && branch_set.contains(*b)
        })
        .copied()
        .collect();

    if gap_branches.is_empty() {
        return Ok(HashMap::new());
    }

    // Build descendant map: for each branch, which branches are reachable
    // by walking "up" through PR edges (base → head → base → head ...)
    let base_to_heads: HashMap<&str, Vec<&str>> = {
        let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
        for pr in prs {
            map.entry(pr.base_ref.as_str())
                .or_default()
                .push(pr.head_ref.as_str());
        }
        map
    };

    let mut result = HashMap::new();

    for gap in &gap_branches {
        // Find all descendants of this gap branch in the PR chain
        let descendants = collect_descendants(gap, &base_to_heads);

        // Candidate parents: chain tails that are NOT descendants of this gap branch
        let candidates: Vec<&str> = chain_tails
            .iter()
            .filter(|t| !descendants.contains(*t))
            .copied()
            .collect();

        match candidates.len() {
            0 => {
                log::debug!("repair: no candidate parent found for gap branch '{}'", gap);
                utils::print_warning(&format!(
                    "Cannot determine parent for '{}' — no PR chain leads to it. \
                     Create a PR or manually rebase.",
                    gap
                ));
            }
            1 => {
                let parent = candidates[0];
                log::debug!(
                    "repair: auto-inferred parent of '{}' = '{}' (sole candidate)",
                    gap,
                    parent
                );
                result.insert(gap.to_string(), parent.to_string());
            }
            _ => {
                // Multiple candidates — ask user
                utils::print_info(&format!(
                    "Multiple possible parents for '{}'. Please select:",
                    gap
                ));
                let labels: Vec<String> = candidates.iter().map(|c| c.to_string()).collect();
                let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
                    .with_prompt(format!("Parent for '{}'", gap))
                    .items(&labels)
                    .default(0)
                    .interact()?;
                let parent = &candidates[selection];
                log::debug!("repair: user selected parent of '{}' = '{}'", gap, parent);
                result.insert(gap.to_string(), parent.to_string());
            }
        }
    }

    Ok(result)
}

/// Silent version of `infer_gap_branch_parents` for use in `check_topology_from_cache`.
/// Only returns single-candidate inferences (no interactive prompts, no warnings).
fn infer_gap_branch_parents_silent(
    prs: &[&PullRequest],
    branch_set: &HashSet<&str>,
) -> Result<HashMap<String, String>> {
    let head_refs: HashSet<&str> = prs.iter().map(|pr| pr.head_ref.as_str()).collect();
    let base_refs: HashSet<&str> = prs.iter().map(|pr| pr.base_ref.as_str()).collect();

    let chain_tails: HashSet<&str> = head_refs
        .iter()
        .filter(|h| !base_refs.contains(*h))
        .copied()
        .collect();

    let gap_branches: Vec<&str> = base_refs
        .iter()
        .filter(|b| {
            !head_refs.contains(*b) && !MAIN_BRANCHES.contains(b) && branch_set.contains(*b)
        })
        .copied()
        .collect();

    if gap_branches.is_empty() {
        return Ok(HashMap::new());
    }

    let base_to_heads: HashMap<&str, Vec<&str>> = {
        let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
        for pr in prs {
            map.entry(pr.base_ref.as_str())
                .or_default()
                .push(pr.head_ref.as_str());
        }
        map
    };

    let mut result = HashMap::new();

    for gap in &gap_branches {
        let descendants = collect_descendants(gap, &base_to_heads);
        let candidates: Vec<&str> = chain_tails
            .iter()
            .filter(|t| !descendants.contains(*t))
            .copied()
            .collect();

        if candidates.len() == 1 {
            result.insert(gap.to_string(), candidates[0].to_string());
        }
        // Skip 0 and multi-candidate cases silently
    }

    Ok(result)
}

/// Collect all transitive descendants of a branch in the PR chain.
pub(crate) fn collect_descendants<'a>(
    branch: &'a str,
    base_to_heads: &HashMap<&'a str, Vec<&'a str>>,
) -> HashSet<&'a str> {
    let mut descendants = HashSet::new();
    let mut queue = vec![branch];
    while let Some(current) = queue.pop() {
        if let Some(heads) = base_to_heads.get(current) {
            for head in heads {
                if descendants.insert(*head) {
                    // Also check if this head is a base for more PRs
                    queue.push(head);
                }
            }
        }
    }
    descendants
}

/// Sort branches topologically so parents are repaired before children.
pub(crate) fn topological_sort(
    expected_parents: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();

    fn visit(
        branch: &str,
        expected_parents: &HashMap<String, String>,
        visited: &mut HashSet<String>,
        result: &mut Vec<(String, String)>,
    ) {
        if visited.contains(branch) {
            return;
        }
        visited.insert(branch.to_string());

        // Visit parent first (if it's also in the map)
        if let Some(parent) = expected_parents.get(branch) {
            if expected_parents.contains_key(parent) {
                visit(parent, expected_parents, visited, result);
            }
        }

        if let Some(parent) = expected_parents.get(branch) {
            result.push((branch.to_string(), parent.clone()));
        }
    }

    for branch in expected_parents.keys() {
        visit(branch, expected_parents, &mut visited, &mut result);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::PullRequest;

    fn make_pr(head: &str, base: &str, number: u64) -> PullRequest {
        PullRequest {
            number,
            title: String::new(),
            body: None,
            state: "open".to_string(),
            head_ref: head.to_string(),
            base_ref: base.to_string(),
            html_url: format!("https://github.com/o/r/pull/{number}"),
            draft: false,
        }
    }

    // ---- topological_sort tests ----

    #[test]
    fn test_topological_sort_linear_chain() {
        // A→main, B→A, C→B → should produce [A, B, C]
        let mut map = HashMap::new();
        map.insert("A".to_string(), "main".to_string());
        map.insert("B".to_string(), "A".to_string());
        map.insert("C".to_string(), "B".to_string());

        let sorted = topological_sort(&map);
        let names: Vec<&str> = sorted.iter().map(|(b, _)| b.as_str()).collect();

        // A must come before B, B must come before C
        let pos_a = names.iter().position(|&n| n == "A").unwrap();
        let pos_b = names.iter().position(|&n| n == "B").unwrap();
        let pos_c = names.iter().position(|&n| n == "C").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_topological_sort_independent_branches() {
        // A→main, B→main — both appear, order doesn't matter
        let mut map = HashMap::new();
        map.insert("A".to_string(), "main".to_string());
        map.insert("B".to_string(), "main".to_string());

        let sorted = topological_sort(&map);
        assert_eq!(sorted.len(), 2);

        let names: HashSet<&str> = sorted.iter().map(|(b, _)| b.as_str()).collect();
        assert!(names.contains("A"));
        assert!(names.contains("B"));
    }

    #[test]
    fn test_topological_sort_diamond() {
        // B→A, C→A, A→main — A must appear before B and C
        let mut map = HashMap::new();
        map.insert("A".to_string(), "main".to_string());
        map.insert("B".to_string(), "A".to_string());
        map.insert("C".to_string(), "A".to_string());

        let sorted = topological_sort(&map);
        let names: Vec<&str> = sorted.iter().map(|(b, _)| b.as_str()).collect();

        let pos_a = names.iter().position(|&n| n == "A").unwrap();
        let pos_b = names.iter().position(|&n| n == "B").unwrap();
        let pos_c = names.iter().position(|&n| n == "C").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
    }

    // ---- collect_descendants tests ----

    #[test]
    fn test_collect_descendants_linear() {
        // A→B→C chain
        let mut base_to_heads: HashMap<&str, Vec<&str>> = HashMap::new();
        base_to_heads.insert("A", vec!["B"]);
        base_to_heads.insert("B", vec!["C"]);

        let desc = collect_descendants("A", &base_to_heads);
        assert!(desc.contains("B"));
        assert!(desc.contains("C"));
        assert_eq!(desc.len(), 2);
    }

    #[test]
    fn test_collect_descendants_branching() {
        // A→{B, C}
        let mut base_to_heads: HashMap<&str, Vec<&str>> = HashMap::new();
        base_to_heads.insert("A", vec!["B", "C"]);

        let desc = collect_descendants("A", &base_to_heads);
        assert!(desc.contains("B"));
        assert!(desc.contains("C"));
        assert_eq!(desc.len(), 2);
    }

    #[test]
    fn test_collect_descendants_no_children() {
        // Leaf node with no children
        let base_to_heads: HashMap<&str, Vec<&str>> = HashMap::new();

        let desc = collect_descendants("leaf", &base_to_heads);
        assert!(desc.is_empty());
    }

    // ---- infer_gap_branch_parents tests ----

    #[test]
    fn test_infer_gap_single_candidate() {
        // PRs: tail→main, child→gap
        // gap has no PR → gap's inferred parent = tail (sole chain tail not descended from gap)
        let pr1 = make_pr("tail", "main", 1);
        let pr2 = make_pr("child", "gap", 2);
        let prs: Vec<&PullRequest> = vec![&pr1, &pr2];
        let branch_set: HashSet<&str> = ["main", "tail", "gap", "child"].iter().copied().collect();

        let result = infer_gap_branch_parents(&prs, &branch_set).unwrap();
        assert_eq!(result.get("gap"), Some(&"tail".to_string()));
    }

    #[test]
    fn test_infer_gap_no_candidates() {
        // PRs: child→gap — tail IS a descendant of gap in the chain
        // Actually: gap→child is the chain direction. gap is the base, child is the head.
        // chain_tails = {child} (child is a head_ref not used as a base_ref)
        // descendants of gap = {child}
        // candidates = chain_tails - descendants = {} → no candidate
        let pr1 = make_pr("child", "gap", 1);
        let prs: Vec<&PullRequest> = vec![&pr1];
        let branch_set: HashSet<&str> = ["main", "gap", "child"].iter().copied().collect();

        let result = infer_gap_branch_parents(&prs, &branch_set).unwrap();
        assert!(result.is_empty()); // No candidate found
    }

    #[test]
    fn test_infer_gap_no_gaps() {
        // All PR bases have their own PRs → no gap branches
        let pr1 = make_pr("A", "main", 1);
        let pr2 = make_pr("B", "A", 2);
        let prs: Vec<&PullRequest> = vec![&pr1, &pr2];
        let branch_set: HashSet<&str> = ["main", "A", "B"].iter().copied().collect();

        let result = infer_gap_branch_parents(&prs, &branch_set).unwrap();
        assert!(result.is_empty());
    }
}
