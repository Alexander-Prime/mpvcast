use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot, watch};

use crate::mpv::protocol::{Command, Message, Reply};
use crate::mpv::snapshot::StatusSnapshot;

#[derive(Debug, thiserror::Error)]
pub enum MpvError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("mpv error: {0}")]
    Mpv(String),

    #[error("connection closed")]
    ConnectionClosed,
}

pub type Result<T> = std::result::Result<T, MpvError>;

/// A number of seconds that may be negative. For a relative seek the sign
/// gives the direction; for an absolute one it counts back from the end of
/// the file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignedDuration(f64);

impl SignedDuration {
    pub fn from_secs_f64(secs: f64) -> Self {
        Self(secs)
    }

    pub fn as_secs_f64(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seek {
    Absolute(SignedDuration),
    Relative(SignedDuration),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

pub struct MpvClient {
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    next_request_id: AtomicU64,
    pending: Pending,
    status_tx: Arc<watch::Sender<StatusSnapshot>>,
}

impl MpvClient {
    pub fn new<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (status_tx, _) = watch::channel(StatusSnapshot::default());
        let status_tx = Arc::new(status_tx);
        tokio::spawn(reader_loop(
            reader,
            Arc::clone(&pending),
            Arc::clone(&status_tx),
        ));
        Self {
            writer: Mutex::new(Box::new(writer)),
            next_request_id: AtomicU64::new(1),
            pending,
            status_tx,
        }
    }

    /// Watch the status for changes. The receiver carries the current snapshot,
    /// so a subscriber can read the state straight away rather than waiting for
    /// the next change.
    pub fn subscribe(&self) -> watch::Receiver<StatusSnapshot> {
        self.status_tx.subscribe()
    }

    pub fn status_snapshot(&self) -> StatusSnapshot {
        self.status_tx.borrow().clone()
    }

    pub async fn observe_status_properties(&self) -> Result<()> {
        const PROPS: &[(&str, u64)] = &[
            ("pause", 1),
            ("time-pos", 2),
            ("duration", 3),
            ("volume", 4),
            ("mute", 5),
            ("playlist", 6),
            ("playlist-pos", 7),
            ("media-title", 8),
        ];
        for (name, id) in PROPS {
            self.send(vec![json!("observe_property"), json!(id), json!(name)])
                .await?;
        }
        Ok(())
    }

    pub async fn get_property(&self, name: &str) -> Result<Value> {
        self.send(vec![json!("get_property"), json!(name)]).await
    }

    pub async fn set_property<T: Into<Value>>(&self, name: &str, value: T) -> Result<()> {
        self.send(vec![json!("set_property"), json!(name), value.into()])
            .await?;
        Ok(())
    }

    pub async fn loadfile(&self, url: &str) -> Result<()> {
        self.send(vec![json!("loadfile"), json!(url), json!("append-play")])
            .await?;
        Ok(())
    }

    pub async fn playlist_clear(&self) -> Result<()> {
        self.send(vec![json!("playlist-clear")]).await?;
        Ok(())
    }

    pub async fn playlist_next(&self) -> Result<()> {
        self.send(vec![json!("playlist-next")]).await?;
        Ok(())
    }

    pub async fn playlist_prev(&self) -> Result<()> {
        self.send(vec![json!("playlist-prev")]).await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        self.send(vec![json!("stop")]).await?;
        Ok(())
    }

    pub async fn seek(&self, seek: Seek) -> Result<()> {
        let (target, mode) = match seek {
            Seek::Absolute(d) => (d.as_secs_f64(), "absolute"),
            Seek::Relative(d) => (d.as_secs_f64(), "relative"),
        };
        self.send(vec![json!("seek"), json!(target), json!(mode)])
            .await?;
        Ok(())
    }

    pub async fn send(&self, args: Vec<Value>) -> Result<Value> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let cmd = Command {
            command: args,
            request_id: id,
        };
        let line = format!("{}\n", serde_json::to_string(&cmd)?);

        let write_result = {
            let mut writer = self.writer.lock().await;
            match writer.write_all(line.as_bytes()).await {
                Ok(()) => writer.flush().await,
                Err(e) => Err(e),
            }
        };
        if let Err(e) = write_result {
            self.pending.lock().await.remove(&id);
            return Err(e.into());
        }

        let reply = rx.await.map_err(|_| MpvError::ConnectionClosed)?;
        if reply.error == "success" {
            Ok(reply.data.unwrap_or(Value::Null))
        } else {
            Err(MpvError::Mpv(reply.error))
        }
    }
}

async fn reader_loop<R: AsyncRead + Unpin>(
    reader: R,
    pending: Pending,
    status_tx: Arc<watch::Sender<StatusSnapshot>>,
) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Message>(trimmed) {
            Ok(Message::Reply(reply)) => {
                let id = reply.request_id;
                if let Some(tx) = pending.lock().await.remove(&id) {
                    let _ = tx.send(reply);
                }
            }
            Ok(Message::Event(event)) => {
                status_tx.send_if_modified(|snapshot| snapshot.apply_event(&event));
            }
            Err(_) => {
                // Malformed line; skip.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::DuplexStream;
    use tokio::task::JoinHandle;

    /// Spawn a fake mpv that reads one command, replies with `reply_data`,
    /// then asserts the `command` array matched `expected_command`. Reply is
    /// sent before the assertion so a mismatched command still unblocks the
    /// client — the test fails via the joined task's panic, not a hang.
    fn scripted_mpv(
        mpv_side: DuplexStream,
        expected_command: Value,
        reply_data: Value,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let (read_half, mut write_half) = tokio::io::split(mpv_side);
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let cmd: Value = serde_json::from_str(line.trim()).unwrap();
            let id = cmd["request_id"].as_u64().unwrap();
            let reply = format!(
                "{}\n",
                json!({"error": "success", "data": reply_data, "request_id": id})
            );
            write_half.write_all(reply.as_bytes()).await.unwrap();
            assert_eq!(cmd["command"], expected_command);
        })
    }

    #[tokio::test]
    async fn send_returns_data_on_successful_reply() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);

        let mpv_task = tokio::spawn(async move {
            let (read_half, mut write_half) = tokio::io::split(mpv_side);
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let cmd: Value = serde_json::from_str(line.trim()).unwrap();
            let id = cmd["request_id"].as_u64().unwrap();
            let reply = format!(
                "{}\n",
                json!({"error": "success", "data": 42, "request_id": id})
            );
            write_half.write_all(reply.as_bytes()).await.unwrap();
        });

        let result = client
            .send(vec![json!("get_property"), json!("pause")])
            .await
            .unwrap();
        assert_eq!(result, json!(42));
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn send_returns_error_when_mpv_reports_failure() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);

        let mpv_task = tokio::spawn(async move {
            let (read_half, mut write_half) = tokio::io::split(mpv_side);
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let cmd: Value = serde_json::from_str(line.trim()).unwrap();
            let id = cmd["request_id"].as_u64().unwrap();
            let reply = format!(
                "{}\n",
                json!({"error": "property unavailable", "request_id": id})
            );
            write_half.write_all(reply.as_bytes()).await.unwrap();
        });

        let result = client
            .send(vec![json!("get_property"), json!("nope")])
            .await;
        match result {
            Err(MpvError::Mpv(msg)) => assert_eq!(msg, "property unavailable"),
            other => panic!("expected MpvError::Mpv, got {:?}", other),
        }
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn multiple_subscribers_each_observe_changes() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mut rx1 = client.subscribe();
        let mut rx2 = client.subscribe();

        let mpv_task = tokio::spawn(async move {
            let (_read_half, mut write_half) = tokio::io::split(mpv_side);
            let event =
                json!({"event": "property-change", "id": 4, "name": "volume", "data": 55.0});
            write_half
                .write_all(format!("{}\n", event).as_bytes())
                .await
                .unwrap();
        });

        for (name, rx) in [("rx1", &mut rx1), ("rx2", &mut rx2)] {
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for change on {name}"))
                .unwrap();
            assert_eq!(rx.borrow().volume, Some(55.0), "{name} saw the wrong value");
        }
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn replies_are_routed_by_request_id() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = Arc::new(MpvClient::new(client_side));

        let mpv_task = tokio::spawn(async move {
            let (read_half, mut write_half) = tokio::io::split(mpv_side);
            let mut reader = BufReader::new(read_half);
            let mut ids = Vec::new();
            for _ in 0..2 {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let cmd: Value = serde_json::from_str(line.trim()).unwrap();
                let id = cmd["request_id"].as_u64().unwrap();
                let cmd_name = cmd["command"][0].as_str().unwrap().to_string();
                ids.push((id, cmd_name));
            }
            // Reply in reverse order; each reply tagged with the originating
            // command name so the test can verify routing by request_id.
            for (id, cmd_name) in ids.iter().rev() {
                let reply = format!(
                    "{}\n",
                    json!({"error": "success", "data": cmd_name, "request_id": id})
                );
                write_half.write_all(reply.as_bytes()).await.unwrap();
            }
        });

        let c1 = Arc::clone(&client);
        let c2 = Arc::clone(&client);
        let h1 = tokio::spawn(async move { c1.send(vec![json!("alpha")]).await });
        let h2 = tokio::spawn(async move { c2.send(vec![json!("beta")]).await });

        let r1 = h1.await.unwrap().unwrap();
        let r2 = h2.await.unwrap().unwrap();
        assert_eq!(r1, json!("alpha"));
        assert_eq!(r2, json!("beta"));
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn get_property_sends_expected_command() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["get_property", "pause"]), json!(true));
        assert_eq!(client.get_property("pause").await.unwrap(), json!(true));
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn set_property_sends_expected_command() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(
            mpv_side,
            json!(["set_property", "pause", true]),
            json!(null),
        );
        client.set_property("pause", true).await.unwrap();
        mpv_task.await.unwrap();
    }

    /// `append-play` rather than `append`: queueing a URL at an idle player
    /// should start it, while leaving an already-playing item alone.
    #[tokio::test]
    async fn loadfile_appends_and_plays_when_idle() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(
            mpv_side,
            json!(["loadfile", "https://example.com/video", "append-play"]),
            json!(null),
        );
        client.loadfile("https://example.com/video").await.unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn playlist_clear_sends_expected_command() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["playlist-clear"]), json!(null));
        client.playlist_clear().await.unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn playlist_next_sends_expected_command() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["playlist-next"]), json!(null));
        client.playlist_next().await.unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn playlist_prev_sends_expected_command() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["playlist-prev"]), json!(null));
        client.playlist_prev().await.unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn stop_sends_expected_command() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["stop"]), json!(null));
        client.stop().await.unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn seek_absolute_sends_absolute_flag() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["seek", 30.5, "absolute"]), json!(null));
        client
            .seek(Seek::Absolute(SignedDuration::from_secs_f64(30.5)))
            .await
            .unwrap();
        mpv_task.await.unwrap();
    }

    /// mpv reads a negative absolute position as counting back from the end
    /// of the file, so the sign has to survive the trip.
    #[tokio::test]
    async fn seek_absolute_passes_a_negative_target_through() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["seek", -5.0, "absolute"]), json!(null));
        client
            .seek(Seek::Absolute(SignedDuration::from_secs_f64(-5.0)))
            .await
            .unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn seek_relative_sends_relative_flag() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["seek", 10.0, "relative"]), json!(null));
        client
            .seek(Seek::Relative(SignedDuration::from_secs_f64(10.0)))
            .await
            .unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn seek_relative_supports_negative_delta() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(mpv_side, json!(["seek", -10.0, "relative"]), json!(null));
        client
            .seek(Seek::Relative(SignedDuration::from_secs_f64(-10.0)))
            .await
            .unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_reflects_property_change_events() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mut status = client.subscribe();

        tokio::spawn(async move {
            let (_read_half, mut write_half) = tokio::io::split(mpv_side);
            let event = json!({"event": "property-change", "id": 1, "name": "pause", "data": true});
            write_half
                .write_all(format!("{}\n", event).as_bytes())
                .await
                .unwrap();
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), status.changed())
            .await
            .expect("timed out waiting for status change")
            .unwrap();
        assert_eq!(status.borrow().pause, Some(true));
        assert_eq!(client.status_snapshot().pause, Some(true));
    }

    #[tokio::test]
    async fn unchanged_state_does_not_notify_subscribers() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mut status = client.subscribe();
        let (release_duplicate, duplicate_released) = oneshot::channel();

        let mpv_task = tokio::spawn(async move {
            let (read_half, mut write_half) = tokio::io::split(mpv_side);
            let mut reader = BufReader::new(read_half);
            let pause = json!({"event": "property-change", "id": 1, "name": "pause", "data": true});
            write_half
                .write_all(format!("{}\n", pause).as_bytes())
                .await
                .unwrap();
            // Hold the duplicate back until the first change has been observed.
            // Sent together, the channel would coalesce both into one wakeup and
            // the duplicate's notification would be invisible either way.
            duplicate_released.await.unwrap();
            write_half
                .write_all(format!("{}\n", pause).as_bytes())
                .await
                .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let cmd: Value = serde_json::from_str(line.trim()).unwrap();
            let id = cmd["request_id"].as_u64().unwrap();
            write_half
                .write_all(
                    format!("{}\n", json!({"error": "success", "request_id": id})).as_bytes(),
                )
                .await
                .unwrap();
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), status.changed())
            .await
            .expect("timed out waiting for the first change")
            .unwrap();
        assert_eq!(status.borrow().pause, Some(true));
        release_duplicate.send(()).unwrap();

        // The reader handles the socket in order, so a reply to a command sent
        // after the duplicate proves the duplicate has already been processed —
        // without this the assertion below would pass vacuously.
        client.get_property("pause").await.unwrap();

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), status.changed())
                .await
                .is_err(),
            "a property-change that altered nothing must not wake subscribers"
        );
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_records_end_file_errors() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mut status = client.subscribe();

        tokio::spawn(async move {
            let (_read_half, mut write_half) = tokio::io::split(mpv_side);
            let event = json!({
                "event": "end-file",
                "reason": "error",
                "file_error": "connection refused"
            });
            write_half
                .write_all(format!("{}\n", event).as_bytes())
                .await
                .unwrap();
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), status.changed())
            .await
            .expect("timed out waiting for status change")
            .unwrap();

        let snapshot = client.status_snapshot();
        assert_eq!(snapshot.errors.len(), 1);
        assert!(
            snapshot.errors[0].message.contains("connection refused"),
            "expected message to contain 'connection refused', got {:?}",
            snapshot.errors[0].message
        );
    }

    #[tokio::test]
    async fn snapshot_clears_errors_on_start_file() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mut status = client.subscribe();
        let (release_start_file, start_file_released) = oneshot::channel();

        tokio::spawn(async move {
            let (_read_half, mut write_half) = tokio::io::split(mpv_side);
            let err_event = json!({
                "event": "end-file",
                "reason": "error",
                "file_error": "playback failed"
            });
            write_half
                .write_all(format!("{}\n", err_event).as_bytes())
                .await
                .unwrap();
            // Hold start-file back until the error has been observed, so the
            // clearing is proven to act on a non-empty error list.
            start_file_released.await.unwrap();
            let start_event = json!({"event": "start-file"});
            write_half
                .write_all(format!("{}\n", start_event).as_bytes())
                .await
                .unwrap();
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), status.changed())
            .await
            .expect("timed out waiting for end-file")
            .unwrap();
        assert_eq!(
            client.status_snapshot().errors.len(),
            1,
            "error must be recorded before start-file can be shown to clear it"
        );

        release_start_file.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), status.changed())
            .await
            .expect("timed out waiting for start-file")
            .unwrap();

        let snapshot = client.status_snapshot();
        assert!(
            snapshot.errors.is_empty(),
            "expected errors cleared, got {:?}",
            snapshot.errors
        );
    }

    #[tokio::test]
    async fn observe_status_properties_sends_all_observers() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);

        let mpv_task = tokio::spawn(async move {
            let (read_half, mut write_half) = tokio::io::split(mpv_side);
            let mut reader = BufReader::new(read_half);
            let mut names = Vec::new();
            for _ in 0..8 {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let cmd: Value = serde_json::from_str(line.trim()).unwrap();
                let id = cmd["request_id"].as_u64().unwrap();
                let name = cmd["command"][2].as_str().unwrap().to_string();
                names.push(name);
                let reply = format!("{}\n", json!({"error": "success", "request_id": id}));
                write_half.write_all(reply.as_bytes()).await.unwrap();
            }
            names
        });

        // Both awaits are bounded: if the observed set ever shrinks, the fake
        // mpv blocks on a command that never arrives, and if it grows, the
        // client blocks on a reply that never comes. Either would hang here
        // rather than fail without the timeouts.
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.observe_status_properties(),
        )
        .await
        .expect("timed out sending observe_property commands")
        .unwrap();
        let names = tokio::time::timeout(std::time::Duration::from_secs(1), mpv_task)
            .await
            .expect("timed out waiting for observe_property commands")
            .unwrap();

        for expected in [
            "pause",
            "time-pos",
            "duration",
            "volume",
            "mute",
            "playlist",
            "playlist-pos",
            "media-title",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing {} in {:?}",
                expected,
                names
            );
        }
    }
}
