use crate::cache::{RestackState, StackCache};
use crate::commands::navigate::get_branches_and_parent_map;
use crate::git::GitRepo;
use crate::stack::Stack;
use crate::utils;
use anyhow::{anyhow, Result};

pub async fn run(all: bool, continue_rebase: bool) -> Result<()> {
    log::debug!("restack: all={}, continue={}", all, continue_rebase);
    let git = GitRepo::open(".")?;

    if continue_rebase {
        if git.is_rebase_in_progress() {
            utils::print_info("Continuing rebase...");
            git.rebase_continue()?;
            utils::print_success("Rebase continued successfully");
        }
        // State file is preserved — do_restack will load old_tips from it
        return do_restack(&git, true).await;
    }

    // Fresh start — clear any stale state from a previous aborted restack
    StackCache::new(&git.git_dir()).clear_restack_state();

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

    log::debug!("restack: {} branches to rebase", branches_to_rebase.len());
    for (branch, parent) in &branches_to_rebase {
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
