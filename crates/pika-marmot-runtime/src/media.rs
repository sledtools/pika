use std::path::Path;

use nostr_sdk::prelude::{Tag, TagKind};

pub fn is_imeta_tag(tag: &Tag) -> bool {
    matches!(tag.kind(), TagKind::Custom(kind) if kind.as_ref() == "imeta")
}

pub fn mime_from_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "heic" => Some("image/heic"),
        "svg" => Some("image/svg+xml"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "webm" => Some("video/webm"),
        "mp3" => Some("audio/mpeg"),
        "ogg" => Some("audio/ogg"),
        "wav" => Some("audio/wav"),
        "pdf" => Some("application/pdf"),
        "txt" | "md" => Some("text/plain"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nostr_sdk::prelude::Tag;

    #[test]
    fn is_imeta_tag_matches() {
        let tag = Tag::parse(["imeta", "url https://example.com/img.jpg"]).unwrap();
        assert!(is_imeta_tag(&tag));
    }

    #[test]
    fn is_imeta_tag_rejects_other_tags() {
        let tag = Tag::parse(["p", "deadbeef"]).unwrap();
        assert!(!is_imeta_tag(&tag));
    }

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
    fn mime_unknown_extension() {
        assert_eq!(mime_from_extension(Path::new("file.xyz")), None);
    }

    #[test]
    fn mime_no_extension() {
        assert_eq!(mime_from_extension(Path::new("README")), None);
    }
}
