# BLE·네트워크 동시 공유와 페어링 통합 설계

> 2026-08-25 · 이 문서는 `2026-08-18-ble-ios-mirror-design.md`(이하 **원 스펙**) 5·6장을 개정한다.
> 원 스펙의 보안 근거(무차별 대입 방어, 창 소유권 없음, 챌린지-응답 재인증)는 **그대로 유지**하며,
> 바뀌는 것은 그 창이 어느 전송에 속하느냐뿐이다.

## 1. 문제

지금은 BLE 공유와 네트워크(iroh) 공유 중 **하나만** 켤 수 있다. `ble_set_enabled` /
`network_set_enabled` 가 서로를 확인해 거부한다(`lib.rs:129`, `lib.rs:279`).

사용자가 원하는 것: **폰 A 는 BLE 로, 폰 B 는 네트워크로 동시에** 연결. 예를 들어 책상 위
아이패드는 BLE 로, 외출한 폰은 네트워크로.

### 왜 지금 막혀 있나

기술적 제약이 아니다. 확인한 바:

- 1Hz 틱 루프는 **이미** 매 틱마다 두 브릿지 모두에 스냅샷을 넘긴다(`lib.rs:1014-1015`).
  각 브릿지가 `enabled` 를 보고 스스로 조기 반환한다.
- iroh 소켓은 공유 on/off 와 무관하게 **항상** 떠 있다. "공유 꺼짐" 은 accept 루프가 들어오는
  연결을 즉시 닫는 것으로 구현돼 있다.
- 두 전송의 `CentralId` 네임스페이스가 겹치지 않는다 — BLE 는 `CBCentral.identifier`(UUID),
  네트워크는 iroh `EndpointId`.

막고 있는 것은 **정책 가드 두 줄**과, 그 가드가 존재하는 이유인 **페어링 모델**이다.

### 진짜 제약 — 창이 둘로 쪼개진다

`BleBridge` 와 `NetworkBridge` 는 **각자 자기 `PairingManager` 인스턴스**를 갖는다
(`ble/mod.rs:26`, `network/mod.rs:48`). 토큰 파일도 따로다
(`ble-peers.json` / `network-peers.json`).

두 전송을 그냥 동시에 켜면 페어링 창이 둘이 된다. 원 스펙 5.2 의 무차별 대입 방어 근거는
**"창 하나당 시도 5회"** 인데, 창이 둘이면 서로 독립된 코드 두 개에 각각 5회씩 —
노출이 대략 2배가 된다(그래도 10만분의 1 수준이지만, 문서가 약속한 값은 아니다).

`network/peers.rs` 의 기존 주석이 이 순간을 예고해뒀다:

> BLE와 네트워크는 별도 PairingManager 인스턴스를 쓰므로 페어링 목록도 분리 —
> **Phase 5에서 신원 통합을 검토할 때 재논의**.

## 2. 결정

**페어링을 두 전송이 공유한다.** 창 하나, 코드 하나, 시도 예산 하나, 토큰 저장소 하나.

이렇게 하면 원 스펙 5.1 의 "창은 하나" 전제가 **문자 그대로** 유지되고, 5.2 의 무차별 대입
근거가 손대지 않아도 성립한다 — 공격자는 어느 전송으로 오든 같은 예산 5회를 나눠 쓴다.

부수 효과로 **Mac 쪽에서는 토큰이 전송을 가로지른다**: 어느 전송으로 발급한 토큰이든
같은 저장소에 들어가고, 어느 전송으로 들어온 `PROOF` 든 같은 토큰 집합으로 검증한다.
토큰이 곧 기기 정체성이므로(원 스펙 6장, `peer_id = hex(SHA-256(토큰))[..8]`) 자연스러운
귀결이고, 기기 목록이 하나로 합쳐지는 근거이기도 하다.

**다만 폰은 그렇지 않다.** iOS 는 토큰을 Keychain 에 전송별로 다른 계정 키로 저장한다
(`TokenStore` 의 `ble-pairing-token`, `NetworkTokenStore` 의 `network-pairing-token`).
그래서 BLE 로 페어링한 폰이 네트워크로 바꿔 붙으면 자기 네트워크 토큰이 없어 코드부터
다시 요구한다 — Mac 저장소를 합치는 것만으로는 이게 해결되지 않는다. 폰 쪽 토큰 통합은
**이번 범위 밖**이다(10장).

### 대안과 기각 사유

- **전송별로 독립 페어링** — 변경은 가장 작지만(가드 두 줄 제거) 화면에 코드가 둘 뜨고,
  기기 목록이 둘로 나뉘며, 5.2 의 근거를 "전송당 5회" 로 약화시켜야 한다. 기각.
- **브릿지가 `Arc<Mutex<PairingManager>>` 를 나눠 갖기** — 시그니처 변경은 적지만 브릿지
  락 안에서 페어링 락을 또 잡는 중첩이 생기고, "페어링은 누구 것인가" 가 흐릿하게 남는다.
  기각(아래 3장 채택안이 순 코드량도 더 적다).

## 3. 소유권 — 페어링을 브릿지 밖으로

`lib.rs` 가 `Arc<Mutex<PairingManager>>` 하나를 소유하고 `app.manage` 로 등록한다
(`settings_state` 와 같은 패턴). 두 브릿지에서 `pairing` 필드를 없앤다.

### 브릿지에서 삭제되는 것 (전송과 무관한 순수 위임, 각 7개 × 2 = 14개)

`begin_pairing` · `pairing_window` · `paired_peers` · `stored_peers` · `load_peers` ·
`unpair_peer` · `unpair_all`

공유되는 순간 이것들은 앱 레벨 개념이지 전송 계층 개념이 아니다. `lib.rs` 가
`PairingManager` 를 직접 호출한다.

### 브릿지에 남는 것 (전송에 실제로 묶인 것)

```rust
// 응답 바이트 조립·전송이 전송별로 다르므로 남는다.
fn handle_auth(&mut self, central: &CentralId, data: &[u8], now: SystemTime,
               pairing: &mut PairingManager) -> /* 전송별 반환형 유지 */;

// 인가 필터에 pairing 이 필요하다. 반환형·나머지 동작은 그대로.
fn on_snapshot(&mut self, snap: &Snapshot, now: SystemTime, pairing: &PairingManager);

// 이제 페어링을 만지지 않는다 — 자기 전송 자원만 정리한다.
// BLE: revoke_targets(macOS pump 대상). 네트워크: snapshot_senders 제거.
fn forget_central(&mut self, central: &CentralId);

// 신규. 앱이 언페어링/세션 종료 후 "이 central 들 정리해라" 라고 알린다.
fn drop_sessions(&mut self, ids: &[CentralId]);
```

호출부(`lib.rs`)가 `PairingManager` 락을 먼저 잡고 브릿지 메서드에 넘긴다 — 중첩 락이
없고, 테스트에서 `PairingManager` 를 직접 만들어 주입할 수 있다.

## 4. 끄기 범위 — 자기 전송만 정리

지금 `set_enabled(false)` 는 `pairing.end_all_sessions()` 로 **전체** 세션 인가를 지운다
(`ble/mod.rs:65`, `network/mod.rs:82`). 공유하면 BLE 를 끄는 순간 네트워크 세션까지 죽는다.

`PairingManager` 에 추가:

```rust
/// 주어진 central 들의 세션 인가만 내린다. 저장된 토큰은 남긴다.
/// 전송 하나를 끌 때, 그 전송이 서비스 중이던 central 만 정리하는 데 쓴다 —
/// 공유 PairingManager 에서 end_all_sessions 를 쓰면 다른 전송의 세션까지 죽는다.
pub fn end_sessions(&mut self, ids: &[CentralId]);
```

각 전송이 끌 때 넘기는 목록:
- BLE — `peripheral.subscribers()` 의 id (그 전송이 서비스 중이던 central)
- 네트워크 — `snapshot_senders` 의 키

기존 `end_all_sessions` 는 `unpair_all` 경로에만 남긴다(그때는 전부 내리는 게 맞다).

## 5. 토큰 저장소 통합

새 경로: `~/.config/ai-agent-monitor/paired-peers.json`.
형식은 기존과 **동일**하다 — `network/peers.rs` 가 이미 `ble::peers::PeerStore` 를 재사용하고
경로만 분리하고 있어, 두 파일의 `StoredPeer { token, paired_at }` 형식이 같다.

### 마이그레이션

앱 시작 시 `paired-peers.json` 이 **없으면**:

1. `ble-peers.json` 과 `network-peers.json` 을 각각 읽는다(`LoadOutcome`, 손상/부재는 빈 목록).
2. 토큰 기준으로 합친다. 같은 토큰이 양쪽에 있으면 `paired_at` 이 **이른 쪽**을 쓴다
   (그 기기를 처음 페어링한 시각이 맞다).
3. `paired-peers.json` 으로 저장한다.
4. **옛 파일은 지우지 않는다** — 이 버전을 되돌릴 여지를 남긴다.

`paired-peers.json` 이 이미 있으면 그것만 읽는다(마이그레이션은 1회).

기존 페어링은 전부 살아남고, 사용자는 재페어링할 필요가 없다.

## 6. 커맨드와 상태

### 통합

| 지금 | 바뀜 |
|---|---|
| `ble_begin_pairing`, `network_begin_pairing` | `begin_pairing() -> PairingInfo` |
| `ble_unpair(peer_id)`, `network_unpair(peer_id)` | `unpair(peer_id)` |
| `ble_unpair_all()`, `network_unpair_all()` | `unpair_all()` |
| `BleStatus.pairing_window`·`paired_peers`, `NetworkStatus` 의 동일 필드(중복) | `pairing_status() -> PairingStatus` |

```rust
pub struct PairingInfo {
    pub code: String,
    /// 네트워크 공유가 켜져 있을 때만 Some — 폰이 QR 로 스캔할 페이로드다.
    /// BLE 만 켜져 있으면 QR 을 그릴 이유가 없다.
    pub qr_payload: Option<String>,
}

pub struct PairingStatus {
    pub pairing_window: pairing::PairingWindow,
    pub paired_peers: Vec<pairing::PairedPeer>,
}
```

`unpair`/`unpair_all` 은 `PairingManager` 에서 폐기한 뒤, 내려간 `CentralId` 목록을
**두 브릿지 모두**에 `drop_sessions` 로 넘긴다(어느 전송에 붙어 있었는지 앱은 모르고,
알 필요도 없다 — 없는 id 는 무시된다).

### 유지

`ble_set_enabled` / `network_set_enabled` 는 남는다. **상호 배타 가드만 제거**한다.
`BleStatus` 는 `supported`·`enabled`·`advertising`·`peers`·`last_error` 를,
`NetworkStatus` 는 `supported`·`enabled`·`endpoint_id`·`last_error` 를 계속 갖는다.

## 7. UI (Devices 탭)

### 토글 둘로 분리

지금은 토글 하나 + 모드 선택기(BLE/네트워크)다. 모드 선택기는 **사라지고**, BLE 와 네트워크가
각자 독립 토글을 갖는다. 플랫폼이 지원하지 않으면 그 토글은 숨긴다(BLE 는 macOS 전용).

### 페어링 영역 하나

`[페어링 시작]` 버튼 하나. 창이 열리면 **켜져 있는 전송에 맞는 것만** 보여준다:

| BLE | 네트워크 | 화면 |
|---|---|---|
| 켜짐 | 꺼짐 | 6자리 코드 |
| 꺼짐 | 켜짐 | QR |
| 켜짐 | 켜짐 | 6자리 코드 + QR (**같은 코드**) |
| 꺼짐 | 꺼짐 | `[페어링 시작]` 자체를 숨긴다 |

남은 초·남은 시도·만료·시도 소진 표시는 지금 로직을 그대로 쓴다(로컬 1초 타이머로
`expires_at` 재계산, 만료·소진 구분).

### 기기 목록 하나

지금은 BLE 목록과 네트워크 목록이 따로다. 하나로 합친다 — `peer_id`, 페어링 시각,
`연결됨` 배지, `해제` 버튼.

`연결됨` 판정은 지금처럼 `PairedPeer.connected`(= 그 토큰으로 인가된 세션이 있는가)를 쓴다.
공유 `authorized` 맵이 두 전송의 세션을 모두 담으므로 어느 쪽으로 붙어 있든 참이 된다.

**어느 전송으로 붙어 있는지는 이번엔 표시하지 않는다.** 표시하려면 `PairingManager` 가
세션마다 전송을 알아야 하는데, 지금은 `CentralId` 네임스페이스로만 암묵적으로 갈릴 뿐
명시적으로 들고 있지 않다. 다만 2장에서 밝힌 대로 **폰은 전송을 가로지르지 못하므로**,
한 기기를 두 전송 모두에 붙이면 토큰이 둘이 되어 **목록에 두 줄로 보인다** — 페어링 시각과
`peer_id` 가 달라 구분은 되지만 같은 기기인지는 알기 어렵다. 실사용에서 이게 실제로
불편한지 보고 나서 전송 라벨을 붙일지 판단한다(지금 넣으면 쓰이지 않을 수도 있는 상태를
`PairingManager` 에 추가하게 된다).

## 8. 한 코드 = 한 기기 (변경 없음)

`CODE:` 가 성공하면 `self.pending = None` 으로 창이 닫힌다(`pairing.rs:432`). 즉 코드 하나로
기기 하나만 페어링된다. **이 동작은 유지한다** — 코드 하나가 여러 기기를 인가하면 코드가
유출됐을 때 피해가 커진다.

기기를 여러 대 붙이려면 `[페어링 시작]` 을 기기마다 한 번씩 누른다. 지금도 그렇게 동작한다.

## 9. 테스트

`PairingManager` 를 파라미터로 받게 되므로 브릿지 테스트에서 직접 주입할 수 있다.

새로 고정할 성질:

1. **공유 예산** — BLE 로 3회, 네트워크로 2회 틀리면 합쳐서 5회로 창이 소진된다.
   (원 스펙 5.2 의 근거가 두 전송에 걸쳐 성립하는지 — 이 설계의 핵심 보안 성질)
2. **끄기 범위** — BLE 를 꺼도 네트워크 세션의 인가가 유지된다. 그 반대도 마찬가지.
3. **마이그레이션** — 옛 두 파일이 새 파일 하나로 정확히 합쳐지고, 같은 토큰이 양쪽에
   있으면 이른 `paired_at` 이 남는다. 새 파일이 이미 있으면 옛 파일을 읽지 않는다.
4. **Mac 쪽 토큰 공유** — BLE 의 `CODE:` 로 발급된 토큰을 네트워크 쪽 `PROOF` 로 검증해도
   `Authorized` 가 나온다. 이는 **Mac 의 저장소가 하나임**을 고정하는 것이지, 폰이 전송을
   가로지를 수 있다는 뜻은 아니다(폰 Keychain 은 전송별로 분리돼 있다 — 2장·10장).
5. **언페어링 전파** — `unpair(peer_id)` 가 두 브릿지 모두에서 세션을 정리한다.

기존 테스트 중 브릿지가 `pairing` 을 직접 들고 있다고 가정한 것들은 **지우지 말고**
주입 방식으로 고쳐 쓴다.

## 10. 범위 밖

- **한 코드로 여러 기기 페어링** — 8장 참고. 보안 설계 변경이라 별도 논의가 필요하다.
- **기기 목록의 전송 표시** — 7장 참고. YAGNI.
- **iOS 앱 변경** — 없다. 폰은 어느 전송으로 붙든 같은 페어링 프로토콜을 쓰므로
  (`BLEClient.decide` 를 `NetworkClient` 가 그대로 재사용) Mac 쪽 변경만으로 동작한다.
- **폰의 전송 간 토큰 공유** — 하지 않는다. iOS 는 토큰을 Keychain 에 전송별 계정 키로
  나눠 저장하므로(2장 참고), 폰이 전송을 바꾸면 그 전송에서 한 번은 코드로 페어링해야 한다.
  합치려면 Keychain 계정 키를 하나로 모으고 기존 두 항목을 마이그레이션해야 하는데,
  이는 Mac 쪽 통합과 독립된 별개의 작업이라 분리한다. **한 기기를 두 전송 모두에서 쓰려면
  전송마다 한 번씩 페어링한다** — 그래도 Mac 기기 목록에는 두 항목으로 보인다(토큰이
  둘이므로 `peer_id` 도 둘이다).
- **BLE 와 네트워크로 동시에 붙은 같은 폰** — 막지 않지만 권장하지도 않는다. 같은 토큰으로
  두 세션이 열리면 스냅샷이 양쪽으로 중복 전송될 뿐 오동작은 아니다.
