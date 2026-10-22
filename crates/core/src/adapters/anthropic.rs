use super::sse::SseParser;
use super::*;
use crate::message::Role;
use futures_util::StreamExt;
use secrecy::ExposeSecret;
use serde_json::{json, Value};

const DEFAULT_BASE: &str = "https://api.anthropic.com";
const VERSION: &str = "2023-06-01";

pub struct Anthropic {
    client: reqwest::Client,
    base_url: String,
}

impl Anthropic {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE)
    }

    pub fn with_base_url(base: &str) -> Self {
        Self { client: http_client(), base_url: base.trim_end_matches('/').to_string() }
    }
}

impl Default for Anthropic {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn build_body(req: &ChatRequest) -> Value {
    let system: Vec<String> = req
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.plain_text())
        .collect();
    let messages: Vec<Value> = req
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(|m| json!({ "role": m.role.as_str(), "content": m.plain_text() }))
        .collect();
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = json!(system.join("\n\n"));
    }
    body
}

pub(crate) fn rate_limit_from(headers: &reqwest::header::HeaderMap) -> RateLimitInfo {
    let reset = |name| {
        header_str(headers, name)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_millis())
    };
    RateLimitInfo {
        requests_remaining: header_i64(headers, "anthropic-ratelimit-requests-remaining"),
        requests_reset_at: reset("anthropic-ratelimit-requests-reset"),
        tokens_remaining: header_i64(headers, "anthropic-ratelimit-tokens-remaining"),
        tokens_reset_at: reset("anthropic-ratelimit-tokens-reset"),
    }
}

pub(crate) fn classify(status: u16, retry_after: Option<u64>, body: &str) -> ProviderError {
    let lower = body.to_lowercase();
    match status {
        401 | 403 => ProviderError::InvalidKey,
        429 => ProviderError::RateLimited { retry_after_secs: retry_after },
        413 => ProviderError::ContextTooLong,
        400 if lower.contains("credit balance") => ProviderError::CreditExhausted,
        400 if lower.contains("prompt is too long") => ProviderError::ContextTooLong,
        500..=599 => ProviderError::Transient(format!("HTTP {status}")),
        _ => ProviderError::Other(format!("HTTP {status}: {}", body.chars().take(200).collect::<String>())),
    }
}

#[async_trait]
impl Provider for Anthropic {
    async fn stream_chat(
        &self,
        key: &SecretString,
        req: &ChatRequest,
        on_chunk: OnChunk<'_>,
    ) -> Result<ChatOutcome, ProviderError> {
        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", key.expose_secret())
            .header("anthropic-version", VERSION)
            .json(&build_body(req))
            .send()
            .await
            .map_err(net_err)?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(classify(status, retry_after(&headers), &body));
        }

        let mut text = String::new();
        let mut usage = Usage::default();
        let mut finished = false;
        let mut parser = SseParser::default();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            for ev in parser.feed(&chunk.map_err(net_err)?) {
                let Ok(v) = serde_json::from_str::<Value>(&ev.data) else { continue };
                match v["type"].as_str() {
                    Some("message_start") => {
                        usage.input_tokens = v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
                    }
                    Some("content_block_delta") if v["delta"]["type"] == "text_delta" => {
                        if let Some(t) = v["delta"]["text"].as_str() {
                            text.push_str(t);
                            on_chunk(t);
                        }
                    }
                    Some("message_delta") => {
                        if let Some(n) = v["usage"]["output_tokens"].as_u64() {
                            usage.output_tokens = n as u32;
                        }
                    }
                    Some("message_stop") => finished = true,
                    Some("error") => {
                        let kind = v["error"]["type"].as_str().unwrap_or("");
                        return Err(match kind {
                            "overloaded_error" | "api_error" => ProviderError::Transient(kind.into()),
                            "rate_limit_error" => ProviderError::RateLimited { retry_after_secs: None },
                            _ => ProviderError::Other(v["error"]["message"].as_str().unwrap_or(kind).into()),
                        });
                    }
                    _ => {}
                }
            }
        }
        if !finished {
            return Err(ProviderError::Transient("stream ended before message_stop".into()));
        }
        Ok(ChatOutcome { text, usage, rate_limit: rate_limit_from(&headers) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req() -> ChatRequest {
        ChatRequest {
            model: "m".into(),
            max_tokens: 100,
            messages: vec![
                Message::text(Role::System, "be brief"),
                Message::text(Role::User, "hi"),
                Message::text(Role::Assistant, "hello"),
                Message::text(Role::User, "again"),
            ],
        }
    }

    const STREAM: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    #[test]
    fn body_puts_system_at_top_level() {
        let b = build_body(&req());
        assert_eq!(b["system"], "be brief");
        assert_eq!(b["messages"].as_array().unwrap().len(), 3);
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["stream"], true);
    }

    #[tokio::test]
    async fn streams_text_usage_and_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "sk-test"))
            .and(header("anthropic-version", VERSION))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .insert_header("anthropic-ratelimit-requests-remaining", "49")
                    .insert_header("anthropic-ratelimit-tokens-remaining", "9000")
                    .insert_header("anthropic-ratelimit-tokens-reset", "2026-01-01T00:00:00Z")
                    .set_body_string(STREAM),
            )
            .mount(&server)
            .await;
        let mut got = Vec::new();
        let out = Anthropic::with_base_url(&server.uri())
            .stream_chat(&SecretString::from("sk-test".to_string()), &req(), &mut |c| got.push(c.to_string()))
            .await
            .unwrap();
        assert_eq!(out.text, "Hello");
        assert_eq!(got, ["Hel", "lo"]);
        assert_eq!(out.usage, Usage { input_tokens: 12, output_tokens: 5 });
        assert_eq!(out.rate_limit.requests_remaining, Some(49));
        assert_eq!(out.rate_limit.tokens_remaining, Some(9000));
        assert_eq!(out.rate_limit.tokens_reset_at, Some(1_767_225_600_000));
    }

    async fn status_case(status: u16, retry: Option<&str>, body: &str) -> ProviderError {
        let server = MockServer::start().await;
        let mut t = ResponseTemplate::new(status).set_body_string(body);
        if let Some(r) = retry {
            t = t.insert_header("retry-after", r);
        }
        Mock::given(method("POST")).respond_with(t).mount(&server).await;
        Anthropic::with_base_url(&server.uri())
            .stream_chat(&SecretString::from("k".to_string()), &req(), &mut |_| {})
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn classifies_errors() {
        assert_eq!(status_case(401, None, "{}").await, ProviderError::InvalidKey);
        assert_eq!(
            status_case(429, Some("7"), "{}").await,
            ProviderError::RateLimited { retry_after_secs: Some(7) }
        );
        assert_eq!(
            status_case(400, None, "Your credit balance is too low").await,
            ProviderError::CreditExhausted
        );
        assert_eq!(status_case(400, None, "prompt is too long: 9 > 8").await, ProviderError::ContextTooLong);
        assert!(matches!(status_case(529, None, "").await, ProviderError::Transient(_)));
    }

    #[tokio::test]
    async fn truncated_stream_is_transient() {
        let server = MockServer::start().await;
        let cut = STREAM.split("event: message_stop").next().unwrap().to_string();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(cut))
            .mount(&server)
            .await;
        let err = Anthropic::with_base_url(&server.uri())
            .stream_chat(&SecretString::from("k".to_string()), &req(), &mut |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Transient(_)));
    }
}
