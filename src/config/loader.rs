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

    let mut config = if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<Config>(&content) {
                Ok(config) => config,
                Err(e) => {
                    warn!("Failed to parse config from {}: {}", path.display(), e);
                    warn!("Using default configuration.");
                    Config::default()
                }
            },
            Err(e) => {
                warn!("Failed to read config from {}: {}", path.display(), e);
                warn!("Using default configuration.");
                Config::default()
            }
        }
    } else {
        Config::default()
    };

    apply_env_overrides(&mut config);
    config
}

/// Apply environment variable overrides to the config.
///
/// Supported env vars (matching nanobot Python conventions):
///   OPENROUTER_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY, DEEPSEEK_API_KEY,
///   GROQ_API_KEY, GEMINI_API_KEY, ZHIPU_API_KEY, DASHSCOPE_API_KEY,
///   MOONSHOT_API_KEY, MINIMAX_API_KEY, AIHUBMIX_API_KEY, VLLM_API_KEY
///   TELEGRAM_BOT_TOKEN, DISCORD_BOT_TOKEN
///   NANOBOT_MODEL, NANOBOT_WORKSPACE
///   BRAVE_SEARCH_API_KEY
fn apply_env_overrides(config: &mut Config) {
    // Provider API keys
    if let Ok(v) = std::env::var("OPENROUTER_API_KEY") {
        config.providers.openrouter.api_key = v;
    }
    if let Ok(v) = std::env::var("ANTHROPIC_API_KEY") {
        config.providers.anthropic.api_key = v;
    }
    if let Ok(v) = std::env::var("OPENAI_API_KEY") {
        config.providers.openai.api_key = v;
    }
    if let Ok(v) = std::env::var("DEEPSEEK_API_KEY") {
        config.providers.deepseek.api_key = v;
    }
    if let Ok(v) = std::env::var("GROQ_API_KEY") {
        config.providers.groq.api_key = v;
    }
    if let Ok(v) = std::env::var("GEMINI_API_KEY") {
        config.providers.gemini.api_key = v;
    }
    if let Ok(v) = std::env::var("ZHIPU_API_KEY") {
        config.providers.zhipu.api_key = v;
    }
    if let Ok(v) = std::env::var("DASHSCOPE_API_KEY") {
        config.providers.dashscope.api_key = v;
    }
    if let Ok(v) = std::env::var("MOONSHOT_API_KEY") {
        config.providers.moonshot.api_key = v;
    }
    if let Ok(v) = std::env::var("MINIMAX_API_KEY") {
        config.providers.minimax.api_key = v;
    }
    if let Ok(v) = std::env::var("AIHUBMIX_API_KEY") {
        config.providers.aihubmix.api_key = v;
    }
    if let Ok(v) = std::env::var("VLLM_API_KEY") {
        config.providers.vllm.api_key = v;
    }

    // Channel tokens
    if let Ok(v) = std::env::var("TELEGRAM_BOT_TOKEN") {
        config.channels.telegram.token = v;
    }
    if let Ok(v) = std::env::var("DISCORD_BOT_TOKEN") {
        config.channels.discord.token = v;
    }

    // Agent defaults
    if let Ok(v) = std::env::var("NANOBOT_MODEL") {
        config.agents.defaults.model = v;
    }
    if let Ok(v) = std::env::var("NANOBOT_WORKSPACE") {
        config.agents.defaults.workspace = v;
    }

    // Web search
    if let Ok(v) = std::env::var("BRAVE_SEARCH_API_KEY") {
        config.tools.web.search.api_key = v;
    }
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
