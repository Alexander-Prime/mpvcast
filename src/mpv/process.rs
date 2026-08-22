use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::time::Instant;

use crate::mpv::client::{MpvClient, MpvError};

#[derive(Debug, thiserror::Error)]
pub enum MpvProcessError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("timed out waiting for mpv socket at {0}")]
    SocketTimeout(PathBuf),

    #[error("mpv client error: {0}")]
    Mpv(#[from] MpvError),
}

/// Asks the kernel to signal mpv when this process dies. `kill_on_drop`
/// covers a tidy exit, but a process killed outright runs no destructors,
/// which would leave a fullscreen player on screen with nothing driving it.
///
/// `PR_SET_PDEATHSIG` is armed against the spawning *thread* rather than the
/// process. tokio's workers live as long as the runtime, so mpv outlives
/// every thread that could plausibly spawn it here.
fn bind_lifetime_to_parent(command: &mut Command) {
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: the closure runs in the forked child before exec, so it may
    // only call async-signal-safe functions. prctl, getppid and _exit are.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent can die between the fork and the call above, which
            // leaves the signal unarmed and mpv reparented to init.
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }
}

pub struct MpvProcess {
    _child: Child,
    socket_path: PathBuf,
    client: Arc<MpvClient>,
}

impl MpvProcess {
    pub async fn spawn() -> Result<Self, MpvProcessError> {
        Self::spawn_inner(&[
            "--idle=yes",
            "--force-window=immediate",
            "--fullscreen",
            "--hwdec=auto-safe",
            "--ytdl=yes",
            "--auto-window-resize=no",
        ])
        .await
    }

    async fn spawn_inner(extra_args: &[&str]) -> Result<Self, MpvProcessError> {
        let socket_path = PathBuf::from(format!("/tmp/mpvcast-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket_path);

        let mut command = Command::new("mpv");
        command
            .arg(format!("--input-ipc-server={}", socket_path.display()))
            .args(extra_args)
            .kill_on_drop(true);
        bind_lifetime_to_parent(&mut command);
        let child = command.spawn()?;

        let stream = Self::connect_with_retry(&socket_path, Duration::from_secs(5)).await?;
        let client = Arc::new(MpvClient::new(stream));
        client.observe_status_properties().await?;

        Ok(Self {
            _child: child,
            socket_path,
            client,
        })
    }

    pub fn client(&self) -> Arc<MpvClient> {
        Arc::clone(&self.client)
    }

    async fn connect_with_retry(
        path: &Path,
        timeout: Duration,
    ) -> Result<UnixStream, MpvProcessError> {
        let start = Instant::now();
        let mut last_err = None;
        while start.elapsed() < timeout {
            match UnixStream::connect(path).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        Err(last_err
            .map(MpvProcessError::Io)
            .unwrap_or_else(|| MpvProcessError::SocketTimeout(path.to_path_buf())))
    }
}

impl Drop for MpvProcess {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mpv_available() -> bool {
        std::process::Command::new("mpv")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    async fn spawn_headless_for_test() -> Result<MpvProcess, MpvProcessError> {
        MpvProcess::spawn_inner(&["--idle=yes", "--vo=null", "--ao=null"]).await
    }

    #[tokio::test]
    async fn connect_with_retry_times_out_when_socket_never_appears() {
        let path = std::env::temp_dir().join("mpvcast-test-nonexistent.sock");
        let _ = std::fs::remove_file(&path);
        let result = MpvProcess::connect_with_retry(&path, Duration::from_millis(200)).await;
        assert!(result.is_err(), "expected timeout, got {:?}", result);
    }

    #[tokio::test]
    async fn spawn_connects_and_serves_get_property() {
        if !mpv_available() {
            eprintln!("skipping: mpv not on PATH");
            return;
        }
        let mpv = spawn_headless_for_test().await.unwrap();
        let version = mpv.client().get_property("mpv-version").await.unwrap();
        assert!(
            version.is_string(),
            "expected string version, got {:?}",
            version
        );
    }
}
