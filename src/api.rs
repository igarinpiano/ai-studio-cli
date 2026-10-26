//! Minimal Gemini API client (the API behind AI Studio's "Get code").

use std::fmt;
use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: Option<u16>,
    pub status_text: String,
    pub message: String,
}

impl ApiError {
    fn other(message: impl Into<String>) -> Self {
        ApiError { status: None, status_text: String::new(), message: message.into() }
    }

    fn from_body(status: u16, body: &str) -> Self {
        let parsed: Option<Value> = serde_json::from_str(body).ok();
        let err = parsed.as_ref().and_then(|v| v.get("error"));
        let message =
            err.and_then(|e| e.get("message")).and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| {
                let b = body.trim();
                if b.is_empty() { "(empty response)".to_string() } else { b.chars().take(500).collect() }
            });
        let status_text = err.and_then(|e| e.get("status")).and_then(Value::as_str).unwrap_or_default().to_string();
        ApiError { status: Some(status), status_text, message }
    }

    fn from_stream_value(v: &Value) -> Self {
        let status = v.get("code").and_then(Value::as_u64).map(|c| c as u16);
        ApiError {
            status,
            status_text: v.get("status").and_then(Value::as_str).unwrap_or_default().to_string(),
            message: v.get("message").and_then(Value::as_str).unwrap_or("stream error").to_string(),
        }
    }

    pub fn hint(&self) -> Option<&'static str> {
        match self.status {
            Some(400) if self.message.to_lowercase().contains("api key") => {
                Some("Check your API key: run /key or `aistudio login`.")
            }
            Some(400) if self.message.to_lowercase().contains("thinking") => {
                Some("This model may not support that thinking setting. Try /thinking default or /thoughts off.")
            }
            Some(401) | Some(403) => Some("Check your API key: run /key or `aistudio login`."),
            Some(404) => Some("Model not found. Run /models to see what your key can use."),
            Some(429) => Some("Rate limit or quota exceeded. Wait a bit, or switch models with /model."),
            Some(500..=599) => Some("Server-side error. Try /retry in a moment."),
            _ => None,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.status, self.status_text.is_empty()) {
            (Some(code), false) => write!(f, "{code} {}: {}", self.status_text, self.message),
            (Some(code), true) => write!(f, "{code}: {}", self.message),
            (None, _) => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub input_token_limit: Option<u64>,
    pub output_token_limit: Option<u64>,
    pub supported_generation_methods: Vec<String>,
    pub thinking: Option<bool>,
}

impl ModelInfo {
    pub fn id(&self) -> &str {
        self.name.trim_start_matches("models/")
    }

    pub fn can_chat(&self) -> bool {
        self.supported_generation_methods.iter().any(|m| m == "generateContent")
    }
}

pub enum StreamEvent {
    Chunk(Value),
    Error(ApiError),
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::blocking::Client,
    base_url: String,
    api_key: String,
}

