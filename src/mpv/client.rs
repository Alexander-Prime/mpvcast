use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot};

use crate::mpv::protocol::{Command, Message, Reply};

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

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

pub struct MpvClient {
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    next_request_id: AtomicU64,
    pending: Pending,
}

impl MpvClient {
    pub fn new<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(reader_loop(reader, Arc::clone(&pending)));
        Self {
            writer: Mutex::new(Box::new(writer)),
            next_request_id: AtomicU64::new(1),
            pending,
        }
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

async fn reader_loop<R: AsyncRead + Unpin>(reader: R, pending: Pending) {
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
            Ok(Message::Event(_)) => {
                // Step 4 wires events to a broadcast channel.
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
}
