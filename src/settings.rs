//! Run settings: the right-hand panel of AI Studio, and how they map onto a
//! `generateContent` request body.

use std::collections::BTreeMap;
use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const DEFAULT_MODEL: &str = "gemini-3.8-flash";
pub const THINKING_LEVELS: [&str; 4] = ["minimal", "low", "medium", "high"];
pub const MEDIA_RESOLUTIONS: [&str; 4] = ["default", "low", "medium", "high"];

/// (short name, API category)
pub const SAFETY_CATEGORIES: [(&str, &str); 5] = [
    ("harassment", "HARM_CATEGORY_HARASSMENT"),
    ("hate", "HARM_CATEGORY_HATE_SPEECH"),
    ("sexual", "HARM_CATEGORY_SEXUALLY_EXPLICIT"),
    ("dangerous", "HARM_CATEGORY_DANGEROUS_CONTENT"),
    ("civic", "HARM_CATEGORY_CIVIC_INTEGRITY"),
];

/// (short name, API threshold)
pub const SAFETY_THRESHOLDS: [(&str, &str); 5] = [
    ("off", "OFF"),
    ("none", "BLOCK_NONE"),
    ("high", "BLOCK_ONLY_HIGH"),
    ("medium", "BLOCK_MEDIUM_AND_ABOVE"),
    ("low", "BLOCK_LOW_AND_ABOVE"),
];

/// Settable keys, for `/set` help and completion: (key, value hint, description).
pub const KEYS: [(&str, &str, &str); 19] = [
    ("model", "<name>", "Model id, e.g. gemini-3.8-flash"),
    ("system", "<text>|clear", "System instructions"),
    (
        "thinking",
        "default|minimal|low|medium|high|off|dynamic|<budget>",
        "Thinking level, or a token budget for older models",
    ),
    ("thoughts", "on|off", "Request and show thought summaries"),
    ("temperature", "<0-2>|default", "Sampling temperature"),
    ("top_p", "<0-1>|default", "Nucleus sampling"),
    ("top_k", "<n>|default", "Top-k sampling"),
    ("output_length", "<tokens>|default", "Max output tokens"),
    ("stop", "<text>|clear|[json list]", "Add a stop sequence (max 5)"),
    ("media_resolution", "default|low|medium|high", "Resolution for image/video/PDF input"),
    ("search", "on|off", "Grounding with Google Search"),
    ("maps", "on|off", "Grounding with Google Maps"),
    ("url_context", "on|off", "URL context"),
    ("code_execution", "on|off", "Code execution"),
    ("structured_output", "on|off", "JSON output (application/json)"),
    ("schema", "<file.json>|<json>|clear", "JSON schema for structured output"),
    ("function_calling", "on|off", "Function calling"),
    ("functions", "<file.json>|<json>|clear", "Function declarations"),
    ("safety", "<category|all> <off|none|high|medium|low|default>", "Safety settings"),
];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RunSettings {
    pub model: String,
    pub system_instruction: String,
    pub thinking_level: Option<String>,
    pub thinking_budget: Option<i64>,
    pub include_thoughts: bool,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub stop_sequences: Vec<String>,
    pub media_resolution: Option<String>,
    pub structured_output: bool,
    pub response_schema: Option<Value>,
    pub code_execution: bool,
    pub google_search: bool,
    pub google_maps: bool,
    pub url_context: bool,
    pub function_calling: bool,
    pub function_declarations: Vec<Value>,
    /// short category -> short threshold
    pub safety: BTreeMap<String, String>,
}

impl Default for RunSettings {
    fn default() -> Self {
        RunSettings {
            model: DEFAULT_MODEL.to_string(),
            system_instruction: String::new(),
            thinking_level: None,
            thinking_budget: None,
            include_thoughts: true,
            temperature: None,
            top_p: None,
            top_k: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            media_resolution: None,
            structured_output: false,
            response_schema: None,
            code_execution: false,
            google_search: true,
            google_maps: false,
            url_context: false,
            function_calling: false,
            function_declarations: Vec::new(),
            safety: BTreeMap::new(),
        }
    }
}

pub fn normalize_key(key: &str) -> String {
    let k = key.trim().trim_start_matches('/').to_lowercase().replace('-', "_");
    let canonical = match k.as_str() {
        "sys" | "system_instruction" | "system_instructions" | "instructions" => "system",
        "think" | "thinking_level" | "thinking_budget" | "effort" => "thinking",
        "thought" | "include_thoughts" | "show_thoughts" => "thoughts",
        "temp" => "temperature",
        "topp" => "top_p",
        "topk" => "top_k",
        "max_tokens" | "maxtokens" | "max_output_tokens" | "output" | "length" => "output_length",
        "stop_sequence" | "stop_sequences" => "stop",
        "media" | "resolution" => "media_resolution",
        "google_search" | "grounding" | "grounding_search" => "search",
        "google_maps" | "grounding_maps" => "maps",
        "url" | "urls" | "urlcontext" => "url_context",
        "code" | "codeexec" | "code_exec" | "exec" => "code_execution",
        "json" | "structured" | "structured_outputs" => "structured_output",
        "response_schema" | "json_schema" => "schema",
        "function" | "fc" | "tools_functions" => "function_calling",
        "function_declarations" | "funcs" | "decls" => "functions",
        "safety_settings" => "safety",
        other => other,
    };
    canonical.to_string()
}

pub fn parse_bool(value: &str, current: bool) -> Result<bool, String> {
    match value.trim().to_lowercase().as_str() {
        "" | "toggle" => Ok(!current),
        "on" | "true" | "1" | "yes" | "y" | "enable" | "enabled" => Ok(true),
        "off" | "false" | "0" | "no" | "n" | "disable" | "disabled" => Ok(false),
        other => Err(format!("expected on/off, got '{other}'")),
    }
}

fn is_default_word(v: &str) -> bool {
    matches!(v.trim().to_lowercase().as_str(), "" | "default" | "auto" | "none" | "unset" | "reset")
}

fn parse_opt_f64(v: &str, min: f64, max: f64) -> Result<Option<f64>, String> {
    if is_default_word(v) {
        return Ok(None);
    }
    let n: f64 = v.trim().parse().map_err(|_| format!("not a number: '{v}'"))?;
    if !(min..=max).contains(&n) {
        return Err(format!("must be between {min} and {max}"));
    }
    Ok(Some(n))
}

fn parse_opt_i64(v: &str, min: i64) -> Result<Option<i64>, String> {
    if is_default_word(v) {
        return Ok(None);
    }
    let n: i64 = v.trim().replace('_', "").parse().map_err(|_| format!("not an integer: '{v}'"))?;
    if n < min {
        return Err(format!("must be at least {min}"));
    }
    Ok(Some(n))
}

/// Accepts a path to a JSON file or inline JSON.
fn load_json_arg(v: &str) -> Result<Value, String> {
    let v = v.trim();
    if v.starts_with('{') || v.starts_with('[') {
        return serde_json::from_str(v).map_err(|e| format!("invalid JSON: {e}"));
    }
    let path = shellexpand_home(v);
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: invalid JSON: {e}"))
}

pub fn shellexpand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).display().to_string();
    }
    p.to_string()
}

/// Accepts a list of declarations, `{"functionDeclarations": [...]}`,
/// `{"tools": [{"functionDeclarations": [...]}]}` or a single declaration.
fn extract_function_declarations(v: Value) -> Result<Vec<Value>, String> {
    let list = match v {
        Value::Array(items) => items,
        Value::Object(ref obj) => {
            if let Some(Value::Array(items)) =
                obj.get("functionDeclarations").or_else(|| obj.get("function_declarations"))
            {
                items.clone()
            } else if let Some(Value::Array(tools)) = obj.get("tools") {
                tools
                    .iter()
                    .filter_map(|t| t.get("functionDeclarations").or_else(|| t.get("function_declarations")))
                    .filter_map(|d| d.as_array())
                    .flatten()
                    .cloned()
                    .collect()
            } else if obj.contains_key("name") {
                vec![v.clone()]
            } else {
                return Err("expected a list of function declarations".into());
            }
        }
        _ => return Err("expected a list of function declarations".into()),
    };
    for d in &list {
        if d.get("name").and_then(Value::as_str).is_none() {
            return Err(format!("function declaration without a name: {d}"));
        }
    }
    if list.is_empty() {
        return Err("no function declarations found".into());
    }
    Ok(list)
}

impl RunSettings {
    pub fn thinking_label(&self) -> String {
        if let Some(level) = &self.thinking_level {
            return level.clone();
        }
        match self.thinking_budget {
            None => "default".into(),
            Some(0) => "off".into(),
            Some(-1) => "dynamic".into(),
            Some(n) => format!("{n} tokens"),
        }
    }

    pub fn enabled_tools(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.google_search {
            out.push("search");
        }
        if self.google_maps {
            out.push("maps");
        }
        if self.url_context {
            out.push("url_context");
        }
        if self.code_execution {
            out.push("code_execution");
        }
        if self.function_calling {
            out.push("functions");
        }
        if self.structured_output {
            out.push("json");
        }
        out
    }

    /// Apply `key = value`. Returns a short confirmation for the user.
    pub fn apply(&mut self, key: &str, value: &str) -> Result<String, String> {
        let key = normalize_key(key);
        let value = value.trim();
        match key.as_str() {
            "model" => {
                let m = value.trim_start_matches("models/").trim();
                if m.is_empty() {
                    return Err("model name required".into());
                }
                self.model = m.to_string();
                Ok(format!("model = {}", self.model))
            }
            "system" => {
                if matches!(value.to_lowercase().as_str(), "clear" | "none" | "off" | "reset") {
                    self.system_instruction.clear();
                    Ok("system instructions cleared".into())
                } else {
                    self.system_instruction = value.to_string();
                    Ok(format!("system instructions set ({} chars)", value.chars().count()))
                }
            }
            "thinking" => {
                let v = value.to_lowercase();
                let v = v.trim_start_matches("budget").trim_start_matches(':').trim();
                if is_default_word(v) || v == "model" {
                    self.thinking_level = None;
                    self.thinking_budget = None;
                } else if THINKING_LEVELS.contains(&v) {
                    self.thinking_level = Some(v.to_string());
                    self.thinking_budget = None;
                } else if v == "off" || v == "disable" {
                    self.thinking_level = None;
                    self.thinking_budget = Some(0);
                } else if v == "dynamic" {
                    self.thinking_level = None;
                    self.thinking_budget = Some(-1);
                } else if let Ok(n) = v.replace('_', "").parse::<i64>() {
                    if n < -1 {
                        return Err("budget must be -1 (dynamic), 0 (off) or positive".into());
                    }
                    self.thinking_level = None;
                    self.thinking_budget = Some(n);
                } else {
                    return Err(format!(
                        "unknown thinking value '{value}' (use default, {}, off, dynamic or a token budget)",
                        THINKING_LEVELS.join(", ")
                    ));
                }
                Ok(format!("thinking = {}", self.thinking_label()))
            }
            "thoughts" => {
                self.include_thoughts = parse_bool(value, self.include_thoughts)?;
                Ok(format!("thought summaries {}", on_off(self.include_thoughts)))
            }
            "temperature" => {
                self.temperature = parse_opt_f64(value, 0.0, 2.0)?;
                Ok(format!("temperature = {}", opt(self.temperature)))
            }
            "top_p" => {
                self.top_p = parse_opt_f64(value, 0.0, 1.0)?;
                Ok(format!("top_p = {}", opt(self.top_p)))
            }
            "top_k" => {
                self.top_k = parse_opt_i64(value, 1)?;
                Ok(format!("top_k = {}", opt(self.top_k)))
            }
            "output_length" => {
                self.max_output_tokens = parse_opt_i64(value, 1)?;
                Ok(format!("output length = {}", opt(self.max_output_tokens)))
            }
            "stop" => {
                if matches!(value.to_lowercase().as_str(), "clear" | "none" | "reset") {
                    self.stop_sequences.clear();
                } else if value.starts_with('[') {
                    let list: Vec<String> =
                        serde_json::from_str(value).map_err(|e| format!("invalid JSON list: {e}"))?;
                    self.stop_sequences = list;
                } else if value.is_empty() {
                    return Err("give a stop sequence, a JSON list, or 'clear'".into());
                } else {
                    self.stop_sequences.push(value.to_string());
                }
                if self.stop_sequences.len() > 5 {
                    self.stop_sequences.truncate(5);
                    return Err("at most 5 stop sequences; extra ones dropped".into());
                }
                Ok(format!("stop sequences = {:?}", self.stop_sequences))
            }
            "media_resolution" => {
                let v = value.to_lowercase();
                let v = v.trim_start_matches("media_resolution_");
                if is_default_word(v) {
                    self.media_resolution = None;
                } else if MEDIA_RESOLUTIONS.contains(&v) {
                    self.media_resolution = Some(v.to_string());
                } else {
                    return Err("use default, low, medium or high".into());
                }
                Ok(format!("media resolution = {}", self.media_resolution.as_deref().unwrap_or("default")))
            }
            "search" => {
                self.google_search = parse_bool(value, self.google_search)?;
                Ok(format!("Grounding with Google Search {}", on_off(self.google_search)))
            }
            "maps" => {
                self.google_maps = parse_bool(value, self.google_maps)?;
                Ok(format!("Grounding with Google Maps {}", on_off(self.google_maps)))
            }
            "url_context" => {
                self.url_context = parse_bool(value, self.url_context)?;
                Ok(format!("URL context {}", on_off(self.url_context)))
            }
            "code_execution" => {
                self.code_execution = parse_bool(value, self.code_execution)?;
                Ok(format!("code execution {}", on_off(self.code_execution)))
            }
            "structured_output" => {
                self.structured_output = parse_bool(value, self.structured_output)?;
                let schema =
                    if self.structured_output && self.response_schema.is_some() { " (with schema)" } else { "" };
                Ok(format!("structured output {}{schema}", on_off(self.structured_output)))
            }
            "schema" => {
                if matches!(value.to_lowercase().as_str(), "clear" | "off" | "none" | "reset") {
                    self.response_schema = None;
                    self.structured_output = false;
                    return Ok("schema cleared, structured output off".into());
                }
                let schema = load_json_arg(value)?;
                if !schema.is_object() {
                    return Err("schema must be a JSON object".into());
                }
                self.response_schema = Some(schema);
                self.structured_output = true;
                Ok("schema set, structured output on".into())
            }
            "function_calling" => {
                let on = parse_bool(value, self.function_calling)?;
                if on && self.function_declarations.is_empty() {
                    return Err("load declarations first: /set functions <file.json>".into());
                }
                self.function_calling = on;
                Ok(format!("function calling {}", on_off(on)))
            }
            "functions" => {
                if matches!(value.to_lowercase().as_str(), "clear" | "off" | "none" | "reset") {
                    self.function_declarations.clear();
                    self.function_calling = false;
                    return Ok("function declarations cleared, function calling off".into());
                }
                let decls = extract_function_declarations(load_json_arg(value)?)?;
                let names: Vec<&str> = decls.iter().filter_map(|d| d.get("name").and_then(Value::as_str)).collect();
                let msg = format!("function calling on: {}", names.join(", "));
                self.function_declarations = decls;
                self.function_calling = true;
                Ok(msg)
            }
            "safety" => self.apply_safety(value),
            "" => Err("missing setting name".into()),
            other => Err(format!("unknown setting '{other}' (see /set)")),
        }
    }

