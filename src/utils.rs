use anyhow::Result;
use colored::*;
use std::io::{self, Write};

pub fn print_success(msg: &str) {
    println!("{} {}", "✓".green().bold(), msg);
}

pub fn print_error(msg: &str) {
    eprintln!("{} {}", "✗".red().bold(), msg);
}

pub fn print_info(msg: &str) {
    println!("{} {}", "ℹ".blue().bold(), msg);
}

pub fn print_warning(msg: &str) {
    println!("{} {}", "⚠".yellow().bold(), msg);
}

pub fn confirm(msg: &str) -> Result<bool> {
    print!("{} {} (y/N): ", "?".cyan().bold(), msg);
    io::stdout().flush()?;
    
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    
    Ok(input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes")
}

pub fn prompt(msg: &str) -> Result<String> {
    print!("{} {}: ", "?".cyan().bold(), msg);
    io::stdout().flush()?;
    
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    
    Ok(input.trim().to_string())
}

#[allow(dead_code)]
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_string_short() {
        let input = "short";
        let result = truncate_string(input, 10);
        assert_eq!(result, "short");
    }

    #[test]
    fn test_truncate_string_exact() {
        let input = "exactly10c";
        let result = truncate_string(input, 10);
        assert_eq!(result, "exactly10c");
    }

    #[test]
    fn test_truncate_string_long() {
        let input = "this is a very long string that should be truncated";
        let result = truncate_string(input, 10);
        assert_eq!(result, "this is...");
    }

    #[test]
    fn test_truncate_string_edge_case() {
        let input = "abc";
        let result = truncate_string(input, 3);
        assert_eq!(result, "abc");
    }

    #[test]
    fn test_truncate_string_very_short_limit() {
        let input = "hello world";
        let result = truncate_string(input, 2);
        assert_eq!(result, "...");
    }
}