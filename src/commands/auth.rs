use crate::git::GitRepo;
use crate::github::GitHubClient;
use crate::token_store::TokenStore;
use crate::utils;
use crate::AuthCommands;
use anyhow::{anyhow, Result};
use dialoguer::{Input, Select};

pub async fn run(command: Option<AuthCommands>) -> Result<()> {
    match command {
        Some(AuthCommands::Login) => login().await,
        Some(AuthCommands::Status) => status().await,
        None => {
            // "Do the right thing": if no token, login; if token exists, show status
            if crate::token_store::get_token().is_some() {
                status().await
            } else {
                login().await
            }
        }
    }
}

async fn login() -> Result<()> {
    log::debug!("auth: starting login flow");
    utils::print_info("Authenticating with GitHub...");

    let auth_methods = vec![
        "Authenticate with browser (recommended)",
        "Enter personal access token manually",
    ];

    let auth_choice = Select::new()
        .with_prompt("How would you like to authenticate with GitHub?")
        .items(&auth_methods)
        .default(0)
        .interact()?;

    let github_token = match auth_choice {
        0 => {
            log::debug!("auth: user chose browser authentication");
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
        }
        1 => {
            log::debug!("auth: user chose manual token entry");
            let token = Input::<String>::new()
                .with_prompt("GitHub personal access token")
                .interact_text()?;

            if token.trim().is_empty() {
                return Err(anyhow!("GitHub token is required"));
            }

            token
        }
        _ => return Err(anyhow!("Invalid authentication method selected")),
    };

    TokenStore::store_token(&github_token)?;
    utils::print_success("GitHub token stored securely");

    // Verify the token works if we have a repo context
    if let Ok(git) = GitRepo::open(".") {
        if let Some(remote_url) = git.get_remote_url("origin") {
            if let Ok(client) = GitHubClient::new(&github_token, &remote_url) {
                match client.get_authenticated_user().await {
                    Ok(username) => {
                        utils::print_success(&format!("Authenticated as {username}"));
                    }
                    Err(_) => {
                        utils::print_warning(
                            "Token stored but could not verify it. You may need to re-authenticate.",
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

async fn status() -> Result<()> {
    let token = match crate::token_store::get_token() {
        Some(t) => t,
        None => {
            utils::print_warning("Not authenticated. Run 'stax auth login' to log in.");
            return Ok(());
        }
    };

    // Try to verify the token against a repo
    if let Ok(git) = GitRepo::open(".") {
        if let Some(remote_url) = git.get_remote_url("origin") {
            match GitHubClient::new(&token, &remote_url) {
                Ok(client) => match client.get_authenticated_user().await {
                    Ok(username) => {
                        utils::print_success(&format!("Authenticated as {username}"));
                        return Ok(());
                    }
                    Err(_) => {
                        utils::print_error(
                            "Token is invalid or expired. Run 'stax auth login' to re-authenticate.",
                        );
                        return Ok(());
                    }
                },
                Err(e) => {
                    utils::print_warning(&format!("Could not create GitHub client: {e}"));
                    utils::print_info(
                        "Token is stored but could not be verified without a valid GitHub remote.",
                    );
                    return Ok(());
                }
            }
        }
    }

    utils::print_info("Token is stored but could not be verified (no git repo or remote found).");
    Ok(())
}
