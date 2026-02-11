/// Specification for a supported LLM provider.
///
/// This is a data-driven registry — adding a new provider is just adding a new entry.
#[derive(Debug, Clone)]
pub struct ProviderSpec {
    pub name: &'static str,
    pub keywords: &'static [&'static str],
    pub env_key: &'static str,
    pub api_base: &'static str,
    pub default_model: &'static str,
    pub is_gateway: bool,
}

/// All supported providers.
pub static PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        name: "openrouter",
        keywords: &["openrouter/"],
        env_key: "OPENROUTER_API_KEY",
        api_base: "https://openrouter.ai/api/v1",
        default_model: "anthropic/claude-sonnet-4-5",
        is_gateway: true,
    },
    ProviderSpec {
        name: "aihubmix",
        keywords: &["aihubmix/"],
        env_key: "AIHUBMIX_API_KEY",
        api_base: "https://aihubmix.com/v1",
        default_model: "anthropic/claude-sonnet-4-5",
        is_gateway: true,
    },
    ProviderSpec {
        name: "anthropic",
        keywords: &["anthropic/", "claude-"],
        env_key: "ANTHROPIC_API_KEY",
        api_base: "https://api.anthropic.com/v1",
        default_model: "claude-sonnet-4-5-20250929",
        is_gateway: false,
    },
    ProviderSpec {
        name: "openai",
        keywords: &["openai/", "gpt-", "o1-", "o3-"],
        env_key: "OPENAI_API_KEY",
        api_base: "https://api.openai.com/v1",
        default_model: "gpt-4o",
        is_gateway: false,
    },
    ProviderSpec {
        name: "deepseek",
        keywords: &["deepseek/", "deepseek-"],
        env_key: "DEEPSEEK_API_KEY",
        api_base: "https://api.deepseek.com/v1",
        default_model: "deepseek-chat",
        is_gateway: false,
    },
    ProviderSpec {
        name: "gemini",
        keywords: &["gemini/", "gemini-"],
        env_key: "GEMINI_API_KEY",
        api_base: "https://generativelanguage.googleapis.com/v1beta/openai",
        default_model: "gemini-2.0-flash",
        is_gateway: false,
    },
    ProviderSpec {
        name: "groq",
        keywords: &["groq/"],
        env_key: "GROQ_API_KEY",
        api_base: "https://api.groq.com/openai/v1",
        default_model: "llama-3.3-70b-versatile",
        is_gateway: false,
    },
    ProviderSpec {
        name: "zhipu",
        keywords: &["zhipu/", "glm-"],
        env_key: "ZHIPU_API_KEY",
        api_base: "https://open.bigmodel.cn/api/paas/v4",
        default_model: "glm-4-flash",
        is_gateway: false,
    },
    ProviderSpec {
        name: "dashscope",
        keywords: &["dashscope/", "qwen-"],
        env_key: "DASHSCOPE_API_KEY",
        api_base: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        default_model: "qwen-plus",
        is_gateway: false,
    },
    ProviderSpec {
        name: "moonshot",
        keywords: &["moonshot/", "moonshot-"],
        env_key: "MOONSHOT_API_KEY",
        api_base: "https://api.moonshot.cn/v1",
        default_model: "moonshot-v1-auto",
        is_gateway: false,
    },
    ProviderSpec {
        name: "minimax",
        keywords: &["minimax/", "minimax-"],
        env_key: "MINIMAX_API_KEY",
        api_base: "https://api.minimax.chat/v1",
        default_model: "MiniMax-Text-01",
        is_gateway: false,
    },
    ProviderSpec {
        name: "vllm",
        keywords: &["vllm/"],
        env_key: "VLLM_API_KEY",
        api_base: "http://localhost:8000/v1",
        default_model: "default",
        is_gateway: false,
    },
];

/// Find the provider spec for a given model string.
pub fn find_provider_for_model(model: &str) -> Option<&'static ProviderSpec> {
    let lower = model.to_lowercase();
    PROVIDERS
        .iter()
        .find(|p| p.keywords.iter().any(|kw| lower.contains(kw)))
}

/// Find a provider by name.
pub fn find_provider_by_name(name: &str) -> Option<&'static ProviderSpec> {
    let lower = name.to_lowercase();
    PROVIDERS.iter().find(|p| p.name == lower)
}

/// Strip the provider prefix from a model name (e.g., "openai/gpt-4o" → "gpt-4o").
pub fn strip_prefix(model: &str) -> &str {
    model.find('/').map(|i| &model[i + 1..]).unwrap_or(model)
}
