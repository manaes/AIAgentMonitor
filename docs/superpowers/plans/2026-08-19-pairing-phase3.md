# 페어링 (3단계) 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 페어링하지 않은 기기가 스냅샷을 한 바이트도 받지 못하게 한다 — 6자리 코드로 최초 인증하고, 발급 토큰으로 재접속한다.

**Architecture:** 인증 상태 기계와 코드·토큰 발급을 의존성 없는 순수 모듈(`pairing.rs`)로 분리해 시뮬레이터·CI에서 전부 검증한다. `BleBridge`는 인가된 central 목록만 `offer_frame` 에 넘기고, macOS 구현체는 Auth 특성 쓰기를 `PeripheralEvent` 로 올린다. iOS 는 Keychain 에 토큰을 두고 재접속 시 자동 제시하며, 없으면 코드 입력 화면을 띄운다.

**Tech Stack:** Rust (serde, dirs-next) · Swift 6 (UIKit, Security.framework) · Svelte 5 · Tuist 4.158.2

**Spec:** `docs/superpowers/specs/2026-08-18-ble-ios-mirror-design.md` (§4.1 UUID, §5 페어링과 보안)

## Global Constraints

- 프로토콜 버전 `PROTOCOL_VERSION = 1`. Auth 메시지도 이 계약 아래 있다.
- Auth 특성 UUID `1403603A-4C78-4899-A2B8-FDA198101900` — 스펙 4.1 값을 문자 그대로 쓴다. **Write + Notify** 속성.
- **코드 유효 120초, 시도 5회.** 6자리(100만 조합) 무차별 대입 차단 근거이므로 임의로 늘리지 않는다.
- 토큰은 **128비트**, 소문자 hex 32자.
- Mac 토큰 저장: `~/.config/ai-agent-monitor/ble-peers.json` (기존 `triggers.json` 과 같은 디렉토리 규약)
- iOS 토큰 저장: **Keychain** (`UserDefaults` 금지 — 백업·평문 노출)
- 인가되지 않은 central 에는 Snapshot 을 **한 바이트도** 보내지 않는다.
- BLE 공유 기본값 off 유지.
- Rust BLE 코드는 `#[cfg(target_os = "macos")]` 게이트. Windows 빌드가 깨지면 안 된다.
- 한글 주석, 한글 사용자 문구. Swift/Rust API 이름은 영어.
- iOS 배포 타깃 17.0. 시뮬레이터 목적지는 `'platform=iOS Simulator,name=iPhone 16,OS=18.5'`.
- Swift 튜플은 `Equatable` 을 만족하지 못한다 — `XCTAssertTrue(a == b)` 를 쓴다.
- Tuist 4.158.2 는 소스 글롭 디렉토리가 없으면 `generate` 가 실패하고, 테스트 타깃 스킴을 자동 생성하지 않는다.
- 숫자 포맷은 전부 `MirrorFormat` 경유(14,540건 골든 표로 고정). `src/lib/format.ts` 를 고치면 `docs/ble-protocol/golden/check-parity-drift.sh` 를 돌린다.

### 상태 기계 (스펙 5.1 그대로)

> **이 절은 3단계 진행 중 두 번 개정됐다. 최신 정본은 스펙 §5.1 이며, Task 1 은 이미 그에 맞춰
> 구현·검증 완료(커밋 `94c5af7`)다. Task 3 이후를 구현하는 사람은 아래 요약과 실제 소스
> (`src-tauri/src/ble/pairing.rs`)를 보라.**

```
사용자가 Mac Devices 탭에서 [페어링 시작] 클릭
    → begin_pairing(now): 6자리 코드 + 120초 창 + 시도 5회
    → 코드는 Mac 화면에만 표시된다 (BLE 로 절대 나가지 않는다)

[미인가] --write(HELLO)--------> 창이 열려 있으면 AwaitingCode, 없으면 Rejected
[코드 대기] --write(CODE:123456)--> 성공 → Granted{token} → [인가]
                                   실패 → Denied{left}, 5회 소진 시 창 폐기

재연결(토큰은 링크를 타지 않는다):
[미인가] --write(AUTH)---------> Nonce{nonce}   (128비트, 30초, 1회용)
[논스 수신] --write(PROOF:<hex>)--> HMAC-SHA256(key=토큰, msg=논스) 검증
                                   일치 → Authorized (필드 없음) → [인가]
```

**왜 사용자 제스처인가**: 시도 예산을 코드에 묶으면 `HELLO` 재발급으로 리셋돼 약 9시간에 뚫린다.
**왜 챌린지-응답인가**: 토큰을 재연결마다 보내면 근접 스니핑 1회로 영구 접근권이 넘어간다.

### 명시된 한계 (스펙 5.2)

BLE 링크 자체는 암호화하지 않는다. 인가된 세션의 트래픽은 근접 스니핑에 노출될 수 있으며, 전달 데이터가 사용률과 프로젝트 이름 수준이고 자격증명을 포함하지 않는다는 판단하에 수용한다. **이 계획은 그 한계를 바꾸지 않는다.**

---

## File Structure

| 파일 | 책임 | 순수성 |
|---|---|---|
| `src-tauri/src/ble/pairing.rs` | 코드 생성·만료·시도 제한, 토큰 발급·검증, central 별 인증 상태 | 순수 · 단위 테스트 |
| `src-tauri/src/ble/peers.rs` | `ble-peers.json` 로드·저장 | 파일 I/O · tempfile 테스트 |
| `ios/Sources/BLETransport/PairingClient.swift` | Auth 프레임 인코딩·디코딩, 클라이언트 측 상태 | 순수 · 단위 테스트 |
| `ios/Sources/BLETransport/TokenStore.swift` | Keychain 읽기·쓰기·삭제 | Security.framework |
| `ios/Sources/MirrorFeature/PairingViewController.swift` | 6자리 코드 입력 화면 | UIKit |

