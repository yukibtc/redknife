#[cfg(unix)]
use std::fs::Permissions;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tokio::fs;

/// Create a directory with owner-only permissions (best-effort on non-unix).
pub async fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir).await?;

    #[cfg(unix)]
    fs::set_permissions(dir, Permissions::from_mode(0o700)).await?;

    Ok(())
}

/// Write bytes with owner-only permissions (best-effort on non-unix).
pub async fn write_secret(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes).await?;

    #[cfg(unix)]
    fs::set_permissions(path, Permissions::from_mode(0o600)).await?;

    Ok(())
}
