//! What this app installs on another machine, and how it gets there.
//!
//! The app carries a daemon for every machine it can attach: its own arm64 daemon for a remote
//! Mac, and a static Linux build for each of the two Linux architectures (MIP-3, section 12).
//! Nothing is downloaded, so there is no pin and no checksum file, and a machine with no
//! internet access can still be installed to.
//!
//! One install is one archive on one ssh round trip: the daemon, the data directory its shells
//! are given, and on a Mac the libghostty-vt it links. The archive is built the same way every
//! time from the same files, so its digest says whether what a machine has is what this app
//! would send, and a machine that has it is not sent it again.

use std::io;
use std::path::{Path, PathBuf};

use muster_ssh::Platform;
use sha2::{Digest, Sha256};

/// Where the daemons this app can install are, as the shell found them.
#[derive(Debug, Clone, Default)]
pub struct Carried {
    /// A directory holding `linux-x86_64/muster-daemon` and `linux-aarch64/muster-daemon`.
    pub linux: Option<PathBuf>,
    /// This machine's own daemon, which is what a remote Mac runs.
    pub mac: Option<PathBuf>,
    /// The libghostty-vt `mac` links, which has to travel with it.
    pub mac_library: Option<PathBuf>,
    /// The data directory every daemon gives its shells, the same on every platform.
    pub data: Option<PathBuf>,
}

/// One machine's install, ready to send.
#[derive(Debug)]
pub struct Payload {
    /// Which build this is, for a log line: `linux-x86_64`, `macos-aarch64`.
    pub build: &'static str,
    pub archive: Vec<u8>,
    /// The archive's SHA-256, which the machine keeps beside the daemon once it is installed.
    pub stamp: String,
}

impl Carried {
    /// What a machine that answered `uname -sm` with `platform` is sent.
    pub fn payload(&self, host: &str, platform: &Platform) -> Result<Payload, String> {
        let build = build_for(platform).ok_or_else(|| {
            format!(
                "{host} is {} {}, and Muster carries a daemon only for Linux on x86_64 or \
                 aarch64 and for macOS on Apple silicon, so that machine's panes are absent \
                 from the window and nothing else is affected. A daemon started there by hand \
                 is reached by naming its socket in the config file's `socket` key.",
                platform.system, platform.machine
            )
        })?;
        let missing = |what: &str| {
            format!(
                "this app carries no {what} to install on {host}, so that machine's panes are \
                 absent from the window. A build stages it beside the app and a bundle carries \
                 it inside; this is a bug in how the app was built or started."
            )
        };
        let data = self.data.as_deref().ok_or_else(|| missing("data directory"))?;
        let (daemon, library) = match build {
            "macos-aarch64" => (
                self.mac.clone().ok_or_else(|| missing("daemon for macOS"))?,
                Some(self.mac_library.clone().ok_or_else(|| missing("libghostty-vt"))?),
            ),
            linux => (
                self.linux
                    .as_deref()
                    .map(|directory| directory.join(linux).join("muster-daemon"))
                    .filter(|daemon| daemon.is_file())
                    .ok_or_else(|| missing(&format!("daemon for {linux}")))?,
                None,
            ),
        };
        let archive = archive(&daemon, library.as_deref(), data).map_err(|error| {
            format!(
                "could not pack the daemon for {host} from {} ({error}), so that machine's \
                 panes are absent from the window. Check that the app's files are readable.",
                daemon.display()
            )
        })?;
        let stamp = format!("{:x}", Sha256::digest(&archive));
        Ok(Payload { build, archive, stamp })
    }
}

/// Which of the daemons this app carries a machine runs, by what `uname -sm` said.
fn build_for(platform: &Platform) -> Option<&'static str> {
    match (platform.system.as_str(), platform.machine.as_str()) {
        ("Linux", "x86_64" | "amd64") => Some("linux-x86_64"),
        ("Linux", "aarch64" | "arm64") => Some("linux-aarch64"),
        ("Darwin", "arm64") => Some("macos-aarch64"),
        _ => None,
    }
}

/// The install as a ustar archive, laid out as the machine keeps it: `muster-daemon`, on a Mac
/// `libghostty-vt.dylib` beside it, and `muster-daemon-data/`.
///
/// The same bytes for the same files: entries in name order, and no owner, group or time of
/// this machine's in any header, so the digest changes only when something sent does.
fn archive(daemon: &Path, library: Option<&Path>, data: &Path) -> io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    add_file(&mut builder, daemon, "muster-daemon", 0o755)?;
    if let Some(library) = library {
        add_file(&mut builder, library, "libghostty-vt.dylib", 0o644)?;
    }
    add_directory(&mut builder, data, Path::new("muster-daemon-data"))?;
    builder.into_inner()
}

fn add_directory(builder: &mut tar::Builder<Vec<u8>>, from: &Path, name: &Path) -> io::Result<()> {
    let mut header = header(0o755);
    header.set_entry_type(tar::EntryType::Directory);
    header.set_size(0);
    builder.append_data(&mut header, name, io::empty())?;
    let mut entries: Vec<_> = std::fs::read_dir(from)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let within = name.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            add_directory(builder, &entry.path(), &within)?;
        } else {
            add_file(builder, &entry.path(), &within.to_string_lossy(), 0o644)?;
        }
    }
    Ok(())
}

