use serde_json::Value;

use crate::mpv::protocol::Event;

const MAX_ERRORS: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub pause: Option<bool>,
    pub time_pos: Option<f64>,
    pub duration: Option<f64>,
    pub volume: Option<f64>,
    pub mute: Option<bool>,
    pub playlist: Vec<PlaylistEntry>,
    pub playlist_pos: Option<i64>,
    pub media_title: Option<String>,
    pub errors: Vec<StatusError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    pub url: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StatusError {
    pub message: String,
}

/// Overwrite `slot`, reporting whether that actually changed the value.
fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
    let changed = *slot != value;
    *slot = value;
    changed
}

impl StatusSnapshot {
    /// Fold an mpv event into the snapshot, reporting whether anything changed.
    /// Subscribers are only woken when this is true, so mpv repeating a value
    /// it has already reported doesn't reach them.
    pub(crate) fn apply_event(&mut self, event: &Event) -> bool {
        match event.event.as_str() {
            "property-change" => {
                let Some(name) = event.extra.get("name").and_then(Value::as_str) else {
                    return false;
                };
                // An absent `data` key means the property is unavailable,
                // the same as an explicit null.
                let data = event.extra.get("data").unwrap_or(&Value::Null);
                self.update_property(name, data)
            }
            "end-file" => {
                let reason = event.extra.get("reason").and_then(Value::as_str);
                if reason != Some("error") {
                    return false;
                }
                let message = event
                    .extra
                    .get("file_error")
                    .and_then(Value::as_str)
                    .unwrap_or("playback error")
                    .to_string();
                self.push_error(message);
                true
            }
            "start-file" => {
                let had_errors = !self.errors.is_empty();
                self.errors.clear();
                had_errors
            }
            _ => false,
        }
    }

    fn update_property(&mut self, name: &str, data: &Value) -> bool {
        match name {
            "pause" => set(&mut self.pause, data.as_bool()),
            "time-pos" => set(&mut self.time_pos, data.as_f64()),
            "duration" => set(&mut self.duration, data.as_f64()),
            "volume" => set(&mut self.volume, data.as_f64()),
            "mute" => set(&mut self.mute, data.as_bool()),
            "playlist-pos" => set(&mut self.playlist_pos, data.as_i64()),
            "media-title" => set(&mut self.media_title, data.as_str().map(String::from)),
            "playlist" => match data.as_array() {
                Some(arr) => {
                    let entries = arr
                        .iter()
                        .filter_map(|entry| {
                            let url = entry.get("filename")?.as_str()?.to_string();
                            let title =
                                entry.get("title").and_then(Value::as_str).map(String::from);
                            Some(PlaylistEntry { url, title })
                        })
                        .collect();
                    set(&mut self.playlist, entries)
                }
                None => false,
            },
            _ => false,
        }
    }

    fn push_error(&mut self, message: String) {
        if self.errors.len() >= MAX_ERRORS {
            self.errors.remove(0);
        }
        self.errors.push(StatusError { message });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(value: Value) -> Event {
        serde_json::from_value(value).unwrap()
    }

    /// A property carrying `"data": null` must clear the field rather than
    /// leave the previous value in place — `position` and `duration` are
    /// specified as null while idle.
    #[test]
    fn null_property_data_clears_the_field() {
        let mut snapshot = StatusSnapshot::default();
        snapshot.apply_event(&event(
            json!({"event": "property-change", "id": 3, "name": "duration", "data": 120.0}),
        ));
        snapshot.apply_event(&event(
            json!({"event": "property-change", "id": 2, "name": "time-pos", "data": 12.5}),
        ));
        assert_eq!(snapshot.duration, Some(120.0));
        assert_eq!(snapshot.time_pos, Some(12.5));

        snapshot.apply_event(&event(
            json!({"event": "property-change", "id": 3, "name": "duration", "data": null}),
        ));
        snapshot.apply_event(&event(
            json!({"event": "property-change", "id": 2, "name": "time-pos", "data": null}),
        ));
        assert_eq!(snapshot.duration, None);
        assert_eq!(snapshot.time_pos, None);
    }

    /// An absent `data` key carries the same meaning as an explicit null, so
    /// the two must be indistinguishable to the snapshot.
    #[test]
    fn absent_property_data_is_treated_as_null() {
        let mut snapshot = StatusSnapshot::default();
        snapshot.apply_event(&event(
            json!({"event": "property-change", "id": 3, "name": "duration", "data": 120.0}),
        ));
        assert_eq!(snapshot.duration, Some(120.0));

        snapshot.apply_event(&event(
            json!({"event": "property-change", "id": 3, "name": "duration"}),
        ));
        assert_eq!(snapshot.duration, None);
    }

    #[test]
    fn errors_are_capped_at_max_dropping_the_oldest() {
        let mut snapshot = StatusSnapshot::default();
        for i in 0..MAX_ERRORS + 2 {
            snapshot.apply_event(&event(json!({
                "event": "end-file",
                "reason": "error",
                "file_error": format!("error {i}"),
            })));
        }

        assert_eq!(snapshot.errors.len(), MAX_ERRORS);
        let messages: Vec<&str> = snapshot.errors.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages[0], "error 2",
            "oldest two should have been dropped"
        );
        assert_eq!(messages[MAX_ERRORS - 1], "error 9");
    }
}
