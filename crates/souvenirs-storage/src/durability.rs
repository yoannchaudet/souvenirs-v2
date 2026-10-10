use std::fs::File;
use std::io;
use std::path::Path;

pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    sync_with_fallback(
        cfg!(target_vendor = "apple"),
        || file.sync_all(),
        || rustix::fs::fsync(file).map_err(io::Error::from),
    )
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    let directory = File::open(path)?;
    sync_file(&directory)
}

pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent: {}", path.display()),
        )
    })?;
    sync_directory(parent)
}

fn sync_with_fallback(
    full_sync_may_be_unsupported: bool,
    full_sync: impl FnOnce() -> io::Result<()>,
    fallback_sync: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match full_sync() {
        Ok(()) => Ok(()),
        Err(error) if full_sync_may_be_unsupported && is_unsupported_full_sync(&error) => {
            fallback_sync()
        }
        Err(error) => Err(error),
    }
}

fn is_unsupported_full_sync(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(22 | 25 | 45 | 102))
}

#[cfg(test)]
mod tests {
    use super::sync_with_fallback;
    use std::cell::Cell;
    use std::io;

    #[test]
    fn falls_back_when_apple_full_sync_is_unsupported() {
        let fallback_called = Cell::new(false);

        sync_with_fallback(
            true,
            || Err(io::Error::from_raw_os_error(45)),
            || {
                fallback_called.set(true);
                Ok(())
            },
        )
        .unwrap();

        assert!(fallback_called.get());
    }

    #[test]
    fn preserves_other_sync_failures() {
        let error = sync_with_fallback(
            true,
            || Err(io::Error::from_raw_os_error(5)),
            || panic!("fallback must not run for a real I/O failure"),
        )
        .unwrap_err();

        assert_eq!(error.raw_os_error(), Some(5));
    }
}
