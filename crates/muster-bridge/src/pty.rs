//! The two things this process needs from the terminal it was handed.

use std::sync::mpsc::{Receiver, Sender, channel};

/// The surface's grid, read from the PTY libghostty gave us.
///
/// This is why resize needs no channel of its own: libghostty sizes the PTY from the
/// surface's pixels and font metrics, so asking the PTY is asking the surface.
pub(crate) fn terminal_size() -> (u16, u16) {
    let mut size = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ writes a winsize we own, and writes nothing when it fails.
    let queried = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut size) };
    if queried != 0 || size.ws_col == 0 { (80, 24) } else { (size.ws_col, size.ws_row) }
}

/// Stops the line discipline echoing what libghostty writes to our stdin.
///
/// Nothing reads stdin - input takes the control plane - but without this the discipline
/// would echo and buffer whatever arrives there, painting over the frames just rendered.
pub(crate) fn make_stdin_raw() {
    // SAFETY: both calls take a termios we own, and a failure leaves it unchanged.
    unsafe {
        let mut raw: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &raw mut raw) == 0 {
            libc::cfmakeraw(&raw mut raw);
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw const raw);
        }
    }
}

/// Reports every window resize, on a channel.
///
/// `sigwait` on a dedicated thread rather than a signal handler: a handler may call almost
/// nothing, so it would have to set a flag that something else polls. Blocking the signal
/// in every thread and waiting for it in one turns an asynchronous interruption into an
/// ordinary blocking read, which is the shape the rest of this process is written in.
///
/// Must be called before any other thread is spawned, so they inherit the block.
pub(crate) fn watch_for_resize() -> Receiver<()> {
    let (sender, receiver) = channel();

    // SAFETY: sigemptyset and sigaddset initialize a set we own; signal installs a handler
    // that does nothing and touches nothing; pthread_sigmask changes only this thread's
    // mask, and every thread spawned after this inherits it.
    unsafe {
        let mut blocked: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut blocked);
        libc::sigaddset(&raw mut blocked, libc::SIGWINCH);

        // A handler that will never run, and without which none of this works. SIGWINCH's
        // default disposition is to be ignored, and a signal that is ignored is discarded
        // where it is generated rather than left pending - so the `sigwait` below waits
        // forever and every resize is dropped in silence. Installing any handler takes the
        // disposition off SIG_IGN; blocking it then keeps the handler from ever being
        // reached, and the signal becomes pending for `sigwait` to collect.
        libc::signal(libc::SIGWINCH, handle_nothing as *const () as libc::sighandler_t);
        libc::pthread_sigmask(libc::SIG_BLOCK, &raw const blocked, std::ptr::null_mut());
        std::thread::spawn(move || wait_for_signals(blocked, &sender));
    }

    receiver
}

/// Never called. It exists to be a disposition rather than to be run - see above.
extern "C" fn handle_nothing(_signal: libc::c_int) {}

fn wait_for_signals(blocked: libc::sigset_t, sender: &Sender<()>) {
    loop {
        let mut signal = 0;
        // SAFETY: the set is blocked in every thread, which is what sigwait requires, and
        // it writes only the signal number into a local.
        if unsafe { libc::sigwait(&raw const blocked, &raw mut signal) } != 0 {
            return;
        }
        // A closed channel means the pump has exited and this pane is finished.
        if sender.send(()).is_err() {
            return;
        }
    }
}
