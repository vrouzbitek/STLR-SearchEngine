# STLR Tabs Bridge

Minimal Manifest V3 extension that lets the STLR launcher search and switch to your open browser tabs.

- Sends a snapshot of open tabs (`tab_id`, `window_id`, `title`, `url`, `favIconUrl`) to `ws://127.0.0.1:49152` whenever tabs change.
- Handles `{"type":"focus_tab","tab_id":…,"window_id":…}` from the launcher.
- Localhost only. The launcher rejects connections that don't come from an extension origin, so web pages can't inject fake tabs.
- Reconnects automatically when the launcher starts or restarts.

## Install (Edge, Chrome, Brave, Opera, Vivaldi)

1. Open `edge://extensions` (or `chrome://extensions`, `brave://extensions`, …) and turn on **Developer mode**.
2. Click **Load unpacked** and pick this `extension/` folder.
