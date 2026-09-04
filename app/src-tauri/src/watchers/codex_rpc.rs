//! Codex quota 능동 조회 — `codex app-server` 의 JSON-RPC `account/rateLimits/read`.
//!
//! rollout tail(codex.rs)은 **실제 턴이 있을 때만** `token_count.rate_limits` 를 준다.
//! 사용자가 손을 놓고 있으면 카드의 %가 계속 늙는다. 예전엔 이걸
//! `codex exec "Reply exactly with the word ok"` 로 억지 턴을 만들어 갱신했는데,
//! 그건 **진짜 토큰을 태워서** quota 를 재는 자기모순이었다(2026-09-04 교체).
//!
//! app-server 의 `account/rateLimits/read` 는 계정 한도 조회 전용 RPC라 토큰을 쓰지
//! 않고 rollout 과 **같은 스냅샷**(primary=5h, secondary=주간)을 돌려준다. Claude 의
//! `/usage` 안전망과 같은 자리지만, 저쪽은 stdout 텍스트 파싱이고 이쪽은 타입이 있는
//! JSON 이라 CLI 출력 포맷이 바뀌어도 조용히 깨지지 않는다.
//!
//! 프로토콜: 한 줄에 JSON-RPC 하나(줄 구분). `initialize` → `initialized` 알림 →
//! 실제 요청 순서를 지켜야 하고, 응답 사이사이에 서버 알림(`remoteControl/status/changed`
//! 등)이 섞여 나오므로 **id 로 골라 읽어야** 한다.

use crate::types::QuotaError;
use crate::watchers::codex::{apply_rate_limits, CodexQuota, RateLimits, Window};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

/// `initialize` 와 실제 요청에 쓰는 id. 응답이 알림과 섞여 오므로 id 로 구분한다.
const INIT_ID: u64 = 1;
const RATE_LIMITS_ID: u64 = 2;

/// app-server 기동 + 왕복까지 넉넉히. 조회 전용이라 오래 걸릴 이유가 없고,
/// 늘어지면 다음 주기에 다시 시도하는 편이 낫다.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// 조회 결과를 quota 슬롯에 반영한다. 실패하면 사람이 읽을 수 있는 이유를
/// `last_error` 에 남겨 카드에 그대로 띄운다(성공 시엔 apply_rate_limits 가 지운다).
pub(crate) async fn poll_rate_limits(bin: &str, cwd: &Path, quota: &CodexQuota) {
    match read_rate_limits(bin, cwd).await {
        Ok(rl) => {
            apply_rate_limits(&rl, quota);
            // 방금 조회가 실제로 성공했다 — 이제서야 이전 실패 문구를 지울 자격이 있다.
            *quota.last_error.lock().unwrap() = None;
            tracing::info!(
                pct_5h = ?quota.used_pct_5h.lock().unwrap(),
                pct_weekly = ?quota.used_pct_weekly.lock().unwrap(),
                "codex quota 동기화(account/rateLimits/read) 완료"
            );
        }
        Err(e) => {
            tracing::warn!(message = %e.message, "codex quota 동기화 실패");
            *quota.last_error.lock().unwrap() = Some(e);
        }
    }
}

/// `codex app-server` 를 띄워 한도 스냅샷 한 번만 받아온다.
/// Err 는 그대로 사용자에게 보여줄 한국어 문장이다.
pub(crate) async fn read_rate_limits(bin: &str, cwd: &Path) -> Result<RateLimits, QuotaError> {
    let mut cmd = Command::new(bin);
    cmd.args(["app-server", "--listen", "stdio://"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = crate::hide_console_window(&mut cmd)
        .spawn()
        .map_err(|e| QuotaError::launch(format!("codex 실행 실패 — 설치돼 있나요? ({e})")))?;

    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill().await;
        return Err(QuotaError::launch("codex app-server 파이프를 열지 못했습니다"));
    };

    let mut lines = BufReader::new(stdout).lines();
    let outcome = tokio::time::timeout(RPC_TIMEOUT, converse(&mut stdin, &mut lines)).await;

    // 조회용으로 잠깐 띄운 프로세스라 끝나면 반드시 정리한다 — 안 그러면
    // 10분마다 app-server 가 하나씩 쌓인다.
    let _ = child.kill().await;

    match outcome {
        Ok(result) => result,
        Err(_) => Err(QuotaError::timeout("codex app-server 응답 시간 초과")),
    }
}

async fn converse(
    stdin: &mut ChildStdin,
    lines: &mut tokio::io::Lines<BufReader<ChildStdout>>,
) -> Result<RateLimits, QuotaError> {
    for msg in [init_request(), initialized_notification(), rate_limits_request()] {
        stdin
            .write_all(msg.as_bytes())
            .await
            .map_err(|e| QuotaError::other(format!("codex app-server 쓰기 실패: {e}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| QuotaError::other(format!("codex app-server 쓰기 실패: {e}")))?;
    }
    stdin
        .flush()
        .await
        .map_err(|e| QuotaError::other(format!("codex app-server 쓰기 실패: {e}")))?;

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| QuotaError::other(format!("codex app-server 읽기 실패: {e}")))?
    {
        if let Some(result) = parse_response(&line, RATE_LIMITS_ID) {
            return result;
        }
    }
    Err(QuotaError::other("codex app-server 가 응답 없이 종료됐습니다"))
}

fn init_request() -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": INIT_ID,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "AIAgentMonitor",
                "title": "AI Agent Monitor",
                "version": env!("CARGO_PKG_VERSION"),
            }
        }
    })
    .to_string()
}

