use anyhow::Result;
use colored::*;
use std::collections::HashSet;
use crate::config::Config;
use crate::git::GitRepo;
use crate::github::GitHubClient;
use crate::stack::Stack;

pub async fn run() -> Result<()> {
    let git = GitRepo::open(".")?;
    let config = Config::load()?;
    
    let github_client = if let Some(token) = &config.github_token {
        if let Some(remote_url) = git.get_remote_url("origin") {
            Some(GitHubClient::new(token, &remote_url)?)
        } else {
            None
        }
    } else {
        None
    };
    
    let stack = Stack::analyze(&git, github_client.as_ref()).await?;
    
    println!("{}", "Stack Visualization".bold().underline());
    println!();
    
    let mut visited = HashSet::new();
    for root in &stack.roots {
        print_stack_tree(&stack, root, 0, &mut visited);
    }
    
    Ok(())
}

fn print_stack_tree(stack: &Stack, branch_name: &str, depth: usize, visited: &mut HashSet<String>) {
    if visited.contains(branch_name) {
        let indent = "  ".repeat(depth);
        let connector = if depth > 0 { "├─ " } else { "" };
        println!("{}{}[CYCLE DETECTED: {}]", indent, connector, branch_name.red());
        return;
    }
    
    if let Some(branch) = stack.branches.get(branch_name) {
        visited.insert(branch_name.to_string());
        
        let indent = "  ".repeat(depth);
        let connector = if depth > 0 { "├─ " } else { "" };
        
        let mut line = format!("{}{}{}", indent, connector, branch.name);
        
        if branch.is_current {
            line = format!("{} {}", line.green().bold(), "← current".dimmed());
        }
        
        if let Some(pr) = &branch.pull_request {
            let status_symbol = match pr.state.as_str() {
                "open" => "●".green(),
                "draft" => "◐".yellow(),
                "closed" => "○".red(),
                "merged" => "✓".blue(),
                _ => "?".white(),
            };
            
            line = format!("{} {} PR #{}", line, status_symbol, pr.number);
        } else {
            line = format!("{} {}", line, "○".dimmed());
        }
        
        println!("{line}");
        
        for child in &branch.children {
            print_stack_tree(stack, child, depth + 1, visited);
        }
        
        visited.remove(branch_name);
    }
}