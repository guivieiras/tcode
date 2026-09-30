//! Resolve links on the machine that owns the workspace. Previews never execute files.
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
use tcode_protocol::{
    FilePreview, FilePreviewContent, MAX_FILE_PREVIEW_BYTES, MAX_FILE_RANGE_BYTES,
};

pub fn open(target: &str, base_dir: Option<&Path>) -> io::Result<FilePreview> {
    let (target, line) = split_location(target);
    let decoded;
    let target = if target.starts_with("file://") {
        let url = url::Url::parse(target).map_err(io::Error::other)?;
        decoded = url
            .to_file_path()
            .map_err(|_| io::Error::other("Invalid file URL"))?;
        decoded.as_path()
    } else {
        decoded = PathBuf::from(
            percent_encoding::percent_decode_str(target)
                .decode_utf8()
                .map_err(io::Error::other)?
                .as_ref(),
        );
        decoded.as_path()
    };
    let path = if let Ok(rest) = target.strip_prefix("~") {
        dirs::home_dir()
            .ok_or_else(|| io::Error::other("Home directory is unavailable"))?
            .join(rest)
    } else if target.is_absolute() {
        target.to_path_buf()
    } else {
        base_dir
            .ok_or_else(|| io::Error::other("The link has no workspace directory"))?
            .join(target)
    };
    let path = path.canonicalize()?;
    let mut file = File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("This path is not a regular file"));
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let content = if let Some(mime) = media_mime(&ext) {
        FilePreviewContent::Video {
            size: metadata.len(),
            mime: mime.into(),
        }
    } else {
        if metadata.len() > MAX_FILE_PREVIEW_BYTES as u64 {
            return Err(io::Error::other(
                "This file exceeds the 8 MiB preview limit",
            ));
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_FILE_PREVIEW_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_FILE_PREVIEW_BYTES {
            return Err(io::Error::other(
                "This file exceeds the 8 MiB preview limit",
            ));
        }
        if image::guess_format(&bytes).is_ok() {
            FilePreviewContent::Image { bytes }
        } else {
            if bytes.len() > 1024 * 1024 {
                return Err(io::Error::other(
                    "This text file exceeds the 1 MiB preview limit",
                ));
            }
            let text = String::from_utf8(bytes)
                .map_err(|_| io::Error::other("This file format cannot be previewed"))?;
            if text.contains('\0') {
                return Err(io::Error::other("This file format cannot be previewed"));
            }
            FilePreviewContent::Text { text, line }
        }
    };
    Ok(FilePreview { path, content })
}

pub fn read_range(
    path: &Path,
    offset: u64,
    length: u32,
    expected_size: u64,
) -> io::Result<Vec<u8>> {
    if length == 0 || length as usize > MAX_FILE_RANGE_BYTES {
        return Err(io::Error::other("Invalid file range length"));
    }
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err(io::Error::other("The file changed; reopen its preview"));
    }
    if offset > expected_size {
        return Err(io::Error::other("File range is past the end"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; (expected_size - offset).min(u64::from(length)) as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn media_mime(extension: &str) -> Option<&'static str> {
    match extension {
        "wav" => Some("audio/wav"),
        "ogg" => Some("audio/ogg"),
        "mp3" => Some("audio/mpeg"),
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        "mkv" => Some("video/x-matroska"),
        "avi" => Some("video/x-msvideo"),
        "3gp" => Some("video/3gpp"),
        "ogv" => Some("video/ogg"),
        _ => None,
    }
}

fn split_location(target: &str) -> (&str, Option<u32>) {
    if let Some((path, location)) = target.rsplit_once("#L") {
        let line = location.split('-').next().and_then(|s| s.parse().ok());
        if line.is_some() {
            return (path, line);
        }
    }
    if let Some((path, last)) = target.rsplit_once(':')
        && let Ok(last) = last.parse::<u32>()
    {
        if let Some((path, line)) = path.rsplit_once(':')
            && let Ok(line) = line.parse::<u32>()
        {
            return (path, Some(line));
        }
        return (path, Some(last));
    }
    (target, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_files_stream_instead_of_entering_the_text_preview() {
        let root =
            std::env::temp_dir().join(format!("tcode-audio-preview-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for (name, mime) in [
            ("sample.WAV", "audio/wav"),
            ("sample.ogg", "audio/ogg"),
            ("sample.mp3", "audio/mpeg"),
        ] {
            let path = root.join(name);
            let size = MAX_FILE_PREVIEW_BYTES as u64 + 1;
            File::create(&path).unwrap().set_len(size).unwrap();
            assert_eq!(
                open(name, Some(&root)).unwrap(),
                FilePreview {
                    path: path.canonicalize().unwrap(),
                    content: FilePreviewContent::Video {
                        size,
                        mime: mime.into()
                    },
                }
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_host_links_and_refuses_unpreviewable_files() {
        let root =
            std::env::temp_dir().join(format!("tcode-file-preview-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("my file.rs");
        std::fs::write(&path, "first\nsecond\nthird").unwrap();
        let canonical = path.canonicalize().unwrap();
        for (link, line) in [
            ("my%20file.rs:2:7".to_owned(), Some(2)),
            (format!("{}#L3", path.display()), Some(3)),
            (url::Url::from_file_path(&path).unwrap().to_string(), None),
        ] {
            assert_eq!(
                open(&link, Some(&root)).unwrap(),
                FilePreview {
                    path: canonical.clone(),
                    content: FilePreviewContent::Text {
                        text: "first\nsecond\nthird".into(),
                        line
                    },
                }
            );
        }
        assert!(open("missing.rs", Some(&root)).is_err());
        assert!(open(root.to_str().unwrap(), None).is_err());
        std::fs::write(root.join("binary"), [0, 255, 1]).unwrap();
        assert!(open("binary", Some(&root)).is_err());
        let large = File::create(root.join("large.txt")).unwrap();
        large.set_len(MAX_FILE_PREVIEW_BYTES as u64 + 1).unwrap();
        assert!(open("large.txt", Some(&root)).is_err());
        std::fs::write(
            root.join("image.png"),
            include_bytes!("../../../assets/icons/app/tcode.png"),
        )
        .unwrap();
        assert!(matches!(
            open("image.png", Some(&root)).unwrap().content,
            FilePreviewContent::Image { .. }
        ));
        std::fs::rename(root.join("large.txt"), root.join("video.mp4")).unwrap();
        assert!(matches!(
            open("video.mp4", Some(&root)).unwrap().content,
            FilePreviewContent::Video { .. }
        ));
        assert_eq!(read_range(&path, 6, 6, 18).unwrap(), b"second");
        assert!(read_range(&path, 0, MAX_FILE_RANGE_BYTES as u32 + 1, 18).is_err());
        assert!(read_range(&path, 0, 1, 19).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
