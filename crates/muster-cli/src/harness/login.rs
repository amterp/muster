//! GUI launches lack the PATH and CODEX_HOME a person's login profile supplies.

use std::collections::BTreeMap;
use std::process::Command;

use crate::Trouble;

use super::{REFRESH_PATIENCE, run_within};

const MARKER: &[u8] = b"\0MUSTER_REFRESH_ENV\0";

/// Reads the user's login environment without allowing a stalled profile to hold a refresh open.
pub fn environment(
    inherited: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, Trouble> {
    let shell = inherited
        .get("SHELL")
        .filter(|shell| !shell.is_empty())
        .cloned()
        .or_else(account_shell)
        .unwrap_or_else(|| "/bin/sh".to_string());
    let mut command = Command::new(&shell);
    // The marker separates environment records from anything the login profile printed.
    command
        .args(["-l", "-i", "-c", "printf '\\000MUSTER_REFRESH_ENV\\000'; exec /usr/bin/env -0"])
        .env_clear()
        .envs(inherited);
    let output = run_within(&mut command, REFRESH_PATIENCE)
        .map_err(|error| failed(&shell, &error.to_string()))?
        .ok_or_else(|| failed(&shell, "it timed out after 10 seconds and was stopped"))?;
    if !output.status.success() {
        return Err(failed(
            &shell,
            &format!("{}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim()),
        ));
    }
    let at = output
        .stdout
        .windows(MARKER.len())
        .position(|part| part == MARKER)
        .ok_or_else(|| failed(&shell, "it printed no environment records"))?;
    let mut environment = BTreeMap::new();
    for pair in
        output.stdout[at + MARKER.len()..].split(|byte| *byte == 0).filter(|pair| !pair.is_empty())
    {
        let pair = std::str::from_utf8(pair).map_err(|error| failed(&shell, &error.to_string()))?;
        let Some((key, value)) = pair.split_once('=') else {
            return Err(failed(&shell, "it printed a malformed environment record"));
        };
        environment.insert(key.to_string(), value.to_string());
    }
    Ok(environment)
}

fn failed(shell: &str, reason: &str) -> Trouble {
    Trouble::Refused(format!(
        "could not read the login environment from {shell} ({reason}), so Codex's adapter was \
         not refreshed. Run `muster harness install codex` from a terminal and review changed \
         hooks in `/hooks`."
    ))
}

fn account_shell() -> Option<String> {
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0 as libc::c_char; 4096];
    let mut found = std::ptr::null_mut();
    // SAFETY: entry and buffer are writable and the buffer length matches the supplied allocation.
    let status = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &raw mut found,
        )
    };
    if status != 0 || found.is_null() {
        return None;
    }
    // SAFETY: getpwuid_r filled entry and its strings point into the live buffer.
    let shell = unsafe { std::ffi::CStr::from_ptr((*found).pw_shell) };
    shell.to_str().ok().filter(|shell| !shell.is_empty()).map(str::to_string)
}
