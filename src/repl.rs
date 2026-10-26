//! Interactive chat: the Playground screen of AI Studio, in a terminal.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use anstyle::AnsiColor;
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{
    Cmd, ConditionalEventHandler, Config as RlConfig, Context, EditMode, Editor, Event, EventContext, EventHandler,
    Helper, InputMode, KeyCode, KeyEvent, Modifiers, RepeatCount,
};
use serde_json::{Value, json};

use crate::api::{ApiError, Client, DEFAULT_BASE_URL, ModelInfo};
use crate::chat::run_turn;
use crate::config::{self, Config};
use crate::conversation::{self, Chat, file_part, list_chats};
use crate::render::{self, MdStream, dim, fg, paint};
use crate::settings::{self, KEYS, RunSettings, SAFETY_CATEGORIES, THINKING_LEVELS, normalize_key, truncate};
use crate::web::WebSession;

/// (command, arguments, description)
const COMMANDS: &[(&str, &str, &str)] = &[
    ("/help", "", "Show this help"),
    ("/new", "", "Start a new chat (alias: /clear)"),
    ("/model", "[name|number]", "Switch model (no argument: pick from list)"),
    ("/models", "", "List the models you can use"),
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
    ("/edit", "", "Write the next message in $EDITOR (also `v` in vi normal mode)"),
    ("/vim", "[on|off]", "Vim keybindings for the input line (saved)"),
    ("/code", "", "Show the equivalent API request (Get code)"),
    ("/debug", "", "Web: save a screenshot and the page's HTML for troubleshooting"),
    ("/key", "", "Set the API key (for --api)"),
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
                .chain(["/clear", "/quit", "/topp", "/topk", "/thinking", "/emacs"])
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
            "/edit" | "/debug" => Vec::new(),
            "/settings" => vec!["save".into(), "reset".into()],
            "/load" => list_chats().into_iter().map(|(_, c)| c.id).collect(),
            _ => vec!["on".into(), "off".into()],
        };
        Ok((arg_start, options.iter().filter(|o| o.starts_with(word)).map(|o| pair(o)).collect()))
    }
}

/// Where prompts go: the AI Studio page in a browser, or the Gemini API.
pub enum Engine {
    Api(Client),
    Web(Box<WebEngine>),
}

pub struct WebEngine {
    pub session: WebSession,
    /// The page's run settings as last read or applied; `None` after it
    /// navigated, so they are read again before the next message.
    pub page: Option<RunSettings>,
}

impl Engine {
    pub fn close(self) {
        if let Engine::Web(w) = self {
            let w = *w;
            w.session.close();
        }
    }
}

/// A file to send with the next message.
struct Attachment {
    name: String,
    path: PathBuf,
    /// Inline data for the API engine.
    part: Option<Value>,
}

/// `v` in vi normal mode: hand the line to $EDITOR (as in bash and zsh).
struct EditInEditor(Arc<Mutex<Option<String>>>);

impl ConditionalEventHandler for EditInEditor {
    fn handle(&self, _: &Event, _: RepeatCount, _: bool, ctx: &EventContext) -> Option<Cmd> {
        if ctx.mode() != EditMode::Vi || ctx.input_mode() != InputMode::Command {
            return None;
        }
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(ctx.line().to_string());
        }
        Some(Cmd::AcceptLine)
    }
}

pub struct Repl {
    engine: Option<Engine>,
    cfg: Config,
    settings: RunSettings,
    chat: Chat,
    pretty: bool,
    attachments: Vec<Attachment>,
    /// Prompt parts of a request that failed, for /retry.
    failed: Option<Vec<Value>>,
    models: Arc<Mutex<Option<Vec<ModelInfo>>>>,
    editor: Editor<ReplHelper, DefaultHistory>,
    edit_request: Arc<Mutex<Option<String>>>,
}

enum Flow {
    Continue,
    Exit,
}

impl Repl {
    pub fn new(
        engine: Engine,
        cfg: Config,
        settings: RunSettings,
        chat: Chat,
        pretty: bool,
        vim: bool,
    ) -> rustyline::Result<Repl> {
        let rl_cfg = RlConfig::builder()
            .edit_mode(if vim { EditMode::Vi } else { EditMode::Emacs })
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
        let edit_request = Arc::new(Mutex::new(None));
        editor.bind_sequence(
            KeyEvent::from('v'),
            EventHandler::Conditional(Box::new(EditInEditor(edit_request.clone()))),
        );
        let _ = editor.load_history(&config::input_history_path());

        // API: fetch the model list in the background. It feeds /model
        // completion and tells us which models accept thinking settings.
        if let Engine::Api(client) = &engine {
            let c = client.clone();
            let m = models.clone();
            thread::spawn(move || {
                if let Ok(list) = c.list_models()
                    && let Ok(mut slot) = m.lock()
                {
                    *slot = Some(list);
                }
            });
        }

        Ok(Repl {
            engine: Some(engine),
            cfg,
            settings,
            chat,
            pretty,
            attachments: Vec::new(),
            failed: None,
            models,
            editor,
            edit_request,
        })
    }

