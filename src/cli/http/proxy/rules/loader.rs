use std::path::{self, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::{future, io};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::fs;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tokio::time::{self, Instant};

use super::Rules;

const RELOAD_DELAY: Duration = Duration::from_millis(200);

struct RulesWatcher {
    // Keep the watcher alive for the entire reload loop.
    _watcher: RecommendedWatcher,
    path: PathBuf,
    content: String,
    events: mpsc::UnboundedReceiver<notify::Result<Event>>,
    rules: watch::Sender<Arc<Rules>>,
}

impl RulesWatcher {
    async fn new(path: PathBuf) -> io::Result<(Self, watch::Receiver<Arc<Rules>>)> {
        let path: PathBuf = path::absolute(path)?;

        let parent: PathBuf = fs::canonicalize(path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Rules file has no parent directory",
            )
        })?)
        .await?;

        let path: PathBuf = parent.join(path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Rules path has no file name")
        })?);

        let (sender, events) = mpsc::unbounded_channel();

        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })
        .map_err(io::Error::other)?;

        // Watch the directory so replacing the file does not remove the watch.
        // Register before reading, so changes during startup remain queued.
        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .map_err(io::Error::other)?;

        let content: String = fs::read_to_string(&path).await?;
        let initial: Rules = toml::from_str(&content)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        let (rules, receiver) = watch::channel(Arc::new(initial));

        Ok((
            Self {
                _watcher: watcher,
                path,
                content,
                events,
                rules,
            },
            receiver,
        ))
    }

    #[inline]
    fn affects_rules(&self, event: &Event) -> bool {
        !event.kind.is_access()
            && (event.need_rescan() || event.paths.iter().any(|path| path == &self.path))
    }

    async fn should_reload<W>(
        &self,
        event: notify::Result<Event>,
        stderr: &mut W,
    ) -> io::Result<bool>
    where
        W: AsyncWrite + Unpin,
    {
        match event {
            Ok(event) => Ok(self.affects_rules(&event)),
            Err(err) => {
                stderr
                    .write_all(
                        format!("Rules watcher error for {}: {err}\n", self.path.display())
                            .as_bytes(),
                    )
                    .await?;
                stderr.flush().await?;
                // The watcher may have missed a change; try reading the file again.
                Ok(true)
            }
        }
    }

    async fn reload(&mut self) -> io::Result<bool> {
        let content: String = fs::read_to_string(&self.path).await?;

        if content == self.content {
            return Ok(false);
        }

        let rules: Rules = toml::from_str(&content)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        self.rules.send_replace(Arc::new(rules));

        self.content = content;

        Ok(true)
    }

    async fn run<W>(&mut self, stderr: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        loop {
            let event = self.events.recv().await.ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "Rules watcher stopped")
            })?;

            if !self.should_reload(event, stderr).await? {
                continue;
            }

            let delay = time::sleep(RELOAD_DELAY);
            tokio::pin!(delay);

            loop {
                tokio::select! {
                    _ = &mut delay => break,
                    event = self.events.recv() => {
                        let event = event.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "Rules watcher stopped")
                        })?;
                        if self.should_reload(event, stderr).await? {
                            delay.as_mut().reset(Instant::now() + RELOAD_DELAY);
                        }
                    }
                }
            }

            let message: String = match self.reload().await {
                Ok(true) => format!("Rules reloaded from {}\n", self.path.display()),
                Ok(false) => continue,
                Err(err) => format!(
                    "Cannot reload rules from {}: {err}; keeping previous rules\n",
                    self.path.display(),
                ),
            };

            stderr.write_all(message.as_bytes()).await?;
            stderr.flush().await?;
        }
    }
}

enum Inner {
    Watching(RulesWatcher),
    NotWatching,
}

impl Inner {
    #[inline]
    async fn watching(path: PathBuf) -> io::Result<(Self, watch::Receiver<Arc<Rules>>)> {
        let (inner, rx) = RulesWatcher::new(path).await?;
        Ok((Self::Watching(inner), rx))
    }
}

