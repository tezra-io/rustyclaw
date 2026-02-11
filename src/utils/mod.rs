use std::path::PathBuf;

/// Get the rustyclaw data directory (~/.rustyclaw).
pub fn get_data_path() -> PathBuf {
    crate::config::loader::get_data_dir()
}

/// Get the workspace path from config.
pub fn get_workspace_path(config: &crate::config::schema::Config) -> PathBuf {
    config.workspace_path()
}

/// Get today's date as YYYY-MM-DD.
pub fn today_date() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Create a safe filename from arbitrary text.
pub fn safe_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Ensure a directory exists, creating it if necessary.
pub fn ensure_dir(path: &PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Parse a session key into (channel, chat_id).
pub fn parse_session_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(':')
}
