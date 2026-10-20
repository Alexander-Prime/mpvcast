use serde::Serialize;
use std::num::NonZeroU32;

use crate::mpv::snapshot::StatusSnapshot;
use crate::mpv::ytdl;

/// A queue entry, and the shape of the currently playing item. `title` comes
/// from media metadata and stays `None` until mpv has resolved it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Track {
    pub url: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorEntry {
    pub message: String,
}

/// The loaded item and everything that only means something while one is
/// loaded. Null while mpv is idle, so neither the type nor a client reading
/// the JSON can pick up a position belonging to a track that has ended.
///
/// `position` and `duration` stay optional even here: mpv reports a file as
/// loaded before it has resolved either, and duration never resolves at all
/// for a live stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Playback {
    pub paused: bool,
    pub current: Track,
    pub current_index: usize,
    pub position: Option<f64>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub playback: Option<Playback>,
    pub volume: u8,
    pub muted: bool,
    /// The video height cap for the next item mpv loads. Null when mpv's
    /// format was not set through mpvcast, so there is no cap to report.
    pub max_height: Option<NonZeroU32>,
    pub errors: Vec<ErrorEntry>,
}

impl From<&StatusSnapshot> for Status {
    fn from(snapshot: &StatusSnapshot) -> Self {
        // mpv reports a playlist position of -1 while idle, and reports pause
        // as false there too, so whether an item is actually loaded is what
        // decides whether there is any playback to report.
        let index = usize::try_from(snapshot.playlist_pos.unwrap_or(-1)).ok();
        let playback = index
            .and_then(|i| snapshot.playlist.get(i).map(|entry| (i, entry)))
            .map(|(index, entry)| Playback {
                paused: snapshot.pause.unwrap_or(false),
                current: Track {
                    url: entry.url.clone(),
                    title: snapshot.media_title.clone(),
                },
                current_index: index,
                position: snapshot.time_pos,
                duration: snapshot.duration,
            });

        Self {
            playback,
            volume: volume(snapshot.volume),
            muted: snapshot.mute.unwrap_or(false),
            max_height: snapshot.ytdl_format.as_deref().and_then(ytdl::max_height),
            errors: snapshot
                .errors
                .iter()
                .map(|error| ErrorEntry {
                    message: error.message.clone(),
                })
                .collect(),
        }
    }
}

/// mpv's own volume range runs past 100, so the API caps it. The fallback
/// covers only the window between connecting and mpv's first report.
fn volume(reported: Option<f64>) -> u8 {
    reported.unwrap_or(100.0).clamp(0.0, 100.0).round() as u8
}

