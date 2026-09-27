//! Descriptors from one daemon to the next. Each set rides a single byte sent with it as
//! SCM_RIGHTS, just before the frame it belongs to, so the frames themselves stay plain.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// Room for the control message carrying `count` descriptors, in u64s so the buffer is aligned
/// as a `cmsghdr` must be; how much of it the message takes, which macOS wants exactly; and
/// how many bytes of descriptors it carries.
fn control_buffer(count: usize) -> io::Result<(Vec<u64>, u32, u32)> {
    let bytes = u32::try_from(count * size_of::<RawFd>()).map_err(io::Error::other)?;
    // SAFETY: CMSG_SPACE only computes a size.
    let space = unsafe { libc::CMSG_SPACE(bytes) };
    Ok((vec![0; (space as usize).div_ceil(size_of::<u64>())], space, bytes))
}

/// Sends `fds` to the other end of `socket`, which takes them with [`receive`].
pub(crate) fn send(socket: &UnixStream, fds: &[BorrowedFd<'_>]) -> io::Result<()> {
    let raw: Vec<RawFd> = fds.iter().map(AsRawFd::as_raw_fd).collect();
    let (mut control, space, bytes) = control_buffer(raw.len())?;
    let mut byte = [0u8];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    // SAFETY: a zeroed msghdr is a valid empty one; every pointer set below outlives the call,
    // and the header CMSG_FIRSTHDR finds lies within `control`, which CMSG_SPACE sized for
    // `bytes` of data.
    unsafe {
        let mut message = std::mem::zeroed::<libc::msghdr>();
        message.msg_iov = &raw mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        // `as _` because the field is a u32 on macOS and a usize on Linux.
        #[allow(clippy::cast_lossless)]
        {
            message.msg_controllen = space as _;
        }
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(bytes) as _;
        std::ptr::copy_nonoverlapping(
            raw.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(header),
            bytes as usize,
        );
        loop {
            match libc::sendmsg(socket.as_raw_fd(), &raw const message, 0) {
                1 => return Ok(()),
                -1 if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => {}
                -1 => return Err(io::Error::last_os_error()),
                _ => return Err(io::Error::other("the descriptors' byte was not sent")),
            }
        }
    }
}

/// Takes the `count` descriptors [`send`] sent, each close-on-exec: nothing a daemon is handed
/// may reach a pane it starts.
pub(crate) fn receive(socket: &UnixStream, count: usize) -> io::Result<Vec<OwnedFd>> {
    let (mut control, space, _) = control_buffer(count)?;
    let mut byte = [0u8];
    let mut iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_CMSG_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = 0;
    let mut fds = Vec::new();
    // SAFETY: as in `send`; recvmsg writes at most `msg_controllen` bytes of control data, and
    // each header CMSG_NXTHDR walks to lies within it. Every descriptor read out is one this
    // process now holds, and is owned from here.
    unsafe {
        let mut message = std::mem::zeroed::<libc::msghdr>();
        message.msg_iov = &raw mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        // `as _` because the field is a u32 on macOS and a usize on Linux.
        #[allow(clippy::cast_lossless)]
        {
            message.msg_controllen = space as _;
        }
        let received = loop {
            match libc::recvmsg(socket.as_raw_fd(), &raw mut message, flags) {
                -1 if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => {}
                -1 => return Err(io::Error::last_os_error()),
                received => break received,
            }
        };
        if received == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let mut header = libc::CMSG_FIRSTHDR(&raw const message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(header);
                let length = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                for index in 0..length / size_of::<RawFd>() {
                    let fd = data.cast::<RawFd>().add(index).read_unaligned();
                    fds.push(OwnedFd::from_raw_fd(fd));
                }
            }
            header = libc::CMSG_NXTHDR(&raw const message, header);
        }
        if message.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::other("the descriptors sent were more than expected"));
        }
    }
    for fd in &fds {
        // SAFETY: fcntl on a descriptor this process owns.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    if fds.len() != count {
        return Err(io::Error::other(format!("{} descriptors came, not {count}", fds.len())));
    }
    Ok(fds)
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::os::fd::AsFd;

    use super::*;

    /// What one end writes to a descriptor sent over is what the other end reads from the
    /// descriptor it kept, and the frame after the descriptors arrives whole.
    #[test]
    fn descriptors_arrive_open_and_close_on_exec_before_the_frame_after_them() {
        let (sending, receiving) = UnixStream::pair().unwrap();
        let (mut kept, sent) = UnixStream::pair().unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        send(&sending, &[sent.as_fd(), file.as_fd()]).unwrap();
        (&sending).write_all(b"frame").unwrap();
        drop(sent);

        let mut fds = receive(&receiving, 2).unwrap();
        let mut frame = [0u8; 5];
        (&receiving).read_exact(&mut frame).unwrap();
        assert_eq!(&frame, b"frame");
        for fd in &fds {
            // SAFETY: fcntl on a descriptor this test owns.
            let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
            assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
        }
        let mut arrived = UnixStream::from(fds.remove(0));
        arrived.write_all(b"hello").unwrap();
        let mut read = [0u8; 5];
        kept.read_exact(&mut read).unwrap();
        assert_eq!(&read, b"hello");
    }

    #[test]
    fn a_count_that_does_not_match_is_an_error() {
        let (sending, receiving) = UnixStream::pair().unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        send(&sending, &[file.as_fd()]).unwrap();
        assert!(receive(&receiving, 2).is_err());
    }
}
