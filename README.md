# Stax

A fast CLI tool for managing stacked pull requests on GitHub.

## Overview

Stax helps developers manage complex feature development workflows by organizing related branches into logical stacks. It provides commands to create branches, visualize stack structures, submit pull requests, and keep your stack synchronized with the main branch.

## Features

- 🌳 **Stack Visualization**: See the hierarchical structure of your branches
- 🔄 **Branch Management**: Create new branches with proper parent-child relationships
- 📝 **PR Integration**: Create and manage GitHub pull requests for your stack
- 🔄 **Sync & Restack**: Keep your branches up to date with their parents
- ⚙️ **Configuration**: Flexible configuration management

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
# Initialize stax in your repository
stax init

# Configure GitHub token (required for PR operations)
stax config set github.token YOUR_GITHUB_TOKEN
```

### Basic Commands

```bash
# Create a new branch
stax branch feature-name

# Show stack visualization
stax stack

# Show current status
stax status

# Submit PR for current branch
stax submit

# Submit PRs for entire stack
stax submit --all

# Sync current branch with its parent
stax sync

# Restack all branches
stax restack --all

# Delete a branch and update dependents
stax delete branch-name
```

### Configuration Management

```bash
# Set a configuration value
stax config set key value

# Get a configuration value
stax config get key

# List all configuration
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

The project includes comprehensive unit tests covering:

- **Utility Functions** (`src/utils.rs`):
  - String truncation with various edge cases
  - User confirmation prompts
  - Colored output functions

- **Stack Management** (`src/stack.rs`):
  - Stack creation and validation
  - Branch relationship detection
  - Pull request state management
  - Stack traversal algorithms

- **Integration Tests**:
  - Command-line interface testing with `assert_cmd`
  - File system operations with `tempfile`
  - Predicate testing with `predicates`

## Project Structure

```
src/
├── main.rs              # CLI entry point and command routing
├── commands/            # Command implementations
│   ├── branch.rs        # Branch creation
│   ├── config.rs        # Configuration management
│   ├── delete.rs        # Branch deletion
│   ├── init.rs          # Repository initialization
│   ├── restack.rs       # Branch restacking
│   ├── stack.rs         # Stack visualization
│   ├── status.rs        # Status display
│   ├── submit.rs        # PR submission
│   └── sync.rs          # Branch synchronization
├── config.rs            # Configuration handling
├── git.rs               # Git operations wrapper
├── github.rs            # GitHub API integration
├── stack.rs             # Stack analysis and management
└── utils.rs             # Utility functions
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

### Development Dependencies
- `tempfile` - Temporary files for testing
- `assert_cmd` - Command-line testing
- `predicates` - Test assertions
- `mockall` - Mocking framework

## Contributing

1. Fork the repository
2. Create a feature branch (`stax branch your-feature`)
3. Make your changes
4. Ensure tests pass (`cargo test`)
5. Ensure linting passes (`cargo clippy -- -D warnings`)
6. Format code (`cargo fmt`)
7. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.