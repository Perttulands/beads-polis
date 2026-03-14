//! Bead data model — computed from event replay.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::event::{BeadSnapshot, Event};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Open,
    InProgress,
    Closed,
    Deferred,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Closed => "closed",
            Self::Deferred => "deferred",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "open" => Self::Open,
            "in_progress" => Self::InProgress,
            "closed" => Self::Closed,
            "deferred" => Self::Deferred,
            _ => Self::Open,
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BeadType {
    Epic,
    Feature,
    Bug,
    #[default]
    Task,
    Chore,
}

impl BeadType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Epic => "epic",
            Self::Feature => "feature",
            Self::Bug => "bug",
            Self::Task => "task",
            Self::Chore => "chore",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "epic" => Self::Epic,
            "feature" => Self::Feature,
            "bug" => Self::Bug,
            "task" => Self::Task,
            "chore" => Self::Chore,
            _ => Self::Task,
        }
    }
}

impl fmt::Display for BeadType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The core work item, computed by replaying events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bead {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: Status,
    pub priority: u8,
    pub bead_type: BeadType,
    pub project: String,
    pub assignee: Option<String>,
    pub parent: Option<String>,
    pub dependencies: Vec<String>,
    pub labels: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub close_reason: Option<String>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub claim_deadline: Option<DateTime<Utc>>,
    pub last_heartbeat: Option<DateTime<Utc>>,
}

impl Bead {
    /// Create a bead from a snapshot (Create or Snapshot event payload).
    pub fn from_snapshot(s: &BeadSnapshot) -> Self {
        Self {
            id: s.id.clone(),
            title: s.title.clone(),
            description: s.description.clone(),
            status: Status::from_str_lossy(&s.status),
            priority: s.priority.min(4),
            bead_type: BeadType::from_str_lossy(&s.bead_type),
            project: s.project.clone(),
            assignee: s.assignee.clone(),
            parent: s.parent.clone(),
            dependencies: s.dependencies.clone(),
            labels: s.labels.clone(),
            created_at: s.created_at,
            updated_at: s.updated_at,
            closed_at: s.closed_at,
            close_reason: s.close_reason.clone(),
            claimed_at: s.claimed_at,
            claim_deadline: s.claim_deadline,
            last_heartbeat: s.last_heartbeat,
        }
    }

    /// Apply a single event to update this bead's state.
    pub fn apply_event(&mut self, event: &Event) {
        match event {
            Event::Create { bead, .. } | Event::Snapshot { bead, .. } => {
                *self = Self::from_snapshot(bead);
            }
            Event::Update { fields, ts, .. } => {
                self.updated_at = *ts;
                for (key, val) in fields {
                    self.apply_field(key, val);
                }
            }
            Event::Close { reason, ts, .. } => {
                self.status = Status::Closed;
                self.close_reason = Some(reason.clone());
                self.closed_at = Some(*ts);
                self.updated_at = *ts;
            }
            Event::Reopen { ts, .. } => {
                self.status = Status::Open;
                self.closed_at = None;
                self.close_reason = None;
                self.updated_at = *ts;
            }
        }
    }

    fn apply_field(&mut self, key: &str, val: &serde_json::Value) {
        match key {
            "title" => {
                if let Some(s) = val.as_str() {
                    self.title = s.to_string();
                }
            }
            "description" => {
                self.description = val.as_str().map(String::from);
            }
            "status" => {
                if let Some(s) = val.as_str() {
                    self.status = Status::from_str_lossy(s);
                }
            }
            "priority" => {
                if let Some(n) = val.as_u64() {
                    self.priority = (n as u8).min(4);
                }
            }
            "type" => {
                if let Some(s) = val.as_str() {
                    self.bead_type = BeadType::from_str_lossy(s);
                }
            }
            "project" => {
                if let Some(s) = val.as_str() {
                    self.project = s.to_string();
                }
            }
            "assignee" => {
                self.assignee = val.as_str().map(String::from);
            }
            "parent" => {
                self.parent = val.as_str().map(String::from);
            }
            "dependencies" => {
                if let Some(arr) = val.as_array() {
                    self.dependencies = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                }
            }
            "labels" => {
                if let Some(arr) = val.as_array() {
                    self.labels = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                }
            }
            "claimed_at" => {
                self.claimed_at = val
                    .as_str()
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());
            }
            "claim_deadline" => {
                self.claim_deadline = val
                    .as_str()
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());
            }
            "last_heartbeat" => {
                self.last_heartbeat = val
                    .as_str()
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());
            }
            _ => {} // ignore unknown fields for forward compatibility
        }
    }

    /// Replay a sequence of events to compute the final bead state.
    /// All events must belong to the same bead ID.
    /// Returns None if events is empty or starts with a non-Create event.
    pub fn from_events(events: &[Event]) -> Option<Self> {
        let first = events.first()?;
        let mut bead = match first {
            Event::Create { bead: snap, .. } | Event::Snapshot { bead: snap, .. } => {
                Self::from_snapshot(snap)
            }
            _ => return None,
        };
        for event in &events[1..] {
            bead.apply_event(event);
        }
        Some(bead)
    }
}
