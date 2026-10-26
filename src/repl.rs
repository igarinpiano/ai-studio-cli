//! Interactive chat: the Playground screen of AI Studio, in a terminal.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use anstyle::AnsiColor;
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Cmd, Config as RlConfig, Context, Editor, Helper, KeyCode, KeyEvent, Modifiers};
use serde_json::{Value, json};

use crate::api::{ApiError, Client, ModelInfo};
use crate::chat::run_turn;
use crate::config::{self, Config};
use crate::conversation::{self, Chat, file_part, list_chats};
use crate::render::{self, MdStream, dim, fg, paint};
use crate::settings::{self, KEYS, RunSettings, SAFETY_CATEGORIES, THINKING_LEVELS, normalize_key, truncate};

/// (command, arguments, description)
const COMMANDS: &[(&str, &str, &str)] = &[
    ("/help", "", "Show this help"),
    ("/new", "", "Start a new chat (alias: /clear)"),
    ("/model", "[name|number]", "Switch model (no argument: pick from list)"),
    ("/models", "", "List models available to your API key"),
    ("/thinking", "[level|budget]", "Thinking level: default, minimal, low, medium, high, off, dynamic, <tokens>"),
    ("/thoughts", "[on|off]", "Show thought summaries"),
    ("/system", "[text|clear]", "System instructions (no argument: multi-line editor)"),
    ("/settings", "[save|reset]", "Show run settings; save them as defaults; or reset"),
    ("/set", "<key> <value>", "Change any run setting (no argument: list keys)"),
    ("/tools", "", "Show which tools are enabled"),
    ("/search", "[on|off]", "Grounding with Google Search"),
    ("/maps", "[on|off]", "Grounding with Google Maps"),
    ("/url", "[on|off]", "URL context"),
    ("/codeexec", "[on|off]", "Code execution"),
    ("/json", "[on|off]", "Structured output (JSON)"),
    ("/schema", "<file|json|clear>", "JSON schema for structured output"),
    ("/functions", "<file|json|clear>", "Function declarations for function calling"),
    ("/safety", "[category threshold]", "Safety settings, e.g. /safety all none"),
    ("/media", "<default|low|medium|high>", "Media resolution"),
    ("/temp", "<value|default>", "Temperature"),
    ("/maxtokens", "<n|default>", "Output length"),
    ("/stop", "<text|clear>", "Add a stop sequence"),
    ("/attach", "<file>...", "Attach files (images, PDF, audio, video, text) to the next message"),
    ("/detach", "", "Remove pending attachments"),
    ("/retry", "", "Regenerate the last response"),
    ("/undo", "", "Remove the last exchange"),
    ("/chats", "", "List saved chats"),
    ("/load", "<number|id>", "Open a saved chat"),
    ("/rename", "<title>", "Rename this chat"),
    ("/export", "<file.md|file.json>", "Export this chat"),
    ("/code", "", "Show the equivalent API request (Get code)"),
    ("/key", "", "Set the API key"),
    ("/render", "<markdown|raw>", "Output style"),
    ("/exit", "", "Quit (also Ctrl+D)"),
];

struct ReplHelper {
    models: Arc<Mutex<Option<Vec<ModelInfo>>>>,
    files: FilenameCompleter,
}

