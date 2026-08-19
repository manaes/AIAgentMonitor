//! 페어링 인증 상태 기계 (스펙 5.1).
//!
//! 이 모듈은 순수하다 — CoreBluetooth 도 모르고 설정 파일도 읽거나 쓰지 않는다.
//! (유일한 예외: 암호학적 난수를 위해 `/dev/urandom` 을 직접 읽는다 — 이
//! 크레이트에는 CSPRNG 가 없다.) 그래야 코드 만료, 시도 제한, 토큰 재사용,
//! 논스 재생 같은 보안 성질을 하드웨어 없이 검증할 수 있다.
//!
//! ## 초기 페어링: 6자리 코드
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
//!
//! ## 재연결: 논스 챌린지-응답
//!
//! 128비트 토큰을 재연결마다 평문으로 그대로 보내면, 근접 스니핑 한 번으로
//! 영구 접근권이 넘어간다. 그래서 재연결은 `AUTH` → 128비트 논스 발급 →
//! `PROOF:<hex>`(= HMAC-SHA256(key=토큰, msg=논스)) 검증으로 바뀌었다.
//! 도청자는 논스와 서명만 보므로 토큰을 복원할 수 없고, 논스는 30초
//! 유효·1회용이라(검증 성공/실패와 무관하게 즉시 폐기) 재생 공격도
//! 막힌다. HMAC 비교는 `hmac::Mac::verify_slice` 를 쓴다 — 이 크레이트가
//! 이미 상수 시간 비교를 구현하므로(RustCrypto `hmac`), 별도로 `subtle` 을
//! 끌어오지 않았다.
use crate::ble::peripheral::CentralId;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

type HmacSha256 = Hmac<Sha256>;

pub const CODE_TTL: Duration = Duration::from_secs(120);
pub const MAX_ATTEMPTS: u8 = 5;
/// 재연결 논스의 수명. 코드보다 훨씬 짧다 — 논스는 연결할 때마다 매번
/// 새로 받아 즉시 쓰는 값이라, 사용자가 화면을 보고 옮겨 적는 코드처럼
/// 여유 시간이 필요 없다.
pub const NONCE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthRequest {
    Hello,
    Code(String),
    /// 재연결 시작. 논스를 요청한다.
    Auth,
    /// `AUTH` 로 받은 논스에 대한 HMAC-SHA256 서명(hex).
    Proof(String),
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthReply {
    /// 창이 열려 있고 이 central 에 바인딩됐다는 응답. 코드 자체는 담지
    /// 않는다 — 코드는 오직 `begin_pairing` 을 통해 로컬(Mac UI)로만 전달된다.
    AwaitingCode,
    /// `CODE:` 성공 시에만 쓴다 — 새로 발급된 토큰을 실제로 전달해야 하는
    /// 유일한 경로이기 때문이다.
    Granted { token: String },
    Denied { left: u8 },
    Rejected,
    /// `AUTH` 응답. 클라이언트는 이 논스를 저장된 토큰으로 서명해
    /// `PROOF:` 로 되돌려보내야 한다.
    Nonce { nonce: String },
    /// `PROOF:` 성공. 클라이언트가 이미 아는 토큰을 그대로 검증했을
    /// 뿐이므로, 되돌려보낼 비밀이 없다 — 그래서 아무 필드도 없다.
    Authorized,
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
    if s == "AUTH" {
        return AuthRequest::Auth;
    }
    if let Some(rest) = s.strip_prefix("CODE:") {
        return AuthRequest::Code(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("PROOF:") {
        return AuthRequest::Proof(rest.to_string());
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

#[derive(Debug)]
struct PendingNonce {
    nonce: String,
    issued_at: SystemTime,
}

#[derive(Debug, Default)]
pub struct PairingManager {
    pending: Option<PendingCode>,
    tokens: HashSet<String>,
    authorized: HashSet<String>,
    /// central id → 그 central 이 마지막 `AUTH` 로 받은 논스. central 마다
    /// 따로 두는 이유는 여러 기기가 동시에 재연결을 시도할 수 있어서다.
    /// 새 `AUTH` 는 이전 논스를 덮어써 무효화한다.
    nonces: HashMap<String, PendingNonce>,
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
            .extend(tokens.into_iter().filter(|t| Self::is_valid_lowercase_hex(t, 32)));
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
    /// 같은 토큰으로 다시 재연결(`AUTH`/`PROOF:`)하면 즉시 재인가된다 —
    /// 완전한 언페어링(토큰 폐기)이 필요하면 `revoke_token`/`revoke_all` 을
    /// 함께 호출해야 한다.
    ///
    /// 호출자(BLE 브리지)는 central 의 연결이 끊어질 때 반드시 이를
    /// 호출해야 한다 — 그러지 않으면 인가 상태가 실제 연결보다 오래
    /// 살아남는다.
    pub fn end_session(&mut self, id: &CentralId) {
        self.authorized.remove(&id.0);
    }

    /// 토큰 하나를 완전히 폐기한다 — 이후 이 토큰으로의 재연결 인증은
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
                    let token = Self::random_hex128();
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
            AuthRequest::Auth => {
                // 새 논스는 이전 논스를 덮어써 무효화한다 — 코드와 마찬가지로
                // 동시에 두 논스가 살아 있을 이유가 없다.
                let nonce = Self::random_hex128();
                self.nonces.insert(
                    id.0.clone(),
                    PendingNonce {
                        nonce: nonce.clone(),
                        issued_at: now,
                    },
                );
                AuthReply::Nonce { nonce }
            }
            AuthRequest::Proof(given) => {
                // 검증 성공/실패와 무관하게 즉시 폐기한다 — 그래야 같은
                // 응답을 재생해도 두 번째부터는 통하지 않는다(1회용 논스).
                let Some(pending) = self.nonces.remove(&id.0) else {
                    // AUTH 없이 곧바로 온 PROOF, 혹은 이미 소비된 논스.
                    return AuthReply::Rejected;
                };
                if !Self::is_valid_lowercase_hex(&given, 64) {
                    // HMAC-SHA256 출력은 정확히 64자 소문자 hex 다. 형식이
                    // 다르면 굳이 디코드를 시도하지 않고 거부한다 — 토큰과
                    // 같은 기준을 적용한다.
                    return AuthReply::Rejected;
                }
                if now.duration_since(pending.issued_at).unwrap_or_default() > NONCE_TTL {
                    return AuthReply::Rejected;
                }
                let ok = self
                    .tokens
                    .iter()
                    .any(|token| Self::verify_proof(token, &pending.nonce, &given));
                if ok {
                    self.authorized.insert(id.0.clone());
                    AuthReply::Authorized
                } else {
                    AuthReply::Rejected
                }
            }
            AuthRequest::Malformed => AuthReply::Rejected,
        }
    }

    fn is_valid_lowercase_hex(s: &str, len: usize) -> bool {
        s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }

    /// HMAC-SHA256(key=token, msg=nonce) 를 계산해 클라이언트가 보낸
    /// 서명과 비교한다. 키·메시지 모두 hex 로 인코딩된 원본 바이트로
    /// 디코드한 뒤 계산한다(iOS 쪽도 동일하게 raw bytes 로 계산해야
    /// 한다 — 3단계 배선 작업에서 맞춰야 하는 지점).
    ///
    /// `Mac::verify_slice` 는 상수 시간으로 비교한다(RustCrypto `hmac`
    /// 크레이트 내부 구현) — 타이밍으로 서명 바이트를 하나씩 유추하는
    /// 사이드 채널을 막는다.
    fn verify_proof(token_hex: &str, nonce_hex: &str, given_hex: &str) -> bool {
        let (Some(token_bytes), Some(nonce_bytes), Some(given_bytes)) = (
            Self::hex_decode(token_hex),
            Self::hex_decode(nonce_hex),
            Self::hex_decode(given_hex),
        ) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(&token_bytes) else {
            return false;
        };
        mac.update(&nonce_bytes);
        mac.verify_slice(&given_bytes).is_ok()
    }

    fn hex_decode(s: &str) -> Option<Vec<u8>> {
        if s.len() % 2 != 0 {
            return None;
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
            .collect()
    }

    fn random_code() -> String {
        format!("{:06}", Self::random_u64() % 1_000_000)
    }

    /// 128비트 난수를 32자 소문자 hex 로. 토큰과 논스가 공유하는 형식이다.
    fn random_hex128() -> String {
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

    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 테스트에서 "iOS 클라이언트" 역할을 대신한다 — 토큰으로 논스에
    /// 서명해 올바른 PROOF 값을 만든다.
    fn compute_proof(token: &str, nonce: &str) -> String {
        let token_bytes = PairingManager::hex_decode(token).expect("토큰은 유효한 hex 다");
        let nonce_bytes = PairingManager::hex_decode(nonce).expect("논스는 유효한 hex 다");
        let mut mac = HmacSha256::new_from_slice(&token_bytes).expect("HMAC 키 길이 오류 없음");
        mac.update(&nonce_bytes);
        hex_encode(&mac.finalize().into_bytes())
    }

    #[test]
    fn parses_each_request_form() {
        assert!(matches!(parse_auth_request(b"HELLO"), AuthRequest::Hello));
        assert!(matches!(parse_auth_request(b"AUTH"), AuthRequest::Auth));
        assert!(matches!(parse_auth_request(b"CODE:123456"), AuthRequest::Code(c) if c == "123456"));
        assert!(matches!(parse_auth_request(b"PROOF:abcdef"), AuthRequest::Proof(p) if p == "abcdef"));
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
    fn unknown_token_is_rejected() {
        let mut m = PairingManager::new();
        // 페어링한 적이 없어 저장된 토큰이 하나도 없다. AUTH 는 그래도
        // 논스를 내주지만(논스 자체는 비밀이 아니다), 어떤 값으로도
        // 서명을 맞출 수 없다.
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1000)) else {
            panic!()
        };
        let bogus_proof = compute_proof(&"0".repeat(32), &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(bogus_proof), t(1000));
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

        // 토큰 자체는 살아있다 — 같은 토큰으로 다시 재연결하면 즉시
        // 재인가된다. 완전한 폐기는 revoke_token 이 담당한다.
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        let proof = compute_proof(&token, &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof), t(1003));
        assert!(matches!(r, AuthReply::Authorized));
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

        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        let proof = compute_proof(&token, &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof), t(1003));
        assert!(matches!(r, AuthReply::Rejected), "폐기된 토큰은 더 이상 통하지 않는다");
    }

    #[test]
    fn revoke_all_clears_every_stored_token() {
        let mut m = PairingManager::new();
        let tok = "a".repeat(32);
        m.load_tokens(vec![tok.clone(), "b".repeat(32)]);
        m.revoke_all();
        assert!(m.issued_tokens().is_empty());

        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1000)) else {
            panic!()
        };
        let proof = compute_proof(&tok, &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof), t(1001));
        assert!(matches!(r, AuthReply::Rejected));
    }

    #[test]
    fn loaded_token_authorizes_via_challenge_response() {
        let mut m = PairingManager::new();
        let token = "a".repeat(32);
        m.load_tokens(vec![token.clone()]);

        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1000)) else {
            panic!()
        };
        let proof = compute_proof(&token, &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof), t(1001));
        assert!(matches!(r, AuthReply::Authorized), "저장된 토큰은 앱 재시작 후에도 통한다");
        assert!(m.is_authorized(&id("A")));
    }

    #[test]
    fn load_tokens_drops_malformed_entries() {
        let mut m = PairingManager::new();
        let valid = "a".repeat(32);
        m.load_tokens(vec![
            "".to_string(),      // 빈 문자열
            "short".to_string(), // 32자 미만
            "A".repeat(32),      // 대문자
            "g".repeat(32),      // hex 가 아님
            valid.clone(),       // 유효한 값 하나
        ]);
        assert_eq!(m.issued_tokens(), vec![valid.clone()],
                   "형식이 틀린 항목(손상된 설정 파일)은 조용히 버려야 한다");

        // 버려진 빈 문자열이 몰래 유효한 키로 취급되지 않는지 확인한다.
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1000)) else {
            panic!()
        };
        let proof_from_empty_key = compute_proof("", &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof_from_empty_key), t(1000));
        assert!(matches!(r, AuthReply::Rejected), "손상된 설정 파일이 인증 우회가 되면 안 된다");
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

    // ---- 논스 챌린지-응답 ----

    #[test]
    fn correct_proof_authorizes() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        let AuthReply::Nonce { nonce } = m.handle(&id("B"), AuthRequest::Auth, t(2000)) else {
            panic!("AUTH 는 항상 논스를 내준다")
        };
        let proof = compute_proof(&token, &nonce);
        let r = m.handle(&id("B"), AuthRequest::Proof(proof), t(2001));
        assert_eq!(r, AuthReply::Authorized, "이미 아는 자격증명을 되돌려보낼 필요가 없다");
        assert!(m.is_authorized(&id("B")));
    }

    #[test]
    fn wrong_proof_does_not_authorize() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        // 재연결은 새 CentralId 로 온다 — A 는 이미 정당하게 인가됐으므로,
        // 확인할 대상은 이 새 연결(B)이 잘못된 서명으로 인가되지 않는지다.
        let AuthReply::Nonce { nonce } = m.handle(&id("B"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        // 전혀 다른 키로 서명한 값.
        let wrong = compute_proof(&"f".repeat(32), &nonce);
        let r = m.handle(&id("B"), AuthRequest::Proof(wrong), t(1003));
        assert!(matches!(r, AuthReply::Rejected));
        assert!(!m.is_authorized(&id("B")));
    }

    #[test]
    fn same_proof_replayed_against_same_nonce_fails() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        let proof = compute_proof(&token, &nonce);

        let first = m.handle(&id("A"), AuthRequest::Proof(proof.clone()), t(1003));
        assert!(matches!(first, AuthReply::Authorized));

        m.end_session(&id("A")); // 세션만 끊는다 — 도청자가 같은 응답을 재생하는 상황을 흉내낸다.
        let replay = m.handle(&id("A"), AuthRequest::Proof(proof), t(1004));
        assert!(matches!(replay, AuthReply::Rejected), "논스는 1회용이다 — 같은 응답을 재생해도 통하면 안 된다");
    }

    #[test]
    fn proof_against_a_different_nonce_fails() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        let AuthReply::Nonce { nonce: nonce_a } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        let AuthReply::Nonce { nonce: nonce_b } = m.handle(&id("B"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        assert_ne!(nonce_a, nonce_b);

        // A 에게 온 논스가 아니라 B 의 논스로 서명한 값을 A 로 제출한다.
        let proof_for_wrong_nonce = compute_proof(&token, &nonce_b);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof_for_wrong_nonce), t(1003));
        assert!(matches!(r, AuthReply::Rejected));
    }

    #[test]
    fn expired_nonce_fails() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(2000)) else {
            panic!()
        };
        let proof = compute_proof(&token, &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof), t(2000 + 31));
        assert!(matches!(r, AuthReply::Rejected), "30초를 넘기면 논스가 만료된다");
    }

    #[test]
    fn proof_without_preceding_auth_fails() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        // AUTH 를 한 번도 보내지 않고 곧바로 PROOF 를 보낸다. 값 자체는
        // (우연히) 유효한 서명 형식일 수 있지만, 검증할 논스가 없다.
        let bogus = compute_proof(&token, &"0".repeat(32));
        let r = m.handle(&id("A"), AuthRequest::Proof(bogus), t(1002));
        assert!(matches!(r, AuthReply::Rejected));
    }

    #[test]
    fn malformed_proof_format_is_rejected() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Code(code), t(1001));

        // 논스가 1회용이라 제출마다 새로 받아야 한다 — 그래야 각 형식
        // 오류가 "AUTH 를 안 보냈다"가 아니라 실제로 형식 검증에 걸린다.
        for bogus in ["", &"F".repeat(64), "not-hex", "abcd"] {
            let reply = m.handle(&id("A"), AuthRequest::Auth, t(1002));
            assert!(matches!(reply, AuthReply::Nonce { .. }));
            let r = m.handle(&id("A"), AuthRequest::Proof(bogus.to_string()), t(1002));
            assert!(matches!(r, AuthReply::Rejected), "형식이 틀린 값({bogus:?})은 느슨하게 봐주지 않는다");
        }
    }

    #[test]
    fn nonce_differs_between_two_auth_requests() {
        let mut m = PairingManager::new();
        let AuthReply::Nonce { nonce: first } = m.handle(&id("A"), AuthRequest::Auth, t(1000)) else {
            panic!()
        };
        let AuthReply::Nonce { nonce: second } = m.handle(&id("A"), AuthRequest::Auth, t(1001)) else {
            panic!()
        };
        assert_ne!(first, second, "매 AUTH 마다 새 논스를 내야 한다 — 예측 가능하면 재생 공격에 취약해진다");
    }

    /// Swift 재인증 모듈과 공유하는 골든 벡터.
    ///
    /// 이 값을 고정하는 이유: HMAC 입력이 "hex 문자열의 UTF-8 바이트"인지
    /// "hex 를 디코드한 raw bytes"인지가 Rust/Swift 양쪽에서 프로즈로만
    /// 합의되면 조용히 어긋날 수 있다. 어긋나면 페어링은 성공하고 토큰도
    /// 저장되지만, 이후 모든 재연결이 이유 없이 실패한다 — 로그가 없는
    /// 앱이라 원인을 알 방법이 없다. token/nonce 는 길이·내용이 뚜렷이
    /// 다른 고정값이다(전부 0 같은 값은 잘못된 해석 여러 개가 우연히
    /// 일치할 수 있어 피한다).
    ///
    /// 갱신: UPDATE_GOLDEN=1 cargo test --manifest-path src-tauri/Cargo.toml ble::pairing::tests::golden
    #[test]
    fn golden_hmac_vector_matches() {
        use std::path::PathBuf;

        let token = "3f14a9c2e5b6d8710f2a4c6e8b1d3f50";
        let nonce = "7ac4e19b2d5f8067c3a1e9d4b6f02358";
        let proof = compute_proof(token, nonce);

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/ble-protocol/golden/hmac-sample.json");
        let actual = serde_json::json!({
            "token": token,
            "nonce": nonce,
            "proof": proof,
            "note": "proof = lowercase-hex(HMAC-SHA256(key = raw bytes decoded from token hex, msg = raw bytes decoded from nonce hex)). NOT the UTF-8 bytes of the hex strings themselves.",
        });

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
            return;
        }
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect(
                "골든 벡터가 없다. UPDATE_GOLDEN=1 로 생성하고 커밋하라",
            ))
            .unwrap();
        assert_eq!(actual, expected, "HMAC 입력 인코딩이 골든 벡터와 어긋났다");
    }
}
