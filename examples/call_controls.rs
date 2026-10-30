// Every mid-call control through the Calls API, driven from stdin.
// Works at any time while the call is up, in any order.
//
// usage: call_controls <chat_id> <video_file>
//
// commands: mute | unmute | pause | resume | cam on | cam off |
//           mic on | mic off | screen on | screen off | state | quit
use std::io::BufRead;

use tgcalls::{Calls, Media};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let chat_id: i64 = args
        .next()
        .expect("usage: call_controls <chat_id> <video_file>")
        .parse()?;
    let path = args.next().expect("missing video file path");

    let (client, shutdown) = ferogram::Client::builder()
        .api_id(std::env::var("API_ID")?.parse()?)
        .api_hash(std::env::var("API_HASH")?)
        .session("tgcalls.session")
        .connect()
        .await?;

    let calls = Calls::new(client);
    calls.on_event(|chat, event| println!("[{chat}] {event:?}"));

    calls.play(chat_id, path.as_str()).await?;
    println!("Playing {path} in {chat_id}. Type a command.");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    while let Some(line) = rx.recv().await {
        let result = match line.trim() {
            "mute" => calls.mute(chat_id).await,
            "unmute" => calls.unmute(chat_id).await,
            "pause" => calls.pause(chat_id).await,
            "resume" => calls.resume(chat_id).await,
            "cam on" => {
                calls
                    .set_camera(chat_id, Media::video(&path, 1280, 720, 30).camera)
                    .await
            }
            "cam off" => calls.set_camera(chat_id, None).await,
            "mic on" => {
                calls
                    .set_microphone(chat_id, Media::audio(&path).microphone)
                    .await
            }
            "mic off" => calls.set_microphone(chat_id, None).await,
            "screen on" => {
                calls
                    .set_screen(chat_id, Some(Media::screen(1280, 720, 30)))
                    .await
            }
            "screen off" => calls.set_screen(chat_id, None).await,
            "state" => {
                match calls.status(chat_id).await {
                    Ok(s) => println!("{s:?}"),
                    Err(e) => println!("error: {e}"),
                }
                continue;
            }
            "quit" => break,
            other => {
                println!("unknown command: {other}");
                continue;
            }
        };
        if let Err(e) = result {
            println!("error: {e}");
        }
    }

    calls.leave(chat_id).await?;
    drop(shutdown);
    Ok(())
}
