use crate::{
    cache::{CachedPullRequest, RestackState, StackCache},
    commands::navigate::get_branches_and_parent_map,
    config::Config,
    git::GitRepo,
    github::{GitHubClient, PullRequest},
    stack,
    stack::Stack,
    token_store, utils,
};
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};

pub async fn run(
    no_restack: bool,
    force: bool,
    continue_rebase: bool,
    metadata_only: bool,
) -> Result<()> {
    log::debug!(
        "sync: no_restack={}, force={}, continue={}, metadata_only={}",
        no_restack,
        force,
        continue_rebase,
        metadata_only
    );
    let git = GitRepo::open(".")?;
    let config = Config::load()?;

    if metadata_only {
        log::debug!("sync: metadata-only mode");
        return refresh_metadata(&git).await;
    }

    if continue_rebase {
        log::debug!("sync: continuing after conflicts");
        return continue_after_conflicts(&git, &config).await;
    }

    // 1. Guard — abort if rebase in progress or working tree is dirty
    if git.is_rebase_in_progress() {
        return Err(anyhow!(
            "A rebase is currently in progress.\n\
             Resolve conflicts and run 'stax sync --continue', or 'git rebase --abort' to cancel."
        ));
    }
    if !git.is_clean()? {
        return Err(anyhow!(
            "Working directory has uncommitted changes. Please commit or stash them first."
        ));
    }

    // Save original branch so we can restore it later
    let original_branch = git.current_branch()?;

    // 2. Fetch
    utils::print_info("Fetching from origin...");
    git.fetch()?;
    utils::print_success("Fetched latest changes");

    // 3. Fast-forward trunk
    let trunk = &config.default_base_branch;
    match git.fast_forward_branch(trunk) {
        Ok(true) => utils::print_success(&format!("Fast-forwarded '{trunk}'")),
        Ok(false) => utils::print_info(&format!("'{trunk}' is already up to date")),
        Err(e) => utils::print_warning(&format!("Could not fast-forward '{trunk}': {e}")),
    }

    // 4. Set up GitHub client (needed for PR state checks)
    let token = token_store::get_token()
        .ok_or_else(|| anyhow!("Not authenticated. Run 'stax auth' to log in."))?;
    let remote_url = git
        .get_remote_url("origin")
        .ok_or_else(|| anyhow!("No 'origin' remote found."))?;
    let github = GitHubClient::new(&token, &remote_url)?;

    // 5. Build stack from cache + live PRs to find merged/closed PRs
    let stack = build_stack_with_live_prs(&git, &github, &original_branch).await?;
    let current_stack: HashSet<String> = stack
        .get_stack_for_branch(&original_branch)
        .iter()
        .map(|b| b.name.clone())
        .collect();
    let mut merged = find_merged_branches(&stack, trunk, &current_stack);

    // Also check if the current branch is locally merged into trunk but was
    // filtered out of the stack (its tip is an ancestor of trunk).
    let locally_merged =
        find_locally_merged_branch(&git, &github, &stack, trunk, &original_branch).await;
    merged.extend(locally_merged);
    log::debug!("sync: found {} merged/closed branches", merged.len());

    // 5b. Check shadow branch sources for dissolution
    dissolve_shadows_if_needed(&git, &merged, trunk)?;

    // 6. Delete merged/closed branches
    if !merged.is_empty() {
        delete_merged_branches(&git, &merged, &original_branch, trunk, force)?;
    } else {
        utils::print_info("No merged or closed branches to clean up");
    }

    // 7. Restack
    if !no_restack {
        log::debug!("sync: restacking branches");
        restack_branches(&git, &config, &github, &original_branch).await?;
    } else {
        log::debug!("sync: skipping restack (--no-restack)");
    }

    // 8. Restore original branch (or trunk if it was deleted)
    let branches = git.get_branches()?;
    let checkout_target = if branches.contains(&original_branch) {
        &original_branch
    } else {
        trunk
    };

    // Only checkout if we're not already there
    let current = git.current_branch().unwrap_or_default();
    if current != *checkout_target {
        git.checkout_branch(checkout_target)?;
    }

    // Refresh cache to reflect all changes (deletions, rebases, trunk fast-forward)
    log::debug!("sync: refreshing cache");
    let _ = get_branches_and_parent_map(&git);

    utils::print_success("Sync complete");
    Ok(())
}

