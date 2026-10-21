//! Files attached to a Coordinator message or to a todo item.
//!
//! Uploaded files live in the planner's own state folder
//! (`~/.local/state/herdr-planner/attachments`), never in the repository or the
//! board's data folder. Three classes are accepted, decided by the file bytes
//! rather than by the name or the declared MIME type: images (PNG, JPEG, WebP,
//! GIF), sound (MP3, WAV, OGG, FLAC, M4A, AAC) and plain UTF-8 text. Files are
//! removed after [`RETENTION`] by [`cleanup`], which runs at startup and after
//! every upload, except those a todo item still holds.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;

/// The largest image a browser may upload.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// The largest sound file.
pub const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;
/// The largest text file.
pub const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
/// The multipart request gets room for boundaries and headers on top of the biggest file.
pub const MAX_BODY_BYTES: usize = MAX_AUDIO_BYTES + 1024 * 1024;
/// How long an uploaded image is kept.
pub const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Files in one Coordinator message or one todo item.
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
pub enum Class {
    Image,
    Audio,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    pub mime: &'static str,
    pub ext: &'static str,
    pub class: Class,
}

impl Kind {
    /// The most this class of file may weigh.
    pub fn limit(&self) -> usize {
        match self.class {
            Class::Image => MAX_IMAGE_BYTES,
            Class::Audio => MAX_AUDIO_BYTES,
            Class::Text => MAX_TEXT_BYTES,
        }
    }
}

const fn img(mime: &'static str, ext: &'static str) -> Kind {
    Kind {
        mime,
        ext,
        class: Class::Image,
    }
}

const fn audio(mime: &'static str, ext: &'static str) -> Kind {
    Kind {
        mime,
        ext,
        class: Class::Audio,
    }
}

const TEXT: Kind = Kind {
    mime: "text/plain; charset=utf-8",
    ext: "txt",
    class: Class::Text,
};

/// Recognises an allowed file: an image or a sound by its first bytes, text by
/// being UTF-8 without control characters. Nothing else is accepted: an HTML
/// file named `shot.png` is text, not an image, and is stored and served as
/// plain text.
pub fn sniff(bytes: &[u8]) -> Option<Kind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(img("image/png", "png"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(img("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(img("image/gif", "gif"))
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(img("image/webp", "webp"))
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        Some(audio("audio/wav", "wav"))
    } else if bytes.starts_with(b"OggS") {
        Some(audio("audio/ogg", "ogg"))
    } else if bytes.starts_with(b"fLaC") {
        Some(audio("audio/flac", "flac"))
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"M4A " | b"M4B ") {
        Some(audio("audio/mp4", "m4a"))
    } else if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xf6 == 0xf0 {
        // ADTS: the layer bits are 00.
        Some(audio("audio/aac", "aac"))
    } else if bytes.starts_with(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0 && bytes[1] & 0x06 != 0)
    {
        Some(audio("audio/mpeg", "mp3"))
    } else if is_text(bytes) {
        Some(TEXT)
    } else {
        None
    }
}

/// Plain text: valid UTF-8, not empty, no NUL or other control characters
/// besides tab, newlines and form feed.
fn is_text(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    !text.is_empty()
        && text
            .chars()
            .all(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r' | '\x0c'))
}

fn kind_for_ext(ext: &str) -> Option<Kind> {
    match ext {
        "png" => Some(img("image/png", "png")),
        "jpg" => Some(img("image/jpeg", "jpg")),
        "gif" => Some(img("image/gif", "gif")),
        "webp" => Some(img("image/webp", "webp")),
        "wav" => Some(audio("audio/wav", "wav")),
        "ogg" => Some(audio("audio/ogg", "ogg")),
        "flac" => Some(audio("audio/flac", "flac")),
        "m4a" => Some(audio("audio/mp4", "m4a")),
        "aac" => Some(audio("audio/aac", "aac")),
        "mp3" => Some(audio("audio/mpeg", "mp3")),
        "txt" => Some(TEXT),
        _ => None,
    }
}

/// Accepts only names this module creates: 32 lowercase hex digits and one of
/// the known extensions. A path, `..` or an unknown extension never matches.
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
        let word = match kind.class {
            Class::Image => "картинка",
            Class::Audio => "звук",
            Class::Text => "текст",
        };
        format!("{word}.{}", kind.ext)
    } else {
        cleaned.to_string()
    }
}

/// What the page may upload, in words.
pub const ACCEPTED: &str =
    "поддерживаются картинки (PNG, JPEG, WebP, GIF), звук (MP3, WAV, OGG, FLAC, M4A, AAC) и текст (UTF-8)";

/// Checks a file's bytes: its kind, and that it is not over its class's limit.
pub fn check(bytes: &[u8]) -> Result<Kind, String> {
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err(format!("файл больше {} МБ", MAX_AUDIO_BYTES / (1024 * 1024)));
    }
    let kind = sniff(bytes).ok_or_else(|| ACCEPTED.to_string())?;
    if bytes.len() > kind.limit() {
        return Err(format!("файл больше {} МБ", kind.limit() / (1024 * 1024)));
    }
    Ok(kind)
}

