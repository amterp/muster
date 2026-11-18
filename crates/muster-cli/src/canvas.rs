use std::io::{Read, Write};

use muster_core::canvas::{CanvasImageType, MAX_BODY};
use muster_proto::{CanvasDocument, CanvasKind, Request, request};

use crate::Trouble;
use crate::args::TextSource;

pub(crate) fn load(
    request: &mut Request,
    from: &TextSource,
    input: &mut impl Read,
) -> Result<(), Trouble> {
    let document = match request.payload.as_mut() {
        Some(request::Payload::OpenCanvas(open)) => open.document.as_mut(),
        Some(request::Payload::WriteCanvas(write)) => write.document.as_mut(),
        _ => None,
    }
    .expect("canvas input requires a document");
    let mut body = Vec::new();
    let limit = (MAX_BODY + 1) as u64;
    let read = match from {
        TextSource::File(path) => {
            std::fs::File::open(path).and_then(|file| file.take(limit).read_to_end(&mut body))
        }
        TextSource::Stdin => input.take(limit).read_to_end(&mut body),
    };
    read.map_err(|error| {
        Trouble::Refused(format!("could not read {} ({error})", from.described()))
    })?;
    if body.len() > MAX_BODY {
        return Err(Trouble::Refused("a canvas document is at most 4 MiB".into()));
    }
    if document.kind() == CanvasKind::Image {
        let kind = CanvasImageType::detect(&body).ok_or_else(|| {
            Trouble::Refused(format!(
                "{} is not a PNG, JPEG, GIF, WebP or SVG image",
                from.described(),
            ))
        })?;
        document.image_type = kind.media_type().to_string();
    } else if std::str::from_utf8(&body).is_err() {
        return Err(Trouble::Refused(format!("{} is not UTF-8 text", from.described())));
    }
    document.body = body;
    Ok(())
}

pub(crate) fn write_original(
    document: &CanvasDocument,
    stdout_is_terminal: bool,
    out: &mut impl Write,
) -> Result<(), Trouble> {
    if stdout_is_terminal && document.kind() == CanvasKind::Image {
        return Err(Trouble::Refused(
            "redirect stdout to a file or pipe to read an image, or use --json for its metadata"
                .into(),
        ));
    }
    out.write_all(&document.body)
        .and_then(|()| out.flush())
        .map_err(|error| Trouble::Refused(format!("could not print the canvas document ({error})")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_input_stops_after_one_byte_past_the_document_limit() {
        let mut input = std::io::Cursor::new(vec![b' '; MAX_BODY + 20]);
        let mut request = Request::new(request::Payload::WriteCanvas(muster_proto::WriteCanvas {
            document: Some(CanvasDocument {
                kind: CanvasKind::Markdown.into(),
                ..CanvasDocument::default()
            }),
            ..muster_proto::WriteCanvas::default()
        }));
        let refusal = load(&mut request, &TextSource::Stdin, &mut input).unwrap_err();
        assert!(refusal.detail().contains("4 MiB"));
        assert_eq!(input.position(), (MAX_BODY + 1) as u64);
    }

    #[test]
    fn canvas_image_read_refuses_a_terminal_before_writing_bytes() {
        let document = CanvasDocument {
            kind: CanvasKind::Image.into(),
            body: vec![0xff],
            ..CanvasDocument::default()
        };
        let mut out = Vec::new();
        assert!(write_original(&document, true, &mut out).is_err());
        assert!(out.is_empty());
    }
}
