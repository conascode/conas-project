//! Pictures in notes: stored as files in %APPDATA%\gaeunmemo\images\<uuid>.<ext>
//! (never inside notes.json, which is rewritten up to ~3x a second while typing)
//! and served to the webviews through the `gmimg` URI scheme.

use crate::model::{image_refs, valid_image_name, Data};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const MAX_BYTES: usize = 25 * 1024 * 1024;

pub fn dir() -> PathBuf {
    crate::store::data_dir().join("images")
}

/// File type from the first bytes; the caller's claim is not trusted.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

pub fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("png") => "image/png",
        Some("jpg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Saves a picture and returns its file name.
pub fn save_in(dir: &Path, bytes: &[u8]) -> Result<String, String> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err("사진이 비어 있거나 25MB를 넘어요".into());
    }
    let ext = sniff(bytes).ok_or("PNG, JPG, GIF, WEBP 사진만 넣을 수 있어요")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = format!("{}.{ext}", crate::model::new_id());
    crate::store::write_atomic(&dir.join(&name), bytes).map_err(|e| e.to_string())?;
    Ok(name)
}

/// Reads a picture for the `gmimg` scheme; only well-formed names, only from the images folder.
pub fn read_in(dir: &Path, name: &str) -> Option<Vec<u8>> {
    if !valid_image_name(name) {
        return None;
    }
    fs::read(dir.join(name)).ok()
}

pub fn referenced(data: &Data) -> HashSet<String> {
    data.notes.iter().flat_map(|n| image_refs(&n.html)).collect()
}

/// Deletes pictures no note refers to. Files younger than `grace` are kept:
/// a picture pasted moments ago may not have been saved into a note yet.
pub fn remove_unreferenced(dir: &Path, keep: &HashSet<String>, grace: Duration) -> usize {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    let now = SystemTime::now();
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !valid_image_name(&name) || keep.contains(&name) {
            continue;
        }
        let young = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| now.duration_since(t).unwrap_or_default() < grace)
            .unwrap_or(true);
        if !young && fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Note;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    #[test]
    fn sniffs_types_and_rejects_others() {
        assert_eq!(sniff(PNG), Some("png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff(b"GIF89a"), Some("gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff(b"<svg onload=x>"), None);
        assert_eq!(sniff(b"MZ\x90\0"), None);
    }

    #[test]
    fn save_read_and_path_safety() {
        let dir = tempfile::tempdir().unwrap();
        let name = save_in(dir.path(), PNG).unwrap();
        assert!(valid_image_name(&name) && name.ends_with(".png"));
        assert_eq!(read_in(dir.path(), &name).unwrap(), PNG);
        assert!(read_in(dir.path(), "../notes.json").is_none());
        assert!(save_in(dir.path(), b"not a picture").is_err());
    }

    #[test]
    fn orphans_removed_referenced_and_young_kept() {
        let dir = tempfile::tempdir().unwrap();
        let used = save_in(dir.path(), PNG).unwrap();
        let orphan = save_in(dir.path(), PNG).unwrap();
        let data = Data { notes: vec![Note { html: format!("<img data-gm=\"{used}\">"), ..Default::default() }], ..Default::default() };
        let keep = referenced(&data);
        assert_eq!(remove_unreferenced(dir.path(), &keep, Duration::from_secs(3600)), 0, "young files kept");
        assert_eq!(remove_unreferenced(dir.path(), &keep, Duration::ZERO), 1);
        assert!(dir.path().join(&used).exists());
        assert!(!dir.path().join(&orphan).exists());
    }
}
