use tgcalls::{Call, Media};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: group_audio_call <chat_id> <file>")
        .parse()
        .expect("chat_id must be a number");
    let path = args.next().expect("missing audio file path");

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let mut call = Call::new(client, chat_id);

    println!("Joining voice chat in {}...", chat_id);
    call.join(Media::audio(&path)).await?;
    println!("Streaming: {}", path);
    println!("Press Ctrl+C to stop.  (RUST_LOG=debug for verbose output)");

    tokio::signal::ctrl_c().await?;

    println!("Leaving...");
    call.leave().await?;

    drop(shutdown);
    Ok(())
}
