//! ffmpeg-backed media sources: files, devices, recording, and the
//! probing/aspect-fit logic behind them. No MTProto or ntgcalls-instance
//! state lives here - see `call.rs`/`signaling.rs` for that.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ntgcalls::{
    AudioDescription, DeviceInfo, MediaDescription, MediaDevices, MediaSource, NTgCalls,
    VideoDescription,
};

use crate::error::TgCallsError;

/// Checks whether `path` has at least one stream of the given ffprobe
/// selector (`"v"` for video, `"a"` for audio). Returns `false` if ffprobe
/// is missing, times out, or the source has no such stream.
fn has_stream(path: &str, selector: &str) -> bool {
    let Ok(child) = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            selector,
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
            path,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return false;
    };

    match run_with_timeout(child, PROBE_TIMEOUT) {
        Some(output) => output.status.success() && !output.stdout.is_empty(),
        None => false,
    }
}

/// Builds a [`MediaDescription`] for `source`, probing it to pick
/// audio-only vs audio+video. `width`/`height`/`fps` bound the video.
pub fn auto_media(source: &str, width: i16, height: i16, fps: u8) -> MediaDescription {
    let has_video = has_stream(source, "v");
    let has_audio = has_stream(source, "a");
    match (has_video, has_audio) {
        (true, true) => Media::av(source, source, width, height, fps),
        (true, false) => Media::video(source, width, height, fps),
        _ => Media::audio(source),
    }
}

/// Like [`auto_media`], starting at `offset` into the source. Used by
/// [`crate::Calls::seek`].
pub fn auto_media_at(
    source: &str,
    offset: Duration,
    width: i16,
    height: i16,
    fps: u8,
) -> MediaDescription {
    let has_video = has_stream(source, "v");
    let has_audio = has_stream(source, "a");
    match (has_video, has_audio) {
        (true, true) => Media::av_at(source, source, offset, width, height, fps),
        (true, false) => Media::video_at(source, offset, width, height, fps),
        _ => Media::audio_at(source, offset),
    }
}

pub struct Media;

/// How long we wait on `ffprobe` before giving up and falling back to the
/// caller-requested dimensions as-is.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

const RECONNECT_FLAGS: &str =
    "-reconnect 1 -reconnect_at_eof 1 -reconnect_streamed 1 -reconnect_delay_max 2";

/// A URL or remote stream needs `-reconnect*` flags, a local file doesn't.
fn needs_reconnect(path: &str) -> bool {
    !Path::new(path).exists()
}

/// Runs `child` to completion, killing it if it overruns `timeout`.
fn run_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Option<std::process::Output> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    child.wait_with_output().ok()
}

/// Reads the source's real width/height via ffprobe. Returns `None` if
/// ffprobe is missing, times out, or the source has no video stream.
/// Total duration of `path`, or `None` if ffprobe fails/times out or the
/// source has no fixed length (a live stream, a device).
pub fn probe_duration(path: &str) -> Option<Duration> {
    let child = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            path,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let output = run_with_timeout(child, PROBE_TIMEOUT)?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let secs: f64 = text.trim().parse().ok()?;
    (secs > 0.0 && secs.is_finite()).then(|| Duration::from_secs_f64(secs))
}

fn probe_dimensions(path: &str) -> Option<(u32, u32)> {
    let child = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=s=x:p=0",
            path,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let output = run_with_timeout(child, PROBE_TIMEOUT)?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|l| l.contains('x'))?;
    let mut parts = line.trim().split('x');
    let w: u32 = parts.next()?.parse().ok()?;
    let h: u32 = parts.next()?.parse().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// Fits `orig` into the `target` box preserving aspect ratio, then rounds
/// both dimensions down to even (yuv420p requires even width/height).
fn fit_even(orig_w: u32, orig_h: u32, target_w: u32, target_h: u32) -> (u32, u32) {
    let ratio = orig_w as f64 / orig_h as f64;
    let mut new_w = target_w.min(orig_w).max(2);
    let mut new_h = (new_w as f64 / ratio).round() as u32;
    if new_h > target_h {
        new_h = target_h.max(2);
        new_w = (new_h as f64 * ratio).round() as u32;
    }
    if !new_w.is_multiple_of(2) {
        new_w -= 1;
    }
    if !new_h.is_multiple_of(2) {
        new_h -= 1;
    }
    (new_w.max(2), new_h.max(2))
}