/// Refresh the local metadata cache without fetching, rebasing, or deleting anything.
/// Recomputes branch parent relationships and fetches fresh PR data from GitHub.
async fn refresh_metadata(git: &GitRepo) -> Result<()> {
    utils::print_info("Refreshing metadata cache...");

    // 1. Recompute branch parents (triggers cache load → validate → partial recompute → save)
    let _ = get_branches_and_parent_map(git)?;
    log::debug!("sync: branch parent cache refreshed");

    // 2. Fetch fresh PR data from GitHub and persist to cache
    if let Some(token) = token_store::get_token() {
        if let Some(remote_url) = git.get_remote_url("origin") {
            if let Ok(github) = GitHubClient::new(&token, &remote_url) {
                if let Ok(open_prs) = github.get_open_pull_requests().await {
                    let all_branches = git.get_branches()?;

                    // Preserve locally-modified base_refs (e.g., from `stax insert`)
                    let local_pr_bases: HashMap<String, String> = {
                        let mut c = StackCache::new(&git.git_dir());
                        c.load();
                        c.data_ref()
                            .map(|d| {
                                d.pull_requests
                                    .iter()
                                    .map(|(k, v)| (k.clone(), v.base_ref.clone()))
                                    .collect()
                            })
                            .unwrap_or_default()
                    };

                    let cached_prs: HashMap<String, CachedPullRequest> = open_prs
                        .iter()
                        .map(|pr| {
                            let base_ref = if let Some(local_base) =
                                local_pr_bases.get(&pr.head_ref)
                            {
                                if *local_base != pr.base_ref && all_branches.contains(local_base) {
                                    local_base.clone()
                                } else {
                                    pr.base_ref.clone()
                                }
                            } else {
                                pr.base_ref.clone()
                            };
                            (
                                pr.head_ref.clone(),
                                CachedPullRequest {
                                    number: pr.number,
                                    state: pr.state.clone(),
                                    head_ref: pr.head_ref.clone(),
                                    base_ref,
                                    html_url: pr.html_url.clone(),
                                    draft: pr.draft,
                                },
                            )
                        })
                        .collect();

                    let mut cache = StackCache::new(&git.git_dir());
                    cache.save_pull_requests(&cached_prs);
                    log::debug!("sync: saved {} PRs to cache", cached_prs.len());

                    // Re-run parent computation so PR overrides take effect
                    let _ = get_branches_and_parent_map(git)?;
                }
            }
        }
    }

    utils::print_success("Metadata cache refreshed");
    Ok(())
}

/// Find branches whose PRs are merged or closed, scoped to the current stack.
fn find_merged_branches(
    stack: &Stack,
    trunk: &str,
    current_stack: &HashSet<String>,
) -> Vec<(String, String)> {
    let main_branches = ["main", "master", "develop"];

    stack
        .branches
        .values()
        .filter_map(|branch| {
            if main_branches.contains(&branch.name.as_str()) || branch.name == trunk {
                return None;
            }
            if !current_stack.contains(&branch.name) {
                return None;
            }
            if let Some(pr) = &branch.pull_request {
                match pr.state.as_str() {
                    "merged" => Some((branch.name.clone(), format!("PR #{} merged", pr.number))),
                    "closed" => Some((branch.name.clone(), format!("PR #{} closed", pr.number))),
                    _ => None,
                }
            } else {
                None
            }
        })
        .collect()
}

/// Check if the current branch is locally merged into trunk but was filtered
/// out of the stack. If so, verify via GitHub that its PR is merged/closed.
async fn find_locally_merged_branch(
    git: &GitRepo,
    github: &GitHubClient,
    analyzed_stack: &Stack,
    trunk: &str,
    current_branch: &str,
) -> Vec<(String, String)> {
    // If the branch is already in the stack, find_merged_branches handles it
    if analyzed_stack.branches.contains_key(current_branch) {
        return Vec::new();
    }

    let main_branches = ["main", "master", "develop"];
    if main_branches.contains(&current_branch) || current_branch == trunk {
        return Vec::new();
    }

    if !stack::is_merged_into(git, current_branch, trunk) {
        return Vec::new();
    }

    // Fetch PR status from GitHub
    if let Ok(Some(pr)) = github.get_pr_for_branch(current_branch).await {
        match pr.state.as_str() {
            "merged" => {
                return vec![(
                    current_branch.to_string(),
                    format!("PR #{} merged", pr.number),
                )]
            }
            "closed" => {
                return vec![(
                    current_branch.to_string(),
                    format!("PR #{} closed", pr.number),
                )]
            }
            _ => {}
        }
    }

    Vec::new()
}

