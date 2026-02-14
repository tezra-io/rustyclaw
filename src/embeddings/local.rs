//! Local embedding provider using ONNX Runtime with all-MiniLM-L6-v2.
//!
//! Produces 384-dimensional embeddings locally with no API key required.
//! Model files are auto-downloaded from HuggingFace on first use.

use async_trait::async_trait;
use ort::session::Session;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

use super::EmbeddingProvider;
use crate::error::RustyClawError;

const MODEL_NAME: &str = "all-MiniLM-L6-v2";
const DIMENSIONS: usize = 384;
const MAX_SEQ_LEN: usize = 256;

const MODEL_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
const TOKENIZER_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";

// SHA-256 hashes of known-good model files (pin to prevent supply chain attacks)
const MODEL_SHA256: &str = "ad23a3bcb5fe1e4a1fe3b04a4e096b1ce75862ca6138c17aee4a270844680ee6";
const TOKENIZER_SHA256: &str = "405a0cafd2c8266dab9e15eeb5d96b30286a1073ce34e638d0ac11870b60abcb";
const DOWNLOAD_SENTINEL: &str = ".download_complete";

/// Local embedding provider using ONNX Runtime with all-MiniLM-L6-v2.
///
/// Auto-downloads model files from HuggingFace to `~/.rustyclaw/models/`
/// on first use. Produces 384-dimensional L2-normalized embeddings.
/// ONNX session is guarded by `Arc<Mutex<Session>>` and runs inference
/// via `spawn_blocking` to avoid blocking the async executor.
pub struct LocalEmbeddingProvider {
    session: Arc<Mutex<Session>>,
    tokenizer: Arc<tokenizers::Tokenizer>,
}

impl LocalEmbeddingProvider {
    /// Create a new local embedding provider.
    ///
    /// Downloads model files to `model_dir` if not already present.
    pub async fn new(model_dir: PathBuf) -> Result<Self, RustyClawError> {
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");
        let sentinel = model_dir.join(DOWNLOAD_SENTINEL);

        // Only skip download if sentinel exists (atomic completion marker)
        if !sentinel.exists() || !model_path.exists() || !tokenizer_path.exists() {
            Self::download_model(&model_dir).await?;
        }

        let mp = model_path.clone();
        let session = tokio::task::spawn_blocking(move || -> Result<Session, RustyClawError> {
            let session = Session::builder()
                .map_err(|e| {
                    RustyClawError::Embedding(format!("failed to create session builder: {}", e))
                })?
                .commit_from_file(&mp)
                .map_err(|e| {
                    RustyClawError::Embedding(format!("failed to load ONNX model: {}", e))
                })?;
            Ok(session)
        })
        .await
        .map_err(|e| RustyClawError::Embedding(format!("join error: {}", e)))??;

        let tp = tokenizer_path.clone();
        let tokenizer = tokio::task::spawn_blocking(move || {
            tokenizers::Tokenizer::from_file(&tp)
                .map_err(|e| RustyClawError::Embedding(format!("failed to load tokenizer: {}", e)))
        })
        .await
        .map_err(|e| RustyClawError::Embedding(format!("join error: {}", e)))??;

        info!("loaded local embedding model: {}", MODEL_NAME);

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            tokenizer: Arc::new(tokenizer),
        })
    }

    /// Try to create a provider, returning `None` if model files
    /// are unavailable or loading fails (graceful degradation).
    pub async fn try_new(model_dir: PathBuf) -> Option<Self> {
        match Self::new(model_dir).await {
            Ok(provider) => Some(provider),
            Err(e) => {
                warn!("local embedding provider unavailable: {}", e);
                None
            }
        }
    }

    /// Download model files from HuggingFace with integrity verification.
    ///
    /// Uses a temp directory + atomic rename to prevent partial downloads
    /// from corrupting state. A sentinel file marks successful completion.
    async fn download_model(model_dir: &Path) -> Result<(), RustyClawError> {
        let tmp_dir = model_dir.join(".tmp");
        // Clean up any previous failed attempt
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        tokio::fs::create_dir_all(&tmp_dir)
            .await
            .map_err(|e| RustyClawError::Embedding(format!("failed to create temp dir: {}", e)))?;

        info!("downloading {} model files...", MODEL_NAME);
        let client = reqwest::Client::new();

        // Download to temp dir first
        let tmp_model = tmp_dir.join("model.onnx");
        Self::download_and_verify(&client, MODEL_URL, &tmp_model, MODEL_SHA256).await?;
        debug!("downloaded and verified model.onnx");

        let tmp_tokenizer = tmp_dir.join("tokenizer.json");
        Self::download_and_verify(&client, TOKENIZER_URL, &tmp_tokenizer, TOKENIZER_SHA256).await?;
        debug!("downloaded and verified tokenizer.json");

        // Atomic move to final location
        tokio::fs::create_dir_all(model_dir)
            .await
            .map_err(|e| RustyClawError::Embedding(format!("failed to create model dir: {}", e)))?;

        let final_model = model_dir.join("model.onnx");
        let final_tokenizer = model_dir.join("tokenizer.json");
        tokio::fs::rename(&tmp_model, &final_model)
            .await
            .map_err(|e| RustyClawError::Embedding(format!("failed to move model.onnx: {}", e)))?;
        tokio::fs::rename(&tmp_tokenizer, &final_tokenizer)
            .await
            .map_err(|e| {
                RustyClawError::Embedding(format!("failed to move tokenizer.json: {}", e))
            })?;

        // Write sentinel to mark successful download
        let sentinel = model_dir.join(DOWNLOAD_SENTINEL);
        tokio::fs::write(&sentinel, MODEL_SHA256)
            .await
            .map_err(|e| RustyClawError::Embedding(format!("failed to write sentinel: {}", e)))?;

        // Clean up temp dir
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

        info!("{} model files downloaded and verified", MODEL_NAME);
        Ok(())
    }

    /// Download a file and verify its SHA-256 hash.
    async fn download_and_verify(
        client: &reqwest::Client,
        url: &str,
        path: &Path,
        expected_hash: &str,
    ) -> Result<(), RustyClawError> {
        let response =
            client.get(url).send().await.map_err(|e| {
                RustyClawError::Embedding(format!("failed to download {}: {}", url, e))
            })?;

        if !response.status().is_success() {
            return Err(RustyClawError::Embedding(format!(
                "download failed for {}: HTTP {}",
                url,
                response.status()
            )));
        }

        let bytes = response.bytes().await.map_err(|e| {
            RustyClawError::Embedding(format!("failed to read response from {}: {}", url, e))
        })?;

        // Verify SHA-256 hash
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual_hash = format!("{:x}", hasher.finalize());

        if actual_hash != expected_hash {
            return Err(RustyClawError::Embedding(format!(
                "integrity check failed for {}: expected {}, got {}",
                url, expected_hash, actual_hash
            )));
        }

        tokio::fs::write(path, &bytes).await.map_err(|e| {
            RustyClawError::Embedding(format!("failed to write {}: {}", path.display(), e))
        })?;

        Ok(())
    }

    /// Tokenize text and return (input_ids, attention_mask, token_type_ids).
    #[allow(clippy::type_complexity)]
    fn tokenize(&self, text: &str) -> Result<(Vec<i64>, Vec<i64>, Vec<i64>), RustyClawError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| RustyClawError::Embedding(format!("tokenization failed: {}", e)))?;

        let mut input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let mut attention_mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&m| m as i64)
            .collect();
        let mut token_type_ids: Vec<i64> =
            encoding.get_type_ids().iter().map(|&t| t as i64).collect();

        // Truncate to max sequence length
        if input_ids.len() > MAX_SEQ_LEN {
            input_ids.truncate(MAX_SEQ_LEN);
            attention_mask.truncate(MAX_SEQ_LEN);
            token_type_ids.truncate(MAX_SEQ_LEN);
        }

        Ok((input_ids, attention_mask, token_type_ids))
    }

    /// Mean pooling over token embeddings, weighted by attention mask.
    /// Input `data` is a flat slice of shape `[1, seq_len, hidden_size]`.
    fn mean_pool(data: &[f32], seq_len: usize, hidden_size: usize, mask: &[i64]) -> Vec<f32> {
        let mut pooled = vec![0.0f32; hidden_size];
        let mut mask_sum = 0.0f32;

        for i in 0..seq_len {
            let m = mask.get(i).copied().unwrap_or(0) as f32;
            mask_sum += m;
            for j in 0..hidden_size {
                pooled[j] += data[i * hidden_size + j] * m;
            }
        }

        if mask_sum > 0.0 {
            for v in &mut pooled {
                *v /= mask_sum;
            }
        }

        // L2 normalize
        let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut pooled {
                *v /= norm;
            }
        }

        pooled
    }
}

