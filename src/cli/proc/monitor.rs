use std::collections::HashSet;
use std::mem::{self, Discriminant};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{future, io, pin};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::signal;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::{self, Instant, MissedTickBehavior};

use super::print_process;
use crate::proc::{ProcessInfo, ProcessScanner};

/// Quiet period after a watcher event before triggering a scan.
const QUIET_PERIOD: Duration = Duration::from_millis(25);

pub(super) async fn run(
    interval: NonZeroU64,
    dirs: Vec<PathBuf>,
    recursive_dirs: Vec<PathBuf>,
    exclude: Vec<String>,
    duration: Option<Duration>,
    stdout: bool,
) -> io::Result<()> {
    let (events_tx, mut events) = mpsc::unbounded_channel();

    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
        let _ = events_tx.send(event);
    })
    .map_err(io::Error::other)?;

    // Watching /proc catches processes being created, removed and accessed:
    // every event triggers an immediate scan to catch short-lived processes.
    add_watch(
        &mut watcher,
        Path::new("/proc"),
        RecursiveMode::NonRecursive,
    );

    for dir in &dirs {
        add_watch(&mut watcher, dir, RecursiveMode::NonRecursive);
    }

    for dir in &recursive_dirs {
        add_watch(&mut watcher, dir, RecursiveMode::Recursive);
    }

    let mut scanner: ProcessScanner = ProcessScanner::default();

    // Processes running now are marked as seen
    // and only processes started from now on are reported.
    scanner.scan()?;

    let watched: usize = dirs.len() + recursive_dirs.len();

    eprintln!("\nMonitoring processes every {}ms", interval.get());

    if watched > 0 {
        eprintln!("Watching {watched} trigger directories");
    }

    eprintln!("Press Ctrl+C to stop...\n");

    let mut ticker = time::interval(Duration::from_millis(interval.get()));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut ctrl_c = pin::pin!(signal::ctrl_c());

    let timeout = async {
        match duration {
            Some(duration) => time::sleep_until(Instant::now() + duration).await,
            None => future::pending().await,
        }
    };
    tokio::pin!(timeout);

    let mut watcher_alive: bool = true;
    let mut warned: HashSet<Discriminant<notify::ErrorKind>> = HashSet::new();

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                report(&mut scanner, &exclude, stdout)?;
            }
            event = events.recv(), if watcher_alive => {
                match event {
                    Some(Ok(_)) => {
                        if debounce(&mut events, &mut warned).await {
                            report(&mut scanner, &exclude, stdout)?;
                        } else {
                            watcher_alive = false;
                        }
                    }
                    Some(Err(err)) => warn_watch_error(&mut warned, &err),
                    None => watcher_alive = false,
                }
            }
            _ = &mut ctrl_c => break,
            _ = &mut timeout => break,
        }
    }

    Ok(())
}

#[inline]
fn add_watch(watcher: &mut RecommendedWatcher, path: &Path, mode: RecursiveMode) {
    if let Err(err) = watcher.watch(path, mode) {
        eprintln!("Warning: watch failed: {err}");
    }
}

#[inline]
fn warn_watch_error(warned: &mut HashSet<Discriminant<notify::ErrorKind>>, err: &notify::Error) {
    if warned.insert(mem::discriminant(&err.kind)) {
        eprintln!("Warning: {err}");
    }
}

/// Wait for a quiet period on the event stream, coalescing bursts of
/// events into a single scan. Returns `false` when the watcher stops.
async fn debounce(
    events: &mut UnboundedReceiver<notify::Result<Event>>,
    warned: &mut HashSet<Discriminant<notify::ErrorKind>>,
) -> bool {
    let sleep = time::sleep_until(Instant::now() + QUIET_PERIOD);
    tokio::pin!(sleep);

    loop {
        tokio::select! {
            _ = sleep.as_mut() => return true,
            event = events.recv() => {
                match event {
                    Some(Ok(_)) => sleep.as_mut().set(time::sleep_until(Instant::now() + QUIET_PERIOD)),
                    Some(Err(err)) => warn_watch_error(warned, &err),
                    None => return false,
                }
            }
        }
    }
}

fn report(scanner: &mut ProcessScanner, exclude: &[String], stdout: bool) -> io::Result<()> {
    let processes: Vec<ProcessInfo> = match scanner.scan() {
        Ok(processes) => processes,
        Err(err) => {
            eprintln!("Warning: scan failed: {err}");
            return Ok(());
        }
    };

    for process in processes {
        if exclude
            .iter()
            .any(|needle| process.display_cmdline().contains(needle.as_str()))
        {
            continue;
        }

        print_process(&process, stdout)?;
    }

    Ok(())
}
