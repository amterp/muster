//! How urgently the OS runs the threads between a keystroke and its glyph.

/// Marks the calling thread as one a person is waiting on.
///
/// On macOS a thread without a class runs at the default one, which the scheduler may put on an
/// efficiency core and, in an application Launch Services started with no window of its own,
/// throttle further. Only the threads a keystroke and its echo pass through ask for more: a
/// pane's reader and writer, a stream's writer and the input connection. Elsewhere the
/// scheduler has no such classes, and this does nothing.
pub(crate) fn interactive() {
    #[cfg(target_os = "macos")]
    // SAFETY: sets the calling thread's own class; it reads and writes no memory of ours.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}