fn initialized_notification() -> String {
    serde_json::json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }).to_string()
}

fn rate_limits_request() -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": RATE_LIMITS_ID,
        "method": "account/rateLimits/read",
        "params": {}
    })
    .to_string()
}

/// 한 줄을 보고 **우리 id 의 응답이면** 결과를, 아니면(알림·다른 응답·파싱 불가)
/// None 을 준다. None 이면 호출자는 계속 읽는다.
fn parse_response(line: &str, id: u64) -> Option<Result<RateLimits, QuotaError>> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
        return None;
    }
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("알 수 없는 오류");
        return Some(Err(friendly_error(msg)));
    }
    match v.get("result") {
        Some(result) => Some(parse_rate_limits(result)),
        None => Some(Err(QuotaError::other("codex app-server 응답에 result 가 없습니다"))),
    }
}

/// `GetAccountRateLimitsResponse` → rollout 과 같은 `RateLimits` 로.
///
/// `rateLimits` 가 하위호환 단일 버킷 뷰이고, 비어 있으면 멀티버킷
/// (`rateLimitsByLimitId`) 의 `codex` 버킷을 본다.
fn parse_rate_limits(result: &serde_json::Value) -> Result<RateLimits, QuotaError> {
    let snapshot = result
        .get("rateLimits")
        .filter(|v| has_window(v))
        .or_else(|| {
            result
                .get("rateLimitsByLimitId")
                .and_then(|m| m.get("codex"))
                .filter(|v| has_window(v))
        });

    let Some(snapshot) = snapshot else {
        // 로그인은 됐는데 한도 정보가 없는 경우(예: API 키 사용 — 플랜 한도가 아예 없음).
        return Err(QuotaError::other(
            "Codex 계정에 보고된 사용 한도가 없습니다 (ChatGPT 플랜 로그인인가요?)",
        ));
    };

    Ok(RateLimits {
        primary: window_from(snapshot.get("primary")),
        secondary: window_from(snapshot.get("secondary")),
    })
}

fn has_window(v: &serde_json::Value) -> bool {
    [v.get("primary"), v.get("secondary")]
        .into_iter()
        .flatten()
        .any(|w| w.is_object())
}

/// `RateLimitWindow`(camelCase) → rollout 쪽 `Window`(snake_case).
/// `usedPercent` 는 스키마상 정수지만 rollout 은 실수로 주므로 f64 로 받는다.
fn window_from(v: Option<&serde_json::Value>) -> Option<Window> {
    let w = v?.as_object()?;
    Some(Window {
        used_percent: w.get("usedPercent")?.as_f64()?,
        window_minutes: w.get("windowDurationMins").and_then(serde_json::Value::as_i64),
        // resets_at 은 rollout 과 맞춰 "없음 = 0" 으로 표현한다(epoch() 가 0을 None 처리).
        resets_at: w
            .get("resetsAt")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    })
}

