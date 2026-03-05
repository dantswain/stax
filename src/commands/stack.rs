use crate::cache::{CachedPullRequest, StackCache};
use crate::commands::navigate::get_branches_and_parent_map;
use crate::git::GitRepo;
use crate::github::{GitHubClient, PullRequest};
use crate::stack::Stack;
use crate::token_store;
use crate::utils;
use anyhow::Result;
use colored::*;
use std::collections::{HashMap, HashSet};

pub async fn run() -> Result<()> {
    log::debug!("stack: visualizing stack");
    let git = GitRepo::open(".")?;

    let current_branch = git.current_branch()?;
    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&git)?;

    // PR base_ref overrides are already applied by get_branches_and_parent_map().
    // Load PR data for display purposes (status symbols, PR numbers).
    let mut cache = StackCache::new(&git.git_dir());
    let cached_prs = cache
        .load()
        .map(|data| data.pull_requests.clone())
        .unwrap_or_default();

    let prs: HashMap<String, PullRequest> = if !cached_prs.is_empty() {
        log::debug!("stack: using {} cached PRs for display", cached_prs.len());
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
        // No cached PRs — fetch from GitHub and cache for next time
        log::debug!("stack: no cached PRs, fetching from GitHub");
        let mut fetched = HashMap::new();
        if let Some(token) = token_store::get_token() {
            if let Some(remote_url) = git.get_remote_url("origin") {
                if let Ok(gh) = GitHubClient::new(&token, &remote_url) {
                    if let Ok(open_prs) = gh.get_open_pull_requests().await {
                        let cached: HashMap<String, CachedPullRequest> = open_prs
                            .iter()
                            .map(|pr| {
                                (
                                    pr.head_ref.clone(),
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
                        cache.save_pull_requests(&cached);

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
    if !mismatches.is_empty() {
        println!();
        utils::print_topology_warning(&mismatches);
        println!();
    }

    // Scope to only the current branch's stack (ancestors + descendants)
    let in_scope: HashSet<String> = stack
        .get_stack_for_branch(&stack.current_branch)
        .iter()
        .map(|b| b.name.clone())
        .collect();

    // Find the root of the current branch's stack
    let mut root = stack.current_branch.as_str();
    while let Some(branch) = stack.branches.get(root) {
        match &branch.parent {
            Some(parent) => root = parent,
            None => break,
        }
    }

    println!("{}", "Stack Visualization".bold().underline());
    println!();

    let mut visited = HashSet::new();
    print_stack_tree(&stack, root, 0, &mut visited, &in_scope);

    Ok(())
}

fn print_stack_tree(
    stack: &Stack,
    branch_name: &str,
    depth: usize,
    visited: &mut HashSet<String>,
    in_scope: &HashSet<String>,
) {
    if !in_scope.contains(branch_name) {
        return;
    }

    if visited.contains(branch_name) {
        let indent = "  ".repeat(depth);
        let connector = if depth > 0 { "├─ " } else { "" };
        println!(
            "{}{}[CYCLE DETECTED: {}]",
            indent,
            connector,
            branch_name.red()
        );
        return;
    }

    if let Some(branch) = stack.branches.get(branch_name) {
        visited.insert(branch_name.to_string());

        let indent = "  ".repeat(depth);
        let connector = if depth > 0 { "├─ " } else { "" };

        let mut line = format!("{}{}{}", indent, connector, branch.name);

        if branch.is_current {
            line = format!("{} {}", line.green().bold(), "← current".dimmed());
        }

        if let Some(pr) = &branch.pull_request {
            let status_symbol = match pr.state.as_str() {
                "open" => "●".green(),
                "draft" => "◐".yellow(),
                "closed" => "○".red(),
                "merged" => "✓".blue(),
                _ => "?".white(),
            };

            line = format!("{} {} PR #{}", line, status_symbol, pr.number);
        } else {
            line = format!("{} {}", line, "○".dimmed());
        }

        println!("{line}");

        for child in &branch.children {
            print_stack_tree(stack, child, depth + 1, visited, in_scope);
        }

        visited.remove(branch_name);
    }
}
