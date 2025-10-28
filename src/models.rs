use serde::{Deserialize, Serialize};

/// Playback status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackStatus {
    pub paused: bool,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub url: Option<String>,
    pub title: Option<String>,
}

/// Playback status update request
#[derive(Debug, Clone, Deserialize)]
pub struct PlaybackStatusUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playback_status_serialization() {
        let status = PlaybackStatus {
            paused: false,
            position: Some(30.5),
            duration: Some(120.0),
            url: Some("https://example.com/video".to_string()),
            title: Some("Test Video".to_string()),
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"paused\":false"));
        assert!(json.contains("\"position\":30.5"));
    }

    #[test]
    fn test_playback_status_update_deserialization() {
        let json = r#"{"paused":true}"#;
        let update: PlaybackStatusUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.paused, Some(true));
        assert_eq!(update.position, None);

        let json = r#"{"position":45.0}"#;
        let update: PlaybackStatusUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.paused, None);
        assert_eq!(update.position, Some(45.0));

        let json = r#"{"paused":false,"position":10.5}"#;
        let update: PlaybackStatusUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.paused, Some(false));
        assert_eq!(update.position, Some(10.5));
    }
}
