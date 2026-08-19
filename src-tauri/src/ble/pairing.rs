//! 페어링 인증 상태 기계 (스펙 5.1).
//!
//! 이 모듈은 순수하다 — CoreBluetooth 도 모르고 설정 파일도 읽거나 쓰지 않는다.
//! (유일한 예외: 암호학적 난수를 위해 `/dev/urandom` 을 직접 읽는다 — 이
//! 크레이트에는 CSPRNG 가 없다.) 그래야 코드 만료, 시도 제한, 토큰 재사용
//! 같은 보안 성질을 하드웨어 없이 검증할 수 있다.
//!
//! 무차별 대입 방어는 코드 수명 120초와, **사용자가 연 창** 하나당 시도
//! 5회다. `HELLO` 는 코드를 발급하지 않는다 — 그러면 공짜로, 무제한으로
//! 예산을 리셋할 수 있어 시도 제한이 무의미해진다. 공격자가
//! `반복 { HELLO 로 예산 리셋 ; 5회 추측 }` 하면 BLE write 속도 기준 약
//! 9시간 만에 100만 조합을 소진할 수 있었다(초안의 결함, 스펙 5.1 참고).
//! 코드는 반드시 `begin_pairing` 을 통해서만 — 사용자가 Mac Devices 탭에서
//! 페어링을 시작할 때 — 발급된다. 6자리는 100만 조합이므로, 창당 5회로는
//! 성공 확률이 0.0005% 다. 이 두 상수를 늘리면 그 근거가 무너지므로 임의로
//! 바꾸지 않는다.
use crate::ble::peripheral::CentralId;
use std::collections::HashSet;
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
    /// 창이 열려 있고 이 central 에 바인딩됐다는 응답. 코드 자체는 담지
    /// 않는다 — 코드는 오직 `begin_pairing` 을 통해 로컬(Mac UI)로만 전달된다.
    AwaitingCode,
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
    /// 이 창을 처음 건드린(HELLO 또는 CODE 를 처음 보낸) central. 처음
    /// 보낸 쪽이 창을 "소유"하게 되고, 그 뒤로는 다른 central 이 무엇을
    /// 보내든 — 심지어 우연히 맞는 코드라도 — 이 창에 대해서는 거부된다.
    /// 아직 아무도 건드리지 않았으면 None.
    owner: Option<String>,
}

#[derive(Debug, Default)]
pub struct PairingManager {
    pending: Option<PendingCode>,
    tokens: HashSet<String>,
    authorized: HashSet<String>,
}

impl PairingManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 사용자가 Mac Devices 탭에서 "페어링 시작"을 누르면 호출한다. 6자리
    /// 코드를 발급하고, 120초짜리 "페어링 창"을 열고, 이 창의 시도 예산을
    /// 5회로 되돌린다. 새 창은 이전 창의 코드를 즉시 무효화한다 — 동시에
    /// 두 코드가 살아 있으면 공격면이 두 배가 된다.
    pub fn begin_pairing(&mut self, now: SystemTime) -> String {
        let code = Self::random_code();
        self.pending = Some(PendingCode {
            code: code.clone(),
            issued_at: now,
            attempts_left: MAX_ATTEMPTS,
            owner: None,
        });
        code
    }

    /// 영속화된 토큰을 복원한다. 앱 재시작 후에도 이미 페어링한 기기가
    /// 통과해야 한다. `ble-peers.json` 이 손상되거나 잘려도 인증 우회로
    /// 이어지지 않도록, 형식(정확히 32자 소문자 hex)에 맞지 않는 항목은
    /// 조용히 버린다.
    pub fn load_tokens(&mut self, tokens: Vec<String>) {
        self.tokens
            .extend(tokens.into_iter().filter(|t| Self::is_valid_token_format(t)));
    }

    fn is_valid_token_format(s: &str) -> bool {
        s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }

    pub fn issued_tokens(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tokens.iter().cloned().collect();
        v.sort();
        v
    }

    pub fn is_authorized(&self, id: &CentralId) -> bool {
        self.authorized.contains(&id.0)
    }

    /// 이 central 의 세션 인가만 지운다(연결이 끊겼을 때, 또는 사용자가
    /// 세션만 끊고 싶을 때 호출). 저장된 토큰 자체는 지우지 않으므로,
    /// 같은 토큰으로 다시 `TOKEN:` 인증하면 즉시 재인가된다 — 완전한
    /// 언페어링(토큰 폐기)이 필요하면 `revoke_token`/`revoke_all` 을 함께
    /// 호출해야 한다.
    ///
    /// 호출자(BLE 브리지)는 central 의 연결이 끊어질 때 반드시 이를
    /// 호출해야 한다 — 그러지 않으면 인가 상태가 실제 연결보다 오래
    /// 살아남는다.
    pub fn end_session(&mut self, id: &CentralId) {
        self.authorized.remove(&id.0);
    }

    /// 토큰 하나를 완전히 폐기한다 — 이후 이 토큰으로의 `TOKEN:` 인증은
    /// 거부된다. 스펙 6의 언페어링(`ble_unpair`)이 사용한다.
    pub fn revoke_token(&mut self, token: &str) {
        self.tokens.remove(token);
    }

    /// 저장된 토큰을 모두 폐기한다(전체 언페어링).
    pub fn revoke_all(&mut self) {
        self.tokens.clear();
    }

    /// 화면에 표시할 코드. 만료됐으면 None — UI 가 따로 만료를 계산하지
    /// 않게 한다.
    pub fn visible_code(&self, now: SystemTime) -> Option<String> {
        let p = self.pending.as_ref()?;
        if Self::expired(p, now) {
            return None;
        }
        Some(p.code.clone())
    }

    /// 이 central 이 현재 어떤 인증 상태인지. UI/로깅이 쓸 수 있게 세 값을
    /// 실제 상태에서 도출한다.
    pub fn state(&self, id: &CentralId, now: SystemTime) -> AuthState {
        if self.is_authorized(id) {
            return AuthState::Authorized;
        }
        if let Some(p) = &self.pending {
            if !Self::expired(p, now) && p.owner.as_deref() == Some(id.0.as_str()) {
                return AuthState::AwaitingCode;
            }
        }
        AuthState::Unauthorized
    }

    fn expired(p: &PendingCode, now: SystemTime) -> bool {
        now.duration_since(p.issued_at).unwrap_or_default() > CODE_TTL
    }

    /// 창이 열려 있는지 확인하고, 아직 아무도 소유하지 않았다면 이
    /// central 에 바인딩한다. 이미 다른 central 에 바인딩돼 있으면 None —
    /// 이 요청은 시도 횟수를 건드리지 않고 거부돼야 한다(다른 central 의
    /// 페어링 세션을 노려 소유자의 시도 예산을 소진시키는 것을 막는다).
    /// 만료됐으면 창을 닫고 None.
    fn open_window_for(&mut self, id: &CentralId, now: SystemTime) -> Option<&mut PendingCode> {
        if matches!(&self.pending, Some(p) if Self::expired(p, now)) {
            self.pending = None;
        }
        let p = self.pending.as_mut()?;
        match &p.owner {
            None => p.owner = Some(id.0.clone()),
            Some(o) if o == &id.0 => {}
            Some(_) => return None,
        }
        Some(p)
    }

    pub fn handle(&mut self, id: &CentralId, req: AuthRequest, now: SystemTime) -> AuthReply {
        match req {
            AuthRequest::Hello => match self.open_window_for(id, now) {
                Some(_) => AuthReply::AwaitingCode,
                None => AuthReply::Rejected,
            },
            AuthRequest::Code(given) => {
                let Some(p) = self.open_window_for(id, now) else {
                    return AuthReply::Rejected;
                };
                if given == p.code {
                    let token = Self::random_token();
                    self.tokens.insert(token.clone());
                    self.authorized.insert(id.0.clone());
                    self.pending = None;
                    AuthReply::Granted { token }
                } else {
                    p.attempts_left -= 1;
                    let left = p.attempts_left;
                    if left == 0 {
                        // 창 소속 예산이 소진됐다 — 창을 즉시 폐기한다. 이후
                        // 이 central 이 HELLO 를 아무리 반복해도 새 창은
                        // `begin_pairing`(사용자 제스처) 없이는 열리지
                        // 않는다.
                        self.pending = None;
                    }
                    AuthReply::Denied { left }
                }
            }
            AuthRequest::Token(given) => {
                if self.tokens.contains(&given) {
                    self.authorized.insert(id.0.clone());
                    // 이미 클라이언트가 알고 있는 자격증명을 평문 링크로
                    // 되돌려보낼 이유가 없다 — 새 토큰이 아니므로 빈 값.
                    AuthReply::Granted {
                        token: String::new(),
                    }
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
    /// 페어링 코드와 토큰은 추측 가능하면 안 되므로 시각 기반 의사난수를
    /// 쓰지 않는다.
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

    /// 실제 코드와 다른 6자리 숫자열 하나를 만든다(마지막 자리만 뒤집는다).
    /// 리터럴 "000000" 을 쓰지 않는 이유: 실제 코드와 100만분의 1 확률로
    /// 우연히 같아질 수 있기 때문이다.
    fn wrong_code(real: &str) -> String {
        let mut chars: Vec<char> = real.chars().collect();
        let last = chars[5];
        chars[5] = if last == '0' { '1' } else { '0' };
        chars.into_iter().collect()
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
    fn begin_pairing_issues_six_digit_code() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()), "6자리 숫자여야 한다: {code}");
        assert!(!m.is_authorized(&id("A")), "코드 발급만으로는 아무도 인가되지 않는다");
    }

    #[test]
    fn hello_without_open_window_is_rejected() {
        let mut m = PairingManager::new();
        let r = m.handle(&id("A"), AuthRequest::Hello, t(1000));
        assert!(matches!(r, AuthReply::Rejected), "열린 창이 없으면 HELLO 도 거부된다");
    }

    #[test]
    fn code_without_open_window_is_rejected() {
        let mut m = PairingManager::new();
        let r = m.handle(&id("A"), AuthRequest::Code("123456".into()), t(1000));
        assert!(matches!(r, AuthReply::Rejected), "열린 창이 없으면 CODE 도 거부된다");
    }

    #[test]
    fn hello_replies_awaiting_code_when_window_open() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        let reply = m.handle(&id("A"), AuthRequest::Hello, t(1000));
        assert!(matches!(reply, AuthReply::AwaitingCode));
        assert!(!m.is_authorized(&id("A")), "코드 대기 상태는 아직 미인가");
    }

    #[test]
    fn code_without_prior_hello_still_binds_and_grants() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        // HELLO 를 생략하고 바로 CODE 를 보내도, 이 central 이 창을
        // 소유하게 되고 통과해야 한다 — 바인딩은 HELLO 전용이 아니다.
        let r = m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        assert!(matches!(r, AuthReply::Granted { .. }));
    }

    #[test]
    fn correct_code_grants_a_128bit_token() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Hello, t(1000));
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
        let code = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Hello, t(1000));
        for expected_left in [4u8, 3, 2, 1] {
            let r = m.handle(&id("A"), AuthRequest::Code(wrong_code(&code)), t(1001));
            assert!(matches!(r, AuthReply::Denied { left } if left == expected_left),
                    "남은 시도 {expected_left} 이어야 한다: {r:?}");
        }
        let last = m.handle(&id("A"), AuthRequest::Code(wrong_code(&code)), t(1002));
        assert!(matches!(last, AuthReply::Denied { left: 0 }), "5회째는 left 0");
        assert!(!m.is_authorized(&id("A")));

        // 예산 소진 후에는 "진짜" 코드를 넣어도 통하지 않는다 — 창이
        // 완전히 폐기됐다(추측용 값이 아니라 실제 코드로 확인한다).
        let after = m.handle(&id("A"), AuthRequest::Code(code), t(1003));
        assert!(matches!(after, AuthReply::Rejected), "창 폐기 후에는 Rejected");
    }

    #[test]
    fn code_expires_after_ttl() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let r = m.handle(&id("A"), AuthRequest::Code(code), t(1000 + 121));
        assert!(matches!(r, AuthReply::Rejected), "120초를 넘기면 만료");
        assert!(!m.is_authorized(&id("A")));
    }

    #[test]
    fn code_still_valid_at_exactly_ttl() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let r = m.handle(&id("A"), AuthRequest::Code(code), t(1000 + 120));
        assert!(matches!(r, AuthReply::Granted { .. }), "정확히 120초는 아직 유효");
    }

    #[test]
    fn issued_token_authorizes_a_new_connection() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        // 재연결은 새 CentralId 로 올 수 있다.
        let r = m.handle(&id("B"), AuthRequest::Token(token), t(2000));
        assert!(matches!(r, AuthReply::Granted { .. }));
        assert!(m.is_authorized(&id("B")));
    }

    #[test]
    fn token_reconnect_does_not_echo_the_token_back() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        let r = m.handle(&id("B"), AuthRequest::Token(token), t(2000));
        let AuthReply::Granted { token: echoed } = r else { panic!("인가돼야 한다") };
        assert!(echoed.is_empty(), "이미 아는 자격증명을 평문 링크로 되돌려보낼 필요가 없다");
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
    fn new_pairing_window_invalidates_previous_code() {
        let mut m = PairingManager::new();
        let first = m.begin_pairing(t(1000));
        let second = m.begin_pairing(t(1001));
        // 첫 창의 코드는 더 이상 통하지 않는다 — 두 번째 창 기준으로는 그저
        // 틀린 추측 하나일 뿐이다(동시에 두 코드가 살아 있으면 공격면이
        // 두 배가 되므로, 예전 코드가 "예외적으로 여전히 유효"해서는 안
        // 된다).
        let r = m.handle(&id("A"), AuthRequest::Code(first), t(1002));
        assert!(matches!(r, AuthReply::Denied { .. }), "이전 창의 코드는 더 이상 통하지 않는다");
        let ok = m.handle(&id("A"), AuthRequest::Code(second), t(1003));
        assert!(matches!(ok, AuthReply::Granted { .. }));
    }

    #[test]
    fn foreign_guesses_do_not_drain_owner_budget() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        // A 가 정당한 소유자로 바인딩된다.
        m.handle(&id("A"), AuthRequest::Hello, t(1000));
        // B(공격자)가 다섯 번 틀린 코드를 넣어도 A 의 예산에는 영향이
        // 없어야 한다.
        for _ in 0..5 {
            let r = m.handle(&id("B"), AuthRequest::Code(wrong_code(&code)), t(1000));
            assert!(matches!(r, AuthReply::Rejected), "다른 central 의 제출은 시도 횟수를 건드리지 않는다");
        }
        // A 는 여전히 5회를 온전히 갖고 있다.
        for expected_left in [4u8, 3, 2, 1, 0] {
            let r = m.handle(&id("A"), AuthRequest::Code(wrong_code(&code)), t(1000));
            assert!(matches!(r, AuthReply::Denied { left } if left == expected_left),
                    "B 의 시도가 A 의 예산을 깎으면 안 된다: {r:?}");
        }
    }

    /// C1 회귀 테스트: 공격자가 `HELLO` 를 반복해 시도 예산을 리셋하려
    /// 해도, 창당(사용자가 연 창) 5회라는 총량을 절대 넘길 수 없어야
    /// 한다. 이 테스트가 이 모듈이 존재하는 이유다.
    #[test]
    fn attacker_cannot_reset_budget_by_repeating_hello() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));

        // 라운드 0: 공격자가 창에 바인딩되고, 5회를 모두 소진한다.
        let hello0 = m.handle(&id("ATK"), AuthRequest::Hello, t(1000));
        assert!(matches!(hello0, AuthReply::AwaitingCode));
        for expected_left in [4u8, 3, 2, 1, 0] {
            let r = m.handle(&id("ATK"), AuthRequest::Code(wrong_code(&code)), t(1000));
            assert!(matches!(r, AuthReply::Denied { left } if left == expected_left));
        }
        assert!(!m.is_authorized(&id("ATK")));

        // 라운드 1, 2: 공격자가 HELLO 를 반복해 예산을 되살리려 한다 —
        // 창이 사용자 제스처(begin_pairing) 없이는 다시 열리지 않으므로
        // 반드시 실패해야 한다.
        for _ in 0..2 {
            let hello = m.handle(&id("ATK"), AuthRequest::Hello, t(1000));
            assert!(matches!(hello, AuthReply::Rejected),
                    "예산 소진 후 창은 닫혀 있다 — HELLO 로 되살아나면 안 된다");
            for _ in 0..5 {
                let r = m.handle(&id("ATK"), AuthRequest::Code(wrong_code(&code)), t(1000));
                assert!(matches!(r, AuthReply::Rejected), "닫힌 창에 대한 시도는 Rejected 다");
            }
        }

        // 진짜 코드를 넣어도 거부된다 — 창이 완전히, 영구히 닫혔다. 공격자는
        // 15번을 시도했지만 실제로 소비 가능했던 시도는 처음 5회뿐이었다.
        let after = m.handle(&id("ATK"), AuthRequest::Code(code), t(1000));
        assert!(matches!(after, AuthReply::Rejected));
        assert!(!m.is_authorized(&id("ATK")));
    }

    #[test]
    fn visible_code_reflects_issue_and_expiry() {
        let mut m = PairingManager::new();
        assert_eq!(m.visible_code(t(1000)), None, "발급 전에는 표시할 코드가 없다");
        let code = m.begin_pairing(t(1000));
        assert_eq!(m.visible_code(t(1050)), Some(code));
        assert_eq!(m.visible_code(t(1121)), None, "만료되면 화면에서도 사라진다");
    }

    #[test]
    fn end_session_revokes_authorization() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        assert!(m.is_authorized(&id("A")));
        m.end_session(&id("A"));
        assert!(!m.is_authorized(&id("A")), "세션을 끊으면 즉시 미인가");
    }

    #[test]
    fn end_session_does_not_revoke_the_token_itself() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        m.end_session(&id("A"));
        assert!(!m.is_authorized(&id("A")));
        // 토큰 자체는 살아있다 — 같은 토큰으로 다시 인증하면 즉시
        // 재인가된다. 완전한 폐기는 revoke_token 이 담당한다.
        let r = m.handle(&id("A"), AuthRequest::Token(token), t(1002));
        assert!(matches!(r, AuthReply::Granted { .. }));
        assert!(m.is_authorized(&id("A")));
    }

    #[test]
    fn revoke_token_prevents_future_authorization() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        m.revoke_token(&token);
        let r = m.handle(&id("A"), AuthRequest::Token(token), t(1002));
        assert!(matches!(r, AuthReply::Rejected), "폐기된 토큰은 더 이상 통하지 않는다");
    }

    #[test]
    fn revoke_all_clears_every_stored_token() {
        let mut m = PairingManager::new();
        m.load_tokens(vec!["a".repeat(32), "b".repeat(32)]);
        m.revoke_all();
        assert!(m.issued_tokens().is_empty());
        let r = m.handle(&id("A"), AuthRequest::Token("a".repeat(32)), t(1000));
        assert!(matches!(r, AuthReply::Rejected));
    }

    #[test]
    fn loaded_tokens_authorize_without_pairing() {
        let mut m = PairingManager::new();
        m.load_tokens(vec!["a".repeat(32)]);
        let r = m.handle(&id("A"), AuthRequest::Token("a".repeat(32)), t(1000));
        assert!(matches!(r, AuthReply::Granted { .. }), "저장된 토큰은 앱 재시작 후에도 통한다");
    }

    #[test]
    fn load_tokens_drops_malformed_entries() {
        let mut m = PairingManager::new();
        m.load_tokens(vec![
            "".to_string(),         // 빈 문자열
            "short".to_string(),    // 32자 미만
            "A".repeat(32),         // 대문자
            "g".repeat(32),         // hex 가 아님
            "a".repeat(32),         // 유효한 값 하나
        ]);
        assert_eq!(m.issued_tokens(), vec!["a".repeat(32)],
                   "형식이 틀린 항목(손상된 설정 파일)은 조용히 버려야 한다");
        let r = m.handle(&id("A"), AuthRequest::Token(String::new()), t(1000));
        assert!(matches!(r, AuthReply::Rejected), "빈 토큰이 인증 우회가 되면 안 된다");
    }

    #[test]
    fn issued_tokens_are_persistable() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        assert!(m.issued_tokens().contains(&token));
    }

    #[test]
    fn different_pairing_windows_get_different_codes() {
        let mut m = PairingManager::new();
        let a = m.begin_pairing(t(1000));
        let b = m.begin_pairing(t(1001));
        assert_ne!(a, b, "코드가 매번 달라야 한다 — 예측 가능하면 무차별 대입에 취약해진다");
    }

    #[test]
    fn two_pairings_produce_distinct_tokens() {
        let mut m = PairingManager::new();
        let code1 = m.begin_pairing(t(1000));
        let AuthReply::Granted { token: tok1 } = m.handle(&id("A"), AuthRequest::Code(code1), t(1001)) else {
            panic!()
        };
        let code2 = m.begin_pairing(t(2000));
        let AuthReply::Granted { token: tok2 } = m.handle(&id("B"), AuthRequest::Code(code2), t(2001)) else {
            panic!()
        };
        assert_ne!(tok1, tok2, "기기마다 다른 토큰을 받아야 한다 — 같으면 모든 기기가 자격증명을 공유하게 된다");
        assert_eq!(m.issued_tokens().len(), 2);
    }

    #[test]
    fn state_reflects_awaiting_code_then_authorized() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        assert_eq!(m.state(&id("A"), t(1000)), AuthState::Unauthorized);
        m.handle(&id("A"), AuthRequest::Hello, t(1000));
        assert_eq!(m.state(&id("A"), t(1000)), AuthState::AwaitingCode);
        m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        assert_eq!(m.state(&id("A"), t(1001)), AuthState::Authorized);
    }
}
