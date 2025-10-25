use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{post, put},
};
use std::sync::Arc;
use tokio::sync::RwLock;

mod queue;

use crate::queue::{Queue, QueueItem};

#[derive(Debug, Clone, Default)]
struct AppState {
    queue: Arc<RwLock<Queue>>,
}

#[tokio::main]
async fn main() {
    let state = AppState::default();

    let app = Router::new()
        .route("/queue", put(replace_queue))
        .route("/queue", post(append_queue))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();

    println!("Listening on http://0.0.0.0:8080");
    axum::serve(listener, app).await.unwrap();
}

async fn replace_queue(
    State(state): State<AppState>,
    Json(items): Json<Vec<QueueItem>>,
) -> StatusCode {
    println!("PUT /queue - Replacing queue with {} items", items.len());
    for item in &items {
        println!("  - {}: {}", item.title, item.url);
    }

    let mut queue = state.queue.write().await;
    queue.replace(items);

    StatusCode::OK
}

async fn append_queue(State(state): State<AppState>, Json(item): Json<QueueItem>) -> StatusCode {
    println!("POST /queue - Appending: {} ({})", item.title, item.url);

    let mut queue = state.queue.write().await;
    queue.push(item);

    StatusCode::OK
}
