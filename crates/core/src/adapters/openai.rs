use super::sse::SseParser;
use super::*;
use futures_util::StreamExt;
use secrecy::ExposeSecret;
use serde_json::{json, Value};

const DEFAULT_BASE: &str = "https://api.openai.com";

pub struct OpenAi {
    client: reqwest::Client,
    base_url: String,
}

impl OpenAi {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE)
    }

    pub fn with_base_url(base: &str) -> Self {
        Self { client: http_client(), base_url: base.trim_end_matches('/').to_string() }
    }
}

impl Default for OpenAi {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn build_body(req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({ "role": m.role.as_str(), "content": m.plain_text() }))
        .collect();
    json!({
        "model": req.model,
        "messages": messages,
        "max_completion_tokens": req.max_tokens,
        "stream": true,
        "stream_options": { "include_usage": true },
    })
}

/// Parses durations such as "1s", "20ms", "6m0s", "1h2m3.5s" into milliseconds.
pub(crate) fn parse_duration_ms(s: &str) -> Option<i64> {
    let mut total = 0.0f64;
    let mut num = String::new();
    let mut chars = s.trim().chars().peekable();
    if chars.peek().is_none() {
        return None;
    }
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        let n: f64 = num.parse().ok()?;
        num.clear();
        total += match c {
            'h' => n * 3_600_000.0,
            'm' if chars.peek() == Some(&'s') => {
                chars.next();
                n
            }
            'm' => n * 60_000.0,
            's' => n * 1000.0,
            _ => return None,
        };
    }
    if !num.is_empty() {
        return None;
    }
    Some(total.round() as i64)
}

pub(crate) fn rate_limit_from(headers: &reqwest::header::HeaderMap, now: i64) -> RateLimitInfo {
    let reset = |name| header_str(headers, name).and_then(parse_duration_ms).map(|ms| now + ms);
    RateLimitInfo {
        requests_limit: header_i64(headers, "x-ratelimit-limit-requests"),
        requests_remaining: header_i64(headers, "x-ratelimit-remaining-requests"),
        requests_reset_at: reset("x-ratelimit-reset-requests"),
        tokens_limit: header_i64(headers, "x-ratelimit-limit-tokens"),
        tokens_remaining: header_i64(headers, "x-ratelimit-remaining-tokens"),
        tokens_reset_at: reset("x-ratelimit-reset-tokens"),
    }
}

pub(crate) fn classify(status: u16, retry_after: Option<u64>, body: &str) -> ProviderError {
    let code = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_string))
        .unwrap_or_default();
    match status {
        401 | 403 => ProviderError::InvalidKey,
        429 if code == "insufficient_quota" => ProviderError::CreditExhausted,
        429 => ProviderError::RateLimited { retry_after_secs: retry_after },
        400 | 413 if code == "context_length_exceeded" => ProviderError::ContextTooLong,
        500..=599 => ProviderError::Transient(format!("HTTP {status}")),
        _ => ProviderError::Other(format!("HTTP {status}: {}", body.chars().take(200).collect::<String>())),
    }
}

