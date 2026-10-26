//! Chat history, response accumulation and saved chats.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::config;
use crate::settings::{RunSettings, truncate};

const TEXT_PART_KEYS: [&str; 3] = ["text", "thought", "thoughtSignature"];

fn is_plain_text_part(p: &Map<String, Value>) -> bool {
    p.contains_key("text") && p.keys().all(|k| TEXT_PART_KEYS.contains(&k.as_str()))
}

/// Append a streamed part, merging consecutive text chunks of the same kind
/// so the stored history looks like a non-streamed response. A thought
/// signature (sent at the end of a text run) stays attached to its run.
pub fn merge_part(parts: &mut Vec<Value>, new: &Value) {
    if let (Some(last), Some(new_obj)) = (parts.last_mut().and_then(Value::as_object_mut), new.as_object()) {
        let same_kind = last.get("thought").and_then(Value::as_bool).unwrap_or(false)
            == new_obj.get("thought").and_then(Value::as_bool).unwrap_or(false);
        if is_plain_text_part(new_obj)
            && is_plain_text_part(last)
            && same_kind
            && !last.contains_key("thoughtSignature")
        {
            let add = new_obj.get("text").and_then(Value::as_str).unwrap_or_default();
            let joined = format!("{}{}", last.get("text").and_then(Value::as_str).unwrap_or_default(), add);
            last.insert("text".into(), Value::String(joined));
            if let Some(sig) = new_obj.get("thoughtSignature") {
                last.insert("thoughtSignature".into(), sig.clone());
            }
            return;
        }
    }
    parts.push(new.clone());
}

/// Everything collected from one streamed model response.
#[derive(Default, Debug)]
pub struct TurnResult {
    pub parts: Vec<Value>,
    pub grounding: Option<Value>,
    pub url_context: Option<Value>,
    pub usage: Option<Value>,
    pub finish_reason: Option<String>,
    pub finish_message: Option<String>,
    pub block_reason: Option<String>,
    pub model_version: Option<String>,
    pub interrupted: bool,
}

impl TurnResult {
    /// Absorb one stream chunk; returns the new raw parts for display.
    pub fn feed(&mut self, chunk: &Value) -> Vec<Value> {
        if let Some(v) = chunk.get("usageMetadata") {
            self.usage = Some(v.clone());
        }
        if let Some(v) = chunk.get("modelVersion").and_then(Value::as_str) {
            self.model_version = Some(v.to_string());
        }
        if let Some(fb) = chunk.get("promptFeedback") {
            if let Some(r) = fb.get("blockReason").and_then(Value::as_str) {
                self.block_reason = Some(r.to_string());
            }
            if let Some(m) = fb.get("blockReasonMessage").and_then(Value::as_str) {
                self.finish_message = Some(m.to_string());
            }
        }
        let Some(cand) = chunk.get("candidates").and_then(|c| c.get(0)) else {
            return Vec::new();
        };
        if let Some(r) = cand.get("finishReason").and_then(Value::as_str) {
            self.finish_reason = Some(r.to_string());
        }
        if let Some(m) = cand.get("finishMessage").and_then(Value::as_str) {
            self.finish_message = Some(m.to_string());
        }
        if let Some(g) = cand.get("groundingMetadata").filter(|g| !is_empty(g)) {
            self.grounding = Some(merge_grounding(self.grounding.take(), g));
        }
        if let Some(u) = cand.get("urlContextMetadata").filter(|g| !is_empty(g)) {
            self.url_context = Some(u.clone());
        }
        let new_parts: Vec<Value> =
            cand.get("content").and_then(|c| c.get("parts")).and_then(Value::as_array).cloned().unwrap_or_default();
        for p in &new_parts {
            merge_part(&mut self.parts, p);
        }
        new_parts
    }

