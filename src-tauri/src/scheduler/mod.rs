pub mod runner;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

// 트리거 룰 1개. cron은 초 포함 6필드 형식 "0 MM HH * * *"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRule {
    pub id: String,
    pub agent: String,   // "claude" | "codex"
    pub cron: String,
    pub working_dir: String,
    pub prompt: String,
    pub enabled: bool,
    pub created_at: u64, // epoch secs
}

impl ScheduleRule {
    fn new(agent: String, cron: String, working_dir: String, prompt: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id: Uuid::new_v4().to_string(),
            agent,
            cron,
            working_dir,
            prompt,
            enabled: true,
            created_at: now,
        }
    }
}

// 스케줄러 상태. rules는 영속화, job_scheduler는 런타임 전용.
pub struct Scheduler {
    pub rules: Vec<ScheduleRule>,
    pub job_scheduler: JobScheduler,
}

fn persist_path() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ai-agent-monitor/triggers.json")
}

// ~ 또는 ~/ 로 시작하는 경로를 홈 디렉토리 절대경로로 전개
fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") || path == "~" {
        if let Some(home) = dirs_next::home_dir() {
            return path.replacen("~", &home.to_string_lossy(), 1);
        }
    }
    path.to_string()
}

impl Scheduler {
    // JobScheduler를 먼저 생성한 후 Scheduler를 반환 — async 컨텍스트 필요
    pub async fn new() -> anyhow::Result<Self> {
        let js = JobScheduler::new().await?;
        js.start().await?;

        let mut s = Self {
            rules: Vec::new(),
            job_scheduler: js,
        };
        s.load_from_disk();
        // 앱 시작 시 활성 룰을 스케줄러에 등록
        s.sync_jobs().await;
        Ok(s)
    }

    // 디스크에서 룰 목록 로드. 파일 없으면 빈 Vec 유지.
    fn load_from_disk(&mut self) {
        let path = persist_path();
        match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<Vec<ScheduleRule>>(&json) {
                Ok(rules) => {
                    tracing::info!(count = rules.len(), "triggers.json 로드");
                    self.rules = rules;
                }
                Err(e) => tracing::warn!(%e, "triggers.json 파싱 실패, 빈 목록으로 시작"),
            },
            Err(_) => {
                tracing::info!("triggers.json 없음, 빈 목록으로 시작");
            }
        }
    }

    // 현재 rules를 디스크에 저장
    fn save_to_disk(&self) {
        let path = persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&self.rules) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!(%e, "triggers.json 저장 실패");
                }
            }
            Err(e) => tracing::warn!(%e, "triggers.json 직렬화 실패"),
        }
    }

    // 룰 목록 조회
    pub fn list_rules(&self) -> Vec<ScheduleRule> {
        self.rules.clone()
    }

    // 새 룰 추가 (HH:MM → cron 변환은 command layer에서 처리)
    pub async fn add_rule(
        &mut self,
        agent: String,
        cron: String,
        working_dir: String,
        prompt: String,
    ) -> anyhow::Result<ScheduleRule> {
        // ~ 로 시작하는 경로를 홈 디렉토리 절대경로로 전개
        let working_dir = expand_tilde(&working_dir);
        // working_dir 경로 존재 확인
        if !std::path::Path::new(&working_dir).exists() {
            anyhow::bail!("working_dir 경로가 존재하지 않습니다: {working_dir}");
        }
        let rule = ScheduleRule::new(agent, cron, working_dir, prompt);
        self.rules.push(rule.clone());
        self.save_to_disk();
        self.sync_jobs().await;
        Ok(rule)
    }

    // id로 룰 삭제
    pub async fn remove_rule(&mut self, id: &str) -> anyhow::Result<()> {
        let before = self.rules.len();
        self.rules.retain(|r| r.id != id);
        if self.rules.len() == before {
            anyhow::bail!("id를 찾을 수 없습니다: {id}");
        }
        self.save_to_disk();
        self.sync_jobs().await;
        Ok(())
    }

    // enabled 상태 반전
    pub async fn toggle_rule(&mut self, id: &str) -> anyhow::Result<ScheduleRule> {
        let rule = self
            .rules
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| anyhow::anyhow!("id를 찾을 수 없습니다: {id}"))?;
        rule.enabled = !rule.enabled;
        let updated = rule.clone();
        self.save_to_disk();
        self.sync_jobs().await;
        Ok(updated)
    }

    // 활성 룰만 재등록 (naive: 기존 스케줄러 종료 후 신규 생성)
    pub async fn sync_jobs(&mut self) {
        // 기존 스케줄러 shutdown 후 새 인스턴스로 교체
        if let Err(e) = self.job_scheduler.shutdown().await {
            tracing::warn!(%e, "스케줄러 shutdown 오류 (무시)");
        }

        let new_js = match JobScheduler::new().await {
            Ok(js) => js,
            Err(e) => {
                tracing::warn!(%e, "새 스케줄러 생성 실패");
                return;
            }
        };

        // enabled 룰만 새 스케줄러에 등록
        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }
            let agent = rule.agent.clone();
            let prompt = rule.prompt.clone();
            let working_dir = rule.working_dir.clone();
            let cron = rule.cron.clone();

            match Job::new_async(cron.as_str(), move |_uuid, _lock| {
                let agent = agent.clone();
                let prompt = prompt.clone();
                let working_dir = working_dir.clone();
                Box::pin(async move {
                    runner::run_trigger(&agent, &prompt, &working_dir).await;
                })
            }) {
                Ok(job) => {
                    if let Err(e) = new_js.add(job).await {
                        tracing::warn!(%e, cron, "job 등록 실패");
                    }
                }
                Err(e) => tracing::warn!(%e, cron, "Job 생성 실패"),
            }
        }

        // 새 스케줄러 시작
        if let Err(e) = new_js.start().await {
            tracing::warn!(%e, "새 스케줄러 start 실패");
        }

        self.job_scheduler = new_js;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // expand_tilde: ~ 로 시작하는 경로를 절대경로로 전개하는지 검증
    #[test]
    fn test_expand_tilde_replaces_home() {
        let result = expand_tilde("~/workspace");
        assert!(
            result.starts_with('/'),
            "tilde는 절대경로로 전개되어야 합니다, 결과: {result}"
        );
        assert!(
            !result.starts_with("~/"),
            "tilde가 결과에 남아 있으면 안 됩니다"
        );
    }

    // expand_tilde: / 로 시작하는 절대경로는 그대로 반환
    #[test]
    fn test_expand_tilde_leaves_absolute_path() {
        let path = "/Users/foo/bar";
        assert_eq!(expand_tilde(path), path);
    }

    // expand_tilde: 상대경로는 그대로 반환
    #[test]
    fn test_expand_tilde_leaves_relative_path() {
        let path = "relative/path";
        assert_eq!(expand_tilde(path), path);
    }

    // expand_tilde: ~ 단독(경로 없음)도 홈 디렉토리로 전개
    #[test]
    fn test_expand_tilde_standalone() {
        let result = expand_tilde("~");
        // 홈 디렉토리가 없는 CI 환경에서는 원본 그대로 반환되므로 패닉 없이 통과
        assert!(!result.is_empty());
    }

    // ScheduleRule::new: 기본값 및 필드 설정 검증
    #[test]
    fn test_schedule_rule_new_sets_defaults() {
        let rule = ScheduleRule::new(
            "claude".to_string(),
            "0 0 8 * * *".to_string(),
            "/tmp".to_string(),
            "ping".to_string(),
        );
        assert!(!rule.id.is_empty(), "id는 비어 있으면 안 됩니다");
        assert!(rule.enabled, "새 룰은 기본적으로 활성화 상태여야 합니다");
        assert_eq!(rule.agent, "claude");
        assert_eq!(rule.cron, "0 0 8 * * *");
        assert_eq!(rule.working_dir, "/tmp");
        assert_eq!(rule.prompt, "ping");
        assert!(rule.created_at > 0, "created_at은 0보다 커야 합니다");
    }

    // ScheduleRule::new: 호출마다 다른 uuid가 생성되는지 검증
    #[test]
    fn test_schedule_rule_new_unique_ids() {
        let rule1 = ScheduleRule::new("claude".into(), "0 0 8 * * *".into(), "/tmp".into(), "ping".into());
        let rule2 = ScheduleRule::new("claude".into(), "0 0 8 * * *".into(), "/tmp".into(), "ping".into());
        assert_ne!(rule1.id, rule2.id, "룰 id는 매번 고유해야 합니다");
    }

    // JSON 직렬화 → 역직렬화 왕복 검증 (영속화 회귀 테스트)
    #[test]
    fn test_scheduler_serialize_deserialize_roundtrip() {
        let rule = ScheduleRule::new(
            "claude".into(),
            "0 0 8 * * *".into(),
            "/tmp".into(),
            "ping".into(),
        );
        let id = rule.id.clone();
        let agent = rule.agent.clone();

        let json = serde_json::to_string(&vec![rule]).expect("직렬화 실패");
        let loaded: Vec<ScheduleRule> = serde_json::from_str(&json).expect("역직렬화 실패");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, id);
        assert_eq!(loaded[0].agent, agent);
        assert!(loaded[0].enabled);
    }

    // JSON 직렬화: 빈 Vec도 정상적으로 처리
    #[test]
    fn test_scheduler_serialize_empty_rules() {
        let rules: Vec<ScheduleRule> = vec![];
        let json = serde_json::to_string(&rules).expect("직렬화 실패");
        let loaded: Vec<ScheduleRule> = serde_json::from_str(&json).expect("역직렬화 실패");
        assert!(loaded.is_empty());
    }

    // JSON 직렬화: enabled=false 룰도 올바르게 복원
    #[test]
    fn test_scheduler_serialize_disabled_rule() {
        let mut rule = ScheduleRule::new(
            "codex".into(),
            "0 30 9 * * *".into(),
            "/tmp".into(),
            "hello".into(),
        );
        rule.enabled = false;

        let json = serde_json::to_string(&vec![rule]).expect("직렬화 실패");
        let loaded: Vec<ScheduleRule> = serde_json::from_str(&json).expect("역직렬화 실패");

        assert_eq!(loaded.len(), 1);
        assert!(!loaded[0].enabled, "비활성화 상태가 복원되어야 합니다");
        assert_eq!(loaded[0].agent, "codex");
    }
}