**수정**: `src-tauri/src/ble/peripheral.rs`(이벤트 2종·트레이트 1개), `src-tauri/src/ble/mod.rs`(인가 필터), `src-tauri/src/ble/macos.rs`(Auth 특성·쓰기 콜백), `src-tauri/src/lib.rs`(상태·명령), `src/lib/tauri.ts`, `src/components/DevicePanel.svelte`, `ios/Sources/BLETransport/BLEClient.swift`, `ios/Sources/BLETransport/ConnectionState.swift`, `ios/Sources/MirrorFeature/MirrorViewController.swift`, `ios/Project.swift`, `docs/ble-protocol/DEVICE-TEST.md`

---

## Task 1: 인증 상태 기계 (`ble/pairing.rs`)

> ✅ **완료 (커밋 `94c5af7`).** 아래 본문은 최초 계획이며, 보안 검토 결과 설계가 두 번 바뀌었다
> (사용자 제스처 게이팅, 챌린지-응답). **실제 구현과 다르므로 참고용으로만 읽고, 인터페이스는
> `src-tauri/src/ble/pairing.rs` 를 정본으로 삼으라.** 최종 공개 API:
> `begin_pairing`, `handle`, `is_authorized`, `state`, `end_session`, `revoke_token`,
> `revoke_all`, `visible_code`, `load_tokens`, `issued_tokens`, `parse_auth_request`.

**Files:**
- Create: `src-tauri/src/ble/pairing.rs`
- Modify: `src-tauri/src/ble/mod.rs` (모듈 선언 1줄)

**Interfaces:**
- Consumes: `crate::ble::peripheral::CentralId`
- Produces: `AuthState` (enum: `Unauthorized`, `AwaitingCode`, `Authorized`), `AuthRequest` (enum: `Hello`, `Code(String)`, `Token(String)`, `Malformed`), `AuthReply` (enum: `CodeIssued { code: String }`, `Granted { token: String }`, `Denied { left: u8 }`, `Rejected`), `PairingManager::new(clock: fn() -> SystemTime)`, `PairingManager::handle(&mut self, id: &CentralId, req: AuthRequest, now: SystemTime) -> AuthReply`, `PairingManager::is_authorized(&self, id: &CentralId) -> bool`, `PairingManager::forget(&mut self, id: &CentralId)`, `PairingManager::visible_code(&self, now: SystemTime) -> Option<String>`, `PairingManager::load_tokens(&mut self, tokens: Vec<String>)`, `PairingManager::issued_tokens(&self) -> Vec<String>`, `parse_auth_request(bytes: &[u8]) -> AuthRequest`
- 상수: `CODE_TTL: Duration = 120초`, `MAX_ATTEMPTS: u8 = 5`

- [ ] **Step 1: 모듈을 선언한다**

`src-tauri/src/ble/mod.rs` 의 `pub mod peripheral;` 아래에 추가:
```rust
pub mod pairing;
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/pairing.rs` 하단에:
```rust
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
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::pairing`
Expected: FAIL — `cannot find type PairingManager`

실제 출력을 보고서에 붙인다.

- [ ] **Step 4: 최소 구현을 쓴다**

`src-tauri/src/ble/pairing.rs` 상단에:
```rust
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
                });
                AuthReply::CodeIssued { code }
            }
            AuthRequest::Code(given) => {
                let Some(p) = self.pending.as_mut() else {
                    return AuthReply::Rejected;
                };
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
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::pairing`
Expected: PASS — 13 tests

- [ ] **Step 6: 커밋한다**

```bash
git add src-tauri/src/ble/pairing.rs src-tauri/src/ble/mod.rs
git commit -m "feat(ble): 페어링 인증 상태 기계 추가"
```

---

## Task 2: 토큰 영속화 (`ble/peers.rs`)

**Files:**
- Create: `src-tauri/src/ble/peers.rs`
- Modify: `src-tauri/src/ble/mod.rs`

**Interfaces:**
- Consumes: 없음
- Produces: `PeerStore::path() -> PathBuf`, `PeerStore::load_from(path: &Path) -> Vec<String>`, `PeerStore::save_to(path: &Path, tokens: &[String]) -> anyhow::Result<()>`

- [ ] **Step 1: 모듈을 선언한다**

`src-tauri/src/ble/mod.rs` 에 추가:
```rust
pub mod peers;
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/peers.rs` 하단에:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_follows_existing_config_convention() {
        let p = PeerStore::path();
        assert!(p.ends_with("ai-agent-monitor/ble-peers.json"),
                "triggers.json 과 같은 디렉토리여야 한다: {p:?}");
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        assert!(PeerStore::load_from(&p).is_empty(), "없는 파일은 빈 목록");
    }

    #[test]
    fn round_trips_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        let tokens = vec!["a".repeat(32), "b".repeat(32)];
        PeerStore::save_to(&p, &tokens).unwrap();
        let back = PeerStore::load_from(&p);
        assert_eq!(back, tokens);
    }

    #[test]
    fn corrupt_file_loads_as_empty_rather_than_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        std::fs::write(&p, b"{ this is not json").unwrap();
        assert!(PeerStore::load_from(&p).is_empty(),
                "손상된 파일 때문에 앱이 죽으면 안 된다 — 빈 목록으로 시작한다");
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/deeper/ble-peers.json");
        PeerStore::save_to(&p, &["c".repeat(32)]).unwrap();
        assert_eq!(PeerStore::load_from(&p).len(), 1);
    }

    #[test]
    fn save_is_atomic_leaving_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        PeerStore::save_to(&p, &["d".repeat(32)]).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "ble-peers.json")
            .collect();
        assert!(leftovers.is_empty(), "임시 파일이 남으면 안 된다: {leftovers:?}");
    }
}
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::peers`
Expected: FAIL — `cannot find type PeerStore`

- [ ] **Step 4: 최소 구현을 쓴다**

`src-tauri/src/ble/peers.rs` 상단에:
```rust
//! 페어링 토큰 영속화. 기존 `triggers.json` 과 같은 디렉토리 규약을 따른다.
//!
//! 저장은 임시 파일에 쓴 뒤 rename 한다. 쓰는 도중 앱이 죽어도 기존 파일이
//! 반쯤 덮인 채로 남지 않게 하기 위함이다 — 토큰이 깨지면 이미 페어링한
//! 기기가 전부 재페어링을 요구받는다.
use std::path::{Path, PathBuf};

