use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// A snapshot of a running process, read from `/proc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    /// Process ID
    pub pid: u32,
    /// Parent process ID
    pub ppid: u32,
    /// Real user ID
    pub uid: u32,
    /// Real group ID
    pub gid: u32,
    /// Start time of the process in clock ticks since boot
    pub start_ticks: u64,
    /// Command name, truncated to 15 characters by the kernel
    pub cmd: String,
    /// Command line with arguments joined by spaces (empty for kernel threads and zombies)
    pub cmdline: String,
}

impl ProcessInfo {
    /// Read the snapshot of the process from the given procfs directory.
    ///
    /// Returns `None` when the process vanished while reading.
    pub(super) fn read_from<P>(proc_dir: P, pid: u32) -> io::Result<Option<Self>>
    where
        P: AsRef<Path>,
    {
        let proc_dir: &Path = proc_dir.as_ref();
        let dir: PathBuf = proc_dir.join(pid.to_string());

        let Some(stat) = read_vanished(dir.join("stat"))? else {
            return Ok(None);
        };

        let (ppid, start_ticks, cmd) = parse_stat(&stat)?;

        let Some(status) = read_vanished(dir.join("status"))? else {
            return Ok(None);
        };

        let (uid, gid) = parse_status_ids(&status)?;

        let cmdline: String = read_vanished(dir.join("cmdline"))?
            .map(|raw| {
                raw.split('\0')
                    .filter(|arg| !arg.is_empty())
                    .collect::<Vec<&str>>()
                    .join(" ")
            })
            .unwrap_or_default();

        Ok(Some(Self {
            pid,
            ppid,
            uid,
            gid,
            start_ticks,
            cmd,
            cmdline,
        }))
    }

    /// Command line formatted for display: kernel threads and zombies,
    /// which have an empty command line, are shown as `[cmd]`.
    #[inline]
    pub fn display_cmdline(&self) -> Cow<'_, str> {
        if self.cmdline.is_empty() {
            Cow::Owned(format!("[{}]", self.cmd))
        } else {
            Cow::Borrowed(&self.cmdline)
        }
    }
}

/// Read a procfs file, returning `None` if it vanished (the process died meanwhile).
#[inline]
fn read_vanished(path: PathBuf) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Parse `/proc/<pid>/stat` into `(ppid, start_ticks, comm)`.
///
/// The command name may contain spaces and parentheses, so everything
/// between the first `(` and the last `)` belongs to it.
fn parse_stat(stat: &str) -> io::Result<(u32, u64, String)> {
    let Some(start) = stat.find('(') else {
        return Err(invalid_data("stat: missing command name"));
    };

    let Some(end) = stat.rfind(')') else {
        return Err(invalid_data("stat: missing command name"));
    };

    let comm: String = stat[start + 1..end].to_string();

    // The fields after the command name are numbered from 3 (state), so
    // the field at index `n - 3` has number `n`: start time is number 22.
    let fields: Vec<&str> = stat[end + 1..].split_ascii_whitespace().collect();

    if fields.len() < 20 {
        return Err(invalid_data("stat: too few fields"));
    }

    let ppid: u32 = fields[1]
        .parse()
        .map_err(|_| invalid_data("stat: invalid parent process ID"))?;

    let start_ticks: u64 = fields[19]
        .parse()
        .map_err(|_| invalid_data("stat: invalid start time"))?;

    Ok((ppid, start_ticks, comm))
}

/// Parse the real UID and GID from `/proc/<pid>/status`.
fn parse_status_ids(status: &str) -> io::Result<(u32, u32)> {
    let mut uid: Option<u32> = None;
    let mut gid: Option<u32> = None;

    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            uid = parse_first_id(rest);
        } else if let Some(rest) = line.strip_prefix("Gid:") {
            gid = parse_first_id(rest);
        }

        if uid.is_some() && gid.is_some() {
            break;
        }
    }

    match (uid, gid) {
        (Some(uid), Some(gid)) => Ok((uid, gid)),
        _ => Err(invalid_data("status: missing user or group ID")),
    }
}

#[inline]
fn parse_first_id(rest: &str) -> Option<u32> {
    rest.split_ascii_whitespace().next()?.parse().ok()
}

#[inline]
fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stat_extracts_fields() {
        // The command name contains spaces and parentheses
        let stat = "1234 (echo (hello) world) S 1000 1234 1234 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 987654 11264 2048 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0\n";

        let (ppid, start_ticks, comm) = parse_stat(stat).unwrap();

        assert_eq!(ppid, 1000);
        assert_eq!(start_ticks, 987654);
        assert_eq!(comm, "echo (hello) world");
    }

    #[test]
    fn parse_stat_rejects_missing_command_name() {
        let stat = "1234 S 1000 1234";

        assert_eq!(
            parse_stat(stat).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn parse_stat_rejects_too_few_fields() {
        let stat = "1234 (sh) S 1000 1234";

        assert_eq!(
            parse_stat(stat).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn parse_status_ids_extracts_real_ids() {
        let status = "Name:\tsh\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n";

        let (uid, gid) = parse_status_ids(status).unwrap();

        assert_eq!(uid, 1000);
        assert_eq!(gid, 1000);
    }

    #[test]
    fn parse_status_ids_rejects_missing_ids() {
        let status = "Name:\tsh\nUid:\t1000\t1000\t1000\t1000\n";

        assert_eq!(
            parse_status_ids(status).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn read_current_process() {
        let process: ProcessInfo = ProcessInfo::read_from("/proc", std::process::id())
            .unwrap()
            .unwrap();

        assert_eq!(process.pid, std::process::id());
        assert!(!process.cmd.is_empty());
    }

    #[test]
    fn read_missing_process() {
        // PID 2^22 and above are out of the default pid_max range
        let process = ProcessInfo::read_from("/proc", 2u32.pow(22)).unwrap();

        assert!(process.is_none());
    }
}
