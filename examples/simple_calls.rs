// The simplest way to play audio in a group call - Calls handles join,
// voice-chat creation, and media detection for you.
//
// usage: simple_calls <chat_id> <file>
use tgcalls::Calls;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: simple_calls <chat_id> <file>")
        .parse()?;
    let path = args.next().expect("missing audio file path");

    let api_id: i32 = std::env::var("API_ID")?.parse()?;
    let api_hash = std::env::var("API_HASH")?;
    let (client, shutdown) =
        ferogram::Client::quick_connect("tgcalls.session", api_id, &api_hash).await?;

    let calls = Calls::new(client);
    calls.play(chat_id, path).await?;

    println!("Playing. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;

    calls.leave(chat_id).await?;
    drop(shutdown);
    Ok(())
}
