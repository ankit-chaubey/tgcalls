//! ffmpeg-backed media sources: files, devices, recording, and the
//! probing/aspect-fit logic behind them. No MTProto or ntgcalls-instance
//! state lives here - see `call.rs`/`signaling.rs` for that.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ntgcalls::{
    AudioDescription, Codec, DeviceInfo, Format, FrameData, MediaDescription, MediaDevices,
    MediaSource, NTgCalls, VideoDescription, VideoRotation,
};

use crate::error::TgCallsError;
use crate::sniff::{sniff, Mkv, MkvVideo, Sniffed};

/// What one `ffprobe` pass tells us about a source: which stream types are
/// present, and its total duration if it has one. `auto_media`/`auto_media_at`
/// used to need two `has_stream` calls each, plus `Calls::play`/`seek` ran
/// a third probe for duration on top of that - three process spawns for
/// one `play()`. This gets everything in a single call.
struct Probed {
    has_video: bool,
    has_audio: bool,
    /// Codec name of the first audio / video stream (`opus`, `h264`, ...).
    /// Only used to decide whether passthrough can copy the stream as-is.
    audio_codec: Option<String>,
    video_codec: Option<String>,
    duration: Option<Duration>,
}

/// Runs `ffprobe` on `path` and waits on it asynchronously via Tokio's
/// process reactor - no polling loop, no blocking thread. `kill_on_drop`
/// means a timeout actually kills the process instead of leaving it
/// running past our own wait.
async fn probe(path: &str) -> Probed {
    let empty = || Probed {
        has_video: false,
        has_audio: false,
        audio_codec: None,
        video_codec: None,
        duration: None,
    };

    let Ok(child) = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name:format=duration",
            "-of",
            "default=noprint_wrappers=1",
            path,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    else {
        return empty();
    };

    let Ok(Ok(output)) = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await else {
        return empty();
    };
    if !output.status.success() {
        return empty();
    }

    let mut probed = empty();
    // ffprobe prints `codec_name` and `codec_type` for each stream, one per
    // line; collect both before attributing the codec to a stream type.
    let (mut name, mut kind): (Option<String>, Option<String>) = (None, None);
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(v) = line.strip_prefix("codec_name=") {
            name = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("codec_type=") {
            kind = Some(v.trim().to_string());
        } else if let Some(secs) = line
            .strip_prefix("duration=")
            .and_then(|s| s.parse::<f64>().ok())
        {
            if secs.is_finite() && secs > 0.0 {
                probed.duration = Some(Duration::from_secs_f64(secs));
            }
        }
        if let (Some(n), Some(k)) = (&name, &kind) {
            match k.as_str() {
                "video" => {
                    probed.has_video = true;
                    probed.video_codec.get_or_insert_with(|| n.clone());
                }
                "audio" => {
                    probed.has_audio = true;
                    probed.audio_codec.get_or_insert_with(|| n.clone());
                }
                _ => {}
            }
            name = None;
            kind = None;
        }
    }
    probed
}

fn media_for(
    source: &str,
    video: bool,
    audio: bool,
    width: i16,
    height: i16,
    fps: u8,
) -> MediaDescription {
    match (video, audio) {
        (true, true) => Media::av(source, source, width, height, fps),
        (true, false) => Media::video(source, width, height, fps),
        _ => Media::audio(source),
    }
}

fn media_for_at(
    source: &str,
    offset: Duration,
    video: bool,
    audio: bool,
    width: i16,
    height: i16,
    fps: u8,
) -> MediaDescription {
    match (video, audio) {
        (true, true) => Media::av_at(source, source, offset, width, height, fps),
        (true, false) => Media::video_at(source, offset, width, height, fps),
        _ => Media::audio_at(source, offset),
    }
}

/// Builds a [`MediaDescription`] for `source`, probing it to pick
/// audio-only vs audio+video. `width`/`height`/`fps` bound the video.
pub async fn auto_media(source: &str, width: i16, height: i16, fps: u8) -> MediaDescription {
    let probed = probe(source).await;
    media_for(
        source,
        probed.has_video,
        probed.has_audio,
        width,
        height,
        fps,
    )
}

