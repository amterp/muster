//! Which libghostty-vt this build encodes with.
//!
//! Worth a line in every run log. The VT library decides what a keystroke becomes, and it
//! is reproduced from `deps/ghostty.pin` rather than installed - so a bug report where the
//! encoding is wrong wants to say which engine produced it, and "the pin in the repo
//! today" is not an answer about a run from last week.

use std::os::unix::ffi::OsStrExt;

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

/// The libghostty-vt this process loaded, when it loaded one from a file of its own.
///
/// On a Mac the library is a dylib beside whatever links it, and a daemon copied to another Mac
/// has to take it along: this is the file the running daemon's builds were linked against, so
/// it is the right one to send. `None` where the library is linked in statically, which is how
/// every Linux build has it.
pub fn library_path() -> Option<std::path::PathBuf> {
    let mut info = libc::Dl_info {
        dli_fname: std::ptr::null(),
        dli_fbase: std::ptr::null_mut(),
        dli_sname: std::ptr::null(),
        dli_saddr: std::ptr::null_mut(),
    };
    let symbol = ffi::ghostty_build_info as *const libc::c_void;
    // SAFETY: `symbol` is a function this process has linked, and `info` is a local dladdr
    // fills in. The name it points at belongs to the loaded image, which is never unloaded.
    let found = unsafe { libc::dladdr(symbol, &raw mut info) };
    if found == 0 || info.dli_fname.is_null() {
        return None;
    }
    // SAFETY: dladdr reported a NUL-terminated path owned by the loader.
    let path = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
    let path = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()));
    path.extension().is_some_and(|extension| extension == "dylib").then_some(path)
}

#[cfg(test)]
mod tests {
    /// A Mac build links libghostty-vt as a dylib, which a daemon copied to another Mac takes
    /// along; this is where that copy comes from.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_build_names_the_libghostty_vt_it_loaded() {
        let path = super::library_path().expect("a Mac build loads libghostty-vt from a file");
        // The file itself, which may carry its version: `libghostty-vt.dylib` is a link to it,
        // and a copy sent elsewhere is given that name back.
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("libghostty-vt") && name.ends_with(".dylib"), "{name}");
        assert!(path.is_file(), "{} is not there", path.display());
    }
}