#[async_trait]
impl Provider for OpenAi {
    async fn stream_chat(
        &self,
        key: &SecretString,
        req: &ChatRequest,
        on_chunk: OnChunk<'_>,
    ) -> Result<ChatOutcome, ProviderError> {
        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .bearer_auth(key.expose_secret())
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
        'outer: while let Some(chunk) = stream.next().await {
            for ev in parser.feed(&chunk.map_err(net_err)?) {
                if ev.data.trim() == "[DONE]" {
                    finished = true;
                    break 'outer;
                }
                let Ok(v) = serde_json::from_str::<Value>(&ev.data) else { continue };
                if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                    text.push_str(t);
                    on_chunk(t);
                }
                if v["usage"].is_object() {
                    usage.input_tokens = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                    usage.output_tokens = v["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;
                }
            }
        }
        if !finished {
            return Err(ProviderError::Transient("stream ended before [DONE]".into()));
        }
        Ok(ChatOutcome { text, usage, rate_limit: rate_limit_from(&headers, now_ms()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req() -> ChatRequest {
        ChatRequest {
            model: "m".into(),
            max_tokens: 100,
            messages: vec![Message::text(Role::System, "be brief"), Message::text(Role::User, "hi")],
        }
    }

    const STREAM: &str = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n\
data: [DONE]\n\n";

    #[test]
    fn parses_go_style_durations() {
        assert_eq!(parse_duration_ms("1s"), Some(1000));
        assert_eq!(parse_duration_ms("20ms"), Some(20));
        assert_eq!(parse_duration_ms("6m0s"), Some(360_000));
        assert_eq!(parse_duration_ms("1h2m3.5s"), Some(3_723_500));
        assert_eq!(parse_duration_ms(""), None);
        assert_eq!(parse_duration_ms("abc"), None);
        assert_eq!(parse_duration_ms("5"), None);
    }

    #[test]
    fn body_keeps_system_message_inline() {
        let b = build_body(&req());
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["stream_options"]["include_usage"], true);
    }

    #[tokio::test]
    async fn streams_text_usage_and_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-limit-requests", "100")
                    .insert_header("x-ratelimit-remaining-requests", "99")
                    .insert_header("x-ratelimit-remaining-tokens", "5000")
                    .insert_header("x-ratelimit-reset-tokens", "1s")
                    .set_body_string(STREAM),
            )
            .mount(&server)
            .await;
        let mut got = Vec::new();
        let before = now_ms();
        let out = OpenAi::with_base_url(&server.uri())
            .stream_chat(&SecretString::from("sk-test".to_string()), &req(), &mut |c| got.push(c.to_string()))
            .await
            .unwrap();
        assert_eq!(out.text, "Hello");
        assert_eq!(got, ["Hel", "lo"]);
        assert_eq!(out.usage, Usage { input_tokens: 7, output_tokens: 2 });
        assert_eq!(out.rate_limit.requests_limit, Some(100));
        assert_eq!(out.rate_limit.requests_remaining, Some(99));
        assert!(out.rate_limit.tokens_reset_at.unwrap() >= before + 1000);
    }

    async fn status_case(status: u16, retry: Option<&str>, body: &str) -> ProviderError {
        let server = MockServer::start().await;
        let mut t = ResponseTemplate::new(status).set_body_string(body);
        if let Some(r) = retry {
            t = t.insert_header("retry-after", r);
        }
        Mock::given(method("POST")).respond_with(t).mount(&server).await;
        OpenAi::with_base_url(&server.uri())
            .stream_chat(&SecretString::from("k".to_string()), &req(), &mut |_| {})
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn distinguishes_rate_limit_from_exhausted_credit() {
        assert_eq!(
            status_case(429, Some("3"), r#"{"error":{"code":"rate_limit_exceeded"}}"#).await,
            ProviderError::RateLimited { retry_after_secs: Some(3) }
        );
        assert_eq!(
            status_case(429, None, r#"{"error":{"code":"insufficient_quota"}}"#).await,
            ProviderError::CreditExhausted
        );
        assert_eq!(status_case(401, None, "{}").await, ProviderError::InvalidKey);
        assert_eq!(
            status_case(400, None, r#"{"error":{"code":"context_length_exceeded"}}"#).await,
            ProviderError::ContextTooLong
        );
        assert!(matches!(status_case(503, None, "").await, ProviderError::Transient(_)));
    }

    #[tokio::test]
    async fn stream_without_done_is_transient() {
        let server = MockServer::start().await;
        let cut = STREAM.replace("data: [DONE]\n\n", "");
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(cut))
            .mount(&server)
            .await;
        let err = OpenAi::with_base_url(&server.uri())
            .stream_chat(&SecretString::from("k".to_string()), &req(), &mut |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Transient(_)));
    }
}
