use crate::commands::navigate::{build_commit_cache, find_children, find_parent, is_main_branch};
use crate::git::GitRepo;
use crate::github::{GitHubClient, PullRequest};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct StackBranch {
    pub name: String,
    pub parent: Option<String>,
    pub children: Vec<String>,
    #[allow(dead_code)]
    pub commit_hash: String,
    pub pull_request: Option<PullRequest>,
    pub is_current: bool,
}

#[derive(Debug, Clone)]
pub struct Stack {
    pub branches: HashMap<String, StackBranch>,
    pub roots: Vec<String>,
    pub current_branch: String,
}

impl Stack {
    pub async fn analyze(git: &GitRepo, github: Option<&GitHubClient>) -> Result<Self> {
        let all_branches = git.get_branches()?;
        let current_branch = git.current_branch()?;
        log::debug!(
            "Analyzing stack: {} branches, current='{}'",
            all_branches.len(),
            current_branch
        );

        let main_branches = ["main", "master", "develop"];

        // Filter out branches that are fully merged into trunk
        let trunk = main_branches
            .iter()
            .find(|name| all_branches.contains(&name.to_string()))
            .map(|s| s.to_string());
        let branches: Vec<String> = all_branches
            .into_iter()
            .filter(|b| {
                main_branches.contains(&b.as_str())
                    || trunk.as_ref().is_none_or(|t| !is_merged_into(git, b, t))
            })
            .collect();

        // Fetch first page of open PRs (single request, covers most small/medium repos)
        // Run concurrently with local git work
        let bulk_handle = if let Some(gh) = github {
            let gh = gh.clone();
            Some(tokio::spawn(
                async move { gh.get_open_pull_requests().await },
            ))
        } else {
            None
        };

        // Do expensive local git work while the bulk fetch is in flight
        let (relationships, commits) = Self::detect_relationships(git, &branches)?;

        // Collect bulk PR results
        let mut prs: HashMap<String, PullRequest> = HashMap::new();
        if let Some(handle) = bulk_handle {
            for pr in handle.await?? {
                prs.insert(pr.head_ref.clone(), pr);
            }
        }

        // Find branches still missing PRs and fetch individually in parallel
        if let Some(gh) = github {
            let missing: Vec<_> = branches
                .iter()
                .filter(|b| !main_branches.contains(&b.as_str()) && !prs.contains_key(*b))
                .cloned()
                .collect();

            if !missing.is_empty() {
                let handles: Vec<_> = missing
                    .into_iter()
                    .map(|branch| {
                        let gh = gh.clone();
                        tokio::spawn(async move { gh.get_pr_for_branch(&branch).await })
                    })
                    .collect();

                for handle in handles {
                    if let Ok(Some(pr)) = handle.await? {
                        prs.insert(pr.head_ref.clone(), pr);
                    }
                }
            }
        }

        let mut stack_branches = HashMap::new();
        for branch_name in &branches {
            let commit_hash = commits.get(branch_name).cloned().unwrap_or_default();
            let pull_request = prs.get(branch_name).cloned();
            let is_current = branch_name == &current_branch;

            stack_branches.insert(
                branch_name.clone(),
                StackBranch {
                    name: branch_name.clone(),
                    parent: None,
                    children: Vec::new(),
                    commit_hash,
                    pull_request,
                    is_current,
                },
            );
        }

        for (child, parent) in relationships {
            log::debug!("Relationship: '{}' -> parent '{}'", child, parent);
            if let Some(child_branch) = stack_branches.get_mut(&child) {
                child_branch.parent = Some(parent.clone());
            }
            if let Some(parent_branch) = stack_branches.get_mut(&parent) {
                parent_branch.children.push(child);
            }
        }

        let roots: Vec<_> = stack_branches
            .values()
            .filter(|b| b.parent.is_none())
            .map(|b| b.name.clone())
            .collect();

        log::debug!(
            "Stack analysis complete: {} branches, {} roots, {} PRs found",
            stack_branches.len(),
            roots.len(),
            prs.len()
        );

        Ok(Stack {
            branches: stack_branches,
            roots,
            current_branch,
        })
    }

