use crate::commands::navigate::get_branches_and_parent_map;
use crate::git::GitRepo;
use crate::github::GitHubClient;
use crate::stack::Stack;
use crate::token_store;
use anyhow::Result;
use colored::*;
use std::collections::HashSet;

pub async fn run() -> Result<()> {
    log::debug!("stack: visualizing stack");
    let git = GitRepo::open(".")?;

    let github_client = if let Some(token) = token_store::get_token() {
        if let Some(remote_url) = git.get_remote_url("origin") {
            GitHubClient::new(&token, &remote_url).ok()
        } else {
            None
        }
    } else {
        None
    };

    let current_branch = git.current_branch()?;
    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&git)?;
    let stack = Stack::from_parent_map(
        &git,
        &current_branch,
        github_client.as_ref(),
        &branches,
        &commits,
        &merged,
        &parent_map,
    )
    .await?;

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
