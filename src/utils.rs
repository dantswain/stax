use anyhow::{anyhow, Result};
use colored::*;
use console::{Key, Term};
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

/// Single-line text input that supports Ctrl+G to open $EDITOR.
/// Returns `Ok(None)` if the user pressed Ctrl+G (caller should open editor).
/// Returns `Ok(Some(text))` for normal text entry.
pub fn input_or_editor(prompt_msg: &str) -> Result<Option<String>> {
    let term = Term::stderr();
    let mut buf = String::new();

    eprint!(
        "{} {} {}: ",
        "?".cyan().bold(),
        prompt_msg,
        "(Ctrl+G for editor)".dimmed()
    );
    io::stderr().flush()?;

    loop {
        let key = term.read_key()?;
        match key {
            Key::Char('\x07') => {
                // Ctrl+G — clear the line and signal editor
                term.clear_line()?;
                eprintln!("{} {} opening editor...", "?".cyan().bold(), prompt_msg,);
                return Ok(None);
            }
            Key::Enter => {
                term.write_line("")?;
                return Ok(Some(buf));
            }
            Key::Backspace => {
                if !buf.is_empty() {
                    buf.pop();
                    // Move cursor back, overwrite with space, move back again
                    eprint!("\x08 \x08");
                    io::stderr().flush()?;
                }
            }
            Key::Char(c) => {
                buf.push(c);
                eprint!("{c}");
                io::stderr().flush()?;
            }
            _ => {}
        }
    }
}

/// Open $VISUAL / $EDITOR (with shell expansion) to edit `initial` text.
/// Returns the edited text, or an error if no editor is available.
pub fn open_editor(initial: &str) -> Result<String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    let mut tmp = tempfile::Builder::new()
        .prefix("stax-")
        .suffix(".md")
        .tempfile()?;
    tmp.write_all(initial.as_bytes())?;
    tmp.flush()?;

    let path = tmp.path().to_owned();

    // Invoke through shell so env vars like $HOME get expanded
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{} \"{}\"", editor, path.display()))
        .status()?;

    if !status.success() {
        return Err(anyhow!("Editor exited with non-zero status"));
    }

    Ok(std::fs::read_to_string(&path)?
        .trim_end_matches('\n')
        .to_string())
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
