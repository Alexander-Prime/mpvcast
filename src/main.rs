use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use std::sync::Arc;

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
        .route("/status", get(not_implemented).patch(not_implemented))
        .route(
            "/queue",
            get(not_implemented)
                .put(not_implemented)
                .post(not_implemented)
                .delete(not_implemented),
        )
        .route("/playback/play", post(not_implemented))
        .route("/playback/pause", post(not_implemented))
        .route("/playback/next", post(not_implemented))
        .route("/playback/previous", post(not_implemented))
        .route("/playback/seek", post(not_implemented))
        .route("/events", get(not_implemented))
        .with_state(state)
}

async fn not_implemented(State(_client): State<AppState>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn stub_client() -> AppState {
        let (client_side, _mpv_side) = tokio::io::duplex(1024);
        Arc::new(MpvClient::new(client_side))
    }

    #[tokio::test]
    async fn stub_router_returns_not_implemented_for_status() {
        let app = router(stub_client());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