    /// The answer text, without thoughts.
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter(|p| !p.get("thought").and_then(Value::as_bool).unwrap_or(false))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect()
    }

    pub fn function_calls(&self) -> Vec<Value> {
        self.parts.iter().filter_map(|p| p.get("functionCall").cloned()).collect()
    }

    /// Whether the response has anything besides thought summaries.
    pub fn has_content(&self) -> bool {
        self.parts.iter().any(|p| {
            let thought = p.get("thought").and_then(Value::as_bool).unwrap_or(false);
            let empty_text = p.get("text").and_then(Value::as_str).is_some_and(|t| t.is_empty())
                && p.as_object().is_some_and(|o| o.keys().all(|k| TEXT_PART_KEYS.contains(&k.as_str())));
            !thought && !empty_text
        })
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(o) => o.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// Grounding metadata can be split across chunks; keep the union of sources
/// and queries, and the latest of everything else.
fn merge_grounding(prev: Option<Value>, new: &Value) -> Value {
    let Some(mut prev) = prev else { return new.clone() };
    let (Some(p), Some(n)) = (prev.as_object_mut(), new.as_object()) else {
        return new.clone();
    };
    for (k, v) in n {
        match (k.as_str(), p.get_mut(k)) {
            ("groundingChunks" | "webSearchQueries", Some(Value::Array(existing))) => {
                if let Some(items) = v.as_array() {
                    for item in items {
                        if !existing.contains(item) {
                            existing.push(item.clone());
                        }
                    }
                }
            }
            _ => {
                p.insert(k.clone(), v.clone());
            }
        }
    }
    prev
}

pub fn user_text(text: &str) -> Value {
    json!({ "text": text })
}

/// Read a file into an inline-data part.
pub fn file_part(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    const LIMIT: usize = 20 * 1024 * 1024;
    if bytes.len() > LIMIT {
        return Err(format!("{} is larger than 20 MB (inline upload limit)", path.display()));
    }
    let mime = mime_guess::from_path(path).first_raw().map(str::to_string).unwrap_or_else(|| {
        if std::str::from_utf8(&bytes).is_ok() { "text/plain".into() } else { "application/octet-stream".into() }
    });
    // Source code and other text files are sent as text/plain, which every model accepts.
    let mime = if mime.starts_with("text/") || is_texty(&mime) { "text/plain".to_string() } else { mime };
    Ok(json!({
        "inlineData": {
            "mimeType": mime,
            "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
        }
    }))
}

fn is_texty(mime: &str) -> bool {
    matches!(
        mime,
        "application/json"
            | "application/xml"
            | "application/javascript"
            | "application/x-sh"
            | "application/toml"
            | "application/x-yaml"
            | "application/yaml"
            | "application/sql"
    )
}

/// Saved chat, like an entry in AI Studio's History.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub created: String,
    pub updated: String,
    pub settings: RunSettings,
    pub contents: Vec<Value>,
}

impl Chat {
    pub fn new(settings: &RunSettings) -> Chat {
        let now = chrono::Local::now();
        Chat {
            id: now.format("%Y%m%d-%H%M%S").to_string(),
            title: String::new(),
            created: now.to_rfc3339(),
            updated: now.to_rfc3339(),
            settings: settings.clone(),
            contents: Vec::new(),
        }
    }

    pub fn path(&self) -> PathBuf {
        config::chats_dir().join(format!("{}.json", self.id))
    }

    pub fn save(&mut self, settings: &RunSettings) -> std::io::Result<()> {
        if self.contents.is_empty() {
            // Everything was undone: drop the saved file instead of keeping stale turns.
            self.title.clear();
            let path = self.path();
            if path.exists() && self.owns(&path) {
                fs::remove_file(path)?;
            }
            return Ok(());
        }
        if self.title.is_empty() {
            self.title = self.first_user_text().map(|t| truncate(&t.replace('\n', " "), 60)).unwrap_or_default();
        }
        self.updated = chrono::Local::now().to_rfc3339();
        self.settings = settings.clone();
        fs::create_dir_all(config::chats_dir())?;
        // Avoid clobbering another chat that started in the same second.
        let mut path = self.path();
        if path.exists() && !self.owns(&path) {
            let mut n = 2;
            while config::chats_dir().join(format!("{}-{n}.json", self.id)).exists() {
                n += 1;
            }
            self.id = format!("{}-{n}", self.id);
            path = self.path();
        }
        fs::write(&path, serde_json::to_string(self)?)
    }

