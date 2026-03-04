use anyhow::{anyhow, Result};
use std::fs;
use std::path::PathBuf;

pub struct TokenStore;

impl TokenStore {
    /// Get the path to the token storage file
    fn get_token_path() -> Result<PathBuf> {
        let home_dir = dirs::home_dir().ok_or_else(|| anyhow!("Could not find home directory"))?;

        let stax_dir = home_dir.join(".stax");

        // Create .stax directory if it doesn't exist
        if !stax_dir.exists() {
            fs::create_dir_all(&stax_dir)?;

            // Set restrictive permissions on Unix systems
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&stax_dir)?.permissions();
                perms.set_mode(0o700); // Only owner can read/write/execute
                fs::set_permissions(&stax_dir, perms)?;
            }
        }

        Ok(stax_dir.join("token"))
    }

    /// Store a GitHub token securely
    pub fn store_token(token: &str) -> Result<()> {
        let token_path = Self::get_token_path()?;
        log::debug!("Storing token to {}", token_path.display());

        // Write token to file
        fs::write(&token_path, token)?;

        // Set restrictive permissions on the token file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&token_path)?.permissions();
            perms.set_mode(0o600); // Only owner can read/write
            fs::set_permissions(&token_path, perms)?;
        }

        #[cfg(windows)]
        {
            // On Windows, we should ideally use Windows Data Protection API (DPAPI)
            // For now, we'll just warn about the security implications
            crate::utils::print_warning(
                "Token stored in plaintext. Consider using a credential manager.",
            );
        }

        Ok(())
    }

    /// Retrieve a stored GitHub token
    pub fn get_token() -> Option<String> {
        let token_path = Self::get_token_path().ok()?;

        if token_path.exists() {
            log::debug!("Token found at {}", token_path.display());
            fs::read_to_string(token_path)
                .ok()
                .map(|s| s.trim().to_string())
        } else {
            log::debug!("No token file found at {}", token_path.display());
            None
        }
    }
}

/// Convenience function to get the stored token
pub fn get_token() -> Option<String> {
    TokenStore::get_token()
}
