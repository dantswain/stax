use anyhow::{anyhow, Result};
use octocrab::Octocrab;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub head_ref: String,
    pub base_ref: String,
    pub html_url: String,
    pub draft: bool,
}

#[derive(Clone)]
pub struct GitHubClient {
    octocrab: Octocrab,
    owner: String,
    repo: String,
}

impl GitHubClient {
    pub fn new(token: &str, repo_url: &str) -> Result<Self> {
        let octocrab = Octocrab::builder()
            .personal_token(token.to_string())
            .build()?;

        let (owner, repo) = parse_github_url(repo_url)?;

        Ok(GitHubClient {
            octocrab,
            owner,
            repo,
        })
    }

    pub async fn get_authenticated_user(&self) -> Result<String> {
        let user = self.octocrab.current().user().await?;
        Ok(user.login)
    }

    pub async fn authenticate_with_oauth() -> Result<String> {
        let oauth_client = crate::oauth::OAuthClient::new();
        oauth_client.authenticate().await
    }

    /// Fetch the first page of open PRs (up to 100). Sufficient for most repos.
    pub async fn get_open_pull_requests(&self) -> Result<Vec<PullRequest>> {
        let page = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .list()
            .state(octocrab::params::State::Open)
            .per_page(100)
            .send()
            .await?;

        Ok(page
            .into_iter()
            .map(|pr| PullRequest {
                number: pr.number,
                title: pr.title.unwrap_or_default(),
                body: pr.body,
                state: if pr.merged_at.is_some() {
                    "merged".to_string()
                } else {
                    format!("{:?}", pr.state.unwrap()).to_lowercase()
                },
                head_ref: pr.head.ref_field,
                base_ref: pr.base.ref_field,
                html_url: pr.html_url.unwrap().to_string(),
                draft: pr.draft.unwrap_or(false),
            })
            .collect())
    }

    /// Fetch the most recent PR for a specific branch using the API's `head` filter.
    /// Prefers open PRs over closed/merged ones.
    pub async fn get_pr_for_branch(&self, branch: &str) -> Result<Option<PullRequest>> {
        let page = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .list()
            .state(octocrab::params::State::All)
            .head(format!("{}:{}", self.owner, branch))
            .per_page(5)
            .send()
            .await?;

        let mut prs: Vec<PullRequest> = page
            .into_iter()
            .map(|pr| {
                let state = if pr.merged_at.is_some() {
                    "merged".to_string()
                } else {
                    format!("{:?}", pr.state.unwrap()).to_lowercase()
                };
                PullRequest {
                    number: pr.number,
                    title: pr.title.unwrap_or_default(),
                    body: pr.body,
                    state,
                    head_ref: pr.head.ref_field,
                    base_ref: pr.base.ref_field,
                    html_url: pr.html_url.unwrap().to_string(),
                    draft: pr.draft.unwrap_or(false),
                }
            })
            .collect();

        // Prefer open PR if one exists, otherwise return the most recent
        if let Some(idx) = prs.iter().position(|pr| pr.state == "open") {
            Ok(Some(prs.swap_remove(idx)))
        } else {
            Ok(prs.into_iter().next())
        }
    }

    pub async fn create_pull_request(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        draft: bool,
    ) -> Result<PullRequest> {
        let pr = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .create(title, head, base)
            .body(body)
            .draft(draft)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to create PR: {}", format_github_error(e)))?;

        Ok(PullRequest {
            number: pr.number,
            title: pr.title.unwrap_or_default(),
            body: pr.body,
            state: format!("{:?}", pr.state.unwrap()).to_lowercase(),
            head_ref: pr.head.ref_field,
            base_ref: pr.base.ref_field,
            html_url: pr.html_url.unwrap().to_string(),
            draft: pr.draft.unwrap_or(false),
        })
    }

    pub async fn update_pull_request(
        &self,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        base: Option<&str>,
    ) -> Result<PullRequest> {
        let pulls = self.octocrab.pulls(&self.owner, &self.repo);
        let mut update = pulls.update(number);

        if let Some(title) = title {
            update = update.title(title);
        }
        if let Some(body) = body {
            update = update.body(body);
        }
        if let Some(base) = base {
            update = update.base(base);
        }

        let pr = update
            .send()
            .await
            .map_err(|e| anyhow!("Failed to update PR: {}", format_github_error(e)))?;

        Ok(PullRequest {
            number: pr.number,
            title: pr.title.unwrap_or_default(),
            body: pr.body,
            state: format!("{:?}", pr.state.unwrap()).to_lowercase(),
            head_ref: pr.head.ref_field,
            base_ref: pr.base.ref_field,
            html_url: pr.html_url.unwrap().to_string(),
            draft: pr.draft.unwrap_or(false),
        })
    }

    #[allow(dead_code)]
    pub async fn close_pull_request(&self, number: u64) -> Result<()> {
        self.octocrab
            .pulls(&self.owner, &self.repo)
            .update(number)
            .state(octocrab::params::pulls::State::Closed)
            .send()
            .await?;
        Ok(())
    }

