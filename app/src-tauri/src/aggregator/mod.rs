pub mod ring;
pub mod rotating;

use crate::clock::Clock;
use crate::types::{
    ActivityStatus, AgentKind, AgentState, ProjectActivity, Snapshot, TokenEvent,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ring::EventRing;
use rotating::RotatingBucket;

const QUOTA_WINDOW: Duration = Duration::from_secs(5 * 3600);
// 윈도우 anchor 재구성을 위해 보관하는 이벤트 타임스탬프 최대 기간
const ANCHOR_LOOKBACK: Duration = Duration::from_secs(24 * 3600);
// 2026-09-03: session_id 로 키를 바꾼 뒤(위 AgentBucket.projects 주석) 세션 맵이
// 영원히 자라기만 했다 — 같은 폴더를 새 세션으로 열 때마다 예전엔 경로 하나로
// 재사용되던 자리가 이제는 별도 항목으로 계속 쌓인다. 세션 맵이 무한정 커지지
// 않게 정리(prune)해야 한다(스냅샷 = BLE/LAN 미러 페이로드에 그대로 실린다).
//
// 처음엔 6시간을 줬지만(2026-09-03 커밋 f166aab), 사용자 피드백: "휴면(회색)
// 상태는 볼 필요 없다, 유휴(주황)까지만 있으면 된다" — 즉 목록에 남아 있는
// 이유가 애초에 "아직 활동 중/막 쉬는 중"인 세션을 보여주기 위해서지, 몇 시간
// 지난 세션까지 붙잡아 둘 이유가 없다. 그래서 idle→dormant 경계(아래 status
// 계산의 300초)와 정확히 맞춰, Dormant 로 넘어가는 순간 목록에서 완전히 지운다.
const PROJECT_PRUNE_AFTER: Duration = Duration::from_secs(300);

#[derive(Default)]
pub struct Aggregator {
    by_agent: HashMap<AgentKind, AgentBucket>,
}

struct AgentBucket {
    ring: EventRing,
    rotating: RotatingBucket,
    // 2026-09-02: 프로젝트 경로가 아니라 session_id 로 키를 바꿨다. 같은
    // 폴더에서 세션 두 개가 동시에 돌면(예: 같은 리포에서 Claude Code 두
    // 창) 예전엔 경로 하나로 묶여 나중 이벤트가 이전 세션을 덮어썼다 —
    // 실기로 재현 확인(2개 작업 중인데 1개만 보임). 세션 단위로 바꾸면
    // 같은 폴더라도 각자 따로 잡힌다.
    projects: HashMap<String, ProjectState>,
    /// 윈도우 anchor 계산용 이벤트 타임스탬프 (정렬+갭세그먼트로 현재 5h 윈도우 시작점 산출)
    event_times: Vec<SystemTime>,
}

struct ProjectState {
    project_path: PathBuf,
    model: String,
    last_event_at: SystemTime,
    rate_ring: EventRing,
    last_prompt_preview: String,
}

impl Default for AgentBucket {
    fn default() -> Self {
        Self { ring: EventRing::new(), rotating: RotatingBucket::new(), projects: HashMap::new(), event_times: Vec::new() }
    }
}

impl Aggregator {
    pub fn new() -> Self { Self::default() }

    /// 모든 에이전트 통틀어 가장 최근 이벤트 시각 (주기적 자동 동기화 활동 판단용)
    pub fn last_event_at(&self) -> Option<SystemTime> {
        self.by_agent
            .values()
            .filter_map(|b| b.event_times.iter().max().copied())
            .max()
    }

    pub fn push(&mut self, ev: TokenEvent) {
        let bucket = self.by_agent.entry(ev.agent).or_default();

        // 프롬프트 미리보기 전용 이벤트(types.rs 문서 참고) — 실제 사용량이
        // 아니므로(counts 는 항상 0) 5h/주간 합계·anchor(event_times)·rate
        // 계산에 절대 관여시키지 않는다. 순수 메타데이터 갱신으로 끝낸다.
        if let Some(preview) = ev.prompt_preview.clone() {
            let proj = bucket.projects.entry(ev.session_id.clone()).or_insert_with(|| ProjectState {
                project_path: ev.project_path.clone(),
                model: "claude-3.7-sonnet".into(),
                last_event_at: ev.ts,
                rate_ring: EventRing::new(),
                last_prompt_preview: String::new(),
            });
            proj.last_prompt_preview = preview;
            return;
        }

        bucket.rotating.add(ev.ts, &ev.counts);
        bucket.event_times.push(ev.ts);
        let is_valid_model = !ev.model.is_empty() && !ev.model.starts_with('<');
        let proj = bucket.projects.entry(ev.session_id.clone()).or_insert_with(|| ProjectState {
            project_path: ev.project_path.clone(),
            model: if is_valid_model { ev.model.clone() } else { "claude-3.7-sonnet".into() },
            last_event_at: ev.ts,
            rate_ring: EventRing::new(),
            last_prompt_preview: String::new(),
        });
        if is_valid_model {
            proj.model = ev.model.clone();
        }
        proj.last_event_at = ev.ts;
        proj.rate_ring.push(ev.clone());
        bucket.ring.push(ev);
    }

    pub fn snapshot<C: Clock>(&mut self, clock: &C) -> Snapshot {
        let now = clock.now();
        let mut agents = Vec::with_capacity(3);
        for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity] {
            let bucket = self.by_agent.entry(kind).or_default();
            bucket
                .projects
                .retain(|_, ps| now.duration_since(ps.last_event_at).unwrap_or_default() < PROJECT_PRUNE_AFTER);
            let rate = bucket.ring.rate_tok_per_sec(clock);
            let tokens_5h = bucket.rotating.sum_5h(clock);
            // Anthropic 5h 롤링 윈도우 시작점(anchor) + 5h. trailing cutoff가 아니라 실제 윈도우
            // 첫 메시지를 anchor로 잡아 연속사용(>5h)/유휴갭/연속윈도우를 올바르게 처리한다.
            // 첫-메시지 앵커+5h 추정은 Claude 폴백용. Codex는 실제 rate_limits를 lib.rs 틱이
            // 주입하므로 여기선 None으로 둬 가짜 카운트다운을 막는다. (event_times prune은 양쪽 모두)
            let anchor = current_window_anchor(&mut bucket.event_times, now);
            let quota_reset_at = if kind == AgentKind::Claude {
                anchor.map(|a| a + QUOTA_WINDOW)
            } else {
                None
            };

            let mut projects: Vec<ProjectActivity> = bucket.projects.iter_mut().map(|(session_id, ps)| {
                let elapsed = now.duration_since(ps.last_event_at).unwrap_or_default();
                let status = if elapsed <= Duration::from_secs(60) { ActivityStatus::Active }
                    else if elapsed <= Duration::from_secs(300) { ActivityStatus::Idle }
                    else { ActivityStatus::Dormant };
                let name = ps.project_path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
                ProjectActivity {
                    path: ps.project_path.clone(),
                    name,
                    model: ps.model.clone(),
                    rate_tok_per_sec: ps.rate_ring.rate_tok_per_sec(clock),
                    last_event_at: ps.last_event_at,
                    status,
                    session_id: session_id.clone(),
                    prompt_preview: ps.last_prompt_preview.clone(),
                }
            }).collect();
            projects.sort_by_key(|p| std::cmp::Reverse(p.last_event_at));

            agents.push(AgentState {
                kind,
                rate_tok_per_sec: rate,
                tokens_5h,
                quota_limit: None,
                quota_reset_at,
                quota_used_pct: None,
                quota_reset_at_weekly: None,
                quota_used_pct_weekly: None,
                quota_error: None,
                projects,
            });
        }
        Snapshot { emitted_at: now, agents }
    }
}

