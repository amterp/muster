//! The macOS probe: `proc_listpids` for a group's members, `proc_pidinfo` for each one's group
//! and name, and `KERN_PROCARGS2` for its arguments and environment.
//!
//! Ported from herdr v0.8.0 `src/platform/macos.rs` (Apache-2.0), and changed: a process's
//! argument block is read once for both its argv and its argv[0], where herdr read it twice.

use std::path::Path;

use super::{Job, Process, agent_hint_in};

/// `PROC_PGRP_ONLY` from `<sys/proc_info.h>`, which the libc crate does not carry.
const PROC_PGRP_ONLY: u32 = 2;

pub(super) fn job(_shell: u32, group: u32) -> Option<Job> {
    let processes: Vec<Process> =
        group_pids(group).into_iter().filter_map(|pid| member(pid, group)).collect();
    (!processes.is_empty()).then_some(Job { group, processes })
}

pub(super) fn leader(group: u32) -> Option<Job> {
    Some(Job { group, processes: vec![member(group, group)?] })
}

pub(super) fn agent_hint(pid: u32) -> Option<String> {
    agent_hint_in(procargs2_env(&kern_procargs2(pid)?)?)
}

/// A process, if it is still in the group: it can leave between the listing and the read.
fn member(pid: u32, group: u32) -> Option<Process> {
    let info = bsdinfo(pid)?;
    if info.pbi_pgid != group {
        return None;
    }
    let name = comm(&info)?;
    let args = kern_procargs2(pid);
    Some(Process {
        pid,
        name,
        argv0: args.as_deref().and_then(procargs2_argv0_name),
        argv: args.as_deref().and_then(procargs2_argv),
    })
}

fn group_pids(group: u32) -> Vec<u32> {
    let mut capacity = 16usize;
    // proc_listpids fills what fits and says how much it wrote, so a full buffer may have
    // been too small: grow and ask again, a bounded number of times.
    for _ in 0..8 {
        let mut pids = vec![0 as libc::pid_t; capacity];
        let Ok(buffer_bytes) = libc::c_int::try_from(size_of_val(pids.as_slice())) else {
            return Vec::new();
        };
        // SAFETY: `pids` is valid for `buffer_bytes` bytes of writes, which is the most
        // proc_listpids writes.
        let returned = unsafe {
            libc::proc_listpids(PROC_PGRP_ONLY, group, pids.as_mut_ptr().cast(), buffer_bytes)
        };
        let Ok(returned) = usize::try_from(returned) else {
            return Vec::new();
        };
        if returned == 0 {
            return Vec::new();
        }
        if returned < size_of_val(pids.as_slice()) {
            let count = returned / size_of::<libc::pid_t>();
            return pids
                .into_iter()
                .take(count)
                .filter_map(|pid| u32::try_from(pid).ok().filter(|&pid| pid > 0))
                .collect();
        }
        capacity = capacity.saturating_mul(2);
    }
    Vec::new()
}

fn bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let size = libc::c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    // SAFETY: `info` is valid for `size` bytes of writes, which is all proc_pidinfo writes.
    let written = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size)
    };
    // SAFETY: proc_pidinfo filled the whole struct when it says it wrote all of it.
    (written == size).then(|| unsafe { info.assume_init() })
}

fn comm(info: &libc::proc_bsdinfo) -> Option<String> {
    let bytes: Vec<u8> = info
        .pbi_comm
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte.cast_unsigned())
        .collect();
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// A process's argument block: `argc`, the executable's path, padding, then argv and the
/// environment, each NUL-terminated. Another user's process refuses this.
fn kern_procargs2(pid: u32) -> Option<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, libc::c_int::try_from(pid).ok()?];
    let mut size: libc::size_t = 0;
    // SAFETY: a null buffer asks sysctl for the size it needs, written through `size`.
    let asked = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if asked != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    // SAFETY: `buffer` is valid for `size` bytes of writes, and sysctl writes at most that
    // and says how much through `size`.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(size);
    Some(buffer)
}

