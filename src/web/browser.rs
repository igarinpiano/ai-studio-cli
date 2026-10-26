//! Finding, launching and connecting to the user's Chromium-based browser.
//!
//! The browser runs with its own profile under the app directory, so the
//! Google sign-in made with `aistudio login` is kept there and nothing touches
//! the user's everyday browser profile.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::config;

pub fn profile_dir() -> PathBuf {
    config::app_home().join("browser-profile")
}

fn candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        for app in [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
            "Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ] {
            out.push(Path::new("/Applications").join(app));
            if let Some(home) = dirs::home_dir() {
                out.push(home.join("Applications").join(app));
            }
        }
    }
    #[cfg(windows)]
    {
        let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
            .collect();
        for root in &roots {
            for exe in [
                r"Google\Chrome\Application\chrome.exe",
                r"Microsoft\Edge\Application\msedge.exe",
                r"BraveSoftware\Brave-Browser\Application\brave.exe",
                r"Chromium\Application\chrome.exe",
            ] {
                out.push(root.join(exe));
            }
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let path = std::env::var_os("PATH").unwrap_or_default();
        for name in [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
            "brave-browser",
        ] {
            for dir in std::env::split_paths(&path) {
                out.push(dir.join(name));
            }
        }
        out.push(PathBuf::from("/snap/bin/chromium"));
    }
    out
}

/// The browser to use: `AISTUDIO_BROWSER`, the config, or the first one found.
pub fn find_browser(configured: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("AISTUDIO_BROWSER").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    if let Some(p) = configured.filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    candidates().into_iter().find(|p| p.is_file()).ok_or_else(|| {
        "No Chrome, Edge, Chromium or Brave found. Install one, or point AISTUDIO_BROWSER at its executable.".into()
    })
}

pub struct LaunchOptions {
    pub executable: PathBuf,
    pub headless: bool,
    /// Expose the DevTools endpoint. Off for `aistudio login`, so the sign-in
    /// window is an ordinary browser window.
    pub remote_debugging: bool,
    pub url: String,
}

pub struct Browser {
    pub child: Option<Child>,
    pub ws_url: Option<String>,
}

impl Browser {
    pub fn launch(opts: &LaunchOptions) -> Result<Browser, String> {
        let profile = profile_dir();
        fs::create_dir_all(&profile).map_err(|e| format!("cannot create {}: {e}", profile.display()))?;
        let port_file = profile.join("DevToolsActivePort");

        // A browser from an earlier session that is still running with this profile.
        if opts.remote_debugging
            && let Some(url) = read_port_file(&port_file).filter(|u| endpoint_alive(u))
        {
            return Ok(Browser { child: None, ws_url: Some(url) });
        }
        let _ = fs::remove_file(&port_file);

        let mut cmd = Command::new(&opts.executable);
        cmd.arg(format!("--user-data-dir={}", profile.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            // English UI, so the page can be read reliably.
            .arg("--lang=en-US")
            .arg("--accept-lang=en-US,en");
        if opts.remote_debugging {
            cmd.arg("--remote-debugging-port=0");
        }
        if opts.headless {
            cmd.arg("--headless=new").arg("--window-size=1440,1000");
        } else {
            cmd.arg("--new-window");
        }
        // Its own process group, so Ctrl+C in the terminal (which stops a
        // response) doesn't also reach the browser.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }
        // Extra flags from the environment, e.g. --no-sandbox in a root container.
        if let Ok(extra) = std::env::var("AISTUDIO_BROWSER_ARGS") {
            cmd.args(extra.split_whitespace());
        }
        cmd.arg(&opts.url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", opts.executable.display()))?;

        if !opts.remote_debugging {
            return Ok(Browser { child: Some(child), ws_url: None });
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(url) = read_port_file(&port_file) {
                return Ok(Browser { child: Some(child), ws_url: Some(url) });
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "{} did not open its DevTools endpoint. If a browser window from `aistudio login` is still open, close it and try again.",
                    opts.executable.display()
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stop the browser if this process started it.
    pub fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut()
            && let Ok(None) = child.try_wait()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn read_port_file(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim();
    let path = lines.next()?.trim();
    if port.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

fn endpoint_alive(ws_url: &str) -> bool {
    let host = ws_url.trim_start_matches("ws://").split('/').next().unwrap_or("");
    host.parse()
        .ok()
        .is_some_and(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok())
}