/// Like [`auto_media`], starting at `offset` into the source. Used by
/// [`crate::Calls::seek`].
pub async fn auto_media_at(
    source: &str,
    offset: Duration,
    width: i16,
    height: i16,
    fps: u8,
) -> MediaDescription {
    let probed = probe(source).await;
    media_for_at(
        source,
        offset,
        probed.has_video,
        probed.has_audio,
        width,
        height,
        fps,
    )
}

/// Like [`auto_media`], but returns the probed duration alongside the
/// media - one `ffprobe` pass covers both instead of two separate calls.
/// What [`crate::Calls::play`] actually uses.
pub async fn auto_media_probed(
    source: &str,
    width: i16,
    height: i16,
    fps: u8,
) -> (MediaDescription, Option<Duration>) {
    let probed = probe(source).await;
    (
        media_for(
            source,
            probed.has_video,
            probed.has_audio,
            width,
            height,
            fps,
        ),
        probed.duration,
    )
}

/// Like [`auto_media_at`], but returns the probed duration alongside the
/// media. What [`crate::Calls::seek`] actually uses.
pub async fn auto_media_at_probed(
    source: &str,
    offset: Duration,
    width: i16,
    height: i16,
    fps: u8,
) -> (MediaDescription, Option<Duration>) {
    let probed = probe(source).await;
    (
        media_for_at(
            source,
            offset,
            probed.has_video,
            probed.has_audio,
            width,
            height,
            fps,
        ),
        probed.duration,
    )
}

/// Passthrough: audio codecs Telegram carries as-is. Opus only, and ntgcalls
/// falls back to re-encoding any packet that isn't a compatible Opus frame.
fn can_copy_audio(codec: Option<&str>) -> bool {
    codec == Some("opus")
}

/// Passthrough: video codecs ntgcalls can forward without re-encoding.
fn can_copy_video(codec: Option<&str>) -> bool {
    matches!(codec, Some("h264" | "vp8" | "vp9"))
}

/// ffmpeg -> Ogg on stdout. `copy` remuxes an existing Opus stream
/// untouched; otherwise it is encoded to Opus once, here, instead of being
/// decoded to PCM and encoded again by ntgcalls.
fn audio_passthrough_cmd(path: &str, offset: Duration, copy: bool) -> String {
    let reconnect = if needs_reconnect(path) {
        RECONNECT_FLAGS
    } else {
        ""
    };
    let seek = seek_flag(offset);
    let path = shell_quote(path);
    let codec = if copy {
        "-c:a copy"
    } else {
        "-c:a libopus -ar 48000 -ac 2 -b:a 64k -frame_duration 20"
    };
    format!("ffmpeg -v error {seek}{reconnect} -i {path} -vn {codec} -f ogg pipe:1")
}

/// ffmpeg -> Matroska on stdout. `copy` remuxes the existing H.264 / VP8 /
/// VP9 stream untouched (so `w`/`h`/`fps` are only hints); otherwise it is
/// encoded once to low-latency H.264 here.
fn video_passthrough_cmd(
    path: &str,
    w: u32,
    h: u32,
    fps: u8,
    offset: Duration,
    copy: bool,
) -> String {
    let reconnect = if needs_reconnect(path) {
        RECONNECT_FLAGS
    } else {
        ""
    };
    let seek = seek_flag(offset);
    let path = shell_quote(path);
    let codec = if copy {
        "-c:v copy".to_string()
    } else {
        format!(
            "-c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline \
             -pix_fmt yuv420p -vf scale={w}:{h},fps={fps} -g {} -bf 0",
            u32::from(fps) * 2
        )
    };
    format!("ffmpeg -v error {seek}{reconnect} -i {path} -an {codec} -f matroska -live 1 pipe:1")
}

fn audio_passthrough(path: &str, offset: Duration, codec: Option<&str>) -> AudioDescription {
    AudioDescription {
        media_source: MediaSource::Shell,
        sample_rate: 48000,
        channel_count: 2,
        input: audio_passthrough_cmd(path, offset, can_copy_audio(codec)),
        keep_open: false,
        bitrate: None,
        format: Some(Format::Ogg),
        max_delay_ms: None,
    }
}

