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
        .expect("usage: p2p_video_call <user_id> <file> [w] [h] [fps]")
        .parse()?;
    let path = args.next().expect("missing file path");
    let width: i16 = args
        .next()
        .unwrap_or_else(|| "1280".into())
        .parse()
        .unwrap_or(1280);
    let height: i16 = args
        .next()
        .unwrap_or_else(|| "720".into())
        .parse()
        .unwrap_or(720);
    let fps: u8 = args
        .next()
        .unwrap_or_else(|| "30".into())
        .parse()
        .unwrap_or(30);

    let (client, _shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let mut stream = client.stream_updates();
    let mut call = P2PCall::new(client, user_id);

    // Set media BEFORE connecting so ntgcalls knows about the video stream
    // during SDP negotiation and sends videoState:"active" from the start.
    let media = Media::av(&path, &path, width, height, fps);

    println!("calling user {}...", user_id);
    let (servers, versions) = call.request(true, &mut stream).await?;
    println!(
        "call accepted, connecting ({} servers, versions: {:?})...",
        servers.len(),
        versions
    );

    let (mut sig_out_rx, mut conn_rx) = call.connect(&servers, &versions, true).await?;

    // Set media immediately after connect() so it's configured before
    // the WebRTC negotiation completes. ntgcalls will send videoState:"active"
    // in the first MediaState once the encoder starts.
    call.set_media(StreamMode::Capture, &media).await?;
    println!(
        "media sources configured ({}x{} @ {}fps)",
        width, height, fps
    );

    println!("exchanging ICE/SDP signaling...");
    let connected = call
        .run_signaling(&mut sig_out_rx, &mut conn_rx, &mut stream)
        .await?;
    if !connected {
        call.end().await;
        return Err(anyhow::anyhow!("WebRTC connection failed"));
    }
    println!("connected.");
    println!("streaming {}x{} @ {}fps from: {}", width, height, fps, path);
    println!("press Enter to hang up.");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    call.end().await;
    println!("call ended.");
    Ok(())
}
