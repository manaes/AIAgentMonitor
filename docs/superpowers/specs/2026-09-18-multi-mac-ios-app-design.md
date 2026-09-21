# 다중 Mac 모니터링 iOS 앱 설계

> 작성 2026-09-18. 기존 1:1 미러 앱(`ios/` 의 App/AppBLE)과 **별개의 새 앱**이다.

## 1. 목표

한 대의 iPhone 에서 **여러 대의 Mac** 에 돌아가는 AI 에이전트 사용량을 한눈에 보고,
그중 하나를 골라 지금의 세부 화면으로 들어간다.

- 앱을 켜면 **장치 목록**이 뜨고, 각 Mac 별로 에이전트 사용량(tok/s·5h·주간 한도)을 요약해서 보여준다.
- 장치를 탭하면 **기존 세부 화면과 같은 화면**(에이전트 카드 + 세션 목록)으로 들어간다.
- Mac 앱(Tauri)은 기존 것을 **공용으로** 쓴다. 필요한 최소 변경만 가한다.

### 성공 기준

1. Mac 2대 이상을 페어링한 뒤 앱을 켜면, 켜져 있는 Mac 들의 사용량이 목록에 채워진다.
2. 꺼져 있는 Mac 이 섞여 있어도 목록이 그 때문에 느려지거나 멈추지 않는다.
3. 장치 하나의 인증 실패·버전 불일치가 **다른 장치나 앱 전체를 끌고 가지 않는다**.
4. 백그라운드에 갔다 돌아오면 연결이 자동으로 복구된다.

## 2. 범위

**포함**
- 새 iOS 앱 타겟(UIKit), 네트워크(iroh) 전송 전용
- 다중 페어링 저장소, 장치 목록/상세/페어링 추가 화면
- Mac 앱 QR 페이로드에 호스트명 추가

**제외**
- 위젯 (이번 스코프 아님)
- BLE 전송 — 스캔·연결 특성상 다중이 무리다. 이 앱은 네트워크 전용이고 `AppBLE` 과는 무관하다.
- 기존 앱 대체 — 기존 App/AppBLE 은 1:1 전제로 계속 유지된다.
- 기존 앱과 페어링 정보 공유 — 서비스 문자열이 달라 Mac 을 다시 스캔해야 한다(의도된 설계).

## 3. 전제와 제약

| 항목 | 값 | 근거 |
|---|---|---|
| 최대 장치 수 | **16대** | 무제한은 probe 라운드·목록 성능이 예측 불가. 개인 사용 상한으로 충분 |
| probe 동시성 | **4** | 순차는 꺼진 Mac 의 타임아웃이 직렬로 쌓여 최악 100초 이상 |
| probe 타임아웃 | **3초** | 도달 확인용이라 상세 연결보다 짧게 |
| 오프라인 전환 임계 | 연속 실패/타임아웃 **3회** | 일시적 끊김과 실제 꺼짐을 구분하는 히스테리시스 |
| 오프라인 재탐색 주기 | **3분** | 포어그라운드 상태에서만. 너무 짧으면 꺼진 Mac 에 대한 낭비가 큼 |
| UI 프레임워크 | **UIKit** | `AgentCardView`/`QuotaBarView`/`Palette`/`Typography` 를 상세 화면에 그대로 재사용 |

**iOS 수명주기 제약**: 앱이 백그라운드로 가면 iOS 가 프로세스를 suspend 하고 QUIC 연결은 조용히 끊긴다.
따라서 포어그라운드 복귀 시 대상은 "오프라인이던 장치"가 아니라 **전부**다.

## 4. 아키텍처

### 4.1 프로젝트 구조

기존 저장소 `4_AIAgentMonitor` 안에 새 앱 타겟을 추가한다(`ios/Project.swift`).
별도 저장소로 떼지 않는 이유는 와이어 프로토콜과 Mac 앱을 같이 고쳐야 하고,
`ios/Sources/Wire` 가 Rust `app/src-tauri/src/ble/wire.rs` 와 골든 벡터로 묶여 있기 때문이다.

**그대로 재사용**

| 모듈 | 용도 |
|---|---|
| `Wire` | 와이어 DTO. 이번에 QR 파싱에 호스트명만 추가 |
| `MirrorFormat` | `toFixed`/`tokensPerSec`/`weeklyCountdown`/`QuotaDisplay.gradient`. Mac·CYD 와 표기를 맞추는 근거 |
| `DesignSystem` | `Palette`/`Typography`/`QuotaBarView` — 상세 화면을 거의 그대로 구성. ⚠️ `AgentCardView`·`SessionListView`·`QRScannerViewController` 는 현재 `MirrorFeature` 에 있는데, 그 모듈은 `BLETransport`·`WidgetShared` 까지 끌고 온다. 세 파일 모두 전송 계층을 모르므로 구현 시 `DesignSystem` 으로 옮겨 두 앱이 공유한다(`DesignSystem` 에 `Wire` 의존성이 추가된다) |
| `NetworkTransport` | iroh 클라이언트. 아래 4.2 의 최소 변경을 가한다 |

### 4.2 연결 계층

기존 `NetworkClient` 를 그대로 N개 만들 수 없다. 두 가지 이유다.

1. 인스턴스마다 `EndpointBuilder().bind()` 를 호출한다(`NetworkClient.swift:127,203`).
   iroh Endpoint 는 소켓 하나가 아니라 **자체 UDP 소켓 + relay 연결 + discovery 상태**를
   끌고 다니는 무거운 객체라, 16개를 만들면 그게 통째로 16벌이 된다.
2. 클래스 전체가 `@MainActor` 라 16세션의 디코딩·상태 갱신이 메인 액터에 직렬화된다.

세 계층으로 나눈다.

```
IrohEndpointProvider (actor)   ← Endpoint 하나를 모두가 공유
        ↑
DeviceSession (장치 1대)        ← 상태기계 + 기존 NetworkClient 조각 재사용
        ↑
DeviceFleet                     ← 16세션 소유, probe 라운드·트리거 관리
        ↑
장치 목록 화면
```

- **`IrohEndpointProvider`**: Endpoint 를 딱 하나 만들어 공유한다. QUIC 은 같은 소켓 위에서
  연결을 다중화하므로 장치가 늘어도 소켓·relay·discovery 는 1벌이면 된다.
  사내 선례: 옆 프로젝트 TunnelKit 이 같은 문제를 `IrohEndpointPool` actor 로 해결했다.
- **`DeviceSession`**: 장치 한 대의 수명주기. 내부는 기존 `NetworkClient` 의 검증된 조각을 쓴다 —
  `authenticate`(BLEClient.decideV2 재사용), `dialAuthenticateAndReadOne`(probe),
  `listenForSnapshots`(승격 후 스트리밍).
- **`DeviceFleet`**: probe 라운드를 동시성 4로 돌리고 트리거를 받는다. 목록 화면은 여기만 구독한다.

**기존 `NetworkTransport` 변경 범위 (최소)**: Endpoint 생성만 `IrohEndpointProvider` 로 빼고
`NetworkClient` 가 그것을 주입받게 한다. 주입하지 않으면 지금처럼 자기 것을 만들므로
**기존 App/AppBLE 의 동작은 바뀌지 않는다**. 모듈을 복제하는 것보다 중복도 회귀 표면도 작다.

### 4.3 probe → 스트리밍 승격

probe 와 스트리밍을 **같은 연결로 이어간다**. probe 는 `dial → 인증 → 첫 스냅샷 수신`까지이고,
성공하면 그 연결을 끊지 않고 그대로 `listenForSnapshots` 로 넘긴다. 다시 dial 하면
가장 비싼 부분(hole-punch·QUIC·인증 왕복)을 두 번 내게 되기 때문이다.

이를 위해 `dialAuthenticateAndReadOne` 을 분해해야 한다. 현재는 연결을 내부에서 만들고 첫
스냅샷만 반환한 뒤 놓아버리는데(위젯은 단발성이라 그게 맞다), 새 앱은 **연결과 첫 스냅샷을 함께
돌려받아야** 한다. 위젯 경로는 "받은 연결을 즉시 닫는" 얇은 래퍼로 남긴다.

### 4.4 상세 화면에 들어가도 나머지 세션은 유지한다

상세로 이동해도 다른 15대의 스트리밍을 끊지 않는다. 목록으로 돌아왔을 때 즉시 최신값이 보이고,
끊었다 다시 붙이면 4.3 에서 피하려던 비용을 그대로 내게 되기 때문이다.

## 5. 데이터 모델과 저장소

