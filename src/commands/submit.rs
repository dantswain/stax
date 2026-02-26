use crate::{config::Config, git::GitRepo, github::GitHubClient, stack::Stack, token_store, utils};
use anyhow::{anyhow, Result};
use dialoguer::{theme::ColorfulTheme, Confirm, Input};

const STACK_COMMENT_MARKER: &str = "<!-- stax-stack-comment -->";

pub async fn run(all: bool) -> Result<()> {
    let git = GitRepo::open(".")?;
    let config = Config::load()?;

    log::debug!("Opening git repository");

    // Check if we have a GitHub token
    let token = token_store::get_token()
        .ok_or_else(|| anyhow!("Not authenticated. Run 'stax auth' to log in."))?;

    // Get the remote URL for GitHub client
    let remote_url = git
        .get_remote_url("origin")
        .ok_or_else(|| anyhow!("No 'origin' remote found. Add a GitHub remote first."))?;

    log::debug!("Github remote URL: {remote_url}");

    let github = GitHubClient::new(&token, &remote_url)?;

    log::debug!("Analyzing stack structure");

    let stack = Stack::analyze(&git, Some(&github)).await?;

    if all {
        log::debug!("Submitting all branches in stack...");
        submit_stack(&git, &github, &stack, &config).await
    } else {
        log::debug!("Submitting current branch...");
        submit_current_branch(&git, &github, &stack, &config).await
    }
}

async fn submit_current_branch(
    git: &GitRepo,
    github: &GitHubClient,
    stack: &Stack,
    config: &Config,
) -> Result<()> {
    let current_branch = &stack.current_branch;

    // Don't create PRs for main branches
    if ["main", "master", "develop"].contains(&current_branch.as_str()) {
        return Err(anyhow!(
            "Cannot create PR for main branch '{current_branch}'"
        ));
    }

    let current_stack_branch = stack
        .branches
        .get(current_branch)
        .ok_or_else(|| anyhow!("Current branch not found in stack"))?;

    // Check if PR already exists
    if let Some(existing_pr) = &current_stack_branch.pull_request {
        utils::print_info(&format!("PR already exists: {}", existing_pr.html_url));

        // Ask if they want to update it
        let update = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Update existing PR?")
            .default(false)
            .interact()?;

        if update {
            update_existing_pr(github, existing_pr.number, config).await?;
        }

        // Update stack comments on all PRs
        let fresh_stack = Stack::analyze(git, Some(github)).await?;
        update_stack_comments(github, &fresh_stack).await?;

        return Ok(());
    }

    create_new_pr(git, github, current_branch, stack, config).await?;

    // Re-analyze to pick up newly created PR, then update stack comments
    let fresh_stack = Stack::analyze(git, Some(github)).await?;
    update_stack_comments(github, &fresh_stack).await?;

    Ok(())
}

async fn submit_stack(
    git: &GitRepo,
    github: &GitHubClient,
    stack: &Stack,
    config: &Config,
) -> Result<()> {
    let current_branch = &stack.current_branch;
    let stack_branches = stack.get_stack_for_branch(current_branch);

    // Filter out main branches and branches that already have PRs
    let branches_to_submit: Vec<_> = stack_branches
        .iter()
        .filter(|b| {
            !["main", "master", "develop"].contains(&b.name.as_str()) && b.pull_request.is_none()
        })
        .collect();

    if branches_to_submit.is_empty() {
        utils::print_info("All branches in stack already have PRs");
        return Ok(());
    }

    utils::print_info(&format!(
        "Creating PRs for {} branches...",
        branches_to_submit.len()
    ));

    for branch in branches_to_submit {
        utils::print_info(&format!("Creating PR for branch: {}", branch.name));
        create_new_pr(git, github, &branch.name, stack, config).await?;
    }

    utils::print_success("Stack submission completed!");

    // Re-analyze to pick up newly created PRs, then update stack comments
    let fresh_stack = Stack::analyze(git, Some(github)).await?;
    update_stack_comments(github, &fresh_stack).await?;

    Ok(())
}