    fn is_web(&self) -> bool {
        matches!(self.engine, Some(Engine::Web(_)))
    }

    fn web(&mut self) -> Option<&mut WebEngine> {
        match self.engine.as_mut() {
            Some(Engine::Web(w)) => Some(w.as_mut()),
            _ => None,
        }
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
                    let pending_edit = self.edit_request.lock().ok().and_then(|mut s| s.take());
                    let line = match pending_edit {
                        Some(draft) => match compose_in_editor(&draft) {
                            Ok(text) => text,
                            Err(e) => {
                                render::error(&e);
                                continue;
                            }
                        },
                        None => line,
                    };
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
        if let Some(engine) = self.engine.take() {
            engine.close();
        }
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
        let via =
            if self.is_web() { "AI Studio in your browser (signed-in Google account)" } else { "Gemini API (API key)" };
        render::info(&format!("via {via}"));
        self.status_line();
        render::info(
            "/help for commands · \"\"\" or Alt+Enter for multi-line input · /edit opens $EDITOR · Ctrl+C stops a response · Ctrl+D quits",
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
        if self.is_web() {
            self.web_send(text);
            return;
        }
        let mut parts: Vec<Value> = self.attachments.drain(..).filter_map(|a| a.part).collect();
        if !text.trim().is_empty() {
            parts.push(conversation::user_text(&text));
        }
        self.failed = None;
        self.chat.contents.push(json!({ "role": "user", "parts": parts }));
        self.generate();
    }

    /// Web: make the page's settings match, type the message, and stream the answer.
    fn web_send(&mut self, text: String) {
        let files: Vec<PathBuf> = self.attachments.iter().map(|a| a.path.clone()).collect();
        let names: Vec<String> = self.attachments.drain(..).map(|a| a.name).collect();
        let settings = self.settings.clone();
        let pretty = self.pretty;
        let Some(web) = self.web() else { return };
        let page = match web.page.take() {
            Some(p) => p,
            None => web.session.read_settings(&settings),
        };
        for w in web.session.apply_settings(&page, &settings) {
            render::warn(&format!("AI Studio: {w}"));
        }
        web.page = Some(settings.clone());
        if let Err(e) = web.session.attach(&files) {
            render::error(&format!("Could not attach files: {e}"));
            return;
        }
        match web.session.send(&text, pretty, settings.include_thoughts, &settings.model) {
            Ok(turn) => {
                let mut parts: Vec<Value> = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(conversation::user_text(&text));
                }
                parts.extend(names.iter().map(|n| json!({ "text": format!("[attachment: {n}]") })));
                self.chat.contents.push(json!({ "role": "user", "parts": parts }));
                if !turn.text.is_empty() {
                    self.chat.contents.push(json!({ "role": "model", "parts": [ { "text": turn.text } ] }));
                }
                if turn.url.contains("/prompts/") && !turn.url.contains("new_chat") {
                    self.chat.web_url = Some(turn.url);
                }
                self.save_chat();
            }
            Err(e) => {
                render::error(&format!("Error: {e}"));
                render::info("If the page looks different than expected, /debug saves a screenshot and its HTML.");
            }
        }
    }

