use crate::cache::{RestackState, StackCache};
use crate::commands::navigate::{get_branches_and_parent_map, is_shadow_branch};
use crate::git::GitRepo;
use crate::stack::Stack;
use crate::utils;
use anyhow::{anyhow, Result};

pub async fn run(all: bool, continue_rebase: bool) -> Result<()> {
    log::debug!("restack: all={}, continue={}", all, continue_rebase);
    let git = GitRepo::open(".")?;

    if continue_rebase {
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
                    // Restore original branch and continue restacking
                    git.checkout_branch(&state.original_branch)?;
                }
                Err(e) => {
                    if let Some(conflict) = e.downcast_ref::<crate::git::ShadowMergeConflict>() {
                        cache.save_shadow_merge_state(&crate::cache::ShadowMergeState {
                            remaining_sources: conflict.remaining_sources.clone(),
                            ..state
                        });
                        return Err(anyhow!(
                            "Another merge conflict while merging '{}'.\n\
                             Resolve the conflicts, stage the files, then run:\n  \
                             stax restack --continue",
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
        // State file is preserved — do_restack will load old_tips from it
        return do_restack(&git, true).await;
    }

    // Fresh start — clear any stale state from a previous aborted restack
    let fresh_cache = StackCache::new(&git.git_dir());
    fresh_cache.clear_restack_state();
    fresh_cache.clear_shadow_merge_state();

    do_restack(&git, all).await
}

/// Core restack logic shared by fresh starts and --continue.
/// Loads persisted old_tips (if resuming) or computes fresh ones.
async fn do_restack(git: &GitRepo, all: bool) -> Result<()> {
    if git.is_rebase_in_progress() {
        return Err(anyhow!(
            "A rebase is currently in progress.\n\
             Resolve conflicts and run 'stax restack --continue', or 'git rebase --abort' to cancel."
        ));
    }
    if !git.is_clean()? {
        return Err(anyhow!(
            "Working directory has uncommitted changes. Please commit or stash them first."
        ));
    }

    let current_branch = git.current_branch()?;
    let (branches, commits, merged_set, parent_map) = get_branches_and_parent_map(git)?;

    // Check for topology issues before restacking — restacking on a broken
    // topology would cement the wrong parent relationships.
    let mismatches = crate::commands::repair::check_topology_from_cache(git, &parent_map);
    if !mismatches.is_empty() {
        crate::utils::print_topology_warning(&mismatches);
        return Err(anyhow!(
            "Topology issues detected. Run 'stax repair' first, then restack."
        ));
    }

    let stack = Stack::from_parent_map(
        git,
        &current_branch,
        None,
        &branches,
        &commits,
        &merged_set,
        &parent_map,
    )
    .await?;

    let main_branches = ["main", "master", "develop"];

    let branches_to_rebase: Vec<(String, String)> = if all {
        let stack_branches = stack.get_stack_for_branch(&current_branch);
        stack_branches
            .iter()
            .filter_map(|b| {
                if main_branches.contains(&b.name.as_str()) {
                    return None;
                }
                b.parent
                    .as_ref()
                    .map(|parent| (b.name.clone(), parent.clone()))
            })
            .collect()
    } else {
        let branch = stack
            .branches
            .get(&current_branch)
            .ok_or_else(|| anyhow!("Current branch '{}' not found in stack", current_branch))?;

        if main_branches.contains(&current_branch.as_str()) {
            utils::print_info("Nothing to restack — on a root branch");
            return Ok(());
        }

        match &branch.parent {
            Some(parent) => vec![(current_branch.clone(), parent.clone())],
            None => {
                utils::print_info("Nothing to restack — no parent branch found");
                return Ok(());
            }
        }
    };

    if branches_to_rebase.is_empty() {
        utils::print_info("Nothing to restack");
        return Ok(());
    }

    let cache = StackCache::new(&git.git_dir());

    // Load persisted old_tips (from a previous --continue) or compute fresh.
    // Persisted entries take precedence — they capture the original pre-restack
    // fork points. Fresh entries fill in branches not in the original plan
    // (e.g., single-branch restack escalating to --all via --continue).
    let old_tips = {
        let persisted = cache.load_restack_state();
        let mut tips = persisted.map(|s| s.old_tips).unwrap_or_default();
        let had_persisted = !tips.is_empty();
        for (branch, parent) in &branches_to_rebase {
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
                "restack: loaded persisted old_tips, merged to {} entries",
                tips.len()
            );
        } else {
            log::debug!("restack: computed fresh old_tips ({} entries)", tips.len());
        }
        tips
    };

    // Persist for potential --continue
    cache.save_restack_state(&RestackState {
        old_tips: old_tips.clone(),
        original_branch: current_branch.clone(),
    });

    let mut restacked = Vec::new();

    // Load shadow branch data for recreating shadows
    let mut shadow_cache = StackCache::new(&git.git_dir());
    shadow_cache.load();
    let shadow_branches: std::collections::HashMap<String, crate::cache::ShadowBranch> =
        shadow_cache
            .data_ref()
            .map(|d| d.shadow_branches.clone())
            .unwrap_or_default();

    log::debug!("restack: {} branches to rebase", branches_to_rebase.len());
    for (branch, parent) in &branches_to_rebase {
        // If the parent is a shadow branch, recreate it from its sources first
        if is_shadow_branch(parent) {
            if let Some(shadow) = shadow_branches.get(parent) {
                utils::print_info(&format!("Recreating shadow branch '{}'", parent));
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
                                original_branch: current_branch.clone(),
                                continue_command: "stax restack --continue".to_string(),
                            });
                            return Err(anyhow::anyhow!(
                                "Merge conflict while rebuilding shadow branch '{}'.\n\
                                 Source '{}' conflicts with prior sources.\n\n\
                                 Resolve the conflicts, stage the files, then run:\n  \
                                 stax restack --continue\n\n\
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
        utils::print_info(&format!("Rebasing '{}' onto '{}'", branch, parent));
        let old_parent_tip = old_tips.get(parent).map(|s| s.as_str());
        git.rebase_onto_with_base(
            branch,
            parent,
            old_parent_tip,
            Some("stax restack --continue"),
        )?;
        restacked.push(branch.as_str());
    }

    // Success — clean up state file
    cache.clear_restack_state();

    // Restore original branch
    git.checkout_branch(&current_branch)?;

    // Refresh cache to reflect rebased branch tips
    log::debug!("restack: refreshing cache");
    let _ = get_branches_and_parent_map(git);

    for branch in &restacked {
        utils::print_success(&format!("Restacked '{}'", branch));
    }

    Ok(())
}
