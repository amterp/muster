//! What the kernel says about a pane's processes. One arm per OS the daemon runs on.

use std::path::PathBuf;

/// The working directory of a process, if it still exists and the kernel will say.
#[cfg(target_os = "macos")]
pub(crate) fn cwd(pid: i32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
    let size = i32::try_from(size_of::<libc::proc_vnodepathinfo>()).ok()?;
    // SAFETY: `info` is valid for `size` bytes of writes, which is all proc_pidinfo writes.
    let written = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, info.as_mut_ptr().cast(), size)
    };
    if written != size {
        return None;
    }
    // SAFETY: proc_pidinfo filled the whole struct.
    let info = unsafe { info.assume_init() };
    let path = &info.pvi_cdir.vip_path;
    // SAFETY: the path is a contiguous array of c_char, which has u8's size and alignment, and
    // the slice covers exactly its bytes. (libc splits MAXPATHLEN into 32 rows of 32.)
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(std::ptr::from_ref(path).cast(), size_of_val(path)) };
    let end = bytes.iter().position(|&byte| byte == 0)?;
    (end > 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..end])))
}

#[cfg(target_os = "linux")]
pub(crate) fn cwd(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}
