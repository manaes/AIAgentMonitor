use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind { Claude, Codex }

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenCounts {
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub tokens_cache_read: u32,
    pub tokens_cache_create: u32,
}

impl TokenCounts {
    pub fn total(&self) -> u32 {
        self.tokens_in
            .saturating_add(self.tokens_out)
            .saturating_add(self.tokens_cache_create)
            .saturating_add(self.tokens_cache_read)
    }
    pub fn add(&mut self, other: &TokenCounts) {
        self.tokens_in = self.tokens_in.saturating_add(other.tokens_in);
        self.tokens_out = self.tokens_out.saturating_add(other.tokens_out);
        self.tokens_cache_read = self.tokens_cache_read.saturating_add(other.tokens_cache_read);
        self.tokens_cache_create = self.tokens_cache_create.saturating_add(other.tokens_cache_create);
    }
}

#[derive(Debug, Clone)]
pub struct TokenEvent {
    pub agent: AgentKind,
    pub project_path: PathBuf,
    pub session_id: String,
    pub model: String,
    pub ts: SystemTime,
    pub counts: TokenCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityStatus { Active, Idle, Dormant }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectActivity {
    pub path: PathBuf,
    pub name: String,
    pub model: String,
    pub rate_tok_per_sec: f32,
    pub last_event_at: SystemTime,
    pub status: ActivityStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub kind: AgentKind,
    pub rate_tok_per_sec: f32,
    pub tokens_5h: TokenCounts,
    pub quota_limit: Option<u32>,
    pub quota_reset_at: Option<SystemTime>,
    pub quota_used_pct: Option<f32>,   // 프록시가 헤더에서 읽은 실제 5h 사용률(%) — 있으면 권위값
    pub projects: Vec<ProjectActivity>,
    pub triggered_by: Option<String>,  // v1.1 자리, v1에는 항상 None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub emitted_at: SystemTime,
    pub agents: Vec<AgentState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_total_saturates() {
        let mut c = TokenCounts::default();
        c.tokens_in = u32::MAX;
        c.tokens_out = 10;
        c.tokens_cache_read = 5;
        assert_eq!(c.total(), u32::MAX);
    }

    #[test]
    fn token_counts_total_includes_cache_read() {
        let c = TokenCounts {
            tokens_in: 10,
            tokens_out: 20,
            tokens_cache_read: 30,
            tokens_cache_create: 40,
        };
        assert_eq!(c.total(), 100);
    }

    #[test]
    fn snapshot_round_trip_serde() {
        let s = Snapshot { emitted_at: SystemTime::now(), agents: vec![] };
        let json = serde_json::to_string(&s).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agents.len(), 0);
    }
}
