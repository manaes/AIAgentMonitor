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
//! **창에는 소유자가 없다.** 창이 열려 있는 동안에는 어느 central 이든
//! `CODE:` 를 제출할 수 있고, 시도 5회는 창 전체가 공유한다. 첫
//! `HELLO`/`CODE` 를 보낸 central 에게 창을 묶는 설계도 검토했으나 채택하지
//! 않았다 — 그러면 근처 공격자가 `HELLO` 한 번으로 시도를 단 1회도 쓰지
//! 않고 창을 조용히 120초 동안 죽일 수 있고, 사람이 코드를 입력하는
//! 속도로는 그 재선점 경쟁에서 이길 수 없다(스펙 5.1/5.2).
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
use std::collections::HashMap;
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
    /// 창 전체가 공유하는 시도 예산. 특정 central 것이 아니다 — 창에는
    /// 소유자가 없다(스펙 5.1).
    attempts_left: u8,
}

#[derive(Debug)]
struct PendingNonce {
    nonce: String,
    issued_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedPeer {
    pub peer_id: String,
    pub paired_at: u64,
    /// 지금 이 기기가 붙어서 인가된 상태인가. `authorized` 값(토큰)으로 판정한다.
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingWindow {
    /// 창이 열려 있다. UI 는 코드와 남은 초를 그린다.
    Open { code: String, seconds_left: u64, attempts_left: u8 },
    /// 시도 5회가 모두 소진돼 닫혔다. 방해일 수 있으므로 만료와 구분해 보여준다.
    Exhausted,
    /// 열린 적 없거나 시간이 지나 닫혔다.
    Closed,
}

#[derive(Debug, Default)]
pub struct PairingManager {
    pending: Option<PendingCode>,
    /// 토큰 → 페어링된 시각(unix secs). 토큰이 곧 기기 정체성이다 —
    /// `CBCentral.identifier` 는 iOS 가 프라이버시를 위해 BLE 주소를 주기적으로
    /// 바꾸므로 재연결 사이에 안정적이지 않아 쓸 수 없다(스펙 6장).
    tokens: HashMap<String, u64>,
    /// central id → 그 central 을 인가시킨 토큰. 어떤 토큰이 어떤 세션을
    /// 열었는지 기록해 둬야 `revoke_token`/`revoke_all` 이 살아 있는
    /// 세션까지 내릴 수 있다(스펙 5.1: "토큰 폐기는 살아 있는 세션까지
    /// 닿는다").
    authorized: HashMap<String, String>,
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
    ///
    /// **이 함수는 사용자의 명시적 제스처(Devices 탭 [페어링 시작] 클릭)에서만
    /// 호출한다.** 연결·구독·`HELLO`·앱 재시작 등 어떤 자동 경로에서도
    /// 호출해서는 안 된다. 6자리 코드의 무차별 대입 방어(스펙 5.2)가
    /// 전적으로 이 성질에 의존한다.
    pub fn begin_pairing(&mut self, now: SystemTime) -> String {
        let code = Self::random_code();
        self.pending = Some(PendingCode {
            code: code.clone(),
            issued_at: now,
            attempts_left: MAX_ATTEMPTS,
        });
        code
    }

    /// 영속화된 페어링을 복원한다. 형식(32자 소문자 hex)에 맞지 않는 항목은
    /// 조용히 버린다 — 파일이 손상돼도 인증 우회로 이어지지 않게 한다.
    pub fn load_peers(&mut self, peers: Vec<(String, u64)>) {
        for (token, paired_at) in peers {
            if Self::is_valid_lowercase_hex(&token, 32) {
                self.tokens.insert(token, paired_at);
            }
        }
    }

    /// 저장할 (토큰, 페어링 시각) 목록. 토큰 순 정렬(결정적 출력).
    pub fn issued_peers(&self) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = self.tokens.iter().map(|(t, &p)| (t.clone(), p)).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// 화면에 그릴 기기 목록. peer_id 순 정렬. 토큰은 포함하지 않는다.
    pub fn paired_peers(&self) -> Vec<PairedPeer> {
        let mut v: Vec<PairedPeer> = self
            .tokens
            .iter()
            .map(|(token, &paired_at)| PairedPeer {
                peer_id: Self::peer_id_of(token),
                paired_at,
                connected: self.authorized.values().any(|t| t == token),
            })
            .collect();
        v.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        v
    }

    /// 토큰에서 기기 id 를 파생한다. 토큰당 하나로 안정적이며, 프론트엔드에는
    /// 이 값만 나간다 — 토큰(영구 자격증명)은 이 모듈 밖으로 나가지 않는다.
    fn peer_id_of(token: &str) -> String {
        use sha2::Digest;
        let digest = Sha256::digest(token.as_bytes());
        digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }

    pub fn is_authorized(&self, id: &CentralId) -> bool {
        self.authorized.contains_key(&id.0)
    }

    /// 이 central 의 세션 인가와, 그 central 이 갖고 있던 미사용 논스를
    /// 지운다(연결이 끊겼을 때, 또는 사용자가 세션만 끊고 싶을 때 호출).
    /// 저장된 토큰 자체는 지우지 않으므로, 같은 토큰으로 다시
    /// 재연결(`AUTH`/`PROOF:`)하면 즉시 재인가된다 — 완전한
    /// 언페어링(토큰 폐기)이 필요하면 `revoke_token`/`revoke_all` 을 함께
    /// 호출해야 한다.
    ///
    /// 호출자(BLE 브리지)는 central 의 연결이 끊어질 때 반드시 이를
    /// 호출해야 한다 — 그러지 않으면 인가 상태가 실제 연결보다 오래
    /// 살아남는다.
    pub fn end_session(&mut self, id: &CentralId) {
        self.authorized.remove(&id.0);
        self.nonces.remove(&id.0);
    }

    /// 토큰 하나를 완전히 폐기한다 — 이후 이 토큰으로의 재연결 인증은
    /// 거부된다. 저장된 토큰만 지우는 게 아니라, **그 토큰으로 이미
    /// 인가된 살아 있는 세션도 함께 내린다** — 언페어링을 눌러도 화면
    /// 미러링이 계속되면 언페어링이 아니다. 내려간 central id 를 반환하니
    /// 호출자(BLE 브리지)가 그 연결을 실제로 끊어야 한다(스펙 6
    /// `ble_unpair`).
    fn revoke_token(&mut self, token: &str) -> Vec<CentralId> {
        self.tokens.remove(token);
        let dropped: Vec<String> = self
            .authorized
            .iter()
            .filter(|(_, t)| t.as_str() == token)
            .map(|(cid, _)| cid.clone())
            .collect();
        for cid in &dropped {
            self.authorized.remove(cid);
        }
        dropped.into_iter().map(CentralId).collect()
    }

    /// peer_id 하나에 해당하는 토큰을 폐기하고, 그 토큰으로 인가돼 있던
    /// central 들의 세션 인가도 내린다. 내려간 central 을 돌려준다.
    /// 없는 peer_id 면 빈 벡터.
    pub fn revoke_peer(&mut self, peer_id: &str) -> Vec<CentralId> {
        let token = self
            .tokens
            .keys()
            .find(|t| Self::peer_id_of(t) == peer_id)
            .cloned();
        match token {
            Some(t) => self.revoke_token(&t),
            None => Vec::new(),
        }
    }

    /// 저장된 토큰을 모두 폐기한다(전체 언페어링). `revoke_token` 과 같은
    /// 이유로, 살아 있는 세션 전부를 내리고 그 central id 를 반환한다.
    pub fn revoke_all(&mut self) -> Vec<CentralId> {
        self.tokens.clear();
        let dropped: Vec<String> = self.authorized.keys().cloned().collect();
        self.authorized.clear();
        dropped.into_iter().map(CentralId).collect()
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
    /// 실제 상태에서 도출한다. 창에는 소유자가 없으므로, 창이 열려 있는
    /// 동안에는 (아직 인가되지 않은) 어느 central 이든 `AwaitingCode` 다.
    pub fn state(&self, id: &CentralId, now: SystemTime) -> AuthState {
        if self.is_authorized(id) {
            return AuthState::Authorized;
        }
        if let Some(p) = &self.pending {
            if !Self::expired(p, now) {
                return AuthState::AwaitingCode;
            }
        }
        AuthState::Unauthorized
    }

    fn expired(p: &PendingCode, now: SystemTime) -> bool {
        now.duration_since(p.issued_at).unwrap_or_default() > CODE_TTL
    }

    /// 창이 열려 있고 아직 시도 예산이 남아 있으면 그 창을 돌려준다. 창에는
    /// 소유자가 없다 — 어느 central 이든 열린 창에 `CODE:` 를 제출할 수
    /// 있다(스펙 5.1). 만료됐으면 창을 닫고 None. 예산이 소진됐으면(TTL 은
    /// 아직 안 지났어도) `pending` 자체는 남겨두되(그래야 `pairing_window` 가
    /// `Exhausted` 를 보여줄 수 있다) None 을 돌려줘 더 이상 통과시키지 않는다.
    fn open_window(&mut self, now: SystemTime) -> Option<&mut PendingCode> {
        if matches!(&self.pending, Some(p) if Self::expired(p, now)) {
            self.pending = None;
        }
        let usable = matches!(&self.pending, Some(p) if p.attempts_left > 0);
        if usable {
            self.pending.as_mut()
        } else {
            None
        }
    }

    /// 페어링 창의 현재 상태를 UI 가 그릴 수 있는 형태로 노출한다. 만료와
    /// 시도 소진을 구분하는 이유: 창에 소유자가 없다는 설계는 "방해하면 시도
    /// 5회가 들고 그 소진이 화면에 보인다"는 전제에 의존한다(스펙 5.1/5.2) —
    /// 그 전제가 성립하려면 소진 상태가 단순 만료와 다르게 보여야 한다.
    pub fn pairing_window(&self, now: SystemTime) -> PairingWindow {
        let Some(p) = &self.pending else {
            return PairingWindow::Closed;
        };
        if Self::expired(p, now) {
            return PairingWindow::Closed;
        }
        if p.attempts_left == 0 {
            return PairingWindow::Exhausted;
        }
        let elapsed = now.duration_since(p.issued_at).unwrap_or_default();
        let seconds_left = CODE_TTL.saturating_sub(elapsed).as_secs();
        PairingWindow::Open {
            code: p.code.clone(),
            seconds_left,
            attempts_left: p.attempts_left,
        }
    }

    /// 만료된 논스를 청소한다. `AUTH` 만 보내고 사라지는 central 이 계속
    /// 쌓이면 원격에서 키우는 메모리 누수가 되므로, 모든 요청 처리
    /// 시점마다 훑는다.
    fn sweep_expired_nonces(&mut self, now: SystemTime) {
        self.nonces
            .retain(|_, n| now.duration_since(n.issued_at).unwrap_or_default() <= NONCE_TTL);
    }

    #[cfg(test)]
    fn nonce_count(&self) -> usize {
        self.nonces.len()
    }

    pub fn handle(&mut self, id: &CentralId, req: AuthRequest, now: SystemTime) -> AuthReply {
        self.sweep_expired_nonces(now);
        match req {
            AuthRequest::Hello => match self.open_window(now) {
                Some(_) => AuthReply::AwaitingCode,
                None => AuthReply::Rejected,
            },
            AuthRequest::Code(given) => {
                let Some(p) = self.open_window(now) else {
                    return AuthReply::Rejected;
                };
                if given == p.code {
                    let token = Self::random_hex128();
                    let paired_at = now
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    self.tokens.insert(token.clone(), paired_at);
                    self.authorized.insert(id.0.clone(), token.clone());
                    self.pending = None;
                    AuthReply::Granted { token }
                } else {
                    p.attempts_left -= 1;
                    let left = p.attempts_left;
                    // 예산이 소진돼도 `pending` 자체는 남겨둔다(즉시 지우지
                    // 않는다) — `pairing_window` 가 TTL 내에는 `Exhausted` 로,
                    // TTL 이 지나야 `Closed` 로 구분해 보여줘야 하기 때문이다.
                    // `open_window` 가 attempts_left == 0 인 창을 이미
                    // "사용 불가"로 취급하므로, HELLO/CODE 어느 쪽으로도 이
                    // 창은 더 이상 통과되지 않는다.
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
                // 만료 검사는 없다 — handle() 진입 시 sweep_expired_nonces 가
                // 이미 이 `now` 기준으로 지난 논스를 전부 제거했으므로,
                // 여기까지 남아 있다면 반드시 유효하다.
                let matched = self
                    .tokens
                    .keys()
                    .find(|token| Self::verify_proof(token, &pending.nonce, &given))
                    .cloned();
                if let Some(token) = matched {
                    self.authorized.insert(id.0.clone(), token);
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

    /// hex 문자열을 바이트로 디코드한다. **바이트 단위로만 동작한다** —
    /// `str` 을 문자 경계 기준으로 슬라이싱(`&s[i..i+2]`)하지 않는다.
    /// 그렇게 슬라이싱하면 멀티바이트 UTF-8 입력(예: `PROOF:한글…`)이
    /// char 경계를 침범해 패닉한다 — 원격에서 아무나 그 값을 보내
    /// Mac 앱을 죽일 수 있다는 뜻이다. `is_valid_lowercase_hex` 같은
    /// 앞단 형식 검사가 대부분의 경우 이 함수에 닿기 전에 걸러내지만,
    /// 방어를 검사 하나에 의존시키지 않는다 — 이 함수 자체가 어떤
    /// 입력에도 패닉하지 않아야 한다.
    fn hex_decode(s: &str) -> Option<Vec<u8>> {
        let bytes = s.as_bytes();
        if bytes.len() % 2 != 0 {
            return None;
        }
        bytes
            .chunks_exact(2)
            .map(|pair| {
                let hi = Self::hex_nibble(pair[0])?;
                let lo = Self::hex_nibble(pair[1])?;
                Some((hi << 4) | lo)
            })
            .collect()
    }

    fn hex_nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
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
    fn code_without_prior_hello_still_grants() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        // HELLO 를 생략하고 바로 CODE 를 보내도 통과해야 한다 — 창에는
        // 소유자가 없으므로 HELLO 를 먼저 보낼 필요가 없다.
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

    /// F1 재검토 후 계약이 뒤집힌 테스트: 예산이 코드 단위였을 때는 "다른
    /// central 의 추측은 내 예산을 깎지 않는다" 였지만, 예산이 창 단위로
    /// 바뀌면서(그리고 소유자 바인딩이 사라지면서) "창은 공유되므로 누구의
    /// 추측이든 같은 예산을 깎는다"로 바뀌었다. 소유자를 남겨뒀다면 근처
    /// 공격자가 `HELLO` 한 번으로 시도를 단 1회도 쓰지 않고 창을 조용히
    /// 120초 동안 죽일 수 있었다 — 그 방어가 이 계약 전환의 이유다.
    #[test]
    fn window_budget_is_shared_across_centrals() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        // 서로 다른 5개 central 이 각각 한 번씩 틀린 코드를 넣는다.
        for (idx, expected_left) in [4u8, 3, 2, 1, 0].into_iter().enumerate() {
            let guesser = id(&format!("C{idx}"));
            let r = m.handle(&guesser, AuthRequest::Code(wrong_code(&code)), t(1000));
            assert!(matches!(r, AuthReply::Denied { left } if left == expected_left),
                    "central 이 달라도 같은 창 예산을 공유해야 한다: {r:?}");
        }
        // 창이 소진됐다 — 이제는 올바른 코드를 내도(누가 냈든) 거부된다.
        let after = m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        assert!(matches!(after, AuthReply::Rejected), "예산이 소진되면 창은 완전히 닫힌다");
    }

    /// F1 회귀 테스트: 소유자 바인딩이 있던 시절엔 근처 공격자가 `HELLO`
    /// 한 번 보내고 사라지는 것만으로 창을 120초 동안 "공짜로" 잠글 수
    /// 있었다(시도를 한 번도 안 쓰고). 소유자를 없앤 뒤에는 그 잠금이
    /// 성립하지 않아야 한다.
    #[test]
    fn hello_does_not_reserve_the_window() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let hello = m.handle(&id("ATK"), AuthRequest::Hello, t(1000));
        assert!(matches!(hello, AuthReply::AwaitingCode));
        // ATK 는 그 뒤로 아무것도 보내지 않고 사라진다. A 가 곧바로 올바른
        // 코드를 제출하면 통과해야 한다 — ATK 의 HELLO 가 창을 선점하지
        // 않는다.
        let r = m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        assert!(matches!(r, AuthReply::Granted { .. }), "HELLO 는 창을 잠그지 않는다");
    }

    #[test]
    fn any_central_may_submit_the_code() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        // 공격자가 한 번 틀린 값을 넣는다.
        let r1 = m.handle(&id("ATK"), AuthRequest::Code(wrong_code(&code)), t(1000));
        assert!(matches!(r1, AuthReply::Denied { left: 4 }));
        // 다른 central 이 곧바로 올바른 코드를 내도 통과해야 한다.
        let r2 = m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        assert!(matches!(r2, AuthReply::Granted { .. }));
    }

    /// C1 회귀 테스트: 공격자가 `HELLO` 를 반복해 시도 예산을 리셋하려
    /// 해도, 창당(사용자가 연 창) 5회라는 총량을 절대 넘길 수 없어야
    /// 한다. 이 테스트가 이 모듈이 존재하는 이유다.
    #[test]
    fn attacker_cannot_reset_budget_by_repeating_hello() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));

        // 라운드 0: 공격자가 창이 공유하는 5회를 모두 소진한다.
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

    /// F3 재검토 후 강화된 테스트: 토큰 폐기는 저장된 토큰을 지우는 데서
    /// 끝나지 않는다 — 그 토큰으로 이미 인가된 살아 있는 세션도 즉시
    /// 내려야 한다. 그러지 않으면 사용자가 언페어링을 눌러도 화면
    /// 미러링이 계속된다. 내려간 central id 를 반환값으로 확인한다 —
    /// 호출자(BLE 브리지)가 실제로 연결을 끊으려면 그 id 가 필요하다.
    #[test]
    fn revoking_a_token_drops_its_live_session() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        assert!(m.is_authorized(&id("A")));

        let dropped = m.revoke_token(&token);
        assert!(!m.is_authorized(&id("A")), "토큰을 폐기하면 이미 인가된 세션도 즉시 내려가야 한다");
        assert_eq!(dropped, vec![id("A")], "내려간 central 을 호출자가 알아야 실제 연결을 끊을 수 있다");

        // 같은 토큰으로의 재인증도 당연히 거부된다.
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        let proof = compute_proof(&token, &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof), t(1003));
        assert!(matches!(r, AuthReply::Rejected), "폐기된 토큰은 더 이상 통하지 않는다");
    }

    #[test]
    fn revoking_a_token_leaves_other_sessions_alone() {
        let mut m = PairingManager::new();
        let code_a = m.begin_pairing(t(1000));
        let AuthReply::Granted { token: token_a } = m.handle(&id("A"), AuthRequest::Code(code_a), t(1001)) else {
            panic!()
        };
        let code_b = m.begin_pairing(t(2000));
        m.handle(&id("B"), AuthRequest::Code(code_b), t(2001));
        assert!(m.is_authorized(&id("A")));
        assert!(m.is_authorized(&id("B")));

        m.revoke_token(&token_a);
        assert!(!m.is_authorized(&id("A")));
        assert!(m.is_authorized(&id("B")), "다른 토큰으로 인가된 세션은 영향받지 않아야 한다");
    }

    #[test]
    fn revoke_all_drops_every_live_session() {
        let mut m = PairingManager::new();
        let code_a = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Code(code_a), t(1001));
        let code_b = m.begin_pairing(t(2000));
        m.handle(&id("B"), AuthRequest::Code(code_b), t(2001));
        assert!(m.is_authorized(&id("A")));
        assert!(m.is_authorized(&id("B")));

        let mut dropped = m.revoke_all();
        dropped.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(dropped, vec![id("A"), id("B")]);
        assert!(!m.is_authorized(&id("A")));
        assert!(!m.is_authorized(&id("B")));
    }

