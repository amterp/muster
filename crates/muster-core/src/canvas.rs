//! Build the page a shell draws in a canvas and state the rules it must enforce.

use std::str::{Utf8Error, from_utf8};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use pulldown_cmark::{Event, Options, Parser, html};

use crate::mirror::PaneId;

const MARKDOWN_STYLE: &str = include_str!("canvas/markdown.css");
const IMAGE_STYLE: &str = include_str!("canvas/image.css");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasKind {
    Unknown,
    Markdown,
    Html,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasInfo {
    pub revision: u64,
    pub kind: CanvasKind,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasMetadata {
    pub info: CanvasInfo,
    pub bytes: u64,
    pub opened_by: Option<PaneId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasDocument {
    pub kind: CanvasDocumentKind,
    pub bytes: Vec<u8>,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasDocumentKind {
    Markdown,
    Html,
    Image(CanvasImageType),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasImageType {
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
}

impl CanvasImageType {
    fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Svg => "image/svg+xml",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasPage {
    pub html: String,
    pub policy: CanvasPolicy,
}

/// Install the load rules before showing the page and refuse to load it if that fails.
/// Keep page navigation, file access, popups, and JavaScript dialogs disabled. Open a
/// link externally only when the person clicks it. Use a non-persistent data store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanvasPolicy {
    pub load_schemes: &'static [&'static str],
    pub scripts_enabled: bool,
    pub external_link_schemes: &'static [&'static str],
}

const POLICY: CanvasPolicy = CanvasPolicy {
    load_schemes: &["data", "blob", "about"],
    scripts_enabled: false,
    external_link_schemes: &["http", "https", "mailto"],
};

/// Render the document as HTML. The daemon validates the image bytes before they get here.
pub fn render(document: &CanvasDocument) -> Result<CanvasPage, Utf8Error> {
    let html = match document.kind {
        CanvasDocumentKind::Markdown => {
            let source = from_utf8(&document.bytes)?;
            let options = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
            let mut page = page_start(&document.title, MARKDOWN_STYLE);
            html::push_html(&mut page, Parser::new_ext(source, options));
            finish_page(page)
        }
        CanvasDocumentKind::Html => {
            let source = from_utf8(&document.bytes)?;
            if is_html_document(source) {
                source.to_owned()
            } else {
                let mut page = page_start(&document.title, "");
                page.push_str(source);
                finish_page(page)
            }
        }
        CanvasDocumentKind::Image(image_type) => {
            let mut page = page_start(&document.title, IMAGE_STYLE);
            page.push_str(concat!(
                r#"<input type="checkbox" id="actual-size" aria-label="Show image at actual size">"#,
                r#"<label for="actual-size"><img alt="Canvas image" src="data:"#,
            ));
            page.push_str(image_type.media_type());
            page.push_str(";base64,");
            STANDARD.encode_string(&document.bytes, &mut page);
            page.push_str(r#""></label>"#);
            finish_page(page)
        }
    };
    Ok(CanvasPage { html, policy: POLICY })
}

fn is_html_document(source: &str) -> bool {
    let mut start =
        source.strip_prefix('\u{feff}').unwrap_or(source).trim_start_matches(is_html_whitespace);
    while let Some((_, rest)) = start.strip_prefix("<!--").and_then(|head| head.split_once("-->")) {
        start = rest.trim_start_matches(is_html_whitespace);
    }
    let Some((name, _)) = start
        .strip_prefix('<')
        .and_then(|tag| tag.split_once(|c| is_html_whitespace(c) || matches!(c, '>' | '/')))
    else {
        return false;
    };
    ["!doctype", "html", "head", "body"].iter().any(|tag| name.eq_ignore_ascii_case(tag))
}

fn is_html_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

fn page_start(title: &str, style: &str) -> String {
    let mut page = String::from(concat!(
        r#"<!doctype html><html><head><meta charset="utf-8">"#,
        r#"<meta name="viewport" content="width=device-width, initial-scale=1"><title>"#,
    ));
    html::push_html(&mut page, std::iter::once(Event::Text(title.into())));
    page.push_str("</title>");
    if !style.is_empty() {
        page.push_str("<style>");
        page.push_str(style);
        page.push_str("</style>");
    }
    page.push_str("</head><body>");
    page
}

fn finish_page(mut page: String) -> String {
    page.push_str("</body></html>");
    page
}
