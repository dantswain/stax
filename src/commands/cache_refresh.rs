use crate::cache::{CachedPullRequest, StackCache};
use crate::git::GitRepo;
use crate::github::GitHubClient;
use crate::token_store;
use anyhow::Result;
use std::collections::HashSet;

/// Background cache refresh: fetch open PRs from GitHub and merge into cache.
/// Invoked as a detached child process via `stax cache-refresh`.
pub async fn run() -> Result<()> {
    log::debug!("cache-refresh: starting background PR refresh");

    let git = GitRepo::open(".")?;
    let token = token_store::get_token().ok_or_else(|| anyhow::anyhow!("Not authenticated"))?;
    let remote_url = git
        .get_remote_url("origin")
        .ok_or_else(|| anyhow::anyhow!("No origin remote"))?;
    let github = GitHubClient::new(&token, &remote_url)?;

    let open_prs = github.get_open_pull_requests().await?;
    let incoming: Vec<CachedPullRequest> = open_prs
        .iter()
        .map(|pr| CachedPullRequest {
            number: pr.number,
            state: pr.state.clone(),
            head_ref: pr.head_ref.clone(),
            base_ref: pr.base_ref.clone(),
            html_url: pr.html_url.clone(),
            draft: pr.draft,
        })
        .collect();

    let local_branches: HashSet<String> = git.get_branches()?.into_iter().collect();
    let mut cache = StackCache::new(&git.git_dir());
    cache.merge_pull_requests(&incoming, &local_branches);

    log::debug!("cache-refresh: merged {} PRs into cache", incoming.len());
    Ok(())
}
