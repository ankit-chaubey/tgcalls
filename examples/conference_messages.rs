// Encrypted messages inside a conference: starts a conference, rings one
// user, then prints every message they send and sends whatever you type.
//
// Conference messages are end-to-end encrypted with the call's key (ntgcalls
// does the encrypting), so only people currently in the conference can read
// them. Nothing is streamed here, this is just the chat side of the call.
//
// Incoming messages are routed by `ConferenceCalls` when it is registered as
// middleware, same as chain blocks, so the update stream goes through a
// Dispatcher. Without that you'd never see `ConferenceEvent::Message`.
//
// usage: conference_messages <chat_id> <invitee_user_id>
use std::io::BufRead;

use ferogram::filters::Dispatcher;
use tgcalls::{ConferenceCalls, ConferenceEvent};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: conference_messages <chat_id> <invitee_user_id>")
        .parse()?;
    let invitee: i64 = args.next().expect("missing invitee_user_id").parse()?;

    let (client, _shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let mut stream = client.stream_updates();

    let conferences = ConferenceCalls::new(client);
    conferences.on_event(|chat, event| match &event {
        ConferenceEvent::Message { from_user_id, data } => match event.message_text() {
            Some(text) => println!("[{chat}] {from_user_id}: {text}"),
            None => println!("[{chat}] {from_user_id}: <{} bytes>", data.len()),
        },
        ConferenceEvent::FingerprintUpdated(emoji) => {
            println!("[{chat}] verification emoji: {emoji}");
        }
        ConferenceEvent::ParticipantsChanged => println!("[{chat}] participants changed"),
        ConferenceEvent::Left => println!("[{chat}] left the conference"),
        _ => {}
    });

    let mut dp = Dispatcher::new();
    dp.middleware(conferences.clone());
    let dispatch = tokio::spawn(async move {
        while let Some(upd) = stream.next().await {
            dp.dispatch(upd).await;
        }
    });

    conferences.create(chat_id, vec![invitee], None).await?;
    println!("conference created, ringing {invitee}. Type a message, quit to leave.");

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
        if line.is_empty() {
            continue;
        }
        if let Err(e) = conferences.send_message(chat_id, line).await {
            println!("error: {e}");
        }
    }

    conferences.leave(chat_id).await?;
    dispatch.abort();
    Ok(())
}
