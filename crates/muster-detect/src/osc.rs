//! OSC 9 progress, captured from a pane's raw output, and the title as a manifest reads it.
//!
//! Ported from herdr v0.8.0 `src/pane/osc.rs` (`OscStreamCollector` and
//! `AgentOscStateTracker`; Apache-2.0), and changed: only OSC 9 is captured. The title comes
//! from the daemon's terminal, the one place a pane's title is kept, and only herdr's
//! sanitizing is applied to it here (`title`).
//!
//! Progress needs its own scanner because manifests match its raw payload - `^4;0;0$`,
//! `^4;1;-1$` - and a terminal hands progress on already parsed into a state and a percentage,
//! which is not the same text.

/// herdr's cap on a title or progress payload, in characters: both are whatever the program
/// in the pane chose to write.
const MAX_CHARS: usize = 256;

/// The title as a manifest's `osc_title` region sees it: without control characters, and no
/// longer than herdr kept.
pub fn title(raw: &str) -> String {
    raw.chars().filter(|ch| !ch.is_control()).take(MAX_CHARS).collect()
}

/// The last OSC 9 payload a pane wrote - the text after `9;`, such as `4;3;` - kept for the
/// `osc_progress` region. Fed every chunk of the pane's output, in order.
#[derive(Debug, Default)]
pub struct Progress {
    collector: Collector,
    latest: Option<String>,
}

impl Progress {
    pub fn observe(&mut self, bytes: &[u8]) {
        let latest = &mut self.latest;
        self.collector.observe(bytes, |body| {
            if let Some(payload) = body.strip_prefix(b"9;") {
                *latest = Some(sanitize(payload));
            }
        });
    }

    /// The last payload, or empty when there has been none since the last `clear`.
    pub fn get(&self) -> &str {
        self.latest.as_deref().unwrap_or("")
    }

    /// Forgets the payload, so a new agent does not inherit the last one's. A sequence part
    /// way through arriving is kept, and belongs to the new agent when it completes.
    pub fn clear(&mut self) {
        self.latest = None;
    }
}

