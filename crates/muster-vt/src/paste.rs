//! What a paste puts on a program's input.
//!
//! libghostty-vt's own paste encoding: control bytes that could end a bracketed paste early
//! or run a command are replaced, and the text is fenced when the program asked for
//! bracketed paste (mode 2004) or has its newlines turned into returns when it did not.

use crate::ffi;

/// Whether `text` can go to a program that did not ask for bracketed paste without a person
/// confirming it: no newline that would run what precedes it, and no bracketed-paste end
/// sequence that would let the rest escape the fence.
pub fn paste_is_safe(text: &str) -> bool {
    // SAFETY: the bytes are live for the call and their length is honest.
    unsafe { ffi::ghostty_paste_is_safe(text.as_ptr().cast(), text.len()) }
}

/// The bytes for pasting `text` into a program with bracketed paste on or off.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    // libghostty rewrites unsafe bytes in the input as it goes, so it gets a copy.
    let mut input = text.as_bytes().to_vec();
    let mut output = vec![0u8; input.len() + 16];
    let mut written = 0usize;
    // SAFETY: both buffers are ours with honest lengths; on OUT_OF_SPACE libghostty writes
    // the size it needs instead.
    let mut result = unsafe { encode(&mut input, bracketed, &mut output, &raw mut written) };
    if result == ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        output = vec![0u8; written];
        input = text.as_bytes().to_vec();
        // SAFETY: as above, with the capacity libghostty asked for.
        result = unsafe { encode(&mut input, bracketed, &mut output, &raw mut written) };
    }
    if result != ffi::GhosttyResult_GHOSTTY_SUCCESS {
        return Vec::new();
    }
    output.truncate(written);
    output
}

unsafe fn encode(
    input: &mut [u8],
    bracketed: bool,
    output: &mut [u8],
    written: *mut usize,
) -> ffi::GhosttyResult {
    // SAFETY: the caller guarantees `written` points at a usize it owns; both slices are live.
    unsafe {
        ffi::ghostty_paste_encode(
            input.as_mut_ptr().cast(),
            input.len(),
            bracketed,
            output.as_mut_ptr().cast(),
            output.len(),
            written,
        )
    }
}