    fn owns(&self, path: &Path) -> bool {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Chat>(&t).ok())
            .is_some_and(|c| c.created == self.created)
    }

    pub fn load(path: &Path) -> Result<Chat, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn first_user_text(&self) -> Option<String> {
        self.contents
            .iter()
            .filter(|c| c.get("role").and_then(Value::as_str) == Some("user"))
            .flat_map(|c| c.get("parts").and_then(Value::as_array).cloned().unwrap_or_default())
            .find_map(|p| p.get("text").and_then(Value::as_str).map(str::to_string))
    }

    pub fn turn_count(&self) -> usize {
        self.contents.iter().filter(|c| c.get("role").and_then(Value::as_str) == Some("model")).count()
    }

    /// Drop the last user prompt and everything after it (model turns and
    /// function-call round trips). Returns the removed prompt's parts.
    pub fn pop_exchange(&mut self) -> Option<Vec<Value>> {
        let idx = self.contents.iter().rposition(|c| {
            c.get("role").and_then(Value::as_str) == Some("user")
                && c.get("parts")
                    .and_then(Value::as_array)
                    .is_some_and(|ps| ps.iter().all(|p| p.get("functionResponse").is_none()))
        })?;
        let removed = self.contents.split_off(idx);
        removed[0].get("parts").and_then(Value::as_array).cloned()
    }

    /// Markdown transcript for /export.
    pub fn to_markdown(&self) -> String {
        let mut out = format!("# {}\n\n", if self.title.is_empty() { "Chat" } else { &self.title });
        out.push_str(&format!("_Model: {}_\n\n", self.settings.model));
        if !self.settings.system_instruction.is_empty() {
            out.push_str(&format!("**System instructions:**\n\n{}\n\n", self.settings.system_instruction));
        }
        for c in &self.contents {
            let role = c.get("role").and_then(Value::as_str).unwrap_or("user");
            let heading = if role == "model" { "Model" } else { "User" };
            out.push_str(&format!("## {heading}\n\n"));
            for p in c.get("parts").and_then(Value::as_array).into_iter().flatten() {
                if p.get("thought").and_then(Value::as_bool).unwrap_or(false) {
                    continue;
                }
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                    out.push_str("\n\n");
                } else if let Some(d) = p.get("inlineData") {
                    out.push_str(&format!(
                        "_[attachment: {}]_\n\n",
                        d.get("mimeType").and_then(Value::as_str).unwrap_or("file")
                    ));
                } else if let Some(code) = p.get("executableCode") {
                    out.push_str(&format!(
                        "```{}\n{}\n```\n\n",
                        code.get("language").and_then(Value::as_str).unwrap_or("").to_lowercase(),
                        code.get("code").and_then(Value::as_str).unwrap_or("").trim_end()
                    ));
                } else if let Some(r) = p.get("codeExecutionResult") {
                    out.push_str(&format!(
                        "```\n{}\n```\n\n",
                        r.get("output").and_then(Value::as_str).unwrap_or("").trim_end()
                    ));
                } else if let Some(f) = p.get("functionCall") {
                    out.push_str(&format!("_function call:_ `{f}`\n\n"));
                } else if let Some(f) = p.get("functionResponse") {
                    out.push_str(&format!("_function response:_ `{f}`\n\n"));
                }
            }
        }
        out
    }
}

/// Saved chats, newest first.
pub fn list_chats() -> Vec<(PathBuf, Chat)> {
    let mut out: Vec<(PathBuf, Chat)> = fs::read_dir(config::chats_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| Chat::load(&p).ok().map(|c| (p, c)))
        .collect();
    out.sort_by(|a, b| b.1.updated.cmp(&a.1.updated));
    out
}

