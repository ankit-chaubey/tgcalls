// Plays a source with quality presets, using the `MediaStream` builder.
//
// One builder replaces picking between `Media::audio`, `av`, the passthrough
// helpers and a hand-set bitrate. Every setting has a default, so
// `MediaStream::new(source)` alone already works.
//
// usage: stream_presets <chat_id> <file_or_url> [audio_preset] [video_preset]
//
// audio presets: voice, low, medium, high, studio, master
// video presets: 4k, 2k, 1080, 720, 480, 360
//
// commands while playing:
//   audio <preset>      restart with another audio quality
//   video <preset>      restart with another video quality
//   seek <seconds>      restart at that position
//   passthrough on|off  switch between the encoded and the decoded route
//   quit
//
// Each command rebuilds the stream and plays it again from the current
// start offset, because ntgcalls takes a whole new source to change quality.
use std::io::BufRead;
use std::time::Duration;

use tgcalls::{AudioQuality, Calls, MediaStream, VideoQuality};

fn audio_preset(name: &str) -> Option<AudioQuality> {
    Some(match name {
        "voice" => AudioQuality::Voice,
        "low" => AudioQuality::Low,
        "medium" => AudioQuality::Medium,
        "high" => AudioQuality::High,
        "studio" => AudioQuality::Studio,
        "master" => AudioQuality::Master,
        _ => return None,
    })
}

fn video_preset(name: &str) -> Option<VideoQuality> {
    Some(match name {
        "4k" => VideoQuality::Uhd4k,
        "2k" => VideoQuality::Qhd2k,
        "1080" => VideoQuality::Fhd1080,
        "720" => VideoQuality::Hd720,
        "480" => VideoQuality::Sd480,
        "360" => VideoQuality::Sd360,
        _ => return None,
    })
}

/// The settings the commands change. `stream()` turns them into a builder.
struct Settings {
    source: String,
    audio: AudioQuality,
    video: VideoQuality,
    passthrough: bool,
    offset: Duration,
}

impl Settings {
    fn stream(&self) -> MediaStream {
        MediaStream::new(self.source.as_str())
            .audio_quality(self.audio)
            .video_quality(self.video)
            .passthrough(self.passthrough)
            .start_at(self.offset)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: stream_presets <chat_id> <file_or_url> [audio] [video]")
        .parse()?;
    let source = args.next().expect("missing file or url");
    let audio = args
        .next()
        .map(|a| audio_preset(&a).expect("unknown audio preset"))
        .unwrap_or(AudioQuality::High);
    let video = args
        .next()
        .map(|v| video_preset(&v).expect("unknown video preset"))
        .unwrap_or_default();

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let calls = Calls::new(client);
    calls.on_event(|chat, event| println!("[{chat}] {event:?}"));

    let mut settings = Settings {
        source,
        audio,
        video,
        passthrough: false,
        offset: Duration::ZERO,
    };
    calls.play_stream(chat_id, settings.stream()).await?;
    println!("Playing. Commands: audio <preset>, video <preset>, seek <seconds>, passthrough on|off, quit.");

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
        match cmd {
            "quit" => break,
            "audio" => match audio_preset(rest) {
                Some(a) => settings.audio = a,
                None => {
                    println!("usage: audio voice|low|medium|high|studio|master");
                    continue;
                }
            },
            "video" => match video_preset(rest) {
                Some(v) => settings.video = v,
                None => {
                    println!("usage: video 4k|2k|1080|720|480|360");
                    continue;
                }
            },
            "seek" => match rest.parse::<f64>() {
                Ok(secs) if secs >= 0.0 => settings.offset = Duration::from_secs_f64(secs),
                _ => {
                    println!("usage: seek <seconds>");
                    continue;
                }
            },
            "passthrough" => match rest {
                "on" => settings.passthrough = true,
                "off" => settings.passthrough = false,
                _ => {
                    println!("usage: passthrough on|off");
                    continue;
                }
            },
            _ => {
                println!("commands: audio <preset>, video <preset>, seek <seconds>, passthrough on|off, quit");
                continue;
            }
        }
        if let Err(e) = calls.play_stream(chat_id, settings.stream()).await {
            println!("error: {e}");
        }
    }

    calls.leave(chat_id).await?;
    drop(shutdown);
    Ok(())
}
