// Passthrough: descriptor shapes, the ffmpeg commands behind them (run for
// real, output checked), and the native ntgcalls accepting encoded sources.
// Needs ffmpeg/ffprobe and the native library, no network or Telegram account.
use std::path::{Path, PathBuf};
use std::process::Command;

use ntgcalls::{Codec, Format, MediaDescription, MediaSource, NTgCalls, StreamMode};
use tgcalls::Media;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
        && Command::new("ffprobe").arg("-version").output().is_ok()
}

fn make_file(dir: &Path, name: &str, args: &[&str]) -> String {
    let path: PathBuf = dir.join(name);
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=15",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "1",
        ])
        .args(args)
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed to make {name}");
    path.to_string_lossy().into_owned()
}

/// Runs a `MediaSource::Shell` command like ntgcalls does and returns stdout.
fn run_shell(cmd: &str) -> Vec<u8> {
    let out = Command::new("sh").args(["-c", cmd]).output().unwrap();
    assert!(
        out.status.success(),
        "{cmd}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tgcalls-pt-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn existing_sources_are_explicitly_raw() {
    // ntgcalls 4 treats a missing format as encoded, so every raw source must say so.
    let a = Media::audio("x.mp3").microphone.unwrap();
    assert_eq!(a.format, Some(Format::Pcm));
    let v = Media::video("x.mp4", 640, 360, 30).camera.unwrap();
    assert_eq!(v.format, Some(Format::I420));
    assert_eq!(
        Media::audio_raw("x.pcm").microphone.unwrap().format,
        Some(Format::Pcm)
    );
    assert_eq!(Media::external_audio(48000, 2).format, Some(Format::Pcm));
    assert_eq!(
        Media::external_video(640, 360, 30).format,
        Some(Format::I420)
    );
    assert_eq!(
        Media::record_audio("o.mp3").microphone.unwrap().format,
        Some(Format::Pcm)
    );
    assert_eq!(Media::screen(640, 360, 30).format, None);
}

#[test]
fn encoded_descriptors_have_the_right_shape() {
    let a = Media::audio_ogg("x.ogg").microphone.unwrap();
    assert_eq!(
        (a.media_source, a.format),
        (MediaSource::File, Some(Format::Ogg))
    );

    let v = Media::video_matroska("x.mkv", 640, 360, 30).camera.unwrap();
    assert_eq!(
        (v.media_source, v.format),
        (MediaSource::File, Some(Format::Matroska))
    );

    let e = Media::external_opus_audio(2);
    assert_eq!(
        (e.media_source, e.format, e.channel_count),
        (MediaSource::External, Some(Format::Ogg), 2)
    );
    let e = Media::external_encoded_video(640, 360, 30);
    assert_eq!(
        (e.media_source, e.format),
        (MediaSource::External, Some(Format::Matroska))
    );

    let r = Media::record_screen_matroska("o.mkv", 640, 360, 30);
    assert!(r.camera.is_none() && r.screen.is_some());

    assert_eq!(Media::opus_frame().codec, Codec::Opus);
    let f = Media::video_frame(Codec::H264, true, 640, 360);
    assert!(f.keyframe && f.codec == Codec::H264 && f.width == 640);
}

#[test]
fn audio_passthrough_copies_opus_and_encodes_everything_else() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tmp("audio");
    let opus = make_file(&dir, "a.mkv", &["-c:v", "libx264", "-c:a", "libopus"]);
    let aac = make_file(&dir, "a.mp4", &["-c:v", "libx264", "-c:a", "aac"]);

    let copy = Media::audio_passthrough(&opus).microphone.unwrap();
    assert!(copy.input.contains("-c:a copy"), "{}", copy.input);
    assert_eq!(copy.format, Some(Format::Ogg));
    assert!(run_shell(&copy.input).starts_with(b"OggS"));

    let enc = Media::audio_passthrough(&aac).microphone.unwrap();
    assert!(enc.input.contains("libopus"), "{}", enc.input);
    assert!(run_shell(&enc.input).starts_with(b"OggS"));
}

#[test]
fn video_passthrough_copies_h264_and_encodes_everything_else() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tmp("video");
    let h264 = make_file(&dir, "v.mp4", &["-c:v", "libx264", "-c:a", "aac"]);
    let mpeg4 = make_file(&dir, "v2.mp4", &["-c:v", "mpeg4", "-c:a", "aac"]);
    let ebml = [0x1A, 0x45, 0xDF, 0xA3];

    let copy = Media::video_passthrough(&h264, 1280, 720, 30)
        .camera
        .unwrap();
    assert!(copy.input.contains("-c:v copy"), "{}", copy.input);
    assert_eq!(copy.format, Some(Format::Matroska));
    assert!(run_shell(&copy.input).starts_with(&ebml));

    let enc = Media::video_passthrough(&mpeg4, 1280, 720, 30)
        .camera
        .unwrap();
    assert!(enc.input.contains("libx264"), "{}", enc.input);
    assert!(run_shell(&enc.input).starts_with(&ebml));
}

#[tokio::test]
async fn native_ntgcalls_accepts_encoded_sources() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tmp("native");
    let src = make_file(&dir, "n.mkv", &["-c:v", "libx264", "-c:a", "libopus"]);

    let ntg = NTgCalls::new();
    ntg.create_call(77).await.unwrap();

    // Shell passthrough (ffmpeg -> Ogg / Matroska) and external encoded.
    let shell = Media::av_passthrough(&src, &src, 320, 240, 15);
    ntg.set_stream_sources(77, StreamMode::Capture, &shell)
        .await
        .unwrap();
    assert!(!ntg.get_state(77).await.unwrap().video_stopped);

    let external = MediaDescription {
        microphone: Some(Media::external_opus_audio(2)),
        speaker: None,
        camera: Some(Media::external_encoded_video(320, 240, 15)),
        screen: None,
    };
    ntg.set_stream_sources(77, StreamMode::Capture, &external)
        .await
        .unwrap();
    assert!(!ntg.get_state(77).await.unwrap().video_stopped);
}
