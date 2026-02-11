use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::providers::registry::{find_provider_by_name, PROVIDERS};

/// Helper for serde skip_serializing_if on bool fields.
fn is_false(v: &bool) -> bool {
    !v
}

/// Root configuration for rustyclaw.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub agents: AgentsConfig,
    #[serde(skip_serializing_if = "ChannelsConfig::is_default")]
    pub channels: ChannelsConfig,
    pub providers: ProvidersConfig,
    #[serde(skip_serializing_if = "GatewayConfig::is_default")]
    pub gateway: GatewayConfig,
    #[serde(skip_serializing_if = "ToolsConfig::is_default")]
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
            workspace: "~/.rustyclaw/workspace".to_string(),
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
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_headers: Option<std::collections::HashMap<String, String>>,
}

impl ProviderConfig {
    pub fn is_empty(&self) -> bool {
        self.api_key.is_empty() && self.api_base.is_none() && self.extra_headers.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProvidersConfig {
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub anthropic: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub openai: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub openrouter: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub deepseek: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub groq: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub zhipu: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub dashscope: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub vllm: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub gemini: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub moonshot: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
    pub minimax: ProviderConfig,
    #[serde(skip_serializing_if = "ProviderConfig::is_empty")]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelsConfig {
    #[serde(skip_serializing_if = "TelegramConfig::is_default")]
    pub telegram: TelegramConfig,
    #[serde(skip_serializing_if = "DiscordConfig::is_default")]
    pub discord: DiscordConfig,
    #[serde(skip_serializing_if = "WhatsAppConfig::is_default")]
    pub whatsapp: WhatsAppConfig,
    #[serde(skip_serializing_if = "FeishuConfig::is_default")]
    pub feishu: FeishuConfig,
    #[serde(skip_serializing_if = "DingTalkConfig::is_default")]
    pub dingtalk: DingTalkConfig,
    #[serde(skip_serializing_if = "MochatConfig::is_default")]
    pub mochat: MochatConfig,
    #[serde(skip_serializing_if = "EmailConfig::is_default")]
    pub email: EmailConfig,
    #[serde(skip_serializing_if = "SlackConfig::is_default")]
    pub slack: SlackConfig,
    #[serde(skip_serializing_if = "QQConfig::is_default")]
    pub qq: QQConfig,
}

impl ChannelsConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TelegramConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_from: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
}

impl TelegramConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct DiscordConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
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

impl DiscordConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct WhatsAppConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    pub bridge_url: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
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

impl WhatsAppConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FeishuConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub app_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub app_secret: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub encrypt_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub verification_token: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_from: Vec<String>,
}

impl FeishuConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct DingTalkConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client_secret: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_from: Vec<String>,
}

impl DingTalkConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MochatConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub claw_token: String,
}

impl MochatConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EmailConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub consent_granted: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub imap_host: String,
    pub imap_port: u16,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub imap_username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub imap_password: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub smtp_host: String,
    pub smtp_port: u16,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub smtp_username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub smtp_password: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_from: Vec<String>,
}

impl EmailConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SlackConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bot_token: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub app_token: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_from: Vec<String>,
}

impl SlackConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct QQConfig {
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub app_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub secret: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_from: Vec<String>,
}

impl QQConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

// --- Gateway Config ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl GatewayConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

// --- Tools Config ---

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolsConfig {
    #[serde(skip_serializing_if = "WebToolsConfig::is_default")]
    pub web: WebToolsConfig,
    #[serde(skip_serializing_if = "ExecToolConfig::is_default")]
    pub exec: ExecToolConfig,
    #[serde(skip_serializing_if = "is_false")]
    pub restrict_to_workspace: bool,
}

impl ToolsConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct WebToolsConfig {
    #[serde(skip_serializing_if = "WebSearchConfig::is_default")]
    pub search: WebSearchConfig,
}

impl WebToolsConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct WebSearchConfig {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    pub max_results: u32,
}

impl WebSearchConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ExecToolConfig {
    pub timeout: u64,
}

impl Default for ExecToolConfig {
    fn default() -> Self {
        Self { timeout: 60 }
    }
}

impl ExecToolConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
