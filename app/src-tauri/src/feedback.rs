//! Feedback form backend (§12 `/api/feedback`, issue #22): logs, screenshot, submit, outbox.
use crate::config;
use crate::ctx::{now_unix, Ctx};
use crate::guard::{self, Error};
use crate::notify;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use image::{DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub const MAX_WIDTH: u32 = 1920;
pub const MAX_BYTES: usize = 3 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screenshot {
    /// Base64 of the encoded image (no `data:` prefix).
    pub data: String,
    /// `image/png` or `image/jpeg`.
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackInput {
    pub kind: String,
    pub title: String,
    pub description: String,
    pub include_logs: bool,
    pub screenshot: Option<Screenshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackResult {
    /// Saved to the outbox because the relay could not be reached; sent later.
    pub queued: bool,
    pub number: Option<u64>,
    pub url: Option<String>,
}

/// Body of `POST /api/feedback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    title: String,
    description: String,
    kind: String,
    app_version: String,
    os: String,
    logs: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    screenshot: Option<String>,
}

pub fn logs(ctx: &Ctx) -> Result<String, Error> {
    // Capped well under the relay's 4.5 MB body limit (§12).
    claude_sync_core::logging::tail(&ctx.paths, config::FEEDBACK_LOG_BYTES)
        .map_err(guard::logged("read the log"))
}

/// Hide the window, capture the primary monitor, show the window again.
pub async fn capture_screen(app: &AppHandle) -> Result<Screenshot, Error> {
    let window = app.get_webview_window(crate::window::LABEL);
    if let Some(w) = &window {
        let _ = w.hide();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let shot = tauri::async_runtime::spawn_blocking(capture_primary)
        .await
        .map_err(|e| e.to_string());
    if let Some(w) = &window {
        let _ = w.show();
        let _ = w.set_focus();
    }
    let img = shot??;
    tauri::async_runtime::spawn_blocking(move || prepare(img))
        .await
        .map_err(|e| e.to_string())?
}

fn capture_primary() -> Result<DynamicImage, Error> {
    let monitors = xcap::Monitor::all().map_err(|e| format!("no monitors: {e}"))?;
    let primary = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .ok_or("no monitor found")?;
    let rgba = primary
        .capture_image()
        .map_err(|e| format!("screen capture failed: {e}"))?;
    Ok(DynamicImage::ImageRgba8(rgba))
}

/// An image file the user attached instead of a capture.
pub fn attach_file(data_b64: &str) -> Result<Screenshot, Error> {
    let raw = B64
        .decode(data_b64.trim())
        .map_err(|e| format!("bad file data: {e}"))?;
    let img = image::load_from_memory(&raw)
        .map_err(|_| "only PNG and JPEG images can be attached".to_string())?;
    prepare(img)
}

/// The image on the clipboard (a screenshot tool's copy, a copied image).
pub fn paste_clipboard() -> Result<Screenshot, Error> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    let data = clipboard.get_image().map_err(|e| match e {
        arboard::Error::ContentNotAvailable => "there is no image on the clipboard".to_string(),
        e => format!("clipboard: {e}"),
    })?;
    from_rgba(data.width, data.height, data.bytes.into_owned())
}

fn from_rgba(width: usize, height: usize, bytes: Vec<u8>) -> Result<Screenshot, Error> {
    let (w, h) = (u32::try_from(width), u32::try_from(height));
    let rgba = match (w, h) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => image::RgbaImage::from_raw(w, h, bytes),
        _ => None,
    }
    .ok_or("the clipboard image is malformed")?;
    prepare(DynamicImage::ImageRgba8(rgba))
}

/// At most `MAX_WIDTH` wide; PNG if it fits in `MAX_BYTES`, else JPEG at falling quality.
pub fn prepare(img: DynamicImage) -> Result<Screenshot, Error> {
    let img = if img.width() > MAX_WIDTH {
        let h = (img.height() as u64 * MAX_WIDTH as u64 / img.width() as u64).max(1) as u32;
        img.resize_exact(MAX_WIDTH, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (width, height) = (img.width(), img.height());
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let (bytes, mime) = if png.len() <= MAX_BYTES {
        (png, "image/png")
    } else {
        let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
        let mut out = Vec::new();
        for q in [85u8, 70, 55, 40] {
            out.clear();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q);
            rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
            if out.len() <= MAX_BYTES {
                break;
            }
        }
        if out.len() > MAX_BYTES {
            return Err("the image is too large even as a JPEG".into());
        }
        (out, "image/jpeg")
    };
    Ok(Screenshot {
        bytes: bytes.len(),
        data: B64.encode(&bytes),
        mime: mime.into(),
        width,
        height,
    })
}

pub async fn submit(app: &AppHandle, input: FeedbackInput) -> Result<FeedbackResult, Error> {
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    let title = input.title.trim().to_string();
    if title.is_empty() {
        return Err("a title is needed".into());
    }
    if !matches!(input.kind.as_str(), "bug" | "idea") {
        return Err("kind must be bug or idea".into());
    }
    let logs = if input.include_logs {
        logs(&ctx).unwrap_or_default()
    } else {
        String::new()
    };
    let payload = Payload {
        title,
        description: input.description,
        kind: input.kind,
        app_version: config::VERSION.into(),
        os: config::os_label(),
        logs,
        screenshot: input.screenshot.map(|s| s.data),
    };
    match send(&ctx, &payload).await {
        Ok(r) => Ok(r),
        Err(SendError::Offline(e)) => {
            tracing::info!("feedback queued: {e}");
            save_to_outbox(&ctx.outbox_dir(), &payload)
                .map_err(|e| format!("could not save the report: {e}"))?;
            Ok(FeedbackResult {
                queued: true,
                number: None,
                url: None,
            })
        }
        Err(SendError::Rejected(e)) => Err(e),
    }
}

enum SendError {
    /// Network trouble or a relay error worth retrying.
    Offline(String),
    /// The relay said no; retrying will not help.
    Rejected(String),
}

async fn send(ctx: &Ctx, payload: &Payload) -> Result<FeedbackResult, SendError> {
    if !config::relay_configured() {
        // Kept in the outbox: a later build with a relay sends it.
        return Err(SendError::Offline(
            "this build has no relay URL (CLAUDE_SYNC_RELAY_URL)".into(),
        ));
    }
    let auth = ctx
        .auth()
        .ok_or_else(|| SendError::Rejected("sign in to send feedback".into()))?;
    let http = claude_sync_core::github::client_builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| SendError::Offline(e.to_string()))?;
    let resp = http
        .post(config::relay("/api/feedback"))
        .bearer_auth(&auth.token)
        .header("User-Agent", format!("claude-sync/{}", config::VERSION))
        .json(payload)
        .send()
        .await
        .map_err(|e| SendError::Offline(e.without_url().to_string()))?;
    let status = resp.status();
    if status.as_u16() == 201 || status.is_success() {
        #[derive(Deserialize)]
        struct Created {
            number: u64,
            url: String,
        }
        let c: Created = resp
            .json()
            .await
            .map_err(|e| SendError::Rejected(format!("unexpected relay reply: {e}")))?;
        return Ok(FeedbackResult {
            queued: false,
            number: Some(c.number),
            url: Some(c.url),
        });
    }
    let body = resp.text().await.unwrap_or_default();
    let msg = relay_error(status, &body);
    if status.is_server_error() || status.as_u16() == 429 {
        Err(SendError::Offline(msg))
    } else {
        Err(SendError::Rejected(msg))
    }
}

/// The relay's `{"error": "..."}` message as is, so the user reads why (for example that
/// GitHub refused their account); else the status and the start of the body.
fn relay_error(status: reqwest::StatusCode, body: &str) -> String {
    #[derive(Deserialize)]
    struct Reply {
        error: String,
    }
    match serde_json::from_str::<Reply>(body) {
        Ok(r) if !r.error.trim().is_empty() => r.error.trim().chars().take(600).collect(),
        _ => format!(
            "relay returned {status}: {}",
            body.chars().take(300).collect::<String>()
        ),
    }
}

fn save_to_outbox(dir: &Path, payload: &Payload) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let name = format!("{}-{}.json", now_unix(), std::process::id());
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(payload)?)?;
    std::fs::rename(tmp, dir.join(name))
}