    fn apply_safety(&mut self, value: &str) -> Result<String, String> {
        let mut it = value.split_whitespace();
        let (cat, thr) = match (it.next(), it.next()) {
            (Some(c), Some(t)) => (c.to_lowercase(), t.to_lowercase()),
            _ => return Err("usage: safety <category|all> <off|none|high|medium|low|default>".into()),
        };
        let cats: Vec<&str> = if cat == "all" {
            SAFETY_CATEGORIES.iter().map(|(c, _)| *c).collect()
        } else {
            let found = SAFETY_CATEGORIES
                .iter()
                .find(|(short, full)| *short == cat || full.eq_ignore_ascii_case(&cat))
                .map(|(short, _)| *short)
                .ok_or_else(|| {
                    let names: Vec<&str> = SAFETY_CATEGORIES.iter().map(|(c, _)| *c).collect();
                    format!("unknown category '{cat}' (use {} or all)", names.join(", "))
                })?;
            vec![found]
        };
        let thr_short = if matches!(thr.as_str(), "default" | "reset" | "unset") {
            None
        } else {
            Some(
                SAFETY_THRESHOLDS
                    .iter()
                    .find(|(short, full)| *short == thr || full.eq_ignore_ascii_case(&thr))
                    .map(|(short, _)| short.to_string())
                    .ok_or_else(|| format!("unknown threshold '{thr}'"))?,
            )
        };
        for c in &cats {
            match &thr_short {
                Some(t) => {
                    self.safety.insert(c.to_string(), t.clone());
                }
                None => {
                    self.safety.remove(*c);
                }
            }
        }
        Ok(format!("safety {} = {}", cat, thr_short.as_deref().unwrap_or("default")))
    }

