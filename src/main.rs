use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response, Sse, sse},
    routing::{get, post, put},
};
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

mod api;
mod mpv;

use crate::mpv::client::{MpvClient, MpvError, Seek, SignedDuration};
use crate::mpv::process::MpvProcess;
use crate::mpv::snapshot::StatusSnapshot;
use crate::mpv::ytdl;

type AppState = Arc<MpvClient>;

/// Unassigned by IANA and not in common use, so it's unlikely to collide
/// with anything already running on the machine.
const DEFAULT_PORT: u16 = 4590;

/// Reads `--port <number>` from the arguments, falling back to
/// [`DEFAULT_PORT`]. Any other argument is an error.
fn parse_port(mut args: impl Iterator<Item = String>) -> Result<u16, String> {
    let mut port = DEFAULT_PORT;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                let value = args.next().ok_or("--port needs a port number")?;
                port = value
                    .parse()
                    .map_err(|_| format!("not a port number: {value}"))?;
            }
            // Ignoring it could leave the server quietly listening somewhere
            // other than where a mistyped flag meant to put it.
            other => return Err(format!("unrecognised argument: {other}")),
        }
    }
    Ok(port)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Without this the filter falls back to errors only, which would hide
    // the startup line below.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let port = parse_port(std::env::args().skip(1))?;

    let mpv = MpvProcess::spawn().await?;
    let state = mpv.client();

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("Listening on http://0.0.0.0:{port}");
    axum::serve(listener, app).await?;

    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/style.css", get(stylesheet))
        .route("/app.js", get(script))
        .route("/status", get(get_status).patch(patch_status))
        .route(
            "/queue",
            get(get_queue).post(append_to_queue).delete(clear_queue),
        )
        .route("/playback/play", post(play))
        .route("/playback/pause", post(pause))
        .route("/playback/next", post(next))
        .route("/playback/previous", post(previous))
        .route(
            "/playback/position",
            put(set_position).patch(adjust_position),
        )
        .route("/events", get(events))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../assets/style.css"),
    )
}

async fn script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../assets/app.js"),
    )
}

async fn get_status(State(client): State<AppState>) -> Json<api::Status> {
    Json(api::Status::from(&client.status_snapshot()))
}

async fn get_queue(State(client): State<AppState>) -> Json<Vec<api::Track>> {
    Json(api::queue(&client.status_snapshot()))
}

/// Everything the page renders from, so a change needs no follow-up request.
#[derive(Serialize)]
struct ClientState {
    status: api::Status,
    queue: Vec<api::Track>,
}

impl From<&StatusSnapshot> for ClientState {
    fn from(snapshot: &StatusSnapshot) -> Self {
        Self {
            status: api::Status::from(snapshot),
            queue: api::queue(snapshot),
        }
    }
}

async fn events(
    State(client): State<AppState>,
) -> Sse<impl Stream<Item = Result<sse::Event, axum::Error>>> {
    let stream = stream::unfold(
        (client.subscribe(), true),
        |(mut receiver, opening)| async move {
            // Emit what mpv last reported before waiting, so a client joining
            // mid-playback is not left blank until the next change.
            if !opening && receiver.changed().await.is_err() {
                return None;
            }
            let state = ClientState::from(&*receiver.borrow_and_update());
            Some((sse::Event::default().json_data(state), (receiver, false)))
        },
    );

    Sse::new(stream).keep_alive(sse::KeepAlive::default())
}

#[derive(Deserialize)]
struct StatusPatch {
    volume: Option<u8>,
    muted: Option<bool>,
    max_height: Option<NonZeroU32>,
}

