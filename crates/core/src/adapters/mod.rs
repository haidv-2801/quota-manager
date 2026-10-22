use crate::message::Message;
use async_trait::async_trait;
use secrecy::SecretString;
use std::time::Duration;

pub mod anthropic;
pub mod openai;
pub mod sse;

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Rate-limit state reported by the provider, normalised. Reset times are unix ms.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateLimitInfo {
    pub requests_limit: Option<i64>,
    pub requests_remaining: Option<i64>,
    pub requests_reset_at: Option<i64>,
    pub tokens_limit: Option<i64>,
    pub tokens_remaining: Option<i64>,
    pub tokens_reset_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ChatOutcome {
    pub text: String,
    pub usage: Usage,
    pub rate_limit: RateLimitInfo,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProviderError {
    #[error("rate limited")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("credit exhausted")]
    CreditExhausted,
    #[error("invalid API key")]
    InvalidKey,
    #[error("context too long")]
    ContextTooLong,
    #[error("transient error: {0}")]
    Transient(String),
    #[error("provider error: {0}")]
    Other(String),
}

pub type OnChunk<'a> = &'a mut (dyn FnMut(&str) + Send);

#[async_trait]
pub trait Provider: Send + Sync {
    /// Streams a reply, calling `on_chunk` for each text delta.
    async fn stream_chat(
        &self,
        key: &SecretString,
        req: &ChatRequest,
        on_chunk: OnChunk<'_>,
    ) -> Result<ChatOutcome, ProviderError>;
}

pub fn for_provider(name: &str) -> Option<Box<dyn Provider>> {
    match name {
        "anthropic" => Some(Box::new(anthropic::Anthropic::new())),
        "openai" => Some(Box::new(openai::OpenAi::new())),
        _ => None,
    }
}

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("http client")
}

pub(crate) fn net_err(e: reqwest::Error) -> ProviderError {
    ProviderError::Transient(e.to_string())
}

pub(crate) fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers.get("retry-after")?.to_str().ok()?.trim().parse().ok()
}

pub(crate) fn header_i64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<i64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

pub(crate) fn header_str<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
