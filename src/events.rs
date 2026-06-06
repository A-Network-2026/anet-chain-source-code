use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainEvent {
    pub event_type: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

impl ChainEvent {
    pub fn new(event_type: &str) -> Self {
        Self {
            event_type: event_type.to_owned(),
            timestamp: Utc::now(),
            tx_hash: None,
            attributes: BTreeMap::new(),
        }
    }

    pub fn with_tx_hash(mut self, tx_hash: impl Into<String>) -> Self {
        self.tx_hash = Some(tx_hash.into());
        self
    }

    pub fn attr(mut self, key: &str, value: impl ToString) -> Self {
        self.attributes.insert(key.to_owned(), value.to_string());
        self
    }
}
