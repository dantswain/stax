use crate::{
    config::Config,
    git::GitRepo,
    github::{GitHubClient, PullRequest},
    stack::{Stack, StackBranch},
    token_store, utils,
};
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
    let main_branches = ["main", "master", "develop"];

    // Don't create PRs for main branches
    if main_branches.contains(&current_branch.as_str()) {
        return Err(anyhow!(
            "Cannot create PR for main branch '{current_branch}'"
        ));
    }

    // Collect ancestor branches (parent-first) that don't have PRs yet
    let mut ancestors_without_prs: Vec<String> = Vec::new();
    let mut cur = current_branch.as_str();
    while let Some(branch) = stack.branches.get(cur) {
        if let Some(parent) = &branch.parent {
            if !main_branches.contains(&parent.as_str()) {
                if let Some(parent_branch) = stack.branches.get(parent.as_str()) {
                    if parent_branch.pull_request.is_none() {
                        ancestors_without_prs.push(parent.clone());
                    }
                }
            }
            cur = parent;
        } else {
            break;
        }
    }
    ancestors_without_prs.reverse(); // parent-first order

    // If there are parent branches without PRs, ask the user before proceeding
    let mut stack = stack.clone();
    if !ancestors_without_prs.is_empty() {
        utils::print_info("The following parent branches don't have PRs yet:");
        for name in &ancestors_without_prs {
            utils::print_info(&format!("  {name}"));
        }

        let should_submit = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Submit PRs for these branches first?")
            .default(true)
            .interact()?;

        if !should_submit {
            return Err(anyhow!(
                "Cannot submit '{}' without PRs for parent branches",
                current_branch
            ));
        }

        for name in &ancestors_without_prs {
            utils::print_info(&format!("Submitting PR for '{name}'..."));
            create_new_pr(git, github, name, &stack, config).await?;
            // Re-analyze after each PR so the next branch sees the updated state
            stack = Stack::analyze(git, Some(github)).await?;
        }
    }

    let current_stack_branch = stack
        .branches
        .get(current_branch)
        .ok_or_else(|| anyhow!("Current branch not found in stack"))?;

    // Push branch to remote (force-push if diverged after rebase)
    push_branch(git, config, current_branch)?;

    // If PR already exists, just push and update stack comments
    if let Some(existing_pr) = &current_stack_branch.pull_request {
        utils::print_success(&format!("Updated PR: {}", existing_pr.html_url));

        let fresh_stack = Stack::analyze(git, Some(github)).await?;
        update_stack_comments(github, &fresh_stack).await?;

        return Ok(());
    }

    create_new_pr(git, github, current_branch, &stack, config).await?;

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

/// Push a branch to remote, force-pushing with lease if it has diverged (e.g. after rebase).
fn push_branch(git: &GitRepo, config: &Config, branch_name: &str) -> Result<()> {
    if git.has_diverged_from_remote(branch_name)? {
        utils::print_info(&format!(
            "Branch '{branch_name}' has diverged from remote, force-pushing..."
        ));
        git.push_branch(branch_name, true)?;
        utils::print_success(&format!("Force-pushed '{branch_name}'"));
    } else if !git.has_remote_branch(branch_name)? {
        if config.auto_push {
            utils::print_info(&format!("Pushing branch '{branch_name}' to remote..."));
            git.push_branch(branch_name, false)?;
        } else {
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

    // Determine the base branch (parent or default).
    // If the detected parent doesn't exist on the remote, fall back to the default.
    let detected_base = branch
        .parent
        .as_ref()
        .unwrap_or(&config.default_base_branch);

    let base_branch = if git.has_remote_branch(detected_base)? {
        detected_base.clone()
    } else {
        utils::print_warning(&format!(
            "Base branch '{detected_base}' not found on remote, using '{}' instead. \
             Run 'stax sync' to update stack relationships.",
            config.default_base_branch
        ));
        config.default_base_branch.clone()
    };
    let base_branch = &base_branch;

    // Push branch to remote (force-push if diverged after rebase)
    push_branch(git, config, branch_name)?;

    git.ensure_tracking_branch(branch_name)?;

    // Default title: first commit message on this branch, falling back to branch name
    let default_title = branch
        .parent
        .as_ref()
        .and_then(|parent| git.first_commit_message(parent, branch_name).ok().flatten())
        .unwrap_or_else(|| {
            branch_name
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
                .join(" ")
        });

    // Get PR title from user
    let title: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("PR title")
        .default(default_title)
        .interact_text()?;

    if title.trim().is_empty() {
        return Err(anyhow!("PR title cannot be empty"));
    }

    // Get PR body (Ctrl+G opens $EDITOR)
    let default_body = config.pr_template.as_deref().unwrap_or("");
    let body: String = match utils::input_or_editor("PR description")? {
        Some(text) => text,
        None => utils::open_editor(default_body)?,
    };

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

    // Walk up to find the root of the stack (the base branch)
    let mut root = current_branch.as_str();
    while let Some(branch) = stack.branches.get(root) {
        match &branch.parent {
            Some(parent) => root = parent,
            None => break,
        }
    }
    let base_branch = root;

    // Collect ancestor chain: [current_branch, parent, ..., root]
    let mut ancestors = Vec::new();
    let mut cur = current_branch.as_str();
    while let Some(branch) = stack.branches.get(cur) {
        ancestors.push(branch);
        match &branch.parent {
            Some(parent) => cur = parent,
            None => break,
        }
    }

    // Render descendants of current_branch as a tree (children before parent = reversed)
    let mut body_lines = Vec::new();
    if let Some(branch) = stack.branches.get(current_branch.as_str()) {
        render_subtree(stack, branch, 0, current_pr_number, &mut body_lines);
    }

    // Render ancestor chain (parent → ... → root's first child), skipping current_branch
    // and skipping the base branch itself (it goes in the footer)
    for ancestor in &ancestors {
        if ancestor.name == current_branch.as_str() {
            continue;
        }
        if ancestor.name == base_branch && ancestor.pull_request.is_none() {
            continue;
        }
        if let Some(pr) = &ancestor.pull_request {
            let is_current = pr.number == current_pr_number;
            body_lines.push(format_stack_line(&ancestor.name, pr, 0, is_current));
        }
    }

    let mut result = vec![STACK_COMMENT_MARKER.to_string(), "### Stack".to_string()];
    result.extend(body_lines);
    result.push(String::new());
    result.push(format!("\u{2193} merges to `{base_branch}`"));

    result.join("\n")
}

/// Recursively render a branch and its descendants. Children are rendered before
/// their parent so the output is in reverse order (leaves at top).
/// When a branch has multiple children, they are indented one level deeper.
fn render_subtree(
    stack: &Stack,
    branch: &StackBranch,
    depth: usize,
    current_pr_number: u64,
    lines: &mut Vec<String>,
) {
    let children: Vec<_> = branch
        .children
        .iter()
        .filter_map(|name| stack.branches.get(name))
        .collect();

    let child_depth = if children.len() > 1 { depth + 1 } else { depth };

    for child in &children {
        render_subtree(stack, child, child_depth, current_pr_number, lines);
    }

    if let Some(pr) = &branch.pull_request {
        let is_current = pr.number == current_pr_number;
        lines.push(format_stack_line(&branch.name, pr, depth, is_current));
    }
}

fn status_icon(pr: &PullRequest) -> &'static str {
    match pr.state.as_str() {
        "merged" => "\u{1F7E3}",     // 🟣
        "closed" => "\u{1F534}",     // 🔴
        _ if pr.draft => "\u{26AB}", // ⚫
        _ => "\u{1F7E2}",            // 🟢
    }
}

fn format_stack_line(
    branch_name: &str,
    pr: &PullRequest,
    depth: usize,
    is_current: bool,
) -> String {
    let indent = "&nbsp;&nbsp;".repeat(depth);
    let icon = status_icon(pr);
    if is_current {
        format!(
            "- {indent}{icon} **`{branch_name}` [#{}]({}) \u{2190} this PR**",
            pr.number, pr.html_url
        )
    } else {
        format!(
            "- {indent}{icon} `{branch_name}` [#{}]({})",
            pr.number, pr.html_url
        )
    }
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

    /// Helper: extract the list-item lines (lines starting with "- ")
    fn list_lines(comment: &str) -> Vec<&str> {
        comment.lines().filter(|l| l.starts_with("- ")).collect()
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

    // ---- basic structure tests ----

    #[test]
    fn test_contains_marker_and_header() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        assert!(comment.starts_with(STACK_COMMENT_MARKER));
        assert!(comment.contains("### Stack"));
    }

    #[test]
    fn test_merges_to_footer() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        assert!(comment.contains("\u{2193} merges to `main`"));
    }

    #[test]
    fn test_base_branch_not_in_list() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        // main should only appear in the footer, not as a list item
        let items = list_lines(&comment);
        assert!(!items.iter().any(|l| l.contains("`main`")));
    }

    // ---- status icons ----

    #[test]
    fn test_status_icon_open() {
        let pr = make_pr(1, "A");
        assert_eq!(status_icon(&pr), "\u{1F7E2}");
    }

    #[test]
    fn test_status_icon_draft() {
        let mut pr = make_pr(1, "A");
        pr.draft = true;
        assert_eq!(status_icon(&pr), "\u{26AB}");
    }

    #[test]
    fn test_status_icon_merged() {
        let mut pr = make_pr(1, "A");
        pr.state = "merged".to_string();
        assert_eq!(status_icon(&pr), "\u{1F7E3}");
    }

    #[test]
    fn test_status_icon_closed() {
        let mut pr = make_pr(1, "A");
        pr.state = "closed".to_string();
        assert_eq!(status_icon(&pr), "\u{1F534}");
    }

    #[test]
    fn test_merged_pr_in_comment() {
        let mut branches = HashMap::new();
        branches.insert(
            "main".to_string(),
            make_branch("main", None, vec!["A"], None),
        );
        let mut pr = make_pr(1, "A");
        pr.state = "merged".to_string();
        branches.insert(
            "A".to_string(),
            make_branch("A", Some("main"), vec![], Some(pr)),
        );
        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "A".to_string(),
        };
        let comment = render_stack_comment(&stack, 1);
        assert!(comment.contains("\u{1F7E3}"));
    }

    #[test]
    fn test_draft_pr_in_comment() {
        let mut branches = HashMap::new();
        branches.insert(
            "main".to_string(),
            make_branch("main", None, vec!["A"], None),
        );
        let mut pr = make_pr(1, "A");
        pr.draft = true;
        branches.insert(
            "A".to_string(),
            make_branch("A", Some("main"), vec![], Some(pr)),
        );
        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "A".to_string(),
        };
        let comment = render_stack_comment(&stack, 1);
        assert!(comment.contains("\u{26AB}"));
    }

    // ---- linear stack ordering (reversed: leaf at top, root at bottom) ----

    #[test]
    fn test_linear_reversed_order() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        let items = list_lines(&comment);

        // Should be C (leaf) at top, then B, then A (closest to main) at bottom
        assert_eq!(items.len(), 3);
        assert!(items[0].contains("`C`"));
        assert!(items[1].contains("`B`"));
        assert!(items[2].contains("`A`"));
    }

    // ---- current PR marker ----

    #[test]
    fn test_current_pr_is_bolded() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        assert!(comment.contains("**`B` [#2]"));
        assert!(comment.contains("\u{2190} this PR**"));
    }

    #[test]
    fn test_other_prs_not_bolded() {
        let stack = make_linear_stack();
        let comment = render_stack_comment(&stack, 2);
        // A and C should not be bolded
        assert!(comment.contains("\u{1F7E2} `C` [#3]"));
        assert!(comment.contains("\u{1F7E2} `A` [#1]"));
    }

    #[test]
    fn test_different_current_per_pr() {
        let stack = make_linear_stack();

        let comment_a = render_stack_comment(&stack, 1);
        assert!(comment_a.contains("**`A` [#1]"));
        assert!(comment_a.contains("\u{1F7E2} `B` [#2]"));

        let comment_c = render_stack_comment(&stack, 3);
        assert!(comment_c.contains("**`C` [#3]"));
        assert!(comment_c.contains("\u{1F7E2} `A` [#1]"));
    }

    // ---- single PR ----

    #[test]
    fn test_single_pr() {
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
        let items = list_lines(&comment);
        assert_eq!(items.len(), 1);
        assert!(items[0].contains("**`feat` [#10]"));
        assert!(comment.contains("\u{2193} merges to `main`"));
    }

    // ---- branches without PRs are skipped ----

    #[test]
    fn test_skips_branches_without_prs() {
        // main -> A(PR) -> B(no PR) -> C(PR), current_branch = C
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
        let items = list_lines(&comment);
        assert_eq!(items.len(), 2);
        assert!(items[0].contains("`C`"));
        assert!(items[1].contains("`A`"));
        assert!(!comment.contains("`B`"));
    }

    // ---- branching stacks ----

    #[test]
    fn test_branching_children_are_indented() {
        // main -> A(PR) -> B(PR), A -> C(PR), current_branch = A
        let mut branches = HashMap::new();
        branches.insert(
            "main".to_string(),
            make_branch("main", None, vec!["A"], None),
        );
        branches.insert(
            "A".to_string(),
            make_branch("A", Some("main"), vec!["B", "C"], Some(make_pr(1, "A"))),
        );
        branches.insert(
            "B".to_string(),
            make_branch("B", Some("A"), vec![], Some(make_pr(2, "B"))),
        );
        branches.insert(
            "C".to_string(),
            make_branch("C", Some("A"), vec![], Some(make_pr(3, "C"))),
        );
        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "A".to_string(),
        };

        let comment = render_stack_comment(&stack, 1);
        let items = list_lines(&comment);

        // B and C should be indented (depth 1), A should not (depth 0)
        assert_eq!(items.len(), 3);
        assert!(items[0].contains("&nbsp;&nbsp;") && items[0].contains("`B`"));
        assert!(items[1].contains("&nbsp;&nbsp;") && items[1].contains("`C`"));
        assert!(!items[2].contains("&nbsp;&nbsp;") && items[2].contains("`A`"));
    }

    #[test]
    fn test_linear_children_not_indented() {
        // main -> A(PR) -> B(PR) -> C(PR), current_branch = A
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
        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "A".to_string(),
        };

        let comment = render_stack_comment(&stack, 1);
        let items = list_lines(&comment);

        // No branching, so no indentation on any line
        for item in &items {
            assert!(!item.contains("&nbsp;"), "unexpected indent: {item}");
        }
    }

    #[test]
    fn test_deep_branching() {
        // main -> A(PR) -> B(PR) -> C(PR), B -> D(PR), current_branch = A
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
            make_branch("B", Some("A"), vec!["C", "D"], Some(make_pr(2, "B"))),
        );
        branches.insert(
            "C".to_string(),
            make_branch("C", Some("B"), vec![], Some(make_pr(3, "C"))),
        );
        branches.insert(
            "D".to_string(),
            make_branch("D", Some("B"), vec![], Some(make_pr(4, "D"))),
        );
        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "A".to_string(),
        };

        let comment = render_stack_comment(&stack, 2);
        let items = list_lines(&comment);

        // C and D are siblings under B → indented at depth 1
        // B and A are linear → depth 0
        assert_eq!(items.len(), 4);
        assert!(items[0].contains("&nbsp;&nbsp;") && items[0].contains("`C`"));
        assert!(items[1].contains("&nbsp;&nbsp;") && items[1].contains("`D`"));
        assert!(!items[2].contains("&nbsp;&nbsp;") && items[2].contains("**`B`")); // no indent, current PR
        assert!(!items[3].contains("&nbsp;&nbsp;") && items[3].contains("`A`"));
        // no indent
    }

    #[test]
    fn test_branching_through_no_pr_branch() {
        // main -> A(PR) -> B(no PR) -> C(PR), B -> D(PR), current_branch = A
        // B has no PR but has 2 children — children should still be indented
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
            make_branch("B", Some("A"), vec!["C", "D"], None),
        );
        branches.insert(
            "C".to_string(),
            make_branch("C", Some("B"), vec![], Some(make_pr(3, "C"))),
        );
        branches.insert(
            "D".to_string(),
            make_branch("D", Some("B"), vec![], Some(make_pr(4, "D"))),
        );
        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "A".to_string(),
        };

        let comment = render_stack_comment(&stack, 1);
        let items = list_lines(&comment);

        // B is invisible (no PR) but C and D are still indented because B has 2 children
        assert_eq!(items.len(), 3);
        assert!(items[0].contains("&nbsp;&nbsp;") && items[0].contains("`C`"));
        assert!(items[1].contains("&nbsp;&nbsp;") && items[1].contains("`D`"));
        assert!(!items[2].contains("&nbsp;&nbsp;") && items[2].contains("**`A`"));
        assert!(!comment.contains("`B`"));
    }
}