/// 백엔드 영문 오류를 카드에 띄울 한 줄로. 로그인 문제는 해결 방법까지 붙인다 —
/// 카드만 보고 `codex login` 을 떠올릴 수 있어야 한다.
fn friendly_error(msg: &str) -> QuotaError {
    let lowered = msg.to_ascii_lowercase();
    if lowered.contains("authentication required") || lowered.contains("not logged in") {
        return QuotaError::auth("Codex 로그인 필요 — 터미널에서 `codex login`");
    }
    if lowered.contains("method not found") {
        return QuotaError::other("이 codex 버전은 한도 조회 RPC를 지원하지 않습니다 (codex update)");
    }
    QuotaError::other(format!("Codex 한도 조회 실패: {msg}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 알림과 다른 id 의 응답은 흘려보내고 우리 응답만 집어야 한다 —
    /// 실제 app-server 는 `remoteControl/status/changed` 를 먼저 뱉는다.
    #[test]
    fn skips_notifications_and_other_ids() {
        assert!(parse_response(
            r#"{"method":"remoteControl/status/changed","params":{"status":"disabled"}}"#,
            RATE_LIMITS_ID
        )
        .is_none());
        assert!(parse_response(r#"{"id":1,"result":{"codexHome":"/x"}}"#, RATE_LIMITS_ID).is_none());
        assert!(parse_response("not json", RATE_LIMITS_ID).is_none());
    }

    #[test]
    fn parses_primary_and_secondary_windows() {
        let line = r#"{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":7,"resetsAt":1788500571,"windowDurationMins":300},"secondary":{"usedPercent":35,"resetsAt":1788748697,"windowDurationMins":10080}}}}"#;
        let rl = parse_response(line, RATE_LIMITS_ID).unwrap().unwrap();
        let quota = CodexQuota::default();
        apply_rate_limits(&rl, &quota);
        assert_eq!(*quota.used_pct_5h.lock().unwrap(), Some(7.0));
        assert_eq!(*quota.used_pct_weekly.lock().unwrap(), Some(35.0));
        assert!(quota.reset_5h.lock().unwrap().is_some());
        assert!(quota.reset_weekly.lock().unwrap().is_some());
    }

    /// rollout 이 남긴 과거 rate_limits 는 **에러를 지우면 안 된다.** 지우면 방금
    /// 실패한 조회의 "로그인 필요" 가 낡은 로그 한 줄에 덮여 카드에서 사라진다.
    #[test]
    fn rollout_apply_does_not_clear_error() {
        let quota = CodexQuota::default();
        *quota.last_error.lock().unwrap() = Some(QuotaError::auth("Codex 로그인 필요"));
        let line = r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":12}}}}"#;
        let rl = parse_response(line, RATE_LIMITS_ID).unwrap().unwrap();
        apply_rate_limits(&rl, &quota);
        assert_eq!(
            quota.last_error.lock().unwrap().as_ref().map(|e| e.message.clone()),
            Some("Codex 로그인 필요".to_string())
        );
        assert_eq!(*quota.used_pct_5h.lock().unwrap(), Some(12.0));
    }

    /// 실제로 로그인 안 된 머신에서 받은 응답(2026-09-04).
    #[test]
    fn auth_error_becomes_actionable_message() {
        let line = r#"{"error":{"code":-32600,"message":"codex account authentication required to read rate limits"},"id":2}"#;
        let err = parse_response(line, RATE_LIMITS_ID).unwrap().unwrap_err();
        assert_eq!(err.message, "Codex 로그인 필요 — 터미널에서 `codex login`");
        assert_eq!(err.kind, crate::types::QuotaErrorKind::Auth, "미러로는 이 분류가 나간다");
    }

    #[test]
    fn falls_back_to_multi_bucket_view() {
        let line = r#"{"id":2,"result":{"rateLimits":{"primary":null,"secondary":null},"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":50,"windowDurationMins":300}}}}}"#;
        let rl = parse_response(line, RATE_LIMITS_ID).unwrap().unwrap();
        let quota = CodexQuota::default();
        apply_rate_limits(&rl, &quota);
        assert_eq!(*quota.used_pct_5h.lock().unwrap(), Some(50.0));
    }

    /// 진짜 `codex` 를 띄워 spawn→handshake→응답 파싱 전 구간을 확인한다.
    /// 로그인 여부에 따라 결과가 갈리므로 CI 에서는 돌리지 않는다:
    ///   - 로그인 O: 5h/주간 창이 실제로 온다
    ///   - 로그인 X: "Codex 로그인 필요 — 터미널에서 `codex login`"
    /// 어느 쪽이든 **프로토콜이 통했다는 뜻**이다. 실패해야 할 건 그 둘 다 아닌 경우.
    #[tokio::test]
    #[ignore = "진짜 codex app-server 를 띄운다 — 손으로만"]
    async fn live_round_trip_against_real_codex() {
        let home = dirs_next::home_dir().unwrap();
        match read_rate_limits("codex", &home).await {
            Ok(rl) => {
                println!("primary={:?} secondary={:?}", rl.primary, rl.secondary);
                assert!(rl.primary.is_some() || rl.secondary.is_some());
            }
            Err(e) => {
                println!("err={}", e.message);
                assert!(
                    e.message.contains("로그인") || e.message.contains("사용 한도가 없습니다"),
                    "프로토콜이 아니라 다른 데서 깨졌다: {}",
                    e.message
                );
            }
        }
    }

    /// 반대 방향: 조회가 **성공하면** 이전 실패 문구가 사라져야 한다. 안 그러면
    /// `codex login` 을 마친 뒤에도 "로그인 필요" 가 카드에 계속 붙어 있다.
    /// (rollout 이 지우면 안 되는 것과 짝이 되는 테스트 —
    /// `rollout_apply_does_not_clear_error` 참고.)
    #[tokio::test]
    #[ignore = "진짜 codex app-server 를 띄운다 — 로그인된 상태에서 손으로만"]
    async fn live_poll_clears_previous_error() {
        let quota = CodexQuota::default();
        *quota.last_error.lock().unwrap() = Some(QuotaError::other("이전 실패 문구"));
        let home = dirs_next::home_dir().unwrap();
        poll_rate_limits("codex", &home, &quota).await;
        assert_eq!(
            *quota.last_error.lock().unwrap(),
            None,
            "조회 성공했는데 이전 에러가 남아 있다"
        );
        assert!(quota.used_pct_5h.lock().unwrap().is_some());
    }

    /// 한도 자체가 안 오는 계정(API 키 등)은 조용히 성공한 척하면 안 된다.
    #[test]
    fn missing_windows_is_an_error() {
        let line = r#"{"id":2,"result":{"rateLimits":{"primary":null,"secondary":null}}}"#;
        assert!(parse_response(line, RATE_LIMITS_ID).unwrap().is_err());
    }
}
