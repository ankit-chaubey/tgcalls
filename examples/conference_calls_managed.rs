// Same scenario as conference_calls.rs (start a conference, ring someone,
// stream audio) but through the `ConferenceCalls` manager instead of a raw
// `ConferenceCall` - the manager auto-routes UpdateGroupCallChainBlocks via
// Dispatcher middleware, so there's no manual update-stream loop here.
// Worth comparing the two examples side by side: this is what you get once
// you're running more than one chat's conference and don't want to wire
// the chain-block relay by hand for each one.
//
// usage: conference_calls_managed <chat_id> <invitee_user_id> <audio_file>

use ferogram::filters::Dispatcher;
use tgcalls::{incoming_conference_call, ConferenceCalls, ConferenceEvent, Media};

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
        .expect("usage: conference_calls_managed <chat_id> <invitee_user_id> <audio_file>")
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

    let conferences = ConferenceCalls::new(client);
    conferences.on_event(|chat_id, event| match event {
        ConferenceEvent::FingerprintUpdated(emoji) => {
            println!("[{chat_id}] verification emoji: {emoji}");
        }
        ConferenceEvent::StreamEnded(..) => println!("[{chat_id}] stream ended"),
        ConferenceEvent::Left => println!("[{chat_id}] left the conference"),
        ConferenceEvent::ParticipantsChanged => println!("[{chat_id}] participants changed"),
        _ => {}
    });

    // Chain blocks arrive as ordinary updates - the manager's Middleware
    // impl auto-routes them to whichever tracked chat they belong to. A
    // real client would register this once and never think about it again;
    // shown explicitly here since this example has nothing else running
    // the dispatcher. Incoming conference invites (a `MessageActionConferenceCall`
    // system message, not a call-specific update) are checked here too -
    // `incoming_conference_call` never touches `ConferenceCalls` itself,
    // it just tells you one exists; joining is still your call.
    let mut dp = Dispatcher::new();
    dp.middleware(conferences.clone());
    let dispatch = tokio::spawn(async move {
        while let Some(upd) = stream.next().await {
            if let Some(invite) = incoming_conference_call(&upd) {
                println!(
                    "[{}] conference invite (call_id {}, active: {}, missed: {})",
                    invite.chat_id, invite.call_id, invite.active, invite.missed
                );
                // Not auto-joining here - see migrate_from_p2p in
                // CONFERENCE_CALLS.md for the P2P-upgrade case, and
                // ConferenceCalls::join for actually joining one of these.
            }
            dp.dispatch(upd).await;
        }
    });

    println!("starting conference and ringing {invitee}...");
    conferences
        .create(chat_id, vec![invitee], Some(Media::audio(&path)))
        .await?;
    println!("conference joined, streaming {path}");

    println!("press Enter to hang up.");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    conferences.leave(chat_id).await?;
    dispatch.abort();
    println!("left the conference.");
    Ok(())
}
