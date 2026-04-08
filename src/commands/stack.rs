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
        // No cached PRs — fetch from GitHub and cache for next time
        log::debug!("stack: no cached PRs, fetching from GitHub");
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

    // Build source-to-consumer lookup for merge annotations
    let mut source_to_consumers: HashMap<String, Vec<String>> = HashMap::new();
    for branch in stack.branches.values() {
        if branch.merge_sources.len() > 1 {
            // Skip the first source (it's the primary parent shown in tree structure)
            for source in &branch.merge_sources[1..] {
                source_to_consumers
                    .entry(source.clone())
                    .or_default()
                    .push(branch.name.clone());
            }
        }
    }

    println!("{}", "Stack Visualization".bold().underline());
    println!();

    let mut visited = HashSet::new();
    print_stack_tree(
        &stack,
        root,
        0,
        &mut visited,
        &in_scope,
        &source_to_consumers,
    );

    Ok(())
}

fn print_stack_tree(
    stack: &Stack,
    branch_name: &str,
    depth: usize,
    visited: &mut HashSet<String>,
    in_scope: &HashSet<String>,
    source_to_consumers: &HashMap<String, Vec<String>>,
) {
    use crate::commands::navigate::is_shadow_branch;

    if !in_scope.contains(branch_name) {
        return;
    }

    // Skip shadow branches from display
    if is_shadow_branch(branch_name) {
        // But still render their children (the consumer branch)
        if let Some(branch) = stack.branches.get(branch_name) {
            for child in &branch.children {
                print_stack_tree(stack, child, depth, visited, in_scope, source_to_consumers);
            }
        }
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

        // Show merge annotation if this branch has merge sources
        if !branch.merge_sources.is_empty() {
            let other_sources: Vec<&str> = branch
                .merge_sources
                .iter()
                .skip(1) // first source is the primary parent, shown by tree structure
                .map(|s| s.as_str())
                .collect();
            if !other_sources.is_empty() {
                line = format!(
                    "{} {}",
                    line,
                    format!("[+{}]", other_sources.join(", ")).dimmed()
                );
            }
        }

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

        // Render consumers from other sources (branches that include this one)
        if let Some(consumers) = source_to_consumers.get(branch_name) {
            for consumer in consumers {
                if !visited.contains(consumer) && in_scope.contains(consumer) {
                    if let Some(cb) = stack.branches.get(consumer) {
                        let other_sources: Vec<&str> = cb
                            .merge_sources
                            .iter()
                            .filter(|s| s.as_str() != branch_name)
                            .map(|s| s.as_str())
                            .collect();
                        let annotation = if other_sources.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", format!("[+{}]", other_sources.join(", ")).dimmed())
                        };
                        let consumer_indent = "  ".repeat(depth + 1);
                        println!("{}├─ {}{}", consumer_indent, consumer.dimmed(), annotation);
                    }
                }
            }
        }

        for child in &branch.children {
            print_stack_tree(
                stack,
                child,
                depth + 1,
                visited,
                in_scope,
                source_to_consumers,
            );
        }

        visited.remove(branch_name);
    }
}
