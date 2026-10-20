use std::num::NonZeroU32;

/// The yt-dlp format selector for the best H.264 video no taller than
/// `max_height`, with the best audio alongside.
pub fn format(max_height: NonZeroU32) -> String {
    // H.264 because it is what the playback machine decodes in hardware. The
    // `?` admits formats whose site doesn't report a height or codec, rather
    // than failing to play them at all.
    let filter = format!("[height<=?{max_height}][vcodec^=?avc1]");
    format!("bestvideo{filter}+bestaudio/best{filter}")
}

/// The height cap of a selector built by [`format`], or `None` for any
/// selector it would not have built.
pub fn max_height(selector: &str) -> Option<NonZeroU32> {
    let (height, _) = selector
        .strip_prefix("bestvideo[height<=?")?
        .split_once(']')?;
    let height = height.parse().ok()?;
    (format(height) == selector).then_some(height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn height(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    #[test]
    fn the_format_caps_height_and_requires_h264() {
        assert_eq!(
            format(height(720)),
            "bestvideo[height<=?720][vcodec^=?avc1]+bestaudio/best[height<=?720][vcodec^=?avc1]"
        );
    }

    #[test]
    fn a_built_format_reads_back_as_its_height() {
        for value in [360, 1080, 2160] {
            assert_eq!(max_height(&format(height(value))), Some(height(value)));
        }
    }

    /// Anything else in mpv's format, such as an mpv.conf value or the
    /// earlier codec-free default, has no height cap the API can report.
    #[test]
    fn a_format_mpvcast_did_not_build_has_no_height() {
        for other in [
            "",
            "best",
            "bestvideo[height<=?1080]+bestaudio/best",
            "bestvideo[height<=?0][vcodec^=?avc1]+bestaudio/best[height<=?0][vcodec^=?avc1]",
            "bestvideo[height<=?720][vcodec^=?avc1]+bestaudio/best[height<=?480][vcodec^=?avc1]",
        ] {
            assert_eq!(max_height(other), None, "{other:?} should not parse");
        }
    }
}
