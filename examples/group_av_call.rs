use tgcalls::{Call, Media};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: group_av_call <chat_id> <file> [w] [h] [fps]")
        .parse()
        .expect("chat_id must be a number");
    let path = args.next().expect("missing file path");
    let width: i16 = args
        .next()
        .unwrap_or("1280".into())
        .parse()
        .unwrap_or(1280i16);
    let height: i16 = args
        .next()
        .unwrap_or("720".into())
        .parse()
        .unwrap_or(720i16);
    let fps: u8 = args.next().unwrap_or("30".into()).parse().unwrap_or(30);

    let (client, _shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let mut call = Call::new(client, chat_id);

    println!("Joining video chat in {}...", chat_id);
    call.join(Media::av(&path, &path, width, height, fps))
        .await?;
    println!("Streaming A/V {}x{} @ {}fps: {}", width, height, fps, path);
    println!("Press Ctrl+C to stop.  (RUST_LOG=debug for verbose output)");

    tokio::signal::ctrl_c().await?;
    call.leave().await?;
    Ok(())
}