    fn generate(&mut self) {
        let Some(Engine::Api(client)) = self.engine.as_ref() else {
            render::warn("Not available with the web engine.");
            return;
        };
        let client = client.clone();
        loop {
            let result = run_turn(&client, &self.settings, &self.chat.contents, self.supports_thinking(), self.pretty);
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
                if let Some(web) = self.web() {
                    if let Err(e) = web.session.new_chat() {
                        render::error(&e);
                    }
                    web.page = None;
                }
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
            "/edit" | "/e" => match compose_in_editor("") {
                Ok(text) if !text.trim().is_empty() => {
                    let _ = self.editor.add_history_entry(text.as_str());
                    self.send(text);
                }
                Ok(_) => render::info("Empty message; nothing sent."),
                Err(e) => render::error(&e),
            },
            "/vim" | "/vi" | "/emacs" => {
                let on = if cmd == "/emacs" {
                    false
                } else {
                    match settings::parse_bool(&arg, self.editor.config_mut().edit_mode() == EditMode::Vi) {
                        Ok(b) => b,
                        Err(e) => {
                            render::error(&e);
                            return Flow::Continue;
                        }
                    }
                };
                self.editor.set_edit_mode(if on { EditMode::Vi } else { EditMode::Emacs });
                self.cfg.edit_mode = Some(if on { "vi" } else { "emacs" }.to_string());
                let _ = self.cfg.save();
                render::success(if on {
                    "Vim keybindings on (Esc for normal mode, `v` there opens $EDITOR)."
                } else {
                    "Emacs keybindings on."
                });
            }
            "/debug" => match self.web() {
                Some(web) => match web.session.debug_dump() {
                    Ok(dir) => render::success(&format!(
                        "Saved screenshot.png, page.html, outline.txt and state.json to {}\n(They contain this chat; check before sharing.)",
                        dir.display()
                    )),
                    Err(e) => render::error(&e),
                },
                None => render::info("/debug is for the web engine."),
            },
            "/detach" => {
                self.attachments.clear();
                render::success("Attachments removed.");
            }
            "/attachments" => {
                for a in &self.attachments {
                    render::out(&format!("  {}\n", a.name));
                }
                if self.attachments.is_empty() {
                    render::info("No pending attachments.");
                }
            }
            "/retry" | "/regenerate" | "/undo" if self.is_web() => {
                render::info(
                    "With the web engine, rerun or delete turns in AI Studio itself (aistudio login opens it).",
                );
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
                    if let Some(Engine::Api(client)) = self.engine.as_mut() {
                        match Client::new(key) {
                            Ok(c) => *client = c,
                            Err(e) => render::error(&e.to_string()),
                        }
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
            "save" if self.is_web() => {
                self.cfg.web_settings = Some(self.settings.clone());
                self.cfg.render = Some(if self.pretty { "markdown" } else { "raw" }.to_string());
                match self.cfg.save() {
                    Ok(p) => render::success(&format!("Saved; applied to AI Studio at every start ({}).", p.display())),
                    Err(e) => render::error(&format!("could not save config: {e}")),
                }
            }
            "reset" if self.is_web() => {
                self.cfg.web_settings = None;
                let _ = self.cfg.save();
                let base = self.settings.clone();
                if let Some(web) = self.web() {
                    let page = web.session.read_settings(&base);
                    web.page = Some(page.clone());
                    self.settings = page;
                }
                render::success("Using AI Studio's own settings again (saved defaults removed).");
                self.status_line();
            }
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
        let fetched = match self.engine.as_mut() {
            Some(Engine::Api(client)) => client.list_models().map_err(|e| {
                self.report_api_error(&e);
            }),
            Some(Engine::Web(web)) => web.session.list_models().map_err(|e| render::error(&format!("Error: {e}"))),
            None => Err(()),
        };
        let list = fetched.ok()?;
        if let Ok(mut slot) = self.models.lock() {
            *slot = Some(list.clone());
        }
        Some(list)
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
            let path = PathBuf::from(settings::shellexpand_home(&p));
            if self.is_web() {
                if path.is_file() {
                    render::success(&format!("Attached {p}"));
                    self.attachments.push(Attachment { name: p, path, part: None });
                } else {
                    render::error(&format!("no such file: {p}"));
                }
                continue;
            }
            match file_part(Path::new(&path)) {
                Ok(part) => {
                    let mime = part["inlineData"]["mimeType"].as_str().unwrap_or("").to_string();
                    render::success(&format!("Attached {p} ({mime})"));
                    self.attachments.push(Attachment { name: p, path, part: Some(part) });
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
                if let Some(web) = self.web() {
                    let Some(url) = chat.web_url.clone() else {
                        render::error("That chat was made with the API engine; open it with `aistudio --api`.");
                        return;
                    };
                    if let Err(e) = web.session.navigate(&url) {
                        render::error(&e);
                        return;
                    }
                    web.page = None;
                }
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
        let url = match self.engine.as_ref() {
            Some(Engine::Api(client)) => client.stream_url(&self.settings.model),
            _ => format!("{DEFAULT_BASE_URL}/models/{}:streamGenerateContent?alt=sse", self.settings.model),
        };
        render::out(&get_code(&url, &self.settings, &self.chat.contents, self.supports_thinking()));
    }
}

/// The request as a curl command, like AI Studio's "Get code".
pub fn get_code(url: &str, settings: &RunSettings, contents: &[Value], supports_thinking: Option<bool>) -> String {
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
        url,
        serde_json::to_string_pretty(&body).unwrap_or_default()
    )
}

/// Write a message in $VISUAL / $EDITOR (vi, or Notepad on Windows) and return it.
pub fn compose_in_editor(initial: &str) -> Result<String, String> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| if cfg!(windows) { "notepad".into() } else { "vi".into() });
    let path = std::env::temp_dir().join(format!(
        "aistudio-{}-{}.md",
        std::process::id(),
        chrono::Local::now().format("%H%M%S%f")
    ));
    std::fs::write(&path, initial).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    // Through the shell, so EDITOR may carry arguments (e.g. "code --wait").
    #[cfg(windows)]
    let status = std::process::Command::new("cmd").arg("/C").arg(format!("{editor} \"{}\"", path.display())).status();
    #[cfg(not(windows))]
    let status =
        std::process::Command::new("sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("sh").arg(&path).status();
    let result = match status {
        Ok(s) if s.success() => std::fs::read_to_string(&path)
            .map(|t| t.trim_end().to_string())
            .map_err(|e| format!("cannot read {}: {e}", path.display())),
        Ok(s) => Err(format!("{editor} exited with {s}; nothing sent")),
        Err(e) => Err(format!("cannot run {editor}: {e}")),
    };
    let _ = std::fs::remove_file(&path);
    result
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
