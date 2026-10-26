//! The web backend: drives the real AI Studio page in the user's own browser.

pub mod browser;
pub mod cdp;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

use crate::api::ModelInfo;
use crate::chat::interrupt_flag;
use crate::config;
use crate::conversation::TurnResult;
use crate::render::{Event, Renderer, Spinner};
use crate::settings::RunSettings;
use browser::{Browser, LaunchOptions};
use cdp::Cdp;

const HOME_URL: &str = "https://aistudio.google.com/prompts/new_chat";

/// The new-chat page (`AISTUDIO_WEB_URL` overrides it, e.g. for testing).
pub fn home_url() -> String {
    std::env::var("AISTUDIO_WEB_URL").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| HOME_URL.to_string())
}
const AGENT_JS: &str = include_str!("agent.js");
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct WebOptions {
    pub browser: Option<String>,
    pub headless: bool,
}

pub struct WebSession {
    browser: Browser,
    cdp: Cdp,
    session: String,
    agent_src: String,
}

/// What the page reported for one turn.
pub struct WebTurn {
    pub result: TurnResult,
    pub text: String,
    pub url: String,
}

fn selector_overrides() -> String {
    let path = config::app_home().join("web-selectors.json");
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) => format!("window.__aiscliOverrides = {v};\n"),
            Err(e) => {
                crate::render::warn(&format!("ignoring {}: {e}", path.display()));
                String::new()
            }
        },
        Err(_) => String::new(),
    }
}