impl Helper for ReplHelper {}
impl Validator for ReplHelper {}
impl Hinter for ReplHelper {
    type Hint = String;
}
impl Highlighter for ReplHelper {}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize, ctx: &Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        let before = &line[..pos];
        if !before.starts_with('/') {
            return Ok((0, Vec::new()));
        }
        let pair = |s: &str| Pair { display: s.to_string(), replacement: s.to_string() };
        let Some(space) = before.find(' ') else {
            let mut out: Vec<Pair> = COMMANDS
                .iter()
                .map(|(c, _, _)| *c)
                .chain(["/clear", "/quit", "/topp", "/topk", "/thinking"])
                .filter(|c| c.starts_with(before))
                .map(pair)
                .collect();
            out.dedup_by(|a, b| a.display == b.display);
            return Ok((0, out));
        };
        let cmd = &before[..space];
        let arg_start = before.rfind(' ').map(|i| i + 1).unwrap_or(pos);
        let word = &before[arg_start..];
        let options: Vec<String> = match cmd {
            "/attach" | "/export" | "/schema" | "/functions" => return self.files.complete(line, pos, ctx),
            "/model" => self
                .models
                .lock()
                .ok()
                .and_then(|m| m.clone())
                .unwrap_or_default()
                .iter()
                .filter(|m| m.can_chat())
                .map(|m| m.id().to_string())
                .collect(),
            "/thinking" => {
                THINKING_LEVELS.iter().chain(["default", "off", "dynamic"].iter()).map(|s| s.to_string()).collect()
            }
            "/set" if arg_start == space + 1 => KEYS.iter().map(|(k, _, _)| k.to_string()).collect(),
            "/safety" if arg_start == space + 1 => {
                SAFETY_CATEGORIES.iter().map(|(c, _)| c.to_string()).chain(["all".to_string()]).collect()
            }
            "/safety" => ["off", "none", "high", "medium", "low", "default"].iter().map(|s| s.to_string()).collect(),
            "/media" => settings::MEDIA_RESOLUTIONS.iter().map(|s| s.to_string()).collect(),
            "/render" => vec!["markdown".into(), "raw".into()],
            "/settings" => vec!["save".into(), "reset".into()],
            "/load" => list_chats().into_iter().map(|(_, c)| c.id).collect(),
            _ => vec!["on".into(), "off".into()],
        };
        Ok((arg_start, options.iter().filter(|o| o.starts_with(word)).map(|o| pair(o)).collect()))
    }
}

pub struct Repl {
    client: Client,
    cfg: Config,
    settings: RunSettings,
    chat: Chat,
    pretty: bool,
    attachments: Vec<(String, Value)>,
    /// Prompt parts of a request that failed, for /retry.
    failed: Option<Vec<Value>>,
    models: Arc<Mutex<Option<Vec<ModelInfo>>>>,
    editor: Editor<ReplHelper, DefaultHistory>,
}

enum Flow {
    Continue,
    Exit,
}

impl Repl {
    pub fn new(
        client: Client,
        cfg: Config,
        settings: RunSettings,
        chat: Chat,
        pretty: bool,
    ) -> rustyline::Result<Repl> {
        let rl_cfg = RlConfig::builder()
            .auto_add_history(false)
            .completion_type(rustyline::CompletionType::List)
            .max_history_size(2000)?
            .build();
        let mut editor: Editor<ReplHelper, DefaultHistory> = Editor::with_config(rl_cfg)?;
        let models = Arc::new(Mutex::new(None));
        editor.set_helper(Some(ReplHelper { models: models.clone(), files: FilenameCompleter::new() }));
        // Alt+Enter / Ctrl+J insert a newline instead of sending.
        editor.bind_sequence(KeyEvent(KeyCode::Enter, Modifiers::ALT), Cmd::Newline);
        editor.bind_sequence(KeyEvent::ctrl('J'), Cmd::Newline);
        let _ = editor.load_history(&config::input_history_path());

        // Fetch the model list in the background: it feeds /model completion
        // and tells us which models accept thinking settings.
        let c = client.clone();
        let m = models.clone();
        thread::spawn(move || {
            if let Ok(list) = c.list_models()
                && let Ok(mut slot) = m.lock()
            {
                *slot = Some(list);
            }
        });

        Ok(Repl { client, cfg, settings, chat, pretty, attachments: Vec::new(), failed: None, models, editor })
    }

