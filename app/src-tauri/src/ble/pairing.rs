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
use crate::crypto::{self, channel::SealedChannel};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// v2 페어링 시작. 클라이언트 임시 공개키(64 hex).
    Hello2(String),
    /// v2 코드 제출. `HMAC(6자리코드, transcript)` 의 hex — **코드 자체가 아니다.**
    Code2(String),
    /// v2 재연결 시작. 클라이언트 임시 공개키(64 hex).
    Auth2(String),
    /// v2 재연결 증명. `HMAC(token, nonce || transcript)` 의 hex.
    Proof2(String),
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthReply {
    /// 창이 열려 있다는 응답. 창에는 소유자가 없다(스펙 5.1) — 이 응답을
    /// 받았다고 해서 그 central 에 창이 묶이는 것은 아니며, 창이 열려 있는
    /// 동안에는 어느 central 이든 CODE: 를 제출할 수 있다. 코드 자체는 담지
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
    /// v2 의 `AwaitingCode`. 맥의 임시 공개키와 논스를 함께 싣는다.
    /// **코드는 여전히 담지 않는다** — 코드는 맥 화면으로만 간다.
    AwaitingCode2 { epk: String, nonce: String },
    /// v2 의 `Granted`. 토큰을 평문 필드가 아니라 봉인 프레임으로 싣는다.
    Granted2 { sealed: String },
    /// v2 의 `Nonce`. 맥의 임시 공개키를 함께 싣는다 — 클라이언트가 이
    /// 임시키로 새 공유 비밀을 만들어야 `session_proof` 에 넣을
    /// transcript 를 계산할 수 있다.
    Nonce2 { epk: String, nonce: String },
    /// v2 의 `Authorized`. `Authorized` 와 같은 이유로 되돌려보낼 비밀이
    /// 없다 — 필드도 없다.
    Authorized2,
}

/// 응답 하나가 전송 계층에 알려주는 두 가지 사실.
///
/// **세 전송이 각자 판정하지 않는다.** 예전에는 BLE·네트워크·LAN 이 같은
/// `matches!` 를 각자 베껴 두고 있었고, 그 형태가 정확히 `Granted2` 를 빠뜨리는
/// 사고를 낳았다 — 빠뜨려도 컴파일되고, 그 세션에서는 멀쩡히 동작하며, 맥을
/// 재시작해야 드러난다. 판정은 `AuthReply` 를 소유한 이 모듈에 한 벌만 둔다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplySignals {
    /// 이 응답이 central 을 인가된 상태로 만들었는가. 전송은 이걸 보고
    /// 스냅샷 경로를 연다.
    pub authorized: bool,
    /// **새 토큰이 발급됐는가.** 앱은 이걸 보고 페어링 목록을 디스크에 쓴다.
    /// 빠뜨리면 그 기기는 맥을 껐다 켜는 순간 영영 재연결하지 못한다.
    pub granted: bool,
}

impl AuthReply {
    /// 이 응답의 의미를 전송이 쓸 수 있는 두 신호로 옮긴다.
    ///
    /// `matches!` 가 아니라 **모든 변형을 적는 `match`** 인 것이 요점이다.
    /// 프로토콜에 응답이 하나 늘면 여기서 컴파일이 깨져 사람이 판정을 다시
    /// 보게 된다 — `matches!` 는 조용히 `false` 를 돌려준다.
    pub fn signals(&self) -> ReplySignals {
        let (authorized, granted) = match self {
            // 인가 + 새 토큰.
            AuthReply::Granted { .. } | AuthReply::Granted2 { .. } => (true, true),
            // 인가만 — 재연결은 이미 아는 토큰을 확인했을 뿐이라 저장할 것이 없다.
            AuthReply::Authorized | AuthReply::Authorized2 => (true, false),
            // 아직 핸드셰이크 중이거나 거절이다.
            AuthReply::AwaitingCode
            | AuthReply::AwaitingCode2 { .. }
            | AuthReply::Nonce { .. }
            | AuthReply::Nonce2 { .. }
            | AuthReply::Denied { .. }
            | AuthReply::Rejected => (false, false),
        };
        ReplySignals { authorized, granted }
    }

