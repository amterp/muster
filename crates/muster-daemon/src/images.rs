use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use flexid::{OsRandom, RandomSource};
use muster_core::diagnostics::poison;
use muster_core::input::image::{check_png, quoted_image_path};
use muster_daemon_proto::{ImageStored, answer};

use crate::session::Reply;

const RETENTION: Duration = Duration::from_hours(168);
const CACHE_BYTES: u64 = 1 << 30;

#[derive(Debug)]
pub(crate) struct Images {
    path: PathBuf,
    writing: Mutex<()>,
}

impl Images {
    pub(crate) fn beside(socket: &Path) -> Images {
        Images { path: socket.with_extension("images"), writing: Mutex::new(()) }
    }

    pub(crate) fn store(&self, png: &[u8]) -> Reply {
        match self.store_at(png, SystemTime::now()) {
            Ok(path) => Reply {
                detail: Some(Box::new(answer::Detail::ImageStored(ImageStored { path }))),
                ..Reply::done()
            },
            Err(why) => Reply::refused(format!("{why}; nothing was pasted")),
        }
    }

    fn store_at(&self, png: &[u8], now: SystemTime) -> Result<String, String> {
        self.store_with(png, now, Write::write_all)
    }

    fn store_with(
        &self,
        png: &[u8],
        now: SystemTime,
        write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
    ) -> Result<String, String> {
        check_png(png)?;
        let _writing = poison::lock(&self.writing, "image cache");
        self.directory().map_err(|error| format!("cannot open the image cache: {error}"))?;
        let _lock = self.lock().map_err(|error| format!("cannot lock the image cache: {error}"))?;
        let used =
            self.clean_at(now).map_err(|error| format!("cannot clean the image cache: {error}"))?;
        if used.saturating_add(png.len() as u64) > CACHE_BYTES {
            return Err("the image cache has reached its 1 GiB limit".into());
        }
        let mut random = [0; 16];
        OsRandom
            .fill_bytes(&mut random)
            .map_err(|error| format!("cannot name an image: {error}"))?;
        let path = self.path.join(format!("image-{:032x}.png", u128::from_le_bytes(random)));
        let name = path.to_str().ok_or("the image cache path is not UTF-8")?.to_string();
        quoted_image_path(&name)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|error| format!("cannot create the image file: {error}"))?;
        let written = write(&mut file, png);
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&path);
            return Err(format!("cannot save the image: {error}"));
        }
        Ok(name)
    }

    fn directory(&self) -> std::io::Result<()> {
        match DirBuilder::new().mode(0o700).create(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(&self.path)?;
        // SAFETY: geteuid has no arguments or preconditions.
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_dir()
            || metadata.uid() != owner
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the image cache must be a private directory owned by this user",
            ));
        }
        Ok(())
    }

    fn lock(&self) -> std::io::Result<fs::File> {
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.path.join(".lock"))?;
        // Both daemons can still be serving uploads during a live replacement.
        loop {
            // SAFETY: lock owns this descriptor for the whole call; flock borrows it.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(lock);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn clean_at(&self, now: SystemTime) -> std::io::Result<u64> {
        let mut used = 0u64;
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            if !generated(&entry.file_name().to_string_lossy()) || !entry.file_type()?.is_file() {
                continue;
            }
            let metadata = entry.metadata()?;
            if now.duration_since(metadata.modified()?).unwrap_or_default() > RETENTION {
                fs::remove_file(entry.path())?;
            } else {
                used = used.saturating_add(metadata.len());
            }
        }
        Ok(used)
    }
}

fn generated(name: &str) -> bool {
    name.strip_prefix("image-")
        .and_then(|name| name.strip_suffix(".png"))
        .is_some_and(|name| name.len() == 32 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::FileTimes;
    use std::os::unix::fs::symlink;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage bytes";

    struct Cache(Images);

    impl Cache {
        fn new() -> Cache {
            let mut random = [0; 16];
            OsRandom.fill_bytes(&mut random).unwrap();
            let path = std::env::temp_dir()
                .join(format!("muster-images-test-{:032x}", u128::from_le_bytes(random)));
            Cache(Images { path, writing: Mutex::new(()) })
        }
    }

    impl Drop for Cache {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.path);
            let _ = fs::remove_file(&self.0.path);
        }
    }

    #[test]
    fn images_are_private_complete_files_with_distinct_names() {
        let cache = Cache::new();
        let first = cache.0.store_at(PNG, SystemTime::now()).unwrap();
        let second = cache.0.store_at(PNG, SystemTime::now()).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), PNG);
        assert_eq!(fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&cache.0.path).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn cleanup_removes_only_expired_generated_regular_files() {
        let cache = Cache::new();
        let now = SystemTime::now();
        let old = PathBuf::from(cache.0.store_at(PNG, now).unwrap());
        let fresh = PathBuf::from(cache.0.store_at(PNG, now).unwrap());
        let past = now - RETENTION - Duration::from_secs(1);
        fs::File::open(&old).unwrap().set_times(FileTimes::new().set_modified(past)).unwrap();
        let unrelated = cache.0.path.join("keep.png");
        fs::write(&unrelated, PNG).unwrap();
        fs::File::open(&unrelated).unwrap().set_times(FileTimes::new().set_modified(past)).unwrap();
        let link = cache.0.path.join("image-00000000000000000000000000000000.png");
        symlink(&old, &link).unwrap();
        cache.0.store_at(PNG, now).unwrap();
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(unrelated.exists());
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    }

    #[test]
    fn a_full_cache_refuses_a_new_image_without_deleting_young_files() {
        let cache = Cache::new();
        let now = SystemTime::now();
        let existing = cache.0.store_at(PNG, now).unwrap();
        OpenOptions::new().write(true).open(&existing).unwrap().set_len(CACHE_BYTES).unwrap();
        assert!(cache.0.store_at(PNG, now).unwrap_err().contains("1 GiB"));
        assert_eq!(fs::metadata(existing).unwrap().len(), CACHE_BYTES);
    }

    #[test]
    fn a_file_or_symlink_cannot_become_the_image_cache() {
        let cache = Cache::new();
        fs::write(&cache.0.path, PNG).unwrap();
        assert!(cache.0.store_at(PNG, SystemTime::now()).is_err());
        assert_eq!(fs::read(&cache.0.path).unwrap(), PNG);
        fs::remove_file(&cache.0.path).unwrap();
        symlink(std::env::temp_dir(), &cache.0.path).unwrap();
        assert!(cache.0.store_at(PNG, SystemTime::now()).is_err());
        fs::remove_file(&cache.0.path).unwrap();
    }

    #[test]
    fn a_partial_write_removes_its_file_and_preserves_earlier_images() {
        let cache = Cache::new();
        let now = SystemTime::now();
        let earlier = cache.0.store_at(PNG, now).unwrap();
        let failed = cache.0.store_with(PNG, now, |file, bytes| {
            file.write_all(&bytes[..8])?;
            Err(std::io::Error::other("the disk stopped accepting bytes"))
        });
        assert!(failed.unwrap_err().contains("cannot save"));
        assert_eq!(fs::read(earlier).unwrap(), PNG);
        assert_eq!(
            fs::read_dir(&cache.0.path)
                .unwrap()
                .filter(|entry| {
                    generated(&entry.as_ref().unwrap().file_name().to_string_lossy())
                })
                .count(),
            1
        );
    }
}
