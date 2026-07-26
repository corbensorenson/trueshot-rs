//! Secure Token Storage
//!
//! Stores OAuth tokens and credentials in the OS keychain.
//! A lightweight on-disk index tracks providers without storing secrets.

use chrono::{DateTime, Utc};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ============================================================================
// Types
// ============================================================================

/// OAuth token pair with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    pub provider: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TokenIndex {
    providers: Vec<String>,
}

/// Token storage errors
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Token not found: {0}")]
    NotFound(String),
    #[error("Keyring error: {0}")]
    Keyring(String),
}

// ============================================================================
// Token Store
// ============================================================================

/// Secure token storage backed by OS keychain.
pub struct TokenStore {
    service: String,
    index_path: PathBuf,
    index: Mutex<TokenIndex>,
}

impl TokenStore {
    /// Create or open token store at path.
    /// Secrets are stored in the OS keychain; `data_dir` holds a provider index only.
    pub fn new(data_dir: impl Into<PathBuf>) -> Result<Self, TokenStoreError> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir).map_err(|e| TokenStoreError::Io(e.to_string()))?;
        let index_path = data_dir.join("tokens.index.json");
        let index = load_index(&index_path)?;

        Ok(Self {
            service: "trueshot".to_string(),
            index_path,
            index: Mutex::new(index),
        })
    }

    /// Save or update a token
    pub fn save_token(&self, token: &StoredToken) -> Result<(), TokenStoreError> {
        let entry = self.entry_for(&token.provider)?;
        let payload = serde_json::to_string(token)
            .map_err(|e| TokenStoreError::Serialization(e.to_string()))?;
        entry
            .set_password(&payload)
            .map_err(|e| TokenStoreError::Keyring(e.to_string()))?;

        let mut index = self.index.lock().unwrap();
        if !index.providers.iter().any(|p| p == &token.provider) {
            index.providers.push(token.provider.clone());
            index.providers.sort();
        }
        persist_index(&self.index_path, &index)?;
        Ok(())
    }

    /// Load a token by provider
    pub fn load_token(&self, provider: &str) -> Result<StoredToken, TokenStoreError> {
        let entry = self.entry_for(provider)?;
        let payload = entry.get_password().map_err(|e| match e {
            keyring::Error::NoEntry => TokenStoreError::NotFound(provider.to_string()),
            _ => TokenStoreError::Keyring(e.to_string()),
        })?;
        let token = serde_json::from_str(&payload)
            .map_err(|e| TokenStoreError::Serialization(e.to_string()))?;
        Ok(token)
    }

    /// List all stored providers
    pub fn list_providers(&self) -> Result<Vec<String>, TokenStoreError> {
        let mut index = self.index.lock().unwrap();
        // Clean stale providers if keyring entries were removed externally.
        index
            .providers
            .retain(|provider| self.entry_exists(provider));
        persist_index(&self.index_path, &index)?;
        Ok(index.providers.clone())
    }

    /// Delete a token
    pub fn delete_token(&self, provider: &str) -> Result<(), TokenStoreError> {
        let entry = self.entry_for(provider)?;
        entry.delete_password().map_err(|e| match e {
            keyring::Error::NoEntry => TokenStoreError::NotFound(provider.to_string()),
            _ => TokenStoreError::Keyring(e.to_string()),
        })?;

        let mut index = self.index.lock().unwrap();
        index.providers.retain(|p| p != provider);
        persist_index(&self.index_path, &index)?;
        Ok(())
    }

    /// Check if token is expired
    pub fn is_expired(&self, provider: &str) -> Result<bool, TokenStoreError> {
        let token = self.load_token(provider)?;
        if let Some(expires_at) = token.expires_at {
            Ok(Utc::now() > expires_at)
        } else {
            Ok(false)
        }
    }

    /// Get all tokens that need refresh (expired or expiring soon)
    pub fn tokens_needing_refresh(&self) -> Result<Vec<StoredToken>, TokenStoreError> {
        let providers = self.list_providers()?;
        let now = Utc::now();
        let refresh_threshold = now + chrono::Duration::minutes(5);

        let mut tokens = Vec::new();
        for provider in providers {
            if let Ok(token) = self.load_token(&provider) {
                if let Some(expires_at) = token.expires_at {
                    if expires_at < refresh_threshold {
                        tokens.push(token);
                    }
                }
            }
        }
        Ok(tokens)
    }

    fn entry_for(&self, provider: &str) -> Result<Entry, TokenStoreError> {
        Entry::new(&self.service, &format!("oauth:{}", provider))
            .map_err(|e| TokenStoreError::Keyring(e.to_string()))
    }

    fn entry_exists(&self, provider: &str) -> bool {
        match self.entry_for(provider) {
            Ok(entry) => entry.get_password().is_ok(),
            Err(_) => false,
        }
    }
}

fn load_index(path: &Path) -> Result<TokenIndex, TokenStoreError> {
    if !path.exists() {
        return Ok(TokenIndex::default());
    }
    let content = std::fs::read_to_string(path).map_err(|e| TokenStoreError::Io(e.to_string()))?;
    serde_json::from_str(&content).map_err(|e| TokenStoreError::Serialization(e.to_string()))
}

fn persist_index(path: &Path, index: &TokenIndex) -> Result<(), TokenStoreError> {
    let json = serde_json::to_string_pretty(index)
        .map_err(|e| TokenStoreError::Serialization(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| TokenStoreError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| TokenStoreError::Io(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    #[ignore = "Requires OS keychain availability"]
    fn test_token_store_crud() {
        let dir = temp_dir().join(format!("trueshot_test_{}", uuid::Uuid::new_v4()));
        let store = TokenStore::new(&dir).unwrap();

        let token = StoredToken {
            provider: "google_drive".to_string(),
            access_token: "test_access".to_string(),
            refresh_token: Some("test_refresh".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            email: Some("test@example.com".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        store.save_token(&token).unwrap();
        let loaded = store.load_token("google_drive").unwrap();
        assert_eq!(loaded.provider, "google_drive");

        let providers = store.list_providers().unwrap();
        assert!(providers.contains(&"google_drive".to_string()));

        store.delete_token("google_drive").unwrap();
        assert!(store.load_token("google_drive").is_err());

        let _ = std::fs::remove_dir_all(dir);
    }
}
