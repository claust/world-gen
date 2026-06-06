use anyhow::{Context, Result};

use crate::debug_api::CommandAppliedEvent;

use super::AppState;

impl AppState {
    pub(super) fn handle_screenshot(
        &mut self,
        command_id: String,
        texture: &wgpu::Texture,
        mut encoder: wgpu::CommandEncoder,
    ) {
        let width = self.gpu.config.width;
        let height = self.gpu.config.height;
        let bytes_per_pixel = 4u32;
        let unpadded_row = width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = unpadded_row.div_ceil(align) * align;
        let buffer_size = (padded_row * height) as u64;

        let staging = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot-staging"),
            size: buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.gpu.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });

        let result = rx
            .recv()
            .map_err(|_| "channel closed".to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));

        let to_clipboard = std::mem::take(&mut self.screenshot_to_clipboard);

        let (ok, message) = match result {
            Ok(()) => {
                let data = slice.get_mapped_range();
                let is_bgra = matches!(
                    self.gpu.config.format,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                );
                let pixels = extract_rgba(&data, width, height, padded_row, unpadded_row, is_bgra);
                // Build the confirmation-toast thumbnail before `pixels` is
                // consumed by the clipboard copy below.
                let thumb = if to_clipboard {
                    Some(thumbnail_image(&pixels, width, height))
                } else {
                    None
                };
                let outcome = match save_screenshot(&self.captures_dir, &pixels, width, height) {
                    Ok(filename) => {
                        if to_clipboard {
                            match copy_to_clipboard(pixels, width, height) {
                                Ok(()) => {
                                    (true, format!("screenshot copied to clipboard: {filename}"))
                                }
                                Err(e) => (
                                    false,
                                    format!("screenshot saved but clipboard copy failed: {e}"),
                                ),
                            }
                        } else {
                            (true, format!("screenshot saved: {filename}"))
                        }
                    }
                    Err(e) => (false, format!("screenshot save failed: {e}")),
                };
                // Pop the on-screen confirmation only once the capture genuinely
                // succeeded; the texture lives in egui until the toast expires.
                if outcome.0 {
                    if let Some(img) = thumb {
                        let texture = self.egui_bridge.ctx().load_texture(
                            "screenshot-toast",
                            img,
                            egui::TextureOptions::LINEAR,
                        );
                        self.screenshot_toast = Some(super::ScreenshotToast {
                            texture,
                            created_at: self.elapsed_seconds,
                            aspect: width as f32 / height as f32,
                        });
                    }
                }
                outcome
            }
            Err(e) => (false, format!("screenshot readback failed: {e}")),
        };

        if let Some(api) = &self.debug_api {
            api.publish_command_applied(CommandAppliedEvent {
                id: command_id,
                frame: self.frame_index,
                ok,
                message,
                day_speed: None,
                object_id: None,
                object_position: None,
                data: None,
            });
        }
    }
}

/// Unpad the GPU staging buffer into a tightly-packed RGBA8 image, swizzling
/// from BGRA when that's the surface format.
fn extract_rgba(
    data: &[u8],
    _width: u32,
    height: u32,
    padded_row: u32,
    unpadded_row: u32,
    bgra: bool,
) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((unpadded_row * height) as usize);
    for row in 0..height {
        let offset = (row * padded_row) as usize;
        let row_bytes = &data[offset..offset + unpadded_row as usize];
        if bgra {
            for chunk in row_bytes.chunks_exact(4) {
                pixels.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
            }
        } else {
            pixels.extend_from_slice(row_bytes);
        }
    }
    pixels
}

/// Downscale a full-resolution RGBA8 frame into a small egui image for the
/// on-screen "screenshot taken" confirmation toast.
fn thumbnail_image(pixels: &[u8], width: u32, height: u32) -> egui::ColorImage {
    const MAX_W: u32 = 320;
    let tw = MAX_W.min(width).max(1);
    let th = (((tw as f32) * (height as f32) / (width as f32)).round() as u32).max(1);
    let src = image::RgbaImage::from_raw(width, height, pixels.to_vec())
        .expect("rgba buffer length matches dimensions");
    let resized = image::imageops::resize(&src, tw, th, image::imageops::FilterType::Triangle);
    egui::ColorImage::from_rgba_unmultiplied(
        [resized.width() as usize, resized.height() as usize],
        resized.as_raw(),
    )
}

/// Copy a tightly-packed RGBA8 image to the system clipboard so it can be
/// pasted into other applications.
fn copy_to_clipboard(pixels: Vec<u8>, width: u32, height: u32) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("failed to open clipboard")?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: std::borrow::Cow::Owned(pixels),
        })
        .context("failed to write image to clipboard")?;
    Ok(())
}

fn save_screenshot(
    dir: &std::path::Path,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    std::fs::create_dir_all(dir).context("failed to create captures dir")?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let filename = format!(
        "floraforge-{:04}{:02}{:02}-{:02}{:02}{:02}.png",
        y, m, d, hours, minutes, seconds,
    );
    let path = dir.join(&filename);
    let latest = dir.join("latest.png");

    image::save_buffer(&path, pixels, width, height, image::ColorType::Rgba8)
        .context("failed to encode PNG")?;
    let _ = std::fs::copy(&path, &latest);

    log::info!("screenshot saved: {}", path.display());
    Ok(filename)
}
