use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::{fs, io};

use sha1::digest::Output;
use sha1::{Digest, Sha1};

pub(crate) fn save_request(raw: &[u8], directory: Option<&Path>) -> io::Result<PathBuf> {
    // Hash the content, so we'll have a stable req identifier.
    let hash: Output<Sha1> = Sha1::digest(raw);

    // Use the deterministic content hash as the filename.
    let mut filename: String = String::from("req-");

    for byte in hash {
        write!(&mut filename, "{byte:02x}").map_err(io::Error::other)?;
    }

    filename.push_str(".txt");

    let path: PathBuf = directory.unwrap_or_else(|| Path::new(".")).join(filename);

    fs::write(&path, raw)?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_preserves_binary_bytes_and_uses_stable_filenames() {
        let directory = tempfile::tempdir().unwrap();
        let raw: &[u8] = b"POST / HTTP/1.1\r\n\r\n\xff\x00";

        let first: PathBuf = save_request(raw, Some(directory.path())).unwrap();
        let second: PathBuf = save_request(raw, Some(directory.path())).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(first).unwrap(), raw);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
