//! Looks inside a local file, in pure Rust, to see whether ntgcalls can read
//! it directly. That is how passthrough knows it can skip ffmpeg and ffprobe
//! entirely: an Ogg Opus file or a Matroska file with H.264 / VP8 / VP9 / AV1
//! video goes straight to ntgcalls, and everything else falls back to ffmpeg.
//!
//! Everything here is best effort and never panics on bad input: anything it
//! cannot make sense of is reported as "unknown", which just means "use
//! ffmpeg".

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use ntgcalls::Codec;

/// How much of the start of a file is read. Matroska keeps its track list
/// near the start; 1 MiB leaves room for padding and cover art before it.
const HEAD_BYTES: u64 = 1 << 20;
/// How much of the end of an Ogg file is read to find its last page.
const TAIL_BYTES: u64 = 1 << 16;

/// The video track of a Matroska file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MkvVideo {
    /// `None` when the codec is not one ntgcalls can forward as is.
    pub codec: Option<Codec>,
    pub width: u32,
    pub height: u32,
    /// From the track's default frame duration, when the file states one.
    pub fps: Option<f64>,
}

/// What a Matroska file contains.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Mkv {
    pub video: Option<MkvVideo>,
    /// Matroska codec id of the first audio track (`A_OPUS`, `A_AAC`, ...).
    pub audio_codec: Option<String>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Sniffed {
    /// An Ogg file holding Opus audio, which ntgcalls plays directly.
    OggOpus {
        duration: Option<Duration>,
    },
    Matroska(Mkv),
}

/// Sniffs a local file. `None` for URLs, missing files and anything that is
/// neither Ogg Opus nor Matroska.
pub(crate) fn sniff(path: &str) -> Option<Sniffed> {
    let path = Path::new(path);
    if !path.is_file() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut head = Vec::new();
    (&mut file).take(HEAD_BYTES).read_to_end(&mut head).ok()?;

    if head.starts_with(b"OggS") {
        return ogg_opus(&mut file, &head, len);
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return parse_matroska(&head).map(Sniffed::Matroska);
    }
    None
}

fn ogg_opus(file: &mut File, head: &[u8], len: u64) -> Option<Sniffed> {
    // The first page carries the "OpusHead" packet right after its header.
    let first = &head[..head.len().min(512)];
    if !first.windows(8).any(|w| w == b"OpusHead") {
        return None;
    }
    Some(Sniffed::OggOpus {
        duration: ogg_duration(file, len),
    })
}

/// Duration from the granule position of the last page (48 kHz samples).
fn ogg_duration(file: &mut File, len: u64) -> Option<Duration> {
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut tail).ok()?;
    let at = tail.windows(4).rposition(|w| w == b"OggS")?;
    let granule = u64::from_le_bytes(tail.get(at + 6..at + 14)?.try_into().ok()?);
    if granule == u64::MAX || granule == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(granule as f64 / 48_000.0))
}

const ID_SEGMENT: u32 = 0x1853_8067;
const ID_INFO: u32 = 0x1549_A966;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_TIMECODE_SCALE: u32 = 0x2A_D7B1;
const ID_DURATION: u32 = 0x4489;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC_ID: u32 = 0x86;
const ID_DEFAULT_DURATION: u32 = 0x23_E383;
const ID_VIDEO: u32 = 0xE0;
const ID_PIXEL_WIDTH: u32 = 0xB0;
const ID_PIXEL_HEIGHT: u32 = 0xBA;

/// Element id: the length marker stays part of the value, as in the spec.
fn read_id(buf: &[u8], pos: usize) -> Option<(u32, usize)> {
    let first = *buf.get(pos)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return None;
    }
    let bytes = buf.get(pos..pos + len)?;
    Some((
        bytes.iter().fold(0u32, |v, b| (v << 8) | u32::from(*b)),
        len,
    ))
}

/// Element size: the length marker is stripped. `None` value means unknown
/// size (all value bits set), used by live and streamed files.
fn read_size(buf: &[u8], pos: usize) -> Option<(Option<u64>, usize)> {
    let first = *buf.get(pos)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let bytes = buf.get(pos..pos + len)?;
    let mut value = u64::from(first) & ((1u64 << (8 - len)) - 1);
    for b in &bytes[1..] {
        value = (value << 8) | u64::from(*b);
    }
    let all_ones = (1u64 << (7 * len)) - 1;
    Some((if value == all_ones { None } else { Some(value) }, len))
}

fn read_uint(data: &[u8]) -> u64 {
    data.iter()
        .take(8)
        .fold(0u64, |v, b| (v << 8) | u64::from(*b))
}

fn read_float(data: &[u8]) -> Option<f64> {
    match data.len() {
        4 => Some(f64::from(f32::from_be_bytes(data.try_into().ok()?))),
        8 => Some(f64::from_be_bytes(data.try_into().ok()?)),
        _ => None,
    }
}

