// Recording a voice chat's incoming audio to a file, through Calls -
// joins silently first if not already in the call.
//
// usage: recording_calls <chat_id> <output.mp3> <seconds>
use tgcalls::{CallEvent, Calls, Media};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: recording_calls <chat_id> <output.mp3> <seconds>")
        .parse()
        .expect("chat_id must be a number");
    let output = args.next().expect("missing output path");
    let seconds: u64 = args
        .next()
        .expect("missing recording duration in seconds")
        .parse()
        .expect("seconds must be a number");

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let calls = Calls::new(client);

    // Recording targets don't have a stream-end signal in the usual sense
    // (there's no "end" to incoming call audio), but StreamEnded still
    // fires if the recording pipe itself dies unexpectedly - worth logging.
    calls.on_event(|chat_id, event| {
        if let CallEvent::StreamEnded(..) = event {
            eprintln!("warning: recording pipe for {chat_id} ended unexpectedly");
        }
    });

    calls.record(chat_id, Media::record_audio(&output)).await?;
    println!("Recording {chat_id}'s audio to {output} for {seconds}s...");

    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;

    calls.leave(chat_id).await?;
    println!("Done - saved to {output}");

    drop(shutdown);
    Ok(())
}
