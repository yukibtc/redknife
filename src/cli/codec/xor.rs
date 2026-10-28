use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::cli::bytes_from_file_or_stdin;

#[inline]
fn xor(data: &mut [u8], key: &[u8]) {
    for (d, k) in data.iter_mut().zip(key.iter().cycle()) {
        *d ^= k;
    }
}

fn select_key(key: Option<String>, key_file: Option<PathBuf>) -> io::Result<Vec<u8>> {
    if let Some(key) = key {
        return Ok(key.into_bytes());
    }

    if let Some(key_file) = key_file {
        let mut bytes: Vec<u8> = Vec::new();
        let mut file = File::open(key_file)?;
        file.read_to_end(&mut bytes)?;
        return Ok(bytes);
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "no key provided",
    ))
}

pub(super) fn run(file: PathBuf, key: Option<String>, key_file: Option<PathBuf>) -> io::Result<()> {
    let mut data: Vec<u8> = bytes_from_file_or_stdin(file)?;

    let key: Vec<u8> = select_key(key, key_file)?;

    if key.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "key must not be empty",
        ));
    }

    xor(&mut data, &key);

    io::stdout().write_all(&data)?;

    Ok(())
}
