use std::collections::HashMap;

use iced::widget::{container, image, text};
use iced::{Alignment, Element, Length, Theme};

use crate::theme;

/// Cache of pre-processed circular avatar images keyed by file path.
pub struct AvatarCache {
    handles: HashMap<String, image::Handle>,
    /// How many new images to decode per view() call (avoids stutter).
    budget: usize,
    spent: usize,
}

impl AvatarCache {
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
            budget: 10,
            spent: 0,
        }
    }

    /// Reset the per-frame decode budget. Call once at the start of view().
    pub fn reset_budget(&mut self) {
        self.spent = 0;
    }

    /// Clear the entire cache (e.g. on logout).
    pub fn clear(&mut self) {
        self.handles.clear();
    }

    fn get_or_load(&mut self, path: &str, size: u32) -> Option<image::Handle> {
        let key = format!("{path}@{size}");
        if let Some(handle) = self.handles.get(&key) {
            return Some(handle.clone());
        }

        // Limit decodes per frame to avoid stutter.
        if self.spent >= self.budget {
            return None;
        }
        self.spent += 1;

        let handle = load_circular_image(path, size)?;
        self.handles.insert(key, handle.clone());
        Some(handle)
    }
}

/// Renders a circular avatar. Uses the cache to avoid repeated decoding.
pub fn avatar_circle<'a, M: 'a>(
    name: Option<&str>,
    picture_url: Option<&str>,
    size: f32,
    cache: &mut AvatarCache,
) -> Element<'a, M, Theme> {
    // Render at 2x for HiDPI displays (Retina). The image widget will
    // scale it down to `size` logical pixels, keeping it crisp at 2x.
    let render_px = (size * 2.0) as u32;
    if let Some(path) = picture_url.and_then(local_path_from_url) {
        if let Some(handle) = cache.get_or_load(&path, render_px) {
            return image(handle)
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .into();
        }
    }

    // Fallback: initial letter circle.
    let initial = name
        .and_then(|n| n.trim().chars().next())
        .unwrap_or('?')
        .to_uppercase()
        .to_string();

    container(
        text(initial)
            .size(size * 0.45)
            .color(theme::TEXT_PRIMARY)
            .center(),
    )
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(theme::avatar_container_style)
    .into()
}

fn load_circular_image(path: &str, size: u32) -> Option<image::Handle> {
    let bytes = std::fs::read(path).ok()?;
    let img = ::image::load_from_memory(&bytes).ok()?;
    let img = img.resize_to_fill(size, size, ::image::imageops::FilterType::Lanczos3);
    let mut rgba = img.into_rgba8();

    let center = size as f32 / 2.0;
    let radius = center;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            if dx * dx + dy * dy > radius * radius {
                rgba.get_pixel_mut(x, y).0[3] = 0;
            }
        }
    }

    Some(image::Handle::from_rgba(size, size, rgba.into_raw()))
}

fn local_path_from_url(url: &str) -> Option<String> {
    url.strip_prefix("file://").map(|s| s.to_string())
}
