//! One builder for what to play and at which quality.
//!
//! [`MediaStream`] sits on top of [`crate::Media`]. It picks the route (decode
//! or passthrough), probes the source once, and applies an [`AudioQuality`]
//! and a [`VideoQuality`]. The `Media::*` constructors stay available for
//! anything it does not cover.
//!
//! ```no_run
//! # async fn demo(calls: tgcalls::Calls) -> Result<(), tgcalls::TgCallsError> {
//! use tgcalls::{AudioQuality, MediaStream, VideoQuality};
//!
//! let stream = MediaStream::new("movie.mkv")
//!     .audio_quality(AudioQuality::High)
//!     .video_quality(VideoQuality::Hd720)
//!     .passthrough(true);
//! calls.play_stream(-1001234567890, stream).await?;
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

use ntgcalls::MediaDescription;

use crate::media::{apply_audio_bitrate, plan_stream};

/// Outgoing audio quality, as an Opus bitrate.
///
/// Sample rate and channel count are not part of a preset: audio always goes
/// out as 48 kHz stereo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioQuality {
    /// 32 kbit/s. Speech.
    Voice,
    /// 48 kbit/s.
    Low,
    /// 64 kbit/s. The bitrate passthrough already encodes at.
    Medium,
    /// 128 kbit/s. Music.
    High,
    /// 192 kbit/s.
    Studio,
    /// 510 kbit/s, the most Opus allows.
    Master,
    /// Any bitrate in bits per second. Clamped to 6 to 510 kbit/s.
    Custom(u32),
}

impl AudioQuality {
    /// The bitrate in bits per second.
    pub const fn bitrate(self) -> u32 {
        match self {
            Self::Voice => 32_000,
            Self::Low => 48_000,
            Self::Medium => 64_000,
            Self::High => 128_000,
            Self::Studio => 192_000,
            Self::Master => 510_000,
            Self::Custom(bps) => bps,
        }
    }
}

/// Outgoing video quality: a bounding box and a frame rate.
///
/// The box is fitted to the source's real aspect ratio, never stretched.
/// A passthrough copy is not rescaled at all, so there the values only
/// describe the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoQuality {
    /// 3840x2160 at 60 fps.
    Uhd4k,
    /// 2560x1440 at 60 fps.
    Qhd2k,
    /// 1920x1080 at 60 fps.
    Fhd1080,
    /// 1280x720 at 30 fps. The default.
    #[default]
    Hd720,
    /// 854x480 at 30 fps.
    Sd480,
    /// 640x360 at 30 fps.
    Sd360,
    Custom {
        width: i16,
        height: i16,
        fps: u8,
    },
}

impl VideoQuality {
    /// `(width, height, fps)`.
    pub const fn size(self) -> (i16, i16, u8) {
        match self {
            Self::Uhd4k => (3840, 2160, 60),
            Self::Qhd2k => (2560, 1440, 60),
            Self::Fhd1080 => (1920, 1080, 60),
            Self::Hd720 => (1280, 720, 30),
            Self::Sd480 => (854, 480, 30),
            Self::Sd360 => (640, 360, 30),
            Self::Custom { width, height, fps } => (width, height, fps),
        }
    }
}

/// What to play and how. Build it, then give it to
/// [`Calls::play_stream`](crate::Calls::play_stream), or call
/// [`MediaStream::build`] to get the media for a [`Call`](crate::Call).
///
/// Defaults: both streams the source has, [`VideoQuality::Hd720`], no audio
/// bitrate override, decoded route, starting at zero.
#[derive(Debug, Clone)]
pub struct MediaStream {
    source: String,
    audio: Option<AudioQuality>,
    video: VideoQuality,
    passthrough: bool,
    offset: Duration,
    send_audio: bool,
    send_video: bool,
}

