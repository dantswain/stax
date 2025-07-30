use anyhow::{anyhow, Result};
use dialoguer::{Input, Select, Confirm};
use crate::config::Config;
use crate::git::GitRepo;
use crate::utils;

pub async fn run() -> Result<()> {
    utils::print_info("Setting up stax configuration...");
    
    let git = GitRepo::open(".")?;
    
    let mut config = Config::load()?;
    
    let github_token = Input::<String>::new()
        .with_prompt("GitHub personal access token")
        .interact_text()?;
    
    if github_token.trim().is_empty() {
        return Err(anyhow!("GitHub token is required"));
    }
    
    config.set("github_token", &github_token)?;
    
    let remote_url = git.get_remote_url("origin")
        .map_err(|_| anyhow!("No 'origin' remote found. Please add a GitHub remote first."))?;
    
    utils::print_info(&format!("Detected repository: {remote_url}"));
    
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