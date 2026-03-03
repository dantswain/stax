use crate::git::GitRepo;
use crate::utils;
use anyhow::Result;

pub async fn run(name: Option<&str>) -> Result<()> {
    let git = GitRepo::open(".")?;

    if !git.is_clean()? {
        utils::print_warning("Working directory has uncommitted changes");
        if !utils::confirm("Continue anyway?")? {
            return Ok(());
        }
    }

    let branch_name = match name {
        Some(name) => name.to_string(),
        None => {
            let input = utils::prompt("Enter branch name")?;
            if input.is_empty() {
                utils::print_error("Branch name cannot be empty");
                return Ok(());
            }
            input
        }
    };

    let current_branch = git.current_branch()?;
    log::debug!(
        "create: branch='{}', parent='{}'",
        branch_name,
        current_branch
    );

    git.create_branch(&branch_name, Some(&format!("refs/heads/{current_branch}")))?;
    git.checkout_branch(&branch_name)?;

    utils::print_success(&format!("Created and switched to branch '{branch_name}'"));
    utils::print_info(&format!("Parent branch: {current_branch}"));

    Ok(())
}