    /// Run the input loop. `show_history` prints the end of a resumed chat first.
    pub fn run(&mut self, show_history: bool) -> i32 {
        self.banner();
        if show_history && !self.chat.contents.is_empty() {
            self.print_transcript(2);
        }
        loop {
            let prompt = self.prompt_string();
            let styled = paint(fg(AnsiColor::BrightBlue).bold(), &prompt);
            match self.editor.readline(&(prompt.as_str(), styled.as_str())) {
                Ok(line) => {
                    let line = match self.read_continuation(line) {
                        Some(l) => l,
                        None => continue,
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let _ = self.editor.add_history_entry(line.as_str());
                    let flow = if line.trim_start().starts_with('/') {
                        self.command(line.trim())
                    } else {
                        self.send(line);
                        Flow::Continue
                    };
                    if let Flow::Exit = flow {
                        break;
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    render::info("(Ctrl+D or /exit to quit)");
                }
                Err(ReadlineError::Eof) => break,
                Err(e) => {
                    render::error(&format!("input error: {e}"));
                    break;
                }
            }
        }
        self.save_chat();
        if let Some(parent) = config::input_history_path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = self.editor.save_history(&config::input_history_path());
        0
    }

    fn prompt_string(&self) -> String {
        if self.attachments.is_empty() {
            "› ".to_string()
        } else {
            format!("[{} file{}] › ", self.attachments.len(), if self.attachments.len() == 1 { "" } else { "s" })
        }
    }

    /// `"""` starts a multi-line block (ended by `"""`); a trailing `\` continues the line.
    fn read_continuation(&mut self, first: String) -> Option<String> {
        if first.trim() == "\"\"\"" {
            let mut lines = Vec::new();
            loop {
                match self.editor.readline("… ") {
                    Ok(l) if l.trim() == "\"\"\"" => break,
                    Ok(l) => lines.push(l),
                    Err(ReadlineError::Interrupted) => return None,
                    Err(_) => break,
                }
            }
            return Some(lines.join("\n"));
        }
        let mut text = first;
        while text.ends_with('\\') {
            text.pop();
            match self.editor.readline("… ") {
                Ok(l) => {
                    text.push('\n');
                    text.push_str(&l);
                }
                Err(ReadlineError::Interrupted) => return None,
                Err(_) => break,
            }
        }
        Some(text)
    }

    fn banner(&self) {
        render::out(&format!(
            "{} {}\n",
            paint(fg(AnsiColor::BrightBlue).bold(), "Google AI Studio CLI"),
            paint(dim(), env!("CARGO_PKG_VERSION"))
        ));
        self.status_line();
        render::info(
            "/help for commands · \"\"\" for multi-line input (or Alt+Enter) · Ctrl+C stops a response · Ctrl+D quits",
        );
        render::out("\n");
    }

    fn status_line(&self) {
        let tools = self.settings.enabled_tools();
        let mut s = format!(
            "{} · thinking: {}",
            paint(fg(AnsiColor::Cyan).bold(), &self.settings.model),
            self.settings.thinking_label()
        );
        if !tools.is_empty() {
            s.push_str(&format!(" · tools: {}", tools.join(", ")));
        }
        if !self.settings.system_instruction.is_empty() {
            s.push_str(" · system instructions set");
        }
        render::out(&format!("{s}\n"));
    }

    fn supports_thinking(&self) -> Option<bool> {
        let guard = self.models.lock().ok()?;
        let list = guard.as_ref()?;
        list.iter().find(|m| m.id() == self.settings.model).and_then(|m| m.thinking)
    }

    // -- chatting ---------------------------------------------------------

    fn send(&mut self, text: String) {
        let mut parts: Vec<Value> = self.attachments.drain(..).map(|(_, p)| p).collect();
        if !text.trim().is_empty() {
            parts.push(conversation::user_text(&text));
        }
        self.failed = None;
        self.chat.contents.push(json!({ "role": "user", "parts": parts }));
        self.generate();
    }

    fn generate(&mut self) {
        loop {
            let result =
                run_turn(&self.client, &self.settings, &self.chat.contents, self.supports_thinking(), self.pretty);
            let result = match result {
                Ok(r) => r,
                Err(e) => {
                    self.report_api_error(&e);
                    self.failed = self.chat.pop_exchange();
                    render::info("Your message was kept: /retry to send it again.");
                    return;
                }
            };
            if !result.has_content() {
                self.failed = self.chat.pop_exchange();
                if !result.interrupted && result.block_reason.is_none() {
                    render::warn("The model returned no content. /retry to try again.");
                }
                return;
            }
            self.chat.contents.push(json!({ "role": "model", "parts": result.parts }));
            self.save_chat();
            let calls = result.function_calls();
            if calls.is_empty() || result.interrupted {
                return;
            }
            match self.function_responses(&calls) {
                Some(parts) => self.chat.contents.push(json!({ "role": "user", "parts": parts })),
                None => {
                    self.failed = self.chat.pop_exchange();
                    render::info("Function call cancelled; exchange removed (/retry to run it again).");
                    return;
                }
            }
        }
    }

    /// AI Studio asks you to type the function's result; so do we.
    fn function_responses(&mut self, calls: &[Value]) -> Option<Vec<Value>> {
        let mut parts = Vec::new();
        render::info("Enter each function's result as JSON (or plain text). Empty input cancels.");
        for call in calls {
            let name = call.get("name").and_then(Value::as_str).unwrap_or("?").to_string();
            let prompt = format!("{name} → ");
            let styled = paint(fg(AnsiColor::Magenta).bold(), &prompt);
            let input = match self.editor.readline(&(prompt.as_str(), styled.as_str())) {
                Ok(l) => l,
                Err(_) => return None,
            };
            let input = input.trim();
            if input.is_empty() {
                return None;
            }
            let response = match serde_json::from_str::<Value>(input) {
                Ok(Value::Object(o)) => Value::Object(o),
                Ok(other) => json!({ "result": other }),
                Err(_) => json!({ "result": input }),
            };
            let mut fr = json!({ "name": name, "response": response });
            if let Some(id) = call.get("id") {
                fr["id"] = id.clone();
            }
            parts.push(json!({ "functionResponse": fr }));
        }
        Some(parts)
    }

    fn report_api_error(&self, e: &ApiError) {
        render::error(&format!("Error: {e}"));
        if let Some(h) = e.hint() {
            render::info(h);
        }
    }

    fn save_chat(&mut self) {
        if let Err(e) = self.chat.save(&self.settings) {
            render::warn(&format!("could not save chat: {e}"));
        }
    }

    fn print_transcript(&self, last_exchanges: usize) {
        let user_idx: Vec<usize> = self
            .chat
            .contents
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.get("role").and_then(Value::as_str) == Some("user")
                    && c.get("parts")
                        .and_then(Value::as_array)
                        .is_some_and(|ps| ps.iter().all(|p| p.get("functionResponse").is_none()))
            })
            .map(|(i, _)| i)
            .collect();
        let start = user_idx.len().saturating_sub(last_exchanges);
        let from = user_idx.get(start).copied().unwrap_or(0);
        if from > 0 {
            render::info(&format!("… {} earlier exchange(s) not shown", start));
        }
        for c in &self.chat.contents[from..] {
            let is_model = c.get("role").and_then(Value::as_str) == Some("model");
            let label = if is_model {
                paint(fg(AnsiColor::Cyan).bold(), "Model")
            } else {
                paint(fg(AnsiColor::BrightBlue).bold(), "You")
            };
            render::out(&format!("{label}\n"));
            for p in c.get("parts").and_then(Value::as_array).into_iter().flatten() {
                if p.get("thought").and_then(Value::as_bool).unwrap_or(false) {
                    continue;
                }
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    if self.pretty {
                        let mut md = MdStream::new(anstyle::Style::new());
                        let mut s = md.feed(t);
                        s.push_str(&md.finish());
                        render::out(&s);
                    } else {
                        render::out(t);
                    }
                    render::out("\n");
                } else if let Some(d) = p.get("inlineData") {
                    render::info(&format!("[{}]", d.get("mimeType").and_then(Value::as_str).unwrap_or("file")));
                } else if let Some(f) = p.get("functionCall") {
                    render::info(&format!("[function call: {}]", f.get("name").and_then(Value::as_str).unwrap_or("?")));
                } else if p.get("functionResponse").is_some() {
                    render::info("[function response]");
                }
            }
            render::out("\n");
        }
    }

    // -- commands ---------------------------------------------------------

    fn command(&mut self, line: &str) -> Flow {
        let (cmd, arg) = match line.split_once(char::is_whitespace) {
            Some((c, a)) => (c.to_lowercase(), a.trim().to_string()),
            None => (line.to_lowercase(), String::new()),
        };
        match cmd.as_str() {
            "/exit" | "/quit" | "/q" => return Flow::Exit,
            "/help" | "/?" => self.help(),
            "/new" | "/clear" | "/reset" => {
                self.save_chat();
                self.chat = Chat::new(&self.settings);
                self.attachments.clear();
                self.failed = None;
                render::success("New chat.");
                self.status_line();
            }
            "/model" => self.cmd_model(&arg),
            "/models" => {
                self.print_models();
            }
            "/thinking" | "/think" | "/effort" if arg.is_empty() => self.pick_thinking(),
            "/system" | "/sys" if arg.is_empty() => self.edit_system(),
            "/settings" | "/config" => self.cmd_settings(&arg),
            "/set" => {
                if arg.is_empty() {
                    self.print_keys();
                } else {
                    let (k, v) = arg.split_once(char::is_whitespace).unwrap_or((arg.as_str(), ""));
                    self.apply(k, v);
                }
            }
            "/tools" => self.print_tools(),
            "/safety" if arg.is_empty() => self.print_safety(),
            "/attach" | "/file" | "/upload" => self.cmd_attach(&arg),
            "/detach" => {
                self.attachments.clear();
                render::success("Attachments removed.");
            }
            "/attachments" => {
                for (name, _) in &self.attachments {
                    render::out(&format!("  {name}\n"));
                }
                if self.attachments.is_empty() {
                    render::info("No pending attachments.");
                }
            }
            "/retry" | "/regenerate" => self.cmd_retry(),
            "/undo" => match self.chat.pop_exchange() {
                Some(_) => {
                    self.save_chat();
                    render::success("Removed the last exchange.");
                }
                None => render::info("Nothing to undo."),
            },
            "/chats" | "/history" => self.print_chats(),
            "/load" | "/open" | "/resume" => self.cmd_load(&arg),
            "/rename" => {
                if arg.is_empty() {
                    render::warn("usage: /rename <title>");
                } else {
                    self.chat.title = arg.clone();
                    self.save_chat();
                    render::success(&format!("Renamed to \"{arg}\"."));
                }
            }
            "/export" => self.cmd_export(&arg),
            "/code" | "/getcode" | "/get-code" => self.cmd_code(),
            "/key" | "/login" => {
                if let Some(key) = prompt_api_key() {
                    self.cfg.api_key = Some(key.clone());
                    match self.cfg.save() {
                        Ok(p) => render::success(&format!("API key saved to {}", p.display())),
                        Err(e) => render::error(&format!("could not save config: {e}")),
                    }
                    match Client::new(key) {
                        Ok(c) => self.client = c,
                        Err(e) => render::error(&e.to_string()),
                    }
                }
            }
            "/render" => match arg.as_str() {
                "markdown" | "md" | "pretty" => {
                    self.pretty = true;
                    render::success("Rendering markdown.");
                }
                "raw" | "plain" => {
                    self.pretty = false;
                    render::success("Printing raw text.");
                }
                _ => render::warn("usage: /render <markdown|raw>"),
            },
            _ => {
                // Any run setting works as a command: /temp 0.5, /search off, /thinking high, ...
                let key = normalize_key(&cmd);
                if KEYS.iter().any(|(k, _, _)| *k == key) {
                    self.apply(&key, &arg);
                } else {
                    render::warn(&format!("Unknown command {cmd}. /help lists commands."));
                }
            }
        }
        Flow::Continue
    }

    fn apply(&mut self, key: &str, value: &str) {
        match self.settings.apply(key, value) {
            Ok(msg) => render::success(&msg),
            Err(e) => render::error(&e),
        }
    }

    fn help(&self) {
        render::out(&format!("{}\n", paint(anstyle::Style::new().bold(), "Commands")));
        for (cmd, args, desc) in COMMANDS {
            let left = format!("{cmd} {args}");
            render::out(&format!(
                "  {}{}  {}\n",
                paint(fg(AnsiColor::Cyan), &left),
                " ".repeat(30usize.saturating_sub(left.chars().count())),
                desc
            ));
        }
        render::out(&format!(
            "\n{}\n  Enter sends · Alt+Enter or Ctrl+J inserts a newline · \"\"\" starts/ends a multi-line block\n  Tab completes commands, models and file names · Ctrl+C stops a response\n",
            paint(anstyle::Style::new().bold(), "Input")
        ));
    }

    fn print_keys(&self) {
        render::out(&format!("{}\n", paint(anstyle::Style::new().bold(), "/set <key> <value>")));
        for (k, hint, desc) in KEYS {
            render::out(&format!("  {} {}  {}\n", paint(fg(AnsiColor::Cyan), k), paint(dim(), hint), desc));
        }
    }

    fn cmd_settings(&mut self, arg: &str) {
        match arg {
            "save" => {
                self.cfg.settings = self.settings.clone();
                self.cfg.render = Some(if self.pretty { "markdown" } else { "raw" }.to_string());
                match self.cfg.save() {
                    Ok(p) => render::success(&format!("Saved as defaults for new chats ({}).", p.display())),
                    Err(e) => render::error(&format!("could not save config: {e}")),
                }
            }
            "reset" => {
                self.settings = RunSettings::default();
                render::success("Run settings reset to built-in defaults (not saved; /settings save to keep).");
                self.status_line();
            }
            "" => {
                let mut section = "";
                for (sec, key, value) in self.settings.describe() {
                    if sec != section {
                        render::out(&format!("{}\n", paint(anstyle::Style::new().bold(), sec)));
                        section = sec;
                    }
                    render::out(&format!(
                        "  {}{}{}\n",
                        paint(fg(AnsiColor::Cyan), key),
                        " ".repeat(20usize.saturating_sub(key.len())),
                        value
                    ));
                }
                render::info("Change with /set <key> <value>; keep as defaults with /settings save.");
            }
            _ => render::warn("usage: /settings [save|reset]"),
        }
    }

    fn print_tools(&self) {
        for (sec, key, value) in self.settings.describe() {
            if sec == "Tools" {
                let on = value.starts_with("on");
                let mark = if on { paint(fg(AnsiColor::Green), "●") } else { paint(dim(), "○") };
                render::out(&format!(
                    "  {mark} {}{}{}\n",
                    key,
                    " ".repeat(20usize.saturating_sub(key.len())),
                    paint(dim(), &value)
                ));
            }
        }
        render::info("Toggle with /search, /maps, /url, /codeexec, /json, /functions <file>.");
    }

    fn print_safety(&self) {
        for (short, full) in SAFETY_CATEGORIES {
            let v = self.settings.safety.get(short).map(String::as_str).unwrap_or("default");
            render::out(&format!(
                "  {}{}{}  {}\n",
                paint(fg(AnsiColor::Cyan), short),
                " ".repeat(12usize.saturating_sub(short.len())),
                v,
                paint(dim(), full)
            ));
        }
        render::info(
            "Thresholds: off, none (block none), high (block only high), medium, low, default. Example: /safety all none",
        );
    }

    fn fetch_models(&mut self) -> Option<Vec<ModelInfo>> {
        if let Some(list) = self.models.lock().ok().and_then(|m| m.clone()) {
            return Some(list);
        }
        match self.client.list_models() {
            Ok(list) => {
                if let Ok(mut slot) = self.models.lock() {
                    *slot = Some(list.clone());
                }
                Some(list)
            }
            Err(e) => {
                self.report_api_error(&e);
                None
            }
        }
    }

    fn chat_models(&mut self) -> Vec<ModelInfo> {
        self.fetch_models().unwrap_or_default().into_iter().filter(|m| m.can_chat()).collect()
    }

    fn print_models(&mut self) -> Vec<ModelInfo> {
        let models = self.chat_models();
        let width = models.iter().map(|m| m.id().len()).max().unwrap_or(10);
        for (i, m) in models.iter().enumerate() {
            let current = m.id() == self.settings.model;
            let marker = if current { paint(fg(AnsiColor::Green), "●") } else { " ".into() };
            let think = if m.thinking == Some(true) { " thinking" } else { "" };
            let limit = m.input_token_limit.map(|n| format!(" {}k ctx", n / 1000)).unwrap_or_default();
            render::out(&format!(
                "{marker}{} {}{}  {}{}\n",
                paint(dim(), &format!("{:>3}.", i + 1)),
                paint(fg(AnsiColor::Cyan), m.id()),
                " ".repeat(width.saturating_sub(m.id().len())),
                truncate(&m.display_name, 40),
                paint(dim(), &format!("{limit}{think}"))
            ));
        }
        models
    }

    fn cmd_model(&mut self, arg: &str) {
        let choice = if arg.is_empty() {
            let models = self.print_models();
            if models.is_empty() {
                return;
            }
            let Ok(input) = self.editor.readline("Model (number or name, Enter to cancel): ") else { return };
            let input = input.trim().to_string();
            if input.is_empty() {
                return;
            }
            match input.parse::<usize>() {
                Ok(n) if (1..=models.len()).contains(&n) => models[n - 1].id().to_string(),
                _ => input,
            }
        } else if let Ok(n) = arg.parse::<usize>() {
            let models = self.chat_models();
            match models.get(n.wrapping_sub(1)) {
                Some(m) => m.id().to_string(),
                None => {
                    render::warn("No such number; /models lists them.");
                    return;
                }
            }
        } else {
            arg.to_string()
        };
        self.apply("model", &choice);
        let known = self.models.lock().ok().and_then(|m| m.clone());
        if let Some(list) = known {
            match list.iter().find(|m| m.id() == self.settings.model) {
                None => render::warn("This model is not in your model list; requests may fail."),
                Some(m) if !m.description.is_empty() => render::info(&truncate(&m.description, 200)),
                _ => {}
            }
        }
    }

    fn pick_thinking(&mut self) {
        render::out(&format!("Current: {}\n", self.settings.thinking_label()));
        let options = ["default", "minimal", "low", "medium", "high", "off", "dynamic"];
        for (i, o) in options.iter().enumerate() {
            let note = match *o {
                "default" => "model default",
                "minimal" | "low" | "medium" | "high" => "thinking level (Gemini 3+)",
                "off" => "budget 0 (models that allow it)",
                "dynamic" => "budget -1 (Gemini 2.5)",
                _ => "",
            };
            render::out(&format!("  {} {:<8} {}\n", paint(dim(), &format!("{}.", i + 1)), o, paint(dim(), note)));
        }
        let Ok(input) = self.editor.readline("Thinking (number, level or token budget, Enter to cancel): ") else {
            return;
        };
        let input = input.trim();
        if input.is_empty() {
            return;
        }
        let value = match input.parse::<usize>() {
            Ok(n) if (1..=options.len()).contains(&n) => options[n - 1].to_string(),
            _ => input.to_string(),
        };
        self.apply("thinking", &value);
    }

    fn edit_system(&mut self) {
        if self.settings.system_instruction.is_empty() {
            render::info("No system instructions.");
        } else {
            render::out(&format!(
                "{}\n{}\n",
                paint(anstyle::Style::new().bold(), "Current system instructions:"),
                self.settings.system_instruction
            ));
        }
        render::info(
            "Type new instructions; finish with a line containing only \"\"\" (empty first line keeps the current ones, /system clear removes them).",
        );
        let mut lines: Vec<String> = Vec::new();
        loop {
            match self.editor.readline("system… ") {
                Ok(l) if l.trim() == "\"\"\"" => break,
                Ok(l) if lines.is_empty() && l.trim().is_empty() => return,
                Ok(l) => lines.push(l),
                Err(_) => return,
            }
        }
        self.apply("system", &lines.join("\n"));
    }

    fn cmd_attach(&mut self, arg: &str) {
        let paths = split_args(arg);
        if paths.is_empty() {
            render::warn("usage: /attach <file>...");
            return;
        }
        for p in paths {
            let expanded = settings::shellexpand_home(&p);
            match file_part(Path::new(&expanded)) {
                Ok(part) => {
                    let mime = part["inlineData"]["mimeType"].as_str().unwrap_or("").to_string();
                    render::success(&format!("Attached {p} ({mime})"));
                    self.attachments.push((p, part));
                }
                Err(e) => render::error(&e),
            }
        }
    }

    fn cmd_retry(&mut self) {
        let parts = match self.failed.take() {
            Some(p) => Some(p),
            None => self.chat.pop_exchange(),
        };
        match parts {
            Some(parts) => {
                self.chat.contents.push(json!({ "role": "user", "parts": parts }));
                self.generate();
            }
            None => render::info("Nothing to retry."),
        }
    }

    fn print_chats(&self) {
        let chats = list_chats();
        if chats.is_empty() {
            render::info("No saved chats yet.");
            return;
        }
        for (i, (_, c)) in chats.iter().enumerate().take(30) {
            let when = c.updated.get(..16).unwrap_or(&c.updated).replace('T', " ");
            let current = c.id == self.chat.id;
            let marker = if current { paint(fg(AnsiColor::Green), "●") } else { " ".into() };
            render::out(&format!(
                "{marker}{} {}  {}\n",
                paint(dim(), &format!("{:>3}.", i + 1)),
                if c.title.is_empty() { "(untitled)" } else { &c.title },
                paint(dim(), &format!("{when} · {} · {} turns · {}", c.settings.model, c.turn_count(), c.id))
            ));
        }
        if chats.len() > 30 {
            render::info(&format!("… and {} more", chats.len() - 30));
        }
        render::info("Open one with /load <number>.");
    }

    fn cmd_load(&mut self, arg: &str) {
        match conversation::find_chat(arg) {
            Ok(chat) => {
                self.save_chat();
                self.settings = chat.settings.clone();
                self.chat = chat;
                self.attachments.clear();
                self.failed = None;
                render::success(&format!("Opened \"{}\".", self.chat.title));
                self.status_line();
                self.print_transcript(2);
            }
            Err(e) => render::error(&e),
        }
    }

    fn cmd_export(&mut self, arg: &str) {
        if arg.is_empty() {
            render::warn("usage: /export <file.md|file.json>");
            return;
        }
        let path = settings::shellexpand_home(arg);
        self.chat.settings = self.settings.clone();
        let data = if path.to_lowercase().ends_with(".json") {
            serde_json::to_string_pretty(&self.chat).unwrap_or_default()
        } else {
            self.chat.to_markdown()
        };
        match std::fs::write(&path, data) {
            Ok(()) => render::success(&format!("Exported to {path}")),
            Err(e) => render::error(&format!("cannot write {path}: {e}")),
        }
    }

    fn cmd_code(&self) {
        render::out(&get_code(&self.client, &self.settings, &self.chat.contents, self.supports_thinking()));
    }
}