fn add_file(
    builder: &mut tar::Builder<Vec<u8>>,
    from: &Path,
    name: &str,
    mode: u32,
) -> io::Result<()> {
    let bytes = std::fs::read(from)?;
    let mut header = header(mode);
    header.set_size(bytes.len() as u64);
    builder.append_data(&mut header, name, bytes.as_slice())
}

fn header(mode: u32) -> tar::Header {
    let mut header = tar::Header::new_ustar();
    header.set_mode(mode);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform(said: &str) -> Platform {
        Platform::from_uname(said).unwrap()
    }

    /// `uname -m` spells one machine two ways, depending on who built the kernel.
    #[test]
    fn a_machine_gets_the_build_for_it_and_nothing_else() {
        assert_eq!(build_for(&platform("Linux x86_64")), Some("linux-x86_64"));
        assert_eq!(build_for(&platform("Linux amd64")), Some("linux-x86_64"));
        assert_eq!(build_for(&platform("Linux aarch64")), Some("linux-aarch64"));
        assert_eq!(build_for(&platform("Linux arm64")), Some("linux-aarch64"));
        assert_eq!(build_for(&platform("Darwin arm64")), Some("macos-aarch64"));
        assert_eq!(build_for(&platform("Darwin x86_64")), None, "no Intel Mac build ships");
        assert_eq!(build_for(&platform("FreeBSD amd64")), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let root = PathBuf::from(format!("/tmp/muster-test/i{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("linux/linux-x86_64")).unwrap();
        std::fs::create_dir_all(root.join("data/terminfo/x")).unwrap();
        std::fs::write(root.join("linux/linux-x86_64/muster-daemon"), b"\x7fELF").unwrap();
        std::fs::write(root.join("data/terminfo/x/xterm-ghostty"), b"entry").unwrap();
        std::fs::write(root.join("data/README.md"), b"readme").unwrap();
        root
    }

    /// The digest is the whole test of "is it installed", so the same files must always pack to
    /// the same bytes, and the archive holds the layout the far machine keeps.
    #[test]
    fn the_same_files_pack_to_the_same_stamp_in_the_layout_a_machine_keeps() {
        let root = scratch("pack");
        let carried = Carried {
            linux: Some(root.join("linux")),
            data: Some(root.join("data")),
            ..Carried::default()
        };
        let first = carried.payload("box", &platform("Linux x86_64")).unwrap();
        let second = carried.payload("box", &platform("Linux x86_64")).unwrap();
        assert_eq!(first.stamp, second.stamp);
        assert_eq!(first.build, "linux-x86_64");

        let mut archive = tar::Archive::new(first.archive.as_slice());
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "muster-daemon",
                "muster-daemon-data",
                "muster-daemon-data/README.md",
                "muster-daemon-data/terminfo",
                "muster-daemon-data/terminfo/x",
                "muster-daemon-data/terminfo/x/xterm-ghostty",
            ]
        );

        std::fs::write(root.join("data/README.md"), b"changed").unwrap();
        let changed = carried.payload("box", &platform("Linux x86_64")).unwrap();
        assert_ne!(changed.stamp, first.stamp, "a changed file is a different install");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The data carries a script a pane's shell runs directly: Ghostty's `ssh` wrapper calls
    /// `bin/ghostty +ssh`. Packed without its execute bit, every `ssh` typed in a pane over
    /// there is refused.
    #[test]
    fn an_executable_in_the_data_stays_executable() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("modes");
        std::fs::create_dir_all(root.join("data/bin")).unwrap();
        std::fs::write(root.join("data/bin/ghostty"), b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(root.join("data/bin/ghostty"), PermissionsExt::from_mode(0o755))
            .unwrap();
        let carried = Carried {
            linux: Some(root.join("linux")),
            data: Some(root.join("data")),
            ..Carried::default()
        };
        let payload = carried.payload("box", &platform("Linux x86_64")).unwrap();

        let mut archive = tar::Archive::new(payload.archive.as_slice());
        let modes: Vec<(String, u32)> = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let name = entry.path().unwrap().to_string_lossy().into_owned();
                (name, entry.header().mode().unwrap())
            })
            .filter(|(name, _)| name.ends_with("bin/ghostty") || name.ends_with("README.md"))
            .collect();
        assert_eq!(
            modes,
            [
                ("muster-daemon-data/README.md".to_string(), 0o644),
                ("muster-daemon-data/bin/ghostty".to_string(), 0o755),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_machine_nothing_is_carried_for_is_named_in_the_refusal() {
        let root = scratch("refuse");
        let carried = Carried {
            linux: Some(root.join("linux")),
            data: Some(root.join("data")),
            ..Carried::default()
        };
        let intel = carried.payload("old-mac", &platform("Darwin x86_64")).unwrap_err();
        assert!(intel.contains("old-mac is Darwin x86_64"), "{intel}");
        let arm = carried.payload("pi", &platform("Linux aarch64")).unwrap_err();
        assert!(arm.contains("daemon for linux-aarch64"), "{arm}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
