// Starts a brand-new E2E conference call, rings one user into it as a
// *video* call, and streams a video file (audio + video from the same
// source) once they have actually joined.
//
// Same flow as conference_calls.rs, with two video-specific differences:
//
// 1. Whether the conference signals "video on" is decided at join time,
//    from the media passed to `start` - `play` later does not change it.
//    Starting with `media: None` would join as "video stopped" and the
//    invitee would never render our camera, even after `play`. So we start
//    with a placeholder camera source (`Media::external_video`): it counts
//    as video for the join and the invite ring, but sends nothing until
//    something feeds it.
// 2. Once `ConferenceEvent::ParticipantsChanged` says someone joined, we
//    swap the placeholder for the real thing with `play(Media::av(..))` -
//    audio and video from one file, two ffmpeg processes. Width/height is
//    a bounding box; the source is fit into it preserving its aspect
//    ratio, never stretched.
//
// We still relay `UpdateGroupCallChainBlocks` from the raw update stream
// into the conference by hand (`ConferenceCall` isn't wrapped by `Calls`),
// so the E2E chain can move forward as the other side sends its own blocks.
//
// usage: conference_video_calls <chat_id> <invitee_user_id> <video_file> [w] [h] [fps]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tgcalls::{ConferenceCall, ConferenceEvent, ConferenceTarget, Media, MediaDescription};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ntgcalls=debug")),
        )
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect(
            "usage: conference_video_calls <chat_id> <invitee_user_id> <video_file> [w] [h] [fps]",
        )
        .parse()?;
    let invitee: i64 = args.next().expect("missing invitee_user_id").parse()?;
    let path = args.next().expect("missing video file path");
    let width: i16 = args
        .next()
        .unwrap_or_else(|| "1280".into())
        .parse()
        .unwrap_or(1280);
    let height: i16 = args
        .next()
        .unwrap_or_else(|| "720".into())
        .parse()
        .unwrap_or(720);
    let fps: u8 = args
        .next()
        .unwrap_or_else(|| "30".into())
        .parse()
        .unwrap_or(30);

    let (client, _shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let mut stream = client.stream_updates();

    // Fires once, the first time the invitee actually joins.
    let (joined_tx, mut joined_rx) = tokio::sync::mpsc::channel::<()>(1);
    let already_signaled = Arc::new(AtomicBool::new(false));

    let conference = Arc::new(ConferenceCall::new(client, chat_id, {
        let already_signaled = already_signaled.clone();
        let joined_tx = joined_tx.clone();
        move |event| match event {
            ConferenceEvent::FingerprintUpdated(emoji) => {
                println!("verification emoji (compare with the other side!): {emoji}");
            }
            ConferenceEvent::StreamEnded(..) => println!("stream ended"),
            ConferenceEvent::Left => println!("left the conference"),
            ConferenceEvent::ParticipantsChanged => {
                println!("participants changed");
                if !already_signaled.swap(true, Ordering::SeqCst) {
                    let _ = joined_tx.try_send(());
                }
            }
            // Not handling Frames/RemoteSourceChanged here - see the docs on
            // ConferenceEvent::Frames if you want to capture or record a call.
            _ => {}
        }
    }));

    // Placeholder camera: makes the join and the invite signal video, but
    // streams nothing until `play` replaces it below.
    let placeholder = MediaDescription {
        microphone: None,
        speaker: None,
        camera: Some(Media::external_video(width, height, fps)),
        screen: None,
    };

    println!("starting conference and ringing {invitee} as a video call...");
    conference
        .start(
            ConferenceTarget::Create {
                invite: vec![invitee],
            },
            Some(placeholder),
        )
        .await?;
    println!("conference created, ringing {invitee} - not streaming yet");
    if let Some(link) = conference.invite_link().await {
        println!("share this to let anyone join: {link}");
    }

    let relay = tokio::spawn({
        let conference = conference.clone();
        async move {
            while let Some(raw) = stream.next_raw().await {
                if let ferogram::tl::enums::Update::GroupCallChainBlocks(u) = &raw.inner {
                    conference.apply_chain_blocks(u);
                }
            }
        }
    });

    println!("waiting for {invitee} to actually join before streaming (Ctrl+C to give up)...");
    joined_rx.recv().await;

    // Audio and video from the same file.
    conference
        .play(Media::av(&path, &path, width, height, fps))
        .await?;
    println!("{invitee} joined, now streaming {width}x{height} @ {fps}fps: {path}");

    println!("press Enter to hang up.");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    relay.abort();
    conference.leave().await?;
    println!("left the conference.");
    Ok(())
}