pub struct PeerStore;

impl PeerStore {
    pub fn path() -> PathBuf {
        dirs_next::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ai-agent-monitor/ble-peers.json")
    }

    /// 없거나 손상된 파일은 빈 목록으로 취급한다. 여기서 죽으면 앱이 시작조차 못 한다.
    pub fn load_from(path: &Path) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        match serde_json::from_str::<Vec<String>>(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "ble-peers.json 파싱 실패, 빈 목록으로 시작");
                Vec::new()
            }
        }
    }

    pub fn save_to(path: &Path, tokens: &[String]) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(tokens)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::peers`
Expected: PASS — 6 tests

- [ ] **Step 6: 커밋한다**

```bash
git add src-tauri/src/ble/peers.rs src-tauri/src/ble/mod.rs
git commit -m "feat(ble): 페어링 토큰 영속화 추가"
```

---

## Task 3: 인가 필터를 `BleBridge` 에 넣는다

지금은 구독한 **모든** central 이 스냅샷을 받는다. 이 태스크가 그것을 인가된 central 로 좁힌다.

**Files:**
- Modify: `src-tauri/src/ble/peripheral.rs` (이벤트 2종 · 트레이트 메서드 2개 · Fake 확장)
- Modify: `src-tauri/src/ble/mod.rs` (`BleBridge` 인가 연동)

**Interfaces:**
- Consumes: `pairing::{PairingManager, AuthRequest, AuthReply, parse_auth_request}`, `peers::PeerStore`
- Produces: `PeripheralEvent::AuthWrite { central: CentralId, data: Vec<u8> }`, `PeripheralEvent::Disconnected(CentralId)`, `BlePeripheral::notify_auth(&self, central: &CentralId, payload: Vec<u8>)`, `BlePeripheral::authorized_subscribers(&self, is_authorized: &dyn Fn(&CentralId) -> bool) -> Vec<Subscriber>`, `BleBridge::handle_auth(&mut self, central: &CentralId, data: &[u8], now: SystemTime) -> Option<Vec<String>>`, `BleBridge::visible_pairing_code(&self, now: SystemTime) -> Option<String>`, `BleBridge::unpair_all(&mut self)`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/mod.rs` 의 테스트 모듈에 추가:
```rust
    #[test]
    fn unauthorized_subscriber_gets_nothing() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));
        assert!(fake.taken_frames().is_empty(),
                "페어링하지 않은 기기는 한 바이트도 받으면 안 된다");
    }

    #[test]
    fn authorized_subscriber_receives_snapshot() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);

        // 사용자가 창을 연다 → HELLO → 그 코드로 인가
        let code = b.begin_pairing(now);
        b.handle_auth(&CentralId("A".into()), b"HELLO", now);
        b.handle_auth(&CentralId("A".into()), format!("CODE:{code}").as_bytes(), now);

        b.on_snapshot(&snap(1.0, 1000), now);
        assert_eq!(fake.taken_frames().len(), 1, "인가 후에는 받는다");
    }

    #[test]
    fn mixed_subscribers_only_authorized_are_targeted() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("A".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("B".into()), max_notify_len: 23 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let code = b.begin_pairing(now);
        b.handle_auth(&CentralId("A".into()), b"HELLO", now);
        b.handle_auth(&CentralId("A".into()), format!("CODE:{code}").as_bytes(), now);

        b.on_snapshot(&snap(1.0, 1000), now);
        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1);
        // 청크 크기는 **인가된** 구독자만 보고 정해야 한다.
        // 미인가 B(23)를 섞으면 청크가 불필요하게 잘게 쪼개진다.
        assert!(frames[0].1[0].len() > 23, "미인가 구독자의 MTU 에 끌려가면 안 된다");
    }

    #[test]
    fn handle_auth_returns_tokens_to_persist_only_on_grant() {
        let (mut b, _fake) = bridge();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        assert_eq!(b.handle_auth(&CentralId("A".into()), b"HELLO", now), None,
                   "코드 발급만으로는 저장할 것이 없다");
        let code = b.visible_pairing_code(now).unwrap();
        let saved = b.handle_auth(&CentralId("A".into()), format!("CODE:{code}").as_bytes(), now);
        assert_eq!(saved.map(|v| v.len()), Some(1), "인가되면 토큰 목록을 돌려준다");
    }

    #[test]
    fn unpair_all_revokes_everyone() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let code = b.begin_pairing(now);
        b.handle_auth(&CentralId("A".into()), b"HELLO", now);
        b.handle_auth(&CentralId("A".into()), format!("CODE:{code}").as_bytes(), now);
        b.on_snapshot(&snap(1.0, 1000), now);
        assert_eq!(fake.taken_frames().len(), 1);

        b.unpair_all();
        b.on_snapshot(&snap(2.0, 1000), now + Duration::from_secs(2));
        assert!(fake.taken_frames().is_empty(), "해제 후에는 다시 아무것도 못 받는다");
    }
```

- [ ] **Step 2: 테스트가 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::tests`
Expected: FAIL — `no method named handle_auth`

- [ ] **Step 3: `peripheral.rs` 를 확장한다**

`PeripheralEvent` 에 두 variant 를 추가한다:
```rust
    /// central 이 Auth 특성에 무언가 썼다. 해석은 pairing 모듈이 한다.
    AuthWrite { central: CentralId, data: Vec<u8> },
    /// 링크가 끊겼다. 인가 상태를 그 자리에서 지우기 위해 필요하다.
    Disconnected(CentralId),
