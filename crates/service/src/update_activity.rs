//! Update feedback survives unrelated service events and coalesced watch notifications.
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateTarget {
    Application,
    Rules,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Downloading {
        received: u64,
        total: u64,
    },
    Verifying,
    Installing,
    Succeeded(String),
    Failed(String),
}

impl UpdatePhase {
    pub fn running(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Verifying | Self::Installing
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpdateActivity {
    pub generation: u64,
    pub phase: UpdatePhase,
    pub completed_at: Option<DateTime<Utc>>,
}

impl UpdateActivity {
    pub fn start(&mut self, phase: UpdatePhase) {
        self.generation += 1;
        self.phase = phase;
        self.completed_at = None;
    }

    pub fn finish(&mut self, result: Result<String, String>) {
        self.phase = match result {
            Ok(message) => UpdatePhase::Succeeded(message),
            Err(message) => UpdatePhase::Failed(message),
        };
        self.completed_at = Some(Utc::now());
    }
}
