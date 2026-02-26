use crate::git::GitRepo;
use crate::stack::Stack;
use crate::utils;
use anyhow::{anyhow, Result};
use dialoguer::{theme::ColorfulTheme, FuzzySelect};

const MAIN_BRANCHES: &[&str] = &["main", "master", "develop"];

fn is_main_branch(name: &str) -> bool {
    MAIN_BRANCHES.contains(&name)
}

fn pick_child(children: &[String]) -> Result<String> {
    let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Multiple children — pick a branch")
        .items(children)
        .default(0)
        .interact()?;
    Ok(children[selection].clone())
}

pub async fn down() -> Result<()> {
    let git = GitRepo::open(".")?;
    let stack = Stack::analyze(&git, None).await?;

    let current = &stack.current_branch;
    let branch = stack
        .branches
        .get(current)
        .ok_or_else(|| anyhow!("Current branch '{}' not found in stack", current))?;

    let parent = branch
        .parent
        .as_ref()
        .ok_or_else(|| anyhow!("Already at the bottom of the stack"))?;

    git.checkout_branch(parent)?;
    utils::print_success(&format!("Moved down to {}", parent));
    Ok(())
}

pub async fn up() -> Result<()> {
    let git = GitRepo::open(".")?;
    let stack = Stack::analyze(&git, None).await?;

    let current = &stack.current_branch;
    let branch = stack
        .branches
        .get(current)
        .ok_or_else(|| anyhow!("Current branch '{}' not found in stack", current))?;

    let target = match branch.children.len() {
        0 => return Err(anyhow!("Already at the top of the stack")),
        1 => branch.children[0].clone(),
        _ => pick_child(&branch.children)?,
    };

    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved up to {}", target));
    Ok(())
}

pub async fn bottom() -> Result<()> {
    let git = GitRepo::open(".")?;
    let stack = Stack::analyze(&git, None).await?;

    let current = &stack.current_branch;
    let branch = stack
        .branches
        .get(current)
        .ok_or_else(|| anyhow!("Current branch '{}' not found in stack", current))?;

    // If already on a main branch, go to its child (the bottom of the stack)
    if is_main_branch(current) {
        return match branch.children.len() {
            0 => {
                utils::print_info("No stack branches above main");
                Ok(())
            }
            1 => {
                git.checkout_branch(&branch.children[0])?;
                utils::print_success(&format!("Moved to bottom of stack: {}", branch.children[0]));
                Ok(())
            }
            _ => {
                let target = pick_child(&branch.children)?;
                git.checkout_branch(&target)?;
                utils::print_success(&format!("Moved to bottom of stack: {}", target));
                Ok(())
            }
        };
    }

    // Walk parent chain until we find a branch whose parent is a main branch (or None)
    let mut target = current.clone();
    loop {
        let b = stack
            .branches
            .get(&target)
            .ok_or_else(|| anyhow!("Branch '{}' not found in stack", target))?;

        match &b.parent {
            Some(parent) if is_main_branch(parent) => break,
            Some(parent) => target = parent.clone(),
            None => break,
        }
    }

    if target == *current {
        utils::print_info("Already at the bottom of the stack");
        return Ok(());
    }

    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved to bottom of stack: {}", target));
    Ok(())
}

pub async fn top() -> Result<()> {
    let git = GitRepo::open(".")?;
    let stack = Stack::analyze(&git, None).await?;

    let current = &stack.current_branch;
    if !stack.branches.contains_key(current) {
        return Err(anyhow!("Current branch '{}' not found in stack", current));
    }

    // Walk children until reaching a leaf, prompting at forks
    let mut target = current.clone();
    loop {
        let b = stack
            .branches
            .get(&target)
            .ok_or_else(|| anyhow!("Branch '{}' not found in stack", target))?;

        match b.children.len() {
            0 => break,
            1 => target = b.children[0].clone(),
            _ => target = pick_child(&b.children)?,
        }
    }

    if target == *current {
        utils::print_info("Already at the top of the stack");
        return Ok(());
    }

    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved to top of stack: {}", target));
    Ok(())
}