fn sanitize(payload: &[u8]) -> String {
    String::from_utf8_lossy(payload).chars().filter(|ch| !ch.is_control()).take(MAX_CHARS).collect()
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Complete OSC bodies out of a byte stream, across chunk boundaries.
#[derive(Debug, Default)]
struct Collector {
    state: State,
    body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Ground,
    Escape,
    Body,
    BodyEscape,
    /// DCS, SOS, PM or APC: a string that may hold anything, including what looks like an OSC.
    IgnoringString,
    IgnoringStringEscape,
    /// An OSC past the size cap, dropped up to its terminator.
    Discarding,
    DiscardingEscape,
}

impl Collector {
    const MAX_BODY_BYTES: usize = 4096;

    fn observe(&mut self, bytes: &[u8], mut receive: impl FnMut(&[u8])) {
        let mut index = 0;
        while index < bytes.len() {
            if self.state == State::Ground {
                // Nearly all output is text, and nothing in it matters until an escape.
                match bytes[index..].iter().position(|&byte| byte == ESC) {
                    Some(offset) => index += offset,
                    None => return,
                }
            }
            self.step(bytes[index], &mut receive);
            index += 1;
        }
    }

    #[expect(clippy::match_same_arms, reason = "grouped by state, it reads as the state table")]
    fn step(&mut self, byte: u8, receive: &mut impl FnMut(&[u8])) {
        self.state = match (self.state, byte) {
            (State::Ground | State::Escape, ESC) => State::Escape,
            (State::Escape, b']') => {
                self.body.clear();
                State::Body
            }
            (State::Escape, b'P' | b'_' | b'^' | b'X') => State::IgnoringString,
            (State::Ground | State::Escape, _) => State::Ground,
            (State::Body, BEL) | (State::BodyEscape, b'\\') => self.finish(receive),
            (State::Body, ESC) => State::BodyEscape,
            (State::Body, _) => self.push(&[byte]),
            // An escape that turned out not to start a terminator is part of the body, as it
            // was in herdr; a BEL after one still ends it.
            (State::BodyEscape, BEL) => match self.push(&[ESC]) {
                State::Body => self.finish(receive),
                _ => State::Ground,
            },
            (State::BodyEscape, ESC) => match self.push(&[ESC]) {
                State::Body => State::BodyEscape,
                _ => State::DiscardingEscape,
            },
            (State::BodyEscape, _) => match self.push(&[ESC]) {
                State::Body => self.push(&[byte]),
                state => state,
            },
            (State::IgnoringString | State::IgnoringStringEscape, ESC) => {
                State::IgnoringStringEscape
            }
            (State::IgnoringStringEscape, b'\\') => State::Ground,
            (State::IgnoringString | State::IgnoringStringEscape, _) => State::IgnoringString,
            (State::Discarding, BEL) | (State::DiscardingEscape, b'\\') => State::Ground,
            (State::Discarding | State::DiscardingEscape, ESC) => State::DiscardingEscape,
            (State::Discarding | State::DiscardingEscape, _) => State::Discarding,
        };
    }

    fn push(&mut self, bytes: &[u8]) -> State {
        self.body.extend_from_slice(bytes);
        if self.body.len() > Self::MAX_BODY_BYTES {
            self.body.clear();
            State::Discarding
        } else {
            State::Body
        }
    }

    fn finish(&mut self, receive: &mut impl FnMut(&[u8])) -> State {
        receive(&self.body);
        self.body.clear();
        State::Ground
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress_after(chunks: &[&[u8]]) -> String {
        let mut progress = Progress::default();
        for chunk in chunks {
            progress.observe(chunk);
        }
        progress.get().to_string()
    }

    #[test]
    fn osc_stream_collector_ignores_strings_and_preserves_escaped_bytes() {
        let mut collector = Collector::default();
        let mut bodies = Vec::new();
        collector.observe(b"\x1bPignored\x1b]0;not-osc\x07\x1b\\\x1b]9;a\x1b", |body| {
            bodies.push(body.to_vec());
        });
        collector.observe(b"\x1b\\\x1b]2;b\x1b\x07", |body| bodies.push(body.to_vec()));
        assert_eq!(bodies, vec![b"9;a\x1b".to_vec(), b"2;b\x1b".to_vec()]);
    }

    #[test]
    fn osc9_sets_progress_with_bel_and_with_st() {
        assert_eq!(progress_after(&[b"\x1b]9;4;3;\x07"]), "4;3;");
        assert_eq!(progress_after(&[b"\x1b]9;4;3;\x07", b"\x1b]9;4;0;\x1b\\"]), "4;0;");
    }

    #[test]
    fn a_sequence_split_across_chunks_completes() {
        let mut progress = Progress::default();
        progress.observe(b"text \x1b]9;4;3");
        assert_eq!(progress.get(), "");
        progress.observe(b";\x07more text");
        assert_eq!(progress.get(), "4;3;");
    }

    #[test]
    fn an_oversized_payload_is_discarded_and_the_stream_recovers() {
        let mut oversized = b"\x1b]9;".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', 4097));
        oversized.push(BEL);
        assert_eq!(progress_after(&[b"\x1b]9;before\x07", &oversized]), "before");
        assert_eq!(progress_after(&[&oversized, b"\x1b]9;after\x07"]), "after");
    }

    #[test]
    fn the_payload_is_capped_and_stripped_of_control_characters() {
        let long = format!("\x1b]9;{}\x07", "a".repeat(MAX_CHARS + 50));
        assert_eq!(progress_after(&[long.as_bytes()]).len(), MAX_CHARS);
        assert_eq!(progress_after(&[b"\x1b]9;4;\x013;\x07"]), "4;3;");
    }

    #[test]
    fn other_oscs_leave_progress_alone() {
        assert_eq!(
            progress_after(&[
                b"\x1b]9;4;3;\x07\x1b]0;title\x07\x1b]99;x\x07\x1b]4;1;rgb:aa/bb/cc\x07"
            ]),
            "4;3;"
        );
        assert_eq!(progress_after(&[b"\x1b]9\x07"]), "", "an OSC 9 with no payload is none");
    }

    #[test]
    fn clearing_keeps_a_sequence_in_flight() {
        let mut progress = Progress::default();
        progress.observe(b"\x1b]9;4;3;\x07\x1b]9;4;0");
        progress.clear();
        assert_eq!(progress.get(), "");
        progress.observe(b";0\x07");
        assert_eq!(progress.get(), "4;0;0");
    }

    #[test]
    fn titles_are_stripped_of_control_characters_and_capped() {
        assert_eq!(title("before\u{1}after"), "beforeafter");
        assert_eq!(title(&"✳".repeat(MAX_CHARS + 1)).chars().count(), MAX_CHARS);
        assert_eq!(title("✳ 修复🙂标题"), "✳ 修复🙂标题");
    }
}
