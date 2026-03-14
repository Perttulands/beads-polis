//! Append-only event types for the JSONL log.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Actor identity, read from POLIS_ACTOR env var.
pub fn actor() -> String {
    std::env::var("POLIS_ACTOR").unwrap_or_else(|_| "unknown".into())
}

/// A single event in the append-only log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Event {
    Create {
        ts: DateTime<Utc>,
        actor: String,
        bead: BeadSnapshot,
    },
    Update {
        ts: DateTime<Utc>,
        actor: String,
        id: String,
        fields: HashMap<String, serde_json::Value>,
    },
    Close {
        ts: DateTime<Utc>,
        actor: String,
        id: String,
        reason: String,
    },
    Reopen {
        ts: DateTime<Utc>,
        actor: String,
        id: String,
    },
    /// Full-state snapshot produced by compaction.
    Snapshot {
        ts: DateTime<Utc>,
        actor: String,
        bead: BeadSnapshot,
    },
}

/// Complete bead state, used in Create and Snapshot events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeadSnapshot {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: String,
    pub priority: u8,
    #[serde(rename = "type")]
    pub bead_type: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_deadline: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<DateTime<Utc>>,
}

impl Event {
    pub fn id(&self) -> &str {
        match self {
            Event::Create { bead, .. } | Event::Snapshot { bead, .. } => &bead.id,
            Event::Update { id, .. } | Event::Close { id, .. } | Event::Reopen { id, .. } => id,
        }
    }

    pub fn ts(&self) -> DateTime<Utc> {
        match self {
            Event::Create { ts, .. }
            | Event::Update { ts, .. }
            | Event::Close { ts, .. }
            | Event::Reopen { ts, .. }
            | Event::Snapshot { ts, .. } => *ts,
        }
    }
}
