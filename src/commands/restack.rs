use crate::git::GitRepo;
use crate::stack::Stack;
use crate::utils;
use anyhow::{anyhow, Result};

pub async fn run(all: bool) -> Result<()> {
    let git = GitRepo::open(".")?;

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

    let mut restacked = Vec::new();

    for (branch, parent) in &branches_to_rebase {
        utils::print_info(&format!("Rebasing '{}' onto '{}'", branch, parent));
        if let Err(e) = git.rebase_onto(branch, parent) {
            // Restore original branch before returning error
            let _ = git.checkout_branch(&current_branch);
            return Err(e);
        }
        restacked.push(branch.as_str());
    }

    // Restore original branch
    git.checkout_branch(&current_branch)?;

    for branch in &restacked {
        utils::print_success(&format!("Restacked '{}'", branch));
    }

    Ok(())
}