```

`BlePeripheral` 에 두 메서드를 추가한다:
```rust
    /// Auth 특성으로 한 central 에만 응답한다.
    fn notify_auth(&self, central: &CentralId, payload: Vec<u8>);

    /// 인가된 구독자만 추린다. 청크 크기 계산도 이 목록으로 해야
    /// 미인가 기기의 작은 MTU 에 끌려가지 않는다.
    fn authorized_subscribers(&self, is_authorized: &dyn Fn(&CentralId) -> bool) -> Vec<Subscriber> {
        self.subscribers()
            .into_iter()
            .filter(|s| is_authorized(&s.id))
            .collect()
    }
```

`FakePeripheral` 에 `notify_auth` 구현과 기록용 접근자를 더한다:
```rust
    auth_replies: Mutex<Vec<(CentralId, Vec<u8>)>>,
```
```rust
    pub fn taken_auth_replies(&self) -> Vec<(CentralId, Vec<u8>)> {
        std::mem::take(&mut *self.auth_replies.lock().unwrap())
    }
```
```rust
    fn notify_auth(&self, central: &CentralId, payload: Vec<u8>) {
        self.auth_replies.lock().unwrap().push((central.clone(), payload));
    }
```

- [ ] **Step 4: `BleBridge` 에 인가를 연동한다**

`BleBridge` 에 필드를 추가한다:
```rust
    pairing: pairing::PairingManager,
```
`new` 에서 `pairing: pairing::PairingManager::new()` 로 초기화한다.

`on_snapshot` 의 구독자 조회를 인가된 목록으로 바꾼다:
```rust
        // 인가된 구독자만 대상으로 삼는다. 미인가 기기가 붙어 있어도
        // 스냅샷은 만들지 않는다(스펙 5.1).
        let authorized = self
            .peripheral
            .authorized_subscribers(&|id| self.pairing.is_authorized(id));
        let Some(max_chunk) = authorized.iter().map(|s| s.max_notify_len).min() else {
            return;
        };
