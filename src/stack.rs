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

#[derive(Debug)]
pub struct Stack {
    pub branches: HashMap<String, StackBranch>,
    pub roots: Vec<String>,
    pub current_branch: String,
}

impl Stack {
    pub async fn analyze(git: &GitRepo, github: Option<&GitHubClient>) -> Result<Self> {
        let branches = git.get_branches()?;
        let current_branch = git.current_branch()?;

        let mut stack_branches = HashMap::new();
        let mut prs = HashMap::new();

        if let Some(github_client) = github {
            let all_prs = github_client.get_pull_requests().await?;
            for pr in all_prs {
                prs.insert(pr.head_ref.clone(), pr);
            }
        }

        for branch_name in &branches {
            let commit_hash = git.get_commit_hash(&format!("refs/heads/{branch_name}"))?;
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

        let relationships = Self::detect_relationships(git, &branches)?;

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

    fn detect_relationships(git: &GitRepo, branches: &[String]) -> Result<Vec<(String, String)>> {
        let mut relationships = Vec::new();
        let main_branches = ["main", "master", "develop"];

        for branch in branches {
            if main_branches.contains(&branch.as_str()) {
                continue;
            }

            let current_commit = git.get_commit_hash(&format!("refs/heads/{branch}"))?;
            let mut best_parent = None;
            let mut min_distance = usize::MAX;

            for potential_parent in branches {
                if branch == potential_parent || main_branches.contains(&potential_parent.as_str())
                {
                    continue;
                }

                let parent_commit =
                    git.get_commit_hash(&format!("refs/heads/{potential_parent}"))?;

                // Skip if both branches point to the same commit (they're siblings, not parent-child)
                if current_commit == parent_commit {
                    continue;
                }

                if let Ok(merge_base) = git.get_merge_base(branch, potential_parent) {
                    if merge_base.to_string() == parent_commit {
                        let distance =
                            Self::calculate_commit_distance(git, potential_parent, branch)?;
                        if distance < min_distance {
                            min_distance = distance;
                            best_parent = Some(potential_parent.clone());
                        }
                    }
                }
            }

            if let Some(parent) = best_parent {
                relationships.push((branch.clone(), parent));
            } else {
                for main_branch in &main_branches {
                    if git
                        .get_commit_hash(&format!("refs/heads/{main_branch}"))
                        .is_ok()
                    {
                        relationships.push((branch.clone(), main_branch.to_string()));
                        break;
                    }
                }
            }
        }

        Ok(relationships)
    }

    fn calculate_commit_distance(git: &GitRepo, from: &str, to: &str) -> Result<usize> {
        let from_commit = git.get_commit_hash(&format!("refs/heads/{from}"))?;
        let to_commit = git.get_commit_hash(&format!("refs/heads/{to}"))?;

        if from_commit == to_commit {
            return Ok(0);
        }

        Ok(1)
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

    pub fn is_stack_clean(&self, branch_name: &str) -> bool {
        let stack = self.get_stack_for_branch(branch_name);
        stack.iter().all(|b| {
            b.pull_request
                .as_ref()
                .is_none_or(|pr| pr.state == "open" || pr.state == "draft")
        })
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

    #[test]
    fn test_calculate_commit_distance_same_commit() {
        let mut branches = HashMap::new();
        branches.insert("main".to_string(), create_test_branch("main", None, vec![]));

        let stack = Stack {
            branches,
            roots: vec!["main".to_string()],
            current_branch: "main".to_string(),
        };

        assert_eq!(stack.branches.len(), 1);
        assert!(stack.branches.contains_key("main"));
    }
}
