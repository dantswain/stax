use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tiny_http::{Server, Response, Header};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AccessTokenRequest {
    client_id: String,
    client_secret: String,
    code: String,
    redirect_uri: String,
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: String,
    _token_type: String,
    _scope: String,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: String,
    _name: Option<String>,
}

pub struct OAuthClient {
    config: OAuthConfig,
    client: Client,
}

impl OAuthClient {
    pub fn new() -> Self {
        let config = OAuthConfig {
            // TODO: These should be actual GitHub OAuth App credentials
            // To set up OAuth authentication, you need to:
            // 1. Create a GitHub OAuth App at https://github.com/settings/applications/new
            // 2. Set "Authorization callback URL" to: http://localhost:8080/callback
            // 3. Replace these placeholder values with your app's credentials
            // 4. For production, these should be loaded from environment variables
            client_id: "your_github_app_client_id".to_string(),
            client_secret: "your_github_app_client_secret".to_string(),
            redirect_uri: "http://localhost:8080/callback".to_string(),
        };

        Self {
            config,
            client: Client::new(),
        }
    }


    pub async fn authenticate(&self) -> Result<String> {
        let state = Uuid::new_v4().to_string();
        let auth_url = self.build_auth_url(&state)?;

        println!("Opening browser for GitHub authentication...");
        println!("If the browser doesn't open automatically, visit: {auth_url}");
        
        // Open browser
        if let Err(e) = webbrowser::open(&auth_url) {
            crate::utils::print_warning(&format!("Failed to open browser: {e}"));
            crate::utils::print_info("Please manually open the URL shown above");
        }

        // Start local server to receive callback
        let code = self.wait_for_callback(&state).await?;
        
        // Exchange code for access token
        let token = self.exchange_code_for_token(&code).await?;
        
        // Verify token works by getting user info
        let user = self.get_user_info(&token).await?;
        crate::utils::print_success(&format!("Successfully authenticated as {}", user.login));
        
        Ok(token)
    }

    fn build_auth_url(&self, state: &str) -> Result<String> {
        let mut url = Url::parse("https://github.com/login/oauth/authorize")?;
        
        url.query_pairs_mut()
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &self.config.redirect_uri)
            .append_pair("scope", "repo,user:email")
            .append_pair("state", state)
            .append_pair("allow_signup", "true");

        Ok(url.to_string())
    }

    async fn wait_for_callback(&self, expected_state: &str) -> Result<String> {
        let server = Server::http("127.0.0.1:8080")
            .map_err(|e| anyhow!("Failed to start callback server: {}", e))?;

        crate::utils::print_info("Waiting for GitHub authentication callback...");

        let (tx, rx) = mpsc::channel();
        let expected_state = expected_state.to_string();

        thread::spawn(move || {
            for request in server.incoming_requests() {
                let url = format!("http://localhost:8080{}", request.url());
                let parsed_url = match Url::parse(&url) {
                    Ok(url) => url,
                    Err(_) => {
                        let _ = request.respond(Response::from_string("Invalid URL").with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"text/plain"[..]).unwrap()
                        ));
                        continue;
                    }
                };

                let query_params: HashMap<String, String> = parsed_url
                    .query_pairs()
                    .into_owned()
                    .collect();

                // Send success/error response to browser
                let response_html = if query_params.contains_key("code") {
                    r#"
                    <!DOCTYPE html>
                    <html>
                    <head><title>Stax Authentication</title></head>
                    <body>
                        <h1>✅ Authentication Successful!</h1>
                        <p>You can close this window and return to your terminal.</p>
                    </body>
                    </html>
                    "#
                } else {
                    r#"
                    <!DOCTYPE html>
                    <html>
                    <head><title>Stax Authentication</title></head>
                    <body>
                        <h1>❌ Authentication Failed</h1>
                        <p>There was an error during authentication. Please try again.</p>
                    </body>
                    </html>
                    "#
                };

                let _ = request.respond(Response::from_string(response_html).with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap()
                ));

                // Validate state parameter
                if let Some(state) = query_params.get("state") {
                    if state != &expected_state {
                        let _ = tx.send(Err(anyhow!("Invalid state parameter")));
                        break;
                    }
                } else {
                    let _ = tx.send(Err(anyhow!("Missing state parameter")));
                    break;
                }

                // Check for authorization code or error
                if let Some(code) = query_params.get("code") {
                    let _ = tx.send(Ok(code.clone()));
                } else if let Some(error) = query_params.get("error") {
                    let error_description = query_params
                        .get("error_description")
                        .map(|s| s.as_str())
                        .unwrap_or("Unknown error");
                    let _ = tx.send(Err(anyhow!("OAuth error: {} - {}", error, error_description)));
                } else {
                    let _ = tx.send(Err(anyhow!("Missing authorization code")));
                }
                break;
            }
        });

        // Wait for callback with timeout
        let result = match rx.recv_timeout(Duration::from_secs(300)) {
            Ok(result) => result,
            Err(_) => return Err(anyhow!("Authentication timed out after 5 minutes")),
        };

        result
    }

    async fn exchange_code_for_token(&self, code: &str) -> Result<String> {
        let request = AccessTokenRequest {
            client_id: self.config.client_id.clone(),
            client_secret: self.config.client_secret.clone(),
            code: code.to_string(),
            redirect_uri: self.config.redirect_uri.clone(),
        };

        let response = self
            .client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .header("User-Agent", "stax-cli")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to exchange code for token: {}", response.status()));
        }

        let token_response: AccessTokenResponse = response.json().await?;
        Ok(token_response.access_token)
    }

    async fn get_user_info(&self, token: &str) -> Result<GitHubUser> {
        let response = self
            .client
            .get("https://api.github.com/user")
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "stax-cli")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to get user info: {}", response.status()));
        }

        let user: GitHubUser = response.json().await?;
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_config_creation() {
        let config = OAuthConfig {
            client_id: "test_client_id".to_string(),
            client_secret: "test_client_secret".to_string(),
            redirect_uri: "http://localhost:8080/callback".to_string(),
        };

        assert_eq!(config.client_id, "test_client_id");
        assert_eq!(config.redirect_uri, "http://localhost:8080/callback");
    }

    #[test]
    fn test_oauth_client_creation() {
        let client = OAuthClient::new();
        assert_eq!(client.config.redirect_uri, "http://localhost:8080/callback");
    }

    #[test]
    fn test_build_auth_url() {
        let client = OAuthClient::new();
        let state = "test_state";
        let auth_url = client.build_auth_url(state).unwrap();
        
        assert!(auth_url.contains("github.com/login/oauth/authorize"));
        assert!(auth_url.contains("state=test_state"));
        assert!(auth_url.contains("scope=repo%2Cuser%3Aemail"));
    }
}