//! The C ABI, and nothing else.
//!
//! Everything here is a shim over [`dispatch`], which takes bytes and returns bytes and is
//! reachable by an ordinary test. That split is deliberate: a shim carrying logic is logic
//! no test can run without a shell, and this boundary is the one place in Muster where a
//! wrong assumption is a crash in someone's window rather than a red suite.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

use muster_core::diagnostics::poison;

use crate::dispatch;
use crate::proto::Event;
use prost::Message;

/// What the shell registers so the core can wake it unasked.
///
/// The bytes are an encoded `Event`, valid only for the duration of the call - the shell
/// copies what it needs. Called from whatever thread noticed the change, which is never
/// the shell's main thread, so an implementation that touches a window has to hop.
pub type EventCallback = extern "C" fn(bytes: *const u8, len: usize);

static CALLBACK: Mutex<Option<EventCallback>> = Mutex::new(None);

/// What this file calls the registration lock when it reports one recovered.
const WHAT: &str = "event-callback";

/// Tells the shell something it did not ask about.
///
/// Silently does nothing when no shell is listening, which is the ordinary state in a test
/// and during the moment before startup finishes.
///
/// The lock is released before the call, because a shell reacting to an event by
/// dispatching a request is ordinary and must not deadlock against the registration it
/// went through to get here.
pub fn emit(event: &Event) {
    let callback = *poison::lock(&CALLBACK, WHAT);
    let Some(callback) = callback else { return };
    let bytes = event.encode_to_vec();
    callback(bytes.as_ptr(), bytes.len());
}

/// # Safety
/// The pointer must be null or a function that stays callable for the process's life.
#[unsafe(no_mangle)]
pub extern "C" fn muster_set_event_callback(callback: Option<EventCallback>) {
    *poison::lock(&CALLBACK, WHAT) = callback;
}

/// # Safety
/// `request` must point at `len` readable bytes, and `out_len` at a writable `usize`. The
/// returned buffer belongs to the caller until it is handed to [`muster_free`] with the
/// same length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn muster_dispatch(
    request: *const u8,
    len: usize,
    out_len: *mut usize,
) -> *const u8 {
    if out_len.is_null() {
        return std::ptr::null();
    }
    // An empty request is spelled as a null pointer rather than as a dangling one, which
    // is what an empty Swift array hands over.
    let bytes = if request.is_null() || len == 0 {
        &[][..]
    } else {
        // SAFETY: the caller guarantees `request` covers `len` readable bytes, and the
        // slice does not outlive this call.
        unsafe { std::slice::from_raw_parts(request, len) }
    };

    // A panic unwinding into C is undefined behavior, and the shell's own failures arrive
    // here as data. Turning one into a null answer costs a keystroke; letting it cross
    // costs the window.
    //
    // "Later requests still work" is a claim about the locks rather than about this guard,
    // and it is only true because a poisoned one is recovered rather than re-panicked on
    // (`muster_core::diagnostics::poison`). Catching the panic here while every later
    // acquisition of the lock it died under panicked in turn would have made this message
    // a lie: the window would have gone on rendering pane output from the data plane while
    // ignoring every key, which is the failure that is hardest to report because nothing
    // about it looks like a crash.
    let Ok(response) = catch_unwind(AssertUnwindSafe(|| dispatch(bytes))) else {
        eprintln!(
            "muster: the core panicked answering a request, so this one went unanswered.\n\
             The app keeps running and later requests are answered, but whatever this one \
             was about did not happen, and any state the panic was part-way through \
             writing is left as it was - `lock.poisoned` in the run log says which, if \
             any. The backtrace above names the core function; it is a bug in muster-seam \
             or below, not in the shell."
        );
        // SAFETY: checked non-null above.
        unsafe { *out_len = 0 };
        return std::ptr::null();
    };

    let response = response.into_boxed_slice();
    // SAFETY: checked non-null above.
    unsafe { *out_len = response.len() };
    Box::into_raw(response).cast::<u8>()
}

/// # Safety
/// `response` and `len` must be exactly what one [`muster_dispatch`] call returned, and
/// must not have been freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn muster_free(response: *mut u8, len: usize) {
    if response.is_null() {
        return;
    }
    // SAFETY: the caller guarantees this came from `muster_dispatch`, which produced it
    // with `Box::into_raw` over a slice of exactly this length.
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(response, len)) });
}
