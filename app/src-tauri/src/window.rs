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
        tracing::warn!("could not open the window: {e}");
    }
}

/// The window was closed (or the app started with none). Once the WebView2 processes are gone,
/// hand the pages the webview touched back to Windows so the idle tray app stays small.
/// Nothing is freed (private memory is unchanged); untouched pages are no longer counted.
pub fn closed(app: &AppHandle) {
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if a.get_webview_window(LABEL).is_none() {
            trim_working_set();
        }
    });
}

#[cfg(windows)]
pub fn trim_working_set() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    // SAFETY: the pseudo handle of this process; (usize::MAX, usize::MAX) means "trim".
    let ok = unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
    if ok == 0 {
        tracing::debug!("working set trim failed");
    }
}

#[cfg(not(windows))]
pub fn trim_working_set() {}