/// Prompt the user (unless --force) and delete merged/closed branches.
fn delete_merged_branches(
    git: &GitRepo,
    branches: &[(String, String)],
    _current_branch: &str,
    trunk: &str,
    force: bool,
) -> Result<()> {
    utils::print_info("Branches with merged/closed PRs:");
    for (name, reason) in branches {
        utils::print_info(&format!("  {name} ({reason})"));
    }

    let should_delete = if force {
        true
    } else {
        utils::confirm("Delete these branches locally and from remote?")?
    };

    if !should_delete {
        utils::print_info("Skipping branch cleanup");
        return Ok(());
    }

    for (name, _reason) in branches {
        // If we're on a branch being deleted, switch to trunk first
        if let Ok(cur) = git.current_branch() {
            if cur == *name {
                git.checkout_branch(trunk)?;
            }
        }

        match git.delete_branch(name, true) {
            Ok(()) => utils::print_success(&format!("Deleted '{name}'")),
            Err(e) => utils::print_warning(&format!("Failed to delete '{name}': {e}")),
        }
    }

    Ok(())
}

/// Re-analyze the stack and rebase branches in topological order (parents first).
async fn restack_branches(
    git: &GitRepo,
    _config: &Config,
    github: &GitHubClient,
    original_branch: &str,
) -> Result<()> {
    // Fresh start — clear any stale restack state from a previous aborted run
    StackCache::new(&git.git_dir()).clear_restack_state();
    restack_all(git, Some(github), original_branch).await
}

