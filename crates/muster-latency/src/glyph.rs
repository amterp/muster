//! Whether a letter reached a surface: present in its bytes once escape sequences are taken out.

/// `bytes` less its escape sequences: CSI, OSC, DCS/PM/APC, and two-byte escapes. A letter inside
/// one - the `m` of a style, the `h` of a mode - is not a glyph.
pub(crate) fn text(bytes: &[u8]) -> Vec<u8> {
    let mut text = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != 0x1b {
            text.push(bytes[at]);
            at += 1;
            continue;
        }
        at += 1;
        match bytes.get(at) {
            Some(b'[') => {
                at += 1;
                while at < bytes.len() && !(0x40..=0x7e).contains(&bytes[at]) {
                    at += 1;
                }
                at += 1;
            }
            Some(b']' | b'P' | b'^' | b'_') => {
                at += 1;
                while at < bytes.len() {
                    if bytes[at] == 0x07 {
                        at += 1;
                        break;
                    }
                    if bytes[at] == 0x1b && bytes.get(at + 1) == Some(&b'\\') {
                        at += 2;
                        break;
                    }
                    at += 1;
                }
            }
            Some(_) => at += 1,
            None => {}
        }
    }
    text
}

pub(crate) fn shows(bytes: &[u8], letter: u8) -> bool {
    text(bytes).contains(&letter)
}

/// Whether `needle` appears in `bytes` outside escape sequences.
pub(crate) fn shows_text(bytes: &[u8], needle: &str) -> bool {
    text(bytes).windows(needle.len()).any(|window| window == needle.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_inside_escape_sequences_are_not_glyphs() {
        assert!(!shows(b"\x1b[?25h\x1b[1m\x1b]0;hello\x07\x1b(B", b'h'));
        assert!(!shows(b"\x1bPtmux;x\x1b\\", b'x'));
        assert!(shows(b"\x1b[1mx\x1b[0m", b'x'));
        assert_eq!(text(b"a\x1b[31mb\x1b]2;t\x1b\\c\x1b7d"), b"abcd");
    }
}
