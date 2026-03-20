# Stax

[![CI](https://github.com/dantswain/stax/actions/workflows/ci.yml/badge.svg)](https://github.com/dantswain/stax/actions/workflows/ci.yml)

A fast CLI tool for managing stacked pull requests on GitHub.

## Overview

Stax helps developers manage complex feature development workflows by organizing related branches into logical stacks. It provides commands to create branches, visualize stack structures, submit pull requests, and keep your stack synchronized with the main branch.

Stax is inspired by [Graphite's CLI](https://graphite.com/docs/cli-overview) but does NOT aim to provide hosted review tooling.  Stax is only intended to help the workflow of managing stacked branches and pull requests.

## Features

- 🌳 **Stack Visualization**: See the hierarchical structure of your branches
- 🔄 **Branch Management**: Create new branches with proper parent-child relationships
- 📝 **PR Integration**: Create and manage GitHub pull requests for your stack
- 🔄 **Sync & Restack**: Keep your branches up to date with their parents
- 💎 **Diamond Merges**: Include multiple independent branches as dependencies for a single branch
- 🔧 **Topology Repair**: Automatically detect and fix broken branch relationships using PR data
- ⚙️ **Configuration**: Flexible configuration management

## Example Usage

```bash
# Start on main, create a feature branch
$ stax create add-auth
✓ Created and switched to branch 'add-auth'
ℹ Parent branch: main

# Do some work, commit, then submit a PR
$ git add -A && git commit -m "Add authentication module"
$ stax submit
ℹ Pushing branch 'add-auth' to remote...
✔ PR title · Add Auth
? PR description (Ctrl+G for editor):
ℹ Creating PR: 'Add Auth' (add-auth → main)
✓ PR created: https://github.com/you/repo/pull/1

# Stack another branch on top
$ stax create add-login-page
✓ Created and switched to branch 'add-login-page'
ℹ Parent branch: add-auth

$ git add -A && git commit -m "Add login page"
$ stax submit
✓ PR created: https://github.com/you/repo/pull/2

# And one more
$ stax create add-logout
✓ Created and switched to branch 'add-logout'
ℹ Parent branch: add-login-page

$ git add -A && git commit -m "Add logout button"
$ stax submit
✓ PR created: https://github.com/you/repo/pull/3

# See the full stack
$ stax stack
Stack Visualization

main ○
  ├─ add-auth ● PR #1
    ├─ add-login-page ● PR #2
      ├─ add-logout ← current ● PR #3

# Navigate around
$ stax bottom
✓ Moved to bottom of stack: add-auth

$ stax top
✓ Moved to top of stack: add-logout

$ stax down
✓ Moved down to add-login-page

# PR #1 gets merged on GitHub, sync everything
$ stax sync
ℹ Fetching from origin...
✓ Fetched latest changes
✓ Fast-forwarded 'main'
ℹ Branches with merged/closed PRs:
ℹ   add-auth (PR #1 merged)
? Delete these branches locally and from remote? (y/N): y
✓ Deleted 'add-auth'
ℹ Rebasing 'add-login-page' onto 'main'
✓ Restacked 'add-login-page'
✓ Restacked 'add-logout'
✓ Sync complete

# Push updated branches and update PR metadata on GitHub
$ stax submit --all
ℹ Branch 'add-login-page' has diverged from remote, force-pushing...
✓ Force-pushed 'add-login-page'
ℹ Branch 'add-logout' has diverged from remote, force-pushing...
✓ Force-pushed 'add-logout'
✓ Updated PR: https://github.com/you/repo/pull/2
✓ Updated PR: https://github.com/you/repo/pull/3
```

## Installation

### Prerequisites

- Rust 1.70+ (install via [rustup](https://rustup.rs/))
- Git
- GitHub account and personal access token

### Building from Source

```bash
# Clone the repository
git clone https://github.com/yourusername/stax.git
cd stax

# Build the project
cargo build --release

# The binary will be available at target/release/stax
```

### Install via Cargo

```bash
cargo install --path .
```

## Usage

### Initial Setup

```bash
# Navigate to your Git repository
cd your-project

# Authenticate with GitHub
stax auth login
```

**Prerequisites:**
- Must be run in a Git repository (`git init` if needed)
- GitHub remote is optional but recommended (`git remote add origin https://github.com/username/repo.git`)

Stax will guide you through authentication setup with two options:

1. **Browser Authentication (Recommended)**: Authenticate through your browser using OAuth
2. **Personal Access Token**: Manually enter a GitHub personal access token

#### OAuth Authentication Details

Stax uses GitHub's device flow for authentication, which is designed specifically for CLI applications:

1. **No local server required** - Works from any environment including remote servers
2. **Secure** - Uses the same OAuth client ID as GitHub CLI
3. **User-friendly** - Simple web-based authorization flow

The authentication process:
1. Stax requests a device code from GitHub
2. User visits GitHub's device activation page
3. User enters the provided code to authorize the application
4. Stax receives the access token automatically

No additional setup is required - the OAuth flow works out of the box!

### File Locations

**Configuration file:**
- **`~/.config/stax/config.toml`** (on macOS/Linux)  
- **`%APPDATA%\stax\config.toml`** (on Windows)

**Authentication token:**
- **`~/.stax/token`** (on macOS/Linux)
- **`%USERPROFILE%\.stax\token`** (on Windows)

**Example config.toml:**
```toml
# Stax Configuration File
# This file is automatically generated but can be manually edited

# Default base branch for new stacks
default_base_branch = "main"

# Automatically push branches when creating PRs
auto_push = true

# Create draft PRs by default
draft_prs = false

# Default PR template (optional)
# pr_template = """
# ## Summary
# 
# ## Testing
# """
```

### Basic Commands

```bash
# Create a new branch
stax create feature-name

# Show stack visualization
stax stack

# Show current status
stax status

# Submit PR for current branch (auto-pushes, force-pushes after rebase)
stax submit

# Submit PRs for entire stack
stax submit --all

# Sync current branch with its parent
stax sync

# Continue sync after resolving rebase conflicts
stax sync --continue

# Restack all branches
stax restack --all

# Continue restack after resolving rebase conflicts
stax restack --continue

# Insert a new branch into the stack (between current and its children)
stax insert above new-branch

# Insert a new branch into the stack (between current and its parent)
stax insert below new-branch

# Reparent an existing branch above/below current
stax insert above existing-branch
stax insert below existing-branch

# Include another branch as a merge dependency (diamond merge)
stax include other-branch

# Continue after resolving shadow merge conflicts
stax include --continue

# Check branch topology against PR data (dry run)
stax repair --check

# Fix broken branch topology automatically
stax repair

# Continue repair after resolving rebase conflicts
stax repair --continue
```

### Stack Navigation

Move between branches in a stack without typing branch names:

```bash
stax up       # Move to child branch (away from main)
stax down     # Move to parent branch (toward main)
stax top      # Jump to the leaf (topmost branch)
stax bottom   # Jump to the first branch above main
```

When a branch has multiple children, you'll be prompted to pick one.

### Diamond Merges

Sometimes a branch needs changes from two independent branches that haven't been merged to main yet. For example, your `add-payments` branch needs both `add-auth` and `add-database`, which are separate PRs in flight. Stax handles this with the `include` command:

```bash
# You're on add-payments, which is stacked on add-auth
$ stax down
✓ Moved down to add-auth

$ stax up
✓ Moved up to add-payments

# Include add-database as an additional dependency
$ stax include add-database
ℹ Creating shadow branch 'stax/shadow/add-payments' from ["add-auth", "add-database"]
ℹ Rebasing 'add-payments' onto shadow branch...
✓ Included 'add-database' into 'add-payments' via shadow branch
ℹ Sources: add-auth, add-database

# The stack shows the merge relationship
$ stax stack
Stack Visualization

main ○
  ├─ add-auth ● PR #1
    ├─ add-payments [+add-database] ← current ○
  ├─ add-database ● PR #2
    ├─ add-payments [+add-auth]
```

**How it works:**

Stax creates a hidden **shadow branch** (`stax/shadow/<your-branch>`) that merges all source branches together, then rebases your branch on top of it. Shadow branches are:

- **Invisible in navigation** — `stax up/down/top/bottom` skip them
- **Hidden in stack visualization** — you see `[+other_source]` annotations instead
- **Automatically recreated during restack** — when sources are updated, the shadow is rebuilt
- **Pushed during submit** — the shadow branch is pushed to the remote as the PR base
- **Dissolved during sync** — when all sources are merged to main, the shadow is deleted and your branch is reparented

You can include multiple branches:

```bash
$ stax include add-logging    # Now depends on add-auth, add-database, and add-logging
```

**Handling merge conflicts:**

If the source branches conflict with each other, `stax include` will leave the merge in progress for you to resolve:

```bash
$ stax include add-conflicting
ℹ Creating shadow branch 'stax/shadow/add-payments' from ["add-auth", "add-conflicting"]
Error: Merge conflict while building shadow branch 'stax/shadow/add-payments'.
Source 'add-conflicting' conflicts with prior sources.

Resolve the conflicts, stage the files, then run:
  stax include --continue

To abort instead:
  git merge --abort && git checkout add-payments
```

After resolving the conflicts and staging the files, run `stax include --continue` to finish building the shadow branch. The same `--continue` flow works for shadow merge conflicts during `stax restack` and `stax sync`.

**When a source branch is merged:**

Running `stax sync` automatically handles dissolution:
- If all sources are merged to main, the shadow is deleted and the consumer is reparented to main
- If some sources remain, the shadow is recreated with only the remaining sources
- If only one source remains, the shadow is dissolved entirely and the consumer becomes a normal stacked branch

### Configuration Management

```bash
# Set a configuration value
stax config set key value

# Get a configuration value
stax config get key

# List all configuration (shows config file location)
stax config list
```

## Development

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Build and install locally
cargo install --path .
```

### Testing

Run the full test suite:

```bash
cargo test
```

Run tests with output:

```bash
cargo test -- --nocapture
```

Run specific tests:

```bash
# Test a specific module
cargo test utils

# Test a specific function
cargo test test_truncate_string
```

### Linting

This project uses Clippy for linting with strict warning settings:

```bash
# Run clippy with default settings
cargo clippy

# Run clippy with warnings treated as errors (recommended)
cargo clippy -- -D warnings

# Fix auto-fixable issues
cargo clippy --fix
```

### Code Formatting

```bash
# Check formatting
cargo fmt --check

# Apply formatting
cargo fmt
```

### Development Workflow

1. **Before making changes:**
   ```bash
   cargo test
   cargo clippy -- -D warnings
   cargo fmt --check
   ```

2. **After making changes:**
   ```bash
   cargo test
   cargo clippy -- -D warnings
   cargo fmt
   ```

3. **Before committing:**
   ```bash
   cargo build --release
   cargo test
   cargo clippy -- -D warnings
   ```

## Testing Coverage

The project includes 254 tests across unit and integration suites:

- **Unit Tests** (92 tests, in-module `#[cfg(test)]`):
  - `cache.rs` — Cache roundtrip, schema validation, restack state persistence, branch upsert, PR data, shadow branch helpers
  - `config.rs` — Config defaults, set/get, TOML generation, path resolution, log level
  - `github.rs` — URL parsing (SSH/HTTPS), PR struct serialization
  - `git.rs` — Repo open, is_clean behavior, URL methods
  - `logging.rs` — Log directory, level parsing
  - `stack.rs` — Stack creation, branch traversal, PR state validation
  - `commands/submit.rs` — Stack comment formatting, PR status icons, branching topologies
  - `utils.rs` — String truncation edge cases
  - `oauth.rs` — Client creation, request structure validation

- **Integration Tests** (162 tests, `tests/`):
  - `git_test.rs` — Branch create/checkout, merge-base, is_clean, tracking, remote operations (29 tests using temp repos with bare remote origins)
  - `navigate_test.rs` — Parent detection, cache warm/cold/partial hits, PR overrides, topological walking, merged branch handling, fork detection (61 tests)
  - `include_test.rs` — Shadow branch creation/replacement/conflict resolution/continue, is_shadow_branch, parent/children exclusion, children_from_map resolution, cache roundtrip, build_parent_map skipping (21 tests)
  - `insert_test.rs` — Insert above/below: create new branches, reparent existing branches, PR base_ref updates, diamond child source updates, self-insert guard, parent confirmation (19 tests)
  - `stack_test.rs` — `Stack::analyze` and `Stack::from_parent_map` with various topologies: linear, branching, diamond, PR overrides (14 async tests)
  - `repair_test.rs` — Topology mismatch detection, gap branch inference, topological sorting (12 tests)
  - `restack_test.rs` — `rebase_onto` simple/noop/conflict/full-stack/branch-preservation (5 tests)
  - `token_store_test.rs` — Token store/retrieve, overwrite, whitespace trimming, unix file permissions (1 test)

## Project Structure

```
src/
├── main.rs              # CLI entry point and command routing
├── lib.rs               # Library crate root (public module exports)
├── commands/            # Command implementations
│   ├── auth.rs          # GitHub authentication (login/status)
│   ├── branch.rs        # Branch creation
│   ├── config.rs        # Configuration management
│   ├── include.rs       # Diamond merge (stax include)
│   ├── insert.rs        # Insert branch into stack (stax insert above/below)
│   ├── navigate.rs      # Stack navigation + parent detection + cache integration
│   ├── repair.rs        # Topology repair using PR data as source of truth
│   ├── restack.rs       # Branch restacking
│   ├── stack.rs         # Stack visualization
│   ├── status.rs        # Status display
│   ├── submit.rs        # PR submission
│   └── sync.rs          # Branch synchronization
├── cache.rs             # Local metadata cache (.git/stax/cache.json)
├── config.rs            # Configuration handling
├── git.rs               # Git operations wrapper
├── github.rs            # GitHub API integration
├── logging.rs           # File-based debug logging
├── oauth.rs             # GitHub device flow OAuth
├── stack.rs             # Stack analysis and management
├── token_store.rs       # Secure token storage (~/.stax/token)
└── utils.rs             # Utility functions
tests/
├── common/mod.rs        # Shared test helpers (temp repo creation)
├── git_test.rs          # Git operations integration tests
├── include_test.rs      # Diamond merge / shadow branch integration tests
├── insert_test.rs       # Insert branch into stack integration tests
├── navigate_test.rs     # Navigation, cache, and parent detection tests
├── repair_test.rs       # Topology repair integration tests
├── restack_test.rs      # Rebase integration tests
├── stack_test.rs        # Stack analysis integration tests
└── token_store_test.rs  # Token storage integration tests
```

## Dependencies

### Runtime Dependencies
- `clap` - Command-line argument parsing
- `git2` - Git operations
- `octocrab` - GitHub API client
- `tokio` - Async runtime
- `serde` - Serialization
- `anyhow` - Error handling
- `colored` - Terminal colors
- `dialoguer` - Interactive prompts
- `reqwest` - HTTP client for OAuth device flow
- `webbrowser` - Open browser for authentication
- `dirs` - Directory paths for secure token storage
- `console` - Terminal key input (Ctrl+G editor shortcut)
- `tempfile` - Temporary files for editor integration

### Development Dependencies
- `assert_cmd` - Command-line testing
- `predicates` - Test assertions
- `mockall` - Mocking framework

## Contributing

1. Fork the repository
2. Create a feature branch (`stax create your-feature`)
3. Make your changes
4. Ensure tests pass (`cargo test`)
5. Ensure linting passes (`cargo clippy -- -D warnings`)
6. Format code (`cargo fmt`)
7. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.