### 5.1 장치 레지스트리 (Keychain, 항목 1개)

현재 `NetworkTokenStore` 는 `network-pairing-token` 같은 **고정 계정 문자열**에 값을 하나씩 넣어
다중화가 불가능하다. 새 앱은 Keychain 항목 하나에 JSON 레지스트리를 넣는다.

```
Device {
  endpointIdHex: String   // 키. iroh EndpointId, Mac 마다 고유하고 안정적이다
  token: String           // 페어링 토큰(비밀)
  relayUrl: String?
  addresses: [String]
  macHostname: String?    // QR 로 받은 Mac 이름
  userLabel: String?      // 사용자가 덮어쓴 이름. 표시 우선순위: userLabel > macHostname > endpointId 앞 8자
  sortIndex: Int
}
```

16대면 몇 KB라 읽기/쓰기 한 번이면 되고 원자적이다. 장치마다 Keychain 항목을 따로 두는 방식은
열거하려고 `kSecMatchLimitAll` 쿼리를 돌리고 계정 문자열을 파싱해야 해서 더 번거롭다.

**access group 은 쓰지 않는다.** 위젯이 없어 프로세스 간 공유가 없으므로,
기존 앱이 겪은 팀 접두사(`$(AppIdentifierPrefix)`) 문제 자체가 이 앱에는 존재하지 않는다.

### 5.2 마지막 스냅샷 캐시 (파일)

Application Support 에 `endpointIdHex` 별 JSON. Keychain 이 아닌 이유는 스트리밍 중 초당 갱신인데
Keychain 쓰기는 느리고 용도도 아니기 때문이다. 쓰기는 백그라운드 전환·종료 시점 등으로 스로틀한다.

앱 시작 시 이 캐시를 **먼저** 그려서 probe 가 끝나기 전에도 목록이 비어 보이지 않게 한다
(전부 오프라인 톤 + 신선도 표기).

## 6. 화면 설계

### 6.1 장치 목록 (루트)

`UICollectionView` + diffable data source. 셀 하나가 Mac 한 대다.

- **구성**: 이름 + 상태 배지 + 에이전트 행 **최대 2개 + "+N"**
  (16대까지 쓰므로 가변 높이는 한 화면에 3~4대밖에 못 넣는다)
- **에이전트 행**: 이름 / tok/s / 5h·주간 막대.
  상세의 `AgentCardView` 는 모델명·프로젝트·카운트다운까지 있어 목록엔 과하므로 압축형을 따로 만든다.
- **정렬**: `온라인 → 불안정 → 오프라인` 그룹, 그룹 안에서는 `sortIndex`(드래그) 순
- **당겨서 새로고침**: 오프라인 장치 재탐색 트리거
- **우상단 +**: 페어링 추가

### 6.2 상세

기존 세부 화면과 동일하다. `AgentCardView` + 세션 목록을 그대로 쓴다.
해당 장치가 이미 스트리밍 중이면 그 세션을 구독하고,
오프라인 장치였다면 **사용자 의도가 명확하므로 즉시 1회 재시도**한 뒤 실패 시 캐시 + 안내를 보여준다.

### 6.3 페어링 추가

QR 스캐너 → 스캔 → 이름 확인/수정 → 저장 → 즉시 연결 시도.
16대에 도달했으면 스캐너를 열기 전에 막고 안내한다.

## 7. 상태 모델

```
확인중 ──probe 성공──→ 온라인(스트리밍)
  │                        │
  │                   연결 끊김
  │                        ↓
  │                    불안정(재시도)
  │                        │
  └──3회 실패──────────────┴──→ 오프라인
                                   │
              타이머(3분)/포어그라운드 복귀/당겨서 새로고침
                                   ↓
                                확인중
```

별도 종단 상태 두 개(재시도로 풀리지 않는다):

- **재페어링 필요** — 토큰 폐기·Mac 재설치. 해당 셀만 이 상태가 되고, 탭하면 그 장치용 QR 스캐너.
- **버전 불일치** — Mac 이 구/신버전.

### 오프라인 장치의 표시 규칙

**한도 %와 막대는 남기고, tok/s 같은 순간값은 감춘다.** 한도는 몇 분 지나도 여전히 유효하지만
tok/s 는 지금 이 순간의 값이라 낡으면 의미가 없기 때문이다. 신선도("12분 전")를 함께 보여준다.

