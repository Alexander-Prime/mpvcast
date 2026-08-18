use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post, put},
};
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
                .post(not_implemented)
                .delete(not_implemented),
        )
        .route("/playback/play", post(not_implemented))
        .route("/playback/pause", post(not_implemented))
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
    use tokio::io::AsyncWriteExt;
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
}
