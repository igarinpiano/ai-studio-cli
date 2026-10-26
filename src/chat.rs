//! One request/response turn, shared by one-shot mode and the REPL.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::Value;

use crate::api::{ApiError, Client, StreamEvent};
use crate::config;
use crate::conversation::TurnResult;
use crate::render::{Event, Renderer, Spinner};
use crate::settings::RunSettings;

/// Set by the Ctrl+C handler while a response is streaming.
pub fn interrupt_flag() -> Arc<AtomicBool> {
    static FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    FLAG.get_or_init(|| {
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        let _ = ctrlc::set_handler(move || {
            if f.swap(true, Ordering::SeqCst) {
                // Second Ctrl+C before the first was handled: give up.
                eprintln!();
                std::process::exit(130);
            }
        });
        flag
    })
    .clone()
}

pub fn run_turn(
    client: &Client,
    settings: &RunSettings,
    contents: &[Value],
    supports_thinking: Option<bool>,
    pretty: bool,
) -> Result<TurnResult, ApiError> {
    let interrupt = interrupt_flag();
    interrupt.store(false, Ordering::SeqCst);

    let body = settings.build_body(contents, supports_thinking);
    let rx = client.stream_generate(&settings.model, body);
    let mut renderer = Renderer::new(pretty, settings.include_thoughts);
    let mut spinner = Spinner::new();
    let mut result = TurnResult::default();
    let started = Instant::now();
    let mut image_count = 0;

    loop {
        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(StreamEvent::Chunk(chunk)) => {
                let parts = result.feed(&chunk);
                if !parts.is_empty() {
                    spinner.clear();
                }
                for part in parts {
                    for ev in part_events(&part, &mut image_count) {
                        renderer.event(ev);
                    }
                }
            }
            Ok(StreamEvent::Error(e)) => {
                spinner.clear();
                if renderer.has_output() {
                    renderer.finish(&result, &settings.model, started.elapsed());
                }
                return Err(e);
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if interrupt.swap(false, Ordering::SeqCst) {
                    result.interrupted = true;
                    break;
                }
                if !renderer.has_output() {
                    let label = if result.parts.is_empty() { "Generating…" } else { "Working…" };
                    spinner.tick(label, started.elapsed());
                }
            }
        }
    }
    spinner.clear();
    renderer.finish(&result, &settings.model, started.elapsed());
    Ok(result)
}

fn part_events(part: &Value, image_count: &mut usize) -> Vec<Event> {
    let mut evs = Vec::new();
    if let Some(t) = part.get("text").and_then(Value::as_str)
        && !t.is_empty()
    {
        if part.get("thought").and_then(Value::as_bool).unwrap_or(false) {
            evs.push(Event::Thought(t.to_string()));
        } else {
            evs.push(Event::Text(t.to_string()));
        }
    }
    if let Some(c) = part.get("executableCode") {
        evs.push(Event::Code {
            lang: c.get("language").and_then(Value::as_str).unwrap_or("").to_lowercase(),
            code: c.get("code").and_then(Value::as_str).unwrap_or("").to_string(),
        });
    }
    if let Some(r) = part.get("codeExecutionResult") {
        evs.push(Event::CodeResult {
            outcome: r.get("outcome").and_then(Value::as_str).unwrap_or("").to_string(),
            output: r.get("output").and_then(Value::as_str).unwrap_or("").to_string(),
        });
    }
    if let Some(f) = part.get("functionCall") {
        evs.push(Event::FunctionCall {
            name: f.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
            args: f.get("args").cloned().unwrap_or(Value::Null),
        });
    }
    if let Some(d) = part.get("inlineData") {
        *image_count += 1;
        match save_inline_data(d, *image_count) {
            Ok(path) => evs.push(Event::Image { path }),
            Err(e) => evs.push(Event::Text(format!("\n[could not save generated file: {e}]\n"))),
        }
    }
    evs
}

fn save_inline_data(d: &Value, n: usize) -> Result<String, String> {
    let mime = d.get("mimeType").and_then(Value::as_str).unwrap_or("application/octet-stream");
    let data = d.get("data").and_then(Value::as_str).unwrap_or("");
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
    let ext = mime_guess::get_mime_extensions_str(mime)
        .and_then(|exts| {
            exts.iter().find(|e| ["png", "jpg", "jpeg", "webp", "gif", "wav", "mp3"].contains(e)).or(exts.first())
        })
        .copied()
        .unwrap_or("bin");
    let dir = config::images_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}-{n}.{ext}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}
