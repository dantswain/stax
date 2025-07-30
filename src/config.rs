use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub github_token: Option<String>,
    pub default_base_branch: String,
    pub auto_push: bool,
    pub draft_prs: bool,
    pub pr_template: Option<String>,
    pub user_settings: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            github_token: None,
            default_base_branch: "main".to_string(),
            auto_push: true,
            draft_prs: false,
            pr_template: None,
            user_settings: HashMap::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = get_config_path()?;
        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            let config: Config = serde_json::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let config_path = get_config_path()?;
        
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&config_path, content)?;
        Ok(())
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "github_token" => self.github_token = Some(value.to_string()),
            "default_base_branch" => self.default_base_branch = value.to_string(),
            "auto_push" => self.auto_push = value.parse()?,
            "draft_prs" => self.draft_prs = value.parse()?,
            "pr_template" => self.pr_template = Some(value.to_string()),
            _ => {
                self.user_settings.insert(key.to_string(), value.to_string());
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "github_token" => self.github_token.clone(),
            "default_base_branch" => Some(self.default_base_branch.clone()),
            "auto_push" => Some(self.auto_push.to_string()),
            "draft_prs" => Some(self.draft_prs.to_string()),
            "pr_template" => self.pr_template.clone(),
            _ => self.user_settings.get(key).cloned(),
        }
    }

    pub fn list(&self) -> HashMap<String, String> {
        let mut settings = HashMap::new();
        
        if let Some(token) = &self.github_token {
            settings.insert("github_token".to_string(), mask_token(token));
        }
        settings.insert("default_base_branch".to_string(), self.default_base_branch.clone());
        settings.insert("auto_push".to_string(), self.auto_push.to_string());
        settings.insert("draft_prs".to_string(), self.draft_prs.to_string());
        
        if let Some(template) = &self.pr_template {
            settings.insert("pr_template".to_string(), template.clone());
        }

        for (key, value) in &self.user_settings {
            settings.insert(key.clone(), value.clone());
        }

        settings
    }
}

fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow!("Could not determine config directory"))?;
    Ok(config_dir.join("stax").join("config.json"))
}

fn mask_token(token: &str) -> String {
    if token.len() > 8 {
        format!("{}...{}", &token[..4], &token[token.len()-4..])
    } else {
        "***".to_string()
    }
}

pub mod commands {
    use super::*;
    use crate::utils;

    pub async fn set(key: &str, value: &str) -> Result<()> {
        let mut config = Config::load()?;
        config.set(key, value)?;
        config.save()?;
        utils::print_success(&format!("Set {key} = {}", 
            if key == "github_token" { mask_token(value) } else { value.to_string() }));
        Ok(())
    }

    pub async fn get(key: &str) -> Result<()> {
        let config = Config::load()?;
        if let Some(value) = config.get(key) {
            println!("{value}");
        } else {
            utils::print_error(&format!("Configuration key '{key}' not found"));
        }
        Ok(())
    }

    pub async fn list() -> Result<()> {
        let config = Config::load()?;
        let settings = config.list();
        
        if settings.is_empty() {
            utils::print_info("No configuration set");
            return Ok(());
        }

        println!("Configuration:");
        for (key, value) in settings {
            println!("  {key} = {value}");
        }
        Ok(())
    }
}