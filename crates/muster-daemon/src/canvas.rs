//! A canvas document on disk. Its revision and bytes commit together.

use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use muster_core::diagnostics::poison;
use muster_daemon_proto as proto;
use prost::Message as _;

const MAX_BODY: usize = 4 << 20;
const MAX_HEADER: usize = proto::connection::LARGEST_MESSAGE as usize;
const MAGIC: &[u8; 8] = b"MUSTERC1";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct Store {
    directory: PathBuf,
    parent_synced: Mutex<bool>,
}

#[derive(Debug)]
pub(crate) struct Document {
    pub(crate) revision: u64,
    pub(crate) content: proto::CanvasDocument,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Header {
    #[prost(uint64, tag = "1")]
    revision: u64,
    #[prost(int32, tag = "2")]
    kind: i32,
    #[prost(string, tag = "3")]
    title: String,
    #[prost(string, tag = "4")]
    image_type: String,
    #[prost(uint32, tag = "5")]
    length: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    BeforeRename,
    AfterRename,
}

#[derive(Debug)]
pub(crate) struct WriteError {
    pub(crate) stage: Stage,
    pub(crate) error: io::Error,
}

impl Store {
    pub(crate) fn beside(socket: &Path) -> Self {
        let mut directory = socket.as_os_str().to_os_string();
        directory.push(".canvases");
        Self { directory: PathBuf::from(directory), parent_synced: Mutex::new(false) }
    }

    pub(crate) fn path(&self, pane: &str) -> PathBuf {
        self.directory.join(file_name(pane))
    }

    pub(crate) fn read(&self, pane: &str) -> io::Result<Document> {
        self.check_directory()?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.path(pane))?;
        read_document(file)
    }

    pub(crate) fn write(
        &self,
        pane: &str,
        document: &Document,
        creating: bool,
        at: impl Fn(Stage) -> io::Result<()>,
    ) -> Result<(), WriteError> {
        let before = |error| WriteError { stage: Stage::BeforeRename, error };
        let after = |error| WriteError { stage: Stage::AfterRename, error };
        let header = Header {
            revision: document.revision,
            kind: document.content.kind,
            title: document.content.title.clone(),
            image_type: document.content.image_type.clone(),
            length: u32::try_from(document.content.body.len())
                .map_err(|error| before(io::Error::other(error)))?,
        };
        let header_bytes = header.encode_to_vec();
        let length = u32::try_from(header_bytes.len())
            .ok()
            .filter(|&length| length as usize <= MAX_HEADER)
            .ok_or_else(|| before(io::Error::other("the canvas header exceeds a daemon frame")))?;
        self.prepare_directory().map_err(before)?;
        let path = self.path(pane);
        match fs::symlink_metadata(&path) {
            Ok(_) if creating => {
                return Err(before(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a canvas file of this name already exists",
                )));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(before(io::Error::other("the canvas path is not a regular file")));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && creating => {}
            Err(error) => return Err(before(error)),
        }
        let (temp, mut file) = self.temporary().map_err(before)?;
        let prepared = (|| {
            file.write_all(MAGIC)?;
            file.write_all(&length.to_be_bytes())?;
            file.write_all(&header_bytes)?;
            file.write_all(&document.content.body)?;
            file.sync_all()?;
            at(Stage::BeforeRename)?;
            fs::rename(&temp, &path)
        })();
        if let Err(error) = prepared {
            let _ = fs::remove_file(&temp);
            return Err(before(error));
        }
        at(Stage::AfterRename).map_err(after)?;
        sync_directory(&self.directory).map_err(after)
    }

