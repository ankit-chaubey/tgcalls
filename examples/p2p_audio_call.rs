use anyhow::Result;
use tgcalls::{Media, P2PCall, StreamMode};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ntgcalls=debug")),
        )
        .init();

    let mut args = std::env::args().skip(1);
    let user_id: i64 = args
        .next()
        .expect("usage: p2p_audio_call <user_id> <audio_file>")
        .parse()?;
    let path = args.next().expect("missing audio file path");

    let (client, _shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let mut stream = client.stream_updates();
    let mut call = P2PCall::new(client, user_id);
    let media = Media::audio(&path);

    println!("calling user {}...", user_id);
    let (servers, versions) = call.request(false, &mut stream).await?;
    println!("call accepted, connecting ({} servers)...", servers.len());

    let (mut sig_out_rx, mut conn_rx) = call.connect(&servers, &versions, true).await?;

    // Configure audio before signaling completes so encoder is ready immediately.
    call.set_media(StreamMode::Capture, &media).await?;

    println!("exchanging signaling...");
    let connected = call
        .run_signaling(&mut sig_out_rx, &mut conn_rx, &mut stream)
        .await?;
    if !connected {
        call.end().await;
        return Err(anyhow::anyhow!("WebRTC connection failed"));
    }
    println!("connected. streaming audio from: {}", path);
    println!("press Enter to hang up.");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    call.end().await;
    println!("call ended.");
    Ok(())
}