/// 현재 사용량 윈도우의 시작 시각(anchor)을 구한다.
/// Anthropic의 5h 롤링 윈도우는 "마지막 >=5h 유휴 이후 첫 메시지"에서 열려 5h 뒤 리셋된다.
/// 보관된 타임스탬프를 정렬해 5h 갭으로 세그먼트하고 가장 최근 윈도우의 anchor를 반환한다.
fn current_window_anchor(times: &mut Vec<SystemTime>, now: SystemTime) -> Option<SystemTime> {
    let cutoff = now.checked_sub(ANCHOR_LOOKBACK).unwrap_or(SystemTime::UNIX_EPOCH);
    times.retain(|&t| t >= cutoff && t <= now);
    if times.is_empty() {
        return None;
    }
    times.sort();
    let mut anchor = times[0];
    for &t in times.iter().skip(1) {
        if t.duration_since(anchor).unwrap_or_default() >= QUOTA_WINDOW {
            anchor = t;
        }
    }
    Some(anchor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::mock::MockClock;
    use crate::types::{AgentKind, TokenCounts, TokenEvent};
    use std::path::PathBuf;
    use std::time::Duration;

    fn ev(agent: AgentKind, ts: std::time::SystemTime, proj: &str, model: &str, total_in: u32) -> TokenEvent {
        ev_with_session(agent, ts, proj, "s1", model, total_in)
    }

    fn ev_with_session(
        agent: AgentKind, ts: std::time::SystemTime, proj: &str, session_id: &str, model: &str, total_in: u32,
    ) -> TokenEvent {
        TokenEvent {
            agent, ts,
            project_path: PathBuf::from(proj),
            session_id: session_id.into(),
            model: model.into(),
            counts: TokenCounts { tokens_in: total_in, ..Default::default() },
            prompt_preview: None,
        }
    }

    #[test]
    fn two_sessions_in_the_same_project_folder_both_appear() {
        // 실기 재현(2026-09-02): 같은 폴더에서 세션 두 개가 동시에 돌 때
        // 예전엔 project_path 로 묶여서 나중 이벤트가 이전 세션을 지웠다.
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev_with_session(AgentKind::Claude, clock.now(), "/tmp/p1", "session-a", "x", 100));
        agg.push(ev_with_session(AgentKind::Claude, clock.now(), "/tmp/p1", "session-b", "x", 200));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(claude.projects.len(), 2, "두 세션이 각자 다른 행으로 보여야 한다");
        let ids: Vec<&str> = claude.projects.iter().map(|p| p.session_id.as_str()).collect();
        assert!(ids.contains(&"session-a"));
        assert!(ids.contains(&"session-b"));
    }

    #[test]
    fn empty_snapshot_has_three_agents() {
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        let snap = agg.snapshot(&clock);
        assert_eq!(snap.agents.len(), 3);
    }

    #[test]
    fn active_project_appears_with_correct_status() {
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/p1", "claude-sonnet-4-6", 500));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(claude.projects.len(), 1);
        assert_eq!(claude.projects[0].status, ActivityStatus::Active);
        assert_eq!(claude.projects[0].name, "p1");
    }

    #[test]
    fn project_becomes_idle_after_60s() {
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/p1", "x", 100));
        clock.advance(Duration::from_secs(61));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(claude.projects[0].status, ActivityStatus::Idle);
    }

    #[test]
    fn dormant_session_is_pruned_instead_of_shown() {
        // 2026-09-03 사용자 피드백: 휴면(회색) 상태는 볼 필요 없다 — idle→dormant
        // 경계를 넘는 순간 목록에서 완전히 지운다(Dormant 는 더 이상 관측되지 않는다).
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/p1", "x", 100));
        clock.advance(Duration::from_secs(301));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(claude.projects.len(), 0, "휴면으로 넘어간 세션은 지워져야 한다");
    }

    #[test]
    fn session_just_under_the_idle_window_is_still_kept() {
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/old", "x", 100));
        clock.advance(Duration::from_secs(300) - Duration::from_secs(1));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(claude.projects.len(), 1, "경계 직전이면 아직 남아있어야 한다");
        assert_eq!(claude.projects[0].status, ActivityStatus::Idle);
    }

    #[test]
    fn quota_reset_at_estimated_from_oldest_bucket() {
        let clock = MockClock::new(1_000_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/p1", "x", 100));
        clock.advance(Duration::from_secs(60));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        let reset = claude.quota_reset_at.expect("reset must be Some after first event");
        let now = clock.now();
        let diff = reset.duration_since(now).unwrap_or_default();
        assert!(diff <= Duration::from_secs(5 * 3600), "reset should be within 5h, got {:?}", diff);
    }

    #[test]
    fn quota_reset_anchors_to_current_window_after_5h() {
        let clock = MockClock::new(1_000_000_000);
        let mut agg = Aggregator::new();
        let t0 = clock.now();
        agg.push(ev(AgentKind::Claude, t0, "/tmp/p", "x", 100));
        // 5h+1min 후 다시 사용 → 새 윈도우가 열려야 한다
        clock.advance(Duration::from_secs(5 * 3600 + 60));
        let t1 = clock.now();
        agg.push(ev(AgentKind::Claude, t1, "/tmp/p", "x", 100));
        clock.advance(Duration::from_secs(600));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        let reset = claude.quota_reset_at.expect("reset present");
        // t0+5h(옛 윈도우)이 아니라 t1+5h(새 윈도우)이어야 한다
        assert_eq!(reset, t1 + Duration::from_secs(5 * 3600));
    }
}
