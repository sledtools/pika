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

/// Per-group profile pic cache path: profile_pics/group_{chat_id}/{pubkey_hex}
pub fn group_cached_path(data_dir: &str, chat_id: &str, pubkey_hex: &str) -> PathBuf {
    std::path::Path::new(data_dir)
        .join("profile_pics")
        .join(format!("group_{chat_id}"))
        .join(pubkey_hex)
}

pub fn ensure_group_dir(data_dir: &str, chat_id: &str) {
    let dir = std::path::Path::new(data_dir)
        .join("profile_pics")
        .join(format!("group_{chat_id}"));
    let _ = std::fs::create_dir_all(&dir);
}

pub fn delete_group_cache(data_dir: &str, chat_id: &str) {
    let dir = std::path::Path::new(data_dir)
        .join("profile_pics")
        .join(format!("group_{chat_id}"));
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
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

pub async fn download_group_image(
    client: &reqwest::Client,
    data_dir: &str,
    chat_id: &str,
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

    let output = match resize_to_jpeg(&bytes) {
        Ok(resized) => resized,
        Err(_) => bytes.to_vec(),
    };

    let dest = group_cached_path(data_dir, chat_id, pubkey_hex);
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &output)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

pub fn save_group_image_bytes(
    data_dir: &str,
    chat_id: &str,
    pubkey_hex: &str,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    let output = match resize_to_jpeg(bytes) {
        Ok(resized) => resized,
        Err(_) => bytes.to_vec(),
    };
    let dest = group_cached_path(data_dir, chat_id, pubkey_hex);
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &output)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

#[cfg(test)]
fn tiny_png() -> Vec<u8> {
    // 1x1 red PNG.
    let mut img = image::RgbImage::new(1, 1);
    img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
    let mut buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut buf);
    image::ImageEncoder::write_image(encoder, &img, 1, 1, image::ColorType::Rgb8.into()).unwrap();
    buf
}

pub fn clear_cache(data_dir: &str) {
    let dir = std::path::Path::new(data_dir).join("profile_pics");
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_cached_path_is_separate() {
        let path = group_cached_path("/data", "chat_abc", "pk123");
        assert_eq!(
            path,
            PathBuf::from("/data/profile_pics/group_chat_abc/pk123")
        );
        assert_ne!(path, cached_path("/data", "pk123"));
    }

    #[test]
    fn save_group_image_bytes_creates_cached_file() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_str().unwrap();
        ensure_group_dir(data_dir, "chat1");

        let dest = save_group_image_bytes(data_dir, "chat1", "pk1", &tiny_png()).unwrap();
        assert!(dest.exists());
        assert_eq!(dest, group_cached_path(data_dir, "chat1", "pk1"));
        // Should be JPEG
        let bytes = std::fs::read(&dest).unwrap();
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn delete_group_cache_removes_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_str().unwrap();
        ensure_group_dir(data_dir, "chat1");
        save_group_image_bytes(data_dir, "chat1", "pk1", &tiny_png()).unwrap();

        delete_group_cache(data_dir, "chat1");
        assert!(!group_cached_path(data_dir, "chat1", "pk1").exists());
    }

    #[test]
    fn save_image_bytes_creates_cached_file() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_str().unwrap();
        ensure_dir(data_dir);

        let pk = "aabbccdd";
        let png = tiny_png();

        let dest = save_image_bytes(data_dir, pk, &png).unwrap();
        assert!(dest.exists());
        assert_eq!(dest, cached_path(data_dir, pk));
        // Output should be a valid JPEG (resize_to_jpeg succeeds on a valid PNG).
        let bytes = std::fs::read(&dest).unwrap();
        assert!(bytes.len() > 2);
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8]); // JPEG magic
    }

    #[test]
    fn save_image_bytes_overwrites_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_str().unwrap();
        ensure_dir(data_dir);

        let pk = "aabbccdd";
        let png = tiny_png();
        save_image_bytes(data_dir, pk, &png).unwrap();

        let mtime1 = cached_path(data_dir, pk)
            .metadata()
            .unwrap()
            .modified()
            .unwrap();

        // Overwrite with same bytes — file should still be replaced (new mtime).
        // Sleep briefly so mtime granularity can distinguish them.
        std::thread::sleep(std::time::Duration::from_millis(50));
        save_image_bytes(data_dir, pk, &png).unwrap();

        let mtime2 = cached_path(data_dir, pk)
            .metadata()
            .unwrap()
            .modified()
            .unwrap();
        assert!(mtime2 > mtime1);
    }

    #[test]
    fn no_tmp_file_left_after_save() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_str().unwrap();
        ensure_dir(data_dir);

        save_image_bytes(data_dir, "aabb", &tiny_png()).unwrap();

        let dir = std::path::Path::new(data_dir).join("profile_pics");
        let tmp_files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("tmp"))
            .collect();
        assert!(tmp_files.is_empty());
    }
}
