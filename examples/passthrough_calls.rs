// Plays media in a voice chat without decoding it (passthrough).
//
// Normally every source is decoded to raw PCM / video frames and ntgcalls
// encodes it again. Here Opus audio and H.264 / VP8 / VP9 video are forwarded
// as they are, and anything else is encoded once by ffmpeg. Same result, far
// less CPU, which adds up quickly when one machine holds many calls.
//
// Files ntgcalls can read as they are (an Ogg Opus file, or a Matroska file
// with H.264 / VP8 / VP9 / AV1 video) are detected and handed over directly,
// with no ffmpeg at all. The audio inside a Matroska file is the one part
// that still goes through an ffmpeg copy.
//
// usage: passthrough_calls <chat_id> <file_or_url>
//
// commands while playing:
//   seek <seconds>                          restart at that position
//   ogg <file.ogg>                          play an Ogg Opus file directly
//   mkv <file> <w> <h> <fps> [audio.ogg]    play a .mkv / .webm directly, with an
//                                           optional Ogg Opus file as its sound
//   quit
//
// `ogg` and `mkv` hand already-encoded files straight to ntgcalls, so they
// need no ffmpeg at all. The video must be H.264, VP8, VP9 or AV1 in
// Matroska, the audio must be Ogg Opus, and <w> <h> <fps> must match the
// video file.
use std::io::BufRead;
use std::time::Duration;

use tgcalls::{Calls, Media};

/// `mkv <file> <w> <h> <fps> [audio.ogg]` -> video (and optional audio) that
/// ntgcalls reads directly.
fn parse_mkv(rest: &str) -> Option<tgcalls::MediaDescription> {
    let mut parts = rest.split_whitespace();
    let file = parts.next()?;
    let width = parts.next()?.parse().ok()?;
    let height = parts.next()?.parse().ok()?;
    let fps = parts.next()?.parse().ok()?;
    let mut media = Media::video_matroska(file, width, height, fps);
    if let Some(audio) = parts.next() {
        media.microphone = Media::audio_ogg(audio).microphone;
    }
    Some(media)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: passthrough_calls <chat_id> <file_or_url>")
        .parse()?;
    let source = args.next().expect("missing file or url");

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let calls = Calls::new(client);
    calls.on_event(|chat, event| println!("[{chat}] {event:?}"));

    // Same as `calls.play(..)`, but kept encoded end to end.
    calls.play_passthrough(chat_id, source.as_str()).await?;
    println!("Passthrough: {source}. Commands: seek <seconds>, ogg <file>, mkv <file> <w> <h> <fps> [audio.ogg], quit.");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    while let Some(line) = rx.recv().await {
        let line = line.trim();
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let result = match cmd {
            "quit" => break,
            "seek" => match rest.parse::<f64>() {
                Ok(secs) => {
                    calls
                        .seek_passthrough(chat_id, source.as_str(), Duration::from_secs_f64(secs))
                        .await
                }
                Err(_) => {
                    println!("usage: seek <seconds>");
                    continue;
                }
            },
            // An Ogg Opus file is read by ntgcalls itself: no ffmpeg process.
            "ogg" if !rest.is_empty() => calls.play_media(chat_id, Media::audio_ogg(rest)).await,
            // A Matroska video (and optional Ogg audio) read by ntgcalls itself.
            "mkv" => match parse_mkv(rest) {
                Some(media) => calls.play_media(chat_id, media).await,
                None => {
                    println!("usage: mkv <file> <w> <h> <fps> [audio.ogg]");
                    continue;
                }
            },
            _ => {
                println!("commands: seek <seconds>, ogg <file>, mkv <file> <w> <h> <fps> [audio.ogg], quit");
                continue;
            }
        };
        if let Err(e) = result {
            println!("error: {e}");
        }
    }

    calls.leave(chat_id).await?;
    drop(shutdown);
    Ok(())
}
