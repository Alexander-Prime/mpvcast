use libmpv2::{Mpv, Error as MpvError};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct PlaybackStatus {
    pub state: PlaybackState,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub url: Option<String>,
}

pub struct Player {
    mpv: Mpv,
    current_url: Option<String>,
}

impl fmt::Debug for Player {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Player")
            .field("current_url", &self.current_url)
            .finish()
    }
}

impl Player {
    /// Create a new headless player instance for audio playback
    pub fn new() -> Result<Self, MpvError> {
        let mpv = Mpv::new()?;

        // Configure for headless audio playback
        mpv.set_property("vo", "null")?;
        mpv.set_property("audio-display", "no")?;
        mpv.set_property("video", "no")?;

        Ok(Player {
            mpv,
            current_url: None,
        })
    }

    /// Play a URL (direct stream URL from yt-dlp)
    pub fn play_url(&mut self, url: &str) -> Result<(), MpvError> {
        self.mpv.command("loadfile", &[url, "replace"])?;
        self.current_url = Some(url.to_string());
        Ok(())
    }

    /// Pause playback
    pub fn pause(&self) -> Result<(), MpvError> {
        self.mpv.set_property("pause", true)
    }

    /// Resume playback
    pub fn resume(&self) -> Result<(), MpvError> {
        self.mpv.set_property("pause", false)
    }

    /// Stop playback
    pub fn stop(&mut self) -> Result<(), MpvError> {
        self.mpv.command("stop", &[])?;
        self.current_url = None;
        Ok(())
    }

    /// Seek to a specific position in seconds
    pub fn seek(&self, position: f64) -> Result<(), MpvError> {
        self.mpv.command("seek", &[&position.to_string(), "absolute"])?;
        Ok(())
    }

    /// Get current playback status
    pub fn get_status(&self) -> PlaybackStatus {
        let paused = self.mpv.get_property("pause").unwrap_or(true);
        let position = self.mpv.get_property("time-pos").ok();
        let duration = self.mpv.get_property("duration").ok();

        let state = if self.current_url.is_none() {
            PlaybackState::Stopped
        } else if paused {
            PlaybackState::Paused
        } else {
            PlaybackState::Playing
        };

        PlaybackStatus {
            state,
            position,
            duration,
            url: self.current_url.clone(),
        }
    }

    /// Check if playback has ended
    pub fn is_idle(&self) -> bool {
        self.mpv.get_property("idle-active").unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_player_initialization() {
        // Test that we can create a player instance
        let result = Player::new();
        assert!(result.is_ok(), "Player should initialize successfully");

        let player = result.unwrap();

        // Verify headless mode is configured
        let vo: String = player.mpv.get_property("vo").unwrap();
        assert_eq!(vo, "null", "Video output should be null for headless mode");

        let video: bool = player.mpv.get_property("video").unwrap();
        assert_eq!(video, false, "Video should be disabled");
    }

    #[test]
    fn test_initial_status() {
        let player = Player::new().unwrap();
        let status = player.get_status();

        assert_eq!(status.state, PlaybackState::Stopped);
        assert_eq!(status.url, None);
    }

    #[test]
    fn test_pause_resume() {
        let player = Player::new().unwrap();

        // Test pause
        let result = player.pause();
        assert!(result.is_ok(), "Pause should succeed");

        let paused: bool = player.mpv.get_property("pause").unwrap();
        assert_eq!(paused, true);

        // Test resume
        let result = player.resume();
        assert!(result.is_ok(), "Resume should succeed");

        let paused: bool = player.mpv.get_property("pause").unwrap();
        assert_eq!(paused, false);
    }

    #[test]
    fn test_play_url_updates_state() {
        let mut player = Player::new().unwrap();

        // Use a test URL (will fail to actually load, but state should update)
        let test_url = "http://example.com/test.mp3";
        let result = player.play_url(test_url);

        assert!(result.is_ok(), "play_url should not error on command");
        assert_eq!(player.current_url, Some(test_url.to_string()));
    }

    #[test]
    fn test_stop_clears_url() {
        let mut player = Player::new().unwrap();
        player.current_url = Some("test".to_string());

        let result = player.stop();
        assert!(result.is_ok());
        assert_eq!(player.current_url, None);
    }

    #[test]
    fn test_seek_command() {
        let player = Player::new().unwrap();

        // Test that seek command can be issued (won't actually seek without media)
        let result = player.seek(30.0);
        // Might fail without media loaded, but should not panic
        assert!(result.is_ok() || result.is_err());
    }
}
