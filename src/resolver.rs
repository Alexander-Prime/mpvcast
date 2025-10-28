use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub enum ResolverError {
    InvalidUrl(String),
}

impl fmt::Display for ResolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolverError::InvalidUrl(msg) => write!(f, "Invalid URL: {}", msg),
        }
    }
}

impl StdError for ResolverError {}

/// Resolved stream information
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub url: String,
    pub title: Option<String>,
    pub duration: Option<f64>,
}

pub struct UrlResolver;

impl fmt::Debug for UrlResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UrlResolver").finish()
    }
}

impl UrlResolver {
    /// Create a new URL resolver
    /// On NixOS, we rely on mpv's built-in yt-dlp support instead of bundling yt-dlp
    pub async fn new() -> Result<Self, ResolverError> {
        Ok(UrlResolver)
    }

    /// Resolve a URL - just validates and returns it
    /// mpv will handle yt-dlp integration internally
    pub async fn resolve(&self, url: &str) -> Result<StreamInfo, ResolverError> {
        // Basic URL validation
        if !self.is_supported(url) {
            return Err(ResolverError::InvalidUrl(
                "URL must start with http:// or https://".to_string(),
            ));
        }

        // For mpv, we pass the original URL directly as mpv has yt-dlp integration
        // mpv will handle the stream extraction internally
        Ok(StreamInfo {
            url: url.to_string(),
            title: None,
            duration: None,
        })
    }

    /// Check if a URL is likely supported (basic validation)
    pub fn is_supported(&self, url: &str) -> bool {
        // Basic URL validation
        url.starts_with("http://") || url.starts_with("https://")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resolver_creation() {
        // Note: This will download yt-dlp and ffmpeg on first run
        let result = UrlResolver::new().await;
        assert!(result.is_ok(), "Should create resolver successfully");
    }

    #[tokio::test]
    async fn test_url_validation() {
        let resolver = UrlResolver::new().await.unwrap();

        assert!(resolver.is_supported("https://www.youtube.com/watch?v=test"));
        assert!(resolver.is_supported("http://example.com/video.mp4"));
        assert!(!resolver.is_supported("ftp://example.com/video.mp4"));
        assert!(!resolver.is_supported("not-a-url"));
    }

    #[tokio::test]
    async fn test_resolve_invalid_url() {
        let resolver = UrlResolver::new().await.unwrap();

        // Should fail for non-http URLs
        let result = resolver.resolve("ftp://example.com/video").await;
        assert!(result.is_err(), "Should fail for non-http(s) URLs");
    }

    #[tokio::test]
    async fn test_resolve_valid_url() {
        let resolver = UrlResolver::new().await.unwrap();

        // Should succeed for valid http(s) URLs
        let result = resolver.resolve("https://www.youtube.com/watch?v=test").await;
        assert!(result.is_ok(), "Should succeed for valid URLs");

        let info = result.unwrap();
        assert_eq!(info.url, "https://www.youtube.com/watch?v=test");
    }

    // Note: We skip testing with real URLs to avoid network dependencies in CI
    // Real URL resolution should be tested manually or in integration tests
}