    /// Build the JSON body for generateContent / streamGenerateContent.
    /// `supports_thinking` comes from the model list when known; `Some(false)`
    /// drops thinkingConfig so non-thinking models don't reject the request.
    pub fn build_body(&self, contents: &[Value], supports_thinking: Option<bool>) -> Value {
        let mut body = Map::new();
        body.insert("contents".into(), Value::Array(contents.to_vec()));

        if !self.system_instruction.trim().is_empty() {
            body.insert("systemInstruction".into(), json!({ "parts": [ { "text": self.system_instruction } ] }));
        }

        let mut gc = Map::new();
        if let Some(t) = self.temperature {
            gc.insert("temperature".into(), json!(t));
        }
        if let Some(p) = self.top_p {
            gc.insert("topP".into(), json!(p));
        }
        if let Some(k) = self.top_k {
            gc.insert("topK".into(), json!(k));
        }
        if let Some(m) = self.max_output_tokens {
            gc.insert("maxOutputTokens".into(), json!(m));
        }
        if !self.stop_sequences.is_empty() {
            gc.insert("stopSequences".into(), json!(self.stop_sequences));
        }
        if supports_thinking != Some(false) {
            let mut tc = Map::new();
            if let Some(level) = &self.thinking_level {
                tc.insert("thinkingLevel".into(), json!(level));
            } else if let Some(budget) = self.thinking_budget {
                tc.insert("thinkingBudget".into(), json!(budget));
            }
            if self.include_thoughts && self.thinking_budget != Some(0) {
                tc.insert("includeThoughts".into(), json!(true));
            }
            if !tc.is_empty() {
                gc.insert("thinkingConfig".into(), Value::Object(tc));
            }
        }
        if let Some(res) = &self.media_resolution {
            gc.insert("mediaResolution".into(), json!(format!("MEDIA_RESOLUTION_{}", res.to_uppercase())));
        }
        if self.structured_output {
            gc.insert("responseMimeType".into(), json!("application/json"));
            if let Some(schema) = &self.response_schema {
                gc.insert("responseJsonSchema".into(), schema.clone());
            }
        }
        if !gc.is_empty() {
            body.insert("generationConfig".into(), Value::Object(gc));
        }

        let mut tools = Vec::new();
        if self.google_search {
            tools.push(json!({ "googleSearch": {} }));
        }
        if self.google_maps {
            tools.push(json!({ "googleMaps": {} }));
        }
        if self.url_context {
            tools.push(json!({ "urlContext": {} }));
        }
        if self.code_execution {
            tools.push(json!({ "codeExecution": {} }));
        }
        if self.function_calling && !self.function_declarations.is_empty() {
            tools.push(json!({ "functionDeclarations": self.function_declarations }));
        }
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
        }

