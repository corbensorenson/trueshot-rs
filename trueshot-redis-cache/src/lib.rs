use anyhow::Context;
use redis::Commands;
use std::time::Duration;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(750);
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(1);

/// Distributed Cache for shared state across studio machines
pub struct StudioCache {
    client: redis::Client,
    connect_timeout: Duration,
    io_timeout: Duration,
}

impl StudioCache {
    pub fn new(connection_string: &str) -> anyhow::Result<Self> {
        Self::with_timeouts(
            connection_string,
            DEFAULT_CONNECT_TIMEOUT,
            DEFAULT_IO_TIMEOUT,
        )
    }

    pub fn with_timeouts(
        connection_string: &str,
        connect_timeout: Duration,
        io_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let client = redis::Client::open(connection_string)?;
        Ok(Self {
            client,
            connect_timeout,
            io_timeout,
        })
    }

    fn connection(&self) -> anyhow::Result<redis::Connection> {
        let connection = self
            .client
            .get_connection_with_timeout(self.connect_timeout)
            .context("Redis cache connection failed")?;
        connection
            .set_read_timeout(Some(self.io_timeout))
            .context("failed to set Redis cache read timeout")?;
        connection
            .set_write_timeout(Some(self.io_timeout))
            .context("failed to set Redis cache write timeout")?;
        Ok(connection)
    }

    pub fn set_calibration(&self, camera_id: &str, params: &str) -> anyhow::Result<()> {
        let mut con = self.connection()?;
        con.set::<_, _, ()>(format!("calib:{}", camera_id), params)?;
        Ok(())
    }

    pub fn get_calibration(&self, camera_id: &str) -> anyhow::Result<String> {
        let mut con = self.connection()?;
        Ok(con.get(format!("calib:{}", camera_id))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn invalid_url_is_rejected() {
        assert!(StudioCache::new("not a Redis URL").is_err());
    }

    #[test]
    fn unavailable_redis_is_bounded() {
        let cache = StudioCache::with_timeouts(
            "redis://127.0.0.1:1/",
            Duration::from_millis(50),
            Duration::from_millis(50),
        )
        .unwrap();
        let started = Instant::now();
        assert!(cache.get_calibration("offline").is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn configured_redis_round_trips_calibration() {
        let Ok(url) = std::env::var("TRUESHOT_REDIS_TEST_URL") else {
            return;
        };
        let cache = StudioCache::new(&url).unwrap();
        let camera_id = format!("test:{}", std::process::id());
        cache.set_calibration(&camera_id, "params").unwrap();
        assert_eq!(cache.get_calibration(&camera_id).unwrap(), "params");

        let mut connection = cache.connection().unwrap();
        let _: () = connection
            .del(format!("calib:{camera_id}"))
            .expect("test key should be deleted");
    }
}
