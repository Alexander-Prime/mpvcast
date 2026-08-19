use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;
use std::sync::Arc;

mod api;
mod mpv;

use crate::mpv::client::MpvClient;
use crate::mpv::process::MpvProcess;

type AppState = Arc<MpvClient>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let mpv = MpvProcess::spawn().await?;
    let state = mpv.client();

    let app = router(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("Listening on http://0.0.0.0:8080");
    axum::serve(listener, app).await?;

    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/status", get(get_status).patch(not_implemented))
        .route(
            "/queue",
            get(get_queue)
                .put(not_implemented)
                .post(append_to_queue)
                .delete(not_implemented),
        )
        .route("/playback/play", post(play))
        .route("/playback/pause", post(pause))
        .route("/playback/next", post(not_implemented))
        .route("/playback/previous", post(not_implemented))
        .route(
            "/playback/position",
            put(not_implemented).patch(not_implemented),
        )
        .route("/events", get(not_implemented))
        .with_state(state)
}

async fn get_status(State(client): State<AppState>) -> Json<api::Status> {
    Json(api::Status::from(&client.status_snapshot()))
}

async fn get_queue(State(client): State<AppState>) -> Json<Vec<api::Track>> {
    Json(api::queue(&client.status_snapshot()))
}

#[derive(Deserialize)]
struct QueueItem {
    url: String,
}

/// Appends to mpv's playlist with `append-play`, so sharing a URL at an idle
/// player starts it while an item already playing carries on undisturbed.
async fn append_to_queue(
    State(client): State<AppState>,
    Json(item): Json<QueueItem>,
) -> Result<StatusCode, MpvUnavailable> {
    client.loadfile(&item.url).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn play(State(client): State<AppState>) -> Result<StatusCode, MpvUnavailable> {
    let snapshot = client.status_snapshot();
    // Stopping leaves mpv idle with the playlist intact, and clearing the
    // pause flag alone won't restart it — something has to be selected first.
    if api::Status::from(&snapshot).playback.is_none() && !snapshot.playlist.is_empty() {
        client.set_property("playlist-pos", 0).await?;
    }
    client.set_property("pause", false).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pause(State(client): State<AppState>) -> Result<StatusCode, MpvUnavailable> {
    client.set_property("pause", true).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// mpv refused or never answered a command. The request itself was well
/// formed, so this reports a failure upstream of the client.
struct MpvUnavailable(mpv::client::MpvError);

impl From<mpv::client::MpvError> for MpvUnavailable {
    fn from(error: mpv::client::MpvError) -> Self {
        Self(error)
    }
}

impl IntoResponse for MpvUnavailable {
    fn into_response(self) -> Response {
        tracing::error!("mpv command failed: {}", self.0);
        StatusCode::BAD_GATEWAY.into_response()
    }
}

async fn not_implemented(State(_client): State<AppState>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::response::Response;
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    /// A client whose mpv never says anything, so its snapshot stays empty.
    fn idle_client() -> AppState {
        let (client_socket, _mpv_socket) = tokio::io::duplex(1024);
        Arc::new(MpvClient::new(client_socket))
    }

    /// A client that has been told about a two-item playlist with the second
    /// item playing, and has taken all of it in before returning.
    async fn loaded_client() -> AppState {
        let (client_socket, mut mpv_socket) = tokio::io::duplex(4096);
        let client = Arc::new(MpvClient::new(client_socket));
        let mut status = client.subscribe();

        tokio::spawn(async move {
            for event in [
                json!({"event": "property-change", "id": 6, "name": "playlist", "data": [
                    {"filename": "https://example.com/a", "title": "A"},
                    {"filename": "https://example.com/b"},
                ]}),
                json!({"event": "property-change", "id": 7, "name": "playlist-pos", "data": 1}),
                json!({"event": "property-change", "id": 8, "name": "media-title", "data": "Resolved Title"}),
                json!({"event": "property-change", "id": 2, "name": "time-pos", "data": 12.5}),
            ] {
                mpv_socket
                    .write_all(format!("{}\n", event).as_bytes())
                    .await
                    .unwrap();
            }
        });

        tokio::time::timeout(
            Duration::from_secs(1),
            status.wait_for(|snapshot| snapshot.time_pos.is_some()),
        )
        .await
        .expect("timed out waiting for mpv's events")
        .unwrap();

        client
    }

    /// Stands in for mpv: emits `events` first, then replies success to the
    /// next `commands` commands and hands back what it saw.
    fn scripted_mpv(
        socket: tokio::io::DuplexStream,
        events: Vec<Value>,
        commands: usize,
    ) -> tokio::task::JoinHandle<Vec<Value>> {
        tokio::spawn(async move {
            let (read_half, mut write_half) = tokio::io::split(socket);
            let mut reader = tokio::io::BufReader::new(read_half);

            for event in events {
                write_half
                    .write_all(format!("{}\n", event).as_bytes())
                    .await
                    .unwrap();
            }

            let mut seen = Vec::new();
            let mut line = String::new();
            while seen.len() < commands {
                line.clear();
                if reader.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                seen.push(request["command"].clone());
                let reply = json!({"error": "success", "request_id": request["request_id"]});
                write_half
                    .write_all(format!("{}\n", reply).as_bytes())
                    .await
                    .unwrap();
            }
            seen
        })
    }

    async fn post_json(state: AppState, uri: &str, body: Value) -> Response {
        router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn post_empty(state: AppState, uri: &str) -> Response {
        router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get(state: AppState, uri: &str) -> Response {
        router(state)
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn json_body(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn status_endpoint_serves_the_idle_document() {
        let response = get(idle_client(), "/status").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await,
            json!({
                "playback": null,
                "volume": 100,
                "muted": false,
                "errors": [],
            })
        );
    }

    #[tokio::test]
    async fn status_endpoint_serves_the_current_track() {
        let response = get(loaded_client().await, "/status").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await["playback"],
            json!({
                "paused": false,
                "current": {"url": "https://example.com/b", "title": "Resolved Title"},
                "current_index": 1,
                "position": 12.5,
                "duration": null,
            })
        );
    }

    #[tokio::test]
    async fn queue_endpoint_lists_the_playlist() {
        let response = get(loaded_client().await, "/queue").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await,
            json!([
                {"url": "https://example.com/a", "title": "A"},
                {"url": "https://example.com/b", "title": null},
            ])
        );
    }

    #[tokio::test]
    async fn queue_is_empty_while_mpv_has_reported_nothing() {
        let response = get(idle_client(), "/queue").await;
        assert_eq!(json_body(response).await, json!([]));
    }

    /// Sharing a URL at an idle player should be one request, so the append
    /// carries mpv's `append-play` rather than a bare append.
    #[tokio::test]
    async fn queueing_a_url_appends_and_plays_it() {
        let (client_socket, mpv_socket) = tokio::io::duplex(4096);
        let client = Arc::new(MpvClient::new(client_socket));
        let mpv = scripted_mpv(mpv_socket, Vec::new(), 1);

        let response = post_json(client, "/queue", json!({"url": "https://example.com/a"})).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![json!(["loadfile", "https://example.com/a", "append-play"])]
        );
    }

    #[tokio::test]
    async fn pause_sets_the_pause_property() {
        let (client_socket, mpv_socket) = tokio::io::duplex(4096);
        let client = Arc::new(MpvClient::new(client_socket));
        let mpv = scripted_mpv(mpv_socket, Vec::new(), 1);

        let response = post_empty(client, "/playback/pause").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![json!(["set_property", "pause", true])]
        );
    }

    #[tokio::test]
    async fn play_clears_the_pause_property() {
        let (client_socket, mpv_socket) = tokio::io::duplex(4096);
        let client = Arc::new(MpvClient::new(client_socket));
        let mpv = scripted_mpv(mpv_socket, Vec::new(), 1);

        let response = post_empty(client, "/playback/play").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![json!(["set_property", "pause", false])]
        );
    }

    /// After the queue has been stopped or cleared mpv sits idle with items
    /// still listed, and clearing the pause flag alone would not restart it.
    #[tokio::test]
    async fn play_starts_the_first_item_when_nothing_is_loaded() {
        let (client_socket, mpv_socket) = tokio::io::duplex(4096);
        let client = Arc::new(MpvClient::new(client_socket));
        let mut status = client.subscribe();
        let mpv = scripted_mpv(
            mpv_socket,
            vec![
                json!({"event": "property-change", "id": 6, "name": "playlist", "data": [
                    {"filename": "https://example.com/a"},
                ]}),
            ],
            2,
        );

        tokio::time::timeout(
            Duration::from_secs(1),
            status.wait_for(|snapshot| !snapshot.playlist.is_empty()),
        )
        .await
        .expect("timed out waiting for the playlist")
        .unwrap();

        let response = post_empty(client, "/playback/play").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![
                json!(["set_property", "playlist-pos", 0]),
                json!(["set_property", "pause", false]),
            ]
        );
    }
}
