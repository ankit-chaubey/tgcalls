// Starts a brand-new E2E conference call, rings one user into it, streams
// an audio file, and prints the verification emoji fingerprint once
// ntgcalls has it. Also relays UpdateGroupCallChainBlocks off the raw
// update stream back into the conference - required for the E2E chain to
// make progress as the other participant sends their own blocks. Same
// idea as `Calls`' route_update for classic group calls, just a different
// update type, done manually here since `ConferenceCall` isn't wrapped by
// `Calls` (yet).
//
// usage: conference_calls <chat_id> <invitee_user_id> <audio_file>

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

    let conference = Arc::new(ConferenceCall::new(client, chat_id, |event| match event {
        ConferenceEvent::FingerprintUpdated(emoji) => {
            println!("verification emoji (compare with the other side!): {emoji}");
        }
        ConferenceEvent::StreamEnded(..) => println!("stream ended"),
        ConferenceEvent::Left => println!("left the conference"),
        ConferenceEvent::ParticipantsChanged => println!("participants changed"),
        // Frames/RemoteSourceChanged omitted here - see the module docs on
        // ConferenceEvent::Frames if you want to capture/record a call.
        _ => {}
    }));

    println!("starting conference and ringing {invitee}...");
    conference
        .start(
            ConferenceTarget::Create {
                invite: vec![invitee],
            },
            Some(Media::audio(&path)),
        )
        .await?;
    println!("conference joined, streaming {path}");
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

    println!("press Enter to hang up.");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    relay.abort();
    conference.leave().await?;
    println!("left the conference.");
    Ok(())
}
