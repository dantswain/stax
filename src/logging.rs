use anyhow::Result;
use log::LevelFilter;
use std::fs;
use std::path::PathBuf;

/// Maximum log file size before rotation (5 MB).
const MAX_LOG_SIZE: u64 = 5 * 1024 * 1024;

/// Determine the platform-appropriate log directory.
pub fn get_log_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            return Ok(home.join("Library").join("Logs").join("stax"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(state_home) = std::env::var("XDG_STATE_HOME") {
            return Ok(PathBuf::from(state_home).join("stax").join("logs"));
        }
        if let Some(home) = dirs::home_dir() {
            return Ok(home.join(".local").join("state").join("stax").join("logs"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local_data) = dirs::data_local_dir() {
            return Ok(local_data.join("stax").join("logs"));
        }
    }

    // Fallback for any platform
    if let Some(home) = dirs::home_dir() {
        return Ok(home.join(".stax").join("logs"));
    }

    Err(anyhow::anyhow!("Could not determine log directory"))
}

/// Parse a log level string into a LevelFilter.
pub fn parse_level(level: &str) -> LevelFilter {
    match level.to_lowercase().as_str() {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Error,
    }
}

/// Rotate the log file if it exceeds MAX_LOG_SIZE.
fn rotate_log(log_path: &PathBuf) {
    if let Ok(metadata) = fs::metadata(log_path) {
        if metadata.len() > MAX_LOG_SIZE {
            let rotated = log_path.with_extension("log.1");
            let _ = fs::rename(log_path, rotated);
        }
    }
}

/// Initialize file-based logging with fern.
///
/// `level` is the effective log level (already resolved from CLI/env/config).
/// Logging failures are non-fatal — the caller should warn to stderr and continue.
pub fn init(level: LevelFilter) -> Result<()> {
    if level == LevelFilter::Off {
        return Ok(());
    }

    let log_dir = get_log_dir()?;
    fs::create_dir_all(&log_dir)?;

    let log_path = log_dir.join("stax.log");

    rotate_log(&log_path);

    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} [{}] {} - {}",
                chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(level)
        // Suppress noisy dependencies even at debug/trace level
        .level_for("hyper", LevelFilter::Warn)
        .level_for("hyper_util", LevelFilter::Warn)
        .level_for("reqwest", LevelFilter::Warn)
        .level_for("octocrab", LevelFilter::Warn)
        .level_for("h2", LevelFilter::Warn)
        .level_for("rustls", LevelFilter::Warn)
        .level_for("tower", LevelFilter::Warn)
        .chain(log_file)
        .apply()
        .map_err(|e| anyhow::anyhow!("Failed to initialize logging: {}", e))?;

    log::debug!(
        "Logging initialized at level {:?}, writing to {}",
        level,
        log_path.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_log_dir_returns_path_with_stax() {
        let dir = get_log_dir().unwrap();
        assert!(dir.to_string_lossy().contains("stax"));
    }

    #[test]
    fn test_parse_level_valid() {
        assert_eq!(parse_level("error"), LevelFilter::Error);
        assert_eq!(parse_level("warn"), LevelFilter::Warn);
        assert_eq!(parse_level("info"), LevelFilter::Info);
        assert_eq!(parse_level("debug"), LevelFilter::Debug);
        assert_eq!(parse_level("trace"), LevelFilter::Trace);
    }

    #[test]
    fn test_parse_level_case_insensitive() {
        assert_eq!(parse_level("DEBUG"), LevelFilter::Debug);
        assert_eq!(parse_level("Error"), LevelFilter::Error);
    }

    #[test]
    fn test_parse_level_invalid_defaults_to_error() {
        assert_eq!(parse_level("invalid"), LevelFilter::Error);
        assert_eq!(parse_level(""), LevelFilter::Error);
    }
}
