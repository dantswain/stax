use crate::config::Config;
use crate::git::GitRepo;
use crate::github::GitHubClient;
use crate::stack::Stack;
use crate::utils;
use anyhow::Result;
use colored::*;

pub async fn run() -> Result<()> {
    let git = GitRepo::open(".")?;
    let config = Config::load()?;

    if !git.is_clean()? {
        utils::print_warning("Working directory has uncommitted changes");
    }

    let github_client = if let Some(token) = &config.github_token {
        if let Some(remote_url) = git.get_remote_url("origin") {
            Some(GitHubClient::new(token, &remote_url)?)
        } else {
            None
        }
    } else {
        None
    };

    let stack = Stack::analyze(&git, github_client.as_ref()).await?;

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

    println!();
    println!("{}", "Stack Overview".bold().underline());

    let current_stack = stack.get_stack_for_branch(&stack.current_branch);

    if current_stack.len() <= 1 {
        utils::print_info("Current branch is not part of a stack");
        return Ok(());
    }

    for (i, branch) in current_stack.iter().enumerate() {
        let indent = "  ".repeat(i);
        let mut line = format!("{}{}", indent, branch.name);

        if branch.is_current {
            line = format!("{} {}", line.green().bold(), "← current".dimmed());
        }

        if let Some(pr) = &branch.pull_request {
            let status_color = match pr.state.as_str() {
                "open" => "green",
                "draft" => "yellow",
                "closed" => "red",
                "merged" => "blue",
                _ => "white",
            };

            line = format!(
                "{} (PR #{} - {})",
                line,
                pr.number.to_string().color(status_color),
                pr.state.color(status_color)
            );
        } else {
            line = format!("{} {}", line, "(no PR)".dimmed());
        }

        println!("{line}");
    }

    println!();

    if !stack.is_stack_clean(&stack.current_branch) {
        utils::print_warning("Some PRs in the stack have issues");
    }

    let has_upstream_branches = stack
        .branches
        .values()
        .any(|b| git.get_branch_upstream(&b.name).unwrap_or(None).is_some());

    if !has_upstream_branches {
        utils::print_info("No branches are tracking upstream. Use 'stax submit' to create PRs.");
    }

    Ok(())
}
