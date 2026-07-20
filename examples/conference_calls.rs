// Starts a brand-new E2E conference call, rings one user into it, streams
// an audio file, and prints the verification emoji once ntgcalls has it.
//
// Media isn't sent until the invitee has actually joined. A conference's
// shared key only covers whoever's currently in the chain, so anything
// sent before the invitee joins gets encrypted against a key they can't
// derive yet and just never decrypts on their end - `Session::encrypt`
// doesn't error on this, it just produces ciphertext nobody can read. The
// fingerprint emoji reflects your own key state, not theirs, so it's not
// a signal to go by either. `ConferenceEvent::ParticipantsChanged` is what
// tells you someone actually joined at the WebRTC level - wait for that,
// then play.
//
// Also relays `UpdateGroupCallChainBlocks` off the raw update stream back
// into the conference, since that's what lets the E2E chain make progress
// as the other participant sends their own blocks. Same idea as `Calls`'
// `route_update` for classic group calls, just a different update type -
// done by hand here since `ConferenceCall` isn't wrapped by `Calls`.
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

    // Fires once, the first time the invitee actually joins - see the
    // file header for why we wait on this before playing anything.
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
            // Frames/RemoteSourceChanged omitted here - see the module docs on
            // ConferenceEvent::Frames if you want to capture/record a call.
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
