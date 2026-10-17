use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use http::Request;
use tokio::fs::{self, File};
use tokio::io::{self, AsyncWriteExt, Stdout};

use crate::http::client::send_http;
use crate::http::request::parse_request;
use crate::http::target::HttpTarget;

const TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run(
    target: HttpTarget,
    file: PathBuf,
    output: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let content: Vec<u8> = fs::read(file).await?;

    let request: Request<Bytes> = parse_request(&content)?;

    // Write received bytes immediately, preserving partial responses on failure.
    match output {
        Some(path) => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent).await?;
            }

            let mut file: File = File::create(path).await?;
            let result = send_http(request, &target, TIMEOUT, &mut file).await;
            file.flush().await?;
            result?;
        }
        None => {
            let mut stdout: Stdout = io::stdout();
            let result = send_http(request, &target, TIMEOUT, &mut stdout).await;
            stdout.flush().await?;
            result?;
        }
    }

    Ok(())
}
