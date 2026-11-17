// Plays audio in a group call at a chosen Opus bitrate, and changes it
// mid-call by swapping the microphone source.
//
// Bitrate is in bits per second, audio only (ntgcalls has no video bitrate).
// Rough guide: 24_000-32_000 voice, 64_000 good music, 128_000+ high quality.
//
// usage: call_bitrate <chat_id> <audio_file> [bitrate_bps]
//
// commands while playing: <number> sets a new bitrate | quit
use std::io::BufRead;

use tgcalls::{Calls, Media};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: call_bitrate <chat_id> <audio_file> [bitrate_bps]")
        .parse()?;
    let path = args.next().expect("missing audio file path");
    let bitrate: u32 = args
        .next()
        .unwrap_or_else(|| "64000".into())
        .parse()
        .unwrap_or(64_000);

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let calls = Calls::new(client);
    calls.on_event(|chat, event| println!("[{chat}] {event:?}"));

    // Whole description: bitrate applies to its microphone.
    let media = Media::with_audio_bitrate(Media::audio(&path), bitrate);
    calls.play_media(chat_id, media).await?;
    println!("Playing {path} at {bitrate} bps. Type a new bitrate, or quit.");

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
        if line == "quit" {
            break;
        }
        let Ok(bps) = line.parse::<u32>() else {
            println!("type a number (bits per second) or quit");
            continue;
        };
        // Swapping the microphone restarts the source from the beginning,
        // with the new bitrate applied.
        let mic = Media::audio_bitrate(
            Media::audio(&path)
                .microphone
                .expect("audio has a microphone"),
            bps,
        );
        match calls.set_audio(chat_id, Some(mic)).await {
            Ok(()) => println!("now {bps} bps"),
            Err(e) => println!("error: {e}"),
        }
    }

    calls.leave(chat_id).await?;
    drop(shutdown);
    Ok(())
}