    /// List comments on a PR (issue comments). Returns vec of (comment_id, body).
    pub async fn list_pr_comments(&self, pr_number: u64) -> Result<Vec<(u64, String)>> {
        let page = self
            .octocrab
            .issues(&self.owner, &self.repo)
            .list_comments(pr_number)
            .per_page(100)
            .send()
            .await?;

        Ok(page
            .items
            .into_iter()
            .map(|c| (c.id.into_inner(), c.body.unwrap_or_default()))
            .collect())
    }

    /// Create a comment on a PR.
    pub async fn create_pr_comment(&self, pr_number: u64, body: &str) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .create_comment(pr_number, body)
            .await?;
        Ok(())
    }

    /// Update an existing PR comment by comment ID.
    pub async fn update_pr_comment(&self, comment_id: u64, body: &str) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .update_comment(comment_id.into(), body)
            .await?;
        Ok(())
    }
}

/// Extract a useful error message from octocrab errors.
/// Octocrab's `Error::GitHub` Display just prints "GitHub" without the message.
fn format_github_error(err: octocrab::Error) -> String {
    match err {
        octocrab::Error::GitHub { source, .. } => source.to_string(),
        other => other.to_string(),
    }
}

fn parse_github_url(url: &str) -> Result<(String, String)> {
    let parsed = Url::parse(url).or_else(|_| {
        if url.starts_with("git@github.com:") {
            let ssh_part = url.strip_prefix("git@github.com:").unwrap();
            let repo_part = ssh_part.strip_suffix(".git").unwrap_or(ssh_part);
            let full_url = format!("https://github.com/{repo_part}");
            Url::parse(&full_url)
        } else {
            Err(url::ParseError::RelativeUrlWithoutBase)
        }
    })?;

    if parsed.host_str() != Some("github.com") {
        return Err(anyhow!("Not a GitHub URL"));
    }

    let path = parsed
        .path()
        .trim_start_matches('/')
        .trim_end_matches(".git");
    let parts: Vec<&str> = path.split('/').collect();

    if parts.len() != 2 {
        return Err(anyhow!("Invalid GitHub repository URL format"));
    }

    Ok((parts[0].to_string(), parts[1].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_url_https() {
        let result = parse_github_url("https://github.com/user/repo.git");
        assert!(result.is_ok());
        let (owner, repo) = result.unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_https_no_git() {
        let result = parse_github_url("https://github.com/user/repo");
        assert!(result.is_ok());
        let (owner, repo) = result.unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_ssh() {
        let result = parse_github_url("git@github.com:user/repo.git");
        assert!(result.is_ok());
        let (owner, repo) = result.unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_ssh_no_git() {
        let result = parse_github_url("git@github.com:user/repo");
        assert!(result.is_ok());
        let (owner, repo) = result.unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_invalid_host() {
        let result = parse_github_url("https://gitlab.com/user/repo.git");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_github_url_invalid_format() {
        let result = parse_github_url("https://github.com/user");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_github_url_invalid_ssh() {
        let result = parse_github_url("git@gitlab.com:user/repo.git");
        assert!(result.is_err());
    }

    #[test]
    fn test_pull_request_struct() {
        let pr = PullRequest {
            number: 42,
            title: "Test PR".to_string(),
            body: Some("Test body".to_string()),
            state: "open".to_string(),
            head_ref: "feature-branch".to_string(),
            base_ref: "main".to_string(),
            html_url: "https://github.com/user/repo/pull/42".to_string(),
            draft: false,
        };

        assert_eq!(pr.number, 42);
        assert_eq!(pr.title, "Test PR");
        assert_eq!(pr.body, Some("Test body".to_string()));
        assert_eq!(pr.state, "open");
        assert_eq!(pr.head_ref, "feature-branch");
        assert_eq!(pr.base_ref, "main");
        assert!(!pr.draft);
    }

    #[test]
    fn test_pull_request_serialization() {
        let pr = PullRequest {
            number: 42,
            title: "Test PR".to_string(),
            body: None,
            state: "draft".to_string(),
            head_ref: "feature".to_string(),
            base_ref: "main".to_string(),
            html_url: "https://github.com/user/repo/pull/42".to_string(),
            draft: true,
        };

        // Test that it can be serialized to JSON (for debugging/logging)
        let json = serde_json::to_string(&pr);
        assert!(json.is_ok());

        // Test that it can be deserialized back
        let json_str = json.unwrap();
        let deserialized: Result<PullRequest, _> = serde_json::from_str(&json_str);
        assert!(deserialized.is_ok());

        let pr2 = deserialized.unwrap();
        assert_eq!(pr.number, pr2.number);
        assert_eq!(pr.title, pr2.title);
        assert_eq!(pr.draft, pr2.draft);
    }
}
