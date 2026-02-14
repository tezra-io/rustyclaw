//! Embedding providers for semantic vector search.
//!
//! Provides an async trait for embedding text into dense vectors,
//! a registry for managing multiple providers with fallback,
//! and cosine similarity computation.

pub mod bootstrap;
mod local;
mod openai;

pub use bootstrap::build_embedding_registry;
pub use local::LocalEmbeddingProvider;
pub use openai::OpenAiEmbeddingProvider;

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::RustyClawError;

/// Compute cosine similarity between two vectors.
///
/// Returns a value in `[-1, 1]`, where 1 means identical direction.
/// Returns `0.0` if either vector has zero magnitude.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }

    dot / (mag_a * mag_b)
}

/// Async trait for embedding text into dense vectors.
///
/// Implementations must be `Send + Sync` for use across async tasks.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed one or more texts into dense vectors.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, RustyClawError>;

    /// The dimensionality of the output vectors.
    fn dimensions(&self) -> usize;

    /// Human-readable provider name.
    fn name(&self) -> &str;
}

/// Registry for managing embedding providers with automatic fallback.
///
/// Providers are tried in registration order. If the primary provider
/// fails, the next one is attempted.
pub struct EmbeddingRegistry {
    providers: Vec<Arc<dyn EmbeddingProvider>>,
}

impl EmbeddingRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a provider. First registered = highest priority.
    ///
    /// Returns an error if the provider's dimensions don't match
    /// already-registered providers (mixing 384-dim and 1536-dim
    /// vectors would produce incomparable embeddings).
    pub fn register(&mut self, provider: Arc<dyn EmbeddingProvider>) -> Result<(), RustyClawError> {
        if let Some(existing) = self.providers.first() {
            if existing.dimensions() != provider.dimensions() {
                return Err(RustyClawError::Embedding(format!(
                    "dimension mismatch: existing providers use {}-dim, but '{}' uses {}-dim",
                    existing.dimensions(),
                    provider.name(),
                    provider.dimensions()
                )));
            }
        }
        self.providers.push(provider);
        Ok(())
    }

    /// Get the primary (first) provider.
    pub fn primary(&self) -> Option<&Arc<dyn EmbeddingProvider>> {
        self.providers.first()
    }

    /// Embed using providers in order, falling back on failure.
    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, RustyClawError> {
        for provider in &self.providers {
            match provider.embed(texts).await {
                Ok(embeddings) => return Ok(embeddings),
                Err(e) => {
                    tracing::warn!(
                        provider = provider.name(),
                        error = %e,
                        "embedding provider failed, trying next"
                    );
                }
            }
        }
        Err(RustyClawError::Embedding(
            "no embedding provider available".into(),
        ))
    }

    /// Check if any providers are registered.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

impl Default for EmbeddingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn cosine_opposite_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn cosine_dimension_mismatch_returns_zero() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_empty_vectors_returns_zero() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn registry_empty() {
        let registry = EmbeddingRegistry::new();
        assert!(registry.is_empty());
        assert!(registry.primary().is_none());
    }

    struct MockProvider {
        dims: usize,
        embedding: Vec<f32>,
    }

    #[async_trait]
    impl EmbeddingProvider for MockProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, RustyClawError> {
            Ok(texts.iter().map(|_| self.embedding.clone()).collect())
        }
        fn dimensions(&self) -> usize {
            self.dims
        }
        fn name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn registry_with_mock_provider() {
        let mut registry = EmbeddingRegistry::new();
        registry
            .register(Arc::new(MockProvider {
                dims: 3,
                embedding: vec![0.1, 0.2, 0.3],
            }))
            .unwrap();

        assert!(!registry.is_empty());
        assert_eq!(registry.primary().unwrap().name(), "mock");

        let result = registry.embed(&["hello".to_string()]).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn registry_provider_switching() {
        struct FailingProvider;

        #[async_trait]
        impl EmbeddingProvider for FailingProvider {
            async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, RustyClawError> {
                Err(RustyClawError::Embedding("intentional failure".into()))
            }
            fn dimensions(&self) -> usize {
                3
            }
            fn name(&self) -> &str {
                "failing"
            }
        }

        let mut registry = EmbeddingRegistry::new();
        registry.register(Arc::new(FailingProvider)).unwrap();
        registry
            .register(Arc::new(MockProvider {
                dims: 3,
                embedding: vec![0.5, 0.5, 0.5],
            }))
            .unwrap();

        let result = registry.embed(&["test".to_string()]).await.unwrap();
        assert_eq!(result[0], vec![0.5, 0.5, 0.5]);
    }

    #[test]
    fn registry_rejects_dimension_mismatch() {
        let mut registry = EmbeddingRegistry::new();
        registry
            .register(Arc::new(MockProvider {
                dims: 384,
                embedding: vec![0.1; 384],
            }))
            .unwrap();

        let result = registry.register(Arc::new(MockProvider {
            dims: 1536,
            embedding: vec![0.1; 1536],
        }));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn registry_all_providers_fail() {
        struct FailingProvider;

        #[async_trait]
        impl EmbeddingProvider for FailingProvider {
            async fn embed(&self, _: &[String]) -> Result<Vec<Vec<f32>>, RustyClawError> {
                Err(RustyClawError::Embedding("fail".into()))
            }
            fn dimensions(&self) -> usize {
                3
            }
            fn name(&self) -> &str {
                "fail"
            }
        }

        let mut registry = EmbeddingRegistry::new();
        registry.register(Arc::new(FailingProvider)).unwrap();
        let result = registry.embed(&["test".to_string()]).await;
        assert!(result.is_err());
    }
}
