use std::path::PathBuf;
use tracing::warn;

use super::schema::Config;

/// Get the default configuration file path.
pub fn get_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nanobot")
        .join("config.json")
}

/// Get the nanobot data directory (~/.nanobot).
pub fn get_data_dir() -> PathBuf {
    let dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nanobot");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Load configuration from file or create default.
pub fn load_config() -> Config {
    load_config_from(None)
}

/// Load configuration from a specific path.
pub fn load_config_from(config_path: Option<PathBuf>) -> Config {
    let path = config_path.unwrap_or_else(get_config_path);

    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<Config>(&content) {
                Ok(config) => return config,
                Err(e) => {
                    warn!("Failed to parse config from {}: {}", path.display(), e);
                    warn!("Using default configuration.");
                }
            },
            Err(e) => {
                warn!("Failed to read config from {}: {}", path.display(), e);
                warn!("Using default configuration.");
            }
        }
    }

    Config::default()
}

/// Save configuration to file.
pub fn save_config(config: &Config) -> std::io::Result<()> {
    save_config_to(config, None)
}

/// Save configuration to a specific path.
pub fn save_config_to(config: &Config, config_path: Option<PathBuf>) -> std::io::Result<()> {
    let path = config_path.unwrap_or_else(get_config_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(path, json)
}
