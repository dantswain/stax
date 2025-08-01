use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Serialize)]
struct DeviceCodeRequest {
    client_id: String,
    scope: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Serialize)]
struct AccessTokenRequest {
    client_id: String,
    device_code: String,
    grant_type: String,
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: String,
    _name: Option<String>,
}

pub struct OAuthClient {
    client_id: String,
    client: Client,
}

impl OAuthClient {
    pub fn new() -> Self {
        Self {
            // GitHub CLI client ID - this is a public client ID used by GitHub CLI
            // and is safe to embed in applications
            client_id: "178c6fc778ccc68e1d6a".to_string(),
            client: Client::new(),
        }
    }

    pub async fn authenticate(&self) -> Result<String> {
        // Step 1: Get device code
        crate::utils::print_info("Requesting device authorization from GitHub...");
        let device_response = self.request_device_code().await?;

        // Step 2: Show user instructions
        println!();
        crate::utils::print_info("To authenticate with GitHub:");
        println!("  1. Visit: {}", device_response.verification_uri);
        println!("  2. Enter code: {}", device_response.user_code);
        println!();

        crate::utils::print_info("Opening browser automatically...");
        if let Err(e) = webbrowser::open(&device_response.verification_uri) {
            crate::utils::print_warning(&format!("Failed to open browser: {e}"));
            crate::utils::print_info("Please manually visit the URL above");
        }

        // Step 3: Poll for access token
        crate::utils::print_info("Waiting for authorization...");
        let token = self.poll_for_token(&device_response).await?;

        // Step 4: Verify token works
        let user = self.get_user_info(&token).await?;
        crate::utils::print_success(&format!("Successfully authenticated as {}", user.login));

        Ok(token)
    }

    async fn request_device_code(&self) -> Result<DeviceCodeResponse> {
        let request = DeviceCodeRequest {
            client_id: self.client_id.clone(),
            scope: "repo,user:email".to_string(),
        };

        let response = self
            .client
            .post("https://github.com/login/device/code")
            .header("Accept", "application/json")
            .header("User-Agent", "stax-cli")
            .form(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to request device code: {}",
                response.status()
            ));
        }

        let device_response: DeviceCodeResponse = response.json().await?;
        Ok(device_response)
    }

    async fn poll_for_token(&self, device_response: &DeviceCodeResponse) -> Result<String> {
        let request = AccessTokenRequest {
            client_id: self.client_id.clone(),
            device_code: device_response.device_code.clone(),
            grant_type: "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        };

        let poll_interval = Duration::from_secs(device_response.interval.max(5));
        let timeout = Duration::from_secs(device_response.expires_in);
        let start_time = std::time::Instant::now();

        loop {
            if start_time.elapsed() > timeout {
                return Err(anyhow!("Device authorization expired. Please try again."));
            }

            tokio::time::sleep(poll_interval).await;

            let response = self
                .client
                .post("https://github.com/login/oauth/access_token")
                .header("Accept", "application/json")
                .header("User-Agent", "stax-cli")
                .form(&request)
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(anyhow!(
                    "Failed to poll for access token: {}",
                    response.status()
                ));
            }

            let token_response: AccessTokenResponse = response.json().await?;

            if let Some(access_token) = token_response.access_token {
                return Ok(access_token);
            }

            if let Some(error) = &token_response.error {
                match error.as_str() {
                    "authorization_pending" => {
                        // Continue polling
                        continue;
                    }
                    "slow_down" => {
                        // Wait longer before next poll
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                    "expired_token" => {
                        return Err(anyhow!("Device authorization expired. Please try again."));
                    }
                    "access_denied" => {
                        return Err(anyhow!("Authorization was denied."));
                    }
                    _ => {
                        let description = token_response
                            .error_description
                            .as_deref()
                            .unwrap_or("Unknown error");
                        return Err(anyhow!("Authorization failed: {} - {}", error, description));
                    }
                }
            }
        }
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
    fn test_oauth_client_creation() {
        let client = OAuthClient::new();
        assert_eq!(client.client_id, "178c6fc778ccc68e1d6a");
    }

    #[test]
    fn test_device_code_request_structure() {
        let request = DeviceCodeRequest {
            client_id: "test_client".to_string(),
            scope: "repo".to_string(),
        };

        assert_eq!(request.client_id, "test_client");
        assert_eq!(request.scope, "repo");
    }

    #[test]
    fn test_access_token_request_structure() {
        let request = AccessTokenRequest {
            client_id: "test_client".to_string(),
            device_code: "test_code".to_string(),
            grant_type: "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        };

        assert_eq!(request.client_id, "test_client");
        assert_eq!(request.device_code, "test_code");
        assert_eq!(
            request.grant_type,
            "urn:ietf:params:oauth:grant-type:device_code"
        );
    }
}
