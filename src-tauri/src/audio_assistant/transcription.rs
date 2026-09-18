use super::{
    capture::Capture,
    model::RETENTION,
    runtime::{publish_error, AudioAssistant},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
use tauri::Manager;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Message},
    MaybeTlsStream, WebSocketStream,
};
use tokio_util::sync::CancellationToken;

fn session_update() -> Value {
    json!({"type":"session.update", "session":{"type":"transcription", "audio":{"input":{
        "format":{"type":"audio/pcm","rate":24000},
        "transcription":{"model":"gpt-live-transcribe","delay":"low"}, "turn_detection":null}}}})
}

pub async fn run(app: tauri::AppHandle, generation: u64, cancel: CancellationToken) {
    let mut delay = 1;
    loop {
        let result = tokio::select! { biased;
            _ = cancel.cancelled() => return,
            result = connection(&app, generation, cancel.child_token()) => result,
        };
        if cancel.is_cancelled() {
            return;
        }
        if let Err(error) = result {
            publish_error(&app, generation, &error);
        }
        tokio::select! { biased;
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
        }
        delay = (delay * 2).min(30);
    }
}

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect() -> Result<Socket, String> {
    let key = crate::openai_credentials::load()?;
    let mut request = "wss://api.openai.com/v1/realtime?intent=transcription"
        .into_client_request()
        .map_err(|_| "Could not prepare transcription.")?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {key}")
            .parse()
            .map_err(|_| "Invalid OpenAI API key. Update it in Settings.")?,
    );
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(15),
        connect_async_with_config(
            request,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(128 * 1024))
                    .max_frame_size(Some(128 * 1024)),
            ),
            false,
        ),
    )
    .await
    .map_err(|_| "Transcription timed out. Reconnecting while listening is on…")?
    .map_err(|_| {
        "Could not connect to transcription. Check your API key and connection in Settings."
    })?;
    send(&mut socket, session_update()).await?;
    // Wait for acknowledgement before opening an audio device.
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = socket.next().await {
            if let Message::Text(text) = message.map_err(|_| "Transcription disconnected.")? {
                let event: Value =
                    serde_json::from_str(&text).map_err(|_| "Invalid transcription response.")?;
                if event["type"] == "session.updated" {
                    return Ok(());
                }
                if event["type"] == "error" {
                    return Err(
                        "OpenAI rejected transcription setup. Check API access in Settings.",
                    );
                }
            }
        }
        Err("Transcription disconnected.")
    })
    .await
    .map_err(|_| "Transcription setup timed out.")??;
    Ok(socket)
}

async fn send(socket: &mut Socket, event: Value) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(event.to_string().into())),
    )
    .await
    .map_err(|_| "Transcription connection stalled. Reconnecting…")?
    .map_err(|_| "Transcription disconnected. Reconnecting…".into())
}

