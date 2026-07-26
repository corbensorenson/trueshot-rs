use anyhow::{Context, Result};
use futures::StreamExt;
use redis::AsyncCommands;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use trueshot_core::events::{EventBus, SystemEvent};
use uuid::Uuid;

const REDIS_CHANNEL: &str = "trueshot.events";
const RECENT_TTL: Duration = Duration::from_secs(2);

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct DistributedEvent {
    origin: String,
    event: SystemEvent,
}

pub async fn start_redis_bridge(bus: Arc<EventBus>, redis_url: String) -> Result<()> {
    let client = redis::Client::open(redis_url)?;
    let origin = Uuid::new_v4().to_string();
    let recent = Arc::new(Mutex::new(HashMap::<String, Instant>::new()));

    let mut pubsub_conn = client
        .get_async_connection()
        .await
        .context("redis pubsub connection failed")?;
    let mut pubsub = pubsub_conn.into_pubsub();
    pubsub
        .subscribe(REDIS_CHANNEL)
        .await
        .context("redis subscribe failed")?;
    let mut pubsub_stream = pubsub.on_message();

    let mut publish_conn = client
        .get_async_connection()
        .await
        .context("redis publish connection failed")?;

    let bus_to_redis = {
        let recent = recent.clone();
        let origin = origin.clone();
        let mut rx = bus.subscribe();
        async move {
            loop {
                let event = match rx.recv().await {
                    Ok(event) => event,
                    Err(_) => continue,
                };
                if should_skip(&event, &recent).await {
                    continue;
                }
                let payload = DistributedEvent {
                    origin: origin.clone(),
                    event,
                };
                let serialized = serde_json::to_string(&payload).unwrap_or_default();
                let _: () = publish_conn
                    .publish(REDIS_CHANNEL, serialized)
                    .await
                    .unwrap_or(());
            }
        }
    };

    let redis_to_bus = {
        let recent = recent.clone();
        async move {
            while let Some(message) = pubsub_stream.next().await {
                let payload: Result<String, _> = message.get_payload();
                let Ok(payload) = payload else {
                    continue;
                };
                let Ok(event) = serde_json::from_str::<DistributedEvent>(&payload) else {
                    continue;
                };
                if event.origin == origin {
                    continue;
                }
                mark_recent(&event.event, &recent).await;
                bus.publish(event.event);
            }
        }
    };

    tokio::select! {
        _ = bus_to_redis => Ok(()),
        _ = redis_to_bus => Ok(()),
    }
}

async fn should_skip(event: &SystemEvent, recent: &Arc<Mutex<HashMap<String, Instant>>>) -> bool {
    let fingerprint = event_fingerprint(event);
    let mut guard = recent.lock().await;
    prune_recent(&mut guard);
    if let Some(ts) = guard.get(&fingerprint) {
        if ts.elapsed() <= RECENT_TTL {
            return true;
        }
    }
    false
}

async fn mark_recent(event: &SystemEvent, recent: &Arc<Mutex<HashMap<String, Instant>>>) {
    let fingerprint = event_fingerprint(event);
    let mut guard = recent.lock().await;
    prune_recent(&mut guard);
    guard.insert(fingerprint, Instant::now());
}

fn prune_recent(map: &mut HashMap<String, Instant>) {
    map.retain(|_, ts| ts.elapsed() <= RECENT_TTL);
}

fn event_fingerprint(event: &SystemEvent) -> String {
    let payload = serde_json::to_vec(event).unwrap_or_default();
    let digest = Sha256::digest(&payload);
    hex::encode(digest)
}