    fn temporary(&self) -> io::Result<(PathBuf, File)> {
        loop {
            let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = self.directory.join(format!(".tmp-{}-{nonce}", std::process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
            {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn prepare_directory(&self) -> io::Result<()> {
        let mut synced = poison::lock(&self.parent_synced, "daemon.canvas.directory");
        match DirBuilder::new().mode(0o700).create(&self.directory) {
            Ok(()) => *synced = false,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        self.check_directory()?;
        if !*synced {
            sync_directory(parent(&self.directory))?;
            *synced = true;
        }
        Ok(())
    }

    fn check_directory(&self) -> io::Result<()> {
        let directory = fs::symlink_metadata(&self.directory)?;
        // SAFETY: geteuid takes no arguments.
        let owner = unsafe { libc::geteuid() };
        if !directory.is_dir()
            || directory.uid() != owner
            || directory.permissions().mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the canvas store must be a private directory owned by this user",
            ));
        }
        Ok(())
    }
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|part| !part.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn file_name(pane: &str) -> String {
    if pane.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !pane.is_empty()
    {
        return pane.to_string();
    }
    let mut name = String::with_capacity(1 + pane.len() * 2);
    name.push('~');
    for byte in pane.bytes() {
        write!(name, "{byte:02x}").expect("writing to a string cannot fail");
    }
    name
}

fn read_document(mut file: File) -> io::Result<Document> {
    let invalid = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("the canvas path is not a regular file"));
    }
    let mut magic = [0; MAGIC.len()];
    file.read_exact(&mut magic)?;
    if magic != *MAGIC {
        return Err(invalid("unrecognized canvas file format"));
    }
    let mut length = [0; 4];
    file.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_HEADER {
        return Err(invalid("the canvas header exceeds a daemon frame"));
    }
    let mut header = vec![0; length];
    file.read_exact(&mut header)?;
    let header = Header::decode(header.as_slice()).map_err(|error| invalid(&error.to_string()))?;
    let body_length = header.length as usize;
    if header.revision == 0
        || body_length > MAX_BODY
        || metadata.len() != (MAGIC.len() + 4 + length + body_length) as u64
    {
        return Err(invalid("invalid canvas revision or file length"));
    }
    let mut body = vec![0; body_length];
    file.read_exact(&mut body)?;
    let content = proto::CanvasDocument {
        kind: header.kind,
        body,
        title: header.title,
        image_type: header.image_type,
    };
    validate(&content).map_err(|why| invalid(&why))?;
    Ok(Document { revision: header.revision, content })
}

pub(crate) fn validate(document: &proto::CanvasDocument) -> Result<(), String> {
    if document.body.len() > MAX_BODY {
        return Err("a canvas document is at most 4 MiB".to_string());
    }
    match document.kind() {
        proto::CanvasKind::Markdown | proto::CanvasKind::Html => {
            if !document.image_type.is_empty() {
                return Err("a Markdown or HTML canvas has no image type".to_string());
            }
            std::str::from_utf8(&document.body)
                .map_err(|_| "canvas text must be UTF-8".to_string())?;
        }
        proto::CanvasKind::Image => {
            if !image_is(&document.image_type, &document.body) {
                return Err(format!("the canvas bytes do not read as {}", document.image_type));
            }
        }
        proto::CanvasKind::Unspecified => {
            return Err("the canvas document needs a kind".to_string());
        }
    }
    Ok(())
}

fn image_is(kind: &str, bytes: &[u8]) -> bool {
    match kind {
        "image/png" => {
            muster_core::input::image::check_png(bytes).is_ok()
                && bytes.get(8..16) == Some(b"\0\0\0\rIHDR")
                && bytes.ends_with(b"\0\0\0\0IEND\xaeB\x60\x82")
        }
        "image/jpeg" => {
            bytes.len() >= 8 && bytes.starts_with(b"\xff\xd8\xff") && bytes.ends_with(b"\xff\xd9")
        }
        "image/gif" => {
            bytes.len() >= 14
                && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
                && bytes.ends_with(b";")
        }
        "image/webp" => {
            bytes.len() >= 20
                && bytes.starts_with(b"RIFF")
                && bytes.get(8..12) == Some(b"WEBP")
                && matches!(bytes.get(12..16), Some(b"VP8 " | b"VP8L" | b"VP8X"))
                && u32::from_le_bytes(bytes[4..8].try_into().expect("checked length")) as usize
                    == bytes.len() - 8
        }
        "image/svg+xml" => std::str::from_utf8(bytes).is_ok_and(is_svg),
        _ => false,
    }
}

fn is_svg(text: &str) -> bool {
    let mut text = text.strip_prefix('\u{feff}').unwrap_or(text).trim_ascii_start();
    if let Some(declaration) = text.strip_prefix("<?xml")
        && declaration.as_bytes().first().is_some_and(u8::is_ascii_whitespace)
    {
        let Some((_, rest)) = declaration.split_once("?>") else { return false };
        text = rest.trim_ascii_start();
    }
    let mut has_doctype = false;
    loop {
        if let Some(comment) = text.strip_prefix("<!--") {
            let Some((_, rest)) = comment.split_once("-->") else { return false };
            text = rest.trim_ascii_start();
        } else if let Some(doctype) = text.strip_prefix("<!DOCTYPE")
            && !has_doctype
            && doctype.as_bytes().first().is_some_and(u8::is_ascii_whitespace)
        {
            let Some(rest) = after_doctype(doctype) else { return false };
            text = rest.trim_ascii_start();
            has_doctype = true;
        } else {
            break;
        }
    }
    text.strip_prefix("<svg").is_some_and(|rest| {
        rest.starts_with('>')
            || rest.starts_with("/>")
            || (rest.as_bytes().first().is_some_and(u8::is_ascii_whitespace) && rest.contains('>'))
    })
}

fn after_doctype(text: &str) -> Option<&str> {
    let mut quote = None;
    let mut brackets = 0_u32;
    let mut at = 0;
    while let Some(&byte) = text.as_bytes().get(at) {
        if let Some(opened) = quote {
            if byte == opened {
                quote = None;
            }
        } else if text.as_bytes()[at..].starts_with(b"<!--") {
            let (_, rest) = text.get(at + 4..)?.split_once("-->")?;
            at = text.len() - rest.len();
            continue;
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'[' => brackets += 1,
                b']' => brackets = brackets.checked_sub(1)?,
                b'>' if brackets == 0 => return text.get(at + 1..),
                _ => {}
            }
        }
        at += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (PathBuf, Store) {
        let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("muster-cv-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let store = Store::beside(&directory.join("daemon.sock"));
        (directory, store)
    }

    fn markdown(revision: u64, body: &str) -> Document {
        Document {
            revision,
            content: proto::CanvasDocument {
                kind: proto::CanvasKind::Markdown.into(),
                body: body.as_bytes().to_vec(),
                title: String::new(),
                image_type: String::new(),
            },
        }
    }

    #[test]
    fn a_document_read_back_has_its_bytes_and_revision_together() {
        let (directory, store) = scratch();
        let first = markdown(1, "# Erste");
        store.write("p1", &first, true, |_| Ok(())).unwrap();
        let second = markdown(2, "# Deuxième");
        store.write("p1", &second, false, |_| Ok(())).unwrap();
        let read = store.read("p1").unwrap();
        assert_eq!(read.revision, second.revision);
        assert_eq!(read.content, second.content);
        assert_eq!(fs::metadata(store.path("p1")).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_failed_sync_reports_whether_the_rename_happened() {
        let (directory, store) = scratch();
        let first = markdown(1, "old");
        store.write("p1", &first, true, |_| Ok(())).unwrap();
        let second = markdown(2, "new");
        for (stage, expected) in [(Stage::BeforeRename, &first), (Stage::AfterRename, &second)] {
            let error = store
                .write("p1", &second, false, |at| {
                    if stage == at { Err(io::Error::other("injected I/O failure")) } else { Ok(()) }
                })
                .unwrap_err();
            assert_eq!(error.stage, stage);
            assert_eq!(error.error.to_string(), "injected I/O failure");
            let read = store.read("p1").unwrap();
            assert_eq!(read.revision, expected.revision);
            assert_eq!(read.content, expected.content);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pane_names_are_just_file_names() {
        let (directory, store) = scratch();
        for pane in ["paper", "PAPER", "..", "x/../paper", "/paper", "~5041504552"] {
            let document = markdown(1, pane);
            store.write(pane, &document, true, |_| Ok(())).unwrap();
            assert_eq!(store.path(pane).parent(), Some(store.directory.as_path()));
            assert_eq!(store.read(pane).unwrap().content, document.content);
        }
        let refused = store.write("paper", &markdown(2, "new"), true, |_| Ok(())).unwrap_err();
        assert_eq!(refused.error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(store.read("paper").unwrap().content.body, b"paper");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_document_limit_is_inclusive_and_text_must_be_utf8() {
        let mut content = markdown(1, "").content;
        content.body.resize(MAX_BODY, b'a');
        assert!(validate(&content).is_ok());
        content.body.push(b'a');
        assert!(validate(&content).is_err());
        content.body.pop();
        content.body[0] = 0xff;
        assert!(validate(&content).is_err());
    }

    #[test]
    fn image_kinds_are_checked_against_the_file_bytes() {
        let png = include_bytes!("../tests/fixtures/canvas/pixel.png").to_vec();
        let gif = include_bytes!("../tests/fixtures/canvas/pixel.gif").to_vec();
        let jpeg = include_bytes!("../tests/fixtures/canvas/pixel.jpeg").to_vec();
        let webp = include_bytes!("../tests/fixtures/canvas/pixel.webp").to_vec();
        let svg = "\u{feff}<?xml version=\"1.0\"?><!--document--><!DOCTYPE svg [<!-- ] --> <!ENTITY x \"a>b\">]><svg/>";
        for (kind, body) in [
            ("image/png", png),
            ("image/gif", gif),
            ("image/jpeg", jpeg),
            ("image/webp", webp),
            ("image/svg+xml", svg.as_bytes().to_vec()),
        ] {
            let document = proto::CanvasDocument {
                kind: proto::CanvasKind::Image.into(),
                body,
                image_type: kind.to_string(),
                title: String::new(),
            };
            assert!(validate(&document).is_ok(), "{kind}");
            let mut mislabeled = document.clone();
            mislabeled.image_type =
                if kind == "image/png" { "image/gif" } else { "image/png" }.to_string();
            assert!(validate(&mislabeled).is_err());
            assert!(!image_is(kind, b"<html><!--<svg/>--></html>"));
        }
        assert!(!is_svg("<svg-fragment/>"));
        assert!(!is_svg("<!-- <svg/>"));
        assert!(!is_svg("<!DOCTYPE svg [><svg/>"));
    }
}
