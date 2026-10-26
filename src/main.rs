//! aistudio: Google AI Studio's chat playground in the terminal.

mod api;
mod chat;
mod config;
mod conversation;
mod render;
mod repl;
mod settings;

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use crate::api::Client;
use crate::config::{Config, mask_key};
use crate::conversation::{Chat, file_part};
use crate::settings::RunSettings;

#[derive(Parser, Debug)]
#[command(
    name = "aistudio",
    version,
    about = "Chat with Gemini from your terminal, with Google AI Studio's run settings.",
    long_about = "Chat with Gemini from your terminal, with Google AI Studio's run settings.\n\n\
        Without a prompt, starts an interactive chat. With a prompt (or piped stdin),\n\
        prints one answer and exits.\n\n\
        Examples:\n  \
        aistudio\n  \
        aistudio \"Explain RAII in two sentences\"\n  \
        cat main.rs | aistudio -t high \"Review this code\"\n  \
        aistudio -f receipt.jpg --schema receipt.schema.json \"Extract the fields\"\n  \
        aistudio -c \"And in Rust?\"          (continue the last chat)",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Prompt to send. Omit to start an interactive chat.
    prompt: Vec<String>,

    /// Model id, e.g. gemini-3.8-flash
    #[arg(short, long)]
    model: Option<String>,
    /// Thinking: default, minimal, low, medium, high, off, dynamic, or a token budget
    #[arg(short, long, value_name = "LEVEL")]
    thinking: Option<String>,
    /// System instructions
    #[arg(short, long)]
    system: Option<String>,
    /// Read system instructions from a file
    #[arg(long, value_name = "FILE")]
    system_file: Option<PathBuf>,
    /// Temperature (0-2)
    #[arg(long)]
    temperature: Option<String>,
    /// Top-p (0-1)
    #[arg(long)]
    top_p: Option<String>,
    /// Top-k
    #[arg(long)]
    top_k: Option<String>,
    /// Output length (max output tokens)
    #[arg(long, value_name = "TOKENS")]
    max_tokens: Option<String>,
    /// Stop sequence (repeatable)
    #[arg(long, value_name = "TEXT")]
    stop: Vec<String>,
    /// Grounding with Google Search on
    #[arg(long, overrides_with = "no_search")]
    search: bool,
    /// Grounding with Google Search off
    #[arg(long)]
    no_search: bool,
    /// Grounding with Google Maps on
    #[arg(long, overrides_with = "no_maps")]
    maps: bool,
    #[arg(long, hide = true)]
    no_maps: bool,
    /// URL context on
    #[arg(long, overrides_with = "no_url_context")]
    url_context: bool,
    #[arg(long, hide = true)]
    no_url_context: bool,
    /// Code execution on
    #[arg(long, overrides_with = "no_code_execution")]
    code_execution: bool,
    #[arg(long, hide = true)]
    no_code_execution: bool,
    /// Structured output: respond with JSON
    #[arg(long)]
    json: bool,
    /// Structured output with a JSON schema file
    #[arg(long, value_name = "FILE")]
    schema: Option<String>,
    /// Function declarations (JSON file) for function calling
    #[arg(long, value_name = "FILE")]
    functions: Option<String>,
    /// Media resolution: default, low, medium, high
    #[arg(long, value_name = "RES")]
    media_resolution: Option<String>,
    /// Safety setting as CATEGORY=THRESHOLD, e.g. all=none (repeatable)
    #[arg(long, value_name = "CAT=THR")]
    safety: Vec<String>,
    /// Don't request or show thought summaries
    #[arg(long)]
    no_thoughts: bool,
    /// Attach a file (image, PDF, audio, video, text). Repeatable.
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    files: Vec<PathBuf>,
    /// Print plain text without markdown styling
    #[arg(long, conflicts_with = "markdown")]
    raw: bool,
    /// Style markdown even when stdout is not a terminal
    #[arg(long)]
    markdown: bool,
    /// Continue the most recent chat
    #[arg(short = 'c', long = "continue")]
    cont: bool,
    /// Continue a saved chat (number from `aistudio chats`, or id)
    #[arg(short, long, value_name = "CHAT")]
    resume: Option<String>,
    /// Stay in interactive chat after answering the prompt
    #[arg(short, long)]
    interactive: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List models available to your API key
    Models {
        /// Print the raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Save an API key to the config file
    Login,
    /// Remove the saved API key
    Logout,
    /// Show config location and saved defaults
    Config,
    /// List saved chats
    Chats,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(msg) => {
            render::error(&msg);
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<u8, String> {
    let mut cfg = Config::load();

    if let Some(cmd) = &cli.command {
        return subcommand(cmd, &mut cfg);
    }

    let mut chat = if cli.cont || cli.resume.is_some() {
        conversation::find_chat(cli.resume.as_deref().unwrap_or("last"))?
    } else {
        Chat::new(&cfg.settings)
    };
    let mut settings = if chat.contents.is_empty() { cfg.settings.clone() } else { chat.settings.clone() };
    apply_flags(&cli, &mut settings)?;

    let mut prompt = cli.prompt.join(" ");
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        let mut piped = String::new();
        stdin.lock().read_to_string(&mut piped).map_err(|e| format!("cannot read stdin: {e}"))?;
        if !piped.trim().is_empty() {
            prompt = if prompt.is_empty() { piped } else { format!("{prompt}\n\n{piped}") };
        }
    }
    let one_shot = !prompt.trim().is_empty() || !cli.files.is_empty();

    let key = match cfg.resolve_api_key() {
        Some((k, _)) => k,
        None if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() => {
            render::out("No API key found (GEMINI_API_KEY or `aistudio login`).\n");
            let k = repl::prompt_api_key().ok_or("an API key is required")?;
            cfg.api_key = Some(k.clone());
            match cfg.save() {
                Ok(p) => render::success(&format!("Saved to {}", p.display())),
                Err(e) => render::warn(&format!("could not save config: {e}")),
            }
            k
        }
        None => {
            return Err("No API key. Set GEMINI_API_KEY, or run `aistudio login`.\nGet one at https://aistudio.google.com/apikey".into());
        }
    };
    let client = Client::new(key).map_err(|e| e.to_string())?;

    let pretty = if cli.raw {
        false
    } else if cli.markdown {
        true
    } else {
        std::io::stdout().is_terminal() && cfg.render.as_deref() != Some("raw")
    };

    if one_shot {
        let mut parts: Vec<Value> = Vec::new();
        for f in &cli.files {
            parts.push(file_part(f)?);
        }
        if !prompt.trim().is_empty() {
            parts.push(json!({ "text": prompt }));
        }
        chat.contents.push(json!({ "role": "user", "parts": parts }));
        let result = match chat::run_turn(&client, &settings, &chat.contents, None, pretty) {
            Ok(r) => r,
            Err(e) => {
                render::error(&format!("Error: {e}"));
                if let Some(h) = e.hint() {
                    render::info(h);
                }
                return Ok(1);
            }
        };
        if result.has_content() {
            chat.contents.push(json!({ "role": "model", "parts": result.parts }));
            if let Err(e) = chat.save(&settings) {
                render::warn(&format!("could not save chat: {e}"));
            }
        } else {
            chat.pop_exchange();
        }
        if !cli.interactive {
            return Ok(if result.interrupted {
                130
            } else if result.has_content() {
                0
            } else {
                1
            });
        }
        if !std::io::stdin().is_terminal() {
            return Err("--interactive needs a terminal on stdin".into());
        }
        let mut r = repl::Repl::new(client, cfg, settings, chat, pretty).map_err(|e| e.to_string())?;
        // The answer is already on screen; don't print it again.
        return Ok(r.run(false) as u8);
    }

    if !std::io::stdin().is_terminal() {
        return Err("nothing to send: give a prompt or pipe text on stdin".into());
    }
    let mut r = repl::Repl::new(client, cfg, settings, chat, pretty).map_err(|e| e.to_string())?;
    Ok(r.run(true) as u8)
}

fn apply_flags(cli: &Cli, s: &mut RunSettings) -> Result<(), String> {
    let mut set = |k: &str, v: &str| s.apply(k, v).map(|_| ()).map_err(|e| format!("--{k}: {e}"));
    if let Some(v) = &cli.model {
        set("model", v)?;
    }
    if let Some(v) = &cli.thinking {
        set("thinking", v)?;
    }
    if let Some(path) = &cli.system_file {
        let text = std::fs::read_to_string(path).map_err(|e| format!("--system-file {}: {e}", path.display()))?;
        set("system", &text)?;
    }
    if let Some(v) = &cli.system {
        set("system", v)?;
    }
    for (k, v) in [
        ("temperature", &cli.temperature),
        ("top_p", &cli.top_p),
        ("top_k", &cli.top_k),
        ("output_length", &cli.max_tokens),
        ("media_resolution", &cli.media_resolution),
        ("schema", &cli.schema),
        ("functions", &cli.functions),
    ] {
        if let Some(v) = v {
            set(k, v)?;
        }
    }
    if !cli.stop.is_empty() {
        set("stop", "clear")?;
        for v in &cli.stop {
            set("stop", v)?;
        }
    }
    for (key, on, off) in [
        ("search", cli.search, cli.no_search),
        ("maps", cli.maps, cli.no_maps),
        ("url_context", cli.url_context, cli.no_url_context),
        ("code_execution", cli.code_execution, cli.no_code_execution),
    ] {
        if on {
            set(key, "on")?;
        } else if off {
            set(key, "off")?;
        }
    }
    if cli.json {
        set("structured_output", "on")?;
    }
    if cli.no_thoughts {
        set("thoughts", "off")?;
    }
    for v in &cli.safety {
        let (cat, thr) = v.split_once('=').ok_or_else(|| format!("--safety expects CATEGORY=THRESHOLD, got '{v}'"))?;
        set("safety", &format!("{cat} {thr}"))?;
    }
    Ok(())
}

fn subcommand(cmd: &Command, cfg: &mut Config) -> Result<u8, String> {
    match cmd {
        Command::Login => {
            let key = repl::prompt_api_key().ok_or("no key entered")?;
            let client = Client::new(key.clone()).map_err(|e| e.to_string())?;
            match client.list_models() {
                Ok(models) => render::success(&format!("Key works ({} models available).", models.len())),
                Err(e) => render::warn(&format!("Could not verify the key ({e}); saving it anyway.")),
            }
            cfg.api_key = Some(key);
            let path = cfg.save().map_err(|e| format!("could not save config: {e}"))?;
            render::success(&format!("Saved to {}", path.display()));
            Ok(0)
        }
        Command::Logout => {
            cfg.api_key = None;
            let path = cfg.save().map_err(|e| format!("could not save config: {e}"))?;
            render::success(&format!("Removed the API key from {}", path.display()));
            for name in config::API_KEY_ENV_VARS {
                if std::env::var_os(name).is_some() {
                    render::warn(&format!("${name} is still set in your environment."));
                }
            }
            Ok(0)
        }
        Command::Config => {
            render::out(&format!("Config file: {}\n", config::config_path().display()));
            render::out(&format!("Chats:       {}\n", config::chats_dir().display()));
            match cfg.resolve_api_key() {
                Some((k, src)) => render::out(&format!("API key:     {} (from {src})\n", mask_key(&k))),
                None => render::out("API key:     (not set)\n"),
            }
            render::out(&format!("Defaults:\n{}\n", serde_json::to_string_pretty(&cfg.settings).unwrap_or_default()));
            Ok(0)
        }
        Command::Chats => {
            let chats = conversation::list_chats();
            if chats.is_empty() {
                render::info("No saved chats yet.");
            }
            for (i, (_, c)) in chats.iter().enumerate() {
                let when = c.updated.get(..16).unwrap_or(&c.updated).replace('T', " ");
                render::out(&format!(
                    "{:>3}. {}  {}\n",
                    i + 1,
                    if c.title.is_empty() { "(untitled)" } else { &c.title },
                    render::paint(render::dim(), &format!("{when} · {} · {}", c.settings.model, c.id))
                ));
            }
            Ok(0)
        }
        Command::Models { json } => {
            let (key, _) = cfg.resolve_api_key().ok_or("No API key. Set GEMINI_API_KEY, or run `aistudio login`.")?;
            let client = Client::new(key).map_err(|e| e.to_string())?;
            let models = client.list_models().map_err(|e| e.to_string())?;
            for m in models.iter().filter(|m| m.can_chat()) {
                if *json {
                    render::out(&format!(
                        "{}\n",
                        json!({
                            "id": m.id(),
                            "displayName": m.display_name,
                            "inputTokenLimit": m.input_token_limit,
                            "outputTokenLimit": m.output_token_limit,
                            "thinking": m.thinking,
                        })
                    ));
                } else {
                    render::out(&format!("{:<40} {}\n", m.id(), render::paint(render::dim(), &m.display_name)));
                }
            }
            Ok(0)
        }
    }
}
