//! The one window (Parity 58). It exists only while open: closing destroys the webview, so the
//! idle app is just the Rust process; the next open builds it again.
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "main";

/// Show the window (building it if needed), optionally on a screen (`notifications`, `feedback`).
pub fn open(app: &AppHandle, route: Option<&str>) {
    if let Some(w) = app.get_webview_window(LABEL) {
        if let Some(r) = route {
            let _ = app.emit_to(LABEL, "navigate", r);
        }
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let url = match route {
        Some(r) => format!("index.html#/{r}"),
        None => "index.html".into(),
    };
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(url.into()))
        .title("claude-sync")
        .inner_size(880.0, 640.0)
        .min_inner_size(640.0, 480.0)
        .center()
        .build();
    if let Err(e) = built {
        eprintln!("claude-sync: could not open the window: {e}");
    }
}