async fn patch_status(
    State(client): State<AppState>,
    Json(patch): Json<StatusPatch>,
) -> Result<StatusCode, ApiError> {
    if let Some(volume) = patch.volume {
        if volume > 100 {
            return Err(ApiError::VolumeOutOfRange);
        }
        client.set_property("volume", volume).await?;
    }
    if let Some(muted) = patch.muted {
        client.set_property("mute", muted).await?;
    }
    if let Some(max_height) = patch.max_height {
        client
            .set_property("ytdl-format", ytdl::format(max_height))
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct QueueItem {
    url: String,
}

/// Adds the URL to the end of the queue. An idle player starts playing it,
/// and an item already playing carries on undisturbed.
async fn append_to_queue(
    State(client): State<AppState>,
    Json(item): Json<QueueItem>,
) -> Result<StatusCode, ApiError> {
    client.loadfile(&item.url).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_queue(State(client): State<AppState>) -> Result<StatusCode, ApiError> {
    client.stop().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn play(State(client): State<AppState>) -> Result<StatusCode, ApiError> {
    let snapshot = client.status_snapshot();
    // Stopping leaves mpv idle with the playlist intact, and clearing the
    // pause flag alone won't restart it — something has to be selected first.
    if api::Status::from(&snapshot).playback.is_none() && !snapshot.playlist.is_empty() {
        client.set_property("playlist-pos", 0).await?;
    }
    client.set_property("pause", false).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pause(State(client): State<AppState>) -> Result<StatusCode, ApiError> {
    client.set_property("pause", true).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn next(State(client): State<AppState>) -> Result<StatusCode, ApiError> {
    client.playlist_next().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn previous(State(client): State<AppState>) -> Result<StatusCode, ApiError> {
    client.playlist_prev().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AbsolutePosition {
    seconds: f64,
}

#[derive(Deserialize)]
struct PositionDelta {
    delta: f64,
}

async fn set_position(
    State(client): State<AppState>,
    Json(body): Json<AbsolutePosition>,
) -> Result<StatusCode, ApiError> {
    apply_seek(
        &client,
        Seek::Absolute(SignedDuration::from_secs_f64(body.seconds)),
    )
    .await
}

async fn adjust_position(
    State(client): State<AppState>,
    Json(body): Json<PositionDelta>,
) -> Result<StatusCode, ApiError> {
    apply_seek(
        &client,
        Seek::Relative(SignedDuration::from_secs_f64(body.delta)),
    )
    .await
}

async fn apply_seek(client: &AppState, seek: Seek) -> Result<StatusCode, ApiError> {
    if api::Status::from(&client.status_snapshot())
        .playback
        .is_none()
    {
        return Err(ApiError::NothingLoaded);
    }
    client.seek(seek).await?;
    Ok(StatusCode::NO_CONTENT)
}

enum ApiError {
    /// The request needs a loaded item and there isn't one.
    NothingLoaded,
    /// A volume outside the 0 to 100 the API declares.
    VolumeOutOfRange,
    /// mpv refused or never answered. The request itself was well formed,
    /// so this reports a failure upstream of the client.
    MpvUnavailable(MpvError),
}

impl From<MpvError> for ApiError {
    fn from(error: MpvError) -> Self {
        Self::MpvUnavailable(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::NothingLoaded => StatusCode::CONFLICT.into_response(),
            Self::VolumeOutOfRange => StatusCode::BAD_REQUEST.into_response(),
            Self::MpvUnavailable(error) => {
                tracing::error!("mpv command failed: {error}");
                StatusCode::BAD_GATEWAY.into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::response::Response;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    fn args<const N: usize>(list: [&str; N]) -> impl Iterator<Item = String> {
        list.into_iter().map(String::from)
    }

    #[test]
    fn no_arguments_uses_the_default_port() {
        assert_eq!(parse_port(args([])).unwrap(), DEFAULT_PORT);
    }

    #[test]
    fn the_port_flag_overrides_the_default() {
        assert_eq!(parse_port(args(["--port", "9000"])).unwrap(), 9000);
    }

    #[test]
    fn the_port_flag_needs_a_value() {
        assert!(parse_port(args(["--port"])).is_err());
    }

    #[test]
    fn a_port_that_is_not_a_number_is_rejected() {
        assert!(parse_port(args(["--port", "http"])).is_err());
    }

    /// Out of u16 range, which is the case a bare integer parse would let
    /// through if the type were wider.
    #[test]
    fn a_port_above_the_valid_range_is_rejected() {
        assert!(parse_port(args(["--port", "70000"])).is_err());
    }

    #[test]
    fn unrecognised_arguments_are_rejected() {
        assert!(parse_port(args(["--verbose"])).is_err());
    }

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

    /// A client mpv has told about one loaded item, paired with a fake mpv
    /// ready to answer `commands` commands.
    async fn loaded_with_mpv(commands: usize) -> (AppState, tokio::task::JoinHandle<Vec<Value>>) {
        let (client_socket, mpv_socket) = tokio::io::duplex(4096);
        let client = Arc::new(MpvClient::new(client_socket));
        let mut status = client.subscribe();
        let mpv = scripted_mpv(
            mpv_socket,
            vec![
                json!({"event": "property-change", "id": 6, "name": "playlist", "data": [
                    {"filename": "https://example.com/a"},
                ]}),
                json!({"event": "property-change", "id": 7, "name": "playlist-pos", "data": 0}),
            ],
            commands,
        );

        tokio::time::timeout(
            Duration::from_secs(1),
            status.wait_for(|snapshot| snapshot.playlist_pos == Some(0)),
        )
        .await
        .expect("timed out waiting for the loaded item")
        .unwrap();

        (client, mpv)
    }

    async fn send_json(state: AppState, method: &str, uri: &str, body: Value) -> Response {
        router(state)
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn send_empty(state: AppState, method: &str, uri: &str) -> Response {
        router(state)
            .oneshot(
                Request::builder()
                    .method(method)
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
                "max_height": null,
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

        let response = send_json(
            client,
            "POST",
            "/queue",
            json!({"url": "https://example.com/a"}),
        )
        .await;
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

        let response = send_empty(client, "POST", "/playback/pause").await;
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

        let response = send_empty(client, "POST", "/playback/play").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![json!(["set_property", "pause", false])]
        );
    }

    async fn text_body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn the_root_serves_the_client_page() {
        let response = get(idle_client(), "/").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert!(text_body(response).await.contains("<title>mpvcast</title>"));
    }

    #[tokio::test]
    async fn the_stylesheet_is_served_as_css() {
        let response = get(idle_client(), "/style.css").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        assert!(text_body(response).await.contains("#scrubber"));
    }

    #[tokio::test]
    async fn the_script_is_served_as_javascript() {
        let response = get(idle_client(), "/app.js").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert!(text_body(response).await.contains("function render"));
    }

    /// mpv's `stop` both ends playback and empties the playlist, where
    /// `playlist-clear` would deliberately spare the current file.
    #[tokio::test]
    async fn deleting_the_queue_stops_playback_and_empties_it() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_empty(client, "DELETE", "/queue").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(mpv.await.unwrap(), vec![json!(["stop"])]);
    }

    #[tokio::test]
    async fn patching_the_volume_sets_it() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_json(client, "PATCH", "/status", json!({"volume": 40})).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![json!(["set_property", "volume", 40])]
        );
    }

    #[tokio::test]
    async fn patching_muted_sets_it() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_json(client, "PATCH", "/status", json!({"muted": true})).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![json!(["set_property", "mute", true])]
        );
    }

    #[tokio::test]
    async fn patching_both_sets_both() {
        let (client, mpv) = loaded_with_mpv(2).await;
        let response = send_json(
            client,
            "PATCH",
            "/status",
            json!({"volume": 0, "muted": false}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![
                json!(["set_property", "volume", 0]),
                json!(["set_property", "mute", false]),
            ]
        );
    }

    /// The API declares 0 to 100, so a value outside it is a malformed
    /// request rather than something to quietly correct.
    #[tokio::test]
    async fn a_volume_above_the_range_is_rejected() {
        let (client, mpv) = loaded_with_mpv(0).await;
        let response = send_json(client, "PATCH", "/status", json!({"volume": 150})).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(mpv.await.unwrap(), Vec::<Value>::new());
    }

    #[tokio::test]
    async fn patching_the_max_height_sets_the_ytdl_format() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_json(client, "PATCH", "/status", json!({"max_height": 720})).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let commands = tokio::time::timeout(Duration::from_secs(1), mpv)
            .await
            .expect("timed out waiting for the set_property command")
            .unwrap();
        assert_eq!(
            commands,
            vec![json!([
                "set_property",
                "ytdl-format",
                ytdl::format(NonZeroU32::new(720).unwrap())
            ])]
        );
    }

    #[tokio::test]
    async fn a_zero_max_height_is_rejected() {
        let (client, mpv) = loaded_with_mpv(0).await;
        let response = send_json(client, "PATCH", "/status", json!({"max_height": 0})).await;
        assert!(
            response.status().is_client_error(),
            "got {}",
            response.status()
        );
        assert_eq!(mpv.await.unwrap(), Vec::<Value>::new());
    }

    #[tokio::test]
    async fn the_events_stream_announces_itself_as_sse() {
        let response = get(idle_client(), "/events").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/event-stream"
        );
    }

    /// A client joining mid-playback needs the current state, not the next
    /// change, so the stream opens with what mpv last reported.
    #[tokio::test]
    async fn the_events_stream_opens_with_the_current_state() {
        let response = get(idle_client(), "/events").await;
        let mut body = response.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(1), body.frame())
            .await
            .expect("timed out waiting for the first event")
            .expect("the stream ended")
            .unwrap();
        let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
        assert!(
            text.contains(r#""playback":null"#),
            "unexpected frame: {text}"
        );
        assert!(text.contains(r#""queue":[]"#), "unexpected frame: {text}");
    }

    #[tokio::test]
    async fn next_advances_the_playlist() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_empty(client, "POST", "/playback/next").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(mpv.await.unwrap(), vec![json!(["playlist-next"])]);
    }

    #[tokio::test]
    async fn previous_steps_back_through_the_playlist() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_empty(client, "POST", "/playback/previous").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(mpv.await.unwrap(), vec![json!(["playlist-prev"])]);
    }

    /// Unlike seeking, these reach mpv whether or not anything is loaded,
    /// and mpv refuses them if it cannot act.
    #[tokio::test]
    async fn next_reaches_mpv_with_nothing_loaded() {
        let (client_socket, mpv_socket) = tokio::io::duplex(4096);
        let client = Arc::new(MpvClient::new(client_socket));
        let mpv = scripted_mpv(mpv_socket, Vec::new(), 1);

        let response = send_empty(client, "POST", "/playback/next").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(mpv.await.unwrap(), vec![json!(["playlist-next"])]);
    }

    #[tokio::test]
    async fn putting_a_position_seeks_to_it() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_json(
            client,
            "PUT",
            "/playback/position",
            json!({"seconds": 30.5}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(mpv.await.unwrap(), vec![json!(["seek", 30.5, "absolute"])]);
    }

    #[tokio::test]
    async fn patching_a_position_seeks_by_the_delta() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_json(
            client,
            "PATCH",
            "/playback/position",
            json!({"delta": -10.0}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(mpv.await.unwrap(), vec![json!(["seek", -10.0, "relative"])]);
    }

    /// The value goes through untouched, and a position past the end
    /// finishes the item.
    #[tokio::test]
    async fn an_out_of_range_position_reaches_mpv_unchanged() {
        let (client, mpv) = loaded_with_mpv(1).await;
        let response = send_json(
            client,
            "PUT",
            "/playback/position",
            json!({"seconds": 99999.0}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            mpv.await.unwrap(),
            vec![json!(["seek", 99999.0, "absolute"])]
        );
    }

    #[tokio::test]
    async fn seeking_with_nothing_loaded_is_a_conflict() {
        let put = send_json(
            idle_client(),
            "PUT",
            "/playback/position",
            json!({"seconds": 5.0}),
        )
        .await;
        assert_eq!(put.status(), StatusCode::CONFLICT);

        let patch = send_json(
            idle_client(),
            "PATCH",
            "/playback/position",
            json!({"delta": 5.0}),
        )
        .await;
        assert_eq!(patch.status(), StatusCode::CONFLICT);
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

        let response = send_empty(client, "POST", "/playback/play").await;
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