    /// Targeted analysis that only discovers the current branch's lineage.
    /// Walks the parent chain up to root, then finds children at each level.
    /// O(n × depth) merge-base calls instead of O(n²).
    pub async fn analyze_for_branch(
        git: &GitRepo,
        branch: &str,
        github: Option<&GitHubClient>,
    ) -> Result<Self> {
        let all_branches = git.get_branches()?;
        let current_branch = git.current_branch()?;

        let commits = build_commit_cache(git, &all_branches)?;

        // Compute merged set using the commit cache
        let trunk = all_branches.iter().find(|b| is_main_branch(b)).cloned();
        let merged: HashSet<String> = if let Some(ref trunk_name) = trunk {
            all_branches
                .iter()
                .filter(|b| {
                    if is_main_branch(b) {
                        return false;
                    }
                    let Some(bh) = commits.get(*b) else {
                        return false;
                    };
                    git.get_merge_base(b, trunk_name)
                        .map(|mb| *bh == mb.to_string())
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        } else {
            HashSet::new()
        };

        // Walk parent chain from the target branch up to the root
        let mut lineage = vec![branch.to_string()];
        let mut current = branch.to_string();
        while !is_main_branch(&current) {
            match find_parent(git, &current, &all_branches, &commits, &merged)? {
                Some(parent) => {
                    lineage.push(parent.clone());
                    current = parent;
                }
                None => break,
            }
        }

        // For each branch in the lineage, discover children (siblings + descendants)
        let mut discovered: HashSet<String> = lineage.iter().cloned().collect();
        let mut queue: Vec<String> = lineage.clone();
        while let Some(b) = queue.pop() {
            let children = find_children(git, &b, &all_branches, &commits, &merged)?;
            for child in children {
                if discovered.insert(child.clone()) {
                    queue.push(child);
                }
            }
        }

        // Build relationships only for discovered branches
        let discovered_branches: Vec<String> = all_branches
            .iter()
            .filter(|b| discovered.contains(*b))
            .cloned()
            .collect();

        // Compute parent for each discovered non-main, non-merged branch
        let mut relationships = Vec::new();
        for b in &discovered_branches {
            if is_main_branch(b) || merged.contains(b) {
                continue;
            }
            if let Some(parent) = find_parent(git, b, &all_branches, &commits, &merged)? {
                relationships.push((b.clone(), parent));
            }
        }

        // Fetch PRs concurrently
        let bulk_handle = if let Some(gh) = github {
            let gh = gh.clone();
            Some(tokio::spawn(
                async move { gh.get_open_pull_requests().await },
            ))
        } else {
            None
        };

        let mut prs: HashMap<String, PullRequest> = HashMap::new();
        if let Some(handle) = bulk_handle {
            for pr in handle.await?? {
                if discovered.contains(&pr.head_ref) {
                    prs.insert(pr.head_ref.clone(), pr);
                }
            }
        }

        // Fetch individual PRs for branches still missing
        if let Some(gh) = github {
            let missing: Vec<_> = discovered_branches
                .iter()
                .filter(|b| !is_main_branch(b) && !prs.contains_key(*b))
                .cloned()
                .collect();

            if !missing.is_empty() {
                let handles: Vec<_> = missing
                    .into_iter()
                    .map(|b| {
                        let gh = gh.clone();
                        tokio::spawn(async move { gh.get_pr_for_branch(&b).await })
                    })
                    .collect();

                for handle in handles {
                    if let Ok(Some(pr)) = handle.await? {
                        prs.insert(pr.head_ref.clone(), pr);
                    }
                }
            }
        }

        // Build StackBranch entries
        let mut stack_branches = HashMap::new();
        for b in &discovered_branches {
            let commit_hash = commits.get(b).cloned().unwrap_or_default();
            let pull_request = prs.get(b).cloned();
            let is_current = *b == current_branch;

            stack_branches.insert(
                b.clone(),
                StackBranch {
                    name: b.clone(),
                    parent: None,
                    children: Vec::new(),
                    commit_hash,
                    pull_request,
                    is_current,
                },
            );
        }

        for (child, parent) in relationships {
            if let Some(child_branch) = stack_branches.get_mut(&child) {
                child_branch.parent = Some(parent.clone());
            }
            if let Some(parent_branch) = stack_branches.get_mut(&parent) {
                parent_branch.children.push(child);
            }
        }

        let roots = stack_branches
            .values()
            .filter(|b| b.parent.is_none())
            .map(|b| b.name.clone())
            .collect();

        Ok(Stack {
            branches: stack_branches,
            roots,
            current_branch,
        })
    }

    #[allow(clippy::type_complexity)]
    fn detect_relationships(
        git: &GitRepo,
        branches: &[String],
    ) -> Result<(Vec<(String, String)>, HashMap<String, String>)> {
        let mut relationships = Vec::new();
        let main_branches = ["main", "master", "develop"];

        // Pre-compute all commit hashes once (avoids redundant calls in inner loop)
        let commits: HashMap<String, String> = branches
            .iter()
            .map(|b| {
                let hash = git.get_commit_hash(&format!("refs/heads/{b}"))?;
                Ok((b.clone(), hash))
            })
            .collect::<Result<_>>()?;

        // Find the trunk branch for fallback and merged-branch detection
        let trunk = main_branches
            .iter()
            .find(|name| branches.contains(&name.to_string()))
            .map(|s| s.to_string());

        // Pre-compute merged status once per branch
        let merged: HashSet<String> = if let Some(ref trunk_name) = trunk {
            branches
                .iter()
                .filter(|b| {
                    !main_branches.contains(&b.as_str())
                        && is_merged_into_cached(&commits, git, b, trunk_name)
                })
                .cloned()
                .collect()
        } else {
            HashSet::new()
        };

        for branch in branches {
            if main_branches.contains(&branch.as_str()) || merged.contains(branch) {
                continue;
            }

            let current_commit = &commits[branch];
            let mut best_parent = None;
            let mut min_distance = usize::MAX;

            for potential_parent in branches {
                if branch == potential_parent {
                    continue;
                }

                let parent_commit = &commits[potential_parent];

                if current_commit == parent_commit {
                    continue;
                }

                // Skip merged non-trunk branches
                if !main_branches.contains(&potential_parent.as_str())
                    && merged.contains(potential_parent)
                {
                    continue;
                }

                if let Ok(merge_base) = git.get_merge_base(branch, potential_parent) {
                    if merge_base.to_string() == *parent_commit {
                        let distance = git.count_commits_between(
                            &format!("refs/heads/{potential_parent}"),
                            &format!("refs/heads/{branch}"),
                        )?;
                        if distance < min_distance {
                            min_distance = distance;
                            best_parent = Some(potential_parent.clone());
                        }
                    }
                }
            }

            // Fall back to trunk if no parent detected
            if best_parent.is_none() {
                if let Some(ref trunk_name) = trunk {
                    best_parent = Some(trunk_name.clone());
                }
            }

            if let Some(parent) = best_parent {
                relationships.push((branch.clone(), parent));
            }
        }

        Ok((relationships, commits))
    }

    pub fn get_stack_for_branch(&self, branch_name: &str) -> Vec<&StackBranch> {
        let mut stack = Vec::new();
        let mut current = branch_name;

        while let Some(branch) = self.branches.get(current) {
            stack.push(branch);
            if let Some(parent) = &branch.parent {
                current = parent;
            } else {
                break;
            }
        }

        stack.reverse();

        let mut queue = vec![branch_name];
        let mut visited = HashSet::new();
        visited.insert(branch_name);

        while let Some(current_branch) = queue.pop() {
            if let Some(branch) = self.branches.get(current_branch) {
                for child in &branch.children {
                    if !visited.contains(child.as_str()) {
                        visited.insert(child);
                        stack.push(self.branches.get(child).unwrap());
                        queue.push(child);
                    }
                }
            }
        }

        stack
    }

    #[allow(dead_code)]
    pub fn is_stack_clean(&self, branch_name: &str) -> bool {
        let stack = self.get_stack_for_branch(branch_name);
        stack.iter().all(|b| {
            b.pull_request
                .as_ref()
                .is_none_or(|pr| pr.state == "open" || pr.state == "draft")
        })
    }
}

/// Check if a branch is fully merged into trunk
/// (its tip is an ancestor of trunk's tip).
pub(crate) fn is_merged_into(git: &GitRepo, branch: &str, trunk: &str) -> bool {
    let branch_hash = git.get_commit_hash(&format!("refs/heads/{branch}")).ok();
    let merge_base = git.get_merge_base(branch, trunk).ok();
    match (branch_hash, merge_base) {
        (Some(bh), Some(mb)) => bh == mb.to_string(),
        _ => false,
    }
}

/// Same check but uses pre-computed commit hashes to avoid redundant lookups.
fn is_merged_into_cached(
    commits: &HashMap<String, String>,
    git: &GitRepo,
    branch: &str,
    trunk: &str,
) -> bool {
    let Some(branch_hash) = commits.get(branch) else {
        return false;
    };
    let merge_base = git.get_merge_base(branch, trunk).ok();
    match merge_base {
        Some(mb) => *branch_hash == mb.to_string(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::PullRequest;
    use std::collections::HashMap;

    fn create_test_branch(
        name: &str,
        parent: Option<String>,
        children: Vec<String>,
    ) -> StackBranch {
        StackBranch {
            name: name.to_string(),
            parent,
            children,
            commit_hash: "abc123".to_string(),
            pull_request: None,
            is_current: false,
        }
    }

    fn create_test_pr(head_ref: &str, state: &str) -> PullRequest {
        PullRequest {
            number: 1,
            title: "Test PR".to_string(),
            body: Some("Test body".to_string()),
            head_ref: head_ref.to_string(),
            base_ref: "main".to_string(),
            state: state.to_string(),
            html_url: "https://github.com/test/test/pull/1".to_string(),
            draft: false,
        }
    }

    #[test]
    fn test_stack_creation() {
        let mut branches = HashMap::new();
        branches.insert(
            "main".to_string(),
            create_test_branch("main", None, vec!["feature1".to_string()]),
        );
        branches.insert(
            "feature1".to_string(),
            create_test_branch("feature1", Some("main".to_string()), vec![]),
        );

        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "feature1".to_string(),
        };

        assert_eq!(stack.roots.len(), 1);
        assert_eq!(stack.current_branch, "feature1");
        assert!(stack.branches.contains_key("main"));
        assert!(stack.branches.contains_key("feature1"));
    }

    #[test]
    fn test_get_stack_for_branch() {
        let mut branches = HashMap::new();
        branches.insert(
            "main".to_string(),
            create_test_branch("main", None, vec!["feature1".to_string()]),
        );
        branches.insert(
            "feature1".to_string(),
            create_test_branch(
                "feature1",
                Some("main".to_string()),
                vec!["feature2".to_string()],
            ),
        );
        branches.insert(
            "feature2".to_string(),
            create_test_branch("feature2", Some("feature1".to_string()), vec![]),
        );

        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "feature2".to_string(),
        };

        let stack_branches = stack.get_stack_for_branch("feature2");
        assert!(stack_branches.len() >= 2);

        let branch_names: Vec<&str> = stack_branches.iter().map(|b| b.name.as_str()).collect();
        assert!(branch_names.contains(&"feature2"));
    }

    #[test]
    fn test_is_stack_clean_with_no_prs() {
        let mut branches = HashMap::new();
        branches.insert(
            "feature1".to_string(),
            create_test_branch("feature1", None, vec![]),
        );

        let stack = Stack {
            branches,
            roots: vec!["feature1".to_string()],
            current_branch: "feature1".to_string(),
        };

        assert!(stack.is_stack_clean("feature1"));
    }

    #[test]
    fn test_is_stack_clean_with_open_pr() {
        let mut branch = create_test_branch("feature1", None, vec![]);
        branch.pull_request = Some(create_test_pr("feature1", "open"));

        let mut branches = HashMap::new();
        branches.insert("feature1".to_string(), branch);

        let stack = Stack {
            branches,
            roots: vec!["feature1".to_string()],
            current_branch: "feature1".to_string(),
        };

        assert!(stack.is_stack_clean("feature1"));
    }

    #[test]
    fn test_is_stack_clean_with_closed_pr() {
        let mut branch = create_test_branch("feature1", None, vec![]);
        branch.pull_request = Some(create_test_pr("feature1", "closed"));

        let mut branches = HashMap::new();
        branches.insert("feature1".to_string(), branch);

        let stack = Stack {
            branches,
            roots: vec!["feature1".to_string()],
            current_branch: "feature1".to_string(),
        };

        assert!(!stack.is_stack_clean("feature1"));
    }
}
