//! Open browser tabs via the `extension/` MV3 bridge over a localhost WebSocket.
//! Protocol (JSON text frames):
//!   extension -> app: {"type":"hello","browser":"Edge"}
//!                     {"type":"tabs","browser":"Edge","tabs":[{tab_id,window_id,title,url,favIconUrl}]}  (full snapshot)
//!                     {"type":"ping"}  (keeps the MV3 service worker alive)
//!   app -> extension: {"type":"focus_tab","tab_id":1,"window_id":2}
//! Security: bound to 127.0.0.1 only, and the handshake requires an extension Origin, because any web page
//! open in a browser can connect to ws://127.0.0.1 and would otherwise inject fake tabs.
use crate::{score, Hit};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Error, Message};

pub const ADDR: &str = "127.0.0.1:49152";

#[derive(Deserialize, Clone, Debug)]
pub struct Tab {
    pub tab_id: i64,
    pub window_id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "favIconUrl", default)]
    pub fav_icon_url: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Incoming {
    Hello { browser: String },
    Tabs { browser: String, tabs: Vec<Tab> },
    Ping,
}

struct Conn {
    browser: String,
    tabs: Vec<Tab>,
    tx: Sender<String>,
}

static CONNS: LazyLock<RwLock<HashMap<u64, Conn>>> = LazyLock::new(Default::default);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Bind and serve in the background. A busy port just disables the feature (logged), never crashes.
pub fn start() {
    match TcpListener::bind(ADDR) {
        Ok(l) => {
            std::thread::spawn(move || serve(l));
        }
        Err(e) => eprintln!("tabs: cannot listen on {ADDR} ({e}); open-tab search disabled"),
    }
}

pub fn serve(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || handle(stream));
    }
}

fn origin_allowed(origin: Option<&str>) -> bool {
    origin.is_some_and(|o| o.starts_with("chrome-extension://") || o.starts_with("moz-extension://"))
}