async fn create_new_pr(
    git: &GitRepo,
    github: &GitHubClient,
    branch_name: &str,
    stack: &Stack,
    config: &Config,
) -> Result<()> {
    let branch = stack
        .branches
        .get(branch_name)
        .ok_or_else(|| anyhow!("Branch not found in stack"))?;

    // Determine the base branch (parent or default)
    let base_branch = branch
        .parent
        .as_ref()
        .unwrap_or(&config.default_base_branch);

    // Auto-push if configured
    if config.auto_push {
        if !git.has_remote_branch(branch_name)? {
            utils::print_info(&format!("Pushing branch '{branch_name}' to remote..."));
            git.push_branch(branch_name, false)?;
        }
    } else {
        // Check if branch exists on remote
        if !git.has_remote_branch(branch_name)? {
            let should_push = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!(
                    "Branch '{branch_name}' not found on remote. Push now?"
                ))
                .default(true)
                .interact()?;

            if should_push {
                git.push_branch(branch_name, false)?;
            } else {
                return Err(anyhow!("Cannot create PR without pushing branch to remote"));
            }
        }
    }

    git.ensure_tracking_branch(branch_name)?;

    // Generate title from branch name (convert kebab-case to Title Case)
    let default_title = branch_name
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Get PR title from user
    let title: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("PR title")
        .default(default_title)
        .interact_text()?;

    if title.trim().is_empty() {
        return Err(anyhow!("PR title cannot be empty"));
    }

    // Get PR body (use template if available)
    let default_body = config.pr_template.as_deref().unwrap_or(
        "## Summary\n\n<!-- Describe your changes -->\n\n## Testing\n\n<!-- How did you test this? -->"
    );

    let body: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("PR description")
        .default(default_body.to_string())
        .interact_text()?;

    // Create the PR
    utils::print_info(&format!(
        "Creating PR: '{title}' ({branch_name} → {base_branch})"
    ));

    let pr = github
        .create_pull_request(&title, &body, branch_name, base_branch, config.draft_prs)
        .await?;

    utils::print_success(&format!("PR created: {}", pr.html_url));

    if config.draft_prs {
        utils::print_info("PR created as draft. Mark as ready for review when complete.");
    }

    Ok(())
}

fn render_stack_comment(stack: &Stack, current_pr_number: u64) -> String {
    let current_branch = &stack.current_branch;
    let stack_branches = stack.get_stack_for_branch(current_branch);

    // Filter to branches that have PRs (skip main/branches without PRs)
    let branches_with_prs: Vec<_> = stack_branches
        .iter()
        .filter(|b| b.pull_request.is_some())
        .collect();

    let mut lines = vec![
        STACK_COMMENT_MARKER.to_string(),
        "### Stack".to_string(),
        "| # | Branch | PR | |".to_string(),
        "|---|--------|----|-|".to_string(),
    ];

    for (i, branch) in branches_with_prs.iter().enumerate() {
        let pr = branch.pull_request.as_ref().unwrap();
        let num = i + 1;
        let is_current = pr.number == current_pr_number;

        let row = if is_current {
            format!(
                "| {num} | **{}** | **[#{}]({})** | **\u{2190} this PR** |",
                branch.name, pr.number, pr.html_url
            )
        } else {
            format!(
                "| {num} | {} | [#{}]({}) | |",
                branch.name, pr.number, pr.html_url
            )
        };
        lines.push(row);
    }

    lines.join("\n")
}