/// Calls `f(id, payload)` for each element in `buf`. Stops at the first
/// element that is cut off or has an unknown size.
fn for_each_element(buf: &[u8], mut f: impl FnMut(u32, &[u8])) {
    let mut pos = 0;
    while pos < buf.len() {
        let Some((id, id_len)) = read_id(buf, pos) else {
            return;
        };
        let Some((Some(size), size_len)) = read_size(buf, pos + id_len) else {
            return;
        };
        let start = pos + id_len + size_len;
        let Some(end) = usize::try_from(size)
            .ok()
            .and_then(|s| start.checked_add(s))
            .filter(|end| *end <= buf.len())
        else {
            return;
        };
        f(id, &buf[start..end]);
        pos = end;
    }
}

fn codec_from_id(id: &str) -> Option<Codec> {
    match id {
        "V_MPEG4/ISO/AVC" => Some(Codec::H264),
        "V_VP8" => Some(Codec::Vp8),
        "V_VP9" => Some(Codec::Vp9),
        "V_AV1" => Some(Codec::Av1),
        _ => None,
    }
}

fn parse_track(entry: &[u8], mkv: &mut Mkv) {
    let (mut kind, mut codec_id, mut default_ns) = (0u64, String::new(), 0u64);
    let (mut width, mut height) = (0u64, 0u64);
    for_each_element(entry, |id, data| match id {
        ID_TRACK_TYPE => kind = read_uint(data),
        ID_CODEC_ID => codec_id = String::from_utf8_lossy(data).into_owned(),
        ID_DEFAULT_DURATION => default_ns = read_uint(data),
        ID_VIDEO => for_each_element(data, |id, data| match id {
            ID_PIXEL_WIDTH => width = read_uint(data),
            ID_PIXEL_HEIGHT => height = read_uint(data),
            _ => {}
        }),
        _ => {}
    });
    match kind {
        // Only the first video / audio track counts, like ntgcalls.
        1 if mkv.video.is_none() => {
            mkv.video = Some(MkvVideo {
                codec: codec_from_id(&codec_id),
                width: width.min(u64::from(u32::MAX)) as u32,
                height: height.min(u64::from(u32::MAX)) as u32,
                fps: (default_ns > 0).then(|| 1e9 / default_ns as f64),
            });
        }
        2 if mkv.audio_codec.is_none() => mkv.audio_codec = Some(codec_id),
        _ => {}
    }
}

fn parse_info(info: &[u8]) -> Option<Duration> {
    let (mut scale, mut duration) = (1_000_000u64, None);
    for_each_element(info, |id, data| match id {
        ID_TIMECODE_SCALE => scale = read_uint(data),
        ID_DURATION => duration = read_float(data),
        _ => {}
    });
    let ns = duration? * scale as f64;
    (ns.is_finite() && ns > 0.0).then(|| Duration::from_secs_f64(ns / 1e9))
}

fn parse_matroska(buf: &[u8]) -> Option<Mkv> {
    let mut mkv = Mkv {
        video: None,
        audio_codec: None,
        duration: None,
    };
    let mut found_tracks = false;
    let mut pos = 0;
    while pos < buf.len() {
        let (id, id_len) = read_id(buf, pos)?;
        let (size, size_len) = read_size(buf, pos + id_len)?;
        let start = pos + id_len + size_len;
        match id {
            // The segment's children follow its header directly, whatever
            // its size says (live files use an unknown one).
            ID_SEGMENT => {
                pos = start;
                continue;
            }
            // Media starts here; the tracks must have come before.
            ID_CLUSTER => break,
            _ => {}
        }
        let end = usize::try_from(size?)
            .ok()
            .and_then(|s| start.checked_add(s))
            .filter(|end| *end <= buf.len());
        match (id, end) {
            (ID_TRACKS, Some(end)) => {
                for_each_element(&buf[start..end], |id, data| {
                    if id == ID_TRACK_ENTRY {
                        parse_track(data, &mut mkv);
                    }
                });
                found_tracks = true;
            }
            (ID_INFO, Some(end)) => mkv.duration = parse_info(&buf[start..end]),
            (_, Some(_)) => {}
            // Cut off by the end of what was read.
            (_, None) => break,
        }
        pos = end?;
    }
    found_tracks.then_some(mkv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_ids_and_sizes_decode() {
        assert_eq!(
            read_id(&[0x1A, 0x45, 0xDF, 0xA3], 0),
            Some((0x1A45_DFA3, 4))
        );
        assert_eq!(read_id(&[0xAE], 0), Some((ID_TRACK_ENTRY, 1)));
        assert_eq!(read_id(&[0x00], 0), None);
        assert_eq!(read_size(&[0x81], 0), Some((Some(1), 1)));
        assert_eq!(read_size(&[0x40, 0x02], 0), Some((Some(2), 2)));
        assert_eq!(read_size(&[0xFF], 0), Some((None, 1)));
        assert_eq!(
            read_size(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], 0),
            Some((None, 8))
        );
        assert_eq!(read_size(&[0x00], 0), None);
    }

    #[test]
    fn codec_ids_map() {
        assert_eq!(codec_from_id("V_VP9"), Some(Codec::Vp9));
        assert_eq!(codec_from_id("V_MPEG4/ISO/AVC"), Some(Codec::H264));
        assert_eq!(codec_from_id("V_THEORA"), None);
    }

    #[test]
    fn garbage_and_truncation_never_panic() {
        // Every prefix of a made-up header, and some noise.
        let header = [
            0x1A, 0x45, 0xDF, 0xA3, 0x80, 0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF, 0xFF, 0x16, 0x54, 0xAE, 0x6B, 0x8A, 0xAE, 0x88, 0x83, 0x81, 0x01, 0x86,
            0x83, 0x56, 0x50,
        ];
        for n in 0..=header.len() {
            let _ = parse_matroska(&header[..n]);
        }
        for seed in 0..64u8 {
            let noise: Vec<u8> = (0..256u16)
                .map(|i| (i as u8).wrapping_mul(seed).wrapping_add(seed ^ 0x5A))
                .collect();
            let _ = parse_matroska(&noise);
        }
        assert!(sniff("/definitely/not/a/file").is_none());
        assert!(sniff("https://example.com/a.ogg").is_none());
    }

    fn make(name: &str, args: &[&str]) -> Option<String> {
        let dir = std::env::temp_dir().join(format!("tgcalls-sniff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(name);
        let status = std::process::Command::new("ffmpeg")
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

    fn matroska(path: &str) -> Mkv {
        match sniff(path) {
            Some(Sniffed::Matroska(m)) => m,
            other => panic!("expected Matroska, got {other:?}"),
        }
    }

    #[test]
    fn ogg_opus_is_recognised_with_its_duration() {
        let Some(path) = make("a.ogg", &["-vn", "-c:a", "libopus"]) else {
            return;
        };
        let Some(Sniffed::OggOpus { duration }) = sniff(&path) else {
            panic!("not recognised as Ogg Opus");
        };
        let secs = duration.expect("duration").as_secs_f64();
        assert!((1.8..2.2).contains(&secs), "{secs}");
    }

    #[test]
    fn matroska_tracks_codec_size_fps_and_duration() {
        let cases: [(&str, &[&str], Codec); 4] = [
            (
                "h264.mkv",
                &["-c:v", "libx264", "-c:a", "libopus"],
                Codec::H264,
            ),
            (
                "vp8.webm",
                &["-c:v", "libvpx", "-c:a", "libopus"],
                Codec::Vp8,
            ),
            (
                "vp9.webm",
                &["-c:v", "libvpx-vp9", "-c:a", "libopus"],
                Codec::Vp9,
            ),
            (
                "av1.mkv",
                &["-c:v", "libsvtav1", "-c:a", "libopus"],
                Codec::Av1,
            ),
        ];
        for (name, args, codec) in cases {
            let Some(path) = make(name, args) else {
                continue; // this ffmpeg lacks the encoder
            };
            let mkv = matroska(&path);
            let video = mkv.video.expect(name);
            assert_eq!(video.codec, Some(codec), "{name}");
            assert_eq!((video.width, video.height), (320, 240), "{name}");
            let fps = video.fps.expect("fps");
            assert!((14.0..16.0).contains(&fps), "{name}: {fps}");
            assert_eq!(mkv.audio_codec.as_deref(), Some("A_OPUS"), "{name}");
            let secs = mkv.duration.expect("duration").as_secs_f64();
            assert!((1.8..2.3).contains(&secs), "{name}: {secs}");
        }
    }

    #[test]
    fn unsupported_or_foreign_files_are_left_to_ffmpeg() {
        // Matroska video ntgcalls cannot forward: seen, but with no codec.
        if let Some(path) = make("mpeg4.mkv", &["-c:v", "mpeg4", "-c:a", "libopus"]) {
            assert_eq!(matroska(&path).video.unwrap().codec, None);
        }
        // Not Ogg / Matroska at all.
        if let Some(path) = make("x.mp4", &["-c:v", "libx264", "-c:a", "aac"]) {
            assert!(sniff(&path).is_none());
        }
        // Ogg that is not Opus.
        if let Some(path) = make("v.ogg", &["-vn", "-c:a", "libvorbis"]) {
            assert!(sniff(&path).is_none());
        }
    }
}
