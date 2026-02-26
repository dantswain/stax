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
        #[arg(long, help = "Continue after resolving rebase conflicts")]
        r#continue: bool,
    },
    #[command(about = "Rebase branches on parents")]
    Restack {
        #[arg(long, help = "Restack all branches")]
        all: bool,
        #[arg(long, help = "Continue after resolving rebase conflicts")]
        r#continue: bool,
    },
    #[command(about = "Move up the stack (away from main)")]
    Up,
    #[command(about = "Move down the stack (toward main)")]
    Down,
    #[command(about = "Move to the top of the stack")]
    Top,
    #[command(about = "Move to the bottom of the stack")]
    Bottom,
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
        Commands::Sync {
            no_restack,
            force,
            r#continue,
        } => sync::run(no_restack, force, r#continue).await,
        Commands::Restack { all, r#continue } => restack::run(all, r#continue).await,
        Commands::Up => navigate::up().await,
        Commands::Down => navigate::down().await,
        Commands::Top => navigate::top().await,
        Commands::Bottom => navigate::bottom().await,
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