#[async_trait]
impl EmbeddingProvider for LocalEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, RustyClawError> {
        let mut results = Vec::with_capacity(texts.len());

        for text in texts {
            let (input_ids, attention_mask, token_type_ids) = self.tokenize(text)?;
            let seq_len = input_ids.len();

            let session = self.session.clone();
            let ids = input_ids;
            let mask = attention_mask.clone();
            let types = token_type_ids;

            let embedding = tokio::task::spawn_blocking(move || {
                let ids_tensor =
                    ort::value::Tensor::from_array(([1usize, seq_len], ids)).map_err(|e| {
                        RustyClawError::Embedding(format!("tensor creation failed: {}", e))
                    })?;
                let mask_tensor = ort::value::Tensor::from_array(([1usize, seq_len], mask.clone()))
                    .map_err(|e| {
                        RustyClawError::Embedding(format!("tensor creation failed: {}", e))
                    })?;
                let types_tensor = ort::value::Tensor::from_array(([1usize, seq_len], types))
                    .map_err(|e| {
                        RustyClawError::Embedding(format!("tensor creation failed: {}", e))
                    })?;

                let mut session_guard = session.lock().map_err(|e| {
                    RustyClawError::Embedding(format!("session lock poisoned: {}", e))
                })?;

                let inputs = ort::inputs![ids_tensor, mask_tensor, types_tensor];

                let outputs = session_guard
                    .run(inputs)
                    .map_err(|e| RustyClawError::Embedding(format!("inference failed: {}", e)))?;

                let (_shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(|e| {
                    RustyClawError::Embedding(format!("output extraction failed: {}", e))
                })?;

                Ok::<Vec<f32>, RustyClawError>(Self::mean_pool(data, seq_len, DIMENSIONS, &mask))
            })
            .await
            .map_err(|e| RustyClawError::Embedding(format!("join error: {}", e)))??;

            results.push(embedding);
        }

        Ok(results)
    }

    fn dimensions(&self) -> usize {
        DIMENSIONS
    }

    fn name(&self) -> &str {
        "local-minilm"
    }
}