fn video_passthrough(
    path: &str,
    width: i16,
    height: i16,
    fps: u8,
    offset: Duration,
    codec: Option<&str>,
) -> VideoDescription {
    let copy = can_copy_video(codec);
    // A copied stream is not rescaled, so it must be declared at its real
    // size; only an encoded one is fit into the requested box.
    let (w, h) = if copy {
        probe_dimensions(path).unwrap_or_else(|| resolve_dims(path, width, height))
    } else {
        resolve_dims(path, width, height)
    };
    VideoDescription {
        media_source: MediaSource::Shell,
        width: w as i16,
        height: h as i16,
        fps,
        input: video_passthrough_cmd(path, w, h, fps, offset, copy),
        keep_open: false,
        format: Some(Format::Matroska),
    }
}

fn media_for_passthrough(
    source: &str,
    offset: Duration,
    probed: &Probed,
    width: i16,
    height: i16,
    fps: u8,
) -> MediaDescription {
    let audio = probed
        .has_audio
        .then(|| audio_passthrough(source, offset, probed.audio_codec.as_deref()));
    let video = probed.has_video.then(|| {
        video_passthrough(
            source,
            width,
            height,
            fps,
            offset,
            probed.video_codec.as_deref(),
        )
    });
    // Same rule as `media_for`: a source with neither stream still gets an
    // audio description, so the failure surfaces from ffmpeg, not from here.
    let audio =
        audio.or_else(|| (video.is_none()).then(|| audio_passthrough(source, offset, None)));
    MediaDescription {
        microphone: audio,
        speaker: None,
        camera: video,
        screen: None,
    }
}

/// Is `ffmpeg` on the `PATH`? Checked once.
fn ffmpeg_available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// An Ogg Opus file read by ntgcalls itself, no ffmpeg involved.
fn ogg_file(path: &str) -> AudioDescription {
    AudioDescription {
        media_source: MediaSource::File,
        sample_rate: 48000,
        channel_count: 2,
        input: path.to_string(),
        keep_open: false,
        bitrate: None,
        format: Some(Format::Ogg),
        max_delay_ms: None,
    }
}

/// A Matroska video read by ntgcalls itself. Size and fps come from the
/// file, because a direct stream is never rescaled; the requested values are
/// only the fallback for files that do not state them.
fn matroska_file(
    path: &str,
    track: &MkvVideo,
    width: i16,
    height: i16,
    fps: u8,
) -> VideoDescription {
    let dim = |own: u32, requested: i16| {
        if own > 0 {
            i16::try_from(own).unwrap_or(i16::MAX)
        } else {
            requested
        }
    };
    let fps = track
        .fps
        .map(|f| f.round().clamp(1.0, 255.0) as u8)
        .unwrap_or(fps);
    VideoDescription {
        media_source: MediaSource::File,
        width: dim(track.width, width),
        height: dim(track.height, height),
        fps,
        input: path.to_string(),
        keep_open: false,
        format: Some(Format::Matroska),
    }
}

/// The video track of `path` if ntgcalls can read it directly.
fn direct_video(path: &str) -> Option<Mkv> {
    match sniff(path)? {
        Sniffed::Matroska(mkv) if mkv.video.as_ref().is_some_and(|t| t.codec.is_some()) => {
            Some(mkv)
        }
        _ => None,
    }
}

/// Everything `Calls::play_passthrough` / `seek_passthrough` need. A local
/// file ntgcalls can read as it is (Ogg Opus, Matroska with H.264 / VP8 /
/// VP9 / AV1) is handed over directly, with no ffmpeg and no ffprobe. A
/// Matroska file's audio is the one part ntgcalls cannot read, so that
/// still goes through an ffmpeg remux when ffmpeg exists. Everything else,
/// and any seek (which needs ffmpeg's `-ss`), takes the ffmpeg route.
async fn passthrough_media(
    source: &str,
    offset: Duration,
    width: i16,
    height: i16,
    fps: u8,
) -> (MediaDescription, Option<Duration>) {
    if offset.is_zero() {
        let owned = source.to_string();
        let sniffed = tokio::task::spawn_blocking(move || sniff(&owned))
            .await
            .ok()
            .flatten();
        match sniffed {
            Some(Sniffed::OggOpus { duration }) => return (Media::audio_ogg(source), duration),
            Some(Sniffed::Matroska(mkv)) => {
                if let Some(track) = mkv.video.as_ref().filter(|t| t.codec.is_some()) {
                    let microphone = match mkv.audio_codec.as_deref() {
                        None => None,
                        Some(codec) if ffmpeg_available() => Some(audio_passthrough(
                            source,
                            Duration::ZERO,
                            (codec == "A_OPUS").then_some("opus"),
                        )),
                        Some(_) => {
                            tracing::warn!(
                                "tgcalls: {source} has audio, but Matroska audio can only be read \
                                 through ffmpeg and ffmpeg was not found; playing video only"
                            );
                            None
                        }
                    };
                    let media = MediaDescription {
                        microphone,
                        speaker: None,
                        camera: Some(matroska_file(source, track, width, height, fps)),
                        screen: None,
                    };
                    return (media, mkv.duration);
                }
            }
            None => {}
        }
    }
    let probed = probe(source).await;
    (
        media_for_passthrough(source, offset, &probed, width, height, fps),
        probed.duration,
    )
}

/// Passthrough twin of [`auto_media_probed`], see `passthrough_media`. What
/// `Calls::play_passthrough` uses.
pub(crate) async fn auto_media_passthrough_probed(
    source: &str,
    width: i16,
    height: i16,
    fps: u8,
) -> (MediaDescription, Option<Duration>) {
    passthrough_media(source, Duration::ZERO, width, height, fps).await
}

/// Like `auto_media_passthrough_probed`, starting at `offset`. What
/// `Calls::seek_passthrough` uses.
pub(crate) async fn auto_media_passthrough_at_probed(
    source: &str,
    offset: Duration,
    width: i16,
    height: i16,
    fps: u8,
) -> (MediaDescription, Option<Duration>) {
    passthrough_media(source, offset, width, height, fps).await
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

/// Total duration of `path`, or `None` if ffprobe fails/times out or the
/// source has no fixed length (a live stream, a device).
pub async fn probe_duration(path: &str) -> Option<Duration> {
    probe(path).await.duration
}

/// Reads the source's real width/height via ffprobe. Returns `None` if
/// ffprobe is missing, times out, or the source has no video stream.
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

/// Quotes `s` for safe interpolation into the shell command string
/// `MediaSource::Shell` runs. ntgcalls executes this through a real shell
/// (`boost::process::shell`, `/bin/sh -c` on POSIX) - Rust's `{:?}` debug
/// formatting only escapes `"`/`\`/control characters, not shell
/// metacharacters, so a path containing `$(...)` or backticks would still
/// run as a subcommand. Single-quoting closes that off: nothing is special
/// inside `'...'` except `'` itself, which gets closed, escaped, and
/// reopened.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn audio_cmd(path: &str, offset: Duration) -> String {
    let reconnect = if needs_reconnect(path) {
        RECONNECT_FLAGS
    } else {
        ""
    };
    let seek = seek_flag(offset);
    let path = shell_quote(path);
    format!("ffmpeg -v error {seek}{reconnect} -i {path} -vn -f s16le -ar 48000 -ac 2 pipe:1")
}

fn video_cmd(path: &str, w: u32, h: u32, fps: u8, offset: Duration) -> String {
    let reconnect = if needs_reconnect(path) {
        RECONNECT_FLAGS
    } else {
        ""
    };
    let seek = seek_flag(offset);
    let path = shell_quote(path);
    format!(
        "ffmpeg -v error {seek}{reconnect} -i {path} -an -f rawvideo -pix_fmt yuv420p \
         -vf scale={w}:{h},fps={fps} pipe:1"
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
    format!(
        "ffmpeg -v error -f s16le -ar 48000 -ac 2 -i pipe:0 -y {}",
        shell_quote(path)
    )
}

fn record_video_cmd(path: &str, w: u32, h: u32, fps: u8) -> String {
    format!(
        "ffmpeg -v error -f rawvideo -pix_fmt yuv420p -s {w}x{h} -r {fps} -i pipe:0 -y {}",
        shell_quote(path)
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
                bitrate: None,
                format: Some(Format::Pcm),
                max_delay_ms: None,
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
                bitrate: None,
                format: Some(Format::Pcm),
                max_delay_ms: None,
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
                bitrate: None,
                format: Some(Format::Pcm),
                max_delay_ms: None,
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
                format: Some(Format::I420),
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
                format: Some(Format::I420),
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
                bitrate: None,
                format: Some(Format::Pcm),
                max_delay_ms: None,
            }),
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width: w as i16,
                height: h as i16,
                fps,
                input: video_cmd(&video_path, w, h, fps, Duration::ZERO),
                keep_open: false,
                format: Some(Format::I420),
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
                bitrate: None,
                format: Some(Format::Pcm),
                max_delay_ms: None,
            }),
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::Shell,
                width: w as i16,
                height: h as i16,
                fps,
                input: video_cmd(&video_path, w, h, fps, offset),
                keep_open: false,
                format: Some(Format::I420),
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
            format: None,
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
            format: Some(Format::I420),
        }
    }

    /// Sets the outgoing audio bitrate in bits per second (Opus encoder
    /// target, e.g. `32_000` for voice, `128_000` for music). Applies to the
    /// microphone of a whole [`MediaDescription`]; a description without a
    /// microphone is returned unchanged. Audio only - ntgcalls has no video
    /// bitrate setting.
    pub fn with_audio_bitrate(
        mut media: MediaDescription,
        bits_per_second: u32,
    ) -> MediaDescription {
        if let Some(mic) = media.microphone.as_mut() {
            mic.bitrate = Some(bits_per_second);
        }
        media
    }

    /// Same as [`Media::with_audio_bitrate`], for a bare [`AudioDescription`]
    /// (what `set_audio` takes).
    pub fn audio_bitrate(mut audio: AudioDescription, bits_per_second: u32) -> AudioDescription {
        audio.bitrate = Some(bits_per_second);
        audio
    }

    /// Audio source fed entirely via `Call::send_external_frame`.
    pub fn external_audio(sample_rate: u32, channels: u8) -> AudioDescription {
        AudioDescription {
            media_source: MediaSource::External,
            sample_rate,
            channel_count: channels,
            input: String::new(),
            keep_open: true,
            bitrate: None,
            format: Some(Format::Pcm),
            max_delay_ms: None,
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
            bitrate: None,
            format: None,
            max_delay_ms: None,
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
            format: None,
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
            bitrate: None,
            format: None,
            max_delay_ms: None,
        }
    }

    /// Records the call's incoming audio to `path`. Pass to `set_sources`
    /// (on any call type) with `StreamMode::Playback`, not `Capture` - same
    /// field, opposite direction, controlled entirely by the mode you pass.
    ///
    /// The extension picks the route: `.ogg` / `.opus` write the received
    /// Opus as it is ([`Media::record_audio_ogg`]) and `.pcm` / `.raw` write
    /// raw PCM ([`Media::record_audio_raw`]), neither needing ffmpeg. Any
    /// other extension (`.mp3`, `.wav`, ...) is encoded by ffmpeg.
    pub fn record_audio(path: impl Into<String>) -> MediaDescription {
        let path = path.into();
        match extension(&path).as_deref() {
            Some("ogg" | "opus") => return Self::record_audio_ogg(path),
            Some("pcm" | "raw") => return Self::record_audio_raw(path),
            _ => {}
        }
        MediaDescription {
            microphone: Some(AudioDescription {
                media_source: MediaSource::Shell,
                sample_rate: 48000,
                channel_count: 2,
                input: record_audio_cmd(&path),
                keep_open: false,
                bitrate: None,
                format: Some(Format::Pcm),
                max_delay_ms: None,
            }),
            speaker: None,
            camera: None,
            screen: None,
        }
    }

    /// Records the call's incoming camera video to `path`. Use
    /// [`Media::record_screen`] for an incoming screen-share instead. Same
    /// `StreamMode::Playback` caveat as [`Media::record_audio`]. A `.mkv` or
    /// `.webm` path writes the received video as it is
    /// ([`Media::record_video_matroska`], no ffmpeg); any other extension is
    /// encoded by ffmpeg.
    pub fn record_video(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let path = path.into();
        if matches!(extension(&path).as_deref(), Some("mkv" | "webm")) {
            return Self::record_video_matroska(path, width, height, fps);
        }
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
                format: Some(Format::I420),
            }),
            screen: None,
        }
    }

    /// Records an incoming screen-share to `path`, as opposed to camera
    /// video - see [`Media::record_video`], including the `.mkv` / `.webm`
    /// shortcut that skips ffmpeg.
    pub fn record_screen(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let path = path.into();
        if matches!(extension(&path).as_deref(), Some("mkv" | "webm")) {
            return Self::record_screen_matroska(path, width, height, fps);
        }
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
                format: Some(Format::I420),
            }),
        }
    }
}

/// Passthrough: send and record already-encoded media without decoding it.
///
/// Normal sources decode to raw PCM / I420 and ntgcalls encodes them again.
/// These keep the media encoded all the way: Opus audio travels as Ogg,
/// H.264 / VP8 / VP9 video as Matroska. That costs a fraction of the CPU and
/// adds no quality loss, which matters on a phone or a small VPS with many
/// calls. Needs ntgcalls 4.0 (the `format` field).
impl Media {
    /// Audio from any file or URL, sent as Opus. An Ogg Opus file goes
    /// straight to ntgcalls with no ffmpeg at all. Another Opus source is
    /// remuxed untouched by ffmpeg; anything else is encoded to Opus once
    /// (48 kHz stereo, 64 kbit/s) instead of PCM-then-Opus.
    pub fn audio_passthrough(path: impl Into<String>) -> MediaDescription {
        let path = path.into();
        if matches!(sniff(&path), Some(Sniffed::OggOpus { .. })) {
            return Self::audio_ogg(path);
        }
        let codec = probe_codec(&path, "a:0");
        MediaDescription {
            microphone: Some(audio_passthrough(&path, Duration::ZERO, codec.as_deref())),
            speaker: None,
            camera: None,
            screen: None,
        }
    }

    /// Video from any file or URL, sent encoded. A Matroska file with
    /// H.264 / VP8 / VP9 / AV1 video goes straight to ntgcalls with no
    /// ffmpeg at all. Other H.264 / VP8 / VP9 sources are remuxed untouched
    /// by ffmpeg (in both cases `width`/`height` only describe the stream,
    /// they do not rescale it); anything else is encoded once to
    /// low-latency H.264 inside the `width`x`height` box.
    pub fn video_passthrough(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let path = path.into();
        if let Some(mkv) = direct_video(&path) {
            if let Some(track) = mkv.video.as_ref() {
                return MediaDescription {
                    microphone: None,
                    speaker: None,
                    camera: Some(matroska_file(&path, track, width, height, fps)),
                    screen: None,
                };
            }
        }
        let codec = probe_codec(&path, "v:0");
        MediaDescription {
            microphone: None,
            speaker: None,
            camera: Some(video_passthrough(
                &path,
                width,
                height,
                fps,
                Duration::ZERO,
                codec.as_deref(),
            )),
            screen: None,
        }
    }

    /// Passthrough audio plus video, from the same or different sources.
    pub fn av_passthrough(
        audio_path: impl Into<String>,
        video_path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let mut media = Self::audio_passthrough(audio_path);
        media.camera = Self::video_passthrough(video_path, width, height, fps).camera;
        media
    }

    /// An Ogg Opus file played directly by ntgcalls: no ffmpeg process at
    /// all. The file must already be Ogg Opus.
    pub fn audio_ogg(path: impl Into<String>) -> MediaDescription {
        MediaDescription {
            microphone: Some(ogg_file(&path.into())),
            speaker: None,
            camera: None,
            screen: None,
        }
    }