pub(in super::super) struct RulesLoader(Inner);

impl RulesLoader {
    pub(in super::super) async fn new(
        path: Option<PathBuf>,
        watch_rules: bool,
    ) -> io::Result<(Self, watch::Receiver<Arc<Rules>>)> {
        match path {
            Some(path) if watch_rules => {
                let (inner, rx) = Inner::watching(path).await?;
                Ok((Self(inner), rx))
            }
            Some(path) => {
                let content: String = fs::read_to_string(path).await?;
                let rules: Rules = toml::from_str(&content)
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
                let (_, rx) = watch::channel(Arc::new(rules));
                Ok((Self(Inner::NotWatching), rx))
            }
            None => {
                let (_, rx) = watch::channel(Arc::new(Rules::default()));
                Ok((Self(Inner::NotWatching), rx))
            }
        }
    }

    pub(in super::super) async fn run<W>(&mut self, stderr: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let Inner::Watching(watcher) = &mut self.0 else {
            return future::pending().await;
        };

        watcher.run(stderr).await
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::FutureExt;
    use http::{Request, StatusCode, Uri};
    use notify::EventKind;
    use notify::event::{AccessKind, ModifyKind};
    use tokio::io::{AsyncBufReadExt, BufReader};

    use super::*;
    use crate::cli::http::proxy::rules::RuleOutcome;

    const INITIAL: &str = r#"
[[rules]]
name = "initial"
match = {}
deny = { code = 403 }
"#;

    const UPDATED: &str = r#"
[[rules]]
name = "updated"
match = {}
deny = { code = 429 }
"#;

    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    #[tokio::test]
    async fn loader_without_rules() {
        let (mut loader, receiver) = RulesLoader::new(None, false).await.unwrap();

        assert!(matches!(&loader.0, Inner::NotWatching));
        assert!(receiver.borrow().rules.is_empty());

        // Without a watcher, run waits and writes nothing
        let mut stderr = Vec::new();
        assert!(loader.run(&mut stderr).now_or_never().is_none());
        assert!(stderr.is_empty());
    }

    #[tokio::test]
    async fn loader_loads_rules_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (mut loader, receiver) = RulesLoader::new(Some(path.clone()), false).await.unwrap();

        assert!(matches!(&loader.0, Inner::NotWatching));
        assert_eq!(receiver.borrow().rules[0].name, "initial");

        let original = Arc::clone(&receiver.borrow());

        // Changes are ignored when watching is disabled
        fs::write(&path, UPDATED).await.unwrap();

        let mut stderr = Vec::new();
        assert!(loader.run(&mut stderr).now_or_never().is_none());
        assert_eq!(receiver.borrow().rules[0].name, "initial");
        assert!(Arc::ptr_eq(&original, &receiver.borrow()));
        assert!(stderr.is_empty());
    }

    #[tokio::test]
    async fn loader_watches_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (loader, receiver) = RulesLoader::new(Some(path), true).await.unwrap();

