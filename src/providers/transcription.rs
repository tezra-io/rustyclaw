use reqwest::multipart;
use tracing::debug;

/// Transcribe audio using Groq's Whisper API.
pub async fn transcribe_audio(
    api_key: &str,
    file_path: &std::path::Path,
) -> crate::error::Result<String> {
    let client = reqwest::Client::new();
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.ogg")
        .to_string();

    let file_bytes = tokio::fs::read(file_path)
        .await
        .map_err(crate::error::NanobotError::Io)?;

    let part = multipart::Part::bytes(file_bytes)
        .file_name(file_name)
        .mime_str("audio/ogg")
        .unwrap();

    let form = multipart::Form::new()
        .text("model", "whisper-large-v3-turbo")
        .part("file", part);

    debug!("Transcribing audio via Groq Whisper API");

    let resp = client
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await
        .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(crate::error::NanobotError::Provider(format!(
            "Transcription failed: {}",
            text
        )));
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

    Ok(data["text"].as_str().unwrap_or("").to_string())
}
