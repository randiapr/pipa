//! A child process under test (the backend or an ingestion instance): its output goes to a log
//! file that failures can show, and it is killed when dropped (or, if it is part of the shared stack and never dropped, when the
//! test process exits).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Context;
use tokio::process::{Child, Command};

pub struct Process {
    name: String,
    child: Child,
    log: PathBuf,
    pid: Option<u32>,
}

impl Process {
    /// Starts `binary` with exactly `envs` as its environment: nothing leaks in from the shell the
    /// tests run in (a `RUSTFS_BUCKET` or `REDIS_URL` of a local stack would redirect it).
    pub fn spawn(name: &str, binary: &Path, envs: &[(String, String)]) -> anyhow::Result<Self> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join("pipa-integration-test");
        std::fs::create_dir_all(&dir)?;
        let log = dir.join(format!(
            "{name}-{}-{}.log",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let file = File::create(&log)?;

        let child = Command::new(binary)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .envs(envs.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(file.try_clone()?)
            .stderr(file)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", binary.display()))?;
        let pid = child.id();
        if let Some(pid) = pid {
            crate::cleanup::kill_process_at_exit(pid);
        }
        Ok(Self {
            name: name.to_string(),
            child,
            log,
            pid,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Whether the process is still alive.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits up to `timeout` for the process to exit on its own.
    pub async fn exited_within(&mut self, timeout: Duration) -> Option<ExitStatus> {
        tokio::time::timeout(timeout, self.child.wait())
            .await
            .ok()
            .and_then(Result::ok)
    }

    /// Everything the process has logged so far.
    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// The last `lines` lines of [`Self::log`], for failure messages.
    pub fn log_tail(&self, lines: usize) -> String {
        let log = self.log();
        let all: Vec<&str> = log.lines().collect();
        all[all.len().saturating_sub(lines)..].join("\n")
    }

    /// Asks the process to shut down (SIGTERM, which both binaries handle by releasing what they
    /// hold) and waits for it, killing it if it doesn't.
    pub async fn terminate(&mut self) {
        if let Some(pid) = self.child.id() {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await;
        }
        if self.exited_within(Duration::from_secs(20)).await.is_none() {
            let _ = self.child.kill().await;
        }
    }

    /// Kills the process outright (SIGKILL): it gets no chance to release anything.
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            crate::cleanup::forget_process(pid);
        }
        if std::thread::panicking() || std::env::var_os("PIPA_TEST_KEEP_LOGS").is_some() {
            eprintln!("log of {}: {}", self.name, self.log.display());
        } else {
            let _ = std::fs::remove_file(&self.log);
        }
    }
}