/// Send reports saved while offline. Stops at the first network failure.
pub async fn retry_outbox(app: &AppHandle) {
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    if ctx.auth().is_none() || !config::relay_configured() {
        return;
    }
    let dir = ctx.outbox_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for path in files {
        let Some(payload) = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Payload>(&b).ok())
        else {
            let _ = std::fs::rename(&path, path.with_extension("bad"));
            continue;
        };
        match send(&ctx, &payload).await {
            Ok(r) => {
                let _ = std::fs::remove_file(&path);
                let n = r.number.map(|n| format!(" as #{n}")).unwrap_or_default();
                notify::show(
                    app,
                    "Feedback sent",
                    &format!("\"{}\" was filed{n}.", payload.title),
                );
            }
            Err(SendError::Offline(e)) => {
                tracing::info!("outbox: still offline ({e})");
                return;
            }
            Err(SendError::Rejected(e)) => {
                tracing::warn!("outbox: {} rejected: {e}", path.display());
                let _ = std::fs::rename(&path, path.with_extension("rejected"));
                notify::show(
                    app,
                    "Feedback not sent",
                    &format!("\"{}\" was not filed: {e}", payload.title),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn small_image_stays_png() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(100, 50, Rgb([10, 20, 30])));
        let s = prepare(img).unwrap();
        assert_eq!((s.mime.as_str(), s.width, s.height), ("image/png", 100, 50));
        assert_eq!(B64.decode(&s.data).unwrap().len(), s.bytes);
    }

    #[test]
    fn clipboard_rgba_becomes_a_png() {
        let s = from_rgba(4, 3, vec![200; 4 * 3 * 4]).unwrap();
        assert_eq!((s.mime.as_str(), s.width, s.height), ("image/png", 4, 3));
        assert!(from_rgba(4, 3, vec![0; 10]).is_err());
        assert!(from_rgba(0, 0, Vec::new()).is_err());
    }

    #[test]
    fn wide_noisy_image_is_scaled_and_fits() {
        // Noise does not compress, so the PNG is far over 3 MB and has to become a JPEG.
        let mut seed = 1u32;
        let img = RgbImage::from_fn(3840, 2160, |_, _| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let b = seed.to_le_bytes();
            Rgb([b[1], b[2], b[3]])
        });
        let s = prepare(DynamicImage::ImageRgb8(img)).unwrap();
        assert_eq!((s.width, s.height), (1920, 1080));
        assert!(s.bytes <= MAX_BYTES, "{} bytes", s.bytes);
        assert_eq!(s.mime, "image/jpeg");
    }

    #[test]
    fn relay_error_shows_the_relay_message() {
        let forbidden = reqwest::StatusCode::FORBIDDEN;
        let body = r#"{"error":"GitHub did not let your account open an issue on o/r: x."}"#;
        assert_eq!(
            relay_error(forbidden, body),
            "GitHub did not let your account open an issue on o/r: x."
        );
        assert_eq!(
            relay_error(reqwest::StatusCode::BAD_GATEWAY, "<html>bad gateway</html>"),
            "relay returned 502 Bad Gateway: <html>bad gateway</html>"
        );
        assert_eq!(
            relay_error(forbidden, r#"{"error":"  "}"#),
            r#"relay returned 403 Forbidden: {"error":"  "}"#
        );
    }

    #[test]
    fn outbox_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("claude-sync-outbox-test-{}", std::process::id()));
        let p = Payload {
            title: "t".into(),
            description: "d".into(),
            kind: "bug".into(),
            app_version: "0.1.0".into(),
            os: "windows x86_64".into(),
            logs: String::new(),
            screenshot: None,
        };
        save_to_outbox(&dir, &p).unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        let back: Payload =
            serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
        assert_eq!(back.title, "t");
        let json = serde_json::to_value(&back).unwrap();
        assert!(json.get("appVersion").is_some() && json.get("screenshot").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