/// Shared restack logic used by both normal sync and --continue.
/// Only restacks branches in the same stack as `current_branch`.
async fn restack_all(
    git: &GitRepo,
    _github: Option<&GitHubClient>,
    current_branch: &str,
) -> Result<()> {
    let (branches, commits, merged_set, parent_map) = get_branches_and_parent_map(git)?;
    let stack = Stack::from_parent_map(
        git,
        current_branch,
        None,
        &branches,
        &commits,
        &merged_set,
        &parent_map,
    )
    .await?;
    let main_branches = ["main", "master", "develop"];

    // Scope: walk up from current_branch to trunk to collect ancestors,
    // then walk down from the topmost ancestor to collect all descendants.
    // This ensures the full chain from trunk → current_branch (and beyond)
    // is rebased in the correct topological order.
    let mut restack_scope: HashSet<String> = HashSet::new();

    // 1. Walk up to find the topmost non-main ancestor
    let mut top = current_branch.to_string();
    {
        let mut cur = current_branch.to_string();
        while let Some(branch) = stack.branches.get(&cur) {
            if let Some(parent) = &branch.parent {
                if main_branches.contains(&parent.as_str()) {
                    break;
                }
                top = parent.clone();
                cur = parent.clone();
            } else {
                break;
            }
        }
    }

    // 2. Walk down from the top to collect the full scope
    let mut queue = vec![top];
    while let Some(name) = queue.pop() {
        if restack_scope.insert(name.clone()) {
            if let Some(branch) = stack.branches.get(&name) {
                for child in &branch.children {
                    queue.push(child.clone());
                }
            }
        }
    }

    // Collect branches with their parents and depth for topological ordering.
    let mut to_rebase: Vec<(String, String, usize)> = Vec::new();

    for branch in stack.branches.values() {
        if main_branches.contains(&branch.name.as_str()) {
            continue;
        }
        if !restack_scope.contains(&branch.name) {
            continue;
        }
        if let Some(parent) = &branch.parent {
            let depth = branch_depth(&stack, &branch.name);
            to_rebase.push((branch.name.clone(), parent.clone(), depth));
        }
    }

    // Sort by depth so parents are rebased before children
    to_rebase.sort_by_key(|(_, _, depth)| *depth);

    if to_rebase.is_empty() {
        utils::print_info("Nothing to restack");
        return Ok(());
    }

    let cache = StackCache::new(&git.git_dir());

    // Load persisted old_tips (from a previous --continue) or compute fresh.
    // Persisted entries take precedence — they capture the original pre-restack
    // fork points. Fresh entries fill in branches not in the original plan.
    let old_tips = {
        let persisted = cache.load_restack_state();
        let mut tips = persisted.map(|s| s.old_tips).unwrap_or_default();
        let had_persisted = !tips.is_empty();
        for (branch, parent, _) in &to_rebase {
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
                "sync restack: loaded persisted old_tips, merged to {} entries",
                tips.len()
            );
        } else {
            log::debug!(
                "sync restack: computed fresh old_tips ({} entries)",
                tips.len()
            );
        }
        tips
    };

    // Persist for potential --continue
    cache.save_restack_state(&RestackState {
        old_tips: old_tips.clone(),
        original_branch: current_branch.to_string(),
    });

    // Load shadow branch data for recreating shadows during restack
    let mut shadow_cache = StackCache::new(&git.git_dir());
    shadow_cache.load();
    let shadow_branches: HashMap<String, crate::cache::ShadowBranch> = shadow_cache
        .data_ref()
        .map(|d| d.shadow_branches.clone())
        .unwrap_or_default();

    let mut restacked = Vec::new();

    for (branch, parent, _) in &to_rebase {
        // If the parent is a shadow branch, recreate it from its sources first
        if crate::commands::navigate::is_shadow_branch(parent) {
            if let Some(shadow) = shadow_branches.get(parent) {
                utils::print_info(&format!("Recreating shadow branch '{parent}'"));
                let source_refs: Vec<&str> = shadow.sources.iter().map(|s| s.as_str()).collect();
                match git.recreate_shadow_branch(parent, &source_refs) {
                    Ok(()) => {}
                    Err(e) => {
                        if let Some(conflict) = e.downcast_ref::<crate::git::ShadowMergeConflict>()
                        {
                            shadow_cache.save_shadow_merge_state(&crate::cache::ShadowMergeState {
                                shadow_name: conflict.shadow_name.clone(),
                                consumer: shadow.consumer.clone(),
                                all_sources: shadow.sources.clone(),
                                remaining_sources: conflict.remaining_sources.clone(),
                                original_branch: current_branch.to_string(),
                                continue_command: "stax sync --continue".to_string(),
                            });
                            return Err(anyhow::anyhow!(
                                "Merge conflict while rebuilding shadow branch '{}'.\n\
                                 Source '{}' conflicts with prior sources.\n\n\
                                 Resolve the conflicts, stage the files, then run:\n  \
                                 stax sync --continue\n\n\
                                 To abort instead:\n  \
                                 git merge --abort && git checkout {}",
                                conflict.shadow_name,
                                conflict.failed_source,
                                current_branch,
                            ));
                        }
                        return Err(e);
                    }
                }
            }
        }
        utils::print_info(&format!("Rebasing '{branch}' onto '{parent}'"));
        // Use the parent's pre-rebase tip so --onto only replays branch's own commits
        let old_parent_tip = old_tips.get(parent).map(|s| s.as_str());
        match git.rebase_onto_with_base(
            branch,
            parent,
            old_parent_tip,
            Some("stax sync --continue"),
        ) {
            Ok(()) => restacked.push(branch.as_str()),
            Err(e) => return Err(e),
        }
    }

    // Success — clean up state file
    cache.clear_restack_state();

    for branch in &restacked {
        utils::print_success(&format!("Restacked '{branch}'"));
    }

    Ok(())
}

/// Continue a sync after the user has resolved rebase or merge conflicts.
/// Handles shadow merge conflicts, rebase conflicts, then restacks remaining.
async fn continue_after_conflicts(git: &GitRepo, _config: &Config) -> Result<()> {
    let cache = StackCache::new(&git.git_dir());

    // Check if we're continuing a shadow merge conflict
    if let Some(state) = cache.load_shadow_merge_state() {
        if git.is_merge_in_progress() {
            utils::print_info("Committing resolved shadow merge...");
        }
        let remaining_refs: Vec<&str> =
            state.remaining_sources.iter().map(|s| s.as_str()).collect();
        match git.continue_shadow_merge(&state.shadow_name, &remaining_refs) {
            Ok(()) => {
                cache.clear_shadow_merge_state();
                utils::print_success(&format!("Shadow branch '{}' rebuilt", state.shadow_name));
                git.checkout_branch(&state.original_branch)?;
            }
            Err(e) => {
                if let Some(conflict) = e.downcast_ref::<crate::git::ShadowMergeConflict>() {
                    cache.save_shadow_merge_state(&crate::cache::ShadowMergeState {
                        remaining_sources: conflict.remaining_sources.clone(),
                        ..state
                    });
                    return Err(anyhow::anyhow!(
                        "Another merge conflict while merging '{}'.\n\
                         Resolve the conflicts, stage the files, then run:\n  \
                         stax sync --continue",
                        conflict.failed_source,
                    ));
                }
                return Err(e);
            }
        }
    } else if git.is_rebase_in_progress() {
        utils::print_info("Continuing rebase...");
        git.rebase_continue()?;
        utils::print_success("Rebase continued successfully");
    }

    // Re-analyze and restack remaining branches (scoped to current stack)
    let current_branch = git.current_branch()?;
    restack_all(git, None, &current_branch).await?;

    // Refresh cache after conflict resolution and restacking
    log::debug!("sync: refreshing cache after --continue");
    let _ = get_branches_and_parent_map(git);

    utils::print_success("Sync complete");
    Ok(())
}

