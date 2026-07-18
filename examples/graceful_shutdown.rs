// Multi-chat playback with graceful shutdown. Ctrl+C leaves every active
// chat concurrently instead of just dropping the process - Telegram sees
// you actually leave, rather than waiting out its own timeout.
//
// usage: graceful_shutdown <chat_id> <file> [<chat_id> <file> ...]
use tgcalls::Calls;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() % 2 != 0 {
        anyhow::bail!("usage: graceful_shutdown <chat_id> <file> [<chat_id> <file> ...]");
    }

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let calls = Calls::new(client);

    for pair in args.chunks(2) {
        let chat_id: i64 = pair[0].parse().expect("chat_id must be a number");
        let path = &pair[1];
        calls.play(chat_id, path.as_str()).await?;
        println!("Playing {path} in {chat_id}");
    }

    println!("Press Ctrl+C to leave every chat and exit.");
    tokio::signal::ctrl_c().await?;

    println!("Shutting down...");
    for (chat_id, result) in calls.shutdown().await {
        match result {
            Ok(()) => println!("  left {chat_id}"),
            Err(e) => eprintln!("  failed to leave {chat_id}: {e}"),
        }
    }

    drop(shutdown);
    Ok(())
}