async fn update_stack_comments(github: &GitHubClient, stack: &Stack) -> Result<()> {
    let current_branch = &stack.current_branch;
    let stack_branches = stack.get_stack_for_branch(current_branch);

    let branches_with_prs: Vec<_> = stack_branches
        .iter()
        .filter(|b| b.pull_request.is_some())
        .collect();

    if branches_with_prs.is_empty() {
        return Ok(());
    }

    utils::print_info("Updating stack comments on PRs...");

    for branch in &branches_with_prs {
        let pr = branch.pull_request.as_ref().unwrap();
        let comment_body = render_stack_comment(stack, pr.number);

        let comments = github.list_pr_comments(pr.number).await?;
        let existing = comments
            .iter()
            .find(|(_, body)| body.contains(STACK_COMMENT_MARKER));

        match existing {
            Some((comment_id, _)) => {
                github.update_pr_comment(*comment_id, &comment_body).await?;
            }
            None => {
                github.create_pr_comment(pr.number, &comment_body).await?;
            }
        }
    }

    utils::print_success("Stack comments updated on all PRs");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::PullRequest;
    use crate::stack::{Stack, StackBranch};
    use std::collections::HashMap;

    fn make_pr(number: u64, branch: &str) -> PullRequest {
        PullRequest {
            number,
            title: format!("PR for {branch}"),
            body: None,
            state: "open".to_string(),
            head_ref: branch.to_string(),
            base_ref: "main".to_string(),
            html_url: format!("https://github.com/owner/repo/pull/{number}"),
            draft: false,
        }
    }

    fn make_branch(
        name: &str,
        parent: Option<&str>,
        children: Vec<&str>,
        pr: Option<PullRequest>,
    ) -> StackBranch {
        StackBranch {
            name: name.to_string(),
            parent: parent.map(|s| s.to_string()),
            children: children.into_iter().map(|s| s.to_string()).collect(),
            commit_hash: "abc123".to_string(),
            pull_request: pr,
            is_current: false,
        }
    }

    /// main -> A (#1) -> B (#2) -> C (#3), current_branch = B
    fn make_linear_stack() -> Stack {
        let mut branches = HashMap::new();
        branches.insert(
            "main".to_string(),
            make_branch("main", None, vec!["A"], None),
        );
        branches.insert(
            "A".to_string(),
            make_branch("A", Some("main"), vec!["B"], Some(make_pr(1, "A"))),
        );
        branches.insert(
            "B".to_string(),
            make_branch("B", Some("A"), vec!["C"], Some(make_pr(2, "B"))),
        );
        branches.insert(
            "C".to_string(),
            make_branch("C", Some("B"), vec![], Some(make_pr(3, "C"))),
        );

        Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "B".to_string(),
        }
    }

    #[test]
    fn test_render_stack_comment_contains_marker() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        assert!(comment.starts_with(STACK_COMMENT_MARKER));
    }

    #[test]
    fn test_render_stack_comment_contains_header() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        assert!(comment.contains("### Stack"));
        assert!(comment.contains("| # | Branch | PR | |"));
    }

    #[test]
    fn test_render_stack_comment_current_pr_is_bolded() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);

        // PR #2 (branch B) should be bolded with arrow
        assert!(comment.contains("| **B** |"));
        assert!(comment.contains("**[#2]"));
        assert!(comment.contains("\u{2190} this PR"));
    }

    #[test]
    fn test_render_stack_comment_other_prs_not_bolded() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);

        // PR #1 (branch A) should NOT be bolded
        assert!(comment.contains("| A |"));
        assert!(comment.contains("[#1](https://github.com/owner/repo/pull/1)"));
        // PR #3 (branch C) should NOT be bolded
        assert!(comment.contains("| C |"));
        assert!(comment.contains("[#3](https://github.com/owner/repo/pull/3)"));
    }

    #[test]
    fn test_render_stack_comment_numbering() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        let lines: Vec<&str> = comment.lines().collect();

        // After header (marker, title, col header, separator) we have 3 data rows
        // Row for A should be #1
        assert!(lines.iter().any(|l| l.starts_with("| 1 | A |")));
        // Row for B should be #2
        assert!(lines.iter().any(|l| l.starts_with("| 2 | **B** |")));
        // Row for C should be #3
        assert!(lines.iter().any(|l| l.starts_with("| 3 | C |")));
    }

    #[test]
    fn test_render_stack_comment_skips_branches_without_prs() {
        let stack = make_linear_stack();
        // main has no PR, so it shouldn't appear in the table
        let comment = render_stack_comment(&stack, 1);
        assert!(!comment.contains("| main |"));
        // Only 3 data rows (A, B, C)
        let data_rows: Vec<&str> = comment
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| #") && !l.starts_with("|--"))
            .collect();
        assert_eq!(data_rows.len(), 3);
    }

    #[test]
    fn test_render_stack_comment_single_pr() {
        let mut branches = HashMap::new();
        branches.insert(
            "main".to_string(),
            make_branch("main", None, vec!["feat"], None),
        );
        branches.insert(
            "feat".to_string(),
            make_branch("feat", Some("main"), vec![], Some(make_pr(10, "feat"))),
        );
        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "feat".to_string(),
        };

        let comment = render_stack_comment(&stack, 10);
        assert!(comment.contains(STACK_COMMENT_MARKER));
        assert!(comment.contains("| 1 | **feat** |"));
        assert!(comment.contains("\u{2190} this PR"));
        // Only one data row
        let data_rows: Vec<&str> = comment
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| #") && !l.starts_with("|--"))
            .collect();
        assert_eq!(data_rows.len(), 1);
    }

    #[test]
    fn test_render_stack_comment_different_current_per_pr() {
        let stack = make_linear_stack();

        // Rendered for PR #1 — A should be current
        let comment_a = render_stack_comment(&stack, 1);
        assert!(comment_a.contains("| **A** |"));
        assert!(comment_a.contains("| B |"));
        assert!(comment_a.contains("| C |"));

        // Rendered for PR #3 — C should be current
        let comment_c = render_stack_comment(&stack, 3);
        assert!(comment_c.contains("| A |"));
        assert!(comment_c.contains("| B |"));
        assert!(comment_c.contains("| **C** |"));
    }

    #[test]
    fn test_render_stack_comment_mixed_pr_coverage() {
        // A has PR, B does not, C has PR
        let mut branches = HashMap::new();
        branches.insert(
            "main".to_string(),
            make_branch("main", None, vec!["A"], None),
        );
        branches.insert(
            "A".to_string(),
            make_branch("A", Some("main"), vec!["B"], Some(make_pr(1, "A"))),
        );
        branches.insert(
            "B".to_string(),
            make_branch("B", Some("A"), vec!["C"], None),
        );
        branches.insert(
            "C".to_string(),
            make_branch("C", Some("B"), vec![], Some(make_pr(3, "C"))),
        );
        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "C".to_string(),
        };

        let comment = render_stack_comment(&stack, 3);

        // B should be skipped (no PR) — only A and C appear
        let data_rows: Vec<&str> = comment
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| #") && !l.starts_with("|--"))
            .collect();
        assert_eq!(data_rows.len(), 2);
        assert!(comment.contains("| 1 | A |"));
        assert!(comment.contains("| 2 | **C** |"));
        assert!(!comment.contains("| B |"));
    }
}

async fn update_existing_pr(github: &GitHubClient, pr_number: u64, _config: &Config) -> Result<()> {
    // Get new title
    let title: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("New PR title (leave empty to keep current)")
        .allow_empty(true)
        .interact_text()?;

    // Get new body
    let body: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("New PR description (leave empty to keep current)")
        .allow_empty(true)
        .interact_text()?;

    let title_option = if title.trim().is_empty() {
        None
    } else {
        Some(title.as_str())
    };
    let body_option = if body.trim().is_empty() {
        None
    } else {
        Some(body.as_str())
    };

    if title_option.is_none() && body_option.is_none() {
        utils::print_info("No changes made to PR");
        return Ok(());
    }

    let updated_pr = github
        .update_pull_request(
            pr_number,
            title_option,
            body_option,
            None, // Don't change base branch
        )
        .await?;

    utils::print_success(&format!("PR updated: {}", updated_pr.html_url));
    Ok(())
}
