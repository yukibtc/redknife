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
use crate::tui::http::editor::{self, ExitLabel};
use crate::tui::terminal::TerminalSession;

const TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run(
    target: HttpTarget,
    file: Option<PathBuf>,
    interactive: bool,
    output: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let open_editor: bool = file.is_none() || interactive;

    if open_editor && output.is_some() {
        return Err(
            io::Error::other("--output requires a request file without --interactive").into(),
        );
    }

    let content: Vec<u8> = match file {
        Some(file) => fs::read(file).await?,
        None => initial_request(&target)?,
    };

    if open_editor {
        let mut terminal: TerminalSession = TerminalSession::new()?;

        editor::repeat(
            terminal.terminal(),
            content,
            &target,
            None,
            ExitLabel::Close,
        )
        .await?;

        return Ok(());
    }

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
            result.map_err(|error| error as Box<dyn Error>)?;
        }
        None => {
            let mut stdout: Stdout = io::stdout();
            let result = send_http(request, &target, TIMEOUT, &mut stdout).await;
            stdout.flush().await?;
            result.map_err(|error| error as Box<dyn Error>)?;
        }
    }

    Ok(())
}

#[inline]
fn initial_request(target: &HttpTarget) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(format!("GET / HTTP/1.1\r\nHost: {}\r\n\r\n", target.host_and_port()).into_bytes())
}