impl Client {
    pub fn new(api_key: String) -> Result<Client, ApiError> {
        let base_url = std::env::var("GEMINI_API_BASE")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            // reqwest is built without a default crypto provider (see Cargo.toml).
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let http = reqwest::blocking::Client::builder()
            .user_agent(concat!("ai-studio-cli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            // Long thinking runs can take minutes; interruption is handled by the caller.
            .timeout(None)
            .build()
            .map_err(|e| ApiError::other(format!("cannot create HTTP client: {e}")))?;
        Ok(Client { http, base_url, api_key })
    }

    pub fn list_models(&self) -> Result<Vec<ModelInfo>, ApiError> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase", default)]
        struct Page {
            models: Vec<ModelInfo>,
            next_page_token: Option<String>,
        }
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut url = format!("{}/models?pageSize=1000", self.base_url);
            if let Some(t) = &token {
                url.push_str("&pageToken=");
                url.push_str(&percent_encode(t));
            }
            let resp = self
                .http
                .get(url)
                .header("x-goog-api-key", &self.api_key)
                .timeout(Duration::from_secs(60))
                .send()
                .map_err(|e| ApiError::other(format!("request failed: {e}")))?;
            let status = resp.status().as_u16();
            let text = resp.text().map_err(|e| ApiError::other(format!("read failed: {e}")))?;
            if status >= 400 {
                return Err(ApiError::from_body(status, &text));
            }
            let page: Page =
                serde_json::from_str(&text).map_err(|e| ApiError::other(format!("bad model list: {e}")))?;
            out.extend(page.models);
            match page.next_page_token.filter(|t| !t.is_empty()) {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        Ok(out)
    }

    pub fn stream_url(&self, model: &str) -> String {
        format!("{}/models/{}:streamGenerateContent?alt=sse", self.base_url, model.trim_start_matches("models/"))
    }

    /// Start a streaming generation on a background thread. Events arrive on
    /// the returned channel; the channel closes when the stream ends. Dropping
    /// the receiver abandons the request.
    pub fn stream_generate(&self, model: &str, body: Value) -> Receiver<StreamEvent> {
        let (tx, rx) = mpsc::channel();
        let client = self.clone();
        let url = self.stream_url(model);
        thread::spawn(move || {
            let resp = match client.post_with_retry(&url, &body) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(e));
                    return;
                }
            };
            for event in SseReader::new(BufReader::new(resp)) {
                let item = match event {
                    Ok(data) => match serde_json::from_str::<Value>(&data) {
                        Ok(v) => match v.get("error") {
                            Some(err) => StreamEvent::Error(ApiError::from_stream_value(err)),
                            None => StreamEvent::Chunk(v),
                        },
                        Err(e) => StreamEvent::Error(ApiError::other(format!("bad stream data: {e}"))),
                    },
                    Err(e) => StreamEvent::Error(ApiError::other(format!("connection lost: {e}"))),
                };
                let stop = matches!(item, StreamEvent::Error(_));
                if tx.send(item).is_err() || stop {
                    return;
                }
            }
        });
        rx
    }

    fn post_with_retry(&self, url: &str, body: &Value) -> Result<reqwest::blocking::Response, ApiError> {
        let delays = [Duration::from_secs(1), Duration::from_secs(3)];
        let mut attempt = 0;
        loop {
            let result = self.http.post(url).header("x-goog-api-key", &self.api_key).json(body).send();
            let retryable = match &result {
                Ok(r) => matches!(r.status().as_u16(), 500 | 502 | 503 | 504),
                Err(e) => e.is_connect() || e.is_timeout(),
            };
            if retryable && attempt < delays.len() {
                thread::sleep(delays[attempt]);
                attempt += 1;
                continue;
            }
            let resp = result.map_err(|e| ApiError::other(format!("request failed: {e}")))?;
            let status = resp.status().as_u16();
            if status >= 400 {
                let text = resp.text().unwrap_or_default();
                return Err(ApiError::from_body(status, &text));
            }
            return Ok(resp);
        }
    }
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Iterates over the `data:` payloads of a server-sent-events stream.
pub struct SseReader<R: BufRead> {
    reader: R,
    done: bool,
}

impl<R: BufRead> SseReader<R> {
    pub fn new(reader: R) -> Self {
        SseReader { reader, done: false }
    }
}

impl<R: BufRead> Iterator for SseReader<R> {
    type Item = std::io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut data = String::new();
        let mut has_data = false;
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    self.done = true;
                    return if has_data { Some(Ok(data)) } else { None };
                }
                Ok(_) => {
                    let l = line.trim_end_matches(['\r', '\n']);
                    if l.is_empty() {
                        if has_data {
                            return Some(Ok(data));
                        }
                        continue;
                    }
                    if let Some(rest) = l.strip_prefix("data:") {
                        if has_data {
                            data.push('\n');
                        }
                        data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                        has_data = true;
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_reader_splits_events() {
        let input = "data: {\"a\":1}\r\n\r\ndata: {\"b\":\ndata: 2}\n\n: comment\n\ndata: {\"c\":3}";
        let events: Vec<String> = SseReader::new(input.as_bytes()).map(|r| r.unwrap()).collect();
        assert_eq!(events, vec!["{\"a\":1}", "{\"b\":\n2}", "{\"c\":3}"]);
    }

    #[test]
    fn api_error_parses_google_error_body() {
        let e = ApiError::from_body(
            404,
            r#"{"error":{"code":404,"message":"models/x is not found","status":"NOT_FOUND"}}"#,
        );
        assert_eq!(e.to_string(), "404 NOT_FOUND: models/x is not found");
        assert!(e.hint().unwrap().contains("/models"));
    }
}
