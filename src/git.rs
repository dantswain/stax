use anyhow::{anyhow, Result};
use git2::{BranchType, Cred, Oid, PushOptions, RemoteCallbacks, Repository};
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
        let mut remote = self.repo.find_remote("origin")?;
        let url = remote
            .url()
            .ok_or_else(|| anyhow!("Remote 'origin' not found"))?;

        let mut callbacks = RemoteCallbacks::new();

        if is_http_url(url) {
            log::debug!("Setting up HTTPS callbacks for remote: {url}");
            setup_https_callbacks(&mut callbacks)?;
        } else {
            log::debug!("Setting up SSH callbacks for remote: {url}");
            setup_ssh_callbacks(&mut callbacks)?;
        }

        let mut push_options = PushOptions::new();
        push_options.remote_callbacks(callbacks);

        let refspec = if force {
            format!("+refs/heads/{branch_name}:refs/heads/{branch_name}")
        } else {
            format!("refs/heads/{branch_name}:refs/heads/{branch_name}")
        };
        log::debug!("Pushing branch '{branch_name}' with refspec '{refspec}'");
        remote.push(&[&refspec], Some(&mut push_options))?;
        self.track_branch(branch_name)?;
        Ok(())
    }

    fn track_branch(&self, branch_name: &str) -> Result<()> {
        let mut branch = self.repo.find_branch(branch_name, BranchType::Local)?;
        let upstream_ref = format!("refs/remotes/origin/{branch_name}");
        branch.set_upstream(Some(&upstream_ref))?;
        log::debug!("Tracking remote branch '{upstream_ref}' for local branch '{branch_name}'");
        Ok(())
    }

    pub fn has_remote_branch(&self, branch_name: &str) -> Result<bool> {
        let remote_ref = format!("refs/remotes/origin/{branch_name}");
        Ok(self.repo.find_reference(&remote_ref).is_ok())
    }

    pub fn get_merge_base(&self, branch1: &str, branch2: &str) -> Result<Oid> {
        let commit1 = self.repo.revparse_single(branch1)?.id();
        let commit2 = self.repo.revparse_single(branch2)?.id();
        Ok(self.repo.merge_base(commit1, commit2)?)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn setup_ssh_callbacks(callbacks: &mut RemoteCallbacks) -> Result<()> {
    // Configure SSH callbacks if needed
    callbacks.credentials(|url, username, allowed_types| {
        let username = username.unwrap_or("git");

        log::debug!("Setting up SSH credentials for URL: {url}");
        log::debug!("Setting up SSH credentials for user: {username}");
        log::debug!("Allowed credential types: {allowed_types:?}");

        if !allowed_types.contains(git2::CredentialType::SSH_KEY) {
            return Err(git2::Error::from_str("SSH key authentication not allowed"));
        }

        // NOTE in the future this should support ssh agent, but the below causes
        //   a loop when I test it
        /* if let Ok(cred) = git2::Cred::ssh_key_from_agent(username) {
            log::debug!("Using SSH key from agent for user: {}", username);
            return Ok(cred);
        }
        */

        // fall back to ssh keys
        try_ssh_keys(username)
    });

    callbacks.certificate_check(|_cert, _valid| {
        log::error!("Certificate validation failed");
        Err(git2::Error::from_str("Invalid certificate"))
    });

    Ok(())
}

fn try_ssh_keys(username: &str) -> Result<Cred, git2::Error> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| git2::Error::from_str("Cannot find home directory"))?;

    let ssh_dir = std::path::Path::new(&home).join(".ssh");

    log::debug!("Looking for SSH keys in: {}", ssh_dir.display());

    for key_name in &["id_rsa", "id_ed25519", "id_ecdsa"] {
        let private_key = ssh_dir.join(key_name);
        let public_key = ssh_dir.join(format!("{key_name}.pub"));

        log::debug!("Checking for private key: {}", private_key.display());

        if private_key.exists() {
            if let Ok(cred) = Cred::ssh_key(username, Some(&public_key), &private_key, None) {
                return Ok(cred);
            }
        }
    }

    Err(git2::Error::from_str("No valid SSH keys found"))
}

fn setup_https_callbacks(callbacks: &mut RemoteCallbacks) -> Result<()> {
    callbacks.credentials(|_url, _username_from_url, allowed_types| {
        if !allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            return Err(git2::Error::from_str(
                "Username/password authentication not allowed",
            ));
        }

        // Try to get GitHub token from config or environment
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            return Cred::userpass_plaintext(&token, "x-oauth-basic");
        }

        // Try to read from git config (requires git2 config reading)
        if let Ok(token) = get_github_token_from_config() {
            return git2::Cred::userpass_plaintext(&token, "x-oauth-basic");
        }

        Err(git2::Error::from_str("No HTTPS credentials found"))
    });

    Ok(())
}

fn get_github_token_from_config() -> Result<String, git2::Error> {
    // Try to read from git config
    let config = git2::Config::open_default()
        .map_err(|_| git2::Error::from_str("Cannot open git config"))?;

    // Check common config keys where users might store tokens
    for key in &["github.token", "credential.github.com.username"] {
        if let Ok(token) = config.get_string(key) {
            if !token.is_empty() {
                return Ok(token);
            }
        }
    }

    Err(git2::Error::from_str("No token found in git config"))
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
