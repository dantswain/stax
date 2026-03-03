use crate::git::GitRepo;
use crate::stack::Stack;
use crate::utils;
use anyhow::{anyhow, Result};
use std::collections::HashMap;

pub async fn run(all: bool, continue_rebase: bool) -> Result<()> {
    log::debug!("restack: all={}, continue={}", all, continue_rebase);
    let git = GitRepo::open(".")?;

    if continue_rebase {
        if git.is_rebase_in_progress() {
            utils::print_info("Continuing rebase...");
            git.rebase_continue()?;
            utils::print_success("Rebase continued successfully");
        }
        // Re-run with --all to restack remaining branches
        return Box::pin(run(true, false)).await;
    }

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

    let stack = Stack::analyze(&git, None).await?;
    let current_branch = stack.current_branch.clone();

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

    // Snapshot branch tips BEFORE any rebasing for --onto
    let mut old_tips: HashMap<String, String> = HashMap::new();
    for (branch, parent) in &branches_to_rebase {
        for name in [branch, parent] {
            if !old_tips.contains_key(name) {
                if let Ok(hash) = git.get_commit_hash(&format!("refs/heads/{name}")) {
                    old_tips.insert(name.clone(), hash);
                }
            }
        }
    }

    let mut restacked = Vec::new();

    log::debug!("restack: {} branches to rebase", branches_to_rebase.len());
    for (branch, parent) in &branches_to_rebase {
        utils::print_info(&format!("Rebasing '{}' onto '{}'", branch, parent));
        let old_parent_tip = old_tips.get(parent).map(|s| s.as_str());
        git.rebase_onto_with_base(branch, parent, old_parent_tip)?;
        restacked.push(branch.as_str());
    }

    // Restore original branch
    git.checkout_branch(&current_branch)?;

    for branch in &restacked {
        utils::print_success(&format!("Restacked '{}'", branch));
    }

    Ok(())
}