pub fn queue(snapshot: &StatusSnapshot) -> Vec<Track> {
    snapshot
        .playlist
        .iter()
        .map(|entry| Track {
            url: entry.url.clone(),
            title: entry.title.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpv::snapshot::{PlaylistEntry, StatusError};
    use serde_json::json;

    fn entry(url: &str, title: Option<&str>) -> PlaylistEntry {
        PlaylistEntry {
            url: url.to_string(),
            title: title.map(String::from),
        }
    }

    /// mpv reports position -1 while idle, and reports pause as false, so
    /// neither the presence of a position nor the pause flag can stand in for
    /// "something is loaded".
    fn loaded_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            pause: Some(false),
            time_pos: Some(12.5),
            duration: Some(300.0),
            volume: Some(80.0),
            mute: Some(false),
            playlist: vec![
                entry("https://example.com/a", Some("A")),
                entry("https://example.com/b", None),
            ],
            playlist_pos: Some(1),
            media_title: Some("Resolved Title".to_string()),
            ytdl_format: Some(ytdl::format(NonZeroU32::new(1080).unwrap())),
            errors: Vec::new(),
        }
    }

    #[test]
    fn empty_snapshot_is_idle() {
        let status = Status::from(&StatusSnapshot::default());
        assert_eq!(status.playback, None);
        assert!(!status.muted);
        assert!(status.errors.is_empty());
    }

    #[test]
    fn negative_playlist_pos_is_idle_even_with_a_queue() {
        let status = Status::from(&StatusSnapshot {
            playlist_pos: Some(-1),
            ..loaded_snapshot()
        });
        assert_eq!(status.playback, None, "nothing is loaded at position -1");
    }

    #[test]
    fn loaded_snapshot_carries_the_current_track() {
        let status = Status::from(&loaded_snapshot());
        assert_eq!(
            status.playback,
            Some(Playback {
                paused: false,
                current: Track {
                    url: "https://example.com/b".to_string(),
                    title: Some("Resolved Title".to_string()),
                },
                current_index: 1,
                position: Some(12.5),
                duration: Some(300.0),
            }),
            "current title comes from media metadata, not the playlist entry"
        );
    }

    #[test]
    fn a_paused_snapshot_reports_paused_playback() {
        let status = Status::from(&StatusSnapshot {
            pause: Some(true),
            ..loaded_snapshot()
        });
        assert!(status.playback.expect("something is loaded").paused);
    }

    /// mpv reports a file as loaded before it reports the pause flag, and it
    /// starts a file playing, so that window reads as playing rather than
    /// paused.
    #[test]
    fn loaded_without_a_reported_pause_flag_is_not_paused() {
        let status = Status::from(&StatusSnapshot {
            pause: None,
            ..loaded_snapshot()
        });
        assert!(!status.playback.expect("something is loaded").paused);
    }

    #[test]
    fn volume_is_clamped_and_rounded() {
        for (raw, expected) in [(130.0, 100), (42.7, 43), (0.4, 0), (-5.0, 0)] {
            let snapshot = StatusSnapshot {
                volume: Some(raw),
                ..loaded_snapshot()
            };
            assert_eq!(
                Status::from(&snapshot).volume,
                expected,
                "volume {raw} should map to {expected}"
            );
        }
    }

    #[test]
    fn a_format_built_by_mpvcast_reports_its_height_cap() {
        let snapshot = StatusSnapshot {
            ytdl_format: Some(ytdl::format(NonZeroU32::new(720).unwrap())),
            ..loaded_snapshot()
        };
        assert_eq!(Status::from(&snapshot).max_height, NonZeroU32::new(720));
    }

    #[test]
    fn any_other_format_reports_no_height_cap() {
        for ytdl_format in [None, Some("best".to_string())] {
            let snapshot = StatusSnapshot {
                ytdl_format,
                ..loaded_snapshot()
            };
            assert_eq!(Status::from(&snapshot).max_height, None);
        }
    }

    #[test]
    fn errors_are_carried_through() {
        let snapshot = StatusSnapshot {
            errors: vec![StatusError {
                message: "connection refused".to_string(),
            }],
            ..loaded_snapshot()
        };
        let status = Status::from(&snapshot);
        assert_eq!(
            status.errors,
            vec![ErrorEntry {
                message: "connection refused".to_string()
            }]
        );
    }

    #[test]
    fn queue_lists_playlist_entries_with_unresolved_titles() {
        assert_eq!(
            queue(&loaded_snapshot()),
            vec![
                Track {
                    url: "https://example.com/a".to_string(),
                    title: Some("A".to_string()),
                },
                Track {
                    url: "https://example.com/b".to_string(),
                    title: None,
                },
            ]
        );
    }

    #[test]
    fn status_serializes_to_the_documented_shape() {
        let status = Status::from(&loaded_snapshot());
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            json!({
                "playback": {
                    "paused": false,
                    "current": {"url": "https://example.com/b", "title": "Resolved Title"},
                    "current_index": 1,
                    "position": 12.5,
                    "duration": 300.0,
                },
                "volume": 80,
                "muted": false,
                "max_height": 1080,
                "errors": [],
            })
        );
    }

    /// Idle is spelled as an explicit null rather than by dropping the key,
    /// so a client can tell "nothing loaded" from "field I forgot to send".
    #[test]
    fn idle_status_serializes_playback_as_null() {
        let status = Status::from(&StatusSnapshot::default());
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            json!({
                "playback": null,
                "volume": 100,
                "muted": false,
                "max_height": null,
                "errors": [],
            })
        );
    }
}
