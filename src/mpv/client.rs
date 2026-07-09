use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast, oneshot};

use crate::mpv::protocol::{Command, Event, Message, Reply};

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

/// A duration in seconds that may be negative. Used for relative seeks,
/// where a positive value means forward and a negative value means back.
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
    Absolute(std::time::Duration),
    Relative(SignedDuration),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

pub struct MpvClient {
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    next_request_id: AtomicU64,
    pending: Pending,
    event_tx: broadcast::Sender<Event>,
}

impl MpvClient {
    pub fn new<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (event_tx, _) = broadcast::channel(128);
        tokio::spawn(reader_loop(reader, Arc::clone(&pending), event_tx.clone()));
        Self {
            writer: Mutex::new(Box::new(writer)),
            next_request_id: AtomicU64::new(1),
            pending,
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
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
        self.send(vec![json!("loadfile"), json!(url), json!("append")])
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
    event_tx: broadcast::Sender<Event>,
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
                let _ = event_tx.send(event);
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
    async fn subscriber_receives_events() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mut rx = client.subscribe();

        let mpv_task = tokio::spawn(async move {
            let (_read_half, mut write_half) = tokio::io::split(mpv_side);
            let event = format!("{}\n", json!({"event": "end-file", "reason": "eof"}));
            write_half.write_all(event.as_bytes()).await.unwrap();
        });

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for event")
            .unwrap();
        assert_eq!(event.event, "end-file");
        assert_eq!(event.extra.get("reason"), Some(&json!("eof")));
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive_events() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mut rx1 = client.subscribe();
        let mut rx2 = client.subscribe();

        let mpv_task = tokio::spawn(async move {
            let (_read_half, mut write_half) = tokio::io::split(mpv_side);
            let event = format!("{}\n", json!({"event": "idle"}));
            write_half.write_all(event.as_bytes()).await.unwrap();
        });

        let e1 = tokio::time::timeout(std::time::Duration::from_secs(1), rx1.recv())
            .await
            .expect("timed out waiting for event on rx1")
            .unwrap();
        let e2 = tokio::time::timeout(std::time::Duration::from_secs(1), rx2.recv())
            .await
            .expect("timed out waiting for event on rx2")
            .unwrap();
        assert_eq!(e1.event, "idle");
        assert_eq!(e2.event, "idle");
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
        let mpv_task = scripted_mpv(
            mpv_side,
            json!(["get_property", "pause"]),
            json!(true),
        );
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

    #[tokio::test]
    async fn loadfile_appends_by_default() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(
            mpv_side,
            json!(["loadfile", "https://example.com/video", "append"]),
            json!(null),
        );
        client
            .loadfile("https://example.com/video")
            .await
            .unwrap();
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
        let mpv_task = scripted_mpv(
            mpv_side,
            json!(["seek", 30.5, "absolute"]),
            json!(null),
        );
        client
            .seek(Seek::Absolute(std::time::Duration::from_secs_f64(30.5)))
            .await
            .unwrap();
        mpv_task.await.unwrap();
    }

    #[tokio::test]
    async fn seek_relative_sends_relative_flag() {
        let (client_side, mpv_side) = tokio::io::duplex(1024);
        let client = MpvClient::new(client_side);
        let mpv_task = scripted_mpv(
            mpv_side,
            json!(["seek", 10.0, "relative"]),
            json!(null),
        );
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
        let mpv_task = scripted_mpv(
            mpv_side,
            json!(["seek", -10.0, "relative"]),
            json!(null),
        );
        client
            .seek(Seek::Relative(SignedDuration::from_secs_f64(-10.0)))
            .await
            .unwrap();
        mpv_task.await.unwrap();
    }
}
