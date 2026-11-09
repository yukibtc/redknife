use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::{fs, io};

use super::process::ProcessInfo;

/// Scanner of the process table with deduplication of already reported processes.
#[derive(Debug, Default)]
pub struct ProcessScanner {
    seen: HashSet<u32>,
}

impl ProcessScanner {
    /// Scan `/proc` and return the processes never returned before,
    /// sorted by start time.
    #[inline]
    pub fn scan(&mut self) -> io::Result<Vec<ProcessInfo>> {
        self.scan_dir("/proc")
    }

    fn scan_dir<P>(&mut self, proc_dir: P) -> io::Result<Vec<ProcessInfo>>
    where
        P: AsRef<Path>,
    {
        let proc_dir: &Path = proc_dir.as_ref();

        let mut new: Vec<ProcessInfo> = Vec::new();
        let mut current: HashSet<u32> = HashSet::new();

        for entry in fs::read_dir(proc_dir)? {
            let Ok(entry) = entry else { continue };

            let filename: OsString = entry.file_name();
            let Some(filename) = filename.to_str() else {
                continue;
            };

            // Parse the pid as u32.
            // Skips all the other non-u32 paths
            let Ok(pid) = filename.parse::<u32>() else {
                continue;
            };

            current.insert(pid);

            if self.seen.insert(pid) {
                match ProcessInfo::read_from(proc_dir, pid) {
                    Ok(Some(process)) => new.push(process),
                    // The process vanished while reading
                    Ok(None) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
            }
        }

        // Forget PIDs that disappeared, so reused PIDs are reported again
        self.seen.retain(|pid| current.contains(pid));

        new.sort_by_key(|process| (process.start_ticks, process.pid));

        Ok(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a fake procfs process directory with the given start time.
    fn write_fake_process(dir: &Path, pid: u32, start_ticks: u64) {
        let pid_dir = dir.join(pid.to_string());

        fs::create_dir_all(&pid_dir).unwrap();

        // The command name may contain spaces and parentheses
        fs::write(
            pid_dir.join("stat"),
            format!(
                "{pid} (sh (test)) S 1 123 123 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 {start_ticks} 11264 2048 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0\n"
            ),
        )
            .unwrap();

        fs::write(
            pid_dir.join("status"),
            "Name:\tsh\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n",
        )
        .unwrap();

        fs::write(pid_dir.join("cmdline"), "/bin/sh\0-c\0test\0").unwrap();
    }

    #[test]
    fn scan_reports_current_processes_once() {
        let mut scanner: ProcessScanner = ProcessScanner::default();

        let first: Vec<ProcessInfo> = scanner.scan().unwrap();
        let second: Vec<ProcessInfo> = scanner.scan().unwrap();

        assert!(!first.is_empty(), "the process table cannot be empty");

        assert!(
            first
                .iter()
                .any(|process| process.pid == std::process::id()),
            "the current process must be in the process table"
        );

        // Only processes started between the two scans are reported again
        for process in &second {
            assert!(
                !first.iter().any(|old| old.pid == process.pid),
                "process {} reported twice",
                process.pid
            );
        }
    }

    #[test]
    fn scan_forgets_vanished_processes_and_reports_reused_pids() {
        let dir = tempfile::tempdir().unwrap();

        let mut scanner: ProcessScanner = ProcessScanner::default();

        // A process is reported once
        write_fake_process(dir.path(), 123, 5000);

        let processes: Vec<ProcessInfo> = scanner.scan_dir(dir.path()).unwrap();

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].pid, 123);
        assert_eq!(processes[0].start_ticks, 5000);
        assert_eq!(processes[0].cmdline, "/bin/sh -c test");

        assert!(scanner.scan_dir(dir.path()).unwrap().is_empty());

        // After the process dies, the PID is forgotten
        fs::remove_dir_all(dir.path().join("123")).unwrap();

        assert!(scanner.scan_dir(dir.path()).unwrap().is_empty());

        // A reused PID with a new start time is reported again
        write_fake_process(dir.path(), 123, 9000);

        let processes: Vec<ProcessInfo> = scanner.scan_dir(dir.path()).unwrap();

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].start_ticks, 9000);
    }
}
