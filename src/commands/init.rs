use anyhow::{anyhow, Result};
use dialoguer::{Input, Select, Confirm};
use crate::config::Config;
use crate::git::GitRepo;
use crate::github::GitHubClient;
use crate::token_store::TokenStore;
use crate::utils;

pub async fn run() -> Result<()> {
    utils::print_info("Setting up stax configuration...");
    
    let git = GitRepo::open(".")?;
    
    let mut config = Config::load()?;
    
    // Check for GitHub remote (optional)
    let remote_url = git.get_remote_url("origin").ok();
    
    if let Some(url) = &remote_url {
        utils::print_info(&format!("Detected repository: {url}"));
    } else {
        utils::print_warning("No 'origin' remote found.");
        utils::print_info("You can add a GitHub remote later with:");
        utils::print_info("  git remote add origin https://github.com/username/repo.git");
        utils::print_info("");
        
        let continue_setup = Confirm::new()
            .with_prompt("Continue setup without GitHub remote?")
            .default(true)
            .interact()?;
            
        if !continue_setup {
            utils::print_info("Setup cancelled. Add a GitHub remote and run 'stax init' again.");
            return Ok(());
        }
    }
    
    // Offer authentication options
    let auth_methods = vec![
        "Authenticate with browser (recommended)",
        "Enter personal access token manually"
    ];
    
    let auth_choice = Select::new()
        .with_prompt("How would you like to authenticate with GitHub?")
        .items(&auth_methods)
        .default(0)
        .interact()?;
    
    let github_token = match auth_choice {
        0 => {
            // Browser OAuth authentication - works without remote URL
            utils::print_info("Starting browser authentication...");
            match GitHubClient::authenticate_with_oauth().await {
                Ok(token) => token,
                Err(e) => {
                    utils::print_error(&format!("Browser authentication failed: {e}"));
                    utils::print_info("Falling back to manual token entry...");
                    
                    let token = Input::<String>::new()
                        .with_prompt("GitHub personal access token")
                        .interact_text()?;
                    
                    if token.trim().is_empty() {
                        return Err(anyhow!("GitHub token is required"));
                    }
                    
                    token
                }
            }
        },
        1 => {
            // Manual token entry
            let token = Input::<String>::new()
                .with_prompt("GitHub personal access token")
                .interact_text()?;
            
            if token.trim().is_empty() {
                return Err(anyhow!("GitHub token is required"));
            }
            
            token
        },
        _ => return Err(anyhow!("Invalid authentication method selected")),
    };
    
    // Store token securely instead of in config
    TokenStore::store_token(&github_token)?;
    utils::print_success("GitHub token stored securely");
    
    let base_branches = vec!["main", "master", "develop"];
    let default_selection = base_branches.iter().position(|&x| x == "main").unwrap_or(0);
    
    let base_branch_idx = Select::new()
        .with_prompt("Default base branch")
        .items(&base_branches)
        .default(default_selection)
        .interact()?;
    
    config.set("default_base_branch", base_branches[base_branch_idx])?;
    
    let auto_push = Confirm::new()
        .with_prompt("Automatically push branches when creating PRs?")
        .default(true)
        .interact()?;
    
    config.set("auto_push", &auto_push.to_string())?;
    
    let draft_prs = Confirm::new()
        .with_prompt("Create draft PRs by default?")
        .default(false)
        .interact()?;
    
    config.set("draft_prs", &draft_prs.to_string())?;
    
    let add_pr_template = Confirm::new()
        .with_prompt("Add a default PR template?")
        .default(false)
        .interact()?;
    
    if add_pr_template {
        let template = Input::<String>::new()
            .with_prompt("PR template (use \\n for new lines)")
            .default("## Summary\n\n## Testing\n\n## Notes".to_string())
            .interact_text()?;
        
        config.set("pr_template", &template.replace("\\n", "\n"))?;
    }
    
    config.save()?;
    
    utils::print_success("Configuration saved successfully!");
    utils::print_info("You can modify settings later with: stax config set <key> <value>");
    
    Ok(())
}