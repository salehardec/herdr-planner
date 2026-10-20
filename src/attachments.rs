//! Images attached to a Coordinator message.
//!
//! Uploaded images live in the planner's own state folder
//! (`~/.local/state/herdr-planner/attachments`), never in the repository or the
//! board's data folder. Only PNG, JPEG, WebP and GIF are accepted, decided by
//! the file bytes rather than by the name or the declared MIME type. Files are
//! removed after [`RETENTION`] by [`cleanup`], which runs at startup and after
//! every upload.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;

/// The largest image a browser may upload.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// The multipart request gets room for boundaries and headers on top.
pub const MAX_BODY_BYTES: usize = MAX_IMAGE_BYTES + 1024 * 1024;
/// How long an uploaded image is kept.
pub const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Images in one Coordinator message.
pub const MAX_PER_MESSAGE: usize = 8;

const ID_HEX: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    /// File name inside the attachments folder (`<32 hex>.png`).
    pub id: String,
    /// The original name, cleaned for display only.
    pub name: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    pub mime: &'static str,
    pub ext: &'static str,
}

/// Recognises an allowed image by its first bytes. Nothing else is accepted:
/// an HTML or SVG file named `shot.png` stays HTML or SVG.
pub fn sniff(bytes: &[u8]) -> Option<Kind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind {
            mime: "image/png",
            ext: "png",
        })
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(Kind {
            mime: "image/jpeg",
            ext: "jpg",
        })
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Kind {
            mime: "image/gif",
            ext: "gif",
        })
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(Kind {
            mime: "image/webp",
            ext: "webp",
        })
    } else {
        None
    }
}

fn kind_for_ext(ext: &str) -> Option<Kind> {
    match ext {
        "png" => Some(Kind {
            mime: "image/png",
            ext: "png",
        }),
        "jpg" => Some(Kind {
            mime: "image/jpeg",
            ext: "jpg",
        }),
        "gif" => Some(Kind {
            mime: "image/gif",
            ext: "gif",
        }),
        "webp" => Some(Kind {
            mime: "image/webp",
            ext: "webp",
        }),
        _ => None,
    }
}

/// Accepts only names this module creates: 32 lowercase hex digits and one of
/// the four extensions. A path, `..` or an unknown extension never matches.
pub fn parse_id(id: &str) -> Option<Kind> {
    let (stem, ext) = id.rsplit_once('.')?;
    if stem.len() != ID_HEX
        || !stem
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    kind_for_ext(ext)
}

pub fn path(dir: &Path, id: &str) -> Option<PathBuf> {
    parse_id(id)?;
    Some(dir.join(id))
}

pub fn mime(id: &str) -> Option<&'static str> {
    parse_id(id).map(|k| k.mime)
}

fn new_id(kind: Kind) -> String {
    use rand::RngCore;
    let mut raw = [0u8; ID_HEX / 2];
    rand::thread_rng().fill_bytes(&mut raw);
    let mut hex = String::with_capacity(ID_HEX);
    for b in raw {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("{hex}.{}", kind.ext)
}

/// The original name is only shown; it never becomes a path.
pub fn clean_name(raw: Option<&str>, kind: Kind) -> String {
    let base = raw.unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control()).take(120).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        format!("картинка.{}", kind.ext)
    } else {
        cleaned.to_string()
    }
}

/// Validates and stores one image. The file appears under a random name with
/// mode `600`; the folder is `700`.
pub async fn save(dir: &Path, raw_name: Option<&str>, bytes: &[u8]) -> Result<Attachment, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "картинка больше {} МБ",
            MAX_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    let kind =
        sniff(bytes).ok_or_else(|| "поддерживаются только PNG, JPEG, WebP и GIF".to_string())?;

    fs::create_dir_all(dir).map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }

    let mut last_error = String::new();
    for _ in 0..5 {
        let id = new_id(kind);
        let file_path = dir.join(&id);
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = match options.open(&file_path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = e.to_string();
                continue;
            }
            Err(e) => return Err(format!("не удалось сохранить картинку: {e}")),
        };
        if let Err(e) = file.write_all(bytes).await {
            let _ = tokio::fs::remove_file(&file_path).await;
            return Err(format!("не удалось сохранить картинку: {e}"));
        }
        if let Err(e) = file.flush().await {
            let _ = tokio::fs::remove_file(&file_path).await;
            return Err(format!("не удалось сохранить картинку: {e}"));
        }
        return Ok(Attachment {
            id,
            name: clean_name(raw_name, kind),
            mime: kind.mime.to_string(),
            size: bytes.len() as u64,
        });
    }
    Err(format!(
        "не удалось выбрать имя файла картинки: {last_error}"
    ))
}

/// Deletes old images and interrupted uploads. Only files this module names are
/// touched, so a stray file in the folder is left alone.
pub fn cleanup(dir: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if parse_id(name).is_none() && !name.starts_with(".tmp-") {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age > RETENTION {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// What `herdr-projects coordinator tell` receives: the human's text followed
/// by the absolute paths of the attached files on this machine.
pub fn message(text: &str, paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return text.to_string();
    }
    let mut out = text.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("Картинки (файлы на машине с доской):\n");
    for path in paths {
        out.push_str(&path.display().to_string());
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "herdr-attachments-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn allowed_images_are_recognised_by_their_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest").unwrap().ext, "png");
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]).unwrap().ext, "jpg");
        assert_eq!(sniff(b"GIF89a....").unwrap().ext, "gif");
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00WEBPVP8 ").unwrap().ext, "webp");
    }

    #[test]
    fn other_files_and_truncated_headers_are_refused() {
        assert!(sniff(b"<html><body>no</body></html>").is_none());
        assert!(sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>").is_none());
        assert!(sniff(b"RIFF\x00\x00\x00\x00WAVE").is_none());
        assert!(sniff(b"").is_none());
        assert!(sniff(b"\x89PNG").is_none());
    }

    #[test]
    fn only_generated_file_names_resolve() {
        let id = "0123456789abcdef0123456789abcdef.png";
        assert_eq!(parse_id(id).unwrap().mime, "image/png");
        assert_eq!(mime(id), Some("image/png"));
        assert!(parse_id("../0123456789abcdef0123456789abcdef.png").is_none());
        assert!(parse_id("0123456789abcdef0123456789abcdef.svg").is_none());
        assert!(parse_id("../../etc/passwd").is_none());
        assert!(parse_id("0123456789ABCDEF0123456789abcdef.png").is_none());
        assert!(path(Path::new("/tmp"), "../../etc/passwd").is_none());
    }

    #[tokio::test]
    async fn an_upload_is_stored_under_a_random_name() {
        let dir = temp_dir("save");
        let bytes = b"\x89PNG\r\n\x1a\npng body";
        let a = save(&dir, Some("../../My Shot.png"), bytes).await.unwrap();
        assert_eq!(a.name, "My Shot.png");
        assert_eq!(a.mime, "image/png");
        assert_eq!(a.size, bytes.len() as u64);
        assert_eq!(a.id.rsplit_once('.').unwrap().1, "png");
        assert_eq!(fs::read(dir.join(&a.id)).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(&a.id)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn oversized_and_foreign_uploads_are_refused() {
        let dir = temp_dir("refuse");
        let big = vec![0u8; MAX_IMAGE_BYTES + 1];
        assert!(save(&dir, Some("big.png"), &big)
            .await
            .unwrap_err()
            .contains("10 МБ"));
        assert!(save(&dir, Some("page.png"), b"<html>nope</html>")
            .await
            .unwrap_err()
            .contains("PNG"));
        assert!(!dir.exists() || fs::read_dir(&dir).unwrap().next().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_files_are_cleaned_and_fresh_ones_stay() {
        let dir = temp_dir("cleanup");
        let fresh = dir.join("0123456789abcdef0123456789abcdef.png");
        let stale = dir.join("fedcba9876543210fedcba9876543210.jpg");
        let untouched = dir.join("notes.txt");
        for name in [&fresh, &stale, &untouched] {
            let mut f = fs::File::create(name).unwrap();
            f.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        }
        let now = SystemTime::now();
        let old = now - RETENTION - Duration::from_secs(60);
        for name in [&stale, &untouched] {
            let f = fs::File::options().write(true).open(name).unwrap();
            f.set_times(fs::FileTimes::new().set_modified(old)).unwrap();
        }
        cleanup(&dir, now);
        assert!(fresh.exists());
        assert!(!stale.exists());
        assert!(untouched.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_coordinator_gets_the_text_and_absolute_paths() {
        let paths = vec![
            PathBuf::from("/home/me/.local/state/herdr-planner/attachments/a.png"),
            PathBuf::from("/home/me/.local/state/herdr-planner/attachments/b.jpg"),
        ];
        let text = message("посмотри скриншоты", &paths);
        assert!(text.starts_with("посмотри скриншоты\n\n"));
        assert!(text.contains("Картинки"));
        assert!(text.contains("/home/me/.local/state/herdr-planner/attachments/a.png"));
        assert!(text.contains("/home/me/.local/state/herdr-planner/attachments/b.jpg"));
        assert_eq!(
            message("", &paths).lines().next(),
            Some("Картинки (файлы на машине с доской):")
        );
        assert_eq!(message("просто текст", &[]), "просто текст");
    }
}
