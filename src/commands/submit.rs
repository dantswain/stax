use anyhow::{anyhow, Result};
use crate::{config::Config, git::GitRepo, github::GitHubClient, stack::Stack, token_store, utils};
use dialoguer::{theme::ColorfulTheme, Input, Confirm};

pub async fn run(all: bool) -> Result<()> {
    let git = GitRepo::open(".")?;
    let config = Config::load()?;
    
    // Check if we have a GitHub token
    let token = token_store::get_token()
        .ok_or_else(|| anyhow!("No GitHub token found. Run 'stax init' to authenticate first."))?;
    
    // Get the remote URL for GitHub client
    let remote_url = git.get_remote_url("origin")
        .ok_or_else(|| anyhow!("No 'origin' remote found. Add a GitHub remote first."))?;
    
    let github = GitHubClient::new(&token, &remote_url)?;
    let stack = Stack::analyze(&git, Some(&github)).await?;
    
    if all {
        submit_stack(&git, &github, &stack, &config).await
    } else {
        submit_current_branch(&git, &github, &stack, &config).await
    }
}

async fn submit_current_branch(
    git: &GitRepo,
    github: &GitHubClient,
    stack: &Stack,
    config: &Config,
) -> Result<()> {
    let current_branch = &stack.current_branch;
    
    // Don't create PRs for main branches
    if ["main", "master", "develop"].contains(&current_branch.as_str()) {
        return Err(anyhow!("Cannot create PR for main branch '{current_branch}'"));
    }
    
    let current_stack_branch = stack.branches.get(current_branch)
        .ok_or_else(|| anyhow!("Current branch not found in stack"))?;
    
    // Check if PR already exists
    if let Some(existing_pr) = &current_stack_branch.pull_request {
        utils::print_info(&format!("PR already exists: {}", existing_pr.html_url));
        
        // Ask if they want to update it
        let update = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Update existing PR?")
            .default(false)
            .interact()?;
            
        if update {
            update_existing_pr(github, existing_pr.number, config).await?;
        }
        return Ok(());
    }
    
    create_new_pr(git, github, current_branch, stack, config).await
}

async fn submit_stack(
    git: &GitRepo,
    github: &GitHubClient,
    stack: &Stack,
    config: &Config,
) -> Result<()> {
    let current_branch = &stack.current_branch;
    let stack_branches = stack.get_stack_for_branch(current_branch);
    
    // Filter out main branches and branches that already have PRs
    let branches_to_submit: Vec<_> = stack_branches
        .iter()
        .filter(|b| {
            !["main", "master", "develop"].contains(&b.name.as_str()) &&
            b.pull_request.is_none()
        })
        .collect();
    
    if branches_to_submit.is_empty() {
        utils::print_info("All branches in stack already have PRs");
        return Ok(());
    }
    
    utils::print_info(&format!("Creating PRs for {} branches...", branches_to_submit.len()));
    
    for branch in branches_to_submit {
        utils::print_info(&format!("Creating PR for branch: {}", branch.name));
        create_new_pr(git, github, &branch.name, stack, config).await?;
    }
    
    utils::print_success("Stack submission completed!");
    Ok(())
}

async fn create_new_pr(
    git: &GitRepo,
    github: &GitHubClient,
    branch_name: &str,
    stack: &Stack,
    config: &Config,
) -> Result<()> {
    let branch = stack.branches.get(branch_name)
        .ok_or_else(|| anyhow!("Branch not found in stack"))?;
    
    // Determine the base branch (parent or default)
    let base_branch = branch.parent.as_ref()
        .unwrap_or(&config.default_base_branch);
    
    // Auto-push if configured
    if config.auto_push {
        if !git.has_remote_branch(branch_name)? {
            utils::print_info(&format!("Pushing branch '{branch_name}' to remote..."));
            git.push_branch(branch_name, false)?;
        }
    } else {
        // Check if branch exists on remote
        if !git.has_remote_branch(branch_name)? {
            let should_push = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Branch '{branch_name}' not found on remote. Push now?"))
                .default(true)
                .interact()?;
                
            if should_push {
                git.push_branch(branch_name, false)?;
            } else {
                return Err(anyhow!("Cannot create PR without pushing branch to remote"));
            }
        }
    }
    
    // Generate title from branch name (convert kebab-case to Title Case)
    let default_title = branch_name
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    
    // Get PR title from user
    let title: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("PR title")
        .default(default_title)
        .interact_text()?;
    
    if title.trim().is_empty() {
        return Err(anyhow!("PR title cannot be empty"));
    }
    
    // Get PR body (use template if available)
    let default_body = config.pr_template.as_deref().unwrap_or(
        "## Summary\n\n<!-- Describe your changes -->\n\n## Testing\n\n<!-- How did you test this? -->"
    );
    
    let body: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("PR description")
        .default(default_body.to_string())
        .interact_text()?;
    
    // Create the PR
    utils::print_info(&format!("Creating PR: '{title}' ({branch_name} → {base_branch})"));
    
    let pr = github.create_pull_request(
        &title,
        &body,
        branch_name,
        base_branch,
        config.draft_prs,
    ).await?;
    
    utils::print_success(&format!("PR created: {}", pr.html_url));
    
    if config.draft_prs {
        utils::print_info("PR created as draft. Mark as ready for review when complete.");
    }
    
    Ok(())
}

async fn update_existing_pr(
    github: &GitHubClient,
    pr_number: u64,
    _config: &Config,
) -> Result<()> {
    // Get new title
    let title: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("New PR title (leave empty to keep current)")
        .allow_empty(true)
        .interact_text()?;
    
    // Get new body
    let body: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("New PR description (leave empty to keep current)")
        .allow_empty(true)
        .interact_text()?;
    
    let title_option = if title.trim().is_empty() { None } else { Some(title.as_str()) };
    let body_option = if body.trim().is_empty() { None } else { Some(body.as_str()) };
    
    if title_option.is_none() && body_option.is_none() {
        utils::print_info("No changes made to PR");
        return Ok(());
    }
    
    let updated_pr = github.update_pull_request(
        pr_number,
        title_option,
        body_option,
        None, // Don't change base branch
    ).await?;
    
    utils::print_success(&format!("PR updated: {}", updated_pr.html_url));
    Ok(())
}