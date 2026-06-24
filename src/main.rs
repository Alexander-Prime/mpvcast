use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, patch, post, put},
};
use std::sync::Arc;
use tokio::sync::RwLock;

mod queue;
mod player;
mod resolver;
mod models;
mod mpv;

use crate::queue::{Queue, QueueItem};
use crate::player::Player;
use crate::resolver::UrlResolver;
use crate::models::{PlaybackStatus, PlaybackStatusUpdate};

/// Shared application state
#[derive(Debug, Clone)]
struct AppState {
    queue: Arc<RwLock<Queue>>,
    player: Arc<RwLock<Option<Player>>>,
    resolver: Arc<UrlResolver>,
    current_index: Arc<RwLock<Option<usize>>>,
}

impl AppState {
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let resolver = UrlResolver::new().await?;

        Ok(AppState {
            queue: Arc::new(RwLock::new(Queue::default())),
            player: Arc::new(RwLock::new(None)),
            resolver: Arc::new(resolver),
            current_index: Arc::new(RwLock::new(None)),
        })
    }

    /// Start playback of a specific queue item
    async fn start_playback(&self, index: usize) -> Result<(), Box<dyn std::error::Error>> {
        let queue = self.queue.read().await;
        let item = queue.get(index).ok_or("Invalid queue index")?;

        tracing::info!("Resolving URL: {}", item.url);

        // Resolve the URL using yt-dlp (though we'll pass original URL to mpv)
        let stream_info = self.resolver.resolve(&item.url).await?;

        tracing::info!("Starting playback: {}", stream_info.title.as_deref().unwrap_or(&item.title));

        // Create or reuse player
        let mut player_lock = self.player.write().await;
        let player = if let Some(ref mut p) = *player_lock {
            p
        } else {
            *player_lock = Some(Player::new()?);
            player_lock.as_mut().unwrap()
        };

        // Start playback with the original URL (mpv handles yt-dlp integration)
        player.play_url(&item.url)?;

        // Update current index
        *self.current_index.write().await = Some(index);

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let state = AppState::new().await.expect("Failed to initialize app state");

    let app = Router::new()
        .route("/queue", put(replace_queue))
        .route("/queue", post(append_queue))
        .route("/status", get(get_status))
        .route("/status", patch(update_status))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();

    println!("Listening on http://0.0.0.0:8080");
    axum::serve(listener, app).await.unwrap();
}

async fn replace_queue(
    State(state): State<AppState>,
    Json(items): Json<Vec<QueueItem>>,
) -> StatusCode {
    tracing::info!("PUT /queue - Replacing queue with {} items", items.len());
    for item in &items {
        tracing::info!("  - {}: {}", item.title, item.url);
    }

    let mut queue = state.queue.write().await;
    queue.replace(items);
    drop(queue); // Release lock before starting playback

    // Start playback on first item
    if let Err(e) = state.start_playback(0).await {
        tracing::error!("Failed to start playback: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    StatusCode::OK
}

async fn append_queue(State(state): State<AppState>, Json(item): Json<QueueItem>) -> StatusCode {
    tracing::info!("POST /queue - Appending: {} ({})", item.title, item.url);

    // Check if queue is empty before adding
    let was_empty = {
        let queue = state.queue.read().await;
        queue.is_empty()
    };

    // Add item to queue
    let mut queue = state.queue.write().await;
    queue.push(item);
    let new_index = queue.len() - 1;
    drop(queue); // Release lock

    // If queue was empty, start playback
    if was_empty {
        if let Err(e) = state.start_playback(new_index).await {
            tracing::error!("Failed to start playback: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }

    StatusCode::OK
}

async fn get_status(State(state): State<AppState>) -> Json<PlaybackStatus> {
    let player_lock = state.player.read().await;
    let current_index = *state.current_index.read().await;

    let (url, title) = if let Some(index) = current_index {
        let queue = state.queue.read().await;
        if let Some(item) = queue.get(index) {
            (Some(item.url.clone()), Some(item.title.clone()))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    if let Some(ref player) = *player_lock {
        let player_status = player.get_status();
        Json(PlaybackStatus {
            paused: player_status.state == crate::player::PlaybackState::Paused,
            position: player_status.position,
            duration: player_status.duration,
            url,
            title,
        })
    } else {
        // No player active
        Json(PlaybackStatus {
            paused: true,
            position: None,
            duration: None,
            url,
            title,
        })
    }
}

async fn update_status(
    State(state): State<AppState>,
    Json(update): Json<PlaybackStatusUpdate>,
) -> StatusCode {
    let player_lock = state.player.read().await;

    if let Some(ref player) = *player_lock {
        // Apply pause/resume
        if let Some(paused) = update.paused {
            let result = if paused {
                player.pause()
            } else {
                player.resume()
            };

            if let Err(e) = result {
                tracing::error!("Failed to update pause state: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }

        // Apply seek
        if let Some(position) = update.position {
            if let Err(e) = player.seek(position) {
                tracing::error!("Failed to seek: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }

        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}