impl WebSession {
    pub fn start(opts: &WebOptions) -> Result<WebSession, String> {
        let executable = browser::find_browser(opts.browser.as_deref())?;
        let browser = Browser::launch(&LaunchOptions {
            executable,
            headless: opts.headless,
            remote_debugging: true,
            url: "about:blank".into(),
        })?;
        let ws_url = browser.ws_url.clone().ok_or("the browser has no DevTools endpoint")?;
        let mut cdp = Cdp::connect(&ws_url).map_err(|e| e.to_string())?;
        let target = cdp
            .call(None, "Target.createTarget", json!({ "url": "about:blank" }), CALL_TIMEOUT)
            .map_err(|e| e.to_string())?;
        let target_id = target["targetId"].as_str().ok_or("no target id")?.to_string();
        let attached = cdp
            .call(None, "Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }), CALL_TIMEOUT)
            .map_err(|e| e.to_string())?;
        let session = attached["sessionId"].as_str().ok_or("no session id")?.to_string();
        let agent_src = format!("{}{}", selector_overrides(), AGENT_JS);
        let mut web = WebSession { browser, cdp, session, agent_src };
        web.cmd("Page.enable", json!({}))?;
        web.cmd("Page.addScriptToEvaluateOnNewDocument", json!({ "source": web.agent_src }))?;
        Ok(web)
    }

    fn cmd(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.cdp.call(Some(&self.session), method, params, CALL_TIMEOUT).map_err(|e| e.to_string())
    }

    fn evaluate(&mut self, expression: &str, by_value: bool) -> Result<Value, String> {
        let r = self.cmd(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": by_value, "awaitPromise": true }),
        )?;
        if let Some(ex) = r.get("exceptionDetails") {
            let msg = ex
                .pointer("/exception/description")
                .or_else(|| ex.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("script error");
            return Err(format!("page script failed: {}", msg.lines().next().unwrap_or(msg)));
        }
        Ok(r.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Call `window.__aiscli.<call>` in the page, injecting the agent if needed.
    fn agent(&mut self, call: &str) -> Result<Value, String> {
        let expr = format!(
            "(async () => {{ if (!window.__aiscli) return {{ __missing: true }}; return await window.__aiscli.{call}; }})()"
        );
        let mut v = self.evaluate(&expr, true)?;
        if v.pointer("/value/__missing").is_some() {
            let src = self.agent_src.clone();
            self.evaluate(&src, true)?;
            v = self.evaluate(&expr, true)?;
        }
        Ok(v.get("value").cloned().unwrap_or(Value::Null))
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), String> {
        self.cmd("Page.navigate", json!({ "url": url }))?;
        thread::sleep(Duration::from_millis(300));
        self.wait_ready(Duration::from_secs(60))
    }

    /// Wait until the prompt box is there. Fails clearly when signed out.
    pub fn wait_ready(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut last = Value::Null;
        while Instant::now() < deadline {
            if let Ok(state) = self.agent("ready()") {
                match state["state"].as_str() {
                    Some("ready") => return Ok(()),
                    Some("dialog") => {
                        // A dialog (terms, announcements) can sit on top of a usable page.
                        return Ok(());
                    }
                    Some("login") => {
                        return Err(
                            "Not signed in to Google AI Studio. Run `aistudio login` and sign in in the browser window."
                                .into(),
                        );
                    }
                    _ => last = state,
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
        Err(format!(
            "AI Studio did not finish loading (last state: {}). Try `aistudio login` (to check the browser window), or `aistudio debug` to capture the page.",
            last
        ))
    }

    pub fn dialog_text(&mut self) -> String {
        self.agent("snapshot(1e9)").ok().and_then(|v| v["dialog"].as_str().map(str::to_string)).unwrap_or_default()
    }

    // -- settings --------------------------------------------------------------

    /// The page's run settings, laid over `base`.
    pub fn read_settings(&mut self, base: &RunSettings) -> RunSettings {
        let mut s = base.clone();
        let Ok(v) = self.agent("readSettings()") else { return s };
        if let Some(m) = v["model"].as_str() {
            s.model = m.to_string();
        }
        if let Some(t) = v["thinking"].as_str() {
            let t = t.to_lowercase();
            if crate::settings::THINKING_LEVELS.contains(&t.as_str()) {
                s.thinking_level = Some(t);
                s.thinking_budget = None;
            }
        }
        if let Some(m) = v["media_resolution"].as_str() {
            let m = m.to_lowercase();
            s.media_resolution = if m == "default" || m.is_empty() { None } else { Some(m) };
        }
        let flags: [(&str, &mut bool); 6] = [
            ("search", &mut s.google_search),
            ("maps", &mut s.google_maps),
            ("url_context", &mut s.url_context),
            ("code_execution", &mut s.code_execution),
            ("structured_output", &mut s.structured_output),
            ("function_calling", &mut s.function_calling),
        ];
        for (key, slot) in flags {
            if let Some(b) = v[key].as_bool() {
                *slot = b;
            }
        }
        if let Some(t) = v["temperature"].as_f64() {
            s.temperature = Some(t);
        }
        if let Some(t) = v["top_p"].as_f64() {
            s.top_p = Some(t);
        }
        if let Some(t) = v["output_length"].as_f64() {
            s.max_output_tokens = Some(t as i64);
        }
        if let Some(t) = v["system"].as_str() {
            s.system_instruction = t.to_string();
        }
        s
    }

    /// Change the page's settings from `current` to `desired`. Returns warnings
    /// for settings that could not be applied.
    pub fn apply_settings(&mut self, current: &RunSettings, desired: &RunSettings) -> Vec<String> {
        let mut warnings = Vec::new();
        let run = |web: &mut WebSession, warnings: &mut Vec<String>, what: &str, call: String| match web.agent(&call) {
            Ok(v) if v["ok"].as_bool() == Some(true) => {}
            Ok(v) => warnings.push(format!("{what}: {}", v["error"].as_str().unwrap_or("not applied"))),
            Err(e) => warnings.push(format!("{what}: {e}")),
        };
        let js = |s: &str| serde_json::to_string(s).unwrap_or_default();

        if desired.model != current.model {
            run(self, &mut warnings, "model", format!("selectModel({})", js(&desired.model)));
        }
        if desired.thinking_level != current.thinking_level
            && let Some(level) = &desired.thinking_level
        {
            let label = format!("{}{}", level[..1].to_uppercase(), &level[1..]);
            run(self, &mut warnings, "thinking", format!("setSelect('thinking', {})", js(&label)));
        }
        if desired.thinking_budget != current.thinking_budget && desired.thinking_budget.is_some() {
            warnings.push("thinking: token budgets are not set from the web UI here; use a level".into());
        }
        if desired.system_instruction != current.system_instruction {
            run(self, &mut warnings, "system instructions", format!("setSystem({})", js(&desired.system_instruction)));
        }
        let toggles = [
            ("search", desired.google_search, current.google_search),
            ("maps", desired.google_maps, current.google_maps),
            ("url_context", desired.url_context, current.url_context),
            ("code_execution", desired.code_execution, current.code_execution),
            ("structured_output", desired.structured_output, current.structured_output),
            ("function_calling", desired.function_calling, current.function_calling),
        ];
        for (key, want, have) in toggles {
            if want != have {
                run(self, &mut warnings, key, format!("setToggle('{key}', {want})"));
            }
        }
        if desired.media_resolution != current.media_resolution {
            let label = match desired.media_resolution.as_deref() {
                None => "Default".to_string(),
                Some(m) => format!("{}{}", m[..1].to_uppercase(), &m[1..]),
            };
            run(self, &mut warnings, "media resolution", format!("setSelect('media', {})", js(&label)));
        }
        let numbers = [
            ("temperature", desired.temperature, current.temperature),
            ("top_p", desired.top_p, current.top_p),
            ("output_length", desired.max_output_tokens.map(|v| v as f64), current.max_output_tokens.map(|v| v as f64)),
        ];
        for (key, want, have) in numbers {
            if want != have
                && let Some(v) = want
            {
                run(self, &mut warnings, key, format!("setNumber('{key}', {v})"));
            }
        }
        for stop in desired.stop_sequences.iter().filter(|s| !current.stop_sequences.contains(s)) {
            run(self, &mut warnings, "stop sequence", format!("addStop({})", js(stop)));
        }
        if desired.top_k != current.top_k {
            warnings.push("top_k: not offered by the AI Studio page".into());
        }
        if desired.safety != current.safety {
            warnings.push("safety settings: change them in the browser (aistudio login) for now".into());
        }
        if desired.response_schema != current.response_schema && desired.response_schema.is_some() {
            warnings.push("schema: edit the structured output schema in the browser for now".into());
        }
        if desired.function_declarations != current.function_declarations && !desired.function_declarations.is_empty() {
            warnings.push("functions: edit function declarations in the browser for now".into());
        }
        warnings
    }

    pub fn list_models(&mut self) -> Result<Vec<ModelInfo>, String> {
        let v = self.agent("listModels()")?;
        if v["ok"].as_bool() != Some(true) {
            return Err(v["error"].as_str().unwrap_or("could not read the model list").to_string());
        }
        Ok(v["models"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| ModelInfo {
                name: format!("models/{}", m["id"].as_str().unwrap_or_default()),
                display_name: m["name"].as_str().unwrap_or_default().to_string(),
                description: m["description"].as_str().unwrap_or_default().to_string(),
                supported_generation_methods: vec!["generateContent".into()],
                ..Default::default()
            })
            .collect())
    }

    // -- attachments -----------------------------------------------------------

    pub fn attach(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let files: Vec<String> = paths
            .iter()
            .map(|p| fs::canonicalize(p).map(|c| c.display().to_string()).map_err(|e| format!("{}: {e}", p.display())))
            .collect::<Result<_, _>>()?;
        // Prefer a file input already in the page; otherwise open the upload
        // menu and catch the file chooser it opens.
        let input = self.evaluate("window.__aiscli && window.__aiscli.fileInput()", false)?;
        if let Some(object_id) = input.get("objectId").and_then(Value::as_str) {
            self.cmd("DOM.setFileInputFiles", json!({ "files": files, "objectId": object_id }))?;
        } else {
            self.cmd("Page.setInterceptFileChooserDialog", json!({ "enabled": true }))?;
            self.cdp.clear_events();
            let opened = self.agent("openUploadMenu()")?;
            if opened["ok"].as_bool() != Some(true) {
                let _ = self.cmd("Page.setInterceptFileChooserDialog", json!({ "enabled": false }));
                return Err(opened["error"].as_str().unwrap_or("could not open the upload menu").to_string());
            }
            let ev = self.cdp.wait_event("Page.fileChooserOpened", Duration::from_secs(10));
            let _ = self.cmd("Page.setInterceptFileChooserDialog", json!({ "enabled": false }));
            let ev = ev.map_err(|_| "AI Studio did not open a file chooser".to_string())?;
            let node = ev.pointer("/params/backendNodeId").cloned().ok_or("no file input")?;
            self.cmd("DOM.setFileInputFiles", json!({ "files": files, "backendNodeId": node }))?;
        }
        // Wait for the uploads to finish.
        let deadline = Instant::now() + Duration::from_secs(120);
        thread::sleep(Duration::from_millis(500));
        while Instant::now() < deadline {
            if self.agent("uploadsBusy()")?.as_bool() != Some(true) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(300));
        }
        Err("uploading the files took too long".into())
    }

    // -- chatting ----------------------------------------------------------------

    /// Type `text` into the prompt box, press Run, and stream the answer.
    pub fn send(&mut self, text: &str, pretty: bool, show_thoughts: bool, model: &str) -> Result<WebTurn, String> {
        let interrupt = interrupt_flag();
        interrupt.store(false, Ordering::SeqCst);
        self.wait_ready(Duration::from_secs(30))?;
        let before = self.agent("turnCount()")?.as_u64().unwrap_or(0);
        let errors_before: Vec<String> = self.agent("snapshot(1e9)")?["errors"]
            .as_array()
            .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
            .unwrap_or_default();

        if !text.is_empty() {
            if self.agent("focusPrompt()")?.as_bool() != Some(true) {
                return Err("could not find the prompt box".into());
            }
            self.cmd("Input.insertText", json!({ "text": text }))?;
            let typed = self.agent("promptValue()")?;
            if typed.as_str().map(|v| v.trim() != text.trim()).unwrap_or(true) {
                self.agent(&format!("setPrompt({})", serde_json::to_string(text).unwrap_or_default()))?;
            }
        }

        // Run becomes clickable once the prompt (and any upload) is accepted.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match self.agent("runState()")?.as_str() {
                Some("enabled") => break,
                Some("running") => break,
                _ if Instant::now() > deadline => {
                    let dialog = self.dialog_text();
                    let extra =
                        if dialog.is_empty() { String::new() } else { format!(" A dialog is open: \"{dialog}\"") };
                    return Err(format!("the Run button stayed disabled.{extra}"));
                }
                _ => thread::sleep(Duration::from_millis(150)),
            }
        }
        if self.agent("run()")?.as_bool() != Some(true) {
            return Err("could not press Run".into());
        }

        let mut renderer = Renderer::new(pretty, show_thoughts);
        let mut spinner = Spinner::new();
        let started = Instant::now();
        let mut printed_text = String::new();
        let mut printed_thoughts = String::new();
        let mut last_text = String::new();
        let mut stable = 0;
        let mut seen_start = false;
        let mut interrupted = false;
        let final_snapshot;
        let late_errors: Vec<String>;
        let mut diverged = false;

        loop {
            thread::sleep(Duration::from_millis(120));
            if interrupt.swap(false, Ordering::SeqCst) {
                let _ = self.agent("stop()");
                interrupted = true;
            }
            let snap = self.agent(&format!("snapshot({before})"))?;
            if std::env::var_os("AISTUDIO_DEBUG").is_some() {
                eprintln!(
                    "[debug] {}",
                    serde_json::to_string(&snap).unwrap_or_default().chars().take(400).collect::<String>()
                );
            }
            let generating = snap["generating"].as_bool().unwrap_or(false);
            let model_turn = snap["turns"]
                .as_array()
                .and_then(|t| t.iter().rev().find(|t| t["role"] == "model"))
                .cloned()
                .unwrap_or(Value::Null);
            let text_now = model_turn["text"].as_str().unwrap_or("").to_string();
            let thoughts_now = model_turn["thoughts"].as_str().unwrap_or("").to_string();
            if generating || !model_turn.is_null() {
                seen_start = true;
            }

            // Stream what is new. The last code fence is held back while
            // generating, since the code inside it is still growing.
            if thoughts_now.len() > printed_thoughts.len()
                && thoughts_now.starts_with(&printed_thoughts)
                && printed_text.is_empty()
            {
                spinner.clear();
                renderer.event(Event::Thought(thoughts_now[printed_thoughts.len()..].to_string()));
                printed_thoughts = thoughts_now.clone();
            }
            let streamable =
                if generating { stable_prefix(hold_back_open_fence(&text_now)) } else { text_now.as_str() };
            if streamable.len() > printed_text.len() {
                if streamable.starts_with(&printed_text) {
                    spinner.clear();
                    renderer.event(Event::Text(streamable[printed_text.len()..].to_string()));
                    printed_text = streamable.to_string();
                } else {
                    diverged = true;
                }
            }
            if !renderer.has_output() {
                spinner.tick(if seen_start { "Generating…" } else { "Waiting for AI Studio…" }, started.elapsed());
            }

            let new_errors: Vec<String> = snap["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e.as_str())
                .filter(|e| !errors_before.iter().any(|b| b == e))
                .map(str::to_string)
                .collect();

            stable = if text_now == last_text { stable + 1 } else { 0 };
            last_text = text_now.clone();
            if seen_start && !generating && (stable >= 3 || interrupted) {
                if text_now.is_empty() && !interrupted {
                    spinner.clear();
                    renderer.finish(&TurnResult::default(), model, started.elapsed());
                    return Err(if new_errors.is_empty() {
                        "AI Studio finished without an answer. Run `aistudio debug` to capture the page.".into()
                    } else {
                        format!("AI Studio reported: {}", new_errors.join(" / "))
                    });
                }
                late_errors = new_errors;
                final_snapshot = snap;
                break;
            }
            if !seen_start && !new_errors.is_empty() && started.elapsed() > Duration::from_secs(2) {
                spinner.clear();
                return Err(format!("AI Studio reported: {}", new_errors.join(" / ")));
            }
            if !seen_start && started.elapsed() > Duration::from_secs(90) {
                spinner.clear();
                let dialog = snap["dialog"].as_str().unwrap_or("");
                return Err(if dialog.is_empty() {
                    "AI Studio did not start generating. Run `aistudio debug` to capture the page.".into()
                } else {
                    format!("AI Studio did not start generating; a dialog is open: \"{dialog}\"")
                });
            }
        }
        spinner.clear();

        let model_turn = final_snapshot["turns"]
            .as_array()
            .and_then(|t| t.iter().rev().find(|t| t["role"] == "model"))
            .cloned()
            .unwrap_or(Value::Null);
        let final_text = model_turn["text"].as_str().unwrap_or("").to_string();
        if final_text.starts_with(&printed_text) && !diverged {
            if final_text.len() > printed_text.len() {
                renderer.event(Event::Text(final_text[printed_text.len()..].to_string()));
            }
        } else {
            // The page re-rendered text that was already printed: show the final version.
            renderer.event(Event::Text("\n\n".into()));
            renderer.event(Event::Text(final_text.clone()));
        }

        let sources: Vec<Value> = model_turn["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| json!({ "web": { "uri": s["uri"], "title": s["title"] } }))
            .collect();
        let mut result = TurnResult { interrupted, model_version: Some(model.to_string()), ..Default::default() };
        if !sources.is_empty() {
            result.grounding = Some(json!({ "groundingChunks": sources }));
        }
        if !final_text.is_empty() {
            result.parts.push(json!({ "text": final_text }));
        }
        renderer.finish(&result, model, started.elapsed());
        if !late_errors.is_empty() {
            crate::render::warn(&format!("AI Studio reported: {}", late_errors.join(" / ")));
        }
        Ok(WebTurn { result, text: final_text, url: final_snapshot["url"].as_str().unwrap_or("").to_string() })
    }

    pub fn new_chat(&mut self) -> Result<(), String> {
        self.navigate(&home_url())
    }

    /// Screenshot, page HTML and an outline of the page, for bug reports.
    pub fn debug_dump(&mut self) -> Result<PathBuf, String> {
        let dir = config::app_home().join("debug").join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let _ = self.agent("ready()");
        if let Ok(shot) = self.cmd("Page.captureScreenshot", json!({ "format": "png" }))
            && let Some(data) = shot["data"].as_str()
            && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data)
        {
            let _ = fs::write(dir.join("screenshot.png"), bytes);
        }
        let html = self.evaluate("document.documentElement.outerHTML", true)?;
        write_value(&dir.join("page.html"), &html)?;
        let outline = self.agent("outline()")?;
        write_value(&dir.join("outline.txt"), &json!({ "value": outline }))?;
        let mut state = json!({});
        state["ready"] = self.agent("ready()").unwrap_or(Value::Null);
        state["snapshot"] = self.agent("snapshot(0)").unwrap_or(Value::Null);
        state["settings"] = self.agent("readSettings()").unwrap_or(Value::Null);
        fs::write(dir.join("state.json"), serde_json::to_string_pretty(&state).unwrap_or_default())
            .map_err(|e| e.to_string())?;
        Ok(dir)
    }

    pub fn close(mut self) {
        let _ = self.cdp.call(None, "Browser.close", json!({}), Duration::from_secs(5));
        self.browser.shutdown();
    }
}

fn write_value(path: &Path, v: &Value) -> Result<(), String> {
    let text = v.get("value").and_then(Value::as_str).unwrap_or_default();
    fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// While generating, leave the last code block out of what gets printed: the
/// page shows it with its closing fence even while code is still arriving.
pub fn hold_back_open_fence(text: &str) -> &str {
    let mut fences = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fences.push(offset);
        }
        offset += line.len();
    }
    match fences.len() {
        n if n % 2 == 1 => &text[..fences[n - 1]],
        n if n >= 2 && text.trim_end().ends_with("```") => &text[..fences[n - 2]],
        _ => text,
    }
}

/// While generating, the end of the last line can still change (a `**` that
/// becomes bold, a link that gets its URL), so it is printed with a lag.
pub fn stable_prefix(text: &str) -> &str {
    const LAG: usize = 24;
    let line_start = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let tail = &text[line_start..];
    match tail.char_indices().rev().nth(LAG - 1) {
        // Long line: everything but the last LAG characters, cut at a space when possible.
        Some((i, _)) if tail.chars().count() > 2 * LAG => {
            let cut = tail[..i].rfind(' ').filter(|&sp| sp > i / 2).map(|sp| sp + 1).unwrap_or(i);
            &text[..line_start + cut]
        }
        _ => &text[..line_start],
    }
}

/// `aistudio login`: an ordinary browser window (no DevTools endpoint) to
/// sign in with. Returns when the user presses Enter or closes the browser.
pub fn interactive_login(opts: &WebOptions) -> Result<(), String> {
    let executable = browser::find_browser(opts.browser.as_deref())?;
    let mut b =
        Browser::launch(&LaunchOptions { executable, headless: false, remote_debugging: false, url: home_url() })?;
    crate::render::out(
        "A browser window opened with AI Studio.\n  1. Sign in with your Google account (and accept AI Studio's terms if asked).\n  2. When the chat screen shows, come back here and press Enter.\n",
    );
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = tx.send(());
    });
    loop {
        if rx.try_recv().is_ok() {
            break;
        }
        if let Some(child) = b.child.as_mut()
            && let Ok(Some(_)) = child.try_wait()
        {
            b.child = None;
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    // Close it gracefully so the sign-in cookies are written to disk.
    if let Some(child) = b.child.as_mut() {
        let pid = child.id().to_string();
        #[cfg(unix)]
        let _ = std::process::Command::new("kill").args(["-TERM", &pid]).status();
        #[cfg(windows)]
        let _ =
            std::process::Command::new("taskkill").args(["/PID", &pid]).stdout(std::process::Stdio::null()).status();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    b.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_fence_is_held_back() {
        assert_eq!(hold_back_open_fence("text\n```py\nprint(1)"), "text\n");
        assert_eq!(hold_back_open_fence("a\n```py\nx\n```\nb"), "a\n```py\nx\n```\nb");
        assert_eq!(hold_back_open_fence("a\n```py\nx\n```"), "a\n");
        assert_eq!(hold_back_open_fence("plain"), "plain");
    }

    #[test]
    fn last_line_is_printed_with_a_lag() {
        assert_eq!(stable_prefix("done line\npartial **bo"), "done line\n");
        let long = format!("x\n{}", "word ".repeat(20));
        let p = stable_prefix(&long);
        assert!(p.len() < long.len() && p.len() > 2 && p.ends_with(' '));
        let wide = "Grüße—naïve·café ".repeat(5).to_string();
        assert!(stable_prefix(&wide).chars().count() < wide.chars().count());
    }
}