## 8. 오류 처리와 엣지케이스

- **장치별 격리**: 한 장치의 실패가 다른 장치나 앱 전체 상태를 바꾸지 않는다.
- **인증 실패 시 재시도 금지**: `NetworkClient.swift:213-222` 에 박혀 있는 교훈을 장치별로도 지킨다.
  코드 없이는 성공할 수 없는 재시도가 QR 스캐너를 깜빡이게 만든 버그가 있었다.
- **중복 스캔은 병합**: 같은 `endpointIdHex` 를 다시 스캔하면 새 항목을 만들지 않고
  토큰·relay·주소를 갱신한다. Mac 의 IP 가 바뀌었을 때 재스캔으로 고치는 경로가 된다.
- **레지스트리 손상**: 단일 항목 방식의 유일한 실질 리스크다. JSON 파싱 실패 시
  **빈 레지스트리로 조용히 시작하지 않는다** — 이전 값을 보존한 채 오류를 표면화한다.
- **취소/정리 레이스**: 오프라인 전환·상세 이탈·포어그라운드 재연결 시점에 늦게 도착한 결과가
  상태를 덮어쓰는 문제. 세션마다 **세대(generation) 카운터**를 두고 모든 완료 지점에서 검사한다.
  (2026-09-17 TunnelKit 리뷰에서 같은 클래스의 버그 3건이 하루에 발견됐다.)

## 9. Mac 앱(Tauri) 변경사항

**QR 페이로드에 호스트명 추가 — 이것 하나뿐이다.**

현재: `aim://pair?endpoint=<hex>&code=<코드>&relay=<hex>&addr=<hex>...`
추가: `&name=<hex 로 인코딩한 hostname>`

쿼리 파라미터라 **기존 클라이언트는 알 수 없는 항목을 그냥 무시**하므로 하위호환이 유지된다.

`MirrorSnapshot` 에 이름을 넣지 않는 이유: 그러면 BLE·CYD 까지 **매 스냅샷마다** 그 바이트를
물어야 한다. `wire.rs` 가 짧은 키를 쓰는 이유가 BLE 대역 절약이다. QR 은 페어링 때 한 번이라 공짜다.

한계: Mac 이름을 나중에 바꾸면 갱신되지 않는다. 사용자 덮어쓰기(`userLabel`)로 커버한다.

## 10. 테스트 전략

판정 로직을 뷰 밖으로 빼서 순수 타입으로 고정한다(기존 앱의 `UsagePresentation` 과 같은 방식 —
위젯 표시 버그가 실기 스크린샷으로만 잡히던 이력에서 얻은 교훈이다).

- 레지스트리 인코딩/디코딩, 16대 캡, 중복 `endpointIdHex` 병합
- 상태기계 전이: 성공 / 실패 3회 → 오프라인 / 재페어링 필요는 재시도하지 않음
- 표시 규칙: 오프라인이면 tok/s 를 감추고 한도는 유지, 이름 표시 우선순위
- 정렬 규칙

**`DeviceSession` 은 전송 계층을 프로토콜로 주입**받게 해서 가짜 transport 로 상태기계를 테스트한다.
실제 iroh 연결은 단위 테스트가 불가능하다.

**실기 검증 필수 3종**: Mac 2대 이상 동시 표시 / 오프라인 전환 / 포어그라운드 복귀.

## 11. 앱 식별자와 배포

| 항목 | 값 |
|---|---|
| 번들 ID | `co.kr.wannypark.aiagentmonitor.multi` |
| 표시명 (`CFBundleDisplayName`) | `AI Monitor Multi` |
| Tuist 타겟명 | `AppMulti` (기존 `App`/`AppBLE` 와 나란히) |
| 최소 iOS | **17.5** — `NetworkTransport` 가 iroh-ffi 때문에 강제한다 |

기존 앱이 `…aiagentmirror`(미러링 전제)인 것과 달리 이 앱은 제품명 그대로 `aiagentmonitor` 를 쓴다.

배포 시 유의: 새 App ID 라 개발자 포털 등록과 프로비저닝 프로파일 발급이 필요하다.
다만 **위젯이 없어 App Group·Keychain Sharing 케이퍼빌리티가 필요 없으므로**,
기존 앱이 위젯 추가 때 겪은 종류의 서명 문제는 발생하지 않는다.
