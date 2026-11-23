//! One recorder per data directory. Keep the descriptor open for the app lifetime.
use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::Path,
};

pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(directory: &Path) -> io::Result<Option<Self>> {
        std::fs::create_dir_all(directory)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join(".batterybar-recorder.lock"))?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Recorder lock is not a regular file",
            ));
        }
        // SAFETY: the owned file descriptor stays open in InstanceLock.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(error)
            };
        }
        Ok(Some(Self { _file: file }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "batterybar-instance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    #[test]
    fn rejects_duplicate_and_releases_on_exit() {
        let dir = directory();
        let first = InstanceLock::acquire(&dir).unwrap().unwrap();
        assert!(InstanceLock::acquire(&dir).unwrap().is_none());
        drop(first);
        let next = InstanceLock::acquire(&dir).unwrap().unwrap();
        drop(next);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn isolated_directories_do_not_conflict() {
        let a = directory();
        let b = directory();
        let first = InstanceLock::acquire(&a).unwrap().unwrap();
        let second = InstanceLock::acquire(&b).unwrap().unwrap();
        drop((first, second));
        std::fs::remove_dir_all(a).unwrap();
        std::fs::remove_dir_all(b).unwrap();
    }
    #[test]
    fn refuses_a_symlink_without_changing_the_target() {
        let dir = directory();
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("original");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, dir.join(".batterybar-recorder.lock")).unwrap();
        assert!(InstanceLock::acquire(&dir).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
