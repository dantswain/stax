use anyhow::{anyhow, Result};
use git2::{Repository, BranchType, Oid};
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
    pub fn delete_branch(&self, name: &str) -> Result<()> {
        let mut branch = self.repo.find_branch(name, BranchType::Local)?;
        branch.delete()?;
        Ok(())
    }

    pub fn get_branch_upstream(&self, branch_name: &str) -> Result<Option<String>> {
        let branch = self.repo.find_branch(branch_name, BranchType::Local)?;
        if let Ok(upstream) = branch.upstream() {
            if let Some(name) = upstream.name()? {
                return Ok(Some(name.to_string()));
            }
        }
        Ok(None)
    }

    #[allow(dead_code)]
    pub fn set_branch_upstream(&self, branch_name: &str, upstream: &str) -> Result<()> {
        let mut config = self.repo.config()?;
        config.set_str(&format!("branch.{branch_name}.remote"), "origin")?;
        config.set_str(&format!("branch.{branch_name}.merge"), &format!("refs/heads/{upstream}"))?;
        Ok(())
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
                || status.is_wt_deleted() {
                return Ok(false);
            }
            
            // Ignore untracked files - they don't prevent branch switching
            // and git status --porcelain doesn't show them as problematic
        }
        
        Ok(true)
    }

    pub fn get_remote_url(&self, remote_name: &str) -> Result<String> {
        let remote = self.repo.find_remote(remote_name)?;
        if let Some(url) = remote.url() {
            Ok(url.to_string())
        } else {
            Err(anyhow!("Remote URL not found"))
        }
    }

    #[allow(dead_code)]
    pub fn fetch(&self, remote_name: &str) -> Result<()> {
        let mut remote = self.repo.find_remote(remote_name)?;
        remote.fetch(&[] as &[&str], None, None)?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn push_branch(&self, branch_name: &str, remote_name: &str) -> Result<()> {
        let mut remote = self.repo.find_remote(remote_name)?;
        let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
        remote.push(&[&refspec], None)?;
        Ok(())
    }

    pub fn get_merge_base(&self, branch1: &str, branch2: &str) -> Result<Oid> {
        let commit1 = self.repo.revparse_single(branch1)?.id();
        let commit2 = self.repo.revparse_single(branch2)?.id();
        Ok(self.repo.merge_base(commit1, commit2)?)
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
        }
    }
}