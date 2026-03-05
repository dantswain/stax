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

## Testing Policy

**Tests are critical to this project. Follow these rules strictly:**

1. **Always run `cargo test` after any code change.** All 190 tests must pass before considering work complete.
2. **Never modify existing test assertions to make them pass.** If a test fails after your change, the change is wrong — fix the implementation, not the test. Tests encode intended behavior.
3. **Only change a test when the behavioral change is explicitly requested** by the user and you understand why the old behavior is no longer correct. When you do change a test, call it out explicitly so the user can verify.
4. **Add tests for new functionality.** New commands, new helper functions, and bug fixes should have corresponding test coverage.
5. **Integration tests (`tests/`) test real git operations** using temporary repos with bare remote origins. They validate end-to-end behavior and are especially important for rebase, cache, and navigation logic.
6. **Run the full quality check before finishing:** `cargo fmt && cargo clippy -- -D warnings && cargo test`

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
- **`cache.rs`**: Local metadata cache at `.git/stax/cache.json`. `StackCache` manages branch parent relationships, commit hashes, merged-branch tracking, and cached PR data. Validates on load (schema version, trunk tip, branch staleness). `RestackState` persists old branch tips to `.git/stax/restack-state.json` for `--continue` conflict recovery across restack, repair, and sync.
- **`git.rs`**: Wraps `git2` crate. `GitRepo` struct provides branch operations, push (with force-push-with-lease), merge-base detection, `rebase_onto_with_base` (uses `--onto` to avoid replaying parent commits, accepts `continue_command` hint for context-appropriate error messages), `rebase_continue`, `is_rebase_in_progress`, and `has_diverged_from_remote`. Caches OID and merge-base results. SSH auth tries key files directly (agent auth commented out due to loop issue).
- **`github.rs`**: `GitHubClient` wraps `octocrab`. Parses both HTTPS and SSH remote URLs via `parse_github_url()`. Custom `PullRequest` struct (not octocrab's) used throughout. `format_github_error()` extracts useful messages from octocrab's opaque error types. Includes PR comment methods for stack visualization.
- **`stack.rs`**: `Stack::from_parent_map()` builds the branch graph from a pre-computed parent map (from cache/navigate). `Stack::analyze()` is the legacy full-compute path. Pre-computes commit hashes and merged-branch status to avoid redundant git calls. Falls back to main/master/develop as parent.
- **`config.rs`**: TOML config at XDG-compliant paths. Key fields: `default_base_branch`, `auto_push`, `draft_prs`, `pr_template`, `log_level`.
- **`logging.rs`**: File-based logging via `fern` + `log` facade. `get_log_dir()` returns platform-specific path (macOS: `~/Library/Logs/stax/`, Linux: `~/.local/state/stax/logs/`). `init()` sets up fern dispatch with formatted output, dependency noise suppression, and simple 5MB rotation. Log level resolved in main.rs: `--verbose` flag > `STAX_LOG` env var > config `log_level` > default `"error"`. Non-fatal init — CLI continues even if logging setup fails.
- **`oauth.rs`**: GitHub device flow auth (uses GitHub CLI's client ID: `178c6fc778ccc68e1d6a`).
- **`token_store.rs`**: Token stored at `~/.stax/token` with 0o600 permissions.
- **`utils.rs`**: `print_success/info/warning/error` colored output, `confirm()` prompts, `input_or_editor()` (Ctrl+G to open `$EDITOR`), `open_editor()` (invokes editor via shell for proper env var expansion).

### Available Commands
- `stax auth [login|status]` - GitHub authentication (OAuth or token)
- `stax create [name]` - Create branch with parent relationship tracking
- `stax stack` - Visual tree display of branch relationships
- `stax submit [--all]` - Create/update PRs; `--all` submits entire stack; auto-pushes and force-pushes after rebase
- `stax sync [--no-restack] [--force] [--continue] [--metadata-only]` - Fetch, fast-forward trunk, clean up merged branches, restack
- `stax restack [--all] [--continue]` - Rebase branches on parents
- `stax repair [--check] [--continue]` - Fix branch topology using PR data as source of truth
- `stax up` / `stax down` / `stax top` / `stax bottom` - Navigate the stack
- `stax status` - Show current repository status
- `stax config set/get/list` - Configuration management
- `stax log [-f] [-n N]` - Show or tail the log file; `-f` follows, `-n` sets line count

## Key Patterns

- All command functions are async (Tokio runtime)
- Interactive UX via `dialoguer` (confirmations, text input, fuzzy-select) and `console` (Ctrl+G keybinding)
- `GitRepo` does not use `git2::Repository::open()` directly — it uses `discover()` which walks up to find the `.git` directory
- Parent detection uses a two-layer approach: merge-base heuristic (in `navigate.rs::find_parent`) with PR `base_ref` overrides (in `navigate.rs::apply_pr_overrides`). Results are cached in `.git/stax/cache.json` with incremental validation.
- `get_branches_and_parent_map()` in `navigate.rs` is the main entry point for branch analysis — handles cache load/validate/recompute/save and returns `(branches, commits, merged_set, parent_map)`
- Rebase uses `git rebase --onto` with pre-rebase parent tips to avoid replaying parent commits
- On rebase conflict, the rebase is left in progress for the user to resolve; `--continue` resumes. `RestackState` persists old branch tips so the correct `--onto` base is used after conflict resolution.
- Main/master/develop are treated as root branches and excluded from PR operations
- Navigate commands (`up`/`down`/`top`/`bottom`) do targeted O(n) lookups instead of full O(n²) stack analysis
- `stax repair` uses PR data as the source of truth for parent relationships. It infers parents for "gap branches" (branches without PRs that are used as PR bases) using the chain_tails algorithm. Only flags true topology errors, not branches that merely need restacking.
- Debug logging uses `log::debug!` throughout; all major operations (git, GitHub API, stack analysis, commands) emit debug-level messages suitable for LLM-assisted troubleshooting