    #[test]
    fn revoke_all_clears_every_stored_token() {
        let mut m = PairingManager::new();
        let tok = "a".repeat(32);
        m.load_peers(vec![(tok.clone(), 1000), ("b".repeat(32), 1000)]);
        m.revoke_all();
        assert!(m.issued_peers().is_empty());

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
        m.load_peers(vec![(token.clone(), 1000)]);

        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1000)) else {
            panic!()
        };
        let proof = compute_proof(&token, &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof), t(1001));
        assert!(matches!(r, AuthReply::Authorized), "저장된 토큰은 앱 재시작 후에도 통한다");
        assert!(m.is_authorized(&id("A")));
    }

    #[test]
    fn load_peers_drops_malformed_entries() {
        let mut m = PairingManager::new();
        let valid = "a".repeat(32);
        m.load_peers(vec![
            ("".to_string(), 1000),      // 빈 문자열
            ("short".to_string(), 1000), // 32자 미만
            ("A".repeat(32), 1000),      // 대문자
            ("g".repeat(32), 1000),      // hex 가 아님
            (valid.clone(), 1000),       // 유효한 값 하나
        ]);
        assert_eq!(m.issued_peers(), vec![(valid.clone(), 1000)],
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
    fn issued_peers_are_persistable() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        assert!(m.issued_peers().iter().any(|(tok, _)| tok == &token));
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
        assert_eq!(m.issued_peers().len(), 2);
    }

    #[test]
    fn state_reflects_awaiting_code_then_authorized() {
        let mut m = PairingManager::new();
        assert_eq!(m.state(&id("A"), t(999)), AuthState::Unauthorized, "창이 없으면 미인가");
        let code = m.begin_pairing(t(1000));
        // 창에는 소유자가 없다 — A 가 아무것도 보내지 않아도, 창이 열려
        // 있는 동안에는 코드 대기 상태로 보인다.
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

    /// F2 회귀 테스트: `AUTH` 만 보내고 사라지는 central 이 계속 쌓이면
    /// `nonces` 맵이 무한히 자란다(원격에서 키우는 메모리 누수). 만료된
    /// 항목은 다음 `handle` 호출에서 청소돼야 한다.
    #[test]
    fn expired_nonces_are_swept() {
        let mut m = PairingManager::new();
        m.handle(&id("A"), AuthRequest::Auth, t(1000));
        m.handle(&id("B"), AuthRequest::Auth, t(1000));
        m.handle(&id("C"), AuthRequest::Auth, t(1000));
        assert_eq!(m.nonce_count(), 3);

        // 31초 뒤, 요청 종류와 무관하게(sweep 은 handle 진입 시 항상
        // 실행된다) 아무 요청이나 하나 처리하면 만료된 논스가 전부
        // 청소돼야 한다.
        m.handle(&id("D"), AuthRequest::Hello, t(1031));
        assert_eq!(m.nonce_count(), 0, "만료된 논스는 다음 handle 호출에서 청소돼야 한다");
    }

    #[test]
    fn end_session_drops_the_nonce() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        m.end_session(&id("A"));
        assert_eq!(m.nonce_count(), 0, "end_session 은 그 central 의 논스도 지워야 한다");

        // 토큰은 여전히 유효하고 proof 계산도 올바르지만, 논스 자체가
        // 지워졌으므로 거부돼야 한다 — end_session 이 논스를 지우지
        // 않았다면 이 제출은 통과했을 것이다.
        let proof = compute_proof(&token, &nonce);
        let r = m.handle(&id("A"), AuthRequest::Proof(proof), t(1003));
        assert!(matches!(r, AuthReply::Rejected));
    }

    /// F4-a 회귀 테스트: 논스는 검증 성공/실패와 무관하게 즉시 폐기돼야
    /// 한다. 성공했을 때만 버리도록 약화되면(뮤테이션으로 실증됨) 같은
    /// 논스에 틀린 PROOF 를 낸 뒤 맞는 PROOF 를 다시 낼 수 있게 된다.
    #[test]
    fn nonce_burns_on_a_failed_proof() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };

        let wrong = compute_proof(&"f".repeat(32), &nonce);
        let r1 = m.handle(&id("A"), AuthRequest::Proof(wrong), t(1003));
        assert!(matches!(r1, AuthReply::Rejected));

        // 같은 논스에 대해 이번엔 올바른 서명을 낸다 — 첫 시도에서 이미
        // 소진됐어야 하므로 이것도 거부돼야 한다.
        let correct = compute_proof(&token, &nonce);
        let r2 = m.handle(&id("A"), AuthRequest::Proof(correct), t(1004));
        assert!(matches!(r2, AuthReply::Rejected), "논스는 실패한 시도에서도 소진돼야 한다");
    }

    /// F4-b 회귀 테스트: `hex_decode` 는 어떤 입력에도 패닉해서는 안 된다.
    /// 멀티바이트 UTF-8, 빈 문자열, 홀수 길이, 대문자, 잘못된 길이를 모두
    /// 넣어 패닉 없이 거부되는지 확인한다. `AUTH` 를 먼저 받아 논스가
    /// 있는 상태에서 시도해야 형식 검사 뒤쪽 경로까지 실제로 닿는다.
    ///
    /// 이 테스트 자체는 `is_valid_lowercase_hex` 형식 검사가 앞에서 대부분
    /// 걸러내므로 통과가 쉽다 — `hex_decode` 자체의 안전성은 그 형식
    /// 검사를 일시적으로 지운 뮤테이션으로 별도 실증했다(보고서 참고).
    #[test]
    fn malformed_proof_never_panics() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Code(code), t(1001));

        // 3바이트 한글 음절 21개 + ASCII 1글자 = 64바이트. hex_decode 가
        // 2바이트 스텝으로 슬라이싱하면 한글의 3바이트 경계와 어긋나서,
        // 예전 구현(`&s[i..i+2]`)이라면 char 경계 위반으로 패닉했을
        // 입력이다.
        let multibyte = format!("{}{}", "가".repeat(21), "x");
        assert_eq!(multibyte.len(), 64);

        let cases = [
            multibyte,
            "".to_string(),
            "abc".to_string(),  // 홀수 길이
            "F".repeat(64),     // 대문자
            "a".repeat(65),     // 65자 — 홀수 길이이자 형식 위반
        ];

        for bogus in cases {
            let AuthReply::Nonce { .. } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
                panic!()
            };
            let r = m.handle(&id("A"), AuthRequest::Proof(bogus.clone()), t(1002));
            assert!(matches!(r, AuthReply::Rejected), "패닉하지 않고 거부해야 한다: {bogus:?}");
        }
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

    // ---- 기기 정체성 (peer_id) ----

    #[test]
    fn peer_id_is_stable_for_a_token() {
        let token_a = "a".repeat(32);
        let token_b = "b".repeat(32);
        assert_eq!(
            PairingManager::peer_id_of(&token_a),
            PairingManager::peer_id_of(&token_a),
            "같은 토큰은 항상 같은 peer_id 를 내야 한다"
        );
        assert_ne!(
            PairingManager::peer_id_of(&token_a),
            PairingManager::peer_id_of(&token_b),
            "다른 토큰은 다른 peer_id 를 내야 한다"
        );
    }

    #[test]
    fn peer_id_does_not_expose_the_token() {
        let token = "a".repeat(32);
        let peer_id = PairingManager::peer_id_of(&token);
        assert!(!token.contains(&peer_id), "peer_id 는 토큰의 부분 문자열이면 안 된다");
    }

    #[test]
    fn paired_peers_reports_connected_state() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1001)) else {
            panic!()
        };
        let peer_id = PairingManager::peer_id_of(&token);

        let peers = m.paired_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, peer_id);
        assert!(peers[0].connected, "방금 인가된 기기는 connected 여야 한다");

        m.end_session(&id("A"));
        let peers = m.paired_peers();
        assert_eq!(peers.len(), 1, "세션이 끊겨도 페어링 목록에는 남아 있어야 한다");
        assert!(!peers[0].connected, "세션이 끊기면 connected 는 false 여야 한다");
    }

    #[test]
    fn revoke_peer_drops_that_peer_only() {
        let mut m = PairingManager::new();
        let code_a = m.begin_pairing(t(1000));
        let AuthReply::Granted { token: token_a } = m.handle(&id("A"), AuthRequest::Code(code_a), t(1001)) else {
            panic!()
        };
        let code_b = m.begin_pairing(t(2000));
        let AuthReply::Granted { token: token_b } = m.handle(&id("B"), AuthRequest::Code(code_b), t(2001)) else {
            panic!()
        };
        let peer_id_a = PairingManager::peer_id_of(&token_a);
        let peer_id_b = PairingManager::peer_id_of(&token_b);

        let dropped = m.revoke_peer(&peer_id_a);
        assert_eq!(dropped, vec![id("A")]);
        assert!(!m.is_authorized(&id("A")));
        assert!(m.is_authorized(&id("B")), "다른 기기의 인가는 유지돼야 한다");

        let remaining = m.paired_peers();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].peer_id, peer_id_b, "폐기한 기기만 목록에서 사라져야 한다");
    }

    #[test]
    fn revoke_peer_with_unknown_id_is_a_noop() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Code(code), t(1001));
        assert_eq!(m.paired_peers().len(), 1);

        let dropped = m.revoke_peer("deadbeef");
        assert!(dropped.is_empty(), "없는 peer_id 는 아무것도 지우지 않는다");
        assert_eq!(m.paired_peers().len(), 1, "기존 페어링이 그대로 남아 있어야 한다");
        assert!(m.is_authorized(&id("A")));
    }

    // ---- 페어링 창 상태 ----

    #[test]
    fn window_reports_exhausted_separately_from_expiry() {
        let mut m = PairingManager::new();
        assert_eq!(m.pairing_window(t(999)), PairingWindow::Closed, "발급 전에는 닫힌 창");

        let code = m.begin_pairing(t(1000));
        match m.pairing_window(t(1000)) {
            PairingWindow::Open { code: c, attempts_left, .. } => {
                assert_eq!(c, code);
                assert_eq!(attempts_left, MAX_ATTEMPTS);
            }
            other => panic!("정상 창은 Open 이어야 한다: {other:?}"),
        }
        match m.pairing_window(t(1050)) {
            PairingWindow::Open { seconds_left, .. } => {
                assert!(seconds_left < CODE_TTL.as_secs(), "시간이 지나면 남은 초가 줄어야 한다");
            }
            other => panic!("아직 열려 있어야 한다: {other:?}"),
        }

        // 시도 5회를 모두 소진한다.
        for _ in 0..MAX_ATTEMPTS {
            m.handle(&id("ATK"), AuthRequest::Code(wrong_code(&code)), t(1000));
        }
        assert_eq!(
            m.pairing_window(t(1000)),
            PairingWindow::Exhausted,
            "소진 직후(TTL 안)에는 Exhausted 여야 한다"
        );

        // TTL 이 지나면 Exhausted 가 아니라 Closed 로 바뀌어야 한다.
        assert_eq!(
            m.pairing_window(t(1000) + CODE_TTL + Duration::from_secs(1)),
            PairingWindow::Closed,
            "TTL 이 지나면 오래된 경고를 계속 띄우지 않기 위해 Closed 여야 한다"
        );
    }

    #[test]
    fn paired_at_survives_a_reload() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code), t(1010)) else {
            panic!()
        };
        let peers = m.issued_peers();

        let mut reloaded = PairingManager::new();
        reloaded.load_peers(peers);
        let paired = reloaded.paired_peers();
        assert_eq!(paired.len(), 1);
        assert_eq!(paired[0].peer_id, PairingManager::peer_id_of(&token));
        assert_eq!(paired[0].paired_at, 1010, "왕복 후에도 페어링 시각이 보존돼야 한다");
    }
}
