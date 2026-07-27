use crate::redis_runtime::RedisRuntimeConfig;
use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use redis::AsyncCommands;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use trueshot_core::events::{EventBus, SystemEvent};
use uuid::Uuid;

const REDIS_CHANNEL: &str = "trueshot.events";
const RECENT_TTL: Duration = Duration::from_secs(2);
type RecentEvents = HashMap<String, (Instant, usize)>;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct DistributedEvent {
    origin: String,
    event: SystemEvent,
}

pub async fn start_redis_bridge(
    bus: Arc<EventBus>,
    redis_url: String,
    config: RedisRuntimeConfig,
) -> Result<()> {
    let client = redis::Client::open(redis_url).context("invalid Redis bridge URL")?;
    let origin = Uuid::new_v4().to_string();
    let recent = Arc::new(Mutex::new(RecentEvents::new()));
    let (event_sender, mut event_receiver) = mpsc::channel(config.event_buffer_capacity);

    let forwarder = tokio::spawn(forward_local_events(
        bus.clone(),
        event_sender,
        recent.clone(),
    ));
    let result = supervise_bridge(
        &client,
        &bus,
        &origin,
        &recent,
        &mut event_receiver,
        &config,
    )
    .await;
    forwarder.abort();
    result
}

async fn forward_local_events(
    bus: Arc<EventBus>,
    event_sender: mpsc::Sender<SystemEvent>,
    recent: Arc<Mutex<RecentEvents>>,
) {
    let mut receiver = bus.subscribe();
    loop {
        let event = match receiver.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    skipped,
                    "Redis bridge local event receiver lagged; events were dropped"
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        if should_skip(&event, &recent).await {
            continue;
        }
        if event_sender.send(event).await.is_err() {
            return;
        }
    }
}

async fn supervise_bridge(
    client: &redis::Client,
    bus: &Arc<EventBus>,
    origin: &str,
    recent: &Arc<Mutex<RecentEvents>>,
    event_receiver: &mut mpsc::Receiver<SystemEvent>,
    config: &RedisRuntimeConfig,
) -> Result<()> {
    let mut failures = 0u32;
    let mut pending_event = None;

    loop {
        let session_started = Instant::now();
        match run_bridge_session(
            client,
            bus,
            origin,
            recent,
            event_receiver,
            &mut pending_event,
            config,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                if session_started.elapsed() >= config.reconnect_max {
                    failures = 0;
                }
                let delay = config.reconnect_delay(failures);
                failures = failures.saturating_add(1);
                tracing::warn!(
                    error = %error,
                    retry_ms = delay.as_millis(),
                    "Redis event bridge disconnected; reconnecting"
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn run_bridge_session(
    client: &redis::Client,
    bus: &Arc<EventBus>,
    origin: &str,
    recent: &Arc<Mutex<RecentEvents>>,
    event_receiver: &mut mpsc::Receiver<SystemEvent>,
    pending_event: &mut Option<SystemEvent>,
    config: &RedisRuntimeConfig,
) -> Result<()> {
    let mut pubsub = tokio::time::timeout(config.connect_timeout, client.get_async_pubsub())
        .await
        .context("Redis pubsub connection timed out")?
        .context("Redis pubsub connection failed")?;
    tokio::time::timeout(config.response_timeout, pubsub.subscribe(REDIS_CHANNEL))
        .await
        .context("Redis subscription timed out")?
        .context("Redis subscription failed")?;
    let mut publisher = tokio::time::timeout(
        config.connect_timeout,
        client.get_connection_manager_with_config(config.manager_config()),
    )
    .await
    .context("Redis publisher connection timed out")?
    .context("Redis publisher connection failed")?;

    if let Some(event) = pending_event.take() {
        if let Err(error) = publish_event(&mut publisher, origin, &event).await {
            *pending_event = Some(event);
            return Err(error);
        }
    }

    let mut messages = pubsub.on_message();
    loop {
        tokio::select! {
            event = event_receiver.recv() => {
                let Some(event) = event else {
                    return Ok(());
                };
                if let Err(error) = publish_event(&mut publisher, origin, &event).await {
                    *pending_event = Some(event);
                    return Err(error);
                }
            }
            message = messages.next() => {
                let Some(message) = message else {
                    return Err(anyhow!("Redis pubsub stream closed"));
                };
                let payload: String = match message.get_payload() {
                    Ok(payload) => payload,
                    Err(error) => {
                        tracing::warn!(error = %error, "Redis bridge ignored malformed payload");
                        continue;
                    }
                };
                let event: DistributedEvent = match serde_json::from_str(&payload) {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::warn!(error = %error, "Redis bridge ignored invalid event JSON");
                        continue;
                    }
                };
                if event.origin == origin {
                    continue;
                }
                mark_recent(&event.event, recent).await;
                bus.publish(event.event);
            }
        }
    }
}

async fn publish_event(
    publisher: &mut redis::aio::ConnectionManager,
    origin: &str,
    event: &SystemEvent,
) -> Result<()> {
    let payload = serde_json::to_string(&DistributedEvent {
        origin: origin.to_string(),
        event: event.clone(),
    })
    .context("Redis event serialization failed")?;
    let _: usize = publisher
        .publish(REDIS_CHANNEL, payload)
        .await
        .context("Redis event publish failed")?;
    Ok(())
}

async fn should_skip(event: &SystemEvent, recent: &Arc<Mutex<RecentEvents>>) -> bool {
    let fingerprint = event_fingerprint(event);
    let mut guard = recent.lock().await;
    prune_recent(&mut guard);
    let Some((_, remaining)) = guard.get_mut(&fingerprint) else {
        return false;
    };
    *remaining -= 1;
    if *remaining == 0 {
        guard.remove(&fingerprint);
    }
    true
}

async fn mark_recent(event: &SystemEvent, recent: &Arc<Mutex<RecentEvents>>) {
    let fingerprint = event_fingerprint(event);
    let mut guard = recent.lock().await;
    prune_recent(&mut guard);
    guard
        .entry(fingerprint)
        .and_modify(|(timestamp, count)| {
            *timestamp = Instant::now();
            *count = count.saturating_add(1);
        })
        .or_insert((Instant::now(), 1));
}

fn prune_recent(map: &mut RecentEvents) {
    map.retain(|_, (timestamp, _)| timestamp.elapsed() <= RECENT_TTL);
}

fn event_fingerprint(event: &SystemEvent) -> String {
    let payload = serde_json::to_vec(event).unwrap_or_default();
    let digest = Sha256::digest(&payload);
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RedisRuntimeConfig {
        RedisRuntimeConfig {
            connect_timeout: Duration::from_millis(100),
            response_timeout: Duration::from_millis(200),
            reconnect_initial: Duration::from_millis(20),
            reconnect_max: Duration::from_millis(100),
            event_buffer_capacity: 32,
        }
    }

    #[test]
    fn fingerprints_are_stable_and_event_sensitive() {
        let first = SystemEvent::CaptureStarted(1);
        let same = SystemEvent::CaptureStarted(1);
        let different = SystemEvent::CaptureStarted(2);
        assert_eq!(event_fingerprint(&first), event_fingerprint(&same));
        assert_ne!(event_fingerprint(&first), event_fingerprint(&different));
    }

    #[tokio::test]
    async fn recent_remote_events_consume_one_echo_token_each() {
        let recent = Arc::new(Mutex::new(HashMap::new()));
        let event = SystemEvent::ScanComplete;
        assert!(!should_skip(&event, &recent).await);
        mark_recent(&event, &recent).await;
        mark_recent(&event, &recent).await;
        assert!(should_skip(&event, &recent).await);
        assert!(should_skip(&event, &recent).await);
        assert!(!should_skip(&event, &recent).await);
    }

    #[tokio::test]
    async fn invalid_bridge_url_fails_without_spawning_a_retry_loop() {
        let result = start_redis_bridge(
            Arc::new(EventBus::new()),
            "not a redis URL".to_string(),
            test_config(),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn configured_redis_bridges_events_between_buses() {
        let Ok(url) = std::env::var("TRUESHOT_REDIS_TEST_URL") else {
            return;
        };
        let first_bus = Arc::new(EventBus::new());
        let second_bus = Arc::new(EventBus::new());
        let first_bridge = tokio::spawn(start_redis_bridge(
            first_bus.clone(),
            url.clone(),
            test_config(),
        ));
        let second_bridge =
            tokio::spawn(start_redis_bridge(second_bus.clone(), url, test_config()));
        let mut receiver = second_bus.subscribe();

        async fn wait_for_relay(
            source: &EventBus,
            receiver: &mut tokio::sync::broadcast::Receiver<SystemEvent>,
            capture_id: u32,
        ) -> Result<(), tokio::time::error::Elapsed> {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    source.publish(SystemEvent::CaptureStarted(capture_id));
                    if let Ok(Ok(SystemEvent::CaptureStarted(received_id))) =
                        tokio::time::timeout(Duration::from_millis(100), receiver.recv()).await
                    {
                        if received_id == capture_id {
                            return;
                        }
                    }
                }
            })
            .await
        }

        wait_for_relay(&first_bus, &mut receiver, 73)
            .await
            .expect("initial Redis relay should become ready");

        let mut admin = redis::Client::open(std::env::var("TRUESHOT_REDIS_TEST_URL").unwrap())
            .unwrap()
            .get_multiplexed_async_connection()
            .await
            .unwrap();
        let killed: usize = redis::cmd("CLIENT")
            .arg("KILL")
            .arg("TYPE")
            .arg("PUBSUB")
            .arg("SKIPME")
            .arg("YES")
            .query_async(&mut admin)
            .await
            .unwrap();
        assert!(killed >= 2, "both bridge subscriptions should be connected");

        wait_for_relay(&first_bus, &mut receiver, 74)
            .await
            .expect("Redis relay should recover after its subscriptions are killed");

        first_bridge.abort();
        second_bridge.abort();
    }
}
