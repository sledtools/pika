use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use nostr_sdk::prelude::{Client, Event, Filter, Kind, PublicKey, RelayUrl};

pub(crate) fn mime_from_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        _ => return None,
    })
}

pub(crate) async fn fetch_latest_key_package(
    client: &Client,
    author: &PublicKey,
    relay_urls: &[RelayUrl],
    timeout: Duration,
) -> Result<Event> {
    let filter = Filter::new()
        .kind(Kind::MlsKeyPackage)
        .author(*author)
        .limit(1);
    let events = client
        .fetch_events_from(relay_urls.to_vec(), filter, timeout)
        .await
        .context("fetch keypackage events")?;
    events
        .iter()
        .next()
        .cloned()
        .ok_or_else(|| anyhow!("no keypackage found for {}", author.to_hex()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_common_types() {
        assert_eq!(
            mime_from_extension(Path::new("photo.jpg")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("photo.JPEG")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("video.mp4")),
            Some("video/mp4")
        );
        assert_eq!(
            mime_from_extension(Path::new("doc.pdf")),
            Some("application/pdf")
        );
    }

    #[test]
    fn mime_unknown_type_returns_none() {
        assert_eq!(mime_from_extension(Path::new("file.xyz")), None);
        assert_eq!(mime_from_extension(Path::new("README")), None);
    }
}