async fn connection(
    app: &tauri::AppHandle,
    generation: u64,
    cancel: CancellationToken,
) -> Result<(), String> {
    let mut socket = connect().await?;
    if cancel.is_cancelled() {
        return Ok(());
    }
    let (_capture, mut packets) = Capture::start(cancel.clone())?;
    publish_error(app, generation, "");
    let mut pending = VecDeque::new();
    let mut items: HashMap<String, Instant> = HashMap::new();
    let mut bytes = 0;
    let mut turn_at = Instant::now();
    let mut pruning = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! { biased;
            _ = cancel.cancelled() => return Ok(()),
            _ = pruning.tick() => {
                let now = Instant::now();
                items.retain(|_, at| now.saturating_duration_since(*at) <= RETENTION);
                app.state::<AudioAssistant>().live.lock().unwrap().transcript.prune(now);
                if bytes >= 4800 && now.saturating_duration_since(turn_at) >= Duration::from_secs(2) {
                    send(&mut socket, json!({"type":"input_audio_buffer.commit"})).await?;
                    pending.push_back(turn_at); bytes = 0;
                }
                if pending.len() > 20 { return Err("Transcription fell behind. Reconnecting…".into()); }
            },
            packet = packets.recv() => {
                let packet = packet.ok_or("Windows audio capture stopped. Check your output device. Reconnecting…")?;
                if bytes == 0 { turn_at = packet.at; }
                bytes += packet.pcm.len();
                let append = json!({"type":"input_audio_buffer.append", "audio":STANDARD.encode(packet.pcm)});
                send(&mut socket, append).await?;
                // Short committed turns give useful timestamps and partial/final text,
                // without a second transcription path or re-uploading old audio.
                if bytes >= 96_000 {
                    send(&mut socket, json!({"type":"input_audio_buffer.commit"})).await?;
                    pending.push_back(turn_at); bytes = 0;
                }
            },
            message = socket.next() => {
                let message = message.ok_or("Transcription disconnected. Reconnecting…")?.map_err(|_| "Transcription disconnected. Reconnecting…")?;
                match message {
                    Message::Text(text) => {
                        let event: Value = serde_json::from_str(&text).map_err(|_| "Invalid transcription response.")?;
                        let id = event["item_id"].as_str().unwrap_or("");
                        match event["type"].as_str() {
                            Some("input_audio_buffer.committed") => { if let Some(at) = pending.pop_front() { items.insert(id.into(), at); } },
                            Some("conversation.item.input_audio_transcription.delta" | "conversation.item.input_audio_transcription.completed") => {
                                let final_text = event["type"] == "conversation.item.input_audio_transcription.completed";
                                let text = event[if final_text { "transcript" } else { "delta" }].as_str().unwrap_or("");
                                // Unknown/expired items cannot re-enter the 30-second window.
                                if let Some(at) = items.get(id) {
                                    let state = app.state::<AudioAssistant>(); let mut live = state.live.lock().unwrap();
                                    if live.accepts(generation) { live.transcript.update(id, *at, text, final_text, Instant::now()); }
                                }
                            },
                            Some("error" | "conversation.item.input_audio_transcription.failed") => return Err("Transcription failed. Check OpenAI access and billing in Settings.".into()),
                            _ => {}
                        }
                    },
                    Message::Close(_) => return Err("Transcription disconnected. Reconnecting…".into()),
                    Message::Ping(data) => { socket.send(Message::Pong(data)).await.map_err(|_| "Transcription disconnected.")?; },
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "explicit opt-in: streams only a synthetic 24 kHz mono PCM WAV fixture using the shared API key"]
    async fn openai_transcribes_synthetic_question() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let file = std::fs::read(
            std::env::var("PULSE_AUDIO_TEST_WAV")
                .expect("Set PULSE_AUDIO_TEST_WAV to a synthetic question WAV"),
        )
        .unwrap();
        assert_eq!(&file[..4], b"RIFF");
        let mut offset = 12;
        let mut pcm = None;
        while offset + 8 <= file.len() {
            let len = u32::from_le_bytes(file[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if &file[offset..offset + 4] == b"fmt " {
                assert_eq!(
                    u16::from_le_bytes(file[offset + 8..offset + 10].try_into().unwrap()),
                    1
                );
                assert_eq!(
                    u16::from_le_bytes(file[offset + 10..offset + 12].try_into().unwrap()),
                    1
                );
                assert_eq!(
                    u32::from_le_bytes(file[offset + 12..offset + 16].try_into().unwrap()),
                    24000
                );
            }
            if &file[offset..offset + 4] == b"data" {
                pcm = Some(&file[offset + 8..offset + 8 + len]);
                break;
            }
            offset += 8 + len + len % 2;
        }
        let mut socket = connect().await.unwrap();
        for chunk in pcm.unwrap().chunks(4800) {
            send(
                &mut socket,
                json!({"type":"input_audio_buffer.append", "audio":STANDARD.encode(chunk)}),
            )
            .await
            .unwrap();
        }
        send(&mut socket, json!({"type":"input_audio_buffer.commit"}))
            .await
            .unwrap();
        let transcript = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let event: Value = serde_json::from_str(&text).unwrap();
                assert_ne!(
                    event["type"], "error",
                    "Transcription returned an API error"
                );
                if event["type"] == "conversation.item.input_audio_transcription.completed" {
                    return event["transcript"].as_str().unwrap().to_string();
                }
            }
            panic!("Transcription disconnected before completion");
        })
        .await
        .unwrap();
        assert!(
            transcript.to_lowercase().contains("project"),
            "Synthetic transcript: {transcript}"
        );
        socket.close(None).await.unwrap();
    }

    #[test]
    fn transcription_is_audio_only_and_mono_pcm() {
        let v = session_update();
        assert_eq!(v["session"]["type"], "transcription");
        assert_eq!(v["session"]["audio"]["input"]["format"]["rate"], 24000);
        assert!(v["session"]["audio"]["input"]["turn_detection"].is_null());
    }
}
