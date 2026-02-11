pub mod base;
pub mod openai_compat;
pub mod registry;
pub mod transcription;

pub use base::{LlmProvider, LlmResponse, ToolCallRequest};
pub use openai_compat::OpenAiCompatProvider;
pub use registry::{find_provider_for_model, ProviderSpec};
