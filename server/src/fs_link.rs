use std::io;
use std::path::Path;

/// Creates a directory symlink (or Windows directory junction when symlink privileges
/// are unavailable) at `link` pointing to `target`.
pub fn link_dir(target: &Path, link: &Path) -> io::Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_dir(target, link) {
            Ok(()) => Ok(()),
            Err(err)
                if err.kind() == io::ErrorKind::PermissionDenied
                    || err.raw_os_error() == Some(1314) =>
            {
                let status = std::process::Command::new("cmd")
                    .arg("/C")
                    .arg("mklink")
                    .arg("/J")
                    .arg(link)
                    .arg(target)
                    .output()?;
                if status.status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "mklink /J failed: {}",
                        String::from_utf8_lossy(&status.stderr)
                    )))
                }
            }
            Err(err) => Err(err),
        }
    }
}

/// Removes a directory symlink (Unix) or directory junction/symlink (Windows)
/// without deleting the contents of its target directory.
pub fn unlink_dir(link: &Path) -> io::Result<()> {
    let meta = match std::fs::symlink_metadata(link) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    #[cfg(unix)]
    {
        if meta.file_type().is_symlink() {
            std::fs::remove_file(link)
        } else {
            std::fs::remove_dir(link)
        }
    }

    #[cfg(windows)]
    {
        let _ = meta;
        std::fs::remove_dir(link)
    }
}
