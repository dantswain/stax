use crate::{
    config::Config, git::GitRepo, github::GitHubClient, stack, stack::Stack, token_store, utils,
};
use anyhow::{anyhow, Result};

pub async fn run(no_restack: bool, force: bool, continue_rebase: bool) -> Result<()> {
    let git = GitRepo::open(".")?;
    let config = Config::load()?;

    if continue_rebase {
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

    // 5. Analyze stack to find merged/closed PRs
    let stack = Stack::analyze(&git, Some(&github)).await?;
    let mut merged = find_merged_branches(&stack, trunk);

    // Also check branches that are locally merged into trunk but were
    // filtered out of Stack::analyze (their tip is an ancestor of trunk).
    let locally_merged = find_locally_merged_branches(&git, &github, &stack, trunk).await;
    merged.extend(locally_merged);

    // 6. Delete merged/closed branches
    if !merged.is_empty() {
        delete_merged_branches(&git, &merged, &original_branch, trunk, force)?;
    } else {
        utils::print_info("No merged or closed branches to clean up");
    }

    // 7. Restack
    if !no_restack {
        restack_branches(&git, &config, &github, &original_branch).await?;
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

    utils::print_success("Sync complete");
    Ok(())
}

/// Find branches whose PRs are merged or closed.
fn find_merged_branches(stack: &Stack, trunk: &str) -> Vec<(String, String)> {
    let main_branches = ["main", "master", "develop"];

    stack
        .branches
        .values()
        .filter_map(|branch| {
            if main_branches.contains(&branch.name.as_str()) || branch.name == trunk {
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

/// Find branches that are locally merged into trunk but were filtered out of
/// Stack::analyze(). For each, check GitHub PR state to confirm it was merged/closed.
async fn find_locally_merged_branches(
    git: &GitRepo,
    github: &GitHubClient,
    analyzed_stack: &Stack,
    trunk: &str,
) -> Vec<(String, String)> {
    let main_branches = ["main", "master", "develop"];
    let all_branches = match git.get_branches() {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    let candidates: Vec<String> = all_branches
        .into_iter()
        .filter(|b| {
            !main_branches.contains(&b.as_str())
                && b != trunk
                && !analyzed_stack.branches.contains_key(b)
                && stack::is_merged_into(git, b, trunk)
        })
        .collect();

    if candidates.is_empty() {
        return Vec::new();
    }

    // Fetch PR status in parallel
    let handles: Vec<_> = candidates
        .into_iter()
        .map(|branch| {
            let gh = github.clone();
            tokio::spawn(async move {
                let pr = gh.get_pr_for_branch(&branch).await;
                (branch, pr)
            })
        })
        .collect();

    let mut result = Vec::new();
    for handle in handles {
        if let Ok((branch, Ok(Some(pr)))) = handle.await {
            match pr.state.as_str() {
                "merged" => result.push((branch, format!("PR #{} merged", pr.number))),
                "closed" => result.push((branch, format!("PR #{} closed", pr.number))),
                _ => {}
            }
        }
    }

    result
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
    _original_branch: &str,
) -> Result<()> {
    restack_all(git, Some(github)).await
}

/// Shared restack logic used by both normal sync and --continue.
async fn restack_all(git: &GitRepo, github: Option<&GitHubClient>) -> Result<()> {
    let stack = Stack::analyze(git, github).await?;
    let main_branches = ["main", "master", "develop"];

    // Collect branches with their parents and depth for topological ordering.
    let mut to_rebase: Vec<(String, String, usize)> = Vec::new();

    for branch in stack.branches.values() {
        if main_branches.contains(&branch.name.as_str()) {
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

    // Snapshot ALL branch tips BEFORE any rebasing.
    // When rebasing B onto A, we use A's OLD tip as the --onto base so that
    // only B's own commits get replayed (not A's original commits).
    let mut old_tips: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (branch, parent, _) in &to_rebase {
        for name in [branch, parent] {
            if !old_tips.contains_key(name) {
                if let Ok(hash) = git.get_commit_hash(&format!("refs/heads/{name}")) {
                    old_tips.insert(name.clone(), hash);
                }
            }
        }
    }

    let mut restacked = Vec::new();

    for (branch, parent, _) in &to_rebase {
        utils::print_info(&format!("Rebasing '{branch}' onto '{parent}'"));
        // Use the parent's pre-rebase tip so --onto only replays branch's own commits
        let old_parent_tip = old_tips.get(parent).map(|s| s.as_str());
        match git.rebase_onto_with_base(branch, parent, old_parent_tip) {
            Ok(()) => restacked.push(branch.as_str()),
            Err(e) => return Err(e),
        }
    }

    for branch in &restacked {
        utils::print_success(&format!("Restacked '{branch}'"));
    }

    Ok(())
}

/// Continue a sync after the user has resolved rebase conflicts.
/// Finishes the in-progress rebase, then restacks any remaining branches.
async fn continue_after_conflicts(git: &GitRepo, _config: &Config) -> Result<()> {
    if git.is_rebase_in_progress() {
        utils::print_info("Continuing rebase...");
        git.rebase_continue()?;
        utils::print_success("Rebase continued successfully");
    }

    // Re-analyze and restack remaining branches
    restack_all(git, None).await?;
    utils::print_success("Sync complete");
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
