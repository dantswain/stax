use crate::git::GitRepo;
use crate::utils;
use anyhow::{anyhow, Result};
use dialoguer::{theme::ColorfulTheme, FuzzySelect};

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

/// Find the parent of `branch` — the closest branch whose tip equals the
/// merge-base of itself and `branch`. O(n) in the number of local branches.
fn find_parent(git: &GitRepo, branch: &str, all_branches: &[String]) -> Result<Option<String>> {
    let branch_commit = git.get_commit_hash(&format!("refs/heads/{branch}"))?;
    let mut best_parent = None;
    let mut min_distance = usize::MAX;

    for candidate in all_branches {
        if candidate == branch {
            continue;
        }
        let candidate_commit = git.get_commit_hash(&format!("refs/heads/{candidate}"))?;
        if candidate_commit == branch_commit {
            continue;
        }
        if let Ok(merge_base) = git.get_merge_base(branch, candidate) {
            if merge_base.to_string() == candidate_commit {
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
fn find_children(git: &GitRepo, branch: &str, all_branches: &[String]) -> Result<Vec<String>> {
    let branch_commit = git.get_commit_hash(&format!("refs/heads/{branch}"))?;

    // Collect candidates: branches whose merge-base with `branch` equals branch's tip
    let mut candidates = Vec::new();
    for candidate in all_branches {
        if candidate == branch || is_main_branch(candidate) {
            continue;
        }
        let candidate_commit = git.get_commit_hash(&format!("refs/heads/{candidate}"))?;
        if candidate_commit == branch_commit {
            continue;
        }
        if let Ok(merge_base) = git.get_merge_base(branch, candidate) {
            if merge_base.to_string() == branch_commit {
                candidates.push(candidate.clone());
            }
        }
    }

    // Filter to direct children: remove candidates that are descendants of
    // another candidate (those are grandchildren, not children).
    let mut direct = Vec::new();
    for candidate in &candidates {
        let candidate_commit = git.get_commit_hash(&format!("refs/heads/{candidate}"))?;
        let is_grandchild = candidates.iter().any(|other| {
            if other == candidate {
                return false;
            }
            let other_commit = git
                .get_commit_hash(&format!("refs/heads/{other}"))
                .unwrap_or_default();
            if other_commit == candidate_commit {
                return false;
            }
            git.get_merge_base(candidate, other)
                .map(|mb| mb.to_string() == other_commit)
                .unwrap_or(false)
        });
        if !is_grandchild {
            direct.push(candidate.clone());
        }
    }

    Ok(direct)
}

/// Get all non-merged local branches (cheap: one is_merged check per branch).
fn get_active_branches(git: &GitRepo) -> Result<Vec<String>> {
    let all = git.get_branches()?;
    let trunk = MAIN_BRANCHES
        .iter()
        .find(|name| all.contains(&name.to_string()))
        .map(|s| s.to_string());

    Ok(all
        .into_iter()
        .filter(|b| is_main_branch(b) || trunk.as_ref().is_none_or(|t| !is_merged_into(git, b, t)))
        .collect())
}

fn is_merged_into(git: &GitRepo, branch: &str, trunk: &str) -> bool {
    let branch_hash = git.get_commit_hash(&format!("refs/heads/{branch}")).ok();
    let merge_base = git.get_merge_base(branch, trunk).ok();
    matches!((branch_hash, merge_base), (Some(bh), Some(mb)) if bh == mb.to_string())
}

pub async fn down() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;

    if is_main_branch(&current) {
        return Err(anyhow!("Already at the bottom of the stack"));
    }

    let branches = get_active_branches(&git)?;
    let parent = find_parent(&git, &current, &branches)?
        .ok_or_else(|| anyhow!("Already at the bottom of the stack"))?;

    git.checkout_branch(&parent)?;
    utils::print_success(&format!("Moved down to {}", parent));
    Ok(())
}

pub async fn up() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;
    let branches = get_active_branches(&git)?;
    let children = find_children(&git, &current, &branches)?;

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
    let branches = get_active_branches(&git)?;

    if is_main_branch(&current) {
        let children = find_children(&git, &current, &branches)?;
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
        let parent = find_parent(&git, &target, &branches)?;
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
    let branches = get_active_branches(&git)?;

    // Walk children until reaching a leaf, prompting at forks
    let mut target = current.clone();
    loop {
        let children = find_children(&git, &target, &branches)?;
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
