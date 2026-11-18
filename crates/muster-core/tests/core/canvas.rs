use conformance::{CaseError, Conformance, Outcome};
use muster_core::canvas::{self, CanvasDocument, CanvasDocumentKind, CanvasImageType};
use serde_json::{Value, json};

#[test]
fn canvas_conformance() {
    let corpus = Conformance::load("canvas.json");
    assert_eq!(corpus.run(render_case), corpus.cases.len());
}

fn render_case(given: &Value) -> Outcome {
    let kind = match given["kind"].as_str() {
        Some("markdown") => CanvasDocumentKind::Markdown,
        Some("html") => CanvasDocumentKind::Html,
        Some("png") => CanvasDocumentKind::Image(CanvasImageType::Png),
        Some("svg") => CanvasDocumentKind::Image(CanvasImageType::Svg),
        _ => return Err(CaseError::new("unknown canvas kind")),
    };
    let bytes = match &given["body"] {
        Value::String(text) => text.as_bytes().to_vec(),
        Value::Array(bytes) => bytes
            .iter()
            .map(|byte| {
                byte.as_u64()
                    .and_then(|byte| u8::try_from(byte).ok())
                    .ok_or_else(|| CaseError::new("body must contain bytes"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(CaseError::new("body must be text or bytes")),
    };
    let title = given["title"].as_str().ok_or_else(|| CaseError::new("missing title"))?.into();
    let page = match canvas::render(&CanvasDocument { kind, bytes, title }) {
        Ok(page) => page,
        Err(error) => return Ok(json!({ "invalid_utf8_at": error.valid_up_to() })),
    };
    let html = if kind == CanvasDocumentKind::Html {
        page.html
    } else {
        let (before, _, after) = split_stylesheet(&page.html)?;
        format!("{before}<style></style>{after}")
    };
    Ok(json!({
        "html": html,
        "policy": {
            "load_schemes": page.policy.load_schemes,
            "external_link_schemes": page.policy.external_link_schemes,
            "scripts_enabled": page.policy.scripts_enabled
        }
    }))
}

fn split_stylesheet(html: &str) -> Result<(&str, &str, &str), CaseError> {
    let (before, rest) = html
        .split_once("<style>")
        .filter(|(head, _)| !head.contains("</head>"))
        .ok_or_else(|| CaseError::new("missing canvas stylesheet in the head"))?;
    let (style, after) =
        rest.split_once("</style>").ok_or_else(|| CaseError::new("unclosed stylesheet"))?;
    Ok((before, style, after))
}

#[test]
fn theme_and_image_controls_are_defined_in_the_page() {
    let markdown = canvas::render(&CanvasDocument {
        kind: CanvasDocumentKind::Markdown,
        bytes: Vec::new(),
        title: String::new(),
    })
    .unwrap();
    let (_, style, _) = split_stylesheet(&markdown.html).unwrap();
    for variable in ["background", "foreground", "accent", "font", "mono"] {
        assert!(
            style.contains(&format!("var(--muster-{variable},")),
            "missing fallback for {variable}"
        );
    }

    let image = canvas::render(&CanvasDocument {
        kind: CanvasDocumentKind::Image(CanvasImageType::Svg),
        bytes: b"<svg/>".to_vec(),
        title: String::new(),
    })
    .unwrap();
    let (_, style, _) = split_stylesheet(&image.html).unwrap();
    assert!(style.contains("place-items: center"));
    assert!(style.contains("inline-size: 100%"));
    assert!(style.contains("block-size: 100vh"));
    assert!(style.contains("object-fit: scale-down"));
    assert!(style.contains("#actual-size:checked + label img"));
    assert!(style.contains("#actual-size:focus-visible + label"));
}
