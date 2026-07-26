use serde::{Serialize, Deserialize};
use reqwest::blocking::Client;

/// Webhook Notifier (Slack/Teams)
pub struct WebhookNotifier {
    url: String,
    client: Client,
}

impl WebhookNotifier {
    pub fn new(url: &str) -> Self {
        Self { url: url.into(), client: Client::new() }
    }
    
    pub fn send_scan_complete(&self, project: &str, status: &str) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "text": format!("Scan Complete: *{}*\nStatus: {}", project, status)
        });
        
        self.client.post(&self.url).json(&payload).send()?;
        Ok(())
    }
}
