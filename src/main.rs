use clap::{Parser, Subcommand};
use std::process;

use stax::commands;
use stax::commands::*;
use stax::AuthCommands;

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
    #[command(about = "Authenticate with GitHub")]
    Auth {
        #[command(subcommand)]
        command: Option<AuthCommands>,
    },
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
        #[arg(long, help = "Skip restacking branches")]
        no_restack: bool,
        #[arg(short, long, help = "Skip confirmation prompts")]
        force: bool,
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
        Commands::Auth { command } => auth::run(command).await,
        Commands::Branch { name } => branch::run(name.as_deref()).await,
        Commands::Stack => commands::stack::run().await,
        Commands::Submit { all } => submit::run(all).await,
        Commands::Sync { no_restack, force } => sync::run(no_restack, force).await,
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
