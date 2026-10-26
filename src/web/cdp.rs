//! A small synchronous Chrome DevTools Protocol client over a WebSocket.

use std::collections::VecDeque;
use std::fmt;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

#[derive(Debug, Clone)]
pub struct CdpError(pub String);

impl fmt::Display for CdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CdpError {}

pub type Result<T> = std::result::Result<T, CdpError>;

fn err(msg: impl Into<String>) -> CdpError {
    CdpError(msg.into())
}

pub struct Cdp {
    ws: WebSocket<TcpStream>,
    next_id: u64,
    events: VecDeque<Value>,
}

impl Cdp {
    /// Connect to a browser-level DevTools endpoint (ws://127.0.0.1:PORT/devtools/browser/ID).
    pub fn connect(ws_url: &str) -> Result<Cdp> {
        let rest = ws_url.strip_prefix("ws://").ok_or_else(|| err(format!("unsupported DevTools URL: {ws_url}")))?;
        let host = rest.split('/').next().unwrap_or(rest);
        let stream =
            TcpStream::connect(host).map_err(|e| err(format!("cannot connect to the browser at {host}: {e}")))?;
        stream.set_nodelay(true).ok();
        let (ws, _) =
            tungstenite::client(ws_url, stream).map_err(|e| err(format!("DevTools handshake failed: {e}")))?;
        Ok(Cdp { ws, next_id: 1, events: VecDeque::new() })
    }

    fn read(&mut self, deadline: Option<Instant>) -> Result<Option<Value>> {
        let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)));
        self.ws.get_mut().set_read_timeout(timeout).ok();
        match self.ws.read() {
            Ok(Message::Text(t)) => {
                serde_json::from_str(t.as_str()).map(Some).map_err(|e| err(format!("bad DevTools message: {e}")))
            }
            Ok(Message::Close(_)) => Err(err("the browser closed the connection")),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Ok(None)
            }
            Err(e) => Err(err(format!("lost the connection to the browser: {e}"))),
        }
    }

    /// Send a command and wait for its result. `session` targets a page.
    pub fn call(&mut self, session: Option<&str>, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.ws
            .send(Message::text(msg.to_string()))
            .map_err(|e| err(format!("lost the connection to the browser: {e}")))?;
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(err(format!("{method} timed out")));
            }
            let Some(v) = self.read(Some(deadline))? else { continue };
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(e) = v.get("error") {
                    let m = e.get("message").and_then(Value::as_str).unwrap_or("error");
                    return Err(err(format!("{method}: {m}")));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            if v.get("method").is_some() {
                self.push_event(v);
            }
        }
    }

    fn push_event(&mut self, v: Value) {
        if self.events.len() > 500 {
            self.events.pop_front();
        }
        self.events.push_back(v);
    }

    /// Wait for an event by name (already received ones count).
    pub fn wait_event(&mut self, method: &str, timeout: Duration) -> Result<Value> {
        if let Some(pos) = self.events.iter().position(|e| e.get("method").and_then(Value::as_str) == Some(method)) {
            return Ok(self.events.remove(pos).unwrap_or_default());
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(v) = self.read(Some(deadline))? {
                if v.get("method").and_then(Value::as_str) == Some(method) {
                    return Ok(v);
                }
                if v.get("method").is_some() {
                    self.push_event(v);
                }
            }
        }
        Err(err(format!("timed out waiting for {method}")))
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
    }
}
