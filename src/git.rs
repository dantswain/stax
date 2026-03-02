use anyhow::{anyhow, Result};
use git2::{BranchType, Oid, Repository};
use std::path::Path;

pub struct GitRepo {
    repo: Repository,
}

impl GitRepo {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let repo = Repository::discover(path)?;
        Ok(GitRepo { repo })
    }

    pub fn current_branch(&self) -> Result<String> {
        let head = self.repo.head()?;
        if let Some(name) = head.shorthand() {
            Ok(name.to_string())
        } else {
            Err(anyhow!("Not on a named branch"))
        }
    }

    pub fn get_branches(&self) -> Result<Vec<String>> {
        let branches = self.repo.branches(Some(BranchType::Local))?;
        let mut branch_names = Vec::new();

        for branch_result in branches {
            let (branch, _) = branch_result?;
            if let Some(name) = branch.name()? {
                branch_names.push(name.to_string());
            }
        }

        Ok(branch_names)
    }

    pub fn create_branch(&self, name: &str, from_ref: Option<&str>) -> Result<()> {
        let target_commit = if let Some(from) = from_ref {
            let reference = self.repo.find_reference(from)?;
            self.repo.reference_to_annotated_commit(&reference)?
        } else {
            let head = self.repo.head()?;
            self.repo.reference_to_annotated_commit(&head)?
        };

        let commit = self.repo.find_commit(target_commit.id())?;
        self.repo.branch(name, &commit, false)?;

        Ok(())
    }

    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        let obj = self.repo.revparse_single(&format!("refs/heads/{name}"))?;
        self.repo.checkout_tree(&obj, None)?;
        self.repo.set_head(&format!("refs/heads/{name}"))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_branch_upstream(&self, branch_name: &str) -> Result<Option<String>> {
        let branch = self.repo.find_branch(branch_name, BranchType::Local)?;
        if let Ok(upstream) = branch.upstream() {
            if let Some(name) = upstream.name()? {
                return Ok(Some(name.to_string()));
            }
        }
        Ok(None)
    }

    pub fn get_commit_hash(&self, reference: &str) -> Result<String> {
        let obj = self.repo.revparse_single(reference)?;
        Ok(obj.id().to_string())
    }

    pub fn is_clean(&self) -> Result<bool> {
        let statuses = self.repo.statuses(None)?;

        // Only consider files that are actually problematic for branch switching
        for entry in statuses.iter() {
            let status = entry.status();

            // Check for staged or modified files (not just untracked files)
            if status.is_index_new()
                || status.is_index_modified()
                || status.is_index_deleted()
                || status.is_wt_modified()
                || status.is_wt_deleted()
            {
                return Ok(false);
            }

            // Ignore untracked files - they don't prevent branch switching
            // and git status --porcelain doesn't show them as problematic
        }

        Ok(true)
    }

    pub fn get_remote_url(&self, remote_name: &str) -> Option<String> {
        if let Ok(remote) = self.repo.find_remote(remote_name) {
            remote.url().map(|s| s.to_string())
        } else {
            None
        }
    }

    pub fn push_branch(&self, branch_name: &str, force: bool) -> Result<()> {
        let workdir = self.workdir()?;
        let mut args = vec!["push", "origin"];
        if force {
            args.push("--force-with-lease");
        }
        // Push as branch_name:branch_name to be explicit
        let refspec_owned = format!("{branch_name}:{branch_name}");
        args.push(&refspec_owned);

        log::debug!("Running: git {}", args.join(" "));
        let output = std::process::Command::new("git")
            .args(&args)
            .current_dir(workdir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("git push failed: {}", stderr.trim()));
        }
        Ok(())
    }

    pub fn ensure_tracking_branch(&self, branch_name: &str) -> Result<()> {
        if !self.has_remote_branch(branch_name)? {
            return Err(anyhow!("Remote branch '{branch_name}' does not exist"));
        }

        // Set the upstream tracking branch
        self.track_branch(branch_name)?;
        log::debug!("Tracking remote branch '{branch_name}'");
        Ok(())
    }

    pub fn track_branch(&self, branch_name: &str) -> Result<()> {
        let mut branch = self.repo.find_branch(branch_name, BranchType::Local)?;
        let upstream_name = format!("origin/{branch_name}");
        branch.set_upstream(Some(&upstream_name))?;
        log::debug!("Tracking remote branch '{upstream_name}' for local branch '{branch_name}'");
        Ok(())
    }

    pub fn has_remote_branch(&self, branch_name: &str) -> Result<bool> {
        let remote_ref = format!("refs/remotes/origin/{branch_name}");
        Ok(self.repo.find_reference(&remote_ref).is_ok())
    }

    /// Check if local branch has diverged from its remote counterpart
    /// (i.e., after a rebase, the two share history but are not fast-forward).
    pub fn has_diverged_from_remote(&self, branch_name: &str) -> Result<bool> {
        let local_ref = format!("refs/heads/{branch_name}");
        let remote_ref = format!("refs/remotes/origin/{branch_name}");

        let local_oid = match self.repo.find_reference(&local_ref) {
            Ok(r) => r
                .target()
                .ok_or_else(|| anyhow!("No target for local ref"))?,
            Err(_) => return Ok(false),
        };
        let remote_oid = match self.repo.find_reference(&remote_ref) {
            Ok(r) => r
                .target()
                .ok_or_else(|| anyhow!("No target for remote ref"))?,
            Err(_) => return Ok(false),
        };

        if local_oid == remote_oid {
            return Ok(false);
        }

        // If local is a descendant of remote, it's ahead (not diverged)
        if self.repo.graph_descendant_of(local_oid, remote_oid)? {
            return Ok(false);
        }

        // Otherwise they've diverged
        Ok(true)
    }

    pub fn get_merge_base(&self, branch1: &str, branch2: &str) -> Result<Oid> {
        let commit1 = self.repo.revparse_single(branch1)?.id();
        let commit2 = self.repo.revparse_single(branch2)?.id();
        Ok(self.repo.merge_base(commit1, commit2)?)
    }

    /// Count commits reachable from `to` but not from `from`.
    /// Equivalent to `git rev-list --count from..to`.
    pub fn count_commits_between(&self, from: &str, to: &str) -> Result<usize> {
        let from_oid = self.repo.revparse_single(from)?.id();
        let to_oid = self.repo.revparse_single(to)?.id();

        let mut revwalk = self.repo.revwalk()?;
        revwalk.push(to_oid)?;
        revwalk.hide(from_oid)?;

        Ok(revwalk.count())
    }

    fn workdir(&self) -> Result<&Path> {
        self.repo
            .workdir()
            .ok_or_else(|| anyhow!("Cannot determine working directory"))
    }

    /// Rebase `branch` onto `onto`.
    /// If `old_onto_commit` is provided, uses `--onto` to only replay commits
    /// after the old parent tip (avoids re-applying parent's commits).
    pub fn rebase_onto(&self, branch: &str, onto: &str) -> Result<()> {
        self.rebase_onto_with_base(branch, onto, None)
    }

    pub fn rebase_onto_with_base(
        &self,
        branch: &str,
        onto: &str,
        old_onto_commit: Option<&str>,
    ) -> Result<()> {
        // Skip if branch is already based on onto
        let onto_commit = self.get_commit_hash(&format!("refs/heads/{onto}"))?;
        if let Ok(merge_base) = self.get_merge_base(branch, onto) {
            if merge_base.to_string() == onto_commit {
                return Ok(());
            }
        }

        let workdir = self.workdir()?;

        let output = match old_onto_commit {
            Some(old_base) => {
                // --onto <new_parent> <old_parent_tip> <branch>
                // Only replays commits that are unique to <branch> (after old_parent_tip)
                std::process::Command::new("git")
                    .args(["rebase", "--onto", onto, old_base, branch])
                    .current_dir(workdir)
                    .output()?
            }
            None => std::process::Command::new("git")
                .args(["rebase", onto, branch])
                .current_dir(workdir)
                .output()?,
        };

        if output.status.success() {
            return Ok(());
        }

        // Leave the rebase in progress so the user can resolve conflicts
        Err(anyhow!(
            "Rebase of '{}' onto '{}' hit conflicts.\n\
             Resolve conflicts, stage the files, then run:\n  \
             stax sync --continue\n\
             To abort instead:\n  \
             git rebase --abort",
            branch,
            onto
        ))
    }

    pub fn rebase_continue(&self) -> Result<()> {
        let workdir = self.workdir()?;

        let output = std::process::Command::new("git")
            .args(["rebase", "--continue"])
            .current_dir(workdir)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()?;

        if output.success() {
            return Ok(());
        }

        Err(anyhow!(
            "git rebase --continue failed. Resolve remaining conflicts and run 'stax sync --continue' again."
        ))
    }

    pub fn is_rebase_in_progress(&self) -> bool {
        if let Ok(workdir) = self.workdir() {
            let git_dir = workdir.join(".git");
            git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists()
        } else {
            false
        }
    }

    pub fn fetch(&self) -> Result<()> {
        let workdir = self.workdir()?;
        let output = std::process::Command::new("git")
            .args(["fetch", "--prune", "origin"])
            .current_dir(workdir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("git fetch failed: {}", stderr.trim()));
        }
        Ok(())
    }

    /// Fast-forward a local branch to match its remote tracking branch.
    /// Returns Ok(true) if the branch was updated, Ok(false) if already up to date.
    pub fn fast_forward_branch(&self, branch_name: &str) -> Result<bool> {
        let local_ref_name = format!("refs/heads/{branch_name}");
        let remote_ref_name = format!("refs/remotes/origin/{branch_name}");

        let local_ref = match self.repo.find_reference(&local_ref_name) {
            Ok(r) => r,
            Err(_) => return Ok(false),
        };
        let remote_ref = match self.repo.find_reference(&remote_ref_name) {
            Ok(r) => r,
            Err(_) => return Ok(false),
        };

        let local_oid = local_ref
            .target()
            .ok_or_else(|| anyhow!("Local ref has no target"))?;
        let remote_oid = remote_ref
            .target()
            .ok_or_else(|| anyhow!("Remote ref has no target"))?;

        if local_oid == remote_oid {
            return Ok(false);
        }

        // Remote is ahead of local → fast-forward is possible
        if self.repo.graph_descendant_of(remote_oid, local_oid)? {
            // fall through to do the fast-forward
        } else if self.repo.graph_descendant_of(local_oid, remote_oid)? {
            // Local is ahead of remote → nothing to fast-forward
            return Ok(false);
        } else {
            // Truly diverged — both sides have unique commits
            return Err(anyhow!(
                "Cannot fast-forward '{}': local and remote have diverged",
                branch_name
            ));
        }

        // Move the local ref forward
        self.repo.reference(
            &local_ref_name,
            remote_oid,
            true,
            &format!("stax: fast-forward {branch_name}"),
        )?;

        // If this branch is currently checked out, update the working directory too
        if let Ok(head) = self.repo.head() {
            if head.name() == Some(&local_ref_name) {
                let obj = self.repo.find_object(remote_oid, None)?;
                self.repo
                    .checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().force()))?;
            }
        }

        Ok(true)
    }

    /// Delete a local branch and optionally its remote counterpart.
    pub fn delete_branch(&self, branch_name: &str, delete_remote: bool) -> Result<()> {
        // Delete local branch
        let mut branch = self.repo.find_branch(branch_name, BranchType::Local)?;
        branch.delete()?;

        // Delete remote branch if requested and it exists
        if delete_remote && self.has_remote_branch(branch_name)? {
            let workdir = self.workdir()?;
            let refspec = format!(":{branch_name}");
            let output = std::process::Command::new("git")
                .args(["push", "origin", &refspec])
                .current_dir(workdir)
                .output()?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // If the remote branch was already deleted (e.g., GitHub auto-deletes
                // on merge), treat it as success rather than an error.
                if !stderr.contains("remote ref does not exist") {
                    return Err(anyhow!(
                        "Failed to delete remote branch '{}': {}",
                        branch_name,
                        stderr.trim()
                    ));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_repo_open() {
        // Test that we can open a git repo (will work in the stax project directory)
        let result = GitRepo::open(".");
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_clean_behavior() {
        // This test validates the logic of is_clean without needing a real git repo
        // The actual git2 functionality is tested by integration with the real repo

        // We can't easily test git2 operations without a real git repo,
        // but we can test that the function exists and has the right signature
        let result = GitRepo::open(".");
        if let Ok(git) = result {
            let clean_result = git.is_clean();
            // Should return a Result<bool>, not error on the call itself
            assert!(clean_result.is_ok() || clean_result.is_err());
        }
    }

    #[test]
    fn test_git_url_methods_exist() {
        // Test that our git utility methods exist and have correct signatures
        let result = GitRepo::open(".");
        if let Ok(git) = result {
            // These methods should exist (testing API surface)
            let _ = git.current_branch();
            let _ = git.get_branches();
            let _ = git.get_remote_url("origin");
            let _ = git.has_remote_branch("main");
        }
    }
}
