use crate::scheduler::{Job, RemoteJobPayload};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    pub id: Uuid,
    pub hostname: String,
    pub base_url: String, // e.g. http://192.168.1.5:3000
    pub status: NodeStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeStatus {
    Idle,
    Busy,
    Offline,
}

pub struct ClusterManager {
    nodes: Arc<Mutex<HashMap<Uuid, ClusterNode>>>,
    client: reqwest::Client,
}

impl Default for ClusterManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusterManager {
    pub fn new() -> Self {
        Self {
            nodes: Arc::new(Mutex::new(HashMap::new())),
            client: reqwest::Client::new(),
        }
    }

    pub async fn add_node(&self, url: String) -> Result<Uuid> {
        // Ping node
        let resp = self
            .client
            .get(format!("{}/health", url))
            .send()
            .await
            .context("Failed to ping node")?;

        if !resp.status().is_success() {
            anyhow::bail!("Node unhealthy: {}", resp.status());
        }

        // Register
        let id = Uuid::new_v4();
        let node = ClusterNode {
            id,
            hostname: "remote-worker".into(),
            base_url: url,
            status: NodeStatus::Idle,
        };

        self.nodes.lock().await.insert(id, node);
        Ok(id)
    }

    /// Dispatch a job to the first available idle node
    pub async fn dispatch_job(&self, job: Box<dyn Job + Send + Sync>) -> Result<Uuid> {
        let mut nodes = self.nodes.lock().await;

        // Simple Round-Robin or First-Fit
        for node in nodes.values_mut() {
            if node.status == NodeStatus::Idle {
                let payload = job.remote_payload().ok_or_else(|| {
                    anyhow::anyhow!("Job type cannot be serialized for remote dispatch")
                })?;

                let request = RemoteJobRequest::new(payload);

                let url = format!("{}/api/jobs", node.base_url.trim_end_matches('/'));
                let resp = self
                    .client
                    .post(url)
                    .json(&request)
                    .send()
                    .await
                    .context("Failed to dispatch job to node")?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    return Err(anyhow::anyhow!(
                        "Remote job dispatch failed: {} {}",
                        status,
                        body
                    ));
                }

                node.status = NodeStatus::Busy;
                return Ok(node.id);
            }
        }

        anyhow::bail!("No idle nodes available")
    }
}

#[derive(Debug, Clone, Serialize)]
struct RemoteJobRequest {
    id: Uuid,
    kind: String,
    name: String,
    payload: serde_json::Value,
}

impl RemoteJobRequest {
    fn new(payload: RemoteJobPayload) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: payload.kind,
            name: payload.name,
            payload: payload.payload,
        }
    }
}
