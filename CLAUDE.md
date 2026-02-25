# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Stax is a Rust CLI tool for managing stacked pull requests on GitHub. It helps developers organize related branches into logical stacks with proper parent-child relationships and provides commands for creating branches, visualizing stack structures, submitting pull requests, and keeping stacks synchronized.

## Common Development Commands

```bash
# Build and test
cargo build                      # Debug build
cargo test                       # Run all tests
cargo test -- --nocapture        # Run tests with output
cargo test utils                 # Test specific module
cargo test test_truncate_string  # Test specific function

# Code quality (run before committing)
cargo clippy -- -D warnings      # Lint with warnings as errors
cargo fmt --check                # Check formatting
cargo fmt                        # Apply formatting

# Installation
cargo install --path .           # Install locally from source
```

## Architecture Overview

### CLI Structure
- **Entry Point**: `src/main.rs` uses clap v4 with derive macros for command parsing
- **Commands Directory**: `src/commands/` contains individual command implementations, each exposing an async `run()` function
- **Error Handling**: Uses `anyhow::Result` throughout; errors propagate to `main()` which prints them and exits with code 1

### Command Architecture Pattern
All commands follow this pattern:
1. Open git repo with `GitRepo::open(".")`
2. Load `Config` from TOML file
3. Optionally get GitHub token and create `GitHubClient`
4. Perform operations with user feedback via `utils::print_*` colored output
5. Return `Result<()>`

### Core Modules
- **`git.rs`**: Wraps `git2` crate. `GitRepo` struct provides branch operations, push, merge-base detection. SSH auth tries key files directly (agent auth commented out due to loop issue). HTTPS auth reads `GITHUB_TOKEN` env var or git config.
- **`github.rs`**: `GitHubClient` wraps `octocrab`. Parses both HTTPS and SSH remote URLs via `parse_github_url()`. Custom `PullRequest` struct (not octocrab's) used throughout.
- **`stack.rs`**: `Stack::analyze()` builds the branch graph. `detect_relationships()` infers parent-child by checking if a branch's merge-base with another equals that branch's tip (closest such branch wins). Falls back to main/master/develop as parent.
- **`config.rs`**: TOML config at XDG-compliant paths. Key fields: `default_base_branch`, `auto_push`, `draft_prs`, `pr_template`.
- **`oauth.rs`**: GitHub device flow auth (uses GitHub CLI's client ID: `178c6fc778ccc68e1d6a`).
- **`token_store.rs`**: Token stored at `~/.stax/token` with 0o600 permissions.
- **`utils.rs`**: `print_success/info/warning/error` colored output, `confirm()` prompts via `dialoguer`.

### Available Commands
- `stax init` - Interactive setup with OAuth or token authentication
- `stax branch [name]` - Create branch with parent relationship tracking
- `stax stack` - Visual tree display of branch relationships
- `stax submit [--all]` - Create/update PRs; `--all` submits entire stack
- `stax sync [--all]` - Sync with remote
- `stax restack [--all]` - Rebase branches on parents
- `stax delete <branch>` - Delete branch and update dependents
- `stax status` - Show current repository status
- `stax config set/get/list` - Configuration management

## Key Patterns

- All command functions are async (Tokio runtime)
- Interactive UX via `dialoguer` (confirmations, text input, fuzzy-select)
- `GitRepo` does not use `git2::Repository::open()` directly — it uses `discover()` which walks up to find the `.git` directory
- Stack relationship detection is heuristic (merge-base analysis), not stored metadata
- Main/master/develop are treated as root branches and excluded from PR operations