    /// 전송 계층(BLE Auth 특성 notify, 네트워크 제어 스트림)에 그대로 실어
    /// 보낼 JSON 바이트. 두 전송 모두 같은 인증 프로토콜을 쓰므로 응답
    /// 포맷도 여기 한 곳에서만 정의한다 — 하나만 고치고 다른 쪽을 잊는
    /// 드리프트를 막는다.
    pub fn to_json_bytes(&self) -> Vec<u8> {
        match self {
            // 코드는 Mac 화면에만 보여준다 — central 에게 보내면 페어링이 무의미해진다.
            AuthReply::AwaitingCode => br#"{"ok":false,"await":"code"}"#.to_vec(),
            AuthReply::Nonce { nonce } => {
                format!(r#"{{"ok":false,"nonce":"{nonce}"}}"#).into_bytes()
            }
            AuthReply::Authorized => br#"{"ok":true}"#.to_vec(),
            AuthReply::Granted { token } => {
                format!(r#"{{"ok":true,"token":"{token}"}}"#).into_bytes()
            }
            AuthReply::Denied { left } => format!(r#"{{"ok":false,"left":{left}}}"#).into_bytes(),
            AuthReply::Rejected => br#"{"ok":false}"#.to_vec(),
            AuthReply::AwaitingCode2 { epk, nonce } => {
                format!(r#"{{"ok":false,"v":2,"await":"code","epk":"{epk}","nonce":"{nonce}"}}"#)
                    .into_bytes()
            }
            AuthReply::Granted2 { sealed } => {
                format!(r#"{{"ok":true,"v":2,"sealed":"{sealed}"}}"#).into_bytes()
            }
            AuthReply::Nonce2 { epk, nonce } => {
                format!(r#"{{"ok":false,"v":2,"epk":"{epk}","nonce":"{nonce}"}}"#).into_bytes()
            }
            AuthReply::Authorized2 => br#"{"ok":true,"v":2}"#.to_vec(),
        }
    }
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
    // v2 를 먼저 본다. `HELLO` 를 먼저 검사하면 `HELLO2:...` 가 걸리지 않지만,
    // 순서를 명시해 두는 편이 나중에 동사가 늘 때 안전하다.
    if let Some(rest) = s.strip_prefix("HELLO2:") {
        return AuthRequest::Hello2(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("CODE2:") {
        return AuthRequest::Code2(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("AUTH2:") {
        return AuthRequest::Auth2(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("PROOF2:") {
        return AuthRequest::Proof2(rest.to_string());
    }
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

/// v2 핸드셰이크 중간 상태. central 마다 하나이며, CODE2/PROOF2 를 받는
/// 순간 소비된다. 임시 개인키는 `EphemeralKeyPair` 안에 있고 한 번만 쓰인다.
#[derive(Debug)]
struct PendingHandshake {
    /// 이 핸드셰이크로 만든 공유 비밀.
    ss: [u8; 32],
    transcript: [u8; 64],
    /// 이 핸드셰이크에 쓰인 논스(hex). 세션 키 파생의 salt 다.
    nonce: String,
    /// 발급 시각. `PendingNonce` 와 같은 이유로 스윕 대상이다(전체 브랜치
    /// 리뷰 I-5) — `HELLO2`/`AUTH2` 만 보내고 사라지는 central 이 쌓이면
    /// 원격에서 키우는 메모리 누수가 된다.
    issued_at: SystemTime,
    /// 이 핸드셰이크가 살아 있어도 되는 기간. **핸드셰이크를 만든 동사에
    /// 따라 다르다** — `nonces` 하나에 `NONCE_TTL` 하나만 쓰면 되는 것과
    /// 다른 이유는, `handshakes` 맵에는 성격이 다른 두 흐름이 섞여
    /// 들어오기 때문이다(이 파일 47-50줄, `NONCE_TTL` 자체의 존재 이유와
    /// 같은 논리를 여기서는 맵 하나 안에서 둘로 나눠 적용한다):
    /// - `HELLO2`(페어링)는 사람 속도다 — 사용자가 맥 화면의 6자리를 읽어
    ///   폰에 옮겨 적는 동안 살아 있어야 하므로 `CODE_TTL`(120초).
    /// - `AUTH2`(재연결)는 기계 속도다 — `AUTH2` → 서명 → `PROOF2` 가
    ///   사람 개입 없이 밀리초 단위로 끝나므로 `NONCE_TTL`(30초)로 충분하고,
    ///   짧을수록 탈취된 공유 비밀 `ss` 가 메모리에 머무는 시간도 줄어든다.
    ///
    /// 두 흐름이 같은 `CentralId` 를 키로 쓰는 한 맵을 공유하므로, 상수
    /// 하나로 스윕하면 한쪽에 맞는 값이 다른 쪽에는 항상 틀리다 — 120초로
    /// 고정하면 재연결 핸드셰이크가 불필요하게 오래 살고, 30초로 고정하면
    /// (라운드 1 회귀, 전체 브랜치 리뷰 I-5 재검토) 코드를 옮겨 적는 사용자의
    /// `HELLO2` 핸드셰이크가 창이 열려 있는데도 먼저 죽는다. 그래서 TTL 을
    /// 핸드셰이크마다 들고 다닌다.
    ttl: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PairedPeer {
    pub peer_id: String,
    pub paired_at: u64,
    /// 지금 이 기기가 붙어서 인가된 상태인가. `authorized` 값(토큰)으로 판정한다.
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PairingWindow {
    /// 창이 열려 있다. 만료 시각을 절대 epoch 초로 준다 — 프론트가 이 값을
    /// 한 번만 받아 자체 타이머로 카운트다운을 계산해야, `ble_status` 이벤트가
    /// (BLE 활동이 있을 때만 발행되므로) 뜸해도 화면이 멈추지 않는다(전체
    /// 브랜치 리뷰 I-1 — `seconds_left` 스냅샷은 재계산 없이는 영원히 그
    /// 값에 멈췄고, 만료돼도 화면이 계속 크게 표시되며 [페어링 시작] 버튼도
    /// 다시 안 나타났다).
    Open { code: String, expires_at: u64, attempts_left: u8 },
    /// 시도 5회가 모두 소진돼 닫혔다. 방해일 수 있으므로 만료와 구분해 보여준다.
    Exhausted,
    /// 열린 적 없거나 시간이 지나 닫혔다.
    Closed,
}

#[derive(Default)]
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
    /// v2 핸드셰이크 중간 상태. central 마다 하나이며, CODE2/PROOF2 를 받는
    /// 순간 소비된다. 임시 개인키는 `EphemeralKeyPair` 안에 있고 한 번만 쓰인다.
    handshakes: HashMap<String, PendingHandshake>,
    /// 인가된 세션의 봉인 채널. `authorized` 와 수명이 같다.
    channels: HashMap<String, SealedChannel>,
}

/// `#[derive(Debug)]` 를 쓰지 않는다 — `SealedChannel` 은 대칭키를 들고
/// 있어서 일부러 `Debug` 를 구현하지 않았고(로그에 키가 찍히면 안 된다),
/// `handshakes` 의 공유 비밀도 마찬가지로 민감하다. **`pending.code` 도
/// 마찬가지로 절대 찍지 않는다** — 코드는 `code_binding` 의 HMAC 키이자
/// 무차별 대입 방어(창당 시도 5회)의 전제 그 자체다(스펙 5.1/5.2). 이
/// 값이 로그·패닉 메시지·크래시 리포트로 한 번이라도 새면, 능동적 MITM 은
/// 추측 없이 곧바로 유효한 `code_binding` 을 만들 수 있어 시도 예산이
/// 통째로 무의미해진다(전체 브랜치 리뷰 I-1). 그래서 `pending` 은 코드를
/// 뺀 요약(남은 시도, 만료 시각)만 보여준다.
impl std::fmt::Debug for PairingManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pending_summary = self.pending.as_ref().map(|p| {
            let expires_at = (p.issued_at + CODE_TTL)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            (p.attempts_left, expires_at)
        });
        f.debug_struct("PairingManager")
            .field("pending_attempts_left_and_expires_at", &pending_summary)
            .field("tokens", &self.tokens.len())
            .field("authorized", &self.authorized.len())
            .field("nonces", &self.nonces.len())
            .field("handshakes", &self.handshakes.len())
            .field("channels", &self.channels.len())
            .finish()
    }
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

    /// 열려 있던 페어링 창을 닫고 리셋한다.
    pub fn reset_pairing_window(&mut self) {
        self.pending = None;
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

    /// 이 central 의 v2 봉인 채널. 인가된 세션에만 존재한다 — 채널이 없으면
    /// v1 세션이거나, 아직 v2 핸드셰이크가 끝나지 않았거나, 세션이 이미
    /// 끝났다는 뜻이다.
    pub fn channel_mut(&mut self, id: &CentralId) -> Option<&mut SealedChannel> {
        self.channels.get_mut(&id.0)
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
        // v2 봉인 채널·핸드셰이크도 `authorized` 와 수명을 맞춘다 — 남겨두면
        // 방금 언페어링된 기기용 채널로 다음 프레임을 봉인하게 된다.
        self.channels.remove(&id.0);
        self.handshakes.remove(&id.0);
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
            self.channels.remove(cid);
            self.handshakes.remove(cid);
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
        // `authorized` 를 통째로 지우므로, 거기 딸려 있던 v2 채널·핸드셰이크도
        // 통째로 지운다 — 하나씩 골라 지울 이유가 없다.
        self.channels.clear();
        self.handshakes.clear();
        dropped.into_iter().map(CentralId).collect()
    }

    /// 세션 인가만 전부 지운다 — `revoke_all` 과 달리 저장된 토큰(`tokens`)은
    /// 남긴다. 공유를 끄거나(`set_enabled(false)`) 블루투스 전원이 꺼지면
    /// `end_session` 을 부르는 `did_unsubscribe`/`Disconnected` 콜백이 오지
    /// 않아 `authorized` 가 실제 연결보다 오래 살아남는다 — 기기 목록의
    /// `연결됨` 배지가 계속 거짓말을 하고, 전원을 반복해서 껐다 켤 때마다
    /// 죽은 central id 가 쌓인다(전체 브랜치 리뷰 I-2). 재시작 없이 다시
    /// 공유를 켜면 같은 토큰으로 즉시 재인가되므로 사용자 경험은 그대로다.
    pub fn end_all_sessions(&mut self) {
        self.authorized.clear();
        self.channels.clear();
        self.handshakes.clear();
    }

    /// 주어진 central 들의 세션 인가만 내린다. 저장된 토큰은 남긴다.
    ///
    /// BLE 와 네트워크가 이 매니저를 **공유**하므로(2026-08-25 스펙), 전송 하나를
    /// 끌 때 `end_all_sessions` 를 쓰면 다른 전송의 세션까지 죽는다. 각 전송은
    /// 자기가 서비스 중이던 central 목록만 넘긴다.
    ///
    /// 모르는 id 는 조용히 무시한다 — 언페어링 시 앱은 그 central 이 어느 전송에
    /// 붙어 있었는지 모르는 채로 두 브릿지 모두에 같은 목록을 넘기기 때문이다.
    pub fn end_sessions(&mut self, ids: &[CentralId]) {
        for id in ids {
            self.end_session(id);
        }
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
        let expires_at = (p.issued_at + CODE_TTL)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        PairingWindow::Open {
            code: p.code.clone(),
            expires_at,
            attempts_left: p.attempts_left,
        }
    }

    /// 만료된 논스·v2 핸드셰이크를 청소한다. `AUTH`/`HELLO2`/`AUTH2` 만
    /// 보내고 사라지는 central 이 계속 쌓이면 원격에서 키우는 메모리
    /// 누수가 되므로, 모든 요청 처리 시점마다 훑는다. 핸드셰이크는
    /// `CODE2`/`PROOF2`/`end_session`/`revoke_*` 로만 지워지고 자체 TTL 이
    /// 없었다(전체 브랜치 리뷰 I-5).
    ///
    /// **`nonces` 는 TTL 이 하나(`NONCE_TTL`)지만, `handshakes` 는 항목마다
    /// 다르다(`PendingHandshake::ttl`).** 논스는 항상 기계 속도라 — `AUTH`
    /// → 서명 → `PROOF` 가 사람 개입 없이 밀리초 단위로 끝나므로
    /// `NONCE_TTL`(30초)이면 충분하다(이 파일 47-50줄: "논스는 연결할
    /// 때마다 매번 새로 받아 즉시 쓰는 값이라... 여유 시간이 필요
    /// 없다"). 반면 `handshakes` 맵에는 성격이 다른 두 흐름이 섞여
    /// 들어온다 — `HELLO2`(페어링, 사람 속도: 사용자가 맥 화면의 6자리를
    /// 폰에 옮겨 적는다)와 `AUTH2`(재연결, 기계 속도: 논스와 마찬가지로
    /// 밀리초 단위). 라운드 1 이 이 맵 전체를 `NONCE_TTL` 로 스윕한 것도
    /// 회귀였고(전체 브랜치 리뷰 I-5 재검토 — 코드를 옮겨 적는 데 30초
    /// 이상 걸리는 사용자는 창은 `Open` 인데 `CODE2` 만 이유 없이
    /// `Rejected` 로 튕겼다), Task 7 이 이어서 이 맵 전체를 `CODE_TTL` 로
    /// 고정한 것도 같은 종류의 실수였다 — 상수 하나로는 두 흐름 중 어느
    /// 쪽에도 정확히 맞지 않는다(재연결 핸드셰이크가 불필요하게 120초씩
    /// 살아 있게 된다). 그래서 TTL 을 핸드셰이크 자신에게 들려 보낸다 —
    /// `Hello2` 는 `CODE_TTL`, `Auth2` 는 `NONCE_TTL` 로 넣는다.
    fn sweep_expired(&mut self, now: SystemTime) {
        self.nonces
            .retain(|_, n| now.duration_since(n.issued_at).unwrap_or_default() <= NONCE_TTL);
        self.handshakes
            .retain(|_, h| now.duration_since(h.issued_at).unwrap_or_default() <= h.ttl);
    }

    #[cfg(test)]
    fn nonce_count(&self) -> usize {
        self.nonces.len()
    }

    #[cfg(test)]
    fn handshake_count(&self) -> usize {
        self.handshakes.len()
    }

    pub fn handle(&mut self, id: &CentralId, req: AuthRequest, now: SystemTime) -> AuthReply {
        self.sweep_expired(now);
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
                    // 이 central 이 예전에 v2 로 페어링됐다가 `end_session` 없이
                    // 다시 v1 으로 들어오는 경우를 대비한다 — 낡은 v2 채널이
                    // 남아 있으면 Task 9 가 `channel_mut` 로 v1/v2 를 가르는
                    // 순간 v1 세션인데 낡은 키로 스냅샷을 봉인하게 된다(전체
                    // 브랜치 리뷰 I-2). `authorized` 를 덮어쓰는 자리이므로
                    // 지우는 자리와 똑같이 취급한다.
                    self.channels.remove(&id.0);
                    self.handshakes.remove(&id.0);
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
                // 만료 검사는 없다 — handle() 진입 시 sweep_expired 가
                // 이미 이 `now` 기준으로 지난 논스를 전부 제거했으므로,
                // 여기까지 남아 있다면 반드시 유효하다.
                let matched = self
                    .tokens
                    .keys()
                    .find(|token| Self::verify_proof(token, &pending.nonce, &given))
                    .cloned();
                if let Some(token) = matched {
                    self.authorized.insert(id.0.clone(), token);
                    // v1 `Code` 성공 경로와 같은 이유(전체 브랜치 리뷰 I-2) —
                    // `authorized` 를 덮어쓰는 자리에서는 낡은 v2 채널·핸드셰이크도
                    // 함께 지운다.
                    self.channels.remove(&id.0);
                    self.handshakes.remove(&id.0);
                    AuthReply::Authorized
                } else {
                    AuthReply::Rejected
                }
            }
            AuthRequest::Hello2(cpk_hex) => {
                let Some(cpk) = Self::hex32(&cpk_hex) else {
                    // 형식 오류는 코드 추측이 아니므로 예산을 소모하지 않는다.
                    return AuthReply::Rejected;
                };
                if self.open_window(now).is_none() {
                    return AuthReply::Rejected;
                }
                let kp = crypto::ephemeral_keypair();
                let spk = kp.public;
                let Some(ss) = crypto::agree(kp, &cpk) else {
                    // 저차 점 — 공유 비밀이 상수가 된다.
                    return AuthReply::Rejected;
                };
                let nonce = Self::random_hex128();
                self.handshakes.insert(
                    id.0.clone(),
                    PendingHandshake {
                        ss,
                        transcript: crypto::transcript(&cpk, &spk),
                        nonce: nonce.clone(),
                        issued_at: now,
                        // 사람 속도(스펙 5.1) — `HELLO2` 와 `CODE2` 사이에
                        // 사용자가 맥 화면의 6자리를 폰에 옮겨 적는다.
                        ttl: CODE_TTL,
                    },
                );
                AuthReply::AwaitingCode2 { epk: hex_encode_bytes(&spk), nonce }
            }
            AuthRequest::Code2(given) => {
                // 핸드셰이크는 성공·실패와 무관하게 소비한다 — 같은 transcript
                // 로 여러 번 추측하지 못하게 한다. 재시도하려면 HELLO2 부터다.
                let Some(hs) = self.handshakes.remove(&id.0) else {
                    return AuthReply::Rejected;
                };
                let Some(p) = self.open_window(now) else {
                    return AuthReply::Rejected;
                };
                if !crypto::verify_code_binding(&p.code, &hs.transcript, &given) {
                    p.attempts_left -= 1;
                    return AuthReply::Denied { left: p.attempts_left };
                }
                let token = Self::random_hex128();
                let paired_at = now
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let token_bytes = Self::hex_decode(&token).expect("방금 만든 hex 다");
                let nonce_bytes = Self::hex_decode(&hs.nonce).expect("방금 만든 hex 다");

                // 토큰 한 건만 k_pair 로 봉인한다.
                let k_pair = crypto::derive_pair_key(&hs.ss, &nonce_bytes);
                let mut pair_ch = SealedChannel::new(k_pair, k_pair);
                let payload = format!(r#"{{"token":"{token}"}}"#);
                let sealed = hex_encode_bytes(&pair_ch.seal(payload.as_bytes()));

                // 그 즉시 세션 키로 전환한다(스펙 6.1) — 왕복이 더 필요 없다.
                let (s2c, c2s) = crypto::derive_session_keys(&hs.ss, &token_bytes, &nonce_bytes);
                self.channels.insert(id.0.clone(), SealedChannel::new(s2c, c2s));

                self.tokens.insert(token.clone(), paired_at);
                self.authorized.insert(id.0.clone(), token);
                self.pending = None;
                AuthReply::Granted2 { sealed }
            }
            AuthRequest::Auth2(cpk_hex) => {
                let Some(cpk) = Self::hex32(&cpk_hex) else {
                    // 형식 오류는 코드 추측이 아니다 — v1 Hello2 와 같은 기준.
                    return AuthReply::Rejected;
                };
                let kp = crypto::ephemeral_keypair();
                let spk = kp.public;
                let Some(ss) = crypto::agree(kp, &cpk) else {
                    // 저차 점 — 공유 비밀이 상수가 된다.
                    return AuthReply::Rejected;
                };
                let nonce = Self::random_hex128();
                // v1 `Auth` 와 같이, 새 논스는 이전 논스를 덮어써 무효화한다.
                // `nonces` 항목은 `NONCE_TTL`(30초, 기계 속도)로 스윕된다.
                self.nonces.insert(
                    id.0.clone(),
                    PendingNonce { nonce: nonce.clone(), issued_at: now },
                );
                // 핸드셰이크(공유 비밀·transcript)는 `HELLO2` 와 같은
                // `handshakes` 맵에 둔다 — 여기 들어오는 항목은 재연결용이라
                // 열린 페어링 창이 전혀 필요 없다는 점만 다르다. TTL 은
                // `PendingHandshake::ttl` 의 doc comment 가 설명하듯 흐름마다
                // 다르다 — `Auth2` 는 기계 속도(`AUTH2`→서명→`PROOF2` 가
                // 밀리초 단위)이므로 `NONCE_TTL`(30초)을 쓴다. 이 값은
                // `nonces` 항목의 TTL 과 정확히 같다 — 어차피 `Proof2` 가
                // 둘 다 있어야 통과하므로, 둘을 다르게 둘 이유가 없다.
                self.handshakes.insert(
                    id.0.clone(),
                    PendingHandshake {
                        ss,
                        transcript: crypto::transcript(&cpk, &spk),
                        nonce: nonce.clone(),
                        issued_at: now,
                        ttl: NONCE_TTL,
                    },
                );
                AuthReply::Nonce2 { epk: hex_encode_bytes(&spk), nonce }
            }
            AuthRequest::Proof2(given) => {
                // v1 `Proof` 와 같이 성공·실패와 무관하게 즉시 폐기한다
                // (1회용 논스) — 캡처한 PROOF2 를 재생해도 두 번째부터는
                // 통하지 않아야 한다.
                let Some(pending) = self.nonces.remove(&id.0) else {
                    // AUTH2 없이 곧바로 온 PROOF2, 혹은 이미 소비/만료된 논스.
                    return AuthReply::Rejected;
                };
                let Some(hs) = self.handshakes.remove(&id.0) else {
                    return AuthReply::Rejected;
                };
                let Some(nonce_bytes) = Self::hex_decode(&pending.nonce) else {
                    return AuthReply::Rejected;
                };
                // 토큰은 이 라운드트립 어디에도 실리지 않는다 — 저장된 토큰
                // 후보들에 대해 각각 proof 를 검증해, 어떤 토큰의 소유자가
                // 이 요청을 보냈는지 알아낼 뿐이다(v1 `Proof` 와 같은 구조).
                // `verify_session_proof` 가 transcript 도 함께 검증하므로,
                // 능동적 MITM 이 임시 키를 바꿔치기하면(=다른 transcript)
                // 올바른 토큰으로 만든 proof 라도 실패한다.
                let matched = self
                    .tokens
                    .keys()
                    .find(|token| {
                        Self::hex_decode(token).is_some_and(|tb| {
                            crypto::verify_session_proof(&tb, &nonce_bytes, &hs.transcript, &given)
                        })
                    })
                    .cloned();
                let Some(token) = matched else {
                    // 실패는 코드 추측이 아니다 — 페어링 창의 시도 예산과는
                    // 무관하므로 여기서 소모할 예산 자체가 없다.
                    return AuthReply::Rejected;
                };
                let token_bytes = Self::hex_decode(&token).expect("저장된 토큰은 유효한 hex 다");
                // `ikm = ss || token` — 토큰이 새도 임시 개인키가 필요하고,
                // X25519 가 깨져도 토큰이 필요하다(스펙 6.1).
                let (s2c, c2s) = crypto::derive_session_keys(&hs.ss, &token_bytes, &nonce_bytes);
                self.channels.insert(id.0.clone(), SealedChannel::new(s2c, c2s));
                self.authorized.insert(id.0.clone(), token);
                AuthReply::Authorized2
            }
            AuthRequest::Malformed => AuthReply::Rejected,
        }
    }

    fn is_valid_lowercase_hex(s: &str, len: usize) -> bool {
        s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }

    /// v2 임시 공개키(64자 소문자 hex)를 32바이트로 디코드한다. 형식이
    /// 다르면 `hex_decode` 를 시도조차 하지 않는다 — 다른 hex 필드와 같은
    /// 기준.
    fn hex32(s: &str) -> Option<[u8; 32]> {
        if !Self::is_valid_lowercase_hex(s, 64) {
            return None;
        }
        let v = Self::hex_decode(s)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        Some(out)
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

/// 모듈 수준 hex 인코더. 기존 `hex_encode` 는 `mod tests` 안에만 있어
/// 프로덕션 코드에서 쓸 수 없다.
pub(crate) fn hex_encode_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 테스트 전용 v2 클라이언트. 이 파일의 `mod tests` 안에 있던 것을 형제
/// 모듈에서도 볼 수 있게 끌어올렸다 — 전송 배선(`ble::mod`, `network::mod`)
/// 테스트도 **진짜** 핸드셰이크를 밟아야 봉인 여부를 확인할 수 있는데, 셋이
/// 각자 베껴 두면 스펙이 바뀔 때 한 곳만 고치고 나머지를 잊는다.
#[cfg(test)]
pub(crate) mod test_client {
    use super::*;
    use crate::crypto;

    pub(crate) fn hex_encode(bytes: &[u8]) -> String {
        hex_encode_bytes(bytes)
    }

    /// 테스트에서 "v2 클라이언트" 역할을 한다.
    pub(crate) struct V2Client {
        kp: Option<crypto::EphemeralKeyPair>,
        pub(crate) public: [u8; 32],
    }

    impl V2Client {
        pub(crate) fn new() -> Self {
            let kp = crypto::ephemeral_keypair();
            let public = kp.public;
            Self { kp: Some(kp), public }
        }
        /// 서버 응답으로부터 공유 비밀과 transcript 를 만든다.
        pub(crate) fn agree(&mut self, epk_hex: &str) -> ([u8; 32], [u8; 64]) {
            let spk = hex32(epk_hex);
            let kp = self.kp.take().expect("임시 키는 한 번만 쓴다");
            let ss = crypto::agree(kp, &spk).expect("정상 키끼리는 합의된다");
            (ss, crypto::transcript(&self.public, &spk))
        }
    }

    pub(crate) fn hex32(s: &str) -> [u8; 32] {
        let v = PairingManager::hex_decode(s).expect("유효한 hex");
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        out
    }

    pub(crate) fn hex_decode(s: &str) -> Vec<u8> {
        PairingManager::hex_decode(s).expect("유효한 hex")
    }

    pub(crate) fn epk_and_nonce(reply: &AuthReply) -> (String, String) {
        match reply {
            AuthReply::AwaitingCode2 { epk, nonce } => (epk.clone(), nonce.clone()),
            other => panic!("AwaitingCode2 를 기대했다: {other:?}"),
        }
    }

    /// `Granted2` 의 봉인 토큰을 열고, 그 토큰으로 **클라이언트 방향** 세션
    /// 채널을 만든다. 맥은 `(s2c, c2s)` 로, 클라이언트는 그 반대로 넣는다 —
    /// 이 뒤집기가 전송 계층 테스트에서 가장 틀리기 쉬운 지점이라 한 곳에
    /// 모아 둔다(스펙 6.1).
    pub(crate) fn open_pairing_and_session(
        ss: &[u8; 32],
        nonce_hex: &str,
        sealed_hex: &str,
    ) -> (String, SealedChannel) {
        let nonce_bytes = hex_decode(nonce_hex);
        let k_pair = crypto::derive_pair_key(ss, &nonce_bytes);
        let mut pair_ch = SealedChannel::new(k_pair, k_pair);
        let plain = pair_ch
            .open(&hex_decode(sealed_hex))
            .expect("k_pair 로 열려야 한다");
        let json: serde_json::Value = serde_json::from_slice(&plain).expect("유효한 JSON");
        let token = json["token"].as_str().expect("토큰이 들어 있어야 한다").to_string();

        let token_bytes = hex_decode(&token);
        let (s2c, c2s) = crypto::derive_session_keys(ss, &token_bytes, &nonce_bytes);
        (token, SealedChannel::new(c2s, s2c))
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

    use crate::crypto::{self, channel::SealedChannel};
    use super::test_client::{epk_and_nonce, hex_encode, V2Client};

    /// 응답 하나하나가 전송에 무엇을 뜻하는지 한자리에서 못박는다. 세 전송이
    /// 각자 판정하던 것을 여기로 모았으므로, 이 표가 그 유일한 기준이다.
    /// `signals()` 가 모든 변형을 적는 `match` 라서 응답이 늘면 컴파일이 먼저
    /// 깨지고, 그다음을 이 표가 잡는다.
    #[test]
    fn every_reply_has_signals() {
        let table = vec![
            (AuthReply::Granted { token: "t".into() }, true, true),
            (AuthReply::Granted2 { sealed: "s".into() }, true, true),
            (AuthReply::Authorized, true, false),
            (AuthReply::Authorized2, true, false),
            (AuthReply::AwaitingCode, false, false),
            (AuthReply::AwaitingCode2 { epk: "e".into(), nonce: "n".into() }, false, false),
            (AuthReply::Nonce { nonce: "n".into() }, false, false),
            (AuthReply::Nonce2 { epk: "e".into(), nonce: "n".into() }, false, false),
            (AuthReply::Denied { left: 2 }, false, false),
            (AuthReply::Rejected, false, false),
        ];
        for (reply, authorized, granted) in table {
            assert_eq!(
                reply.signals(),
                ReplySignals { authorized, granted },
                "{reply:?} 의 판정이 다르다"
            );
        }
    }

    /// 새 토큰을 발급하는 응답은 정확히 둘이다. 이 성질이 깨지면 어떤 기기는
    /// 페어링에 성공하고도 토큰이 디스크에 남지 않아 재부팅 후 사라진다.
    #[test]
    fn granted_implies_authorized_and_only_grants_carry_a_new_token() {
        let all = vec![
            AuthReply::Granted { token: "t".into() },
            AuthReply::Granted2 { sealed: "s".into() },
            AuthReply::Authorized,
            AuthReply::Authorized2,
            AuthReply::AwaitingCode,
            AuthReply::AwaitingCode2 { epk: "e".into(), nonce: "n".into() },
            AuthReply::Nonce { nonce: "n".into() },
            AuthReply::Nonce2 { epk: "e".into(), nonce: "n".into() },
            AuthReply::Denied { left: 2 },
            AuthReply::Rejected,
        ];
        let granting: Vec<&AuthReply> = all.iter().filter(|r| r.signals().granted).collect();
        assert_eq!(granting.len(), 2, "발급 응답은 v1/v2 각각 하나씩뿐이다: {granting:?}");
        for r in &all {
            let s = r.signals();
            assert!(!s.granted || s.authorized, "{r:?}: 발급했는데 인가가 아니다");
        }
    }

    #[test]
    fn v2_pairing_delivers_the_token_sealed() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();

        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, nonce) = epk_and_nonce(&reply);
        let (ss, tr) = c.agree(&epk);

        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let reply = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1002));
        let AuthReply::Granted2 { sealed } = reply else {
            panic!("Granted2 를 기대했다: {reply:?}")
        };

        // 클라이언트가 토큰을 꺼낸다.
        let nonce_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let k_pair = crypto::derive_pair_key(&ss, &nonce_bytes);
        let mut ch = SealedChannel::new(k_pair, k_pair);
        let frame = PairingManager::hex_decode(&sealed).unwrap();
        let plain = ch.open(&frame).expect("k_pair 로 열려야 한다");
        let json: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        let token = json["token"].as_str().expect("토큰이 들어 있어야 한다");

        assert_eq!(token.len(), 32, "토큰은 128비트 소문자 hex 다");
        assert!(m.is_authorized(&id("A")));
        assert_eq!(m.issued_peers().len(), 1);
    }

    /// **이 스펙의 존재 이유다.** 토큰이 평문으로 나가면 안 된다.
    #[test]
    fn v2_never_puts_the_token_in_cleartext() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, _nonce) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let reply = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1002));

        let bytes = reply.to_json_bytes();
        let text = String::from_utf8(bytes).unwrap();
        let token = m.issued_peers()[0].0.clone();
        assert!(!text.contains(&token), "토큰이 평문으로 나갔다: {text}");
        assert!(!text.contains("\"token\""), "token 필드 자체가 없어야 한다: {text}");
    }

    /// 6자리 코드도 어느 방향으로도 나가지 않는다.
    #[test]
    fn v2_never_puts_the_code_on_the_wire() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let text = String::from_utf8(reply.to_json_bytes()).unwrap();
        assert!(!text.contains(&code), "코드가 응답에 실렸다: {text}");
    }

    #[test]
    fn v2_wrong_code_binding_spends_an_attempt() {
        let mut m = PairingManager::new();
        let _code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, _) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);

        let wrong = hex_encode(&crypto::code_binding("999999", &tr));
        let reply = m.handle(&id("A"), AuthRequest::Code2(wrong), t(1002));
        assert!(matches!(reply, AuthReply::Denied { left: 4 }), "시도를 소모한다: {reply:?}");
    }

    /// 형식 오류는 코드 추측이 아니므로 예산을 소모하지 않는다.
    #[test]
    fn v2_malformed_public_key_does_not_spend_an_attempt() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        let reply = m.handle(&id("A"), AuthRequest::Hello2("짧다".into()), t(1001));
        assert!(matches!(reply, AuthReply::Rejected));
        match m.pairing_window(t(1002)) {
            PairingWindow::Open { attempts_left, .. } => {
                assert_eq!(attempts_left, MAX_ATTEMPTS, "형식 오류는 예산을 안 깎는다")
            }
            other => panic!("창이 열려 있어야 한다: {other:?}"),
        }
    }

    /// 저차 점을 보내면 공유 비밀이 상수가 된다. 거부해야 한다.
    #[test]
    fn v2_rejects_low_order_public_key() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&[0u8; 32])), t(1001));
        assert!(matches!(reply, AuthReply::Rejected), "저차 점 거부: {reply:?}");
    }

    /// HELLO2 없이 온 CODE2 는 transcript 가 없으므로 검증할 수 없다.
    #[test]
    fn v2_code_without_hello_is_rejected() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        let reply = m.handle(&id("A"), AuthRequest::Code2("ab".repeat(32)), t(1001));
        assert!(matches!(reply, AuthReply::Rejected));
    }

    /// 전체 브랜치 리뷰 I-3 회귀 테스트: 틀린 바인딩을 낸 CODE2 도 핸드셰이크를
    /// 소비해야 한다. 그러지 않으면 같은 transcript 로 여러 바인딩을 계속
    /// 시도할 수 있어(HELLO2 를 다시 안 보내도) 시도 예산만으로는 막히지 않는
    /// 반복 추측 경로가 열린다.
    #[test]
    fn v2_failed_code_binding_consumes_the_handshake() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, _) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);

        let wrong = hex_encode(&crypto::code_binding("999999", &tr));
        let r1 = m.handle(&id("A"), AuthRequest::Code2(wrong), t(1002));
        assert!(matches!(r1, AuthReply::Denied { left: 4 }));

        // 같은 transcript 로 이번엔 진짜 바인딩을 내도 거부돼야 한다 —
        // 핸드셰이크가 이미 소비됐다. 재시도하려면 새 HELLO2 부터다.
        let correct = hex_encode(&crypto::code_binding(&code, &tr));
        let r2 = m.handle(&id("A"), AuthRequest::Code2(correct), t(1003));
        assert!(matches!(r2, AuthReply::Rejected), "핸드셰이크는 이미 소비됐어야 한다: {r2:?}");
    }

    /// 성공한 CODE2 도 핸드셰이크를 소비한다 — 같은 값을 재전송해도 통과하면
    /// 안 된다.
    #[test]
    fn v2_code2_replay_after_success_is_rejected() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, _) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));

        let first = m.handle(&id("A"), AuthRequest::Code2(cbind.clone()), t(1002));
        assert!(matches!(first, AuthReply::Granted2 { .. }), "{first:?}");

        let replay = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1003));
        assert!(matches!(replay, AuthReply::Rejected), "성공 후 재전송은 거부돼야 한다: {replay:?}");
    }

    /// 전체 브랜치 리뷰 I-5 재검토 회귀 테스트: 라운드 1 이 핸드셰이크를
    /// `NONCE_TTL`(30초)로 스윕하게 만들었는데, 이는 회귀였다 — 사용자가
    /// 맥 화면의 6자리 코드를 폰에 옮겨 적는 데 30초보다 오래 걸리면
    /// (얼마든지 있을 수 있는 일이다) 창은 아직 `Open`(120초)인데 `CODE2`
    /// 만 이유 없이 `Rejected` 로 튕겼다. HELLO2 와 CODE2 사이에 100초를
    /// 두어(30초는 지났지만 120초 페어링 창 안) 이 사용자가 페어링에
    /// 성공하는지 확인한다.
    #[test]
    fn v2_pairing_survives_a_slow_human() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1000));
        let (epk, _) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));

        // 100초 뒤 — NONCE_TTL(30초)은 지났지만 CODE_TTL(120초) 안이다.
        let reply = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1100));
        assert!(matches!(reply, AuthReply::Granted2 { .. }), "느린 사용자도 페어링돼야 한다: {reply:?}");
    }

    /// 핸드셰이크 스윕이 조용히 꺼져 있지 않은지 확인하는 짝 테스트 —
    /// 페어링 창(120초)보다 오래 묵은 핸드셰이크는 여전히 청소돼야 한다.
    #[test]
    fn v2_handshake_expires_with_the_pairing_window() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1000));
        let (epk, _) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));

        // 200초 뒤 — CODE_TTL(120초)도 지났다.
        let reply = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1200));
        assert!(matches!(reply, AuthReply::Rejected), "120초를 넘긴 핸드셰이크는 거부돼야 한다: {reply:?}");
    }

    /// v2 로 페어링한 뒤 만든 `V2Client` 헬퍼. 전체 브랜치 리뷰 I-4 테스트들이
    /// 공유한다 — `channel_mut` 이 세션마다 실제로 채워지고, 정리 경로마다
    /// 실제로 비는지 확인해야 한다.
    fn v2_pair(m: &mut PairingManager, central: &CentralId, now: SystemTime) {
        let code = m.begin_pairing(now);
        let mut c = V2Client::new();
        let reply = m.handle(central, AuthRequest::Hello2(hex_encode(&c.public)), now);
        let (epk, _nonce) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let reply = m.handle(central, AuthRequest::Code2(cbind), now);
        assert!(matches!(reply, AuthReply::Granted2 { .. }), "v2 페어링 셋업 실패: {reply:?}");
    }

    /// 전체 브랜치 리뷰 I-4 회귀 테스트: `end_session` 이 v2 봉인 채널도
    /// 지워야 한다. 남겨두면 다음에 이 central 이 다시 붙었을 때(v1 이든
    /// v2 든) 낡은 키로 봉인된 채널이 살아남는다.
    #[test]
    fn end_session_clears_the_v2_channel() {
        let mut m = PairingManager::new();
        v2_pair(&mut m, &id("A"), t(1000));
        assert!(m.channel_mut(&id("A")).is_some(), "페어링 직후에는 채널이 있어야 한다");

        m.end_session(&id("A"));
        assert!(m.channel_mut(&id("A")).is_none(), "세션을 끊으면 v2 채널도 지워야 한다");
    }

    /// 전체 브랜치 리뷰 I-4 회귀 테스트.
    #[test]
    fn revoke_peer_clears_the_v2_channel() {
        let mut m = PairingManager::new();
        v2_pair(&mut m, &id("A"), t(1000));
        assert!(m.channel_mut(&id("A")).is_some());

        let token = m.issued_peers()[0].0.clone();
        let peer_id = PairingManager::peer_id_of(&token);
        m.revoke_peer(&peer_id);
        assert!(m.channel_mut(&id("A")).is_none(), "peer 를 폐기하면 v2 채널도 지워야 한다");
    }

    /// 전체 브랜치 리뷰 I-4 회귀 테스트.
    #[test]
    fn revoke_all_clears_v2_channels() {
        let mut m = PairingManager::new();
        v2_pair(&mut m, &id("A"), t(1000));
        assert!(m.channel_mut(&id("A")).is_some());

        m.revoke_all();
        assert!(m.channel_mut(&id("A")).is_none(), "전체 폐기 후에는 v2 채널도 남으면 안 된다");
    }

    /// 전체 브랜치 리뷰 I-4 회귀 테스트.
    #[test]
    fn end_all_sessions_clears_v2_channels() {
        let mut m = PairingManager::new();
        v2_pair(&mut m, &id("A"), t(1000));
        assert!(m.channel_mut(&id("A")).is_some());

        m.end_all_sessions();
        assert!(m.channel_mut(&id("A")).is_none(), "전체 세션 종료 후에는 v2 채널도 남으면 안 된다");
    }

    /// 페어링해서 토큰과 세션 키를 얻은 뒤, 연결을 끊고 재연결한다.
    #[test]
    fn v2_reconnect_with_token_authorizes() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));

        // ── 1차: 페어링 ──
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, nonce) = epk_and_nonce(&reply);
        let (ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let AuthReply::Granted2 { sealed } = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1002))
        else {
            panic!("Granted2 를 기대했다")
        };
        let nonce_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let k_pair = crypto::derive_pair_key(&ss, &nonce_bytes);
        let mut pair_ch = SealedChannel::new(k_pair, k_pair);
        let plain = pair_ch
            .open(&PairingManager::hex_decode(&sealed).unwrap())
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        let token = json["token"].as_str().unwrap().to_string();

        m.end_session(&id("A"));
        assert!(!m.is_authorized(&id("A")));

        // ── 2차: 재연결 ──
        let mut c2 = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Auth2(hex_encode(&c2.public)), t(2000));
        let AuthReply::Nonce2 { epk, nonce } = reply else {
            panic!("Nonce2 를 기대했다: {reply:?}")
        };
        let (_ss2, tr2) = c2.agree(&epk);
        let token_bytes = PairingManager::hex_decode(&token).unwrap();
        let nonce2_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let proof = hex_encode(&crypto::session_proof(&token_bytes, &nonce2_bytes, &tr2));

        let reply = m.handle(&id("A"), AuthRequest::Proof2(proof), t(2001));
        assert_eq!(reply, AuthReply::Authorized2);
        assert!(m.is_authorized(&id("A")));
    }

    /// v1 `same_proof_replayed_against_same_nonce_fails`, v2 `Code2`
    /// `v2_code2_replay_after_success_is_rejected` 와 같은 이유 — 논스는
    /// 성공/실패와 무관하게 1회용이다. 캡처한 PROOF2 를 그대로 재전송해도
    /// 두 번째부터는 통과하면 안 된다(도청자가 첫 성공 응답을 그대로
    /// 재생하는 시나리오).
    #[test]
    fn v2_proof2_replay_after_success_is_rejected() {
        let mut m = PairingManager::new();
        let token = "aa".repeat(16);
        m.load_peers(vec![(token.clone(), 900)]);
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Auth2(hex_encode(&c.public)), t(1001));
        let AuthReply::Nonce2 { epk, nonce } = reply else {
            panic!("Nonce2 를 기대했다: {reply:?}")
        };
        let (_ss, tr) = c.agree(&epk);
        let token_bytes = PairingManager::hex_decode(&token).unwrap();
        let nonce_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let proof = hex_encode(&crypto::session_proof(&token_bytes, &nonce_bytes, &tr));

        let first = m.handle(&id("A"), AuthRequest::Proof2(proof.clone()), t(1002));
        assert_eq!(first, AuthReply::Authorized2, "{first:?}");

        let replay = m.handle(&id("A"), AuthRequest::Proof2(proof), t(1003));
        assert!(
            matches!(replay, AuthReply::Rejected),
            "성공 후 캡처한 PROOF2 를 재전송해도 거부돼야 한다: {replay:?}"
        );
    }

    /// `v2_proof2_replay_after_success_is_rejected` 의 반대쪽 절반 — 성공
    /// 뒤가 아니라 **실패 뒤** 재시도를 막는지 확인한다. 논스는 성공·실패와
    /// 무관하게 1회용이므로, 첫 `PROOF2`가 틀린 값이었어도 같은 논스로
    /// 두 번째에 올바른 proof 를 보내면 거부돼야 한다 — "실패하면 그 논스는
    /// 아직 안 썼으니 남겨 둔다"는 식의 리팩터링(소비를 성공 경로로만
    /// 옮기는 것)이 들어오면 이 테스트가 잡아낸다. 재시도하려면 새
    /// `AUTH2` 부터 다시 해야 한다.
    #[test]
    fn v2_correct_proof_after_a_failed_one_is_rejected() {
        let mut m = PairingManager::new();
        let token = "aa".repeat(16);
        m.load_peers(vec![(token.clone(), 900)]);
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Auth2(hex_encode(&c.public)), t(1001));
        let AuthReply::Nonce2 { epk, nonce } = reply else {
            panic!("Nonce2 를 기대했다: {reply:?}")
        };
        let (_ss, tr) = c.agree(&epk);
        let token_bytes = PairingManager::hex_decode(&token).unwrap();
        let nonce_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let correct_proof = hex_encode(&crypto::session_proof(&token_bytes, &nonce_bytes, &tr));

        let bogus = m.handle(&id("A"), AuthRequest::Proof2("00".repeat(32)), t(1002));
        assert!(matches!(bogus, AuthReply::Rejected), "{bogus:?}");

        // 같은 논스에 대한 진짜 proof 를 뒤늦게 보내도 거부돼야 한다 —
        // 실패한 시도가 논스를 살려 두면 안 된다.
        let second = m.handle(&id("A"), AuthRequest::Proof2(correct_proof), t(1003));
        assert!(
            matches!(second, AuthReply::Rejected),
            "실패한 PROOF2 뒤에 같은 논스로 온 올바른 proof 도 거부돼야 한다: {second:?}"
        );
    }

    /// v1 과 같다 — proof 실패는 코드 추측이 아니므로 예산을 소모하지 않는다.
    #[test]
    fn v2_bad_proof_does_not_spend_an_attempt() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        m.load_peers(vec![("aa".repeat(16), 900)]);
        let c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Auth2(hex_encode(&c.public)), t(1001));
        assert!(matches!(reply, AuthReply::Nonce2 { .. }));
        let reply = m.handle(&id("A"), AuthRequest::Proof2("00".repeat(32)), t(1002));
        assert!(matches!(reply, AuthReply::Rejected));
        match m.pairing_window(t(1003)) {
            PairingWindow::Open { attempts_left, .. } => assert_eq!(attempts_left, MAX_ATTEMPTS),
            other => panic!("창이 열려 있어야 한다: {other:?}"),
        }
    }

    /// **transcript 바인딩이 실제로 동작하는지** 확인한다. 중간자가 임시 키를
    /// 바꿔치기한 상황을 흉내낸다 — 올바른 토큰으로 만든 proof 라도 transcript
    /// 가 다르면 통과하면 안 된다.
    #[test]
    fn v2_proof_from_a_different_transcript_is_rejected() {
        let mut m = PairingManager::new();
        let token = "aa".repeat(16);
        m.load_peers(vec![(token.clone(), 900)]);
        let c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Auth2(hex_encode(&c.public)), t(1001));
        let AuthReply::Nonce2 { nonce, .. } = reply else { panic!() };

        // 서버가 준 epk 가 아니라 엉뚱한 키로 transcript 를 만든다.
        let bogus_tr = crypto::transcript(&c.public, &[0x77u8; 32]);
        let token_bytes = PairingManager::hex_decode(&token).unwrap();
        let nonce_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let proof = hex_encode(&crypto::session_proof(&token_bytes, &nonce_bytes, &bogus_tr));

        assert!(matches!(
            m.handle(&id("A"), AuthRequest::Proof2(proof), t(1002)),
            AuthReply::Rejected
        ));
    }

    /// AUTH2 없이 온 PROOF2 는 핸드셰이크가 없으므로 거부한다.
    #[test]
    fn v2_proof_without_auth_is_rejected() {
        let mut m = PairingManager::new();
        m.load_peers(vec![("aa".repeat(16), 900)]);
        assert!(matches!(
            m.handle(&id("A"), AuthRequest::Proof2("00".repeat(32)), t(1000)),
            AuthReply::Rejected
        ));
    }

    /// 인가가 사라지면 봉인 채널도 사라져야 한다 — 남으면 해제된 기기에
    /// 계속 봉인해 보낼 수 있다.
    #[test]
    fn v2_ending_a_session_drops_its_channel() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, _) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        m.handle(&id("A"), AuthRequest::Code2(cbind), t(1002));
        assert!(m.channel_mut(&id("A")).is_some());

        m.end_session(&id("A"));
        assert!(m.channel_mut(&id("A")).is_none(), "세션이 끝나면 채널도 없어야 한다");
    }

    /// 수정 라운드 1 회귀 테스트: `handshakes` 맵에 섞여 들어오는 두 흐름
    /// (`HELLO2` 페어링·`AUTH2` 재연결)이 **서로 다른 TTL** 로 스윕되는지
    /// 확인한다.
    ///
    /// `Auth2` 핸드셰이크의 `PROOF2` 응답만 보면 안 되는 이유는 여전하다
    /// — `Auth2` 는 `nonces` 에도 항목을 남기고, 그 TTL(`NONCE_TTL`, 30초)이
    /// 지금은 핸드셰이크 TTL 과 **똑같아서**(둘 다 `NONCE_TTL`), `Proof2`
    /// 가 `Rejected` 를 내는 것만으로는 핸드셰이크 스윕이 실제로 돌았는지
    /// 논스 스윕과 구분할 수 없다. 그래서 `handshake_count()` 로 맵 크기를
    /// 직접 들여다본다. 동시에 **재연결에는 열린 페어링 창이 필요 없다**
    /// 는 성질을 이용해, 같은 시각에 `HELLO2`(사람 속도, `CODE_TTL`)로
    /// 만든 핸드셰이크는 40초 뒤에도 살아 있어야 한다는 대조 assertion을
    /// 같이 둔다 — 이게 있어야 "핸드셰이크가 다 같이 지워지는지"가 아니라
    /// "종류별로 다른 TTL 이 실제로 적용되는지"를 증명한다.
    #[test]
    fn v2_handshake_ttl_differs_by_kind() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));

        // central A: HELLO2(페어링) 핸드셰이크 — 사람 속도, CODE_TTL(120초).
        let mut ca = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&ca.public)), t(1000));
        let (epk_a, _nonce_a) = epk_and_nonce(&reply);
        let (_ss_a, tr_a) = ca.agree(&epk_a);

        // central B: AUTH2(재연결) 핸드셰이크 — 기계 속도, NONCE_TTL(30초).
        let cb = V2Client::new();
        m.handle(&id("B"), AuthRequest::Auth2(hex_encode(&cb.public)), t(1000));

        assert_eq!(m.handshake_count(), 2, "두 핸드셰이크 모두 등록돼 있어야 한다");

        // 40초 뒤: NONCE_TTL(30초)은 지났지만 CODE_TTL(120초)은 아직이다.
        let reply = m.handle(&id("B"), AuthRequest::Proof2("00".repeat(32)), t(1040));
        assert!(
            matches!(reply, AuthReply::Rejected),
            "AUTH2 핸드셰이크는 이미 만료돼 있어야 한다: {reply:?}"
        );
        assert_eq!(
            m.handshake_count(),
            1,
            "만료된 AUTH2(central B) 핸드셰이크만 스윕돼야 한다 — HELLO2(central A) 는 남아야 한다"
        );

        // 대조군: 같은 시각(t=1000)에 만든 HELLO2(central A) 핸드셰이크는
        // 40초 뒤에도 아직 살아 있어야 한다 — CODE2 를 실제로 완주해서
        // 증명한다(핸드셰이크가 없으면 CODE2 는 Rejected 다).
        let cbind = hex_encode(&crypto::code_binding(&code, &tr_a));
        let reply = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1040));
        assert!(
            matches!(reply, AuthReply::Granted2 { .. }),
            "HELLO2 핸드셰이크는 CODE_TTL(120초) 안이라 아직 살아 있어야 한다: {reply:?}"
        );
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
    fn parses_v2_request_forms() {
        assert!(matches!(
            parse_auth_request(b"HELLO2:aabb"),
            AuthRequest::Hello2(p) if p == "aabb"
        ));
        assert!(matches!(
            parse_auth_request(b"CODE2:ccdd"),
            AuthRequest::Code2(p) if p == "ccdd"
        ));
        assert!(matches!(
            parse_auth_request(b"AUTH2:eeff"),
            AuthRequest::Auth2(p) if p == "eeff"
        ));
        assert!(matches!(
            parse_auth_request(b"PROOF2:0011"),
            AuthRequest::Proof2(p) if p == "0011"
        ));
    }

    /// v1 동사가 그대로 남아야 한다 — 이미 페어링된 아이폰이 전환 기간에
    /// 끊기지 않는 근거다(스펙 8장).
    #[test]
    fn v1_verbs_still_parse_alongside_v2() {
        assert!(matches!(parse_auth_request(b"HELLO"), AuthRequest::Hello));
        assert!(matches!(parse_auth_request(b"AUTH"), AuthRequest::Auth));
        assert!(matches!(parse_auth_request(b"CODE:123456"), AuthRequest::Code(c) if c == "123456"));
    }

    /// `HELLO2` 는 접두사 뒤에 공개키가 반드시 와야 한다. 콜론 없는 `HELLO2`
    /// 가 `HELLO` 로 오인되면 v2 클라이언트가 조용히 v1 경로로 떨어진다.
    #[test]
    fn hello2_without_payload_is_malformed() {
        assert!(matches!(parse_auth_request(b"HELLO2"), AuthRequest::Malformed));
        assert!(matches!(parse_auth_request(b"AUTH2"), AuthRequest::Malformed));
        assert!(matches!(parse_auth_request(b"CODE2"), AuthRequest::Malformed));
        assert!(matches!(parse_auth_request(b"PROOF2"), AuthRequest::Malformed));
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

    // ── 두 전송이 공유하는 창 (2026-08-25 스펙 9장) ──

    /// 이 설계의 **핵심 보안 성질**: 시도 예산은 창 하나에 묶여 있으므로,
    /// 공격자가 BLE 와 네트워크로 나눠 들어와도 합쳐서 5회다. 전송마다 매니저를
    /// 따로 두면 5+5 가 되어 원 스펙 5.2 의 근거가 약해진다.
    #[test]
    fn attempt_budget_is_shared_across_transports() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));

        // BLE 쪽에서 3회, 네트워크 쪽에서 2회 — central 네임스페이스가 다르다.
        for (idx, expected_left) in [4u8, 3, 2].into_iter().enumerate() {
            let r = m.handle(&id(&format!("BLE-{idx}")), AuthRequest::Code(wrong_code(&code)), t(1000));
            assert!(matches!(r, AuthReply::Denied { left } if left == expected_left), "{r:?}");
        }
        for (idx, expected_left) in [1u8, 0].into_iter().enumerate() {
            let r = m.handle(&id(&format!("NET-{idx}")), AuthRequest::Code(wrong_code(&code)), t(1000));
            assert!(matches!(r, AuthReply::Denied { left } if left == expected_left), "{r:?}");
        }

        assert!(
            matches!(m.handle(&id("NET-9"), AuthRequest::Code(code), t(1001)), AuthReply::Rejected),
            "합쳐서 5회면 창이 소진된다 — 전송을 바꿔도 예산이 늘지 않는다"
        );
    }

    /// Mac 저장소가 하나이므로, BLE 의 `CODE:` 로 발급된 토큰을 네트워크 쪽
    /// `PROOF` 로 검증해도 통과한다. (폰이 전송을 가로지를 수 있다는 뜻은
    /// 아니다 — iOS Keychain 은 전송별로 분리돼 있다, 스펙 2장·10장)
    #[test]
    fn a_token_issued_on_one_transport_verifies_on_the_other() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("BLE-A"), AuthRequest::Code(code), t(1001))
        else {
            panic!("BLE 로 페어링")
        };

        // 같은 토큰으로 네트워크 쪽 central 이 재인증한다.
        let AuthReply::Nonce { nonce } = m.handle(&id("NET-B"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        assert_eq!(
            m.handle(&id("NET-B"), AuthRequest::Proof(compute_proof(&token, &nonce)), t(1003)),
            AuthReply::Authorized,
            "저장소가 하나이므로 전송이 달라도 같은 토큰으로 인증된다"
        );
    }

    /// 전송 하나를 끌 때, 그 전송이 서비스 중이던 central 만 정리해야 한다.
    /// BLE 와 네트워크가 하나의 PairingManager 를 공유하므로(2026-08-25 스펙 4장),
    /// end_all_sessions 를 쓰면 BLE 를 끄는 순간 네트워크 세션까지 죽는다.
    #[test]
    fn end_sessions_only_drops_the_given_centrals() {
        let mut m = PairingManager::new();
        let code_a = m.begin_pairing(t(1000));
        m.handle(&id("BLE-A"), AuthRequest::Code(code_a), t(1001));
        let code_b = m.begin_pairing(t(2000));
        m.handle(&id("NET-B"), AuthRequest::Code(code_b), t(2001));
        assert!(m.is_authorized(&id("BLE-A")));
        assert!(m.is_authorized(&id("NET-B")));

        m.end_sessions(&[id("BLE-A")]);

        assert!(!m.is_authorized(&id("BLE-A")), "넘긴 central 은 인가가 내려간다");
        assert!(
            m.is_authorized(&id("NET-B")),
            "다른 전송의 세션은 살아 있어야 한다 — 이게 end_all_sessions 와의 차이다"
        );
        assert_eq!(m.issued_peers().len(), 2, "토큰은 둘 다 남는다");
    }

    /// 없는 id 를 넘겨도 조용히 무시한다 — 앱은 언페어링된 central 이 어느
    /// 전송에 붙어 있었는지 모르는 채로 두 브릿지 모두에 넘기기 때문이다.
    #[test]
    fn end_sessions_ignores_unknown_centrals() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        m.handle(&id("A"), AuthRequest::Code(code), t(1001));

        m.end_sessions(&[id("NOBODY")]);

        assert!(m.is_authorized(&id("A")));
    }

    #[test]
    fn end_all_sessions_drops_authorization_but_keeps_tokens() {
        let mut m = PairingManager::new();
        let code_a = m.begin_pairing(t(1000));
        let AuthReply::Granted { token } = m.handle(&id("A"), AuthRequest::Code(code_a), t(1001))
        else {
            panic!()
        };
        assert!(m.is_authorized(&id("A")));

        m.end_all_sessions();

        assert!(!m.is_authorized(&id("A")), "공유를 끄면 세션 인가는 즉시 내려가야 한다");
        assert_eq!(
            m.issued_peers().len(),
            1,
            "저장된 토큰은 남아야 한다 — 다시 공유를 켜면 같은 토큰으로 재인가된다"
        );

        // 재인가 확인: 저장된 토큰으로 다시 붙으면(AUTH→PROOF) 통과해야 한다.
        let AuthReply::Nonce { nonce } = m.handle(&id("A"), AuthRequest::Auth, t(1002)) else {
            panic!()
        };
        let proof = compute_proof(&token, &nonce);
        assert_eq!(
            m.handle(&id("A"), AuthRequest::Proof(proof), t(1003)),
            AuthReply::Authorized
        );
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
            .join("../../docs/ble-protocol/golden/hmac-sample.json");
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
            PairingWindow::Open { expires_at, .. } => {
                assert_eq!(
                    expires_at,
                    1000 + CODE_TTL.as_secs(),
                    "만료 시각은 발급 시각 기준 절대값이라 조회 시점(now)이 지나도 변하지 않아야 한다"
                );
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

    // ---- Tauri IPC 로 나가는 JSON 모양 (5단계) ----

    /// `#[serde(tag = "kind", rename_all = "lowercase")]` 가 없으면 `Open{..}` 이
    /// `{"Open":{"code":...}}` 로 나가서 프론트의 `pairing_window.kind === "open"` 이
    /// 절대 매치하지 못한다 — 컴파일도 되고 다른 테스트도 안 걸리는 종류라 가장
    /// 놓치기 쉬운 지점이다. 이 테스트가 그 모양을 고정한다.
    #[test]
    fn pairing_window_serializes_with_kind_tag() {
        let open = PairingWindow::Open {
            code: "123456".to_string(),
            expires_at: 1_700_000_090,
            attempts_left: 5,
        };
        let json = serde_json::to_value(&open).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "open",
                "code": "123456",
                "expires_at": 1_700_000_090,
                "attempts_left": 5,
            }),
            "실제 JSON: {json}"
        );

        assert_eq!(
            serde_json::to_value(PairingWindow::Exhausted).unwrap(),
            serde_json::json!({ "kind": "exhausted" })
        );
        assert_eq!(
            serde_json::to_value(PairingWindow::Closed).unwrap(),
            serde_json::json!({ "kind": "closed" })
        );
    }

    #[test]
    fn paired_peer_serializes_as_plain_object() {
        let peer = PairedPeer {
            peer_id: "deadbeef".to_string(),
            paired_at: 1234,
            connected: true,
        };
        let json = serde_json::to_value(&peer).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "peer_id": "deadbeef", "paired_at": 1234, "connected": true })
        );
    }
}
