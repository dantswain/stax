use crate::git::GitRepo;
use crate::github::GitHubClient;
use crate::stack::Stack;
use crate::token_store;
use crate::utils;
use anyhow::Result;
use colored::*;

pub async fn run() -> Result<()> {
    let git = GitRepo::open(".")?;

    if !git.is_clean()? {
        utils::print_warning("Working directory has uncommitted changes");
    }

    let github_client = if let Some(token) = token_store::get_token() {
        if let Some(remote_url) = git.get_remote_url("origin") {
            GitHubClient::new(&token, &remote_url).ok()
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

    Ok(())
}
