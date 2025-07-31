use clap::{Parser, Subcommand};
use std::process;

mod commands;
mod config;
mod git;
mod github;
mod oauth;
mod stack;
mod token_store;
mod utils;

use commands::*;

#[derive(Parser)]
#[command(name = "stax")]
#[command(about = "A fast CLI tool for managing stacked pull requests")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Setup configuration")]
    Init,
    #[command(about = "Create new branch")]
    Branch { name: Option<String> },
    #[command(about = "Show visual stack structure")]
    Stack,
    #[command(about = "Create/update PRs")]
    Submit {
        #[arg(long, help = "Submit all branches in stack")]
        all: bool,
    },
    #[command(about = "Sync with remote")]
    Sync {
        #[arg(long, help = "Sync all branches in stack")]
        all: bool,
    },
    #[command(about = "Rebase branches on parents")]
    Restack {
        #[arg(long, help = "Restack all branches")]
        all: bool,
    },
    #[command(about = "Delete branch, update dependents")]
    Delete { branch: String },
    #[command(about = "Show current status")]
    Status,
    #[command(about = "Manage configuration")]
    #[command(subcommand)]
    Config(ConfigCommands),
}

#[derive(Subcommand)]
enum ConfigCommands {
    #[command(about = "Set configuration value")]
    Set { key: String, value: String },
    #[command(about = "Get configuration value")]
    Get { key: String },
    #[command(about = "List all configuration")]
    List,
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => init::run().await,
        Commands::Branch { name } => branch::run(name.as_deref()).await,
        Commands::Stack => commands::stack::run().await,
        Commands::Submit { all } => submit::run(all).await,
        Commands::Sync { all } => sync::run(all).await,
        Commands::Restack { all } => restack::run(all).await,
        Commands::Delete { branch } => delete::run(&branch).await,
        Commands::Status => status::run().await,
        Commands::Config(config_cmd) => match config_cmd {
            ConfigCommands::Set { key, value } => commands::config::set(&key, &value).await,
            ConfigCommands::Get { key } => commands::config::get(&key).await,
            ConfigCommands::List => commands::config::list().await,
        },
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}