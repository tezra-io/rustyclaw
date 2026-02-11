use thiserror::Error;

#[derive(Error, Debug)]
pub enum NanobotError {
    #[error("config error: {0}")]
    Config(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("channel error: {0}")]
    Channel(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("cron error: {0}")]
    Cron(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
}

pub type Result<T> = std::result::Result<T, NanobotError>;
