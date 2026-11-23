//! Anonymous, best-effort desktop usage counts. Only fixed event names enter the queue.
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Serialize;
use std::{
    sync::{mpsc as sync_channel, OnceLock},
    time::Duration,
};
use tokio::sync::mpsc;

const EVENTS_URL: &str = "https://analytics.gloscai.com/api/v0/events";
const APP_KEY: &str = "A-SH-0831749366";
// Aptabase accepts up to 25 events per request.
const BATCH_SIZE: usize = 25;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemProps {
    os_name: &'static str,
    os_version: String,
    app_version: String,
    sdk_version: &'static str,
}

impl SystemProps {
    fn desktop(app_version: String) -> Self {
        let os_name = match std::env::consts::OS {
            "windows" => "Windows",
            "macos" => "macOS",
            "linux" => "Linux",
            other => other,
        };
        Self {
            os_name,
            os_version: os_info::get().version().to_string(),
            app_version,
            sdk_version: "amm-http@1",
        }
    }
}

struct Event {
    name: &'static str,
    timestamp: DateTime<Utc>,
}

enum Message {
    Track(Event),
    Finish(sync_channel::Sender<()>),
}

static EVENTS: OnceLock<mpsc::Sender<Message>> = OnceLock::new();

pub(crate) fn start(version: String) {
    if cfg!(any(test, debug_assertions)) || std::env::var_os("AMM_DISABLE_ANALYTICS").is_some() {
        return;
    }
    let (sender, receiver) = mpsc::channel(100);
    if EVENTS.set(sender).is_err() {
        return;
    }
    track("app_started");
    tauri::async_runtime::spawn(async move {
        if let Ok(client) = Client::builder()
            .timeout(Duration::from_secs(4))
            .redirect(reqwest::redirect::Policy::none())
            .build()
        {
            dispatch(
                receiver,
                client,
                EVENTS_URL,
                APP_KEY,
                SystemProps::desktop(version),
            )
            .await;
        }
    });
}

pub(crate) fn enabled() -> bool {
    EVENTS.get().is_some()
}

pub(crate) fn track(name: &'static str) {
    if let Some(events) = EVENTS.get() {
        let _ = events.try_send(Message::Track(Event {
            name,
            timestamp: Utc::now(),
        }));
    }
}

pub(crate) fn finish() {
    if let Some(events) = EVENTS.get() {
        let (sent, done) = sync_channel::channel();
        if events.try_send(Message::Finish(sent)).is_ok() {
            // Allow a final in-flight request to finish without holding an offline app open.
            let _ = done.recv_timeout(Duration::from_millis(500));
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestEvent<'a> {
    event_name: &'static str,
    timestamp: DateTime<Utc>,
    session_id: String,
    system_props: &'a SystemProps,
}

async fn dispatch(
    mut receiver: mpsc::Receiver<Message>,
    client: Client,
    url: &str,
    key: &str,
    system_props: SystemProps,
) {
    let mut session_day = None;
    let mut session_id = String::new();
    let mut messages = Vec::with_capacity(BATCH_SIZE);
    while receiver.recv_many(&mut messages, BATCH_SIZE).await > 0 {
        let mut batch = Vec::with_capacity(messages.len());
        let mut finish = None;
        for message in messages.drain(..) {
            match message {
                Message::Track(event) => {
                    let day = event.timestamp.date_naive();
                    if session_day != Some(day) {
                        // Aptabase numeric session IDs contain epoch seconds and eight random digits.
                        session_id = format!(
                            "{}{:08}",
                            event.timestamp.timestamp(),
                            rand::random_range(0..100_000_000u32)
                        );
                        session_day = Some(day);
                    }
                    batch.push(RequestEvent {
                        event_name: event.name,
                        timestamp: event.timestamp,
                        session_id: session_id.clone(),
                        system_props: &system_props,
                    });
                }
                Message::Finish(signal) => {
                    finish = Some(signal);
                    break;
                }
            }
        }
        if !batch.is_empty() {
            let _ = client
                .post(url)
                .header("App-Key", key)
                .json(&batch)
                .send()
                .await;
        }
        if let Some(signal) = finish {
            let _ = signal.send(());
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Bytes, http::HeaderMap, routing::post, Router};
    use serde_json::{json, Value};

    #[test]
    fn test_desktop_lifecycle_never_registers_a_production_sender() {
        start("0.1.0".to_owned());
        track("app_started");
        finish();
        assert!(EVENTS.get().is_none());
    }

    #[tokio::test]
    async fn loopback_ingestion_sends_only_counts_and_version_dimensions() {
        let (captured, mut requests) = mpsc::unbounded_channel();
        let router = Router::new().route(
            "/api/v0/events",
            post(move |headers: HeaderMap, body: Bytes| async move {
                let body = serde_json::from_slice::<Vec<Value>>(&body).unwrap();
                captured.send((headers, body)).unwrap();
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/api/v0/events", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let (sender, receiver) = mpsc::channel(50);
        let first = DateTime::parse_from_rfc3339("2026-01-01T23:59:59Z")
            .unwrap()
            .to_utc();
        for i in 0..=BATCH_SIZE {
            let name = if i == 0 {
                "app_started"
            } else {
                "game_launch_requested"
            };
            let timestamp = if i == BATCH_SIZE {
                first + chrono::Duration::seconds(2)
            } else {
                first
            };
            sender
                .try_send(Message::Track(Event { name, timestamp }))
                .unwrap();
        }
        let (sent, done) = sync_channel::channel();
        sender.try_send(Message::Finish(sent)).unwrap();
        let props = SystemProps {
            os_name: "Windows",
            os_version: "11".to_owned(),
            app_version: "1.2.3".to_owned(),
            sdk_version: "amm-http@1",
        };
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(4))
            .build()
            .unwrap();
        dispatch(receiver, client, &endpoint, "A-DEV-0000000000", props).await;
        done.try_recv().unwrap();
        let (headers, batch) = requests.try_recv().unwrap();
        assert_eq!(headers["App-Key"], "A-DEV-0000000000");
        assert_eq!(batch.len(), BATCH_SIZE);
        let session = batch[0]["sessionId"].as_str().unwrap();
        assert_eq!(
            session.parse::<i64>().unwrap() / 100_000_000,
            first.timestamp()
        );
        for (i, event) in batch.iter().enumerate() {
            assert_eq!(
                event,
                &json!({
                    "eventName": if i == 0 { "app_started" } else { "game_launch_requested" },
                    "timestamp": first, "sessionId": session,
                    "systemProps": { "osName": "Windows", "osVersion": "11", "appVersion": "1.2.3", "sdkVersion": "amm-http@1" }
                })
            );
        }
        let (_, next) = requests.try_recv().unwrap();
        assert_eq!(next.len(), 1);
        assert_ne!(next[0]["sessionId"], session);
        assert!(requests.try_recv().is_err());
        server.abort();
    }
}