fn handle(stream: TcpStream) {
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
        if origin_allowed(origin) {
            Ok(resp)
        } else {
            let mut r = ErrorResponse::new(Some("extension origin required".into()));
            *r.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(r)
        }
    };
    let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else { return };
    // Short read timeout = poll loop that also flushes outgoing focus commands (<= ~20ms latency).
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(20)));
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<String>();
    CONNS.write().unwrap().insert(id, Conn { browser: "Browser".into(), tabs: vec![], tx });
    'conn: loop {
        while let Ok(out) = rx.try_recv() {
            if ws.send(Message::text(out)).is_err() {
                break 'conn;
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => apply(id, t.as_str()),
            Ok(Message::Close(_)) => break,
            Ok(_) => {} // pings are answered by tungstenite itself
            Err(Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
    }
    CONNS.write().unwrap().remove(&id);
}

fn apply(id: u64, text: &str) {
    let Ok(msg) = serde_json::from_str::<Incoming>(text) else { return }; // ignore malformed frames
    let mut conns = CONNS.write().unwrap();
    let Some(c) = conns.get_mut(&id) else { return };
    match msg {
        Incoming::Hello { browser } => c.browser = browser,
        Incoming::Tabs { browser, tabs } => {
            c.browser = browser;
            c.tabs = tabs;
        }
        Incoming::Ping => {}
    }
}

#[derive(serde::Serialize, Debug)]
pub struct BrowserStatus {
    pub browser: String,
    pub tabs: usize,
}

/// Open http(s) tabs across connected browsers: (title, url, browser). For the first-run wizard.
pub fn open_sites() -> Vec<(String, String, String)> {
    let conns = CONNS.read().unwrap();
    conns
        .values()
        .flat_map(|c| c.tabs.iter().map(move |t| (t.title.clone(), t.url.clone(), c.browser.clone())))
        .filter(|(_, u, _)| u.starts_with("https://") || u.starts_with("http://"))
        .collect()
}

/// Connected browsers for the settings panel.
pub fn status() -> Vec<BrowserStatus> {
    let mut v: Vec<_> = CONNS.read().unwrap().values().map(|c| BrowserStatus { browser: c.browser.clone(), tabs: c.tabs.len() }).collect();
    v.sort_by(|a, b| a.browser.cmp(&b.browser));
    v
}

/// Title match on the shared scale (+0.1: an app with the same name wins ties); URL substring = 3.
pub fn search(q: &str, max: usize) -> Vec<Hit> {
    if q.is_empty() {
        return vec![];
    }
    let conns = CONNS.read().unwrap();
    let mut v: Vec<Hit> = Vec::new();
    for (id, c) in conns.iter() {
        for t in &c.tabs {
            let s = score(&t.title.to_lowercase(), q).or_else(|| t.url.to_lowercase().contains(q).then_some(3));
            if let Some(s) = s {
                let icon = t.fav_icon_url.clone().filter(|u| u.starts_with("https://") || u.starts_with("data:image/"));
                v.push(Hit {
                    name: if t.title.is_empty() { t.url.clone() } else { t.title.clone() },
                    path: format!("tab:{id}:{}:{}", t.window_id, t.tab_id),
                    kind: "tab",
                    score: s as f32 + 0.1,
                    detail: Some(format!("{} Tab", c.browser)),
                    icon,
                    ..Default::default()
                });
            }
        }
    }
    v.sort_by(|a, b| a.score.total_cmp(&b.score).then(a.name.len().cmp(&b.name.len())));
    v.truncate(max);
    v
}

/// Switch to an open tab already showing `url` (any connected browser). False if there is none.
pub fn focus_url(url: &str) -> bool {
    let found = CONNS.read().unwrap().iter().find_map(|(id, c)| {
        c.tabs.iter().find(|t| same_page(&t.url, url)).map(|t| format!("tab:{id}:{}:{}", t.window_id, t.tab_id))
    });
    found.is_some_and(|p| focus(&p))
}

/// Same host and path, ignoring query, fragment, trailing slash and "www.". Sites that rewrite the path
/// (Drive: /drive/folders/ID -> /drive/u/0/folders/ID) still match when the tab ends with the same long id.
fn same_page(tab: &str, target: &str) -> bool {
    let norm = |u: &str| {
        let u = u.split(['?', '#']).next().unwrap_or(u).trim_end_matches('/').to_lowercase();
        u.split("://").nth(1).unwrap_or(&u).trim_start_matches("www.").to_string()
    };
    let (a, b) = (norm(tab), norm(target));
    let host = |s: &str| s.split('/').next().unwrap_or("").to_string();
    let id = b.rsplit('/').next().unwrap_or("");
    a == b || (host(&a) == host(&b) && id.len() >= 16 && a.ends_with(&format!("/{id}")))
}

/// Ask the owning browser to activate the tab and focus its window. False if that browser disconnected.
pub fn focus(path: &str) -> bool {
    let parts: Vec<&str> = path.strip_prefix("tab:").unwrap_or("").split(':').collect();
    let [conn, window, tab] = parts.as_slice() else { return false };
    let (Ok(conn), Ok(window_id), Ok(tab_id)) = (conn.parse::<u64>(), window.parse::<i64>(), tab.parse::<i64>()) else {
        return false;
    };
    // We are the foreground process right now (user pressed Enter): let the browser take the foreground.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
    let msg = serde_json::json!({ "type": "focus_tab", "tab_id": tab_id, "window_id": window_id }).to_string();
    CONNS.read().unwrap().get(&conn).is_some_and(|c| c.tx.send(msg).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tungstenite::client::IntoClientRequest;

    fn connect(port: u16, origin: &str) -> Result<tungstenite::WebSocket<TcpStream>, Error> {
        let mut req = format!("ws://127.0.0.1:{port}").into_client_request().unwrap();
        req.headers_mut().insert("Origin", origin.parse().unwrap());
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tungstenite::client(req, stream).map(|(ws, _)| ws).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(e) => e,
            _ => Error::ConnectionClosed,
        })
    }

    fn wait_for(cond: impl Fn() -> bool) -> bool {
        (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            cond()
        })
    }

    #[test]
    fn messages_parse() {
        let m: Incoming = serde_json::from_str(
            r#"{"type":"tabs","browser":"Edge","tabs":[{"tab_id":5,"window_id":1,"title":"X","url":"https://x.y","favIconUrl":null}]}"#,
        )
        .unwrap();
        assert!(matches!(m, Incoming::Tabs { ref tabs, .. } if tabs[0].tab_id == 5));
        assert!(serde_json::from_str::<Incoming>(r#"{"type":"evil"}"#).is_err());
        assert!(origin_allowed(Some("chrome-extension://abcdef")));
        assert!(!origin_allowed(Some("https://evil.example")) && !origin_allowed(None));
    }

    #[test]
    fn same_page_matching() {
        let drive = "https://drive.google.com/drive/folders/1RnK7weS7sLWjQqnu39ZVcx0JqpouPtfS?hl=pl";
        assert!(same_page("https://drive.google.com/drive/folders/1RnK7weS7sLWjQqnu39ZVcx0JqpouPtfS", drive));
        assert!(same_page("https://drive.google.com/drive/u/0/folders/1RnK7weS7sLWjQqnu39ZVcx0JqpouPtfS#x", drive));
        assert!(!same_page("https://drive.google.com/drive/folders/OTHERFOLDERID12345678", drive));
        assert!(!same_page("https://evil.example/1RnK7weS7sLWjQqnu39ZVcx0JqpouPtfS", drive));
        assert!(same_page("https://www.github.com/", "https://github.com"));
        // short last segments must match exactly (no "/" == "/a/b/" style accidents)
        assert!(!same_page("https://github.com/x/issues", "https://github.com/y/issues"));
    }

    #[test]
    fn live_websocket_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || serve(listener));

        // A web page must be rejected.
        assert!(connect(port, "https://evil.example").is_err());

        let mut ws = connect(port, "chrome-extension://stlrtest").unwrap();
        ws.send(Message::text(r#"{"type":"hello","browser":"Edge"}"#)).unwrap();
        ws.send(Message::text(
            r#"{"type":"tabs","browser":"Edge","tabs":[
                {"tab_id":42,"window_id":7,"title":"Zxqw Unique Tab - YouTube","url":"https://youtube.com/watch?v=1","favIconUrl":"https://youtube.com/favicon.ico"},
                {"tab_id":43,"window_id":7,"title":"Other","url":"https://zxqwunique.example/","favIconUrl":"javascript:x"}]}"#,
        ))
        .unwrap();
        assert!(wait_for(|| search("zxqw unique", 5).len() == 1), "tab indexed");
        let t = std::time::Instant::now();
        let hit = &search("zxqw unique", 5)[0];
        assert!(t.elapsed() < Duration::from_millis(1), "RAM lookup");
        assert_eq!(hit.detail.as_deref(), Some("Edge Tab"));
        assert_eq!(hit.icon.as_deref(), Some("https://youtube.com/favicon.ico"));
        assert!(search("zxqwunique.example", 5)[0].icon.is_none(), "non-http favicon dropped");

        // focus_tab reaches the right browser connection.
        assert!(focus(&hit.path));
        ws.get_ref().set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let got = ws.read().unwrap();
        let v: serde_json::Value = serde_json::from_str(got.to_text().unwrap()).unwrap();
        assert_eq!((v["type"].as_str(), v["tab_id"].as_i64(), v["window_id"].as_i64()), (Some("focus_tab"), Some(42), Some(7)));

        // Disconnect removes the browser's tabs.
        ws.close(None).unwrap();
        let _ = ws.flush();
        assert!(wait_for(|| search("zxqw unique", 5).is_empty()), "tabs dropped on disconnect");
        assert!(!focus(&hit.path));
        assert!(!focus("tab:garbage"));
    }
}