/// Fits `width`x`height` to the source's real aspect ratio via ffprobe,
/// falling back to the raw (rounded-even) box if probing fails.
fn resolve_dims(path: &str, width: i16, height: i16) -> (u32, u32) {
    let base_w = (width.max(2) as u32) & !1;
    let base_h = (height.max(2) as u32) & !1;
    match probe_dimensions(path) {
        Some((ow, oh)) => fit_even(ow, oh, base_w, base_h),
        None => (base_w, base_h),
    }
}

fn audio_cmd(path: &str, offset: Duration) -> String {
    let reconnect = if needs_reconnect(path) {
        RECONNECT_FLAGS
    } else {
        ""
    };
    let seek = seek_flag(offset);
    format!(
        "ffmpeg -v error {seek}{reconnect} -i {path:?} -vn -f s16le -ar 48000 -ac 2 pipe:1",
        seek = seek,
        reconnect = reconnect,
        path = path,
    )
}

fn video_cmd(path: &str, w: u32, h: u32, fps: u8, offset: Duration) -> String {
    let reconnect = if needs_reconnect(path) {
        RECONNECT_FLAGS
    } else {
        ""
    };
    let seek = seek_flag(offset);
    format!(
        "ffmpeg -v error {seek}{reconnect} -i {path:?} -an -f rawvideo -pix_fmt yuv420p \
         -vf scale={w}:{h},fps={fps} pipe:1",
        seek = seek,
        reconnect = reconnect,
        path = path,
    )
}

/// `-ss` before `-i` is fast, keyframe-based input seeking (as opposed to
/// `-ss` after `-i`, which decodes from the start - slow). Empty for zero
/// offset so the command looks identical to the no-seek case.
fn seek_flag(offset: Duration) -> String {
    if offset.is_zero() {
        String::new()
    } else {
        format!("-ss {:.3} ", offset.as_secs_f64())
    }
}

fn record_audio_cmd(path: &str) -> String {
    format!("ffmpeg -v error -f s16le -ar 48000 -ac 2 -i pipe:0 -y {path:?}")
}

