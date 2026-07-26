// use image::RgbaImage;
use redis::Commands;
// use std::sync::Arc;

/// Distributed Cache for shared state across studio machines
pub struct StudioCache {
    client: redis::Client,
}

impl StudioCache {
    pub fn new(connection_string: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(connection_string)?;
        Ok(Self { client })
    }

    pub fn set_calibration(&self, camera_id: &str, params: &str) -> anyhow::Result<()> {
        let mut con = self.client.get_connection()?;
        con.set::<_, _, ()>(format!("calib:{}", camera_id), params)?;
        Ok(())
    }

    pub fn get_calibration(&self, camera_id: &str) -> anyhow::Result<String> {
        let mut con = self.client.get_connection()?;
        Ok(con.get(format!("calib:{}", camera_id))?)
    }
}
