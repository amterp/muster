use super::CanvasImageType;

pub(super) fn matches(kind: CanvasImageType, bytes: &[u8]) -> bool {
    match kind {
        CanvasImageType::Png => png(bytes),
        CanvasImageType::Jpeg => jpeg(bytes),
        CanvasImageType::Gif => gif(bytes),
        CanvasImageType::Webp => webp(bytes),
        CanvasImageType::Svg => std::str::from_utf8(bytes).is_ok_and(is_svg),
    }
}

fn png(bytes: &[u8]) -> bool {
    let Some(mut chunks) = bytes.strip_prefix(b"\x89PNG\r\n\x1a\n") else { return false };
    if !chunks.starts_with(b"\0\0\0\rIHDR") {
        return false;
    }
    let mut image = false;
    while let Some((header, rest)) = chunks.split_first_chunk::<8>() {
        let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let Some(remaining) = size.checked_add(4).and_then(|end| rest.get(end..)) else {
            return false;
        };
        match &header[4..] {
            b"IDAT" => image |= size > 0,
            b"IEND" => return image && size == 0 && remaining.is_empty(),
            _ => {}
        }
        chunks = remaining;
    }
    false
}

fn jpeg(bytes: &[u8]) -> bool {
    let Some(mut segments) =
        bytes.strip_prefix(b"\xff\xd8").and_then(|rest| rest.strip_suffix(b"\xff\xd9"))
    else {
        return false;
    };
    let mut frame = false;
    while segments.starts_with(b"\xff") {
        let first = segments.iter().position(|&byte| byte != 0xff).unwrap_or(segments.len());
        let Some((marker, rest)) = segments.get(first..).and_then(<[u8]>::split_first) else {
            return false;
        };
        let Some((length, _)) = rest.split_first_chunk::<2>() else { return false };
        let length = u16::from_be_bytes(*length) as usize;
        if !(2..=rest.len()).contains(&length) {
            return false;
        }
        segments = &rest[length..];
        match marker {
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                frame |= length >= 8;
            }
            0xda => return frame && length >= 6 && !segments.is_empty(),
            _ => {}
        }
    }
    false
}

fn gif(bytes: &[u8]) -> bool {
    let Some((header, blocks)) = bytes.split_first_chunk::<13>() else { return false };
    if !matches!(&header[..6], b"GIF87a" | b"GIF89a") {
        return false;
    }
    let Some(mut blocks) = blocks.get(gif_table_length(header[10])..) else { return false };
    let mut image = false;
    while let Some((kind, rest)) = blocks.split_first() {
        blocks = match kind {
            0x3b => return image && rest.is_empty(),
            0x21 => match rest.get(1..).and_then(gif_subblocks) {
                Some(remaining) => remaining,
                None => return false,
            },
            0x2c => {
                let Some((descriptor, rest)) = rest.split_first_chunk::<9>() else {
                    return false;
                };
                let Some((code_size, data)) =
                    rest.get(gif_table_length(descriptor[8])..).and_then(<[u8]>::split_first)
                else {
                    return false;
                };
                if !(2..=8).contains(code_size) || data.first() == Some(&0) {
                    return false;
                }
                let Some(remaining) = gif_subblocks(data) else { return false };
                image = true;
                remaining
            }
            _ => return false,
        };
    }
    false
}

fn gif_table_length(packed: u8) -> usize {
    if packed & 0x80 == 0 { 0 } else { 3 << (1 + (packed & 7)) }
}

fn gif_subblocks(mut bytes: &[u8]) -> Option<&[u8]> {
    loop {
        let (&length, rest) = bytes.split_first()?;
        if length == 0 {
            return Some(rest);
        }
        bytes = rest.get(usize::from(length)..)?;
    }
}

fn webp(bytes: &[u8]) -> bool {
    let Some((header, chunks)) = bytes.split_first_chunk::<12>() else { return false };
    &header[..4] == b"RIFF"
        && &header[8..] == b"WEBP"
        && u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize
            == bytes.len() - 8
        && matches!(chunks.get(..4), Some(b"VP8 " | b"VP8L" | b"VP8X"))
        && webp_chunks(chunks, true)
}

fn webp_chunks(mut chunks: &[u8], frames: bool) -> bool {
    let mut image = false;
    while let Some((header, rest)) = chunks.split_first_chunk::<8>() {
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let Some(payload) = rest.get(..length) else { return false };
        let Some(remaining) = rest.get(length + (length % 2)..) else { return false };
        image |= match &header[..4] {
            b"VP8 " => payload.get(3..6) == Some(b"\x9d\x01\x2a") && payload.len() >= 10,
            b"VP8L" => payload.starts_with(b"\x2f") && payload.len() >= 5,
            b"ANMF" if frames => payload.get(16..).is_some_and(|inner| webp_chunks(inner, false)),
            _ => false,
        };
        chunks = remaining;
    }
    image && chunks.is_empty()
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