        assert!(matches!(&loader.0, Inner::Watching(_)));
        assert_eq!(receiver.borrow().rules[0].name, "initial");
        assert!(!receiver.has_changed().unwrap());
    }

    #[tokio::test]
    async fn loader_rejects_invalid_initial_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        // A missing file fails in both loading modes
        assert!(RulesLoader::new(Some(path.clone()), false).await.is_err());
        assert!(RulesLoader::new(Some(path.clone()), true).await.is_err());

        // Invalid TOML also fails without watching
        fs::write(&path, "[[rules").await.unwrap();
        assert!(RulesLoader::new(Some(path.clone()), false).await.is_err());
        assert!(RulesLoader::new(Some(path), true).await.is_err());

        // The parent directory must exist
        let path = dir.path().join("missing").join("rules.toml");
        assert!(RulesLoader::new(Some(path.clone()), false).await.is_err());
        assert!(RulesLoader::new(Some(path), true).await.is_err());
    }

    #[tokio::test]
    async fn watcher_filters_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (loader, _) = RulesLoader::new(Some(path), true).await.unwrap();
        let Inner::Watching(watcher) = loader.0 else {
            panic!("Expected a rules watcher");
        };

        // Accept modifications to the rules file
        let event = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(watcher.path.clone());
        assert!(watcher.affects_rules(&event));

        // Ignore other files in the same directory
        let event =
            Event::new(EventKind::Modify(ModifyKind::Any)).add_path(dir.path().join("other.toml"));
        assert!(!watcher.affects_rules(&event));

        // Reading the file must not trigger another reload
        let event = Event::new(EventKind::Access(AccessKind::Any)).add_path(watcher.path.clone());
        assert!(!watcher.affects_rules(&event));
    }

    #[tokio::test]
    async fn watcher_reload_updates_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (loader, receiver) = RulesLoader::new(Some(path.clone()), true).await.unwrap();
        let Inner::Watching(mut watcher) = loader.0 else {
            panic!("Expected a rules watcher");
        };
        let original = Arc::clone(&receiver.borrow());
        let uri = Uri::from_static("http://example.com/test");
        let mut request = Request::builder().uri("/test").body(Bytes::new()).unwrap();

        // Publish new rules without changing existing snapshots
        fs::write(&path, UPDATED).await.unwrap();

        assert!(watcher.reload().await.unwrap());

        let updated = Arc::clone(&receiver.borrow());

        assert!(!Arc::ptr_eq(&original, &updated));
        assert!(matches!(
            original.apply(&uri, &mut request).outcome,
            RuleOutcome::Deny(StatusCode::FORBIDDEN)
        ));

        let report = updated.apply(&uri, &mut request);

        assert!(matches!(
            report.outcome,
            RuleOutcome::Deny(StatusCode::TOO_MANY_REQUESTS)
        ));
        assert_eq!(report.matched_rules, vec!["updated"]);
    }

    #[tokio::test]
    async fn watcher_reload_keeps_previous_rules_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (loader, receiver) = RulesLoader::new(Some(path.clone()), true).await.unwrap();
        let Inner::Watching(mut watcher) = loader.0 else {
            panic!("Expected a rules watcher");
        };
        let original = Arc::clone(&receiver.borrow());

        // Invalid TOML must not replace the active rules
        fs::write(&path, "[[rules").await.unwrap();
        assert!(watcher.reload().await.is_err());
        assert!(!receiver.has_changed().unwrap());
        assert!(Arc::ptr_eq(&original, &receiver.borrow()));

        // A missing file must not clear the active rules
        fs::remove_file(&path).await.unwrap();
        assert!(watcher.reload().await.is_err());
        assert!(!receiver.has_changed().unwrap());
        assert!(Arc::ptr_eq(&original, &receiver.borrow()));

        // Recover after a valid file is saved again
        fs::write(&path, UPDATED).await.unwrap();
        assert!(watcher.reload().await.unwrap());
        assert_eq!(receiver.borrow().rules[0].name, "updated");
    }

    #[tokio::test]
    async fn watcher_reload_skips_unchanged_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (loader, receiver) = RulesLoader::new(Some(path.clone()), true).await.unwrap();
        let Inner::Watching(mut watcher) = loader.0 else {
            panic!("Expected a rules watcher");
        };
        let original = Arc::clone(&receiver.borrow());

        // Repeated notifications do not publish the same rules again
        assert!(!watcher.reload().await.unwrap());

        fs::write(&path, INITIAL).await.unwrap();

        assert!(!watcher.reload().await.unwrap());
        assert!(!receiver.has_changed().unwrap());
        assert!(Arc::ptr_eq(&original, &receiver.borrow()));
    }

    #[tokio::test]
    async fn watcher_reloads_after_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (mut loader, mut receiver) = RulesLoader::new(Some(path.clone()), true).await.unwrap();
        let mut stderr = Vec::new();

        let checks = async {
            fs::write(&path, UPDATED).await.unwrap();
            receiver.changed().await.unwrap();
            assert_eq!(receiver.borrow().rules[0].name, "updated");
        };

        tokio::select! {
            result = loader.run(&mut stderr) => panic!("Reload loop stopped: {result:?}"),
            result = time::timeout(TEST_TIMEOUT, checks) => result.unwrap(),
        }
    }

    #[tokio::test]
    async fn watcher_reloads_after_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        let replacement = dir.path().join("replacement.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (mut loader, mut receiver) = RulesLoader::new(Some(path.clone()), true).await.unwrap();
        let mut stderr = Vec::new();

        let checks = async {
            // Replace the file once
            fs::write(&replacement, UPDATED).await.unwrap();
            fs::rename(&replacement, &path).await.unwrap();

            receiver.changed().await.unwrap();

            assert_eq!(receiver.borrow_and_update().rules[0].name, "updated");

            // The directory watch must survive another replacement
            fs::write(&replacement, INITIAL).await.unwrap();
            fs::rename(&replacement, &path).await.unwrap();

            receiver.changed().await.unwrap();

            assert_eq!(receiver.borrow().rules[0].name, "initial");
        };

        tokio::select! {
            result = loader.run(&mut stderr) => panic!("Reload loop stopped: {result:?}"),
            result = time::timeout(TEST_TIMEOUT, checks) => result.unwrap(),
        }
    }

    #[tokio::test]
    async fn watcher_recovers_after_invalid_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (mut loader, mut receiver) = RulesLoader::new(Some(path.clone()), true).await.unwrap();
        let (mut stderr, output) = tokio::io::duplex(4096);
        let mut lines = BufReader::new(output).lines();

        let checks = async {
            // Wait for the error, which can span several diagnostic lines
            fs::write(&path, "[[rules").await.unwrap();

            loop {
                let message = lines.next_line().await.unwrap().unwrap();
                if message.contains("keeping previous rules") {
                    break;
                }
            }

            assert!(!receiver.has_changed().unwrap());
            assert_eq!(receiver.borrow().rules[0].name, "initial");

            // The watcher continues after the file is corrected
            fs::write(&path, UPDATED).await.unwrap();
            receiver.changed().await.unwrap();
            assert_eq!(receiver.borrow().rules[0].name, "updated");
            let message = lines.next_line().await.unwrap().unwrap();
            assert!(message.contains("Rules reloaded"));
        };

        tokio::select! {
            result = loader.run(&mut stderr) => panic!("Reload loop stopped: {result:?}"),
            result = time::timeout(TEST_TIMEOUT, checks) => result.unwrap(),
        }
    }

    #[tokio::test]
    async fn watcher_recovers_after_recreation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");

        fs::write(&path, INITIAL).await.unwrap();

        let (mut loader, mut receiver) = RulesLoader::new(Some(path.clone()), true).await.unwrap();
        let (mut stderr, output) = tokio::io::duplex(4096);
        let mut lines = BufReader::new(output).lines();

        let checks = async {
            // Keep the active rules while the file is absent
            fs::remove_file(&path).await.unwrap();
            let message = lines.next_line().await.unwrap().unwrap();
            assert!(message.contains("keeping previous rules"));
            assert!(!receiver.has_changed().unwrap());
            assert_eq!(receiver.borrow().rules[0].name, "initial");

            // Recreating the file publishes the new rules
            fs::write(&path, UPDATED).await.unwrap();
            receiver.changed().await.unwrap();
            assert_eq!(receiver.borrow().rules[0].name, "updated");
            let message = lines.next_line().await.unwrap().unwrap();
            assert!(message.contains("Rules reloaded"));
        };

        tokio::select! {
            result = loader.run(&mut stderr) => panic!("Reload loop stopped: {result:?}"),
            result = time::timeout(TEST_TIMEOUT, checks) => result.unwrap(),
        }
    }
}
