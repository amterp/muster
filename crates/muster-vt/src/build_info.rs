//! Which libghostty-vt this build encodes with.
//!
//! Worth a line in every run log. The VT library decides what a keystroke becomes, and it
//! is reproduced from `deps/ghostty.pin` rather than installed - so a bug report where the
//! encoding is wrong wants to say which engine produced it, and "the pin in the repo
//! today" is not an answer about a run from last week.

use crate::ffi;

/// The engine's version string, or `None` if this build of libghostty-vt will not say.
pub fn engine_version() -> Option<String> {
    let mut version = ffi::GhosttyString { ptr: std::ptr::null(), len: 0 };
    // SAFETY: the out parameter is a local of the type build_info.h documents for
    // VERSION_STRING. The string it points at is static, so it outlives this call.
    let result = unsafe {
        ffi::ghostty_build_info(
            ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_STRING,
            (&raw mut version).cast(),
        )
    };
    if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || version.ptr.is_null() || version.len == 0 {
        return None;
    }
    // SAFETY: libghostty reported a pointer and a length for a borrowed static string, and
    // it is copied here rather than held.
    let bytes = unsafe { std::slice::from_raw_parts(version.ptr, version.len) };
    Some(String::from_utf8_lossy(bytes).into_owned())
}
