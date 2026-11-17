// Live-chat messages in a voice chat: prints every message posted in the
// call and sends whatever you type on stdin.
//
// Incoming messages are routed by `Calls` when it is registered as
// middleware (same as video auto-subscribe), so this example runs the update
// stream through a Dispatcher.
//
// Plain group / voice chats only - conferences are end-to-end encrypted and
// need ntgcalls' encrypt/decrypt, which is not published yet.
//
// usage: call_messages <chat_id> <audio_file>
use std::io::BufRead;

use ferogram::filters::Dispatcher;
use tgcalls::{CallEvent, Calls};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: call_messages <chat_id> <audio_file>")
        .parse()?;
    let path = args.next().expect("missing audio file path");

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let mut stream = client.stream_updates();
    let calls = Calls::new(client);

    calls.on_event(|chat, event| match event {
        CallEvent::Message {
            from_user_id,
            text,
            from_admin,
            ..
        } => {
            let who = from_user_id.map_or("someone".to_string(), |id| id.to_string());
            let badge = if from_admin { " (admin)" } else { "" };
            println!("[{chat}] {who}{badge}: {text}");
        }
        other => println!("[{chat}] {other:?}"),
    });

    // Without this, messages are never delivered to `on_event`.
    let mut dp = Dispatcher::new();
    dp.middleware(calls.clone());
    let dispatch = tokio::spawn(async move {
        while let Some(upd) = stream.next().await {
            dp.dispatch(upd).await;
        }
    });

    calls.play(chat_id, path.as_str()).await?;
    println!("In the call. Type a message and press Enter (quit to leave).");

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
        if let Err(e) = calls.send_message(chat_id, line).await {
            println!("error: {e}");
        }
    }

    dispatch.abort();
    calls.leave(chat_id).await?;
    drop(shutdown);
    Ok(())
}
