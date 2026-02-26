use crate::git::GitRepo;
use crate::utils;
use anyhow::{anyhow, Result};
use dialoguer::{theme::ColorfulTheme, FuzzySelect};
use std::collections::HashMap;

const MAIN_BRANCHES: &[&str] = &["main", "master", "develop"];

fn is_main_branch(name: &str) -> bool {
    MAIN_BRANCHES.contains(&name)
}

fn pick_branch(prompt: &str, choices: &[String]) -> Result<String> {
    let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(choices)
        .default(0)
        .interact()?;
    Ok(choices[selection].clone())
}

/// Pre-compute commit hashes for all branches.
fn build_commit_cache(git: &GitRepo, branches: &[String]) -> Result<HashMap<String, String>> {
    branches
        .iter()
        .map(|b| {
            let hash = git.get_commit_hash(&format!("refs/heads/{b}"))?;
            Ok((b.clone(), hash))
        })
        .collect()
}

/// Find the parent of `branch` — the closest branch whose tip equals the
/// merge-base of itself and `branch`. O(n) merge-base calls.
fn find_parent(
    git: &GitRepo,
    branch: &str,
    all_branches: &[String],
    commits: &HashMap<String, String>,
) -> Result<Option<String>> {
    let branch_commit = &commits[branch];
    let mut best_parent = None;
    let mut min_distance = usize::MAX;

    for candidate in all_branches {
        if candidate == branch {
            continue;
        }
        let candidate_commit = &commits[candidate];
        if candidate_commit == branch_commit {
            continue;
        }
        if let Ok(merge_base) = git.get_merge_base(branch, candidate) {
            if merge_base.to_string() == *candidate_commit {
                let distance = git.count_commits_between(
                    &format!("refs/heads/{candidate}"),
                    &format!("refs/heads/{branch}"),
                )?;
                if distance < min_distance {
                    min_distance = distance;
                    best_parent = Some(candidate.clone());
                }
            }
        }
    }

    // Fall back to trunk
    if best_parent.is_none() {
        for name in MAIN_BRANCHES {
            if all_branches.iter().any(|b| b == name) && *name != branch {
                return Ok(Some(name.to_string()));
            }
        }
    }

    Ok(best_parent)
}

/// Find direct children of `branch` — branches whose closest parent is
/// `branch`. O(n) merge-base checks + O(k²) filtering among candidates.
fn find_children(
    git: &GitRepo,
    branch: &str,
    all_branches: &[String],
    commits: &HashMap<String, String>,
) -> Result<Vec<String>> {
    let branch_commit = &commits[branch];

    // Collect candidates: branches whose merge-base with `branch` equals branch's tip
    let mut candidates = Vec::new();
    for candidate in all_branches {
        if candidate == branch || is_main_branch(candidate) {
            continue;
        }
        let candidate_commit = &commits[candidate];
        if candidate_commit == branch_commit {
            continue;
        }
        if let Ok(merge_base) = git.get_merge_base(branch, candidate) {
            if merge_base.to_string() == *branch_commit {
                candidates.push(candidate.clone());
            }
        }
    }

    // Filter to direct children: remove candidates that are descendants of
    // another candidate (those are grandchildren, not children).
    let mut direct = Vec::new();
    for candidate in &candidates {
        let candidate_commit = &commits[candidate];
        let is_grandchild = candidates.iter().any(|other| {
            if other == candidate {
                return false;
            }
            let other_commit = &commits[other];
            if other_commit == candidate_commit {
                return false;
            }
            git.get_merge_base(candidate, other)
                .map(|mb| mb.to_string() == *other_commit)
                .unwrap_or(false)
        });
        if !is_grandchild {
            direct.push(candidate.clone());
        }
    }

    Ok(direct)
}

/// Get all local branches with pre-computed commit hashes.
/// Skips the expensive is_merged_into filtering — navigate's find_parent/
/// find_children already handle merged branches correctly via merge-base checks.
fn get_branches_with_cache(git: &GitRepo) -> Result<(Vec<String>, HashMap<String, String>)> {
    let all = git.get_branches()?;
    let commits = build_commit_cache(git, &all)?;
    Ok((all, commits))
}

pub async fn down() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;

    if is_main_branch(&current) {
        return Err(anyhow!("Already at the bottom of the stack"));
    }

    let (branches, commits) = get_branches_with_cache(&git)?;
    let parent = find_parent(&git, &current, &branches, &commits)?
        .ok_or_else(|| anyhow!("Already at the bottom of the stack"))?;

    git.checkout_branch(&parent)?;
    utils::print_success(&format!("Moved down to {}", parent));
    Ok(())
}

pub async fn up() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;
    let (branches, commits) = get_branches_with_cache(&git)?;
    let children = find_children(&git, &current, &branches, &commits)?;

    let target = match children.len() {
        0 => return Err(anyhow!("Already at the top of the stack")),
        1 => children[0].clone(),
        _ => pick_branch("Multiple children — pick a branch", &children)?,
    };

    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved up to {}", target));
    Ok(())
}

pub async fn bottom() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;
    let (branches, commits) = get_branches_with_cache(&git)?;

    if is_main_branch(&current) {
        let children = find_children(&git, &current, &branches, &commits)?;
        return match children.len() {
            0 => {
                utils::print_info("No stack branches above main");
                Ok(())
            }
            1 => {
                git.checkout_branch(&children[0])?;
                utils::print_success(&format!("Moved to bottom of stack: {}", children[0]));
                Ok(())
            }
            _ => {
                let target = pick_branch("Multiple stacks — pick a branch", &children)?;
                git.checkout_branch(&target)?;
                utils::print_success(&format!("Moved to bottom of stack: {}", target));
                Ok(())
            }
        };
    }

    // Walk parent chain until parent is a main branch (or None)
    let mut target = current.clone();
    loop {
        let parent = find_parent(&git, &target, &branches, &commits)?;
        match parent {
            Some(p) if is_main_branch(&p) => break,
            Some(p) => target = p,
            None => break,
        }
    }

    if target == current {
        utils::print_info("Already at the bottom of the stack");
        return Ok(());
    }

    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved to bottom of stack: {}", target));
    Ok(())
}

pub async fn top() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;
    let (branches, commits) = get_branches_with_cache(&git)?;

    // Walk children until reaching a leaf, prompting at forks
    let mut target = current.clone();
    loop {
        let children = find_children(&git, &target, &branches, &commits)?;
        match children.len() {
            0 => break,
            1 => target = children[0].clone(),
            _ => target = pick_branch("Multiple children — pick a branch", &children)?,
        }
    }

    if target == current {
        utils::print_info("Already at the top of the stack");
        return Ok(());
    }

    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved to top of stack: {}", target));
    Ok(())
}