    /// A Matroska (`.mkv` / `.webm`) file played directly by ntgcalls: no
    /// ffmpeg process at all. The video must already be H.264, VP8 or VP9;
    /// `width`/`height`/`fps` must match the file.
    pub fn video_matroska(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        MediaDescription {
            microphone: None,
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::File,
                width,
                height,
                fps,
                input: path.into(),
                keep_open: false,
                format: Some(Format::Matroska),
            }),
            screen: None,
        }
    }

    /// Encoded audio you feed yourself with `send_external_frame`: one Opus
    /// packet per call, with [`Media::opus_frame`] as the frame data.
    pub fn external_opus_audio(channels: u8) -> AudioDescription {
        AudioDescription {
            media_source: MediaSource::External,
            sample_rate: 48000,
            channel_count: channels,
            input: String::new(),
            keep_open: true,
            bitrate: None,
            format: Some(Format::Ogg),
            max_delay_ms: None,
        }
    }

    /// Encoded video you feed yourself with `send_external_frame`: one
    /// encoded frame (H.264 / VP8 / VP9) per call, with
    /// [`Media::video_frame`] as the frame data.
    pub fn external_encoded_video(width: i16, height: i16, fps: u8) -> VideoDescription {
        VideoDescription {
            media_source: MediaSource::External,
            width,
            height,
            fps,
            input: String::new(),
            keep_open: true,
            format: Some(Format::Matroska),
        }
    }

    /// Frame data for one encoded Opus packet sent through
    /// [`Media::external_opus_audio`].
    pub fn opus_frame() -> FrameData {
        FrameData {
            absolute_capture_timestamp_ms: 0,
            rotation: VideoRotation::VideoRotation0,
            width: 0,
            height: 0,
            codec: Codec::Opus,
            keyframe: false,
        }
    }

    /// Frame data for one encoded video frame sent through
    /// [`Media::external_encoded_video`]. `keyframe` must be true for
    /// keyframes (IDR for H.264), otherwise receivers cannot start decoding.
    pub fn video_frame(codec: Codec, keyframe: bool, width: u16, height: u16) -> FrameData {
        FrameData {
            absolute_capture_timestamp_ms: 0,
            rotation: VideoRotation::VideoRotation0,
            width,
            height,
            codec,
            keyframe,
        }
    }

    /// Records the incoming call audio to an Ogg Opus file with no decoding
    /// and no ffmpeg: ntgcalls writes the received Opus packets as they
    /// are. Use as playback media, like [`Media::record_audio`].
    pub fn record_audio_ogg(path: impl Into<String>) -> MediaDescription {
        let mut media = Self::audio_ogg(path);
        if let Some(mic) = media.microphone.as_mut() {
            mic.keep_open = false;
        }
        media
    }

    /// Records incoming video to a Matroska file with no decoding and no
    /// ffmpeg. Use as playback media, like [`Media::record_video`].
    pub fn record_video_matroska(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        Self::video_matroska(path, width, height, fps)
    }

    /// Like [`Media::record_video_matroska`], for a screen share.
    pub fn record_screen_matroska(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        let mut media = Self::video_matroska(path, width, height, fps);
        media.screen = media.camera.take();
        media
    }
}

/// Raw frames and tuning, all read or written by ntgcalls itself.
impl Media {
    /// Video from a raw YUV420p (I420) file. Skips ffmpeg entirely; the file
    /// must already be `width`x`height` at `fps`, it is never rescaled.
    pub fn video_raw(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        MediaDescription {
            microphone: None,
            speaker: None,
            camera: Some(VideoDescription {
                media_source: MediaSource::File,
                width,
                height,
                fps,
                input: path.into(),
                keep_open: false,
                format: Some(Format::I420),
            }),
            screen: None,
        }
    }

    /// Records the call's incoming audio to a raw s16le PCM file (48 kHz
    /// stereo), written by ntgcalls with no ffmpeg. Playback media, like
    /// [`Media::record_audio`].
    pub fn record_audio_raw(path: impl Into<String>) -> MediaDescription {
        let mut media = Self::audio_raw(path);
        if let Some(mic) = media.microphone.as_mut() {
            mic.keep_open = false;
        }
        media
    }

