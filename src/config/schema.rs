use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::providers::registry::{find_provider_by_name, PROVIDERS};

/// Root configuration for rustyclaw.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub agents: AgentsConfig,
    pub channels: ChannelsConfig,
    pub providers: ProvidersConfig,
    pub gateway: GatewayConfig,
    pub tools: ToolsConfig,
}

impl Config {
    /// Get expanded workspace path.
    pub fn workspace_path(&self) -> PathBuf {
        let path = shellexpand::tilde(&self.agents.defaults.workspace);
        PathBuf::from(path.as_ref())
    }

    /// Match a provider config by model name. Returns (ProviderConfig, spec_name).
    fn match_provider(
        &self,
        model: Option<&str>,
    ) -> (Option<&ProviderConfig>, Option<&'static str>) {
        let model_str = model.unwrap_or(&self.agents.defaults.model);
        let model_lower = model_str.to_lowercase();

        // Match by keyword (order follows PROVIDERS registry)
        for spec in PROVIDERS {
            if let Some(p) = self.providers.by_name(spec.name) {
                if spec.keywords.iter().any(|kw| model_lower.contains(kw)) && !p.api_key.is_empty()
                {
                    return (Some(p), Some(spec.name));
                }
            }
        }

        // Fallback: gateways first, then others
        for spec in PROVIDERS {
            if let Some(p) = self.providers.by_name(spec.name) {
                if !p.api_key.is_empty() {
                    return (Some(p), Some(spec.name));
                }
            }
        }

        (None, None)
    }

    /// Get matched provider config for a model.
    pub fn get_provider(&self, model: Option<&str>) -> Option<&ProviderConfig> {
        self.match_provider(model).0
    }

    /// Get the registry name of the matched provider.
    pub fn get_provider_name(&self, model: Option<&str>) -> Option<&'static str> {
        self.match_provider(model).1
    }

    /// Get API base URL for the given model.
    pub fn get_api_base(&self, model: Option<&str>) -> Option<String> {
        let (p, name) = self.match_provider(model);
        if let Some(p) = p {
            if let Some(base) = &p.api_base {
                return Some(base.clone());
            }
        }
        // Gateways get a default api_base from the registry
        if let Some(name) = name {
            if let Some(spec) = find_provider_by_name(name) {
                if spec.is_gateway && !spec.api_base.is_empty() {
                    return Some(spec.api_base.to_string());
                }
            }
        }
        None
    }
}

// --- Agent Config ---

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentsConfig {
    pub defaults: AgentDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentDefaults {
    pub workspace: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f64,
    pub max_tool_iterations: u32,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: "~/.nanobot/workspace".to_string(),
            model: "anthropic/claude-sonnet-4-5".to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            max_tool_iterations: 20,
        }
    }
}

// --- Provider Config ---

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderConfig {
    pub api_key: String,
    pub api_base: Option<String>,
    pub extra_headers: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProvidersConfig {
    pub anthropic: ProviderConfig,
    pub openai: ProviderConfig,
    pub openrouter: ProviderConfig,
    pub deepseek: ProviderConfig,
    pub groq: ProviderConfig,
    pub zhipu: ProviderConfig,
    pub dashscope: ProviderConfig,
    pub vllm: ProviderConfig,
    pub gemini: ProviderConfig,
    pub moonshot: ProviderConfig,
    pub minimax: ProviderConfig,
    pub aihubmix: ProviderConfig,
}

impl ProvidersConfig {
    /// Get a provider config by registry name.
    pub fn by_name(&self, name: &str) -> Option<&ProviderConfig> {
        match name {
            "anthropic" => Some(&self.anthropic),
            "openai" => Some(&self.openai),
            "openrouter" => Some(&self.openrouter),
            "deepseek" => Some(&self.deepseek),
            "groq" => Some(&self.groq),
            "zhipu" => Some(&self.zhipu),
            "dashscope" => Some(&self.dashscope),
            "vllm" => Some(&self.vllm),
            "gemini" => Some(&self.gemini),
            "moonshot" => Some(&self.moonshot),
            "minimax" => Some(&self.minimax),
            "aihubmix" => Some(&self.aihubmix),
            _ => None,
        }
    }
}

// --- Channel Config ---

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelsConfig {
    pub telegram: TelegramConfig,
    pub discord: DiscordConfig,
    pub whatsapp: WhatsAppConfig,
    pub feishu: FeishuConfig,
    pub dingtalk: DingTalkConfig,
    pub mochat: MochatConfig,
    pub email: EmailConfig,
    pub slack: SlackConfig,
    pub qq: QQConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub token: String,
    pub allow_from: Vec<String>,
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiscordConfig {
    pub enabled: bool,
    pub token: String,
    pub allow_from: Vec<String>,
    pub gateway_url: String,
    pub intents: u64,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            allow_from: Vec::new(),
            gateway_url: "wss://gateway.discord.gg/?v=10&encoding=json".to_string(),
            intents: 37377,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WhatsAppConfig {
    pub enabled: bool,
    pub bridge_url: String,
    pub allow_from: Vec<String>,
}

impl Default for WhatsAppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bridge_url: "ws://localhost:3001".to_string(),
            allow_from: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FeishuConfig {
    pub enabled: bool,
    pub app_id: String,
    pub app_secret: String,
    pub encrypt_key: String,
    pub verification_token: String,
    pub allow_from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DingTalkConfig {
    pub enabled: bool,
    pub client_id: String,
    pub client_secret: String,
    pub allow_from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MochatConfig {
    pub enabled: bool,
    pub base_url: String,
    pub claw_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct EmailConfig {
    pub enabled: bool,
    pub consent_granted: bool,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_username: String,
    pub imap_password: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub allow_from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SlackConfig {
    pub enabled: bool,
    pub mode: String,
    pub bot_token: String,
    pub app_token: String,
    pub allow_from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QQConfig {
    pub enabled: bool,
    pub app_id: String,
    pub secret: String,
    pub allow_from: Vec<String>,
}

// --- Gateway Config ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 18790,
        }
    }
}

// --- Tools Config ---

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolsConfig {
    pub web: WebToolsConfig,
    pub exec: ExecToolConfig,
    pub restrict_to_workspace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WebToolsConfig {
    pub search: WebSearchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WebSearchConfig {
    pub api_key: String,
    pub max_results: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExecToolConfig {
    pub timeout: u64,
}

impl Default for ExecToolConfig {
    fn default() -> Self {
        Self { timeout: 60 }
    }
}
