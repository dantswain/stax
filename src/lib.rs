use clap::Subcommand;

pub mod commands;
pub mod config;
pub mod git;
pub mod github;
pub mod oauth;
pub mod stack;
pub mod token_store;
pub mod utils;

#[derive(Subcommand)]
pub enum AuthCommands {
    #[command(about = "Log in to GitHub")]
    Login,
    #[command(about = "Show current authentication status")]
    Status,
}