fn procargs2_argc(buffer: &[u8]) -> Option<usize> {
    let argc = i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?);
    usize::try_from(argc).ok().filter(|&argc| argc >= 1)
}

/// Where argv[0] starts: past the executable's path and the NULs that pad it.
fn procargs2_argv_start(rest: &[u8]) -> Option<usize> {
    let exec_end = rest.iter().position(|&byte| byte == 0)?;
    let start = exec_end + rest[exec_end..].iter().take_while(|&&byte| byte == 0).count();
    (start < rest.len()).then_some(start)
}

fn procargs2_argv(buffer: &[u8]) -> Option<Vec<String>> {
    let count = procargs2_argc(buffer)?;
    let rest = &buffer[4..];
    let mut current = procargs2_argv_start(rest)?;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        if current >= rest.len() {
            return None;
        }
        let end = rest[current..]
            .iter()
            .position(|&byte| byte == 0)
            .map_or(rest.len(), |at| current + at);
        if end == current {
            return None;
        }
        args.push(String::from_utf8_lossy(&rest[current..end]).into_owned());
        current = end + 1;
    }
    Some(args)
}

/// argv[0]'s basename, without the dash a login shell puts in front of it.
fn procargs2_argv0_name(buffer: &[u8]) -> Option<String> {
    procargs2_argc(buffer)?;
    let rest = &buffer[4..];
    let start = procargs2_argv_start(rest)?;
    let end = rest[start..].iter().position(|&byte| byte == 0).map_or(rest.len(), |at| start + at);
    let argv0 = std::str::from_utf8(&rest[start..end]).ok()?;
    let basename = Path::new(argv0).file_name()?.to_str()?;
    let name = basename.strip_prefix('-').unwrap_or(basename);
    (!name.is_empty()).then(|| name.to_string())
}

fn procargs2_env(buffer: &[u8]) -> Option<&[u8]> {
    let argc = procargs2_argc(buffer)?;
    let rest = &buffer[4..];
    let mut current = procargs2_argv_start(rest)?;
    for _ in 0..argc {
        let end = rest.get(current..)?.iter().position(|&byte| byte == 0)?;
        current = current.checked_add(end)?.checked_add(1)?;
    }
    rest.get(current..)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procargs2(exec_path: &str, argv: &[&str], env: &[&str]) -> Vec<u8> {
        let mut buffer = i32::try_from(argv.len()).unwrap().to_ne_bytes().to_vec();
        buffer.extend_from_slice(exec_path.as_bytes());
        buffer.extend_from_slice(&[0, 0, 0]);
        for record in argv.iter().chain(env) {
            buffer.extend_from_slice(record.as_bytes());
            buffer.push(0);
        }
        buffer
    }

    #[test]
    fn procargs2_argv_excludes_environment_entries() {
        let buffer = procargs2("/usr/bin/node", &["node", "/tmp/pi"], &["MUSTER_AGENT=pi"]);
        assert_eq!(procargs2_argv(&buffer), Some(vec!["node".to_string(), "/tmp/pi".to_string()]));
    }

    #[test]
    fn procargs2_env_reads_agent_hint_after_argv() {
        let buffer = procargs2("/usr/bin/node", &["node"], &["HOME=/x", "MUSTER_AGENT=codex"]);
        assert_eq!(agent_hint_in(procargs2_env(&buffer).unwrap()).as_deref(), Some("codex"));
    }

    #[test]
    fn procargs2_env_does_not_treat_argv_as_environment() {
        let buffer = procargs2("/usr/bin/env", &["env", "MUSTER_AGENT=claude", "sh"], &["HOME=/x"]);
        assert_eq!(agent_hint_in(procargs2_env(&buffer).unwrap()), None);
    }

    #[test]
    fn procargs2_argv0_name_is_a_basename_without_a_login_dash() {
        assert_eq!(
            procargs2_argv0_name(&procargs2("/bin/zsh", &["-zsh"], &[])).as_deref(),
            Some("zsh")
        );
        assert_eq!(
            procargs2_argv0_name(&procargs2("/x/node", &["/opt/bin/pi", "a"], &[])).as_deref(),
            Some("pi")
        );
    }
}