```

메서드 3개를 추가한다:
```rust
    /// Auth 특성 쓰기를 처리하고 응답을 보낸다.
    /// 인가가 성립하면 영속화할 토큰 목록을 돌려준다(호출부가 저장한다).
    pub fn handle_auth(
        &mut self,
        central: &CentralId,
        data: &[u8],
        now: SystemTime,
    ) -> Option<Vec<String>> {
        let req = pairing::parse_auth_request(data);
        let reply = self.pairing.handle(central, req, now);
        let payload = match &reply {
            // 코드는 Mac 화면에만 보여준다 — central 에게 보내면 페어링이 무의미해진다.
            pairing::AuthReply::AwaitingCode => br#"{"ok":false,"await":"code"}"#.to_vec(),
            pairing::AuthReply::Nonce { nonce } => format!(r#"{{"ok":false,"nonce":"{nonce}"}}"#).into_bytes(),
            pairing::AuthReply::Authorized => br#"{"ok":true}"#.to_vec(),
            pairing::AuthReply::Granted { token } => {
                format!(r#"{{"ok":true,"token":"{token}"}}"#).into_bytes()
            }
            pairing::AuthReply::Denied { left } => {
                format!(r#"{{"ok":false,"left":{left}}}"#).into_bytes()
            }
            pairing::AuthReply::Rejected => br#"{"ok":false}"#.to_vec(),
        };
        self.peripheral.notify_auth(central, payload);

        match reply {
            pairing::AuthReply::Granted { .. } => Some(self.pairing.issued_tokens()),
            _ => None,
        }
    }

    pub fn visible_pairing_code(&self, now: SystemTime) -> Option<String> {
        self.pairing.visible_code(now)
    }

    /// 링크가 끊긴 central 의 인가를 지운다. 같은 식별자가 재사용될 수 있으므로
    /// 연결 단위 인가는 연결이 끝나면 사라져야 한다.
    pub fn forget_central(&mut self, central: &CentralId) {
        self.pairing.end_session(central);
    }

    /// 저장된 토큰을 복원한다.
    pub fn load_tokens(&mut self, tokens: Vec<String>) {
        self.pairing.load_tokens(tokens);
    }

    /// 모든 기기의 인가와 토큰을 폐기한다.
    pub fn unpair_all(&mut self) {
        self.pairing = pairing::PairingManager::new();
    }
```

`use` 에 `pairing` 을 더한다.

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::`
Expected: PASS — 기존 테스트 전부 + 신규 5개

기존 `emits_chunked_snapshot_frame` 등은 이제 인가를 거쳐야 통과한다. 브리프의 새 테스트가
그 경로를 보여주므로, 기존 테스트도 같은 방식으로 인가를 붙여 고친다. **테스트를 지우지 말고 고친다.**

- [ ] **Step 6: 커밋한다**

```bash
git add src-tauri/src/ble/peripheral.rs src-tauri/src/ble/mod.rs
git commit -m "feat(ble): 인가된 central 에만 스냅샷을 보내도록 제한"
```

---

## Task 4: macOS 쪽 Auth 특성과 쓰기 콜백

**Files:**
- Modify: `src-tauri/src/ble/macos.rs`

**Interfaces:**
- Consumes: `CharId::Auth`, `PeripheralEvent::{AuthWrite, Disconnected}`
- Produces: `MacPeripheral` 이 `notify_auth` 를 구현하고 Auth 특성을 게시한다

- [ ] **Step 1: Auth 특성을 서비스에 추가한다**

`publish()` 에서 Snapshot 특성만 만들던 것을 Auth 도 만들도록 바꾼다:
```rust
        // Auth 는 central 이 쓰고(Write) Mac 이 답한다(Notify).
        let auth_ch: Retained<CBMutableCharacteristic> = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &uuid(CharId::Auth.uuid()),
                CBCharacteristicProperties::Write | CBCharacteristicProperties::Notify,
                None,
                CBAttributePermissions::Writeable,
            )
        };
```
`chars` 맵에 `CharId::Auth.uuid()` 로 넣고, 서비스의 characteristics 배열에 두 특성을 함께 담는다.

- [ ] **Step 2: 쓰기 콜백을 구현한다**

델리게이트에 추가한다:
```rust
        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_writes(&self, mgr: &CBPeripheralManager, requests: &NSArray<CBATTRequest>) {
            for i in 0..requests.count() {
                let req = requests.objectAtIndex(i);
                let central = unsafe { req.central() };
                let data = unsafe { req.value() }
                    .map(|d| d.to_vec())
                    .unwrap_or_default();
                let _ = self.ivars().borrow().events.send(PeripheralEvent::AuthWrite {
                    central: CentralId(central_id(&central)),
                    data,
                });
            }
            // 응답하지 않으면 iOS 쪽 write 가 타임아웃된다.
            if requests.count() > 0 {
                unsafe {
                    mgr.respondToRequest_withResult(&requests.objectAtIndex(0), CBATTError::Success)
                };
            }
        }
```

`did_unsubscribe` 에서 `PeripheralEvent::Disconnected` 도 함께 보낸다 — 인가는 연결 단위다.

- [ ] **Step 3: `notify_auth` 를 구현한다**

`MacPeripheral` 에 추가한다:
```rust
    fn notify_auth(&self, central: &CentralId, payload: Vec<u8>) {
        let target = central.clone();
        with_delegate(&self.app, move |d| {
            let (Some(mgr), Some(ch)) = (
                d.ivars().borrow().manager.clone(),
                d.ivars().borrow().chars.get(CharId::Auth.uuid()).cloned(),
            ) else {
                return;
            };
            let subs = d.ivars().borrow().subs.clone();
            let Some((c, _)) = subs.get(&target.0) else { return };
            let arr = NSArray::from_slice(&[&**c]);
            let data = NSData::with_bytes(&payload);
            unsafe {
                mgr.updateValue_forCharacteristic_onSubscribedCentrals(&data, &ch, Some(&arr))
            };
        });
    }
```

- [ ] **Step 4: 빌드와 기존 테스트를 확인한다**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: 컴파일 성공

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: 기존 테스트 전부 통과

- [ ] **Step 5: 커밋한다**

```bash
git add src-tauri/src/ble/macos.rs
git commit -m "feat(ble): macOS Auth 특성과 쓰기 콜백 구현"
```

---

## Task 5: `lib.rs` 연동과 Devices 탭 확장

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/lib/tauri.ts`
- Modify: `src/components/DevicePanel.svelte`

**Interfaces:**
- Consumes: `BleBridge::{begin_pairing, handle_auth, visible_pairing_code, forget_central, load_tokens, unpair_all}`, `PeerStore`
- Produces: `BleStatus.pairing_code: Option<String>`, `BleStatus.paired_count: usize`, Tauri commands `ble_begin_pairing()` · `ble_unpair_all()`

> **코드는 사용자가 [페어링 시작] 을 눌러야만 발급된다.** 이 버튼이 없으면 3단계의 보안 근거가
> 성립하지 않는다(스펙 §5.1). `ble_begin_pairing` 이 `BleBridge::begin_pairing` 을 호출하고,
> 반환된 코드는 `BleStatus.pairing_code` 로만 화면에 흐른다 — BLE 로는 절대 나가지 않는다.

- [ ] **Step 1: 시작 시 토큰을 복원한다**

`setup` 에서 `BleHandle` 을 만든 직후에:
```rust
                {
                    let tokens = ble::peers::PeerStore::load_from(&ble::peers::PeerStore::path());
                    if !tokens.is_empty() {
                        tracing::info!(count = tokens.len(), "ble-peers.json 로드");
                    }
                    ble_handle.bridge.blocking_lock().load_tokens(tokens);
                }
```

- [ ] **Step 2: Auth 이벤트를 처리한다**

이벤트 루프의 `match &ev` 에 팔을 추가한다:
```rust
                                ble::peripheral::PeripheralEvent::AuthWrite { central, data } => {
                                    let now = std::time::SystemTime::now();
                                    let saved = h.bridge.lock().await.handle_auth(central, data, now);
                                    if let Some(tokens) = saved {
                                        let path = ble::peers::PeerStore::path();
                                        if let Err(e) = ble::peers::PeerStore::save_to(&path, &tokens) {
                                            tracing::error!(%e, "ble-peers.json 저장 실패");
                                        }
                                    }
                                }
                                ble::peripheral::PeripheralEvent::Disconnected(central) => {
                                    h.bridge.lock().await.forget_central(central);
                                }
```

- [ ] **Step 3: `BleStatus` 를 확장한다**

```rust
    /// 지금 화면에 띄울 6자리 코드. 만료되면 None.
    pub pairing_code: Option<String>,
    /// 저장된 페어링 토큰 수.
    pub paired_count: usize,
```
`ble_status` 에서 채운다:
```rust
    let now = std::time::SystemTime::now();
    let pairing_code = bridge.visible_pairing_code(now);
```

- [ ] **Step 4: 해제 명령을 추가한다**

```rust
#[tauri::command]
async fn ble_unpair_all(state: tauri::State<'_, Arc<BleHandle>>) -> Result<(), String> {
    state.bridge.lock().await.unpair_all();
    let path = ble::peers::PeerStore::path();
    ble::peers::PeerStore::save_to(&path, &[]).map_err(|e| e.to_string())?;
    Ok(())
}
```
`generate_handler!` 목록에 `ble_unpair_all,` 을 더한다.

- [ ] **Step 5: 프론트 타입과 화면을 확장한다**

`src/lib/tauri.ts` 의 `BleStatus` 에:
```ts
  pairing_code: string | null;
  paired_count: number;
```
그리고:
```ts
export async function bleUnpairAll(): Promise<void> {
  return invoke<void>("ble_unpair_all");
}
```

`src/components/DevicePanel.svelte` 에서 1단계의 경고 문구를 페어링 안내로 교체한다:
```svelte
  {#if !store.ble?.pairing_code}
    <button class="inline-btn" onclick={() => store.beginPairing()}>페어링 시작</button>
  {/if}

  {#if store.ble?.pairing_code}
    <div class="code-box">
      <p class="code-label">iPhone 에 아래 6자리를 입력하세요 (120초)</p>
      <p class="code">{store.ble.pairing_code}</p>
    </div>
  {/if}
```
그리고 `1단계에는 기기 인증이 없습니다…` 경고를 지우고, 대신 페어링된 기기 수와 해제 버튼을 둔다:
```svelte
  {#if (store.ble?.paired_count ?? 0) > 0}
    <div class="row">
      <span class="subtle">페어링된 기기 {store.ble?.paired_count}대</span>
      <button class="inline-btn" onclick={() => store.unpairAll()}>전체 해제</button>
    </div>
  {/if}
```
`store.svelte.ts` 에 `unpairAll()` 을 더한다(기존 `setBleEnabled` 와 같은 try/catch 패턴).

- [ ] **Step 6: 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: 전부 통과

Run: `pnpm check` → 0 errors / `pnpm build` → 성공

- [ ] **Step 7: 커밋한다**

```bash
git add src-tauri/src/lib.rs src/lib/tauri.ts src/lib/store.svelte.ts src/components/DevicePanel.svelte
git commit -m "feat(ble): 페어링 코드 표시와 기기 해제 UI 추가"
```

---

## Task 6: iOS 페어링 프로토콜과 Keychain

**Files:**
- Create: `ios/Sources/BLETransport/PairingClient.swift`
- Create: `ios/Sources/BLETransport/TokenStore.swift`
- Create: `ios/Tests/BLETransportTests/PairingClientTests.swift`

**Interfaces:**
- Consumes: 없음(순수) · Security.framework
- Produces: `AuthReplyPayload` (Decodable: `ok: Bool`, `token: String?`, `left: Int?`, `awaiting: String?` ← JSON 키 `await`, `nonce: String?`), `PairingClient.helloFrame() -> Data`, `PairingClient.codeFrame(_ code: String) -> Data`, `PairingClient.authFrame() -> Data`, `PairingClient.proofFrame(token: String, nonce: String) -> Data?`, `PairingClient.parse(_ data: Data) -> AuthReplyPayload?`, `TokenStore.save(_ token: String)`, `TokenStore.load() -> String?`, `TokenStore.clear()`

> **`proofFrame` 은 HMAC-SHA256(key = 토큰 hex 를 디코드한 원시 바이트, msg = 논스 hex 를 디코드한
> 원시 바이트) 을 소문자 hex 로 만든다.** hex 문자열의 UTF-8 바이트로 계산하면 안 된다 — 그러면
> 페어링은 성공하는데 재연결마다 조용히 실패한다. `docs/ble-protocol/golden/hmac-sample.json` 이
> 이 계약을 고정하므로, 이 태스크의 테스트가 그 파일을 읽어 검증해야 한다. CryptoKit `HMAC<SHA256>` 사용.

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`ios/Tests/BLETransportTests/PairingClientTests.swift`:
```swift
import XCTest
@testable import BLETransport

final class PairingClientTests: XCTestCase {

    func testFrameEncodingMatchesRustParser() {
        XCTAssertEqual(String(data: PairingClient.helloFrame(), encoding: .utf8), "HELLO")
        XCTAssertEqual(String(data: PairingClient.codeFrame("123456"), encoding: .utf8), "CODE:123456")
        XCTAssertEqual(String(data: PairingClient.authFrame(), encoding: .utf8), "AUTH")
    }

    func testParsesGrant() {
        let d = Data(#"{"ok":true,"token":"deadbeef"}"#.utf8)
        let r = PairingClient.parse(d)
        XCTAssertEqual(r?.ok, true)
        XCTAssertEqual(r?.token, "deadbeef")
    }

    func testParsesDenialWithRemainingAttempts() {
        let d = Data(#"{"ok":false,"left":3}"#.utf8)
        let r = PairingClient.parse(d)
        XCTAssertEqual(r?.ok, false)
        XCTAssertEqual(r?.left, 3)
        XCTAssertNil(r?.token)
    }

    func testParsesAwaitingCode() {
        let d = Data(#"{"ok":false,"await":"code"}"#.utf8)
        let r = PairingClient.parse(d)
        XCTAssertEqual(r?.ok, false)
        XCTAssertEqual(r?.awaiting, "code")
    }

    func testParsesBareRejection() {
        let r = PairingClient.parse(Data(#"{"ok":false}"#.utf8))
        XCTAssertEqual(r?.ok, false)
        XCTAssertNil(r?.left)
    }

    func testMalformedPayloadReturnsNil() {
        XCTAssertNil(PairingClient.parse(Data("not json".utf8)))
    }

    func testTokenStoreRoundTrip() {
        TokenStore.clear()
        XCTAssertNil(TokenStore.load(), "지운 뒤에는 없어야 한다")
        TokenStore.save("cafebabe")
        XCTAssertEqual(TokenStore.load(), "cafebabe")
        TokenStore.save("f00dface")
        XCTAssertEqual(TokenStore.load(), "f00dface", "덮어쓰기가 되어야 한다")
        TokenStore.clear()
        XCTAssertNil(TokenStore.load())
    }
}
```

- [ ] **Step 2: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find 'PairingClient' in scope`

- [ ] **Step 3: 최소 구현을 쓴다**

`ios/Sources/BLETransport/PairingClient.swift`:
```swift
import Foundation

/// Mac 의 `src-tauri/src/ble/pairing.rs` 가 해석하는 형식과 정확히 맞춰야 한다.
/// 프레임은 평문이고 짧다 — 이 채널로 오가는 것은 코드와 토큰뿐이다.
public struct AuthReplyPayload: Decodable, Equatable, Sendable {
    public let ok: Bool
    public let token: String?
    public let left: Int?
    /// Rust 가 보내는 키는 `await` 인데 Swift 예약어라 이름을 바꿔 받는다.
    public let awaiting: String?

    private enum CodingKeys: String, CodingKey {
        case ok, token, left
        case awaiting = "await"
    }
}

public enum PairingClient {
    public static func helloFrame() -> Data { Data("HELLO".utf8) }
    public static func codeFrame(_ code: String) -> Data { Data("CODE:\(code)".utf8) }
    public static func authFrame() -> Data { Data("AUTH".utf8) }

    /// 논스에 대한 서명. **hex 문자열이 아니라 디코드한 원시 바이트**로 계산한다 —
    /// 이 계약은 docs/ble-protocol/golden/hmac-sample.json 이 고정한다.
    public static func proofFrame(token: String, nonce: String) -> Data? {
        guard let key = Data(hexString: token), let msg = Data(hexString: nonce) else { return nil }
        let mac = HMAC<SHA256>.authenticationCode(for: msg, using: SymmetricKey(data: key))
        let hex = mac.map { String(format: "%02x", $0) }.joined()
        return Data("PROOF:\(hex)".utf8)
    }

    public static func parse(_ data: Data) -> AuthReplyPayload? {
        try? JSONDecoder().decode(AuthReplyPayload.self, from: data)
    }
}
```

`ios/Sources/BLETransport/TokenStore.swift`:
```swift
import Foundation
import Security

/// 페어링 토큰 보관소. UserDefaults 는 백업에 평문으로 실려 나가므로 쓰지 않는다.
public enum TokenStore {
    private static let service = "com.dgitx.aiagentmonitor.mirror"
    private static let account = "ble-pairing-token"

    private static var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    public static func save(_ token: String) {
        clear()
        var q = baseQuery
        q[kSecValueData as String] = Data(token.utf8)
        // 기기가 잠긴 동안에는 읽히지 않아야 하고, 백업으로 다른 기기에 옮겨가서도 안 된다.
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        SecItemAdd(q as CFDictionary, nil)
    }

    public static func load() -> String? {
        var q = baseQuery
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    public static func clear() {
        SecItemDelete(baseQuery as CFDictionary)
    }
}
```

- [ ] **Step 4: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "Executed|TEST"
```
Expected: `Executed 19 tests, with 0 failures` (기존 12 + 7)

- [ ] **Step 5: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 페어링 프로토콜과 Keychain 토큰 보관소 추가"
```

---

## Task 7: `BLEClient` 인증 흐름과 페어링 화면

**Files:**
- Modify: `ios/Sources/BLETransport/ConnectionState.swift`
- Modify: `ios/Sources/BLETransport/BLEClient.swift`
- Create: `ios/Sources/MirrorFeature/PairingViewController.swift`
- Modify: `ios/Sources/MirrorFeature/MirrorViewController.swift`
- Modify: `ios/Tests/BLETransportTests/ConnectionStateTests.swift`

**Interfaces:**
- Consumes: `PairingClient`, `TokenStore`, `MirrorUUIDs.auth`
- Produces: `ConnectionState.needsPairing`, `ConnectionState.pairingFailed(left: Int)`, `BLEClient.submitPairingCode(_ code: String)`

- [ ] **Step 1: 상태 2종을 추가한다**

`ConnectionState.swift` 에:
```swift
    case needsPairing
    case pairingFailed(left: Int)
```
`label` 에:
```swift
        case .needsPairing: return "페어링 필요 · Mac 화면의 6자리 코드를 입력하세요"
        case .pairingFailed(let left): return "코드가 틀렸습니다 · \(left)회 남음"
```
`ConnectionStateTests` 에 두 라벨을 단언하는 테스트를 더한다.

- [ ] **Step 2: `BLEClient` 에 인증 단계를 넣는다**

특성 탐색을 Auth 까지 확장한다:
```swift
        peripheral.discoverCharacteristics([MirrorUUIDs.snapshot, MirrorUUIDs.auth], for: service)
```

Auth 특성을 찾으면 구독하고, **Snapshot 구독은 인가 후로 미룬다.** 저장된 토큰이 있으면 `AUTH` 로 논스를 요청하고, 없으면 `HELLO` 를 보낸 뒤 `.needsPairing` 으로 간다:
```swift
        if let token = TokenStore.load() {
            // 토큰 자체는 보내지 않는다. 논스를 받아 서명해 답한다.
            peripheral.writeValue(PairingClient.authFrame(), for: authCh, type: .withResponse)
        } else {
            peripheral.writeValue(PairingClient.helloFrame(), for: authCh, type: .withResponse)
        }
```

Auth notify 를 처리한다:
```swift
        guard let reply = PairingClient.parse(data) else { return }
        if reply.ok, let token = reply.token {
            TokenStore.save(token)
            peripheral.setNotifyValue(true, for: snapshotCh)   // 여기서 비로소 데이터가 흐른다
        } else if let left = reply.left {
            stateSubject.send(.pairingFailed(left: left))
        } else if reply.awaiting == "code" {
            stateSubject.send(.needsPairing)
        } else {
            // 논스를 받았으면 서명해 답한다. 서명이 거부되면 저장된 토큰이 폐기된 것이므로
            // 지우고 코드 페어링으로 되돌아간다.
            TokenStore.clear()
            peripheral.writeValue(PairingClient.helloFrame(), for: authCh, type: .withResponse)
        }
```

`submitPairingCode(_:)` 를 공개한다:
```swift
    public func submitPairingCode(_ code: String) {
        guard let p = peripheral, let ch = authCharacteristic else { return }
        p.writeValue(PairingClient.codeFrame(code), for: ch, type: .withResponse)
    }
```

- [ ] **Step 3: 페어링 화면을 만든다**

`ios/Sources/MirrorFeature/PairingViewController.swift` — 6자리 숫자 입력 필드, 안내 문구, 남은 시도 표시, 확인 버튼. `Palette`/`Typography` 를 쓰고 한글 문구를 쓴다. 입력이 6자리가 되면 확인 버튼을 활성화한다.

`MirrorViewController` 는 상태가 `.needsPairing` 또는 `.pairingFailed` 일 때 이 화면을 modal 로 띄우고, `.streaming` 이 되면 닫는다.

- [ ] **Step 4: 확인한다**

Run:
```bash
cd ios && tuist generate --no-open
cd ios && for S in WireTests BLETransportTests MirrorFormatTests DesignSystemTests MirrorFeatureTests; do
  printf "%s: " "$S"
  xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme $S \
    -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 \
    | grep -oE "Executed [0-9]+ tests?, with [0-9]+ failures?" | tail -1
done
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | grep -E "BUILD SUCCEEDED|BUILD FAILED"
```
Expected: 전부 통과, BUILD SUCCEEDED

- [ ] **Step 5: 실기기 절차 문서를 3단계 기준으로 갱신한다**

`docs/ble-protocol/DEVICE-TEST.md` 의 "페어링이 없다" 는 서술을 지우고, 페어링 확인 항목을 넣는다:
- Mac Devices 탭에 6자리 코드가 뜨고 120초 뒤 사라진다
- iPhone 에 코드 입력 화면이 뜨고, 맞으면 화면이 흐르기 시작한다
- 틀린 코드를 넣으면 남은 횟수가 줄고, 5회 소진 시 다시 시도해야 한다
- 앱을 껐다 켜도 코드 입력 없이 바로 연결된다
- Mac 에서 "전체 해제" 를 누르면 iPhone 이 다시 코드를 요구한다

- [ ] **Step 6: 커밋한다**

```bash
git add ios/ docs/ble-protocol/DEVICE-TEST.md
git commit -m "feat(ios): 페어링 화면과 인증 흐름 연결"
```

---

## Self-Review

**스펙 커버리지 (5장)**

| 스펙 항목 | 태스크 |
|---|---|
| 5.1 상태 기계 전체 | Task 1 (순수 구현 + 13 테스트) |
| 5.1 인가된 central 에만 notify | Task 3 (`authorized_subscribers` + 5 테스트) |
| 5.2 코드 120초 · 시도 5회 | Task 1 (`CODE_TTL`, `MAX_ATTEMPTS`, 경계 테스트) |
| 5.2 128비트 토큰 | Task 1 (hex 32자 단언) |
| 5.2 Mac 토큰 저장 위치 | Task 2 (`ble-peers.json`) |
| 5.2 iOS Keychain | Task 6 (`TokenStore`) |
| 5.2 기본값 off | 변경 없음 — 1단계에서 이미 그렇다 |
| 5.3 macOS 권한 | 변경 없음 — 1단계에서 `Info.plist` 추가 완료 |
| Devices 탭 코드 표시·해제 | Task 5 |
| 실기기 확인 | Task 7 Step 5 |

**의도한 스펙 이탈 2건**
1. **코드는 central 에게 보내지 않는다.** 스펙 5.1 의 도식은 `[코드 대기]` 로 가는 전이만 적고 응답 내용을 명시하지 않았다. 코드를 BLE 로 보내면 근처의 누구나 읽을 수 있어 페어링이 무의미해지므로, `HELLO` 의 응답은 `{"ok":false,"await":"code"}` 로 하고 코드는 Mac 화면에만 띄운다.
2. **인가는 연결 단위다.** central 식별자는 재사용될 수 있으므로 링크가 끊기면 `forget_central`(→ `PairingManager::end_session`) 로 즉시 지운다. 재연결 시에는 저장된 토큰으로 논스에 서명해 다시 인가된다 — 사용자에게는 끊김 없이 보인다.

**Triggers 특성은 여전히 범위 밖이다** — 전송 경로가 없다. Auth 만 추가한다.

**타입 일관성 확인**
- `AuthRequest`/`AuthReply`/`PairingManager::{handle,is_authorized,forget,visible_code,load_tokens,issued_tokens}` — Task 1 정의, Task 3 사용 일치
- `PeerStore::{path,load_from,save_to}` — Task 2 정의, Task 5 사용 일치
- `PeripheralEvent::{AuthWrite,Disconnected}`, `BlePeripheral::{notify_auth,authorized_subscribers}` — Task 3 정의, Task 4 구현, Task 5 소비 일치
- `BleBridge::{begin_pairing,handle_auth,visible_pairing_code,forget_central,load_tokens,unpair_all}` — Task 3 정의, Task 5 호출 일치
- `PairingClient::{helloFrame,codeFrame,authFrame,proofFrame,parse}`, `TokenStore::{save,load,clear}` — Task 6 정의, Task 7 사용 일치
- HMAC 입력 인코딩(원시 바이트) — Rust `pairing.rs` 와 Swift `PairingClient` 가 `hmac-sample.json` 으로 고정
- 와이어 문자열 `HELLO` / `CODE:` / `AUTH` / `PROOF:` — Task 1 파서와 Task 6 인코더가 같은 리터럴을 쓴다(Task 6 테스트가 이를 단언)
- `AuthReplyPayload.awaiting` ↔ JSON 키 `await` — Task 3 이 내보내는 키와 Task 6 의 `CodingKeys` 가 일치

---

## Execution Handoff

계획 완료. 실행 방식 두 가지 중 선택한다.

1. **Subagent-Driven (권장)** — 태스크마다 새 서브에이전트를 붙이고 사이사이 리뷰.
2. **Inline Execution** — 이 세션에서 체크포인트를 두고 배치 실행.
