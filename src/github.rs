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

    pub async fn authenticate_with_oauth() -> Result<String> {
        let oauth_client = crate::oauth::OAuthClient::new();
        oauth_client.authenticate().await
    }

    pub async fn get_pull_requests(&self) -> Result<Vec<PullRequest>> {
        let page = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .list()
            .state(octocrab::params::State::All)
            .per_page(100)
            .send()
            .await?;

        let mut prs = Vec::new();
        for pr in page {
            prs.push(PullRequest {
                number: pr.number,
                title: pr.title.unwrap_or_default(),
                body: pr.body,
                state: format!("{:?}", pr.state.unwrap()),
                head_ref: pr.head.ref_field,
                base_ref: pr.base.ref_field,
                html_url: pr.html_url.unwrap().to_string(),
                draft: pr.draft.unwrap_or(false),
            });
        }

        Ok(prs)
    }

    #[allow(dead_code)]
    pub async fn get_pull_request_by_branch(&self, branch: &str) -> Result<Option<PullRequest>> {
        let prs = self.get_pull_requests().await?;
        Ok(prs.into_iter().find(|pr| pr.head_ref == branch))
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
            .await?;

        Ok(PullRequest {
            number: pr.number,
            title: pr.title.unwrap_or_default(),
            body: pr.body,
            state: format!("{:?}", pr.state.unwrap()),
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

        let pr = update.send().await?;

        Ok(PullRequest {
            number: pr.number,
            title: pr.title.unwrap_or_default(),
            body: pr.body,
            state: format!("{:?}", pr.state.unwrap()),
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
