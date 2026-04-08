use crate::cache::{CachedPullRequest, StackCache};
use crate::commands::navigate::{get_branches_and_parent_map, maybe_spawn_cache_refresh};
use crate::git::GitRepo;
use crate::github::{GitHubClient, PullRequest};
use crate::stack::Stack;
use crate::token_store;
use crate::utils;
use anyhow::Result;
use colored::*;
use std::collections::{HashMap, HashSet};

pub async fn run() -> Result<()> {
    log::debug!("status: gathering repository status");
    let git = GitRepo::open(".")?;

    if !git.is_clean()? {
        utils::print_warning("Working directory has uncommitted changes");
    }

    let current_branch = git.current_branch()?;
    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&git)?;

    // Load PR data from cache for display
    let mut cache = StackCache::new(&git.git_dir());
    let cached_prs = cache
        .load()
        .map(|data| data.pull_requests.clone())
        .unwrap_or_default();

    let prs: HashMap<String, PullRequest> = if !cached_prs.is_empty() {
        log::debug!("status: using {} cached PRs for display", cached_prs.len());
        maybe_spawn_cache_refresh(&git);
        cached_prs
            .values()
            .map(|cpr| {
                (
                    cpr.head_ref.clone(),
                    PullRequest {
                        number: cpr.number,
                        title: String::new(),
                        body: None,
                        state: cpr.state.clone(),
                        head_ref: cpr.head_ref.clone(),
                        base_ref: cpr.base_ref.clone(),
                        html_url: cpr.html_url.clone(),
                        draft: cpr.draft,
                    },
                )
            })
            .collect()
    } else {
        // No cached PRs — try fetching from GitHub
        log::debug!("status: no cached PRs, fetching from GitHub");
        let mut fetched = HashMap::new();
        if let Some(token) = token_store::get_token() {
            if let Some(remote_url) = git.get_remote_url("origin") {
                if let Ok(gh) = GitHubClient::new(&token, &remote_url) {
                    if let Ok(open_prs) = gh.get_open_pull_requests().await {
                        let incoming: Vec<CachedPullRequest> = open_prs
                            .iter()
                            .map(|pr| CachedPullRequest {
                                number: pr.number,
                                state: pr.state.clone(),
                                head_ref: pr.head_ref.clone(),
                                base_ref: pr.base_ref.clone(),
                                html_url: pr.html_url.clone(),
                                draft: pr.draft,
                            })
                            .collect();
                        let branch_set: HashSet<String> = branches.iter().cloned().collect();
                        cache.merge_pull_requests(&incoming, &branch_set);

                        for pr in open_prs {
                            fetched.insert(pr.head_ref.clone(), pr);
                        }
                    }
                }
            }
        }
        fetched
    };

    let mut stack = Stack::from_parent_map(
        &git,
        &current_branch,
        None,
        &branches,
        &commits,
        &merged,
        &parent_map,
    )
    .await?;

    // Inject PR data into the stack for display
    for pr in prs.values() {
        if let Some(branch) = stack.branches.get_mut(&pr.head_ref) {
            branch.pull_request = Some(pr.clone());
        }
    }

    // Check for topology issues
    let mismatches = crate::commands::repair::check_topology_from_cache(&git, &parent_map);

    println!("{}", "Repository Status".bold().underline());
    println!("Current branch: {}", stack.current_branch.green().bold());

    if let Some(current_branch) = stack.branches.get(&stack.current_branch) {
        if let Some(pr) = &current_branch.pull_request {
            println!(
                "Pull Request: {} ({})",
                pr.html_url.blue(),
                pr.state.yellow()
            );
        }

        if let Some(parent) = &current_branch.parent {
            println!("Parent branch: {}", parent.cyan());
        }

        if !current_branch.children.is_empty() {
            println!(
                "Child branches: {}",
                current_branch.children.join(", ").cyan()
            );
        }
    }

    if !mismatches.is_empty() {
        println!();
        utils::print_topology_warning(&mismatches);
    }

    Ok(())
}
