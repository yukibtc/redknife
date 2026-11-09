use std::env;
use std::io::{self, IsTerminal, Write};
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Local;
use clap::Subcommand;

mod monitor;

use crate::proc::ProcessInfo;

/// Bold colors used to distinguish UIDs.
const UID_COLORS: [u8; 6] = [31, 32, 33, 34, 35, 36]; // red, green, yellow, blue, purple, teal
const COLOR_RESET: &str = "\x1b[0m";

const DEFAULT_RECURSIVE_DIRS: &str = "/tmp,/etc,/opt,/var,/usr,/home";

#[derive(Debug, Subcommand)]
pub(super) enum ProcCommand {
    /// Monitor process creation without root permissions
    Monitor {
        /// Milliseconds between periodic scans of /proc
        #[arg(long, default_value_t = NonZeroU64::new(100).unwrap())]
        interval: NonZeroU64,
        /// Directory to watch as trigger for process scans (non-recursive, repeatable)
        #[arg(short = 'd', long)]
        dirs: Vec<PathBuf>,
        /// Directory to watch recursively as trigger for process scans (repeatable)
        #[arg(short = 'r', long, default_value = DEFAULT_RECURSIVE_DIRS, value_delimiter = ',')]
        recursive_dirs: Vec<PathBuf>,
        /// Ignore processes whose command line contains this substring (repeatable)
        #[arg(long)]
        exclude: Vec<String>,
        /// Stop after this many seconds
        #[arg(long)]
        duration: Option<NonZeroU64>,
        /// Write events to stdout
        #[arg(long)]
        stdout: bool,
    },
}

impl ProcCommand {
    pub(super) async fn run(self) -> io::Result<()> {
        match self {
            Self::Monitor {
                interval,
                dirs,
                recursive_dirs,
                exclude,
                duration,
                stdout,
            } => {
                monitor::run(
                    interval,
                    dirs,
                    recursive_dirs,
                    exclude,
                    duration.map(|duration| Duration::from_secs(duration.get())),
                    stdout,
                )
                .await?
            }
        }

        Ok(())
    }
}

fn print_process(process: &ProcessInfo, stdout: bool) -> io::Result<()> {
    let timestamp = Local::now().format("%Y/%m/%d %H:%M:%S");
    let event: String = format!(
        "CMD: UID={:<5} PID={:<7} PPID={:<7} | {}",
        process.uid,
        process.pid,
        process.ppid,
        process.display_cmdline(),
    );

    let event: String = if colored(stdout) {
        format!("\x1b[{};1m{event}{COLOR_RESET}", uid_color(process.uid))
    } else {
        event
    };

    if stdout {
        writeln!(io::stdout().lock(), "{timestamp} {event}")
    } else {
        writeln!(io::stderr().lock(), "{timestamp} {event}")
    }
}

/// Colors are enabled when the target stream is a terminal and NO_COLOR is unset.
fn colored(stdout: bool) -> bool {
    env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && if stdout {
            io::stdout().is_terminal()
        } else {
            io::stderr().is_terminal()
        }
}

/// Same color for the same UID (FNV-1a mod palette).
fn uid_color(uid: u32) -> u8 {
    // FNV-1a over the decimal digits of the UID
    let mut hash: usize = 0x811c_9dc5;

    for byte in uid.to_be_bytes() {
        hash ^= usize::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }

    UID_COLORS[hash % UID_COLORS.len()]
}
