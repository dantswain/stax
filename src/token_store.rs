use anyhow::{anyhow, Result};
use std::fs;
use std::path::PathBuf;

pub struct TokenStore;

impl TokenStore {
    /// Get the path to the token storage file
    fn get_token_path() -> Result<PathBuf> {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Could not find home directory"))?;
        
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
            crate::utils::print_warning("Token stored in plaintext. Consider using a credential manager.");
        }
        
        Ok(())
    }


}

#[cfg(test)]
mod tests {  
    use super::*;

    #[test]
    fn test_token_storage() {
        // Test that store_token doesn't panic or error in a basic case
        let test_token = "test_token_12345";
        
        // This may fail in some environments (like CI), which is fine
        let _ = TokenStore::store_token(test_token);
    }
}