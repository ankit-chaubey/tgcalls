use anyhow::Result;
use tgcalls::{Call, Media};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: group_screen_share <chat_id> <audio_file>")
        .parse()?;
    let audio_path = args.next().expect("missing audio file path");

    let (client, _shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let mut call = Call::new(client, chat_id);

    call.join(Media::audio(&audio_path)).await?;
    println!("joined group call.");

    let screen = Media::screen(1280, 720, 30);
    call.join_presentation(screen).await?;
    println!("presentation started. Press Enter to stop.");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    call.stop_presentation().await?;
    println!("presentation stopped.");

    call.leave().await?;
    println!("left call.");
    Ok(())
}
