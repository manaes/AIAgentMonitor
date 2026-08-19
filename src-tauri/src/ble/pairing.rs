//! 페어링 인증 상태 기계 (스펙 5.1).
//!
//! 이 모듈은 순수하다 — CoreBluetooth 도 파일 I/O 도 모른다. 그래야 코드 만료,
//! 시도 제한, 토큰 재사용 같은 보안 성질을 하드웨어 없이 검증할 수 있다.
//!
//! 무차별 대입 방어는 두 축이다: 코드 수명 120초와 시도 5회. 6자리는 100만
//! 조합이므로, 5회로는 성공 확률이 0.0005% 다. 이 두 상수를 늘리면 그 근거가
//! 무너지므로 임의로 바꾸지 않는다.
use crate::ble::peripheral::CentralId;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

pub const CODE_TTL: Duration = Duration::from_secs(120);
pub const MAX_ATTEMPTS: u8 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthRequest {
    Hello,
    Code(String),
    Token(String),
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthReply {
    CodeIssued { code: String },
    Granted { token: String },
    Denied { left: u8 },
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    Unauthorized,
    AwaitingCode,
    Authorized,
}

/// central 이 Auth 특성에 쓴 바이트를 요청으로 해석한다.
/// 알 수 없는 형태는 전부 Malformed — 모르는 것을 관대하게 받아주지 않는다.
pub fn parse_auth_request(bytes: &[u8]) -> AuthRequest {
    let Ok(s) = std::str::from_utf8(bytes) else {
        return AuthRequest::Malformed;
    };
    let s = s.trim();
    if s == "HELLO" {
        return AuthRequest::Hello;
    }
    if let Some(rest) = s.strip_prefix("CODE:") {
        return AuthRequest::Code(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("TOKEN:") {
        return AuthRequest::Token(rest.to_string());
    }
    AuthRequest::Malformed
}

#[derive(Debug)]
struct PendingCode {
    code: String,
    issued_at: SystemTime,
    attempts_left: u8,
    /// HELLO 를 보낸 central. 다른 central 이 이 코드를 들이밀어도 시도 횟수를
    /// 깎지 않는다 — 남의 페어링 세션을 노려 소유자의 시도 횟수를 소진시키는
    /// 것을 막는다.
    owner: String,
}

#[derive(Debug, Default)]
pub struct PairingManager {
    pending: Option<PendingCode>,
    tokens: HashSet<String>,
    authorized: HashMap<String, ()>,
}

impl PairingManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 영속화된 토큰을 복원한다. 앱 재시작 후에도 이미 페어링한 기기가 통과해야 한다.
    pub fn load_tokens(&mut self, tokens: Vec<String>) {
        self.tokens.extend(tokens);
    }

    pub fn issued_tokens(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tokens.iter().cloned().collect();
        v.sort();
        v
    }

    pub fn is_authorized(&self, id: &CentralId) -> bool {
        self.authorized.contains_key(&id.0)
    }

    pub fn forget(&mut self, id: &CentralId) {
        self.authorized.remove(&id.0);
    }

    /// 화면에 표시할 코드. 만료됐으면 None — UI 가 따로 만료를 계산하지 않게 한다.
    pub fn visible_code(&self, now: SystemTime) -> Option<String> {
        let p = self.pending.as_ref()?;
        if Self::expired(p, now) {
            return None;
        }
        Some(p.code.clone())
    }

    fn expired(p: &PendingCode, now: SystemTime) -> bool {
        now.duration_since(p.issued_at).unwrap_or_default() > CODE_TTL
    }

    pub fn handle(&mut self, id: &CentralId, req: AuthRequest, now: SystemTime) -> AuthReply {
        match req {
            AuthRequest::Hello => {
                // 새 코드를 내면 이전 코드는 폐기한다. 동시에 둘이 살아 있으면 공격면이 넓어진다.
                let code = Self::random_code();
                self.pending = Some(PendingCode {
                    code: code.clone(),
                    issued_at: now,
                    attempts_left: MAX_ATTEMPTS,
                    owner: id.0.clone(),
                });
                AuthReply::CodeIssued { code }
            }
            AuthRequest::Code(given) => {
                let Some(p) = self.pending.as_mut() else {
                    return AuthReply::Rejected;
                };
                if p.owner != id.0 {
                    // 이 코드는 다른 central 의 페어링 세션 것이다 — 폐기된 것과
                    // 같이 취급한다. 시도 횟수는 건드리지 않는다.
                    return AuthReply::Rejected;
                }
                if Self::expired(p, now) {
                    self.pending = None;
                    return AuthReply::Rejected;
                }
                if p.attempts_left == 0 {
                    self.pending = None;
                    return AuthReply::Rejected;
                }
                if given == p.code {
                    let token = Self::random_token();
                    self.tokens.insert(token.clone());
                    self.authorized.insert(id.0.clone(), ());
                    self.pending = None;
                    AuthReply::Granted { token }
                } else {
                    p.attempts_left -= 1;
                    let left = p.attempts_left;
                    if left == 0 {
                        // 소진되면 코드를 즉시 폐기한다. 다음 시도는 Rejected 로 떨어진다.
                        self.pending = None;
                    }
                    AuthReply::Denied { left }
                }
            }
            AuthRequest::Token(given) => {
                if self.tokens.contains(&given) {
                    self.authorized.insert(id.0.clone(), ());
                    AuthReply::Granted { token: given }
                } else {
                    AuthReply::Rejected
                }
            }
            AuthRequest::Malformed => AuthReply::Rejected,
        }
    }

    fn random_code() -> String {
        format!("{:06}", Self::random_u64() % 1_000_000)
    }

    fn random_token() -> String {
        format!("{:016x}{:016x}", Self::random_u64(), Self::random_u64())
    }

    /// 암호학적 난수. std 에 CSPRNG 가 없으므로 OS 엔트로피를 직접 읽는다.
    /// 페어링 코드와 토큰은 추측 가능하면 안 되므로 시각 기반 의사난수를 쓰지 않는다.
    fn random_u64() -> u64 {
        use std::io::Read;
        let mut buf = [0u8; 8];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut buf))
            .expect("/dev/urandom 을 읽을 수 없다");
        u64::from_le_bytes(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::peripheral::CentralId;
    use std::time::{Duration, UNIX_EPOCH};

    fn t(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }
    fn id(s: &str) -> CentralId {
        CentralId(s.to_string())
    }

    #[test]
    fn parses_each_request_form() {
        assert!(matches!(parse_auth_request(b"HELLO"), AuthRequest::Hello));
        assert!(matches!(parse_auth_request(b"CODE:123456"), AuthRequest::Code(c) if c == "123456"));
        assert!(matches!(parse_auth_request(b"TOKEN:abcdef"), AuthRequest::Token(t) if t == "abcdef"));
        assert!(matches!(parse_auth_request(b"NONSENSE"), AuthRequest::Malformed));
        assert!(matches!(parse_auth_request(&[0xff, 0xfe]), AuthRequest::Malformed),
                "UTF-8 이 아니면 Malformed");
    }

    #[test]
    fn hello_issues_six_digit_code() {
        let mut m = PairingManager::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello, t(1000));
        let AuthReply::CodeIssued { code } = reply else { panic!("코드가 발급돼야 한다") };
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()), "6자리 숫자여야 한다: {code}");
        assert!(!m.is_authorized(&id("A")), "코드만 발급된 상태는 아직 미인가");
    }

    #[test]
    fn correct_code_grants_a_128bit_token() {
        let mut m = PairingManager::new();
        let AuthReply::CodeIssued { code } = m.handle(&id("A"), AuthRequest::Hello, t(1000)) else {
            panic!()
        };
        let reply = m.handle(&id("A"), AuthRequest::Code(code), t(1010));
        let AuthReply::Granted { token } = reply else { panic!("인가돼야 한다") };
        assert_eq!(token.len(), 32, "128비트 = hex 32자");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "소문자 hex 여야 한다: {token}");
        assert!(m.is_authorized(&id("A")));
    }

    #[test]
    fn wrong_code_counts_down_and_locks_out_after_five() {
        let mut m = PairingManager::new();
        m.handle(&id("A"), AuthRequest::Hello, t(1000));
        for expected_left in [4u8, 3, 2, 1] {
            let r = m.handle(&id("A"), AuthRequest::Code("000000".into()), t(1001));
            assert!(matches!(r, AuthReply::Denied { left } if left == expected_left),
                    "남은 시도 {expected_left} 이어야 한다: {r:?}");
        }
        let last = m.handle(&id("A"), AuthRequest::Code("000000".into()), t(1002));
        assert!(matches!(last, AuthReply::Denied { left: 0 }), "5회째는 left 0");
        assert!(!m.is_authorized(&id("A")));

        // 시도 소진 후에는 올바른 코드도 통하지 않는다 — 코드가 폐기됐다.
        let after = m.handle(&id("A"), AuthRequest::Code("000000".into()), t(1003));
        assert!(matches!(after, AuthReply::Rejected), "코드 폐기 후에는 Rejected");
    }

    #[test]
    fn code_expires_after_ttl() {
        let mut m = PairingManager::new();
        let AuthReply::CodeIssued { code } = m.handle(&id("A"), AuthRequest::Hello, t(1000)) else {
            panic!()
        };
        let r = m.handle(&id("A"), AuthRequest::Code(code), t(1000 + 121));
        assert!(matches!(r, AuthReply::Rejected), "120초를 넘기면 만료");
        assert!(!m.is_authorized(&id("A")));
    }

    #[test]
    fn code_still_valid_at_exactly_ttl() {
        let mut m = PairingManager::new();
        let AuthReply::CodeIssued { code } = m.handle(&id("A"), AuthRequest::Hello, t(1000)) else {
            panic!()
        };
        let r = m.handle(&id("A"), AuthRequest::Code(code), t(1000 + 120));
        assert!(matches!(r, AuthReply::Granted { .. }), "정확히 120초는 아직 유효");
    }

    #[test]
    fn issued_token_authorizes_a_new_connection() {
        let mut m = PairingManager::new();
        let AuthReply::CodeIssued { code } = m.handle(&id("A"), AuthRequest::Hello, t(1000)) else {
            panic!()
        };
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        // 재연결은 새 CentralId 로 올 수 있다.
        let r = m.handle(&id("B"), AuthRequest::Token(token), t(2000));
        assert!(matches!(r, AuthReply::Granted { .. }));
        assert!(m.is_authorized(&id("B")));
    }

    #[test]
    fn unknown_token_is_rejected() {
        let mut m = PairingManager::new();
        let r = m.handle(&id("A"), AuthRequest::Token("deadbeef".into()), t(1000));
        assert!(matches!(r, AuthReply::Rejected));
        assert!(!m.is_authorized(&id("A")));
    }

    #[test]
    fn malformed_input_never_authorizes() {
        let mut m = PairingManager::new();
        assert!(matches!(m.handle(&id("A"), AuthRequest::Malformed, t(1000)), AuthReply::Rejected));
        assert!(!m.is_authorized(&id("A")));
    }

    #[test]
    fn code_is_not_reused_across_hellos() {
        let mut m = PairingManager::new();
        let AuthReply::CodeIssued { code: first } = m.handle(&id("A"), AuthRequest::Hello, t(1000)) else {
            panic!()
        };
        let AuthReply::CodeIssued { code: second } = m.handle(&id("B"), AuthRequest::Hello, t(1001)) else {
            panic!()
        };
        // 첫 코드는 무효화돼야 한다 — 동시에 두 코드가 살아 있으면 공격면이 두 배가 된다.
        let r = m.handle(&id("A"), AuthRequest::Code(first), t(1002));
        assert!(matches!(r, AuthReply::Rejected), "이전 코드는 폐기된다");
        let ok = m.handle(&id("B"), AuthRequest::Code(second), t(1003));
        assert!(matches!(ok, AuthReply::Granted { .. }));
    }

    #[test]
    fn visible_code_reflects_issue_and_expiry() {
        let mut m = PairingManager::new();
        assert_eq!(m.visible_code(t(1000)), None, "발급 전에는 표시할 코드가 없다");
        let AuthReply::CodeIssued { code } = m.handle(&id("A"), AuthRequest::Hello, t(1000)) else {
            panic!()
        };
        assert_eq!(m.visible_code(t(1050)), Some(code));
        assert_eq!(m.visible_code(t(1121)), None, "만료되면 화면에서도 사라진다");
    }

    #[test]
    fn forget_revokes_authorization() {
        let mut m = PairingManager::new();
        let AuthReply::CodeIssued { code } = m.handle(&id("A"), AuthRequest::Hello, t(1000)) else {
            panic!()
        };
        m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        assert!(m.is_authorized(&id("A")));
        m.forget(&id("A"));
        assert!(!m.is_authorized(&id("A")), "해제하면 즉시 미인가");
    }

    #[test]
    fn loaded_tokens_authorize_without_pairing() {
        let mut m = PairingManager::new();
        m.load_tokens(vec!["a".repeat(32)]);
        let r = m.handle(&id("A"), AuthRequest::Token("a".repeat(32)), t(1000));
        assert!(matches!(r, AuthReply::Granted { .. }), "저장된 토큰은 앱 재시작 후에도 통한다");
    }

    #[test]
    fn issued_tokens_are_persistable() {
        let mut m = PairingManager::new();
        let AuthReply::CodeIssued { code } = m.handle(&id("A"), AuthRequest::Hello, t(1000)) else {
            panic!()
        };
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        assert!(m.issued_tokens().contains(&token));
    }
}