/// Validates and stores one file. It appears under a random name with
/// mode `600`; the folder is `700`.
pub async fn save(dir: &Path, raw_name: Option<&str>, bytes: &[u8]) -> Result<Attachment, String> {
    let kind = check(bytes)?;

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
            Err(e) => return Err(format!("не удалось сохранить файл: {e}")),
        };
        if let Err(e) = file.write_all(bytes).await {
            let _ = tokio::fs::remove_file(&file_path).await;
            return Err(format!("не удалось сохранить файл: {e}"));
        }
        if let Err(e) = file.flush().await {
            let _ = tokio::fs::remove_file(&file_path).await;
            return Err(format!("не удалось сохранить файл: {e}"));
        }
        return Ok(Attachment {
            id,
            name: clean_name(raw_name, kind),
            mime: kind.mime.to_string(),
            size: bytes.len() as u64,
        });
    }
    Err(format!(
        "не удалось выбрать имя файла: {last_error}"
    ))
}

/// Deletes old files and interrupted uploads, except those named in `keep` (the
/// files a todo item still holds). Only files this module names are touched, so
/// a stray file in the folder is left alone.
pub fn cleanup(dir: &Path, now: SystemTime, keep: &HashSet<String>) {
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
        if (parse_id(name).is_none() && !name.starts_with(".tmp-")) || keep.contains(name) {
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
    let images = paths.iter().all(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .and_then(kind_for_ext)
            .is_some_and(|k| k.class == Class::Image)
    });
    out.push_str(if images {
        "Картинки (файлы на машине с доской):\n"
    } else {
        "Вложения (файлы на машине с доской):\n"
    });
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
    fn sound_is_recognised_by_its_bytes() {
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00WAVEfmt ").unwrap().ext, "wav");
        assert_eq!(sniff(b"OggS\x00\x02").unwrap().ext, "ogg");
        assert_eq!(sniff(b"fLaC\x00\x00").unwrap().ext, "flac");
        assert_eq!(sniff(b"ID3\x04\x00\x00").unwrap().ext, "mp3");
        assert_eq!(sniff(&[0xff, 0xfb, 0x90, 0x00]).unwrap().ext, "mp3");
        assert_eq!(sniff(&[0xff, 0xf1, 0x50, 0x80]).unwrap().ext, "aac");
        assert_eq!(sniff(b"\x00\x00\x00\x20ftypM4A \x00\x00").unwrap().ext, "m4a");
        assert_eq!(sniff(b"OggS\x00\x02").unwrap().class, Class::Audio);
        // a video container is not sound
        assert!(sniff(b"\x00\x00\x00\x20ftypisom\x00\x00\x02\x00").is_none());
    }

    #[test]
    fn text_is_utf8_without_control_characters() {
        let text = sniff("Привет, мир\n\tстрока 2".as_bytes()).unwrap();
        assert_eq!((text.ext, text.class), ("txt", Class::Text));
        // HTML and SVG are text too: they are stored and served as plain text, never as markup
        assert_eq!(sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>").unwrap().mime, "text/plain; charset=utf-8");
        assert!(sniff(b"").is_none());
        assert!(sniff(b"a\x00b").is_none());
        assert!(sniff(&[0xc3, 0x28]).is_none());
        assert!(sniff(b"RIFF\x00\x00\x00\x00AVI LIST").is_none());
        assert!(sniff(b"\x89PNG").is_none());
        assert!(sniff(&[0x7f, 0x45, 0x4c, 0x46]).is_none());
    }

    #[test]
    fn only_generated_file_names_resolve() {
        let id = "0123456789abcdef0123456789abcdef.png";
        assert_eq!(parse_id(id).unwrap().mime, "image/png");
        assert_eq!(mime(id), Some("image/png"));
        assert_eq!(parse_id("0123456789abcdef0123456789abcdef.txt").unwrap().class, Class::Text);
        assert_eq!(parse_id("0123456789abcdef0123456789abcdef.mp3").unwrap().class, Class::Audio);
        assert!(parse_id("0123456789abcdef0123456789abcdef.html").is_none());
        assert!(parse_id("0123456789abcdef0123456789abcdef.exe").is_none());
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
        let mut big = vec![0u8; MAX_IMAGE_BYTES + 1];
        big[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        assert!(save(&dir, Some("big.png"), &big)
            .await
            .unwrap_err()
            .contains("10 МБ"));
        let long_text = vec![b'a'; MAX_TEXT_BYTES + 1];
        assert!(save(&dir, Some("long.txt"), &long_text)
            .await
            .unwrap_err()
            .contains("2 МБ"));
        let huge = vec![b'a'; MAX_AUDIO_BYTES + 1];
        assert!(save(&dir, Some("huge.bin"), &huge)
            .await
            .unwrap_err()
            .contains("25 МБ"));
        assert!(save(&dir, Some("prog.png"), b"\x7fELF\x02\x01")
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
        cleanup(&dir, now, &HashSet::new());
        assert!(fresh.exists());
        assert!(!stale.exists());
        assert!(untouched.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_a_todo_item_holds_is_not_cleaned() {
        let dir = temp_dir("keep");
        let held = dir.join("fedcba9876543210fedcba9876543210.txt");
        let lost = dir.join("0123456789abcdef0123456789abcdef.txt");
        for name in [&held, &lost] {
            let mut f = fs::File::create(name).unwrap();
            f.write_all(b"text").unwrap();
            let old = SystemTime::now() - RETENTION - Duration::from_secs(60);
            f.set_times(fs::FileTimes::new().set_modified(old)).unwrap();
        }
        let keep: HashSet<String> = ["fedcba9876543210fedcba9876543210.txt".to_string()].into();
        cleanup(&dir, SystemTime::now(), &keep);
        assert!(held.exists());
        assert!(!lost.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn sound_and_text_are_stored_with_their_own_names() {
        let dir = temp_dir("kinds");
        let wav = save(&dir, None, b"RIFF\x00\x00\x00\x00WAVEfmt ").await.unwrap();
        assert_eq!((wav.name.as_str(), wav.mime.as_str()), ("звук.wav", "audio/wav"));
        let note = save(&dir, Some("notes.md"), "# заметка".as_bytes()).await.unwrap();
        assert_eq!(note.name, "notes.md");
        assert!(note.id.ends_with(".txt"));
        assert_eq!(note.mime, "text/plain; charset=utf-8");
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
        let mixed = vec![PathBuf::from("/s/a.png"), PathBuf::from("/s/b.txt")];
        assert!(message("смотри", &mixed).contains("Вложения (файлы на машине с доской):"));
    }
}
