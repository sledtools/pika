use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Semaphore;

const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_SECS: u64 = 15;
const MAX_CONCURRENT_DOWNLOADS: usize = 4;
const MAX_DIMENSION: u32 = 400;
const JPEG_QUALITY: u8 = 85;

pub fn new_download_semaphore() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS))
}

pub fn ensure_dir(data_dir: &str) {
    let dir = std::path::Path::new(data_dir).join("profile_pics");
    let _ = std::fs::create_dir_all(&dir);
    // Clean up partial downloads from previous crashes.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("tmp") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// One file per user, keyed by hex pubkey.
pub fn cached_path(data_dir: &str, pubkey_hex: &str) -> PathBuf {
    std::path::Path::new(data_dir)
        .join("profile_pics")
        .join(pubkey_hex)
}

pub fn path_to_file_url(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

pub async fn download_image(
    client: &reqwest::Client,
    data_dir: &str,
    pubkey_hex: &str,
    url: &str,
    semaphore: &Arc<Semaphore>,
) -> anyhow::Result<PathBuf> {
    let _permit = semaphore.acquire().await?;

    let resp = client
        .get(url)
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .send()
        .await?
        .error_for_status()?;

    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_IMAGE_BYTES {
        anyhow::bail!("image too large ({} bytes)", bytes.len());
    }

    // Decode, resize, and re-encode as JPEG. Fall back to raw bytes on failure.
    let output = match resize_to_jpeg(&bytes) {
        Ok(resized) => resized,
        Err(_) => bytes.to_vec(),
    };

    let dest = cached_path(data_dir, pubkey_hex);
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &output)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// Resize an image so its longest side is at most MAX_DIMENSION, then encode as JPEG.
fn resize_to_jpeg(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img = image::load_from_memory(bytes)?;

    let img = if img.width() > MAX_DIMENSION || img.height() > MAX_DIMENSION {
        img.resize(
            MAX_DIMENSION,
            MAX_DIMENSION,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };

    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    img.write_with_encoder(encoder)?;
    Ok(buf)
}

/// Resize and save raw image bytes directly to the profile pic cache.
/// Used by the upload path to avoid re-downloading an image we already have.
pub fn save_image_bytes(data_dir: &str, pubkey_hex: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let output = match resize_to_jpeg(bytes) {
        Ok(resized) => resized,
        Err(_) => bytes.to_vec(),
    };
    let dest = cached_path(data_dir, pubkey_hex);
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &output)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

pub fn clear_cache(data_dir: &str) {
    let dir = std::path::Path::new(data_dir).join("profile_pics");
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}
