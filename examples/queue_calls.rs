// Playing multiple sources back-to-back in one chat.
//
// tgcalls has no Queue type, and doesn't need one - a queue is just a
// Vec<String> plus "when this one ends, play the next", and we already
// have the signal for "ended": CallEvent::StreamEnded. This example is
// the whole pattern, not a wrapper around hidden crate functionality.
//
// usage: queue_calls <chat_id> <file> [<file> ...]
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use tgcalls::{CallEvent, Calls};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: queue_calls <chat_id> <file> [<file> ...]")
        .parse()
        .expect("chat_id must be a number");
    let files: Vec<String> = args.collect();
    if files.is_empty() {
        anyhow::bail!("need at least one file to queue");
    }

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let calls = Calls::new(client);

    // One queue per chat_id - a real bot would key this the same way.
    let queues: Arc<Mutex<HashMap<i64, VecDeque<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    queues
        .lock()
        .unwrap()
        .insert(chat_id, files.into_iter().collect());

    let calls_for_handler = calls.clone();
    let queues_for_handler = queues.clone();
    calls.on_event(move |chat_id, event| {
        if !matches!(event, CallEvent::StreamEnded(..)) {
            return;
        }
        let next = queues_for_handler
            .lock()
            .unwrap()
            .get_mut(&chat_id)
            .and_then(VecDeque::pop_front);
        let Some(next) = next else { return };
        let calls = calls_for_handler.clone();
        tokio::spawn(async move {
            println!("Track ended, playing next: {next}");
            if let Err(e) = calls.play(chat_id, next).await {
                eprintln!("failed to play next queued track: {e}");
            }
        });
    });

    let first = queues
        .lock()
        .unwrap()
        .get_mut(&chat_id)
        .and_then(VecDeque::pop_front)
        .expect("at least one file");
    calls.play(chat_id, first.as_str()).await?;
    println!("Playing {first} in {chat_id} - queue will auto-advance.");
    println!("Press Ctrl+C to leave.");

    tokio::signal::ctrl_c().await?;
    calls.leave(chat_id).await?;

    drop(shutdown);
    Ok(())
}
