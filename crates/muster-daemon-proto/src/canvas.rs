use muster_core::canvas::{CanvasDocument, CanvasDocumentKind, CanvasKind};

use crate::{CanvasDocument as WireDocument, CanvasKind as WireKind};

pub fn kind(kind: WireKind) -> CanvasKind {
    match kind {
        WireKind::Unspecified => CanvasKind::Unknown,
        WireKind::Markdown => CanvasKind::Markdown,
        WireKind::Html => CanvasKind::Html,
        WireKind::Image => CanvasKind::Image,
    }
}

pub fn encode(document: CanvasDocument) -> WireDocument {
    let CanvasDocument { kind, bytes, title } = document;
    let (kind, image_type) = match kind {
        CanvasDocumentKind::Markdown => (WireKind::Markdown, ""),
        CanvasDocumentKind::Html => (WireKind::Html, ""),
        CanvasDocumentKind::Image(image) => (WireKind::Image, image.media_type()),
    };
    WireDocument { kind: kind.into(), body: bytes, image_type: image_type.into(), title }
}

pub fn decode(document: WireDocument) -> Result<CanvasDocument, &'static str> {
    CanvasDocument::from_parts(
        kind(document.kind()),
        document.body,
        document.title,
        &document.image_type,
    )
}