/// The request as a curl command, like AI Studio's "Get code".
pub fn get_code(
    client: &Client,
    settings: &RunSettings,
    contents: &[Value],
    supports_thinking: Option<bool>,
) -> String {
    let mut contents: Vec<Value> = contents.to_vec();
    for c in contents.iter_mut() {
        if let Some(parts) = c.get_mut("parts").and_then(Value::as_array_mut) {
            for p in parts {
                if let Some(d) = p.get_mut("inlineData").and_then(|d| d.get_mut("data")) {
                    *d = json!("<base64 data>");
                }
            }
        }
    }
    if contents.is_empty() {
        contents.push(json!({ "role": "user", "parts": [ { "text": "INSERT_INPUT_HERE" } ] }));
    }
    let body = settings.build_body(&contents, supports_thinking);
    format!(
        "curl \"{}\" \\\n  -H \"x-goog-api-key: $GEMINI_API_KEY\" \\\n  -H \"Content-Type: application/json\" \\\n  -X POST \\\n  -d @- <<'EOF'\n{}\nEOF\n",
        client.stream_url(&settings.model),
        serde_json::to_string_pretty(&body).unwrap_or_default()
    )
}

pub fn prompt_api_key() -> Option<String> {
    render::out(&format!(
        "Create an API key at {} (Get API key), then paste it here.\n",
        paint(fg(AnsiColor::Cyan), "https://aistudio.google.com/apikey")
    ));
    match rpassword::prompt_password("API key (input hidden): ") {
        Ok(k) if !k.trim().is_empty() => Some(k.trim().to_string()),
        Ok(_) => {
            render::warn("No key entered.");
            None
        }
        Err(e) => {
            render::error(&format!("could not read key: {e}"));
            None
        }
    }
}

/// Split arguments on whitespace, honouring "double" and 'single' quotes.
/// Backslashes are kept as-is so Windows paths work.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in s.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            None => {
                cur.push(ch);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_args_handles_quotes_and_windows_paths() {
        assert_eq!(
            split_args(r#"a.png "my file.pdf" 'x y' C:\Users\me\doc.txt"#),
            vec!["a.png", "my file.pdf", "x y", r"C:\Users\me\doc.txt"]
        );
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn every_listed_setting_command_is_a_known_key() {
        for cmd in [
            "/search",
            "/maps",
            "/url",
            "/codeexec",
            "/json",
            "/schema",
            "/functions",
            "/media",
            "/temp",
            "/maxtokens",
            "/stop",
            "/thinking",
            "/thoughts",
            "/system",
            "/safety",
        ] {
            let key = normalize_key(cmd);
            assert!(KEYS.iter().any(|(k, _, _)| *k == key), "{cmd} -> {key}");
        }
    }
}
