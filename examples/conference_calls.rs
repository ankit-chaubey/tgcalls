// Starts a brand-new E2E conference call, rings one user into it, streams
// an audio file, and prints the verification emoji once ntgcalls has it.
//
// We hold off on playing anything until the invitee has actually joined -
// the conference's shared key only covers participants currently in the
// chain, so audio sent too early would just encrypt to a key they can't
// derive yet. `ConferenceEvent::ParticipantsChanged` is the signal that
// someone joined at the WebRTC level, so we wait for that before calling
// `play`.
//
// We also relay `UpdateGroupCallChainBlocks` from the raw update stream
// back into the conference - that's what lets the E2E chain move forward
// as the other side sends its own blocks. `ConferenceCall` isn't wrapped
// by `Calls`, so we wire this by hand here.
//
// usage: conference_calls <chat_id> <invitee_user_id> <audio_file>

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tgcalls::{ConferenceCall, ConferenceEvent, ConferenceTarget, Media};

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
        .expect("usage: conference_calls <chat_id> <invitee_user_id> <audio_file>")
        .parse()?;
    let invitee: i64 = args.next().expect("missing invitee_user_id").parse()?;
    let path = args.next().expect("missing audio file path");

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

    println!("starting conference and ringing {invitee}...");
    conference
        .start(
            ConferenceTarget::Create {
                invite: vec![invitee],
            },
            None,
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
    conference.play(Media::audio(&path)).await?;
    println!("{invitee} joined, now streaming {path}");

    println!("press Enter to hang up.");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    relay.abort();
    conference.leave().await?;
    println!("left the conference.");
    Ok(())
}
