use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind { Claude, Codex, Antigravity }

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
    // 사람이 방금 뭘 물어봤는지 짧게 미리보기(2026-09-02, "무슨 작업 중인지"
    // 탐색용). 실제 사용량 이벤트(assistant usage)에는 None — 이 필드가
    // Some 인 이벤트는 counts 가 전부 0인 "미리보기 전용" 이벤트이고,
    // Aggregator::push() 가 5h/주간 합계·anchor·rate 계산에서 완전히 제외
    // 한다(별도 분기, types.rs 문서 참고). 데스크톱 화면에만 쓰고 BLE/LAN
    // 미러(MirrorProject)로는 내보내지 않는다 — 대화 내용이라 프라이버시
    // 상 별도 취급이 필요하다.
    pub prompt_preview: Option<String>,
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
    /// 같은 폴더에서 세션이 여러 개 동시에 돌 때 구분하는 키(2026-09-02).
    /// 화면엔 안 보이고 프론트엔드 each-키로만 쓴다.
    pub session_id: String,
    /// 최근 사용자 메시지 미리보기(짧게 잘림). 데스크톱 화면 전용 —
    /// BLE/LAN 미러(MirrorProject)로는 나가지 않는다(프라이버시).
    pub prompt_preview: String,
}

/// 사용량 조회 실패의 종류. **문장이 아니라 이 분류가 미러로 나간다.**
///
/// 문자열을 그대로 보내지 않는 이유가 둘 있다. (1) BLE 는 프레임을 MTU 로 잘라
/// 보내므로 에이전트마다 붙는 한국어 문장이 그대로 대역이 된다. (2) CYD 펌웨어는
/// 이 구조체를 전역 `Transport` 안에 **값으로** 들고 있어서 필드가 DRAM(.bss)에
/// 고정으로 잡힌다 — `firmware/cyd/lib/snapshot/snapshot.h` 에 에이전트 상한을
/// 8→4 로 줄인 이유가 `region 'dram0_0_seg' overflowed by 6192 bytes` 실측이라고
/// 적혀 있다. 게다가 240px 짜리 화면과 아이폰과 맥이 같은 길이의 문구를 쓸 이유도
/// 없다 — 코드만 보내고 문구는 각자 고른다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuotaErrorKind {
    /// 로그인이 풀렸거나 인증이 거부됐다.
    Auth,
    /// CLI 를 실행하지 못했다(미설치, 경로 문제).
    Launch,
    /// 실행은 됐는데 응답이 오지 않았다.
    Timeout,
    /// 그 외(출력 파싱 실패, 알 수 없는 서버 오류).
    Other,
}

impl QuotaErrorKind {
    /// 와이어에 실리는 1바이트 코드. **한 번 정한 값은 바꾸지 않는다** — 이미
    /// 배포된 iOS·CYD 가 이 숫자로 문구를 고른다. 새 종류는 뒤에 덧붙인다.
    pub fn code(self) -> u8 {
        match self {
            QuotaErrorKind::Auth => 1,
            QuotaErrorKind::Launch => 2,
            QuotaErrorKind::Timeout => 3,
            QuotaErrorKind::Other => 4,
        }
    }
}

/// 데스크톱에 띄울 문장과, 미러로 보낼 분류를 함께 들고 다닌다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QuotaError {
    pub kind: QuotaErrorKind,
    pub message: String,
}

impl QuotaError {
    pub fn auth(message: impl Into<String>) -> Self {
        Self { kind: QuotaErrorKind::Auth, message: message.into() }
    }
    pub fn launch(message: impl Into<String>) -> Self {
        Self { kind: QuotaErrorKind::Launch, message: message.into() }
    }
    pub fn timeout(message: impl Into<String>) -> Self {
        Self { kind: QuotaErrorKind::Timeout, message: message.into() }
    }
    pub fn other(message: impl Into<String>) -> Self {
        Self { kind: QuotaErrorKind::Other, message: message.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub kind: AgentKind,
    pub rate_tok_per_sec: f32,
    pub tokens_5h: TokenCounts,
    pub quota_limit: Option<u32>,
    pub quota_reset_at: Option<SystemTime>,
    pub quota_used_pct: Option<f32>,          // 실제 5h 사용률(%) — Claude:프록시 / Codex:rollout
    pub quota_reset_at_weekly: Option<SystemTime>,
    pub quota_used_pct_weekly: Option<f32>,   // 주간(7d) 사용률(%)
    /// 사용량을 못 읽고 있는 이유(로그인 안 됨, CLI 없음, 타임아웃 등).
    /// 값이 있으면 카드의 프로젝트 줄 아래에 문장을 띄우고, 한도 %·리셋
    /// 카운트다운은 숨긴다 — 0% 를 조용히 보여주면 "안 쓰는 중"과 "못 읽는
    /// 중"이 구분되지 않고, 마지막으로 받아둔 낡은 %는 지금 상태를 말해주지
    /// 못한다(2026-09-04).
    ///
    /// 미러(BLE/LAN/네트워크)로는 `message` 가 아니라 `kind.code()` 1바이트만
    /// 나간다(`MirrorAgent::e`) — 이유는 QuotaErrorKind 문서 참고.
    pub quota_error: Option<QuotaError>,
    pub projects: Vec<ProjectActivity>,
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
