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
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &staging,
                layout: wgpu::ImageDataLayout {
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
        self.gpu.device.poll(wgpu::Maintain::Wait);

        let result = rx
            .recv()
            .map_err(|_| "channel closed".to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));

        let (ok, message) = match result {
            Ok(()) => {
                let data = slice.get_mapped_range();
                let is_bgra = matches!(
                    self.gpu.config.format,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                );
                match save_screenshot(&data, width, height, padded_row, unpadded_row, is_bgra) {
                    Ok(filename) => (true, format!("screenshot saved: {filename}")),
                    Err(e) => (false, format!("screenshot save failed: {e}")),
                }
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
            });
        }
    }
}

fn save_screenshot(
    data: &[u8],
    width: u32,
    height: u32,
    padded_row: u32,
    unpadded_row: u32,
    bgra: bool,
) -> Result<String> {
    use std::time::{SystemTime, UNIX_EPOCH};

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

    std::fs::create_dir_all("captures").context("failed to create captures dir")?;

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
        "world-gen-{:04}{:02}{:02}-{:02}{:02}{:02}.png",
        y, m, d, hours, minutes, seconds,
    );
    let path = std::path::Path::new("captures").join(&filename);
    let latest = std::path::Path::new("captures").join("latest.png");

    image::save_buffer(&path, &pixels, width, height, image::ColorType::Rgba8)
        .context("failed to encode PNG")?;
    let _ = std::fs::copy(&path, &latest);

    log::info!("screenshot saved: {}", path.display());
    Ok(filename)
}
