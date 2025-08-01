# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Stax is a Rust CLI tool for managing stacked pull requests on GitHub. It helps developers organize related branches into logical stacks with proper parent-child relationships and provides commands for creating branches, visualizing stack structures, submitting pull requests, and keeping stacks synchronized.

## Common Development Commands

```bash
# Build and test
cargo build                      # Debug build
cargo build --release          # Optimized release build
cargo test                      # Run all tests
cargo test -- --nocapture      # Run tests with output
cargo test utils                # Test specific module
cargo test test_truncate_string # Test specific function

# Code quality (run before committing)
cargo clippy -- -D warnings    # Lint with warnings as errors
cargo fmt --check              # Check formatting
cargo fmt                       # Apply formatting

# Installation
cargo install --path .         # Install locally from source
```

## Architecture Overview

### CLI Structure
- **Entry Point**: `src/main.rs` uses clap v4 with derive macros for command parsing
- **Command Routing**: All commands are async functions returning `Result<()>`
- **Commands Directory**: `src/commands/` contains individual command implementations
- **Error Handling**: Uses `anyhow::Result` throughout with `?` operator for propagation

### Core Modules
- **`git.rs`**: Git operations wrapper using `git2` crate for repository management
- **`github.rs`**: GitHub API client using `octocrab` for PR operations  
- **`oauth.rs`**: GitHub OAuth device flow implementation (uses GitHub CLI's client ID)
- **`config.rs`**: TOML-based configuration with XDG directory compliance
- **`stack.rs`**: Core stack analysis and branch relationship detection
- **`token_store.rs`**: Secure token storage with Unix file permissions (0o600)
- **`utils.rs`**: Colored terminal output utilities and interactive prompts

### Command Architecture Pattern
All commands follow this pattern:
1. Load Git repository with `GitRepo::open(".")`
2. Load configuration from TOML file
3. Optionally create GitHub client if token available
4. Perform operations with user feedback via colored output
5. Return Result for main.rs error handling

### Available Commands
- `stax init` - Interactive setup with OAuth or token authentication
- `stax branch [name]` - Create branch with parent relationship (prompts if no name)
- `stax stack` - Visual tree display of branch relationships
- `stax submit [--all]` - PR creation (stub implementation - needs development)
- `stax sync [--all]` - Remote synchronization
- `stax restack [--all]` - Rebase branches on parents
- `stax delete <branch>` - Delete branch and update dependents
- `stax status` - Show current repository status
- `stax config set/get/list` - Configuration management

## Key Dependencies

### Runtime
- **`clap`** (4.4): CLI parsing with derive macros and color support
- **`git2`** (0.18): Git operations with vendored OpenSSL
- **`octocrab`** (0.34): GitHub API client
- **`tokio`** (1.0): Async runtime with full features
- **`serde`** + **`toml`**: Configuration serialization
- **`anyhow`**: Error handling and chaining
- **`colored`**: Terminal output styling
- **`dialoguer`**: Interactive prompts with fuzzy selection
- **`reqwest`**: HTTP client for OAuth device flow
- **`webbrowser`**: Open browser for authentication

### Development
- **`tempfile`**: Temporary files for testing
- **`assert_cmd`**: CLI testing framework
- **`predicates`**: Test assertions
- **`mockall`**: Mocking framework

## Configuration & Authentication

### File Locations (XDG compliant)
- **Config**: `~/.config/stax/config.toml` (Linux/macOS), `%APPDATA%\stax\config.toml` (Windows)
- **Token**: `~/.stax/token` with 0o600 permissions (Linux/macOS), `%USERPROFILE%\.stax\token` (Windows)

### OAuth Implementation
- Uses GitHub's device flow (same client ID as GitHub CLI: `178c6fc778ccc68e1d6a`)
- No local server required - works in any environment
- Secure browser-based authorization flow
- Automatic token retrieval and storage

### Setup Requirements
- Must be run in a Git repository
- GitHub remote is optional but recommended
- Supports both HTTPS and SSH remote formats

## Testing Structure

Comprehensive test coverage across modules:
- **Utils**: String truncation, prompts, colored output edge cases
- **Git Operations**: Repository operations, branch management
- **GitHub API**: URL parsing, PR serialization
- **Configuration**: TOML generation, validation, path resolution
- **Stack Analysis**: Branch relationships, traversal algorithms
- **OAuth**: Request structure validation

## Development Notes

### Code Patterns
- All command functions are async using Tokio runtime
- Extensive use of `?` operator for error propagation
- Interactive UX prioritized with confirmation prompts using `dialoguer`
- Security-conscious token storage with proper Unix permissions
- Configuration is both programmatic and human-editable

### Current Limitations
- **Submit command is stubbed** - main missing feature for PR creation
- Token refresh logic not implemented for OAuth tokens
- Cross-platform token security needs improvement on Windows

### Build Configuration
Release profile optimized for CLI distribution:
- Link-time optimization enabled
- Single codegen unit
- Panic abort for smaller binaries
- Debug symbols stripped