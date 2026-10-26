//! Terminal output: streamed markdown styling, thoughts, tool blocks and the
//! sources/usage footer.

use std::io::{IsTerminal, Write};
use std::sync::OnceLock;
use std::time::Duration;

use anstyle::{AnsiColor, Color, Effects, Style};
use serde_json::Value;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

use crate::conversation::TurnResult;

pub fn fg(c: AnsiColor) -> Style {
    Style::new().fg_color(Some(Color::Ansi(c)))
}

pub fn dim() -> Style {
    Style::new().effects(Effects::DIMMED)
}

pub fn paint(style: Style, text: &str) -> String {
    format!("{}{}{}", style.render(), text, style.render_reset())
}

pub fn out(s: &str) {
    let mut o = anstream::stdout().lock();
    let _ = o.write_all(s.as_bytes());
    let _ = o.flush();
}

pub fn err(s: &str) {
    let mut e = anstream::stderr().lock();
    let _ = e.write_all(s.as_bytes());
    let _ = e.flush();
}

pub fn info(msg: &str) {
    out(&format!("{}\n", paint(dim(), msg)));
}

pub fn success(msg: &str) {
    out(&format!("{}\n", paint(fg(AnsiColor::Green), msg)));
}

pub fn warn(msg: &str) {
    err(&format!("{}\n", paint(fg(AnsiColor::Yellow), msg)));
}

pub fn error(msg: &str) {
    err(&format!("{}\n", paint(fg(AnsiColor::Red).bold(), msg)));
}

// ---------------------------------------------------------------------------
// Syntax highlighting

fn truecolor() -> bool {
    static TC: OnceLock<bool> = OnceLock::new();
    *TC.get_or_init(|| {
        let ct = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
        if ct.contains("truecolor") || ct.contains("24bit") {
            return true;
        }
        if std::env::var_os("WT_SESSION").is_some() {
            return true; // Windows Terminal
        }
        let tp = std::env::var("TERM_PROGRAM").unwrap_or_default();
        matches!(tp.as_str(), "iTerm.app" | "vscode" | "WezTerm" | "ghostty" | "Hyper" | "Tabby")
    })
}

struct Highlight {
    syntaxes: SyntaxSet,
    theme: Theme,
}

fn highlighter() -> &'static Highlight {
    static H: OnceLock<Highlight> = OnceLock::new();
    H.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults();
        let theme = themes.themes.remove("base16-ocean.dark").unwrap_or_default();
        Highlight { syntaxes: SyntaxSet::load_defaults_newlines(), theme }
    })
}

struct CodeBlock {
    fence: char,
    hl: Option<HighlightLines<'static>>,
}

impl CodeBlock {
    fn new(lang: &str, fence: char) -> CodeBlock {
        let hl = if truecolor() && !lang.is_empty() {
            let h = highlighter();
            h.syntaxes.find_syntax_by_token(lang).map(|syntax| HighlightLines::new(syntax, &h.theme))
        } else {
            None
        };
        CodeBlock { fence, hl }
    }

    fn line(&mut self, line: &str) -> String {
        if let Some(hl) = self.hl.as_mut() {
            let with_nl = format!("{line}\n");
            if let Ok(ranges) = hl.highlight_line(&with_nl, &highlighter().syntaxes) {
                let mut s = as_24_bit_terminal_escaped(&ranges, false);
                s.push_str("\x1b[0m");
                return s;
            }
        }
        format!("{}\n", paint(fg(AnsiColor::Cyan), line))
    }
}

pub fn highlight_block(lang: &str, code: &str) -> String {
    let mut block = CodeBlock::new(lang, '`');
    code.trim_end_matches('\n').lines().map(|l| block.line(l)).collect()
}

// ---------------------------------------------------------------------------
// Streaming markdown

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Normal,
    Header,
    Quote,
}

/// Styles markdown as it streams in, without re-rendering: block markers are
/// recognised at the start of each line and inline `**bold**`, `*italic*` and
/// `code` toggle styles. Code blocks are highlighted line by line.
pub struct MdStream {
    base: Style,
    line_start: bool,
    head: String,
    kind: LineKind,
    code: Option<CodeBlock>,
    code_line: String,
    bold: bool,
    italic: bool,
    code_span: bool,
    pending: String,
    prev: char,
    active: Option<Style>,
}

impl MdStream {
    pub fn new(base: Style) -> MdStream {
        MdStream {
            base,
            line_start: true,
            head: String::new(),
            kind: LineKind::Normal,
            code: None,
            code_line: String::new(),
            bold: false,
            italic: false,
            code_span: false,
            pending: String::new(),
            prev: ' ',
            active: None,
        }
    }

    pub fn feed(&mut self, text: &str) -> String {
        let mut o = String::new();
        for ch in text.chars() {
            self.push(ch, &mut o);
        }
        o
    }

    /// Flush anything held back (end of stream).
    pub fn finish(&mut self) -> String {
        let mut o = String::new();
        if self.code.is_some() {
            if !self.code_line.is_empty() {
                let line = std::mem::take(&mut self.code_line);
                self.code_line_out(&line, &mut o);
            }
        } else if self.line_start && !self.head.is_empty() {
            self.start_line(&mut o);
        }
        self.flush_pending(' ', &mut o);
        self.set_style(None, &mut o);
        o
    }

    fn push(&mut self, ch: char, o: &mut String) {
        if self.code.is_some() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.code_line);
                self.code_line_out(&line, o);
            } else {
                self.code_line.push(ch);
            }
            return;
        }
        if self.line_start {
            self.head.push(ch);
            if ch == '\n' || self.head_decided() {
                self.start_line(o);
            }
            return;
        }
        self.inline(ch, o);
    }

    fn code_line_out(&mut self, line: &str, o: &mut String) {
        let fence = self.code.as_ref().map(|c| c.fence).unwrap_or('`');
        let t = line.trim_start();
        if t.starts_with(&fence.to_string().repeat(3)) && t.trim_end().chars().all(|c| c == fence) {
            self.code = None;
            o.push_str(&paint(dim(), line));
            o.push('\n');
            return;
        }
        let rendered = self.code.as_mut().map(|c| c.line(line)).unwrap_or_default();
        o.push_str(&rendered);
    }

    /// Whether enough of the line is buffered to know what kind of line it is.
    fn head_decided(&self) -> bool {
        let t = self.head.trim_start_matches(' ');
        if t.is_empty() {
            return false;
        }
        let first = t.chars().next().unwrap_or(' ');
        if first == '`' || first == '~' {
            // A fence needs its whole line (for the language tag).
            let run = t.chars().take_while(|c| *c == first).count();
            let could_be_fence = run >= 3 || run == t.chars().count();
            return !could_be_fence;
        }
        if self.head.chars().count() > 12 {
            return true;
        }
        match first {
            '#' => !t.chars().all(|c| c == '#'),
            '>' => t.chars().count() >= 2,
            '-' | '*' | '_' | '+' => {
                // Could still become a list item, a rule ("---") or "**bold**".
                !t.chars().all(|c| c == first || c == ' ')
            }
            '0'..='9' => {
                let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
                let after: String = t.chars().skip(digits).collect();
                !(after.is_empty() || after == "." || after == ")")
            }
            _ => true,
        }
    }

    fn start_line(&mut self, o: &mut String) {
        self.line_start = false;
        let head = std::mem::take(&mut self.head);
        let t = head.trim_start_matches(' ');
        let indent = &head[..head.len() - t.len()];
        let line = t.trim_end_matches(['\n', '\r']);

        // Code fence
        for fence in ['`', '~'] {
            let marker = fence.to_string().repeat(3);
            if line.starts_with(&marker) && head.ends_with('\n') {
                let lang = line.trim_start_matches(fence).trim();
                let lang = lang.split_whitespace().next().unwrap_or("");
                self.code = Some(CodeBlock::new(lang, fence));
                o.push_str(&paint(dim(), head.trim_end_matches(['\n', '\r'])));
                o.push('\n');
                self.line_start = true;
                return;
            }
        }
        // Horizontal rule
        if head.ends_with('\n') {
            let compact: String = line.chars().filter(|c| *c != ' ').collect();
            if compact.chars().count() >= 3
                && ["-", "*", "_"].iter().any(|m| compact.chars().all(|c| c.to_string() == *m))
            {
                o.push_str(&paint(dim(), &"─".repeat(40)));
                o.push('\n');
                self.line_start = true;
                return;
            }
        }
        // Header
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
            self.kind = LineKind::Header;
            self.feed_inline_str(t[hashes..].trim_start_matches(' '), o);
            return;
        }
        // Bullet list
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = t.strip_prefix(marker) {
                o.push_str(indent);
                o.push_str(&paint(fg(AnsiColor::Cyan), "•"));
                o.push(' ');
                self.prev = ' ';
                self.feed_inline_str(rest, o);
                return;
            }
        }
        // Ordered list
        let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") ")) {
            o.push_str(indent);
            o.push_str(&paint(fg(AnsiColor::Cyan), &t[..digits + 1]));
            o.push(' ');
            self.prev = ' ';
            self.feed_inline_str(&t[digits + 2..], o);
            return;
        }
        // Block quote
        if let Some(rest) = t.strip_prefix('>') {
            self.kind = LineKind::Quote;
            o.push_str(indent);
            o.push_str(&paint(fg(AnsiColor::Green), "│"));
            o.push(' ');
            self.feed_inline_str(rest.strip_prefix(' ').unwrap_or(rest), o);
            return;
        }
        self.feed_inline_str(&head, o);
    }

    fn feed_inline_str(&mut self, s: &str, o: &mut String) {
        for ch in s.chars() {
            self.inline(ch, o);
        }
    }

    fn inline(&mut self, ch: char, o: &mut String) {
        let marker = ch == '`' || (ch == '*' && !self.code_span);
        if marker && (self.pending.is_empty() || self.pending.ends_with(ch)) {
            self.pending.push(ch);
            return;
        }
        self.flush_pending(ch, o);
        if marker {
            self.pending.push(ch);
            return;
        }
        if ch == '\n' {
            self.set_style(None, o);
            o.push('\n');
            self.bold = false;
            self.italic = false;
            self.code_span = false;
            self.kind = LineKind::Normal;
            self.line_start = true;
            self.prev = ' ';
            return;
        }
        self.emit(ch, o);
    }

    fn flush_pending(&mut self, next: char, o: &mut String) {
        if self.pending.is_empty() {
            return;
        }
        let p = std::mem::take(&mut self.pending);
        let opens = !next.is_whitespace() && (self.prev.is_whitespace() || !self.prev.is_alphanumeric());
        let closes = !self.prev.is_whitespace();
        if p.starts_with('`') {
            // Inline code toggles on any backtick run.
            self.code_span = !self.code_span;
            return;
        }
        let n = p.chars().count();
        let toggle_bold = n >= 2 && if self.bold { closes } else { opens };
        let toggle_italic = n % 2 == 1 && if self.italic { closes } else { opens };
        if n <= 3 && (toggle_bold || toggle_italic) && (n != 3 || (toggle_bold && toggle_italic)) {
            if toggle_bold {
                self.bold = !self.bold;
            }
            if toggle_italic {
                self.italic = !self.italic;
            }
            return;
        }
        for c in p.chars() {
            self.emit(c, o);
        }
    }

    fn emit(&mut self, ch: char, o: &mut String) {
        let mut style = self.base;
        match self.kind {
            LineKind::Header => style = style.fg_color(Some(Color::Ansi(AnsiColor::BrightBlue))).bold(),
            LineKind::Quote => style = style.italic(),
            LineKind::Normal => {}
        }
        if self.bold {
            style = style.bold();
        }
        if self.italic {
            style = style.italic();
        }
        if self.code_span {
            style = style.fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
        }
        self.set_style(Some(style), o);
        o.push(ch);
        self.prev = ch;
    }

    fn set_style(&mut self, style: Option<Style>, o: &mut String) {
        if self.active == style {
            return;
        }
        if self.active.is_some_and(|a| a != Style::new()) {
            o.push_str(&anstyle::Reset.render().to_string());
        }
        if let Some(s) = style
            && s != Style::new()
        {
            o.push_str(&s.render().to_string());
        }
        self.active = style;
    }
}

// ---------------------------------------------------------------------------
// Response renderer

pub enum Event {
    Thought(String),
    Text(String),
    Code { lang: String, code: String },
    CodeResult { outcome: String, output: String },
    FunctionCall { name: String, args: Value },
    Image { path: String },
}

#[derive(PartialEq, Clone, Copy)]
enum Section {
    None,
    Thought,
    Text,
    Block,
}

pub struct Renderer {
    /// Styled output on a terminal; otherwise plain answer text on stdout and
    /// everything else on stderr.
    pretty: bool,
    show_thoughts: bool,
    section: Section,
    thought_md: MdStream,
    text_md: MdStream,
    /// Last char written to stdout, to know whether a newline is needed.
    last_out: char,
}

impl Renderer {
    pub fn new(pretty: bool, show_thoughts: bool) -> Renderer {
        Renderer {
            pretty,
            show_thoughts,
            section: Section::None,
            thought_md: MdStream::new(dim().italic()),
            text_md: MdStream::new(Style::new()),
            last_out: '\n',
        }
    }

    pub fn has_output(&self) -> bool {
        self.section != Section::None
    }

    fn write(&mut self, s: &str) {
        if let Some(c) = s.chars().last() {
            self.last_out = c;
        }
        out(s);
    }

    fn newline_if_needed(&mut self) {
        if self.last_out != '\n' {
            self.write("\n");
        }
    }

    fn close_section(&mut self) {
        match self.section {
            Section::Thought => {
                let rest = self.thought_md.finish();
                self.write(&rest);
                self.newline_if_needed();
                self.write("\n");
                self.thought_md = MdStream::new(dim().italic());
            }
            Section::Text => {
                let rest = self.text_md.finish();
                self.write(&rest);
                self.newline_if_needed();
                self.text_md = MdStream::new(Style::new());
            }
            Section::Block | Section::None => {}
        }
    }

    fn enter(&mut self, section: Section) {
        if self.section == section {
            return;
        }
        self.close_section();
        // Thoughts end with their own blank line; other sections get one here.
        if matches!(self.section, Section::Text | Section::Block) && section != Section::Thought {
            self.write("\n");
        }
        self.section = section;
    }

    pub fn event(&mut self, ev: Event) {
        if !self.pretty {
            self.plain_event(ev);
            return;
        }
        match ev {
            Event::Thought(t) => {
                if !self.show_thoughts {
                    return;
                }
                if self.section != Section::Thought {
                    self.enter(Section::Thought);
                    self.write(&format!("{}\n", paint(dim().bold(), "✻ Thinking")));
                }
                let s = self.thought_md.feed(&t);
                self.write(&s);
            }
            Event::Text(t) => {
                self.enter(Section::Text);
                let s = self.text_md.feed(&t);
                self.write(&s);
            }
            Event::Code { lang, code } => {
                self.enter(Section::Block);
                self.section = Section::None;
                let label = format!("▶ Code ({})", if lang.is_empty() { "code" } else { &lang });
                self.write(&format!("{}\n", paint(fg(AnsiColor::Cyan).bold(), &label)));
                let body = highlight_block(&lang, &code);
                self.write(&body);
                self.section = Section::Block;
            }
            Event::CodeResult { outcome, output } => {
                self.enter(Section::Block);
                self.section = Section::None;
                let ok = outcome.is_empty() || outcome == "OUTCOME_OK";
                let color = if ok { AnsiColor::Green } else { AnsiColor::Red };
                let label = if ok { "◀ Output".to_string() } else { format!("◀ Output ({outcome})") };
                self.write(&format!("{}\n", paint(fg(color).bold(), &label)));
                let body = output.trim_end_matches('\n');
                if !body.is_empty() {
                    self.write(&format!("{}\n", paint(dim(), body)));
                }
                self.section = Section::Block;
            }
            Event::FunctionCall { name, args } => {
                self.enter(Section::Block);
                self.section = Section::None;
                self.write(&format!("{}\n", paint(fg(AnsiColor::Magenta).bold(), &format!("ƒ Function call: {name}"))));
                let pretty = serde_json::to_string_pretty(&args).unwrap_or_default();
                let body = highlight_block("json", &pretty);
                self.write(&body);
                self.section = Section::Block;
            }
            Event::Image { path } => {
                self.enter(Section::Block);
                self.section = Section::None;
                self.write(&format!("{}\n", paint(fg(AnsiColor::Cyan), &format!("[image] saved to {path}"))));
                self.section = Section::Block;
            }
        }
    }

    fn plain_event(&mut self, ev: Event) {
        match ev {
            Event::Thought(t) => {
                if self.show_thoughts && std::io::stderr().is_terminal() {
                    err(&paint(dim().italic(), &t));
                    self.section = Section::Thought;
                }
            }
            Event::Text(t) => {
                if self.section == Section::Thought {
                    err("\n\n");
                }
                self.section = Section::Text;
                self.write(&t);
            }
            Event::Code { lang, code } => {
                self.newline_if_needed();
                self.write(&format!("```{}\n{}\n```\n", lang, code.trim_end_matches('\n')));
                self.section = Section::Block;
            }
            Event::CodeResult { output, .. } => {
                self.newline_if_needed();
                self.write(&format!("```\n{}\n```\n", output.trim_end_matches('\n')));
                self.section = Section::Block;
            }
            Event::FunctionCall { name, args } => {
                err(&format!("function call: {name}({args})\n"));
                self.section = Section::Block;
            }
            Event::Image { path } => {
                err(&format!("image saved to {path}\n"));
                self.section = Section::Block;
            }
        }
    }

    /// Close open styles and print sources, warnings and usage.
    pub fn finish(&mut self, result: &TurnResult, model: &str, elapsed: Duration) {
        if self.pretty {
            self.close_section();
            self.newline_if_needed();
        } else if self.section != Section::None && self.last_out != '\n' {
            self.write("\n");
        }
        let meta = footer(result, model, elapsed, self.pretty);
        if meta.is_empty() {
            return;
        }
        if self.pretty {
            out(&meta);
        } else {
            err(&meta);
        }
    }
}

fn footer(r: &TurnResult, model: &str, elapsed: Duration, pretty: bool) -> String {
    let mut s = String::new();
    if r.interrupted {
        s.push_str(&format!("{}\n", paint(fg(AnsiColor::Yellow), "[interrupted]")));
    }
    if let Some(reason) = &r.block_reason {
        let msg = r.finish_message.as_deref().unwrap_or("");
        s.push_str(&format!("{}\n", paint(fg(AnsiColor::Red).bold(), &format!("Prompt blocked: {reason} {msg}"))));
    }
    if let Some(reason) = r.finish_reason.as_deref().filter(|f| *f != "STOP" && !f.is_empty()) {
        let hint = match reason {
            "MAX_TOKENS" => " — raise the limit with /set output_length <n>",
            "SAFETY" => " — see /safety",
            _ => "",
        };
        let msg = r.finish_message.as_deref().map(|m| format!(": {m}")).unwrap_or_default();
        s.push_str(&format!("{}\n", paint(fg(AnsiColor::Yellow), &format!("Stopped: {reason}{msg}{hint}"))));
    }
    if let Some(g) = &r.grounding {
        let queries: Vec<&str> = g
            .get("webSearchQueries")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let sources: Vec<(String, String)> = g
            .get("groundingChunks")
            .and_then(Value::as_array)
            .map(|chunks| {
                chunks
                    .iter()
                    .filter_map(|c| c.get("web").or_else(|| c.get("maps")).or_else(|| c.get("retrievedContext")))
                    .map(|w| {
                        (
                            w.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                            w.get("uri").and_then(Value::as_str).unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !queries.is_empty() || !sources.is_empty() {
            s.push('\n');
        }
        if !queries.is_empty() {
            s.push_str(&format!("{} {}\n", paint(dim().bold(), "Searched:"), paint(dim(), &queries.join(" · "))));
        }
        if !sources.is_empty() {
            s.push_str(&format!("{}\n", paint(dim().bold(), "Sources:")));
            for (i, (title, uri)) in sources.iter().enumerate() {
                let title = if title.is_empty() { uri.as_str() } else { title.as_str() };
                s.push_str(&format!(
                    "  {} {}  {}\n",
                    paint(fg(AnsiColor::Cyan), &format!("[{}]", i + 1)),
                    title,
                    paint(dim(), uri)
                ));
            }
        }
    }
    if let Some(u) = &r.url_context {
        let urls: Vec<String> = u
            .get("urlMetadata")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|m| {
                        let url = m.get("retrievedUrl").and_then(Value::as_str).unwrap_or("?");
                        let status = m
                            .get("urlRetrievalStatus")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim_start_matches("URL_RETRIEVAL_STATUS_")
                            .to_lowercase();
                        format!("  {url} ({status})")
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !urls.is_empty() {
            s.push_str(&format!("{}\n{}\n", paint(dim().bold(), "URL context:"), paint(dim(), &urls.join("\n"))));
        }
    }
    if pretty || std::io::stderr().is_terminal() {
        let mut bits = vec![r.model_version.clone().unwrap_or_else(|| model.to_string())];
        bits.push(format!("{:.1}s", elapsed.as_secs_f64()));
        if let Some(u) = &r.usage {
            let n = |k: &str| u.get(k).and_then(Value::as_u64);
            if let Some(v) = n("promptTokenCount") {
                bits.push(format!("{v} in"));
            }
            if let Some(v) = n("candidatesTokenCount") {
                bits.push(format!("{v} out"));
            }
            if let Some(v) = n("thoughtsTokenCount") {
                bits.push(format!("{v} thinking"));
            }
            if let Some(v) = n("toolUsePromptTokenCount") {
                bits.push(format!("{v} tool"));
            }
        }
        s.push_str(&format!("{}\n", paint(dim(), &bits.join(" · "))));
    }
    s
}

/// One-line spinner on stderr while waiting for the first token.
pub struct Spinner {
    enabled: bool,
    frame: usize,
    width: usize,
}

impl Spinner {
    pub fn new() -> Spinner {
        Spinner { enabled: std::io::stderr().is_terminal(), frame: 0, width: 0 }
    }

    pub fn tick(&mut self, label: &str, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let text = format!("{} {label} {:.0}s", FRAMES[self.frame % FRAMES.len()], elapsed.as_secs_f64());
        self.frame += 1;
        self.width = self.width.max(text.chars().count());
        err(&format!("\r{}", paint(dim(), &text)));
    }

    pub fn clear(&mut self) {
        if self.enabled && self.width > 0 {
            err(&format!("\r{}\r", " ".repeat(self.width)));
            self.width = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for d in it.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn render_chunks(chunks: &[&str]) -> String {
        let mut md = MdStream::new(Style::new());
        let mut o = String::new();
        for c in chunks {
            o.push_str(&md.feed(c));
        }
        o.push_str(&md.finish());
        o
    }

    #[test]
    fn markdown_markers_are_rendered_regardless_of_chunking() {
        let src = "# Title\nSome **bold** and *it* and `code`.\n- item one\n1. first\n> quote\n---\n2 * 3 = 6\n```python\nprint('hi')\n```\nafter";
        let expected = "Title\nSome bold and it and code.\n• item one\n1. first\n│ quote\n────────────────────────────────────────\n2 * 3 = 6\n```python\nprint('hi')\n```\nafter";
        assert_eq!(strip(&render_chunks(&[src])), expected);
        let chars: Vec<String> = src.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = chars.iter().map(String::as_str).collect();
        assert_eq!(strip(&render_chunks(&refs)), expected);
    }

    #[test]
    fn bold_at_line_start_is_not_a_rule() {
        assert_eq!(strip(&render_chunks(&["**Note:** ok\n"])), "Note: ok\n");
        assert_eq!(strip(&render_chunks(&["***\n"])), format!("{}\n", "─".repeat(40)));
    }

    #[test]
    fn styles_are_reset_at_end() {
        let s = render_chunks(&["**unterminated"]);
        assert!(s.ends_with("\x1b[0m"));
        assert_eq!(strip(&s), "unterminated");
    }

    #[test]
    fn multibyte_text_passes_through() {
        assert_eq!(strip(&render_chunks(&["Grüße, ", "**naïve café** ✓\n"])), "Grüße, naïve café ✓\n");
    }
}