    /// Records incoming video to a raw I420 file, written by ntgcalls with
    /// no ffmpeg. Large: about 1.5 bytes per pixel per frame. Playback
    /// media, like [`Media::record_video`].
    pub fn record_video_raw(
        path: impl Into<String>,
        width: i16,
        height: i16,
        fps: u8,
    ) -> MediaDescription {
        Self::video_raw(path, width, height, fps)
    }

    /// For recordings of incoming audio: how long ntgcalls may hold packets
    /// back to put them in order and conceal gaps, in milliseconds
    /// (default 1000). A larger value gives a cleaner recording on a bad
    /// connection, a smaller one a lower delay. Applies to the audio
    /// descriptions in `media`.
    pub fn with_max_delay(mut media: MediaDescription, milliseconds: u32) -> MediaDescription {
        for audio in [media.microphone.as_mut(), media.speaker.as_mut()]
            .into_iter()
            .flatten()
        {
            audio.max_delay_ms = Some(milliseconds);
        }
        media
    }
}

/// Lower-cased extension of `path`, if it has one.
fn extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// First codec name of the stream `select` (`a:0` / `v:0`) in `path`, via a
/// blocking ffprobe. `None` if ffprobe is missing, times out or the source
/// has no such stream, which makes passthrough encode instead of copy.
fn probe_codec(path: &str, select: &str) -> Option<String> {
    let child = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            select,
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
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
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn auto_passthrough_picks_streams_from_the_file() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("tgcalls-auto-pt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.mkv");
        let status = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc=size=320x240:rate=15")
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .args(["-t", "1", "-c:v", "libx264", "-c:a", "libopus"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let both = path.to_string_lossy().into_owned();

        let (media, duration) = auto_media_passthrough_probed(&both, 640, 360, 30).await;
        assert!(media.microphone.is_some() && media.camera.is_some());
        assert!(duration.is_some());

        let (at, _) =
            auto_media_passthrough_at_probed(&both, Duration::from_secs(1), 640, 360, 30).await;
        assert!(at.camera.unwrap().input.contains("-ss 1.000"));
    }

    fn gen(name: &str, args: &[&str]) -> Option<String> {
        let dir = std::env::temp_dir().join(format!("tgcalls-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(name);
        let status = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc=size=320x240:rate=15")
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .args(["-t", "2"])
            .args(args)
            .arg(&path)
            .status()
            .ok()?;
        status
            .success()
            .then(|| path.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn play_passthrough_plans_direct_where_it_can() {
        let (Some(ogg), Some(mkv)) = (
            gen("p.ogg", &["-vn", "-c:a", "libopus"]),
            gen("p.mkv", &["-c:v", "libx264", "-c:a", "libopus"]),
        ) else {
            return;
        };

        // Ogg Opus: audio only, read by ntgcalls itself, duration known.
        let (media, duration) = auto_media_passthrough_probed(&ogg, 1280, 720, 30).await;
        let mic = media.microphone.unwrap();
        assert_eq!(
            (mic.media_source, mic.input.as_str()),
            (MediaSource::File, ogg.as_str())
        );
        assert!(media.camera.is_none() && duration.is_some());

        // Matroska: video direct at its own size, its audio through an
        // ffmpeg copy (ntgcalls cannot read audio out of Matroska).
        let (media, duration) = auto_media_passthrough_probed(&mkv, 1280, 720, 30).await;
        let cam = media.camera.unwrap();
        assert_eq!(
            (cam.media_source, cam.width, cam.height),
            (MediaSource::File, 320, 240)
        );
        let mic = media.microphone.unwrap();
        assert_eq!(mic.media_source, MediaSource::Shell);
        assert!(mic.input.contains("-c:a copy"));
        assert!(duration.is_some());

        // A seek needs ffmpeg's -ss, so it leaves the direct route.
        let (media, _) =
            auto_media_passthrough_at_probed(&mkv, Duration::from_secs(1), 1280, 720, 30).await;
        assert_eq!(media.camera.unwrap().media_source, MediaSource::Shell);
    }
}
