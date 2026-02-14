//! OpenAI embedding provider using text-embedding-3-small.
//!
//! Requires an API key. Intended as an optional fallback when the
//! local ONNX-based provider is unavailable.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::EmbeddingProvider;
use crate::error::RustyClawError;

const DEFAULT_MODEL: &str = "text-embedding-3-small";
const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";
const DIMENSIONS: usize = 1536;

/// OpenAI embedding provider using the text-embedding-3-small model.
///
/// Requires an API key. Use this as a fallback when the local provider
/// is unavailable, or when higher-quality embeddings are needed.
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    model: String,
}

impl OpenAiEmbeddingProvider {
    /// Create a new OpenAI embedding provider.
    pub fn new(api_key: String, api_base: Option<String>, model: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            api_base: api_base.unwrap_or_else(|| DEFAULT_API_BASE.to_string()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }
}

#[derive(Serialize)]
struct EmbeddingRequest {
    input: Vec<String>,
    model: String,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, RustyClawError> {
        let url = format!("{}/embeddings", self.api_base);

        let request = EmbeddingRequest {
            input: texts.to_vec(),
            model: self.model.clone(),
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| RustyClawError::Embedding(format!("OpenAI request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RustyClawError::Embedding(format!(
                "OpenAI API error ({}): {}",
                status, body
            )));
        }

        let result: EmbeddingResponse = response.json().await.map_err(|e| {
            RustyClawError::Embedding(format!("failed to parse OpenAI response: {}", e))
        })?;

        debug!(
            model = %self.model,
            count = texts.len(),
            "OpenAI embeddings generated"
        );

        Ok(result.data.into_iter().map(|d| d.embedding).collect())
    }

    fn dimensions(&self) -> usize {
        DIMENSIONS
    }

    fn name(&self) -> &str {
        "openai"
    }
}
