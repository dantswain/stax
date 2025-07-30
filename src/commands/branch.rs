use anyhow::Result;
use crate::git::GitRepo;
use crate::utils;

pub async fn run(name: &str) -> Result<()> {
    let git = GitRepo::open(".")?;
    
    if !git.is_clean()? {
        utils::print_warning("Working directory has uncommitted changes");
        if !utils::confirm("Continue anyway?")? {
            return Ok(());
        }
    }
    
    let current_branch = git.current_branch()?;
    
    git.create_branch(name, Some(&format!("refs/heads/{current_branch}")))?;
    git.checkout_branch(name)?;
    
    utils::print_success(&format!("Created and switched to branch '{name}'"));
    utils::print_info(&format!("Parent branch: {current_branch}"));
    
    Ok(())
}