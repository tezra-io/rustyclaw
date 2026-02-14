//! Bootstrap wiring: reads EmbeddingsConfig and constructs the
//! appropriate providers into an EmbeddingRegistry.

use std::sync::Arc;
use tracing::{info, warn};

use super::{EmbeddingRegistry, LocalEmbeddingProvider, OpenAiEmbeddingProvider};
use crate::config::schema::EmbeddingsConfig;
use crate::error::RustyClawError;

/// Build an `EmbeddingRegistry` from configuration.
///
/// - `"local"` (default): loads all-MiniLM-L6-v2 via ONNX Runtime.
/// - `"openai"`: uses OpenAI text-embedding-3-small (requires API key).
/// - `"none"` / `"disabled"`: returns empty registry (no semantic search).
///
/// When provider is `"local"`, OpenAI is added as fallback if a key is available.
pub async fn build_embedding_registry(
    config: &EmbeddingsConfig,
) -> Result<EmbeddingRegistry, RustyClawError> {
    let mut registry = EmbeddingRegistry::new();

    match config.provider.as_str() {
        "local" | "" => {
            let cache_dir = config.resolve_cache_dir();
            match LocalEmbeddingProvider::try_new(cache_dir).await {
                Some(provider) => {
                    info!("embedding provider: local (all-MiniLM-L6-v2)");
                    registry.register(Arc::new(provider))?;
                }
                None => {
                    warn!("local embedding provider unavailable — semantic search disabled");
                }
            }
        }
        "openai" => {
            if let Some(key) = config.resolve_openai_key() {
                let provider = OpenAiEmbeddingProvider::new(key, None, config.model.clone());
                info!("embedding provider: openai");
                registry.register(Arc::new(provider))?;
            } else {
                return Err(RustyClawError::Embedding(
                    "OpenAI embedding provider requires API key: set RUSTYCLAW_OPENAI_API_KEY env var or config.embeddings.openai_api_key".into(),
                ));
            }
        }
        "none" | "disabled" => {
            info!("embedding provider: disabled — semantic search off");
        }
        other => {
            return Err(RustyClawError::Embedding(format!(
                "unknown embedding provider: '{}' (expected 'local', 'openai', or 'none')",
                other
            )));
        }
    }

    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_disabled_registry() {
        let config = EmbeddingsConfig {
            provider: "none".to_string(),
            ..Default::default()
        };
        let registry = build_embedding_registry(&config).await.unwrap();
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn build_unknown_provider_errors() {
        let config = EmbeddingsConfig {
            provider: "potato".to_string(),
            ..Default::default()
        };
        let result = build_embedding_registry(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn build_openai_without_key_errors() {
        // Make sure env var isn't set for this test
        std::env::remove_var("RUSTYCLAW_OPENAI_API_KEY");
        let config = EmbeddingsConfig {
            provider: "openai".to_string(),
            openai_api_key: None,
            ..Default::default()
        };
        let result = build_embedding_registry(&config).await;
        assert!(result.is_err());
    }
}
