// STLR Tabs Bridge: sends a full tab snapshot to the local launcher on every change and focuses tabs on request.
// Works in any Chromium browser (Edge, Chrome, Brave, Opera, Vivaldi). Talks to 127.0.0.1 only.
const SERVER = "ws://127.0.0.1:49152";
const RECONNECT_MS = 2000;
const PING_MS = 20000; // < 30s: an active WebSocket keeps the MV3 service worker alive (Chrome 116+)
const SNAPSHOT_DEBOUNCE_MS = 150;

function detectBrowser() {
  const brands = (navigator.userAgentData?.brands || []).map((b) => b.brand).join(" ");
  const ua = navigator.userAgent;
  if (navigator.brave || brands.includes("Brave")) return "Brave";
  if (ua.includes("Edg/") || brands.includes("Edge")) return "Edge";
  if (ua.includes("OPR/") || brands.includes("Opera")) return "Opera";
  if (ua.includes("Vivaldi") || brands.includes("Vivaldi")) return "Vivaldi";
  return "Chrome";
}

const BROWSER = detectBrowser();
let ws = null;
let snapshotTimer = null;

function send(msg) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
}

function pushTabs() {
  clearTimeout(snapshotTimer);
  snapshotTimer = setTimeout(async () => {
    const tabs = await chrome.tabs.query({});
    send({
      type: "tabs",
      browser: BROWSER,
      tabs: tabs
        .filter((t) => t.id !== undefined && t.id >= 0)
        .map((t) => ({
          tab_id: t.id,
          window_id: t.windowId,
          title: t.title || "",
          url: t.url || "",
          favIconUrl: t.favIconUrl || null,
        })),
    });
  }, SNAPSHOT_DEBOUNCE_MS);
}

async function focusTab(tabId, windowId) {
  try {
    await chrome.tabs.update(tabId, { active: true });
    await chrome.windows.update(windowId, { focused: true });
  } catch (e) {
    pushTabs(); // tab/window is gone: resync the launcher
  }
}

function connect() {
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return;
  try {
    ws = new WebSocket(SERVER);
  } catch {
    return;
  }
  ws.onopen = () => {
    send({ type: "hello", browser: BROWSER });
    pushTabs();
  };
  ws.onmessage = (e) => {
    let msg;
    try {
      msg = JSON.parse(e.data);
    } catch {
      return;
    }
    if (msg.type === "focus_tab") focusTab(msg.tab_id, msg.window_id);
  };
  ws.onclose = () => {
    ws = null;
    setTimeout(connect, RECONNECT_MS); // launcher not running yet / restarted
  };
  ws.onerror = () => {}; // onclose follows and schedules the reconnect
}

chrome.tabs.onCreated.addListener(pushTabs);
chrome.tabs.onRemoved.addListener(pushTabs);
chrome.tabs.onReplaced.addListener(pushTabs);
chrome.tabs.onUpdated.addListener((_id, change) => {
  if (change.title || change.url || change.favIconUrl || change.status === "complete") pushTabs();
});

setInterval(() => send({ type: "ping" }), PING_MS);
// Wake the service worker periodically so it reconnects even after the browser suspended it.
chrome.alarms.create("stlr-reconnect", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(connect);
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
connect();