        if !self.safety.is_empty() {
            let list: Vec<Value> = SAFETY_CATEGORIES
                .iter()
                .filter_map(|(short, full)| {
                    let thr = self.safety.get(*short)?;
                    let api_thr = SAFETY_THRESHOLDS.iter().find(|(s, _)| s == thr)?.1;
                    Some(json!({ "category": full, "threshold": api_thr }))
                })
                .collect();
            body.insert("safetySettings".into(), Value::Array(list));
        }

        Value::Object(body)
    }

    /// Rows for the settings table: (section, label, value).
    pub fn describe(&self) -> Vec<(&'static str, &'static str, String)> {
        let sys = if self.system_instruction.is_empty() {
            "(none)".to_string()
        } else {
            truncate(&self.system_instruction.replace('\n', " "), 60)
        };
        let schema = match &self.response_schema {
            Some(_) if self.structured_output => "on (schema)".to_string(),
            None if self.structured_output => "on (no schema)".to_string(),
            Some(_) => "off (schema loaded)".to_string(),
            None => "off".to_string(),
        };
        let funcs = if self.function_declarations.is_empty() {
            on_off(self.function_calling).to_string()
        } else {
            let names: Vec<&str> =
                self.function_declarations.iter().filter_map(|d| d.get("name").and_then(Value::as_str)).collect();
            format!("{} ({})", on_off(self.function_calling), names.join(", "))
        };
        let safety = if self.safety.is_empty() {
            "default".to_string()
        } else {
            self.safety.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")
        };
        let stops =
            if self.stop_sequences.is_empty() { "(none)".to_string() } else { format!("{:?}", self.stop_sequences) };
        vec![
            ("Model", "model", self.model.clone()),
            ("Model", "system", sys),
            ("Model", "thinking", self.thinking_label()),
            ("Model", "thoughts", on_off(self.include_thoughts).to_string()),
            ("Tools", "structured_output", schema),
            ("Tools", "code_execution", on_off(self.code_execution).to_string()),
            ("Tools", "function_calling", funcs),
            ("Tools", "search", on_off(self.google_search).to_string()),
            ("Tools", "maps", on_off(self.google_maps).to_string()),
            ("Tools", "url_context", on_off(self.url_context).to_string()),
            ("Advanced", "media_resolution", self.media_resolution.clone().unwrap_or_else(|| "default".into())),
            ("Advanced", "safety", safety),
            ("Advanced", "stop", stops),
            ("Advanced", "output_length", opt(self.max_output_tokens)),
            ("Advanced", "temperature", opt(self.temperature)),
            ("Advanced", "top_p", opt(self.top_p)),
            ("Advanced", "top_k", opt(self.top_k)),
        ]
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "default".into())
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_body_has_search_and_contents() {
        let s = RunSettings::default();
        let contents = vec![json!({"role": "user", "parts": [{"text": "hi"}]})];
        let body = s.build_body(&contents, None);
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(body["tools"], json!([{ "googleSearch": {} }]));
        assert_eq!(body["generationConfig"]["thinkingConfig"], json!({ "includeThoughts": true }));
        assert!(body.get("systemInstruction").is_none());
    }

    #[test]
    fn full_settings_map_to_api_fields() {
        let mut s = RunSettings::default();
        for (k, v) in [
            ("model", "models/gemini-2.5-pro"),
            ("system", "Be terse"),
            ("thinking", "high"),
            ("temp", "0.7"),
            ("top-p", "0.9"),
            ("topk", "40"),
            ("max_tokens", "65536"),
            ("stop", "END"),
            ("media", "low"),
            ("search", "off"),
            ("maps", "on"),
            ("url", "on"),
            ("code", "on"),
            ("schema", r#"{"type":"object","properties":{"a":{"type":"string"}}}"#),
            ("functions", r#"[{"name":"get_weather","parameters":{"type":"object"}}]"#),
            ("safety", "hate none"),
        ] {
            s.apply(k, v).unwrap_or_else(|e| panic!("{k}: {e}"));
        }
        assert_eq!(s.model, "gemini-2.5-pro");
        let body = s.build_body(&[], Some(true));
        let gc = &body["generationConfig"];
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be terse");
        assert_eq!(gc["thinkingConfig"], json!({"thinkingLevel": "high", "includeThoughts": true}));
        assert_eq!(gc["temperature"], json!(0.7));
        assert_eq!(gc["topP"], json!(0.9));
        assert_eq!(gc["topK"], json!(40));
        assert_eq!(gc["maxOutputTokens"], json!(65536));
        assert_eq!(gc["stopSequences"], json!(["END"]));
        assert_eq!(gc["mediaResolution"], "MEDIA_RESOLUTION_LOW");
        assert_eq!(gc["responseMimeType"], "application/json");
        assert_eq!(gc["responseJsonSchema"]["type"], "object");
        assert_eq!(
            body["tools"],
            json!([
                {"googleMaps": {}},
                {"urlContext": {}},
                {"codeExecution": {}},
                {"functionDeclarations": [{"name": "get_weather", "parameters": {"type": "object"}}]}
            ])
        );
        assert_eq!(
            body["safetySettings"],
            json!([{"category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "BLOCK_NONE"}])
        );
    }

    #[test]
    fn thinking_budget_and_level_are_exclusive() {
        let mut s = RunSettings::default();
        s.apply("thinking", "low").unwrap();
        s.apply("thinking", "2048").unwrap();
        assert_eq!(s.thinking_level, None);
        assert_eq!(s.thinking_budget, Some(2048));
        let tc = &s.build_body(&[], None)["generationConfig"]["thinkingConfig"];
        assert_eq!(tc, &json!({"thinkingBudget": 2048, "includeThoughts": true}));

        s.apply("thinking", "off").unwrap();
        let tc = &s.build_body(&[], None)["generationConfig"]["thinkingConfig"];
        assert_eq!(tc, &json!({"thinkingBudget": 0}));

        s.apply("thinking", "medium").unwrap();
        assert_eq!(s.thinking_budget, None);
        assert!(s.apply("thinking", "ultra").is_err());
    }

    #[test]
    fn non_thinking_model_gets_no_thinking_config() {
        let mut s = RunSettings::default();
        s.apply("thinking", "high").unwrap();
        let body = s.build_body(&[], Some(false));
        assert!(body["generationConfig"].get("thinkingConfig").is_none());
    }

    #[test]
    fn function_calling_requires_declarations() {
        let mut s = RunSettings::default();
        assert!(s.apply("function_calling", "on").is_err());
        s.apply("functions", r#"{"tools":[{"functionDeclarations":[{"name":"f"}]}]}"#).unwrap();
        assert!(s.function_calling);
        s.apply("functions", "clear").unwrap();
        assert!(!s.function_calling);
    }

    #[test]
    fn safety_all_and_reset() {
        let mut s = RunSettings::default();
        s.apply("safety", "all off").unwrap();
        assert_eq!(s.safety.len(), SAFETY_CATEGORIES.len());
        s.apply("safety", "civic default").unwrap();
        assert_eq!(s.safety.len(), SAFETY_CATEGORIES.len() - 1);
        assert!(s.apply("safety", "nope off").is_err());
    }

    #[test]
    fn settings_roundtrip_and_tolerate_unknown_fields() {
        let s: RunSettings = serde_json::from_str(r#"{"model":"x","future_field":1}"#).unwrap();
        assert_eq!(s.model, "x");
        assert!(s.google_search);
    }
}
