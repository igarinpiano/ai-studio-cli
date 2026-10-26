# AI Studio CLI

> [!WARNING]
> **Experimental. Unofficial. Use at your own risk.**
>
> - This is an unofficial, experimental tool. It is not affiliated with, endorsed by, or supported by Google.
> - It works by automating the Google AI Studio web app in your own browser. Automated use may conflict with Google's Terms of Service, and could in principle lead to restrictions on your account. Decide for yourself whether that is acceptable before using it.
> - AI Studio's web interface changes without notice, and any change can break this tool at any time.
> - It is provided "as is", without warranty of any kind. The authors accept no liability for any damage, data loss, account issues, or other consequences of using it.

A command-line client for the [Google AI Studio](https://aistudio.google.com/) Playground (chat). It works as a **wrapper around the AI Studio web app** and ships as a single binary (written in Rust) for macOS, Windows and Linux.

- Streaming chat with markdown styling, code highlighting, thoughts and sources
- Model switching and thinking level (minimal / low / medium / high)
- The Run settings panel: system instructions, grounding with Google Search / Google Maps, URL context, code execution, structured outputs and function calling on/off, media resolution, temperature, top P, output length, stop sequences
- File attachments, saved and resumable chats, export to Markdown / JSON
- **Vim keybindings** (`--vim` / `/vim`) and composing messages in `$EDITOR`

## How it works

The CLI starts your Chrome (or Edge / Chromium / Brave) with a profile of its own and drives **the real AI Studio page** (aistudio.google.com) in the background. It types your prompt into the prompt box, presses Run, and streams the answer shown on the page to your terminal. Settings are changed by operating the switches and menus of the Run settings panel.

- You sign in to your Google account yourself, once, in the browser window that `aistudio login` opens. The CLI never sees your password.
- It is the web app itself: no API key is needed, and you get the web app's free usage and model selection. Chats are also saved to your AI Studio history (Google Drive), just as in the browser.
- Your everyday browser profile is never touched.
- The older mode that calls the Gemini API with an API key is still available with `--api`.

If AI Studio's layout changes and buttons can no longer be found, run `aistudio debug` to capture the page and include it in a bug report (see [Troubleshooting](#troubleshooting)).

## Installation

### macOS / Linux

```sh
curl -fsSL https://raw.githubusercontent.com/igarinpiano/ai-studio-cli/main/install.sh | sh
```

Installs to `~/.local/bin/aistudio`. The Linux build is statically linked (musl), so it runs on any distribution.

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/igarinpiano/ai-studio-cli/main/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\aistudio\aistudio.exe` and adds it to your user PATH (takes effect in new terminals). x64 and ARM64 are supported.

### Installer options

| Environment variable | Meaning |
| --- | --- |
| `AISTUDIO_VERSION` | Version to install (e.g. `v0.1.0`). Defaults to the latest release |
| `AISTUDIO_INSTALL_DIR` | Installation directory |
| `GITHUB_TOKEN` / `GH_TOKEN` | Token for downloading from a private repository. Taken from `gh auth token` automatically when the GitHub CLI is signed in |

```sh
# Example: pick the version and the installation directory
curl -fsSL https://raw.githubusercontent.com/igarinpiano/ai-studio-cli/main/install.sh | AISTUDIO_VERSION=v0.1.0 AISTUDIO_INSTALL_DIR=/usr/local/bin sh
```

The install scripts verify the archive against the release's `SHA256SUMS`. Every release also carries `install.sh` and `install.ps1`.

### Manual / from source

- Download the archive for your platform from [Releases](https://github.com/igarinpiano/ai-studio-cli/releases) and put the `aistudio` (`aistudio.exe`) inside it somewhere on your PATH
- Or, with a Rust toolchain: `cargo install --git https://github.com/igarinpiano/ai-studio-cli`

### Uninstall

Delete the binary (macOS / Linux: `rm ~/.local/bin/aistudio`; Windows: delete `%LOCALAPPDATA%\Programs\aistudio`). Settings and history live in the directory listed under [Files](#files).

## Getting started: sign in

You need Chrome, Edge, Chromium or Brave. The CLI finds it automatically; if yours is somewhere unusual, point `AISTUDIO_BROWSER` at its executable.

```sh
aistudio login
```

1. A browser window opens. Sign in with your Google account (the first time, also accept AI Studio's terms and any other prompts on the page).
2. When the chat screen appears, come back to the terminal and press Enter.

The sign-in is kept in the CLI's own browser profile, and from then on the browser runs without a window. `aistudio logout` deletes that profile.

### Using an API key instead (`--api`)

Save an [API key](https://aistudio.google.com/apikey) with `aistudio login --api` (or set `GEMINI_API_KEY`) and start with `aistudio --api`. To always use the API, put `"backend": "api"` in the config file. API mode additionally supports safety settings, JSON schemas, function declarations, top-K, thinking budgets, `/retry`, `/undo` and `/code`.

## Usage

### Interactive

```sh
aistudio
```

```
Google AI Studio CLI 0.1.0
via AI Studio in your browser (signed-in Google account)
gemini-3.8-flash · thinking: medium · tools: search
/help for commands · """ or Alt+Enter for multi-line input · /edit opens $EDITOR · Ctrl+C stops a response · Ctrl+D quits

› Hello
```

At start, the model and settings are read from AI Studio's Run settings panel as they are.

- **Enter** sends. **Alt+Enter** or **Ctrl+J** inserts a newline. A line with just `"""` starts and ends a multi-line block, and a trailing `\` continues on the next line
- `/edit` opens `$VISUAL` / `$EDITOR` (vi if unset, Notepad on Windows) for writing longer messages
- **Tab** completes commands, model names and file names
- **Ctrl+C** while generating stops the answer (the part received so far is kept). **Ctrl+D** or `/exit` quits
- Thought summaries are shown dimmed, search queries and sources below the answer, and the model and timing in the footer

### One-shot

```sh
aistudio "Explain RAII in two sentences"
cat main.rs | aistudio -t high "Review this code"
aistudio -f receipt.jpg --json "Extract the fields"
aistudio -c "And in Rust?"        # continue the most recent chat
aistudio -r 3 -i                  # open saved chat no. 3 and stay interactive
```

When the output goes to a pipe or a file, only the plain answer text goes to stdout; thoughts, sources and timing go to stderr.

### Vim keybindings

```sh
aistudio --vim        # for this session
```

In interactive mode, `/vim on` (and `/vim off` to go back to Emacs-style keys) is saved, so later sessions start in Vim mode too. The input line then works in vi mode: `Esc` for normal mode, `i` / `a` to insert, `h` `l` `w` `b` `0` `$` to move, `x` `dw` `dd` `cw` `p` `u` `.` to edit, `k` / `j` for input history and `Ctrl+R` to search it. Pressing `v` in normal mode opens the current line in `$EDITOR`; save and quit to send it (as in the vi mode of bash and zsh).

### Commands (interactive)

| Command | Description |
| --- | --- |
| `/model [name\|number]` | Switch model (no argument: pick from a list) |
| `/models` | List available models (read from AI Studio's model picker) |
| `/thinking [value]` | `default` / `minimal` / `low` / `medium` / `high`, or `off` / `dynamic` / a token count (thinking budget, API mode) |
| `/thoughts on\|off` | Show thought summaries |
| `/system [text\|clear]` | System instructions (no argument: multi-line input) |
| `/settings [save\|reset]` | Show the run settings; `save` applies them at every start |
| `/set <key> <value>` | Change any setting (no argument: list the keys) |
| `/tools` | Show which tools are on |
| `/search`, `/maps`, `/url`, `/codeexec`, `/json` `[on\|off]` | Grounding with Google Search / Google Maps / URL context / code execution / structured outputs |
| `/schema <file\|json\|clear>` | JSON schema for structured outputs (API mode) |
| `/functions <file\|json\|clear>` | Function declarations for function calling (API mode); when the model calls a function you are asked for its result, as in AI Studio |
| `/safety [category threshold]` | Safety settings (API mode), e.g. `/safety all none`, `/safety hate high` |
| `/media <default\|low\|medium\|high>` | Media resolution |
| `/temp`, `/maxtokens`, `/stop` | Temperature / output length / stop sequence |
| `/attach <file...>` / `/detach` | Attach files to the next message / remove them |
| `/retry` / `/undo` | Regenerate the last answer / remove the last exchange (API mode) |
| `/new` | New chat |
| `/chats` / `/load <number>` / `/rename <title>` | List / open / rename saved chats |
| `/export <file.md\|file.json>` | Export the chat |
| `/edit` | Write a message in `$EDITOR` and send it |
| `/vim [on\|off]` | Vim keybindings (saved) |
| `/debug` | Save a screenshot and the HTML of the AI Studio page (for bug reports) |
| `/code` | Show the current settings and chat as a Gemini API `curl` command (like "Get code") |
| `/key` | Set the API key (for `--api`) |
| `/render markdown\|raw` | Output style |

### Run settings

| AI Studio | Interactive | Command line |
| --- | --- | --- |
| Model | `/model` | `-m gemini-3.8-flash` |
| System instructions | `/system` | `-s "..."`, `--system-file FILE` |
| Thinking level | `/thinking medium` | `-t medium` |
| Structured outputs | `/json`, `/schema FILE` | `--json`, `--schema FILE` |
| Code execution | `/codeexec on` | `--code-execution` |
| Function calling | `/functions FILE` | `--functions FILE` |
| Grounding with Google Search | `/search on\|off` | `--search` / `--no-search` |
| Grounding with Google Maps | `/maps on` | `--maps` |
| URL context | `/url on` | `--url-context` |
| Media resolution | `/media low` | `--media-resolution low` |
| Safety settings | `/safety all none` | `--safety all=none` |
| Add stop sequence | `/stop END` | `--stop END` |
| Output length | `/maxtokens 65536` | `--max-tokens 65536` |
| Temperature / Top P / Top K | `/temp 1.0`, `/set top_p 0.95`, `/set top_k 40` | `--temperature`, `--top-p`, `--top-k` |

Changed settings are applied to the AI Studio page right before the next message is sent; anything that could not be applied is reported as a warning. `/settings save` applies the current settings at every start, and `/settings reset` goes back to AI Studio's own settings.

Not yet supported in web mode (available with `--api`): safety settings, editing the structured output schema and function declarations (switching them on and off works), top-K, thinking budgets, `/retry` and `/undo`, and answering function calls. For now, do these in the browser itself: start with `aistudio --headed` and use the window it shows.

### Other subcommands

```sh
aistudio models                  # list available models
aistudio chats                   # list saved chats
aistudio config                  # config file location, API key source, defaults
aistudio login / logout          # with --api: save / remove the API key
aistudio debug                   # capture the page state (for bug reports)
```

### Main options

| Option | Meaning |
| --- | --- |
| `--headed` | Show the browser window (if signing in doesn't carry over to the hidden browser, or to watch what happens). To always do this, set `"headless": false` in the config file |
| `--api` | Use the Gemini API (API key) |
| `--vim` / `--emacs` | Keybindings for the input line |
| `-c` / `-r <number>` | Continue the most recent / a given chat (opens that chat's page in AI Studio) |

## Files

| OS | Location |
| --- | --- |
| macOS | `~/Library/Application Support/ai-studio-cli/` |
| Linux | `~/.config/ai-studio-cli/` (respects `$XDG_CONFIG_HOME`) |
| Windows | `%APPDATA%\ai-studio-cli\` |

It holds `config.json` (settings), `browser-profile/` (the CLI's own browser profile, which keeps your sign-in), `chats/` (chat history), `debug/` (output of `aistudio debug`) and `input_history.txt`. Set `AISTUDIO_CLI_HOME` to use another location.

## Troubleshooting

- **"Not signed in"**: run `aistudio login` again. If it still appears after signing in, try `aistudio --headed` (some accounts are not accepted in a hidden browser).
- **Buttons or settings not found, or no answer read**: AI Studio's layout has probably changed. `aistudio debug` saves `screenshot.png`, `page.html`, `outline.txt` and `state.json`; please attach them to a bug report (they contain your chats, so check them before sharing). With `AISTUDIO_DEBUG=1`, the CLI prints what it reads from the page while an answer streams.
- **Patching selectors without a new build**: a `web-selectors.json` in the files directory adds ways to find page elements (same keys as `SEL` in `src/web/agent.js`, e.g. `{"runButton": ["button.my-run"]}`).
- **Containers running as root**: where Chrome only starts without its sandbox, set `AISTUDIO_BROWSER_ARGS="--no-sandbox"`.

## Development

```sh
cargo run              # interactive mode
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Web mode: `src/web/browser.rs` starts the browser, `src/web/cdp.rs` connects to it over the Chrome DevTools Protocol, and `src/web/agent.js` is injected into the page to drive it. For testing or mirrors, `AISTUDIO_WEB_URL` (the page to open) and `AISTUDIO_BROWSER_ARGS` (extra browser flags) override the defaults.

API mode uses the OS TLS stack on Windows and macOS (SChannel / Security.framework, so certificates trusted by the OS, such as corporate proxy CAs, are trusted) and rustls with ring on Linux. Proxies follow `HTTPS_PROXY` and friends, and the OS settings. `GEMINI_API_BASE` overrides the API endpoint (for testing or relays).

## Releasing

1. Bump `version` in `Cargo.toml` and merge to main
2. Run **Actions → release → Run workflow** on GitHub (entering the same number as `version` checks that they match)
3. Only when every target builds are the `v<version>` tag and the GitHub Release created, with the archives, `SHA256SUMS` and the install scripts attached

Targets (`.github/workflows/reusable-build-matrix.yml`): macOS (arm64 / x86_64), Windows (x64 / ARM64), Linux (x86_64 / aarch64, glibc and musl). **Actions → build-check** builds them all without publishing. Versions with a hyphen, like `1.0.0-beta.1`, become pre-releases and are skipped by the install scripts' "latest". When the repository is public, releases also get build provenance attestations.

## License

Licensed under the [Apache License, Version 2.0](LICENSE). Copyright 2026 Igarin.

This project is not affiliated with, endorsed by, or sponsored by Google. "Google", "Gemini" and "Google AI Studio" are trademarks of Google LLC.