fn record_video_cmd(path: &str, w: u32, h: u32, fps: u8) -> String {
    format!(
        "ffmpeg -v error -f rawvideo -pix_fmt yuv420p -s {w}x{h} -r {fps} -i pipe:0 -y {path:?}"
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
                input: audio_cmd(&path, Duration::ZERO),
                keep_open: false,
            }),
            speaker: None,
            camera: None,
            screen: None,
        }
    }

    /// Like [`Media::audio`], starting playback at `offset` into the source
    /// (fast, keyframe-based - see the module docs on seeking for the
    /// accuracy tradeoff).
    pub fn audio_at(path: impl Into<String>, offset: Duration) -> MediaDescription {
        let path = path.into();
        MediaDescription {
            microphone: Some(AudioDescription {
                media_source: MediaSource::Shell,
                sample_rate: 48000,
                channel_count: 2,
                input: audio_cmd(&path, offset),
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
    /// `width`/`height` is a bounding box - the source is fit into it
    /// preserving real aspect ratio, never stretched.
    pub fn video(path: impl Into<String>, width: i16, height: i16, fps: u8) -> MediaDescription {
        let path = path.into();
        let (w, h) = resolve_dims(&path, width, height);
        MediaDescription {
            microphone: None,
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width: w as i16,
                height: h as i16,
                fps,
                input: video_cmd(&path, w, h, fps, Duration::ZERO),
                keep_open: false,
            }),
            screen: None,
        }
    }

    /// Like [`Media::video`], starting at `offset` into the source.
    pub fn video_at(
        path: impl Into<String>,
        offset: Duration,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let path = path.into();
        let (w, h) = resolve_dims(&path, width, height);
        MediaDescription {
            microphone: None,
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width: w as i16,
                height: h as i16,
                fps,
                input: video_cmd(&path, w, h, fps, offset),
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
        let (w, h) = resolve_dims(&video_path, width, height);
        MediaDescription {
            microphone: Some(AudioDescription {
                media_source: MediaSource::Shell,
                sample_rate: 48000,
                channel_count: 2,
                input: audio_cmd(&audio_path, Duration::ZERO),
                keep_open: false,
            }),
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width: w as i16,
                height: h as i16,
                fps,
                input: video_cmd(&video_path, w, h, fps, Duration::ZERO),
                keep_open: false,
            }),
            screen: None,
        }
    }

    /// Like [`Media::av`], starting both streams at `offset` into the source.
    pub fn av_at(
        audio_path: impl Into<String>,
        video_path: impl Into<String>,
        offset: Duration,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let audio_path = audio_path.into();
        let video_path = video_path.into();
        let (w, h) = resolve_dims(&video_path, width, height);
        MediaDescription {
            microphone: Some(AudioDescription {
                media_source: MediaSource::Shell,
                sample_rate: 48000,
                channel_count: 2,
                input: audio_cmd(&audio_path, offset),
                keep_open: false,
            }),
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width: w as i16,
                height: h as i16,
                fps,
                input: video_cmd(&video_path, w, h, fps, offset),
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

    /// Lists the physical mic/speaker/camera/screen devices ntgcalls can see
    /// on this machine.
    pub fn list_devices() -> Result<MediaDevices, TgCallsError> {
        Ok(NTgCalls::get_media_devices()?)
    }

    /// A physical microphone as the outgoing audio source.
    pub fn microphone(device: &DeviceInfo) -> AudioDescription {
        AudioDescription {
            media_source: MediaSource::Device,
            sample_rate: 48000,
            channel_count: 2,
            input: device.metadata.clone(),
            keep_open: true,
        }
    }

    /// A physical camera as the outgoing video source.
    pub fn camera(device: &DeviceInfo, width: i16, height: i16, fps: u8) -> VideoDescription {
        VideoDescription {
            media_source: MediaSource::Device,
            width,
            height,
            fps,
            input: device.metadata.clone(),
            keep_open: true,
        }
    }

    /// A physical speaker as the destination for the call's incoming audio
    /// (as opposed to recording it to a file - see [`Media::record_audio`]).
    /// Put the result in `MediaDescription.microphone` and pass
    /// `StreamMode::Playback` - the same field used for capture is reused
    /// for the receive direction, same as recording; there's no separate
    /// slot for it.
    pub fn speaker(device: &DeviceInfo) -> AudioDescription {
        AudioDescription {
            media_source: MediaSource::Device,
            sample_rate: 48000,
            channel_count: 2,
            input: device.metadata.clone(),
            keep_open: true,
        }
    }

    /// Records the call's incoming audio to `path` (encoded by ffmpeg from
    /// raw PCM). Pass to `Call::set_stream_sources`/`Call::record` with
    /// `StreamMode::Playback`, not `Capture` - same field, opposite
    /// direction, controlled entirely by the mode you pass.
    pub fn record_audio(path: impl Into<String>) -> MediaDescription {
        let path = path.into();
        MediaDescription {
            microphone: Some(AudioDescription {
                media_source: MediaSource::Shell,
                sample_rate: 48000,
                channel_count: 2,
                input: record_audio_cmd(&path),
                keep_open: false,
            }),
            speaker: None,
            camera: None,
            screen: None,
        }
    }

    /// Records the call's incoming camera video to `path`. Use
    /// [`Media::record_screen`] for an incoming screen-share instead. Same
    /// `StreamMode::Playback` caveat as [`Media::record_audio`].
    pub fn record_video(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let path = path.into();
        let w = (width.max(2) as u32) & !1;
        let h = (height.max(2) as u32) & !1;
        MediaDescription {
            microphone: None,
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width: w as i16,
                height: h as i16,
                fps,
                input: record_video_cmd(&path, w, h, fps),
                keep_open: false,
            }),
            screen: None,
        }
    }

    /// Records an incoming screen-share to `path`, as opposed to camera
    /// video - see [`Media::record_video`].
    pub fn record_screen(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let path = path.into();
        let w = (width.max(2) as u32) & !1;
        let h = (height.max(2) as u32) & !1;
        MediaDescription {
            microphone: None,
            speaker: None,
            camera: None,
            screen: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width: w as i16,
                height: h as i16,
                fps,
                input: record_video_cmd(&path, w, h, fps),
                keep_open: false,
            }),
        }
    }
}