impl MediaStream {
    /// A file path or URL, anything ffmpeg can read.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            audio: None,
            video: VideoQuality::default(),
            passthrough: false,
            offset: Duration::ZERO,
            send_audio: true,
            send_video: true,
        }
    }

    /// Sets the audio bitrate. Without it ntgcalls keeps its own default.
    ///
    /// On the passthrough route it has no effect when the source's Opus is
    /// copied as it is, because nothing is encoded there.
    pub fn audio_quality(mut self, quality: AudioQuality) -> Self {
        self.audio = Some(quality);
        self
    }

    /// Sets the video box and frame rate.
    pub fn video_quality(mut self, quality: VideoQuality) -> Self {
        self.video = quality;
        self
    }

    /// Keeps the media encoded end to end instead of decoding it to raw
    /// frames. See [`Media::audio_passthrough`](crate::Media::audio_passthrough).
    pub fn passthrough(mut self, enabled: bool) -> Self {
        self.passthrough = enabled;
        self
    }

    /// Starts `offset` into the source. Seeking goes through ffmpeg, so a
    /// non-zero offset always leaves the direct, ffmpeg-free route.
    pub fn start_at(mut self, offset: Duration) -> Self {
        self.offset = offset;
        self
    }

    /// Sends the audio and drops the video, even if the source has some.
    pub fn audio_only(mut self) -> Self {
        self.send_audio = true;
        self.send_video = false;
        self
    }

    /// Sends the video and drops the audio, even if the source has some.
    pub fn video_only(mut self) -> Self {
        self.send_audio = false;
        self.send_video = true;
        self
    }

    /// The start offset, zero unless [`MediaStream::start_at`] set one.
    pub fn offset(&self) -> Duration {
        self.offset
    }

    /// Probes the source and returns the media to hand to
    /// [`CallControls::set_sources`](crate::CallControls::set_sources) or
    /// [`Calls::play_media`](crate::Calls::play_media), plus the source's
    /// duration when it has one.
    pub async fn build(&self) -> (MediaDescription, Option<Duration>) {
        let (width, height, fps) = self.video.size();
        let (mut media, duration) = plan_stream(
            &self.source,
            self.offset,
            width,
            height,
            fps,
            self.passthrough,
        )
        .await;

        if !self.send_video {
            media.camera = None;
            media.screen = None;
        }
        if !self.send_audio {
            media.microphone = None;
        }
        if let Some(quality) = self.audio {
            apply_audio_bitrate(&mut media, quality.bitrate());
        }
        (media, duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_presets_rise_with_quality() {
        let order = [
            AudioQuality::Voice,
            AudioQuality::Low,
            AudioQuality::Medium,
            AudioQuality::High,
            AudioQuality::Studio,
            AudioQuality::Master,
        ];
        assert!(order.windows(2).all(|w| w[0].bitrate() < w[1].bitrate()));
        assert_eq!(AudioQuality::Custom(100_000).bitrate(), 100_000);
    }

    #[test]
    fn video_presets_match_their_names() {
        assert_eq!(VideoQuality::Hd720.size(), (1280, 720, 30));
        assert_eq!(VideoQuality::Fhd1080.size(), (1920, 1080, 60));
        assert_eq!(VideoQuality::default(), VideoQuality::Hd720);
        let custom = VideoQuality::Custom {
            width: 400,
            height: 300,
            fps: 24,
        };
        assert_eq!(custom.size(), (400, 300, 24));
    }

    #[test]
    fn builder_keeps_what_it_is_given() {
        let s = MediaStream::new("a.mp3")
            .audio_quality(AudioQuality::High)
            .start_at(Duration::from_secs(5))
            .passthrough(true);
        assert_eq!(s.offset(), Duration::from_secs(5));
        assert_eq!(s.audio, Some(AudioQuality::High));
        assert!(s.passthrough);
    }

    #[tokio::test]
    async fn raw_pcm_route_gets_the_bitrate_and_slot_filters() {
        // A missing file makes the probe find no streams, which plans an
        // audio-only description. That is enough to check the post-steps.
        let s = MediaStream::new("/nonexistent/none.mp3").audio_quality(AudioQuality::Studio);
        let (media, _) = s.build().await;
        assert_eq!(media.microphone.unwrap().bitrate, Some(192_000));

        let (media, _) = MediaStream::new("/nonexistent/none.mp3")
            .video_only()
            .build()
            .await;
        assert!(media.microphone.is_none());
    }

    #[tokio::test]
    async fn passthrough_encode_command_carries_the_bitrate() {
        let (media, _) = MediaStream::new("/nonexistent/none.mp3")
            .passthrough(true)
            .audio_quality(AudioQuality::Custom(96_000))
            .build()
            .await;
        let mic = media.microphone.unwrap();
        assert!(mic.input.contains("-b:a 96000"));
        assert!(!mic.input.contains("-b:a 64k"));
    }

    #[tokio::test]
    async fn bitrate_is_clamped_to_what_opus_accepts() {
        let (media, _) = MediaStream::new("/nonexistent/none.mp3")
            .audio_quality(AudioQuality::Custom(9_000_000))
            .build()
            .await;
        assert_eq!(media.microphone.unwrap().bitrate, Some(510_000));
    }
}
