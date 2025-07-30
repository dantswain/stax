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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    let path = parsed.path().trim_start_matches('/').trim_end_matches(".git");
    let parts: Vec<&str> = path.split('/').collect();
    
    if parts.len() != 2 {
        return Err(anyhow!("Invalid GitHub repository URL format"));
    }

    Ok((parts[0].to_string(), parts[1].to_string()))
}