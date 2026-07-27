use crate::config::ServerConfig;
use anyhow::{anyhow, Context, Result};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::AsyncCommands;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 750;
const DEFAULT_RESPONSE_TIMEOUT_MS: u64 = 1_000;
const DEFAULT_RECONNECT_INITIAL_MS: u64 = 100;
const DEFAULT_RECONNECT_MAX_MS: u64 = 5_000;
const DEFAULT_EVENT_BUFFER_CAPACITY: usize = 1_024;

#[derive(Debug, Clone)]
pub struct RedisRuntimeConfig {
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub reconnect_initial: Duration,
    pub reconnect_max: Duration,
    pub event_buffer_capacity: usize,
}

impl RedisRuntimeConfig {
    pub fn from_server(config: &ServerConfig) -> Self {
        let connect_timeout = Duration::from_millis(
            config
                .redis_connect_timeout_ms
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS)
                .clamp(10, 30_000),
        );
        let response_timeout = Duration::from_millis(
            config
                .redis_response_timeout_ms
                .unwrap_or(DEFAULT_RESPONSE_TIMEOUT_MS)
                .clamp(10, 60_000),
        );
        let reconnect_initial = Duration::from_millis(
            config
                .redis_reconnect_initial_ms
                .unwrap_or(DEFAULT_RECONNECT_INITIAL_MS)
                .clamp(10, 60_000),
        );
        let reconnect_max = Duration::from_millis(
            config
                .redis_reconnect_max_ms
                .unwrap_or(DEFAULT_RECONNECT_MAX_MS)
                .clamp(reconnect_initial.as_millis() as u64, 300_000),
        );
        let event_buffer_capacity = config
            .redis_event_buffer_capacity
            .unwrap_or(DEFAULT_EVENT_BUFFER_CAPACITY)
            .clamp(16, 65_536);
        Self {
            connect_timeout,
            response_timeout,
            reconnect_initial,
            reconnect_max,
            event_buffer_capacity,
        }
    }

    pub fn reconnect_delay(&self, failures: u32) -> Duration {
        let exponent = failures.min(20);
        let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
        let millis = (self.reconnect_initial.as_millis() as u64)
            .saturating_mul(multiplier)
            .min(self.reconnect_max.as_millis() as u64);
        Duration::from_millis(millis)
    }

    pub(crate) fn manager_config(&self) -> ConnectionManagerConfig {
        ConnectionManagerConfig::new()
            .set_exponent_base(2)
            .set_factor(self.reconnect_initial.as_millis() as u64)
            .set_max_delay(self.reconnect_max.as_millis() as u64)
            .set_number_of_retries(3)
            .set_connection_timeout(self.connect_timeout)
            .set_response_timeout(self.response_timeout)
    }
}

struct PoolState {
    manager: Option<ConnectionManager>,
    failures: u32,
    retry_after: Option<Instant>,
}

/// Lazy, reconnecting Redis access that preserves local-only operation when
/// the optional Redis service is absent.
pub struct RedisPool {
    client: redis::Client,
    config: RedisRuntimeConfig,
    state: Mutex<PoolState>,
}

impl RedisPool {
    pub fn new(redis_url: &str, config: RedisRuntimeConfig) -> Result<Arc<Self>> {
        let client = redis::Client::open(redis_url).context("invalid Redis URL")?;
        Ok(Arc::new(Self {
            client,
            config,
            state: Mutex::new(PoolState {
                manager: None,
                failures: 0,
                retry_after: None,
            }),
        }))
    }

    pub async fn connection(&self) -> Result<ConnectionManager> {
        let mut state = self.state.lock().await;
        if let Some(manager) = state.manager.as_ref() {
            return Ok(manager.clone());
        }
        if let Some(retry_after) = state.retry_after {
            if retry_after > Instant::now() {
                return Err(anyhow!("Redis reconnect is cooling down"));
            }
        }

        let connection = tokio::time::timeout(
            self.config.connect_timeout,
            self.client
                .get_connection_manager_with_config(self.config.manager_config()),
        )
        .await
        .context("Redis connection timed out")
        .and_then(|connection| connection.context("Redis connection failed"));

        match connection {
            Ok(manager) => {
                state.manager = Some(manager.clone());
                state.failures = 0;
                state.retry_after = None;
                Ok(manager)
            }
            Err(error) => {
                let delay = self.config.reconnect_delay(state.failures);
                state.failures = state.failures.saturating_add(1);
                state.retry_after = Some(Instant::now() + delay);
                Err(error)
            }
        }
    }

    pub async fn set_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let payload = serde_json::to_string(value).context("Redis cache serialization failed")?;
        let mut connection = self.connection().await?;
        let _: () = connection
            .set(key, payload)
            .await
            .context("Redis cache write failed")?;
        Ok(())
    }

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let mut connection = self.connection().await?;
        let payload: Option<String> = connection
            .get(key)
            .await
            .context("Redis cache read failed")?;
        payload
            .map(|payload| {
                serde_json::from_str(&payload).context("Redis cache payload is malformed")
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::AsyncCommands;

    fn test_config() -> RedisRuntimeConfig {
        RedisRuntimeConfig {
            connect_timeout: Duration::from_millis(40),
            response_timeout: Duration::from_millis(100),
            reconnect_initial: Duration::from_millis(200),
            reconnect_max: Duration::from_millis(400),
            event_buffer_capacity: 32,
        }
    }

    #[test]
    fn reconnect_delay_is_exponential_and_capped() {
        let config = test_config();
        assert_eq!(config.reconnect_delay(0), Duration::from_millis(200));
        assert_eq!(config.reconnect_delay(1), Duration::from_millis(400));
        assert_eq!(config.reconnect_delay(8), Duration::from_millis(400));
    }

    #[tokio::test]
    async fn unavailable_redis_fails_bounded_and_throttles_retries() {
        let pool = RedisPool::new("redis://127.0.0.1:1/", test_config()).unwrap();
        let started = Instant::now();
        assert!(pool.connection().await.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));

        let retry_started = Instant::now();
        let error = match pool.connection().await {
            Ok(_) => panic!("Redis reconnect cooldown should reject an immediate retry"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("cooling down"));
        assert!(retry_started.elapsed() < Duration::from_millis(20));
    }

    #[tokio::test]
    async fn configured_redis_round_trips_json() {
        let Ok(url) = std::env::var("TRUESHOT_REDIS_TEST_URL") else {
            return;
        };
        let pool = RedisPool::new(&url, test_config()).unwrap();
        let key = format!("trueshot:test:{}", uuid::Uuid::new_v4());
        let value = serde_json::json!({"camera": "test", "rms": 0.12});
        pool.set_json(&key, &value).await.unwrap();
        let loaded: Option<serde_json::Value> = pool.get_json(&key).await.unwrap();
        assert_eq!(loaded, Some(value));

        let mut connection = pool.connection().await.unwrap();
        let _: () = connection.del(key).await.unwrap();
    }
}
