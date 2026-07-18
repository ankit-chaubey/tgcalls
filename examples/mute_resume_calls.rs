// Mute/resume/volume through the high-level Calls API - the control
// surface a bot's command handlers would actually call.
//
// usage: mute_resume_calls <chat_id> <file> <user_id>
use tgcalls::Calls;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: mute_resume_calls <chat_id> <file> <user_id>")
        .parse()
        .expect("chat_id must be a number");
    let path = args.next().expect("missing audio file path");
    let user_id: i64 = args
        .next()
        .expect("missing user_id to adjust volume for")
        .parse()
        .expect("user_id must be a number");

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let calls = Calls::new(client);

    calls.play(chat_id, path.as_str()).await?;
    println!("Playing {path} in {chat_id}");

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    calls.mute(chat_id).await?;
    println!("Muted.");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    calls.unmute(chat_id).await?;
    println!("Unmuted.");

    // Volume is Telegram's raw 0..20000 scale: 10000 = 100%, 15000 = 150%.
    calls.set_volume(chat_id, user_id, 15000).await?;
    println!("Set {user_id}'s volume to 150% for this listener.");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    calls.pause(chat_id).await?;
    println!("Paused.");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    calls.resume(chat_id).await?;
    println!("Resumed.");

    println!("Press Ctrl+C to leave.");
    tokio::signal::ctrl_c().await?;
    calls.leave(chat_id).await?;

    drop(shutdown);
    Ok(())
}