/// Build a Stack using the cache-aware path with live PR data from GitHub.
/// Fetches fresh PR data (including merged/closed PRs) and persists it to cache.
async fn build_stack_with_live_prs(
    git: &GitRepo,
    github: &GitHubClient,
    current_branch: &str,
) -> Result<Stack> {
    let (branches, commits, merged_set, mut parent_map) = get_branches_and_parent_map(git)?;

    // Fetch live PRs (including merged/closed state) for branch cleanup decisions
    let mut prs: HashMap<String, PullRequest> = HashMap::new();
    if let Ok(open_prs) = github.get_open_pull_requests().await {
        // Load existing cached PR data to detect local modifications
        // (e.g., from `stax insert` reparenting that hasn't been submitted yet)
        let local_pr_bases: HashMap<String, String> = {
            let mut c = StackCache::new(&git.git_dir());
            c.load();
            c.data_ref()
                .map(|d| {
                    d.pull_requests
                        .iter()
                        .map(|(k, v)| (k.clone(), v.base_ref.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };

        for mut pr in open_prs {
            // If the cached base_ref differs from live AND exists as a local branch,
            // preserve the cached version — it represents a local reparent (e.g.,
            // from `stax insert`) that hasn't been submitted to GitHub yet.
            if let Some(local_base) = local_pr_bases.get(&pr.head_ref) {
                if *local_base != pr.base_ref && branches.contains(local_base) {
                    log::debug!(
                        "sync: preserving local base_ref '{}' for '{}' (live: '{}')",
                        local_base,
                        pr.head_ref,
                        pr.base_ref
                    );
                    pr.base_ref = local_base.clone();
                }
            }

            // Apply PR base_ref overrides
            if parent_map.contains_key(&pr.head_ref) && branches.contains(&pr.base_ref) {
                let current = parent_map.get(&pr.head_ref).and_then(|p| p.as_ref());
                if current != Some(&pr.base_ref) {
                    log::debug!(
                        "sync: PR #{} overrides parent of '{}': {:?} → '{}'",
                        pr.number,
                        pr.head_ref,
                        current,
                        pr.base_ref
                    );
                    parent_map.insert(pr.head_ref.clone(), Some(pr.base_ref.clone()));
                }
            }
            prs.insert(pr.head_ref.clone(), pr);
        }

        // Persist PR metadata to cache (with locally-modified base_refs preserved)
        let cached_prs: HashMap<String, CachedPullRequest> = prs
            .iter()
            .map(|(k, pr)| {
                (
                    k.clone(),
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
        cache.save_pull_requests(&cached_prs);
    }

    // Also fetch PRs for branches that might be merged/closed (not in open PRs)
    // by checking individually for branches not yet covered
    let missing: Vec<_> = branches
        .iter()
        .filter(|b| !["main", "master", "develop"].contains(&b.as_str()) && !prs.contains_key(*b))
        .cloned()
        .collect();

    if !missing.is_empty() {
        let handles: Vec<_> = missing
            .into_iter()
            .map(|b| {
                let gh = github.clone();
                tokio::spawn(async move { gh.get_pr_for_branch(&b).await })
            })
            .collect();

        for handle in handles {
            if let Ok(Some(pr)) = handle.await? {
                prs.insert(pr.head_ref.clone(), pr);
            }
        }
    }

    let mut stack = Stack::from_parent_map(
        git,
        current_branch,
        None,
        &branches,
        &commits,
        &merged_set,
        &parent_map,
    )
    .await?;

    // Inject PR data for merged/closed detection
    for pr in prs.values() {
        if let Some(branch) = stack.branches.get_mut(&pr.head_ref) {
            branch.pull_request = Some(pr.clone());
        }
    }

    Ok(stack)
}

/// Check shadow branches and dissolve them when sources have been merged.
///
/// - All sources merged: delete shadow, reparent consumer to trunk.
/// - Some sources merged: remove them from sources, recreate shadow or
///   dissolve to single parent.
pub fn dissolve_shadows_if_needed(
    git: &GitRepo,
    merged_branches: &[(String, String)],
    trunk: &str,
) -> Result<()> {
    let mut cache = StackCache::new(&git.git_dir());
    if cache.load().is_none() {
        return Ok(());
    }

    let merged_set: HashSet<String> = merged_branches.iter().map(|(n, _)| n.clone()).collect();

    let shadows: Vec<(String, crate::cache::ShadowBranch)> = cache
        .data_ref()
        .map(|d| {
            d.shadow_branches
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();

    for (shadow_name, shadow) in &shadows {
        let remaining: Vec<String> = shadow
            .sources
            .iter()
            .filter(|s| !merged_set.contains(*s))
            .cloned()
            .collect();

        if remaining.len() == shadow.sources.len() {
            // No sources merged — nothing to do
            continue;
        }

        if remaining.len() <= 1 {
            // All (or all but one) sources merged — dissolve the shadow
            let new_parent = remaining.first().map(|s| s.as_str()).unwrap_or(trunk);
            utils::print_info(&format!(
                "Dissolving shadow '{}' — reparenting '{}' to '{}'",
                shadow_name, shadow.consumer, new_parent
            ));

            // Delete shadow branch (local + remote)
            if git
                .get_commit_hash(&format!("refs/heads/{shadow_name}"))
                .is_ok()
            {
                let current = git.current_branch()?;
                if current == *shadow_name {
                    git.checkout_branch(new_parent)?;
                }
                let _ = git.delete_branch(shadow_name, true);
            }

            // Rebase consumer onto new parent immediately.
            // We must do this now (not defer to the later restack step) because
            // the consumer is currently based on the shadow's merge commit.
            // A `--onto` rebase later would use the wrong base and try to
            // replay the merge commit.
            utils::print_info(&format!(
                "Rebasing '{}' onto '{}'",
                shadow.consumer, new_parent
            ));
            git.rebase_onto(&shadow.consumer, new_parent)?;

            // Update cache: remove shadow entry, update consumer parent
            cache.remove_shadow(shadow_name);
            let consumer_tip = git
                .get_commit_hash(&format!("refs/heads/{}", shadow.consumer))
                .unwrap_or_default();
            if let Some(data) = cache.data_mut() {
                if let Some(branch) = data.branches.get_mut(&shadow.consumer) {
                    branch.parent = Some(new_parent.to_string());
                    branch.merge_sources.clear();
                    branch.tip = consumer_tip;
                }
                data.branches.remove(shadow_name);
            }
            cache.save_current();

            utils::print_success(&format!("Shadow '{}' dissolved", shadow_name));
        } else {
            // Some sources merged but 2+ remain — recreate the shadow
            utils::print_info(&format!(
                "Recreating shadow '{}' with remaining sources: {:?}",
                shadow_name, remaining
            ));
            let source_refs: Vec<&str> = remaining.iter().map(|s| s.as_str()).collect();
            git.recreate_shadow_branch(shadow_name, &source_refs)?;

            // Update cache
            let shadow_tip = git.get_commit_hash(&format!("refs/heads/{shadow_name}"))?;
            cache.upsert_shadow(
                shadow_name,
                crate::cache::ShadowBranch {
                    consumer: shadow.consumer.clone(),
                    sources: remaining.clone(),
                    tip: shadow_tip,
                },
            );
            if let Some(data) = cache.data_mut() {
                if let Some(branch) = data.branches.get_mut(&shadow.consumer) {
                    branch.merge_sources = remaining;
                }
            }
            cache.save_current();
        }
    }

    Ok(())
}

/// Compute the depth of a branch in the stack (distance from root).
fn branch_depth(stack: &Stack, branch_name: &str) -> usize {
    let mut depth = 0;
    let mut current = branch_name;
    while let Some(branch) = stack.branches.get(current) {
        match &branch.parent {
            Some(parent) => {
                depth += 1;
                current = parent;
            }
            None => break,
        }
    }
    depth
}