/// Find a saved chat by list number (1 = newest), id, or path.
pub fn find_chat(selector: &str) -> Result<Chat, String> {
    let chats = list_chats();
    let sel = selector.trim();
    if sel.is_empty() || sel == "last" || sel == "latest" {
        return chats.into_iter().next().map(|(_, c)| c).ok_or_else(|| "no saved chats yet".into());
    }
    if let Ok(n) = sel.parse::<usize>()
        && n >= 1
        && n <= chats.len()
    {
        return Ok(chats[n - 1].1.clone());
    }
    if let Some((_, c)) = chats.iter().find(|(_, c)| c.id == sel) {
        return Ok(c.clone());
    }
    let path = Path::new(sel);
    if path.exists() {
        return Chat::load(path);
    }
    Err(format!("no saved chat '{sel}' (see /chats)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_streamed_text_and_keeps_signature() {
        let mut parts = Vec::new();
        merge_part(&mut parts, &json!({"text": "Let me ", "thought": true}));
        merge_part(&mut parts, &json!({"text": "think", "thought": true}));
        merge_part(&mut parts, &json!({"text": "Hel"}));
        merge_part(&mut parts, &json!({"text": "lo"}));
        merge_part(&mut parts, &json!({"text": "", "thoughtSignature": "sig"}));
        merge_part(&mut parts, &json!({"text": " again"}));
        merge_part(&mut parts, &json!({"functionCall": {"name": "f", "args": {}}}));
        assert_eq!(
            parts,
            vec![
                json!({"text": "Let me think", "thought": true}),
                json!({"text": "Hello", "thoughtSignature": "sig"}),
                json!({"text": " again"}),
                json!({"functionCall": {"name": "f", "args": {}}}),
            ]
        );
    }

    #[test]
    fn turn_result_collects_metadata() {
        let mut r = TurnResult::default();
        r.feed(&json!({"candidates": [{"content": {"parts": [{"text": "a", "thought": true}]}}]}));
        assert!(!r.has_content());
        r.feed(&json!({"candidates": [{"content": {"parts": [{"text": "Answer"}]},
            "groundingMetadata": {"groundingChunks": [{"web": {"uri": "u1", "title": "t1"}}], "webSearchQueries": ["q"]}}]}));
        r.feed(&json!({"candidates": [{"content": {"parts": [{"text": "!"}]}, "finishReason": "STOP",
            "groundingMetadata": {"groundingChunks": [{"web": {"uri": "u2", "title": "t2"}}]}}],
            "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2}}));
        assert!(r.has_content());
        assert_eq!(r.text(), "Answer!");
        assert_eq!(r.finish_reason.as_deref(), Some("STOP"));
        assert_eq!(r.grounding.as_ref().unwrap()["groundingChunks"].as_array().unwrap().len(), 2);
        assert_eq!(r.grounding.as_ref().unwrap()["webSearchQueries"], json!(["q"]));
        assert_eq!(r.usage.as_ref().unwrap()["promptTokenCount"], 3);
    }

    #[test]
    fn pop_exchange_removes_function_round_trips() {
        let mut chat = Chat::new(&RunSettings::default());
        chat.contents = vec![
            json!({"role": "user", "parts": [{"text": "first"}]}),
            json!({"role": "model", "parts": [{"text": "one"}]}),
            json!({"role": "user", "parts": [{"text": "second"}]}),
            json!({"role": "model", "parts": [{"functionCall": {"name": "f"}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"name": "f", "response": {}}}]}),
            json!({"role": "model", "parts": [{"text": "two"}]}),
        ];
        let removed = chat.pop_exchange().unwrap();
        assert_eq!(removed, vec![json!({"text": "second"})]);
        assert_eq!(chat.contents.len(), 2);
    }
}
