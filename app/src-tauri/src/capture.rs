//! The feedback form's "Capture screen" (as v1): hide the window, grab the primary monitor,
//! show the window again. Returns base64 of a PNG (a JPEG when the PNG is over 3 MiB).
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use image::{DynamicImage, ImageFormat};
use std::io::Cursor;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const MAX_WIDTH: u32 = 1920;
const MAX_BYTES: usize = 3 * 1024 * 1024;

#[tauri::command]
pub async fn capture_screen(app: AppHandle) -> Result<String, String> {
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

fn capture_primary() -> Result<DynamicImage, String> {
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

/// At most `MAX_WIDTH` wide; PNG if it fits in `MAX_BYTES`, else JPEG at falling quality.
fn prepare(img: DynamicImage) -> Result<String, String> {
    let img = if img.width() > MAX_WIDTH {
        let h = (u64::from(img.height()) * u64::from(MAX_WIDTH) / u64::from(img.width())).max(1);
        img.resize_exact(
            MAX_WIDTH,
            u32::try_from(h).unwrap_or(MAX_WIDTH),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    if png.len() <= MAX_BYTES {
        return Ok(B64.encode(&png));
    }
    let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
    let mut out = Vec::new();
    for q in [85u8, 70, 55, 40] {
        out.clear();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q);
        rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
        if out.len() <= MAX_BYTES {
            return Ok(B64.encode(&out));
        }
    }
    Err("the image is too large even as a JPEG".into())
}
