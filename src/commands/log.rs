use crate::{logging, utils};
use anyhow::Result;

pub async fn run(follow: bool, lines: usize) -> Result<()> {
    let log_dir = logging::get_log_dir()?;
    let log_path = log_dir.join("stax.log");

    if !log_path.exists() {
        utils::print_info(&format!(
            "Log file: {} (not yet created)",
            log_path.display()
        ));
        utils::print_info("Run a command with -v to generate log output");
        return Ok(());
    }

    utils::print_info(&format!("Log file: {}", log_path.display()));

    if follow {
        let status = std::process::Command::new("tail")
            .args(["-f", &log_path.to_string_lossy()])
            .status()?;
        if !status.success() {
            utils::print_error("Failed to tail log file");
        }
    } else {
        let output = std::process::Command::new("tail")
            .args(["-n", &lines.to_string(), &log_path.to_string_lossy()])
            .output()?;
        if output.status.success() {
            let content = String::from_utf8_lossy(&output.stdout);
            if content.is_empty() {
                utils::print_info("Log file is empty");
            } else {
                print!("{content}");
            }
        } else {
            utils::print_error("Failed to read log file");
        }
    }

    Ok(())
}
