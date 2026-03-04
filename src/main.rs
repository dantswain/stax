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
    #[arg(short, long, global = true, help = "Enable debug logging")]
    verbose: bool,

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
    Create { name: Option<String> },
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
    #[command(about = "Show or tail the log file")]
    Log {
        #[arg(short = 'f', long, help = "Follow log output")]
        follow: bool,
        #[arg(
            short = 'n',
            long,
            default_value = "50",
            help = "Number of lines to show"
        )]
        lines: usize,
    },
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
    let cli = Cli::parse();

    // Resolve log level: --verbose > STAX_LOG env var > config file > "error"
    let log_level = if cli.verbose {
        ::log::LevelFilter::Debug
    } else if let Ok(env_level) = std::env::var("STAX_LOG") {
        stax::logging::parse_level(&env_level)
    } else {
        let level_str = stax::config::Config::load()
            .map(|c| c.log_level)
            .unwrap_or_else(|_| "error".to_string());
        stax::logging::parse_level(&level_str)
    };

    // Initialize logging (non-fatal: warn to stderr if it fails)
    if let Err(e) = stax::logging::init(log_level) {
        eprintln!("Warning: could not initialize logging: {e}");
    }

    ::log::debug!(
        "stax {} starting, command: {:?}",
        env!("CARGO_PKG_VERSION"),
        std::env::args().collect::<Vec<_>>()
    );
    ::log::debug!("working directory: {:?}", std::env::current_dir().ok());

    let result = match cli.command {
        Commands::Auth { command } => auth::run(command).await,
        Commands::Create { name } => branch::run(name.as_deref()).await,
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
        Commands::Log { follow, lines } => commands::log::run(follow, lines).await,
    };

    if let Err(e) = result {
        ::log::error!("Command failed: {e:#}");
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
