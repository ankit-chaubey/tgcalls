mod call;
mod error;
mod p2p;
mod signaling;

pub use call::{Call, CallState};
pub use error::TgCallsError;
pub use p2p::{P2PCall, P2PCallState};

pub use ntgcalls::{
    AudioDescription, AuthParams, DhConfig, FrameData, MediaDescription, MediaSegmentPartStatus,
    MediaSource, MediaState, RTCServer, SsrcGroup, StreamDevice, StreamMode, StreamStatus,
    VideoDescription,
};

pub struct Media;

fn audio_cmd(path: &str) -> String {
    format!(
        "ffmpeg -i {path:?} -vn -f s16le -ar 48000 -ac 2 pipe:1",
        path = path
    )
}

fn video_cmd(path: &str, width: i16, height: i16, fps: u8) -> String {
    let w = (width & !1) as u32;
    let h = (height & !1) as u32;
    format!(
        "ffmpeg -i {path:?} -an -f rawvideo -pix_fmt yuv420p \
         -vf scale={w}:{h},fps={fps} pipe:1",
        path = path,
    )
}

impl Media {
    /// Audio from any format ffmpeg understands, decoded to s16le 48kHz stereo PCM.
    pub fn audio(path: impl Into<String>) -> MediaDescription {
        let path = path.into();
        MediaDescription {
            microphone: Some(AudioDescription {
                media_source: MediaSource::Shell,
                sample_rate: 48000,
                channel_count: 2,
                input: audio_cmd(&path),
                keep_open: false,
            }),
            speaker: None,
            camera: None,
            screen: None,
        }
    }

    /// Audio from a raw s16le PCM file (48kHz stereo). Skips ffmpeg entirely.
    pub fn audio_raw(path: impl Into<String>) -> MediaDescription {
        MediaDescription {
            microphone: Some(AudioDescription {
                media_source: MediaSource::File,
                sample_rate: 48000,
                channel_count: 2,
                input: path.into(),
                keep_open: false,
            }),
            speaker: None,
            camera: None,
            screen: None,
        }
    }

    /// Video from any format ffmpeg understands, decoded to raw YUV420p.
    pub fn video(path: impl Into<String>, width: i16, height: i16, fps: u8) -> MediaDescription {
        let path = path.into();
        MediaDescription {
            microphone: None,
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width,
                height,
                fps,
                input: video_cmd(&path, width, height, fps),
                keep_open: false,
            }),
            screen: None,
        }
    }

    /// Audio + video from file(s) via ffmpeg. `audio_path` and `video_path`
    /// can be the same file - two separate ffmpeg processes are spawned.
    pub fn av(
        audio_path: impl Into<String>,
        video_path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let audio_path = audio_path.into();
        let video_path = video_path.into();
        MediaDescription {
            microphone: Some(AudioDescription {
                media_source: MediaSource::Shell,
                sample_rate: 48000,
                channel_count: 2,
                input: audio_cmd(&audio_path),
                keep_open: false,
            }),
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width,
                height,
                fps,
                input: video_cmd(&video_path, width, height, fps),
                keep_open: false,
            }),
            screen: None,
        }
    }

    /// Screen-share presentation source.
    pub fn screen(width: i16, height: i16, fps: u8) -> VideoDescription {
        VideoDescription {
            media_source: MediaSource::Desktop,
            width,
            height,
            fps,
            input: String::new(),
            keep_open: false,
        }
    }

    /// Video source fed entirely via `Call::send_external_frame`.
    pub fn external_video(width: i16, height: i16, fps: u8) -> VideoDescription {
        VideoDescription {
            media_source: MediaSource::External,
            width,
            height,
            fps,
            input: String::new(),
            keep_open: true,
        }
    }

    /// Audio source fed entirely via `Call::send_external_frame`.
    pub fn external_audio(sample_rate: u32, channels: u8) -> AudioDescription {
        AudioDescription {
            media_source: MediaSource::External,
            sample_rate,
            channel_count: channels,
            input: String::new(),
            keep_open: true,
        }
    }
}
