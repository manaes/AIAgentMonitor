# AI Monitor Multi 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 한 대의 iPhone 에서 최대 16대의 Mac 사용량을 목록으로 보고, 탭하면 기존 세부 화면으로 들어가는 새 iOS 앱을 만든다.

**Architecture:** 기존 저장소 안에 새 Tuist 앱 타겟(`AppMulti`)을 추가한다. iroh Endpoint 하나를 모든 장치가 공유하고(`IrohEndpointProvider`), 장치마다 상태기계(`DeviceSession`)를 두며, `DeviceFleet` 이 probe 라운드와 재탐색 트리거를 관리한다. 판정 로직(상태 전이·표시 규칙·레지스트리)은 전부 뷰 밖 순수 타입으로 두어 테스트로 고정한다.

**Tech Stack:** Swift / UIKit + SnapKit / Tuist 4.208.0 / iroh(IrohLib) / Combine / XCTest, Mac 쪽은 Rust(Tauri)

**Spec:** `docs/superpowers/specs/2026-09-18-multi-mac-ios-app-design.md`

## Global Constraints

- 최대 장치 수 **16대**. 초과 페어링은 저장 단계에서 거부한다.
- probe 동시성 **4**, probe 타임아웃 **3초**, 오프라인 전환 임계 **연속 3회** 실패, 오프라인 재탐색 주기 **3분**.
- 번들 ID `co.kr.wannypark.aiagentmonitor.multi`, 표시명 `AI Monitor Multi`, Tuist 타겟명 `AppMulti`.
- 최소 iOS **17.5** (`NetworkTransport` 가 iroh-ffi 때문에 강제. `Project.swift` 의 `iOS` 상수 재사용).
- **위젯 없음 / BLE 없음 / App Group·Keychain access group 없음.**
- 기존 `App`/`AppBLE` 의 동작은 바뀌면 안 된다. `NetworkTransport` 변경은 하위호환으로만.
- `MirrorFormat` 의 표기 함수(`toFixed`/`tokensPerSec`/`weeklyCountdown`/`QuotaDisplay`)를 반드시 재사용한다 — Mac·CYD 와 숫자 표기를 맞추는 근거다.
- 빌드·테스트 시뮬레이터: `platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23`.
- `Project.swift` 를 고칠 때마다 `cd ios && mise exec -- tuist generate --no-open` 을 다시 실행해야 `.xcodeproj` 에 반영된다.
- 커밋 메시지는 한글로 쓴다(기존 저장소 관행).

---

### Task 1: Mac 앱 QR 페이로드에 호스트명 추가

**Files:**
- Modify: `app/src-tauri/src/lib.rs:328-350` (`build_qr_payload`)

**Interfaces:**
- Produces: QR 문자열에 `name=<hex 로 인코딩한 UTF-8 hostname>` 파라미터. Task 2 의 파서가 읽는다.

기존 클라이언트는 모르는 쿼리 항목을 무시하므로 하위호환이 깨지지 않는다.

- [ ] **Step 1: 실패하는 테스트 작성**

`app/src-tauri/src/lib.rs` 의 테스트 모듈(파일 하단 `#[cfg(test)] mod tests` — 없으면 파일 끝에 새로 만든다)에 추가:

```rust
#[test]
fn qr_payload_carries_hex_encoded_hostname() {
    let payload = qr_params_with_name(
        "deadbeef",
        "123456",
        &[],
        Some("wanny-macbook".to_string()),
    );
    // "wanny-macbook" 의 UTF-8 hex
    assert!(payload.contains("name=77616e6e792d6d6163626f6f6b"));
}

#[test]
fn qr_payload_omits_name_when_hostname_is_unavailable() {
    let payload = qr_params_with_name("deadbeef", "123456", &[], None);
    assert!(!payload.contains("name="));
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd app/src-tauri && cargo test qr_payload
```

기대: `cannot find function 'qr_params_with_name'` 로 컴파일 실패.

- [ ] **Step 3: 순수 함수로 분리하고 호스트명 추가**

`build_qr_payload` 은 `NetworkHandle` 과 async 를 쓰므로 테스트할 수 없다. 문자열 조립만 순수 함수로 뽑는다.

```rust
/// QR 쿼리 문자열을 조립한다. `build_qr_payload` 에서 I/O 를 걷어낸 부분이라
/// 단위 테스트가 가능하다.
fn qr_params_with_name(
    endpoint_id_hex: &str,
    code: &str,
    addrs: &[iroh::TransportAddr],
    hostname: Option<String>,
) -> String {
    let mut params = vec![
        format!("endpoint={endpoint_id_hex}"),
        format!("code={code}"),
    ];
    for a in addrs {
        match a {
            iroh::TransportAddr::Relay(url) => {
                params.push(format!("relay={}", hex_encode(url.to_string().as_bytes())));
            }
            iroh::TransportAddr::Ip(sock) => {
                params.push(format!("addr={}", hex_encode(sock.to_string().as_bytes())));
            }
            _ => {}
        }
    }
    // 이름은 iOS 가 표시용으로만 쓴다. 못 읽어도 페어링 자체는 성립해야 하므로
    // Option 이고, 없으면 파라미터를 아예 넣지 않는다.
    if let Some(name) = hostname {
        if !name.is_empty() {
            params.push(format!("name={}", hex_encode(name.as_bytes())));
        }
    }
    format!("aim://pair?{}", params.join("&"))
}
```

그리고 `build_qr_payload` 를 이 함수를 쓰도록 바꾼다:

```rust
async fn build_qr_payload(handle: &NetworkHandle, code: &str) -> String {
    let endpoint_id_hex = hex_encode(handle.endpoint.id().as_bytes());
    let addr = wait_for_addr(&handle.endpoint).await;
    let hostname = hostname_for_display();
    qr_params_with_name(&endpoint_id_hex, code, &addr.addrs, hostname)
}

/// 표시용 Mac 이름. `scutil --get ComputerName` 이 사용자가 설정에서 정한 이름이라
/// `gethostname()`(네트워크 호스트명, 보통 "-" 가 섞임)보다 읽기 좋다.
fn hostname_for_display() -> Option<String> {
    let out = std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    let name = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}
```

- [ ] **Step 4: 테스트 재실행**

```bash
cd app/src-tauri && cargo test qr_payload
```

기대: 2개 통과.

- [ ] **Step 5: 전체 Rust 테스트로 회귀 확인**

```bash
cd app/src-tauri && cargo test 2>&1 | tail -5
```

기대: 기존 테스트 전부 통과(현재 기준 408개 중 필터 없이 전부).

- [ ] **Step 6: 커밋**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat: QR 페이로드에 Mac 표시 이름(name) 추가"
```

---

### Task 2: QR 파서가 호스트명을 읽게 한다

**Files:**
- Modify: `ios/Sources/NetworkTransport/NetworkClient.swift:159-180` (`parseQrPayload`)
- Test: `ios/Tests/NetworkTransportTests/NetworkClientTests.swift`

**Interfaces:**
- Consumes: Task 1 의 `name=<hex>` 파라미터
- Produces: `public static func parseQrPayload(_ payload: String) -> ParsedPairingPayload?`
  와 `public struct ParsedPairingPayload { public let endpointIdHex: String; public let code: String; public let relayUrl: String?; public let addresses: [String]; public let macName: String? }`

현재 `parseQrPayload` 는 `nonisolated static` 이고 이름 없는 튜플을 반환하며 internal 이다. Task 12 의 페어링 화면이 쓸 수 있도록 **public 명명 구조체**로 바꾼다.

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/NetworkTransportTests/NetworkClientTests.swift` 끝에 추가:

```swift
func testParseQrPayloadReadsMacName() throws {
    // "wanny-macbook" 의 UTF-8 hex
    let payload = "aim://pair?endpoint=deadbeef&code=123456&name=77616e6e792d6d6163626f6f6b"

    let parsed = try XCTUnwrap(NetworkClient.parseQrPayload(payload))

    XCTAssertEqual(parsed.endpointIdHex, "deadbeef")
    XCTAssertEqual(parsed.code, "123456")
    XCTAssertEqual(parsed.macName, "wanny-macbook")
}

/// 이름은 표시용이라 없어도 페어링은 성립해야 한다 — 구버전 Mac 과의 하위호환.
func testParseQrPayloadWithoutNameStillSucceeds() throws {
    let payload = "aim://pair?endpoint=deadbeef&code=123456"

    let parsed = try XCTUnwrap(NetworkClient.parseQrPayload(payload))

    XCTAssertNil(parsed.macName)
    XCTAssertEqual(parsed.endpointIdHex, "deadbeef")
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: 컴파일 실패(`parseQrPayload` 가 internal 이고 `.macName` 이 없음).

- [ ] **Step 3: 구현**

`NetworkClient.swift` 의 `parseQrPayload` 를 다음으로 교체한다:

```swift
/// QR 로 받은 페어링 정보. 이름은 표시용이라 optional 이다 — 구버전 Mac 은 보내지 않는다.
public struct ParsedPairingPayload: Equatable, Sendable {
    public let endpointIdHex: String
    public let code: String
    public let relayUrl: String?
    public let addresses: [String]
    public let macName: String?
}

nonisolated public static func parseQrPayload(_ payload: String) -> ParsedPairingPayload? {
    guard let components = URLComponents(string: payload),
          components.scheme == "aim", components.host == "pair",
          let items = components.queryItems,
          let endpointIdHex = items.first(where: { $0.name == "endpoint" })?.value,
          let code = items.first(where: { $0.name == "code" })?.value else {
        return nil
    }
    // relay/addr/name 은 Rust 쪽에서 URL 인코딩을 피하려고 hex 로 실어 보낸다
    // (Data(hexString:) 는 이 파일이 이미 BLE 쪽에서 재사용하고 있다).
    func decodeHex(_ value: String?) -> String? {
        guard let value, let data = Data(hexString: value) else { return nil }
        return String(data: data, encoding: .utf8)
    }
    return ParsedPairingPayload(
        endpointIdHex: endpointIdHex,
        code: code,
        relayUrl: decodeHex(items.first(where: { $0.name == "relay" })?.value),
        addresses: items.filter { $0.name == "addr" }.compactMap { decodeHex($0.value) },
        macName: decodeHex(items.first(where: { $0.name == "name" })?.value)
    )
}
```

호출부(`pair(qrPayload:)`, `NetworkClient.swift:67-81`)는 튜플 필드명이 그대로라 수정이 필요 없다. 컴파일 에러가 나면 `parsed.endpointIdHex` 등 같은 이름을 쓰는지 확인한다.

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 기존 테스트 + 신규 2개 전부 통과(1개는 기존 XCTSkip).

- [ ] **Step 5: 기존 앱 회귀 확인**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD"
```

기대: `BUILD SUCCEEDED`.

- [ ] **Step 6: 커밋**

```bash
git add ios/Sources/NetworkTransport/NetworkClient.swift ios/Tests/NetworkTransportTests/NetworkClientTests.swift
git commit -m "feat: QR 파서가 Mac 표시 이름을 읽고 ParsedPairingPayload 를 공개"
```

---

### Task 3: IrohEndpointProvider — Endpoint 공유

**Files:**
- Create: `ios/Sources/NetworkTransport/IrohEndpointProvider.swift`
- Modify: `ios/Sources/NetworkTransport/NetworkClient.swift:11-30` (init 에 provider 주입), `:127-130`, `:203-207` (bind 호출부)
- Test: `ios/Tests/NetworkTransportTests/IrohEndpointProviderTests.swift`

**Interfaces:**
- Produces: `public actor IrohEndpointProvider { public static let shared: IrohEndpointProvider; public init(); public func endpoint() async throws -> Endpoint }`
- Produces: `NetworkClient.init(endpointProvider: IrohEndpointProvider?)` — 기본값 `nil` 이면 기존처럼 자기 것을 만든다.

iroh Endpoint 는 소켓 하나가 아니라 자체 UDP 소켓 + relay 연결 + discovery 상태를 끌고 다닌다. 16개를 만들면 그게 통째로 16벌이 되므로 하나만 만들어 공유한다. QUIC 은 같은 소켓 위에서 연결을 다중화한다.

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/NetworkTransportTests/IrohEndpointProviderTests.swift` 생성:

```swift
import XCTest
@testable import NetworkTransport

final class IrohEndpointProviderTests: XCTestCase {

    /// 두 번 불러도 같은 Endpoint 여야 한다 — 이게 깨지면 장치마다 Endpoint 가
    /// 하나씩 생겨 16대에서 소켓·relay 연결이 16벌이 된다.
    func testEndpointIsCreatedOnceAndShared() async throws {
        let provider = IrohEndpointProvider()

        let first = try await provider.endpoint()
        let second = try await provider.endpoint()

        XCTAssertTrue(first === second)
    }

    /// 동시에 여러 세션이 요청해도 bind 는 한 번만 일어나야 한다.
    func testConcurrentRequestsShareASingleBind() async throws {
        let provider = IrohEndpointProvider()

        async let a = provider.endpoint()
        async let b = provider.endpoint()
        async let c = provider.endpoint()
        let (x, y, z) = try await (a, b, c)

        XCTAssertTrue(x === y)
        XCTAssertTrue(y === z)
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && mise exec -- tuist generate --no-open && \
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'IrohEndpointProvider' in scope`.

- [ ] **Step 3: 구현**

`ios/Sources/NetworkTransport/IrohEndpointProvider.swift` 생성:

```swift
import Foundation
import IrohLib

/// iroh Endpoint 를 **하나만** 만들어 공유한다.
///
/// Endpoint 는 소켓 하나가 아니라 자체 UDP 소켓 + relay 연결 + discovery 상태를
/// 끌고 다니는 무거운 객체다. 장치마다 `EndpointBuilder().bind()` 를 하면 그게
/// 그대로 N벌이 된다. QUIC 은 같은 소켓 위에서 연결을 다중화하므로, Endpoint 는
/// 하나로 두고 `connect(addr:alpn:)` 만 대상마다 호출하면 된다.
public actor IrohEndpointProvider {
    public static let shared = IrohEndpointProvider()

    private var endpoint: Endpoint?
    /// 생성 중인 Task 를 공유한다. 이게 없으면 동시에 들어온 요청마다 bind 가
    /// 돌아 Endpoint 가 여러 개 생긴다.
    private var bindTask: Task<Endpoint, Error>?

    public init() {}

    public func endpoint() async throws -> Endpoint {
        if let endpoint { return endpoint }
        if let bindTask { return try await bindTask.value }

        let task = Task<Endpoint, Error> {
            let builder = EndpointBuilder()
            builder.applyN0()
            builder.alpns(alpns: [NetworkClient.alpnData])
            return try await builder.bind()
        }
        bindTask = task
        do {
            let bound = try await task.value
            endpoint = bound
            bindTask = nil
            return bound
        } catch {
            // 실패한 Task 를 남겨두면 이후 요청이 영원히 같은 실패를 되받는다.
            bindTask = nil
            throw error
        }
    }
}
```

`NetworkClient` 의 `alpn` 은 현재 private 이므로 provider 가 쓸 수 있게 노출한다. `NetworkClient.swift:12` 를 다음으로 바꾼다:

```swift
    /// `IrohEndpointProvider` 도 같은 값으로 bind 해야 하므로 모듈 내부에 공개한다.
    static let alpnData = Data("aim/mirror/1".utf8)
    private static var alpn: Data { alpnData }
```

`NetworkClient` 에 주입 지점을 만든다. `NetworkClient.swift:18-30` 부근:

```swift
    private var endpoint: Endpoint?
    /// 주입되면 Endpoint 를 여기서 받아 쓴다(여러 장치가 공유). nil 이면 기존처럼
    /// 자기 것을 만든다 — 기존 App/AppBLE 의 동작을 바꾸지 않기 위한 기본값이다.
    private let endpointProvider: IrohEndpointProvider?

    public init(endpointProvider: IrohEndpointProvider? = nil) {
        self.endpointProvider = endpointProvider
        super.init()
    }
```

그리고 bind 하던 두 곳을 헬퍼로 모은다. `NetworkClient.swift:127-130` 과 `:203-207` 의 `let builder = ... let ep = try await builder.bind()` 를 각각 `let ep = try await resolveEndpoint()` 로 바꾸고, 헬퍼를 추가한다:

```swift
    private func resolveEndpoint() async throws -> Endpoint {
        if let endpointProvider { return try await endpointProvider.endpoint() }
        let builder = EndpointBuilder()
        builder.applyN0()
        builder.alpns(alpns: [Self.alpn])
        return try await builder.bind()
    }
```

`:203-207` 쪽은 결과를 `endpoint = ep` 로 보관하던 줄이 있다. 공유 Endpoint 를 받은 경우에는 **보관하지 않는다** — 이 인스턴스가 소유한 게 아니라서 정리할 권한이 없기 때문이다:

```swift
            let ep = try await resolveEndpoint()
            if endpointProvider == nil { endpoint = ep }
```

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 신규 2개 포함 전부 통과.

- [ ] **Step 5: 기존 앱 회귀 확인**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD" && \
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFeatureTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: `BUILD SUCCEEDED`, MirrorFeatureTests 47개 통과.

- [ ] **Step 6: 커밋**

```bash
git add ios/Sources/NetworkTransport/IrohEndpointProvider.swift ios/Sources/NetworkTransport/NetworkClient.swift ios/Tests/NetworkTransportTests/IrohEndpointProviderTests.swift
git commit -m "feat: iroh Endpoint 를 공유하는 IrohEndpointProvider 추가"
```

---

### Task 4: probe 가 연결을 돌려주도록 분해

**Files:**
- Modify: `ios/Sources/NetworkTransport/NetworkClient.swift:89-116` (`fetchSnapshotOnce`), `:118-157` (`dialAuthenticateAndReadOne`)
- Test: `ios/Tests/NetworkTransportTests/NetworkClientTests.swift`

**Interfaces:**
- Produces: `public struct ProbeResult { public let connection: Connection; public let channel: SealedChannel; public let firstSnapshot: MirrorSnapshot }`
- Produces: `public func probe(endpointIdHex: String, relayUrl: String?, addresses: [String], timeoutSeconds: Double) async throws -> ProbeResult`
- 기존 `fetchSnapshotOnce(timeoutSeconds:)` 는 시그니처 그대로 유지한다(위젯이 쓴다).

probe 성공 시 그 연결을 그대로 스트리밍으로 이어가야 한다. 다시 dial 하면 가장 비싼 부분(hole-punch·QUIC·인증 왕복)을 두 번 낸다.

- [ ] **Step 1: 실패하는 테스트 작성**

실제 iroh 연결은 단위 테스트가 불가능하다. 여기서 검증할 수 있는 건 **타임아웃 계약**뿐이다. `NetworkClientTests.swift` 끝에 추가:

```swift
/// probe 는 도달 불가한 대상에 대해 timeoutSeconds 안에 반드시 반환해야 한다.
/// (실제 연결 성공 경로는 실기 검증 몫 — Task 15)
func testProbeTimesOutWithinBudget() async {
    let client = await NetworkClient()
    let started = Date()

    do {
        // 형식은 맞지만 존재하지 않는 EndpointId (32바이트 hex)
        _ = try await client.probe(
            endpointIdHex: String(repeating: "ab", count: 32),
            relayUrl: nil,
            addresses: [],
            timeoutSeconds: 1
        )
        XCTFail("도달 불가한 대상인데 성공했다")
    } catch {
        let elapsed = Date().timeIntervalSince(started)
        XCTAssertLessThan(elapsed, 4, "타임아웃 예산(1초)+여유를 넘겼다: \(elapsed)초")
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `value of type 'NetworkClient' has no member 'probe'`.

- [ ] **Step 3: 구현 — 연결을 반환하는 형태로 분해**

`dialAuthenticateAndReadOne` 은 첫 스냅샷만 돌려주고 연결을 놓아버린다(위젯은 단발성이라 그게 맞다). 연결까지 함께 돌려주는 형태로 바꾸고, 기존 함수는 그것을 감싸는 래퍼로 남긴다.

`NetworkClient.swift` 에 추가:

```swift
/// probe 결과. 연결을 **닫지 않고** 돌려준다 — 호출부가 그대로 스트리밍으로
/// 이어가거나(AppMulti), 즉시 닫는다(위젯).
public struct ProbeResult {
    public let connection: Connection
    public let channel: SealedChannel
    public let firstSnapshot: MirrorSnapshot
}

/// dial → 인증 → 첫 스냅샷까지 하고 **연결을 살려둔 채** 반환한다.
/// 실패 시 재시도하지 않는다 — 재시도 정책은 호출부(DeviceSession)가 정한다.
public func probe(
    endpointIdHex: String,
    relayUrl: String?,
    addresses: [String],
    timeoutSeconds: Double
) async throws -> ProbeResult {
    let work = Task { () throws -> ProbeResult in
        try await dialAuthenticateAndOpen(
            endpointIdHex: endpointIdHex, relayUrl: relayUrl, addresses: addresses
        )
    }
    let watchdog = Task {
        try? await Task.sleep(nanoseconds: UInt64(timeoutSeconds * 1_000_000_000))
        work.cancel()
    }
    defer { watchdog.cancel() }
    do {
        return try await work.value
    } catch is CancellationError {
        throw NetworkClientError.fetchTimedOut
    }
}
```

`dialAuthenticateAndReadOne` 의 본문을 `dialAuthenticateAndOpen` 으로 옮긴다 — 시그니처만 바뀌고 로직(`Task.checkCancellation()` 포함)은 그대로다:

```swift
private func dialAuthenticateAndOpen(
    endpointIdHex: String, relayUrl: String?, addresses: [String]
) async throws -> ProbeResult {
    guard let idBytes = Data(hexString: endpointIdHex) else {
        throw NetworkClientError.needsPairing
    }
    let endpointId = try EndpointId.fromBytes(bytes: idBytes)
    let addr = EndpointAddr(id: endpointId, relayUrl: relayUrl, addresses: addresses)

    let ep = try await resolveEndpoint()
    let conn = try await ep.connect(addr: addr, alpn: Self.alpn)
    // code: nil — 재연결 경로다(이미 저장된 토큰으로 인증). 새 페어링은 여기서
    // 절대 일어나지 않는다.
    let channel = try await authenticate(conn: conn, code: nil)

    let recv = try await conn.acceptUni()
    var buffer = Data()
    while true {
        // 워치독이 cancel() 을 걸어도 uniffi 브리지가 자체적으로 취소를 감지한다는
        // 보장이 없다 — 매 반복 최소 한 번은 취소 지점을 만들어 무한정 도는 걸 막는다.
        try Task.checkCancellation()
        let chunk = try await recv.read(sizeLimit: Self.snapshotChunkSizeLimit)
        buffer.append(chunk)
        while let newlineIndex = buffer.firstIndex(of: 0x0A) {
            let lineData = Data(buffer[..<newlineIndex])
            buffer.removeSubrange(...newlineIndex)
            // 기존 dialAuthenticateAndReadOne 의 라인 파싱 로직을 그대로 옮긴다.
            // 스냅샷 한 장을 얻으면 연결을 닫지 말고 ProbeResult 로 반환한다.
            if let snapshot = try decodeSnapshotLine(lineData, channel: channel) {
                return ProbeResult(connection: conn, channel: channel, firstSnapshot: snapshot)
            }
        }
    }
}
```

> 구현 주의: 기존 `dialAuthenticateAndReadOne` 안의 라인 복호화·디코딩 부분을 그대로 `decodeSnapshotLine(_:channel:)` 사설 헬퍼로 뽑아 쓴다. 파싱 규칙(개행 구분, `SealedChannel` 복호화, 버전 검사)을 **새로 쓰지 말고 옮기기만** 한다.

기존 `fetchSnapshotOnce` 는 래퍼가 된다:

```swift
/// 위젯 전용. probe 로 연결을 열고 첫 스냅샷만 받은 뒤 **즉시 닫는다**.
public func fetchSnapshotOnce(timeoutSeconds: Double) async throws -> MirrorSnapshot {
    guard let endpointHex = NetworkTokenStore.loadEndpointIdHex() else {
        throw NetworkClientError.needsPairing
    }
    let result = try await probe(
        endpointIdHex: endpointHex,
        relayUrl: NetworkTokenStore.loadRelayUrl(),
        addresses: NetworkTokenStore.loadAddresses(),
        timeoutSeconds: timeoutSeconds
    )
    await result.connection.close()
    return result.firstSnapshot
}
```

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme NetworkTransportTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 전부 통과.

- [ ] **Step 5: 위젯 회귀 확인 — App 빌드 + WidgetShared 테스트**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD" && \
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme WidgetSharedTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: `BUILD SUCCEEDED`, WidgetSharedTests 11개 통과.

- [ ] **Step 6: 커밋**

```bash
git add ios/Sources/NetworkTransport/NetworkClient.swift ios/Tests/NetworkTransportTests/NetworkClientTests.swift
git commit -m "refactor: probe 가 연결을 살려둔 채 반환하도록 분해 (위젯은 즉시 닫는 래퍼)"
```

---

### Task 5: Fleet 모듈 + 장치 레지스트리

**Files:**
- Modify: `ios/Project.swift` (framework `Fleet` + `FleetTests` 타겟, 스킴 추가)
- Create: `ios/Sources/Fleet/Device.swift`, `ios/Sources/Fleet/DeviceRegistry.swift`
- Test: `ios/Tests/FleetTests/DeviceRegistryTests.swift`

**Interfaces:**
- Produces: `public struct Device: Codable, Equatable, Identifiable { public var id: String { endpointIdHex } ... }`
- Produces: `public protocol DeviceRegistryStore { func read() throws -> Data?; func write(_ data: Data) throws }`
- Produces: `public final class DeviceRegistry { public init(store: DeviceRegistryStore); public func load() throws -> [Device]; public func upsert(_ device: Device) throws -> [Device]; public func remove(endpointIdHex: String) throws -> [Device]; public static let maxDevices = 16 }`
- Produces: `public enum DeviceRegistryError: Error, Equatable { case deviceLimitReached, corrupted }`

- [ ] **Step 1: Tuist 타겟 추가**

`ios/Project.swift` 의 `targets:` 배열에서 `framework("WidgetShared", ...)` 줄 **다음에** 추가:

```swift
        framework("Fleet", deps: [.target(name: "Wire"), .target(name: "MirrorFormat"), .target(name: "NetworkTransport")], deploymentTargets: iOS),
        unitTests("FleetTests", for: "Fleet", deploymentTargets: iOS),
```

그리고 `schemes:` 배열에 `MirrorFormatTests` 스킴 선언과 같은 형식으로 추가:

```swift
        .scheme(
            name: "FleetTests",
            buildAction: .buildAction(targets: [.target("FleetTests")]),
            testAction: .targets([.testableTarget(target: .target("FleetTests"))])
        ),
```

- [ ] **Step 2: 실패하는 테스트 작성**

`ios/Tests/FleetTests/DeviceRegistryTests.swift` 생성:

```swift
import XCTest
@testable import Fleet

/// 테스트용 인메모리 저장소. 실제 Keychain 은 호스트 없는 유닛 테스트 번들에서
/// errSecMissingEntitlement 로 실패한다(BLETransportTests/PairingClientTests.swift:39-47
/// 의 선례) — 그래서 저장소를 프로토콜로 주입받는다.
private final class MemoryStore: DeviceRegistryStore {
    var data: Data?
    func read() throws -> Data? { data }
    func write(_ data: Data) throws { self.data = data }
}

final class DeviceRegistryTests: XCTestCase {

    private func makeDevice(_ hex: String, name: String? = nil) -> Device {
        Device(
            endpointIdHex: hex,
            token: "token-\(hex)",
            relayUrl: nil,
            addresses: [],
            macHostname: name,
            userLabel: nil,
            sortIndex: 0
        )
    }

    func testEmptyRegistryLoadsAsEmptyList() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        XCTAssertEqual(try registry.load(), [])
    }

    func testUpsertRoundTrips() throws {
        let store = MemoryStore()
        let registry = DeviceRegistry(store: store)

        _ = try registry.upsert(makeDevice("aa", name: "집 맥"))

        XCTAssertEqual(try DeviceRegistry(store: store).load(), [makeDevice("aa", name: "집 맥")])
    }

    /// 같은 Mac 을 다시 스캔하면 새 항목을 만들지 않고 갱신한다 — Mac 의 IP 가
    /// 바뀌었을 때 재스캔으로 고치는 경로가 된다.
    func testRescanningSameEndpointMergesInsteadOfAdding() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        _ = try registry.upsert(makeDevice("aa"))

        var updated = makeDevice("aa")
        updated.addresses = ["192.168.0.5:1234"]
        updated.token = "새-토큰"
        let devices = try registry.upsert(updated)

        XCTAssertEqual(devices.count, 1)
        XCTAssertEqual(devices[0].addresses, ["192.168.0.5:1234"])
        XCTAssertEqual(devices[0].token, "새-토큰")
    }

    /// 재스캔은 연결 정보만 갱신하고 사용자가 붙인 이름은 보존해야 한다.
    func testRescanKeepsUserLabel() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        var first = makeDevice("aa")
        first.userLabel = "작업실"
        _ = try registry.upsert(first)

        let devices = try registry.upsert(makeDevice("aa", name: "새-호스트명"))

        XCTAssertEqual(devices[0].userLabel, "작업실")
        XCTAssertEqual(devices[0].macHostname, "새-호스트명")
    }

    func testDeviceLimitIsEnforced() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        for i in 0..<DeviceRegistry.maxDevices {
            _ = try registry.upsert(makeDevice(String(format: "%02x", i)))
        }

        XCTAssertThrowsError(try registry.upsert(makeDevice("ff"))) { error in
            XCTAssertEqual(error as? DeviceRegistryError, .deviceLimitReached)
        }
    }

    /// 한도에 도달했어도 **기존 장치 갱신**은 허용돼야 한다.
    func testLimitDoesNotBlockUpdatingAnExistingDevice() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        for i in 0..<DeviceRegistry.maxDevices {
            _ = try registry.upsert(makeDevice(String(format: "%02x", i)))
        }

        XCTAssertNoThrow(try registry.upsert(makeDevice("00", name: "갱신")))
    }

    /// 손상된 레지스트리를 빈 것으로 취급하면 16대 페어링이 조용히 날아간다.
    func testCorruptedRegistryThrowsInsteadOfSilentlyResetting() {
        let store = MemoryStore()
        store.data = Data("이건 JSON 이 아니다".utf8)

        XCTAssertThrowsError(try DeviceRegistry(store: store).load()) { error in
            XCTAssertEqual(error as? DeviceRegistryError, .corrupted)
        }
    }

    func testRemoveDeletesOnlyTheNamedDevice() throws {
        let registry = DeviceRegistry(store: MemoryStore())
        _ = try registry.upsert(makeDevice("aa"))
        _ = try registry.upsert(makeDevice("bb"))

        let devices = try registry.remove(endpointIdHex: "aa")

        XCTAssertEqual(devices.map(\.endpointIdHex), ["bb"])
    }
}
```

- [ ] **Step 3: 재생성 + 테스트 실행해서 실패 확인**

```bash
cd ios && mise exec -- tuist generate --no-open && \
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'DeviceRegistry' in scope`.

- [ ] **Step 4: 구현**

`ios/Sources/Fleet/Device.swift`:

```swift
import Foundation

/// 페어링된 Mac 한 대. `endpointIdHex` 가 키다 — iroh EndpointId 는 Mac 마다
/// 고유하고 재설치 전까지 안정적이다.
public struct Device: Codable, Equatable, Identifiable, Sendable {
    public var endpointIdHex: String
    public var token: String
    public var relayUrl: String?
    public var addresses: [String]
    /// QR 로 받은 Mac 이름. Mac 이름을 나중에 바꿔도 갱신되지 않는다.
    public var macHostname: String?
    /// 사용자가 덮어쓴 이름. 있으면 이쪽이 우선한다.
    public var userLabel: String?
    public var sortIndex: Int

    public var id: String { endpointIdHex }

    public init(
        endpointIdHex: String,
        token: String,
        relayUrl: String?,
        addresses: [String],
        macHostname: String?,
        userLabel: String?,
        sortIndex: Int
    ) {
        self.endpointIdHex = endpointIdHex
        self.token = token
        self.relayUrl = relayUrl
        self.addresses = addresses
        self.macHostname = macHostname
        self.userLabel = userLabel
        self.sortIndex = sortIndex
    }

    /// 표시 이름 우선순위: 사용자가 붙인 이름 > Mac 이 보낸 이름 > EndpointId 앞 8자
    public var displayName: String {
        if let userLabel, !userLabel.isEmpty { return userLabel }
        if let macHostname, !macHostname.isEmpty { return macHostname }
        return String(endpointIdHex.prefix(8))
    }
}
```

`ios/Sources/Fleet/DeviceRegistry.swift`:

```swift
import Foundation

public enum DeviceRegistryError: Error, Equatable {
    case deviceLimitReached
    case corrupted
}

/// 레지스트리 바이트를 어디에 둘지. 실제 앱은 Keychain, 테스트는 인메모리를 쓴다.
public protocol DeviceRegistryStore {
    func read() throws -> Data?
    func write(_ data: Data) throws
}

/// 장치 목록 전체를 **항목 하나**로 저장한다. 16대면 몇 KB라 읽기/쓰기 한 번이면
/// 되고 원자적이다. 장치마다 항목을 따로 두면 열거하려고 kSecMatchLimitAll 쿼리를
/// 돌리고 계정 문자열을 파싱해야 한다.
public final class DeviceRegistry {
    public static let maxDevices = 16

    private let store: DeviceRegistryStore

    public init(store: DeviceRegistryStore) {
        self.store = store
    }

    public func load() throws -> [Device] {
        guard let data = try store.read(), !data.isEmpty else { return [] }
        do {
            return try JSONDecoder().decode([Device].self, from: data)
        } catch {
            // 빈 목록으로 조용히 시작하면 16대 페어링이 통째로 날아간다.
            // 호출부가 사용자에게 알릴 수 있도록 던진다.
            throw DeviceRegistryError.corrupted
        }
    }

    /// 같은 `endpointIdHex` 가 이미 있으면 연결 정보만 갱신하고, 사용자가 붙인
    /// 이름(`userLabel`)과 정렬 순서는 보존한다.
    @discardableResult
    public func upsert(_ device: Device) throws -> [Device] {
        var devices = try load()
        if let index = devices.firstIndex(where: { $0.endpointIdHex == device.endpointIdHex }) {
            var merged = device
            merged.userLabel = devices[index].userLabel ?? device.userLabel
            merged.sortIndex = devices[index].sortIndex
            devices[index] = merged
        } else {
            guard devices.count < Self.maxDevices else {
                throw DeviceRegistryError.deviceLimitReached
            }
            var appended = device
            appended.sortIndex = (devices.map(\.sortIndex).max() ?? -1) + 1
            devices.append(appended)
        }
        try persist(devices)
        return devices
    }

    @discardableResult
    public func remove(endpointIdHex: String) throws -> [Device] {
        var devices = try load()
        devices.removeAll { $0.endpointIdHex == endpointIdHex }
        try persist(devices)
        return devices
    }

    private func persist(_ devices: [Device]) throws {
        try store.write(try JSONEncoder().encode(devices))
    }
}
```

- [ ] **Step 5: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 8개 전부 통과.

- [ ] **Step 6: 커밋**

```bash
git add ios/Project.swift ios/Sources/Fleet ios/Tests/FleetTests
git commit -m "feat: Fleet 모듈과 장치 레지스트리(16대 상한·재스캔 병합) 추가"
```

---

### Task 6: Keychain 저장소 구현체

**Files:**
- Create: `ios/Sources/Fleet/KeychainRegistryStore.swift`
- Test: `ios/Tests/FleetTests/KeychainRegistryStoreTests.swift`

**Interfaces:**
- Consumes: `DeviceRegistryStore` (Task 5)
- Produces: `public struct KeychainRegistryStore: DeviceRegistryStore { public init(service: String) }`

**access group 을 쓰지 않는다.** 위젯이 없어 프로세스 간 공유가 없으므로, 기존 앱이 겪은 팀 접두사 문제가 이 앱에는 존재하지 않는다.

- [ ] **Step 1: 실패하는 테스트 작성**

호스트 없는 유닛 테스트 번들에서는 Keychain 쓰기가 `errSecMissingEntitlement` 로 실패한다 — 기존 저장소에 같은 선례가 있다(`BLETransportTests/PairingClientTests.swift:39-47`). 같은 방식으로 스킵하되, **스킵하지 않는 부분**(쿼리 구성)은 검증한다.

`ios/Tests/FleetTests/KeychainRegistryStoreTests.swift` 생성:

```swift
import XCTest
@testable import Fleet

final class KeychainRegistryStoreTests: XCTestCase {

    /// 실제 Keychain 왕복은 호스트 앱 없는 유닛 테스트 번들에서 돌지 않는다
    /// (errSecMissingEntitlement). BLETransportTests/PairingClientTests.swift:39-47
    /// 과 같은 이유로 스킵하고, 실제 검증은 Task 15 실기 확인에서 한다.
    func testRoundTripOnDevice() throws {
        throw XCTSkip("호스트 없는 테스트 번들에서는 Keychain 접근이 거부된다 — 실기 검증(Task 15) 항목")
    }

    /// access group 을 넣지 않는 것이 이 앱의 설계 결정이다. 실수로 되살아나면
    /// 기존 앱이 겪은 팀 접두사 문제를 그대로 물려받는다.
    func testQueryDoesNotUseAnAccessGroup() {
        let store = KeychainRegistryStore(service: "test.service")
        let query = store.baseQuery()

        XCTAssertNil(query[kSecAttrAccessGroup as String])
        XCTAssertEqual(query[kSecAttrService as String] as? String, "test.service")
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'KeychainRegistryStore' in scope`.

- [ ] **Step 3: 구현**

`ios/Sources/Fleet/KeychainRegistryStore.swift`:

```swift
import Foundation
import Security
import os

/// 장치 레지스트리를 Keychain 항목 하나에 담는다.
///
/// access group 을 **쓰지 않는다** — 위젯이 없어 프로세스 간 공유가 없으므로
/// `$(AppIdentifierPrefix)` 치환값(팀 ID)을 코드에 들고 있을 이유가 없다.
public struct KeychainRegistryStore: DeviceRegistryStore {
    private static let logger = Logger(
        subsystem: "co.kr.wannypark.aiagentmonitor.multi", category: "KeychainRegistryStore"
    )

    private let service: String
    private let account = "device-registry"

    public init(service: String = "co.kr.wannypark.aiagentmonitor.multi") {
        self.service = service
    }

    func baseQuery() -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    public func read() throws -> Data? {
        var query = baseQuery()
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var out: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &out)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = out as? Data else {
            Self.logger.error("레지스트리 조회 실패 status=\(status, privacy: .public)")
            return nil
        }
        return data
    }

    public func write(_ data: Data) throws {
        SecItemDelete(baseQuery() as CFDictionary)

        var query = baseQuery()
        query[kSecValueData as String] = data
        query[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly

        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else {
            Self.logger.error("레지스트리 저장 실패 status=\(status, privacy: .public)")
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
        }
    }
}
```

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 1개 스킵, 나머지 전부 통과.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/Fleet/KeychainRegistryStore.swift ios/Tests/FleetTests/KeychainRegistryStoreTests.swift
git commit -m "feat: 장치 레지스트리 Keychain 저장소 (access group 없음)"
```

---

### Task 7: 마지막 스냅샷 파일 캐시

**Files:**
- Create: `ios/Sources/Fleet/DeviceSnapshotCache.swift`
- Test: `ios/Tests/FleetTests/DeviceSnapshotCacheTests.swift`

**Interfaces:**
- Produces: `public struct CachedSnapshot: Codable, Equatable { public let snapshot: MirrorSnapshot; public let fetchedAt: Date }`
- Produces: `public final class DeviceSnapshotCache { public init(directory: URL); public func load(endpointIdHex: String) -> CachedSnapshot?; public func save(_ snapshot: MirrorSnapshot, fetchedAt: Date, endpointIdHex: String) }`

Keychain 이 아닌 파일인 이유: 스트리밍 중엔 초당 갱신인데 Keychain 쓰기는 느리고 용도도 아니다.

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/FleetTests/DeviceSnapshotCacheTests.swift` 생성:

```swift
import XCTest
import Wire
@testable import Fleet

final class DeviceSnapshotCacheTests: XCTestCase {

    private var directory: URL!

    override func setUpWithError() throws {
        directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private func makeSnapshot() throws -> MirrorSnapshot {
        let json = #"{"v":1,"t":1758000000,"a":[{"k":0,"r":1.5,"t5":1200,"p5":40,"pj":[]}]}"#
        return try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
    }

    func testLoadReturnsNilWhenNothingSaved() {
        let cache = DeviceSnapshotCache(directory: directory)
        XCTAssertNil(cache.load(endpointIdHex: "aa"))
    }

    func testSavedSnapshotRoundTrips() throws {
        let cache = DeviceSnapshotCache(directory: directory)
        let snapshot = try makeSnapshot()
        let fetchedAt = Date(timeIntervalSince1970: 1_758_000_100)

        cache.save(snapshot, fetchedAt: fetchedAt, endpointIdHex: "aa")

        let loaded = cache.load(endpointIdHex: "aa")
        XCTAssertEqual(loaded?.snapshot, snapshot)
        XCTAssertEqual(loaded?.fetchedAt, fetchedAt)
    }

    /// 장치별로 분리돼야 한다 — 한 Mac 의 캐시가 다른 Mac 에 보이면 안 된다.
    func testCachesAreIsolatedPerDevice() throws {
        let cache = DeviceSnapshotCache(directory: directory)
        cache.save(try makeSnapshot(), fetchedAt: Date(), endpointIdHex: "aa")

        XCTAssertNil(cache.load(endpointIdHex: "bb"))
    }

    /// endpointIdHex 가 파일명이 되므로 경로 조작 문자가 섞이면 안 된다.
    func testNonHexIdentifierIsRejected() throws {
        let cache = DeviceSnapshotCache(directory: directory)

        cache.save(try makeSnapshot(), fetchedAt: Date(), endpointIdHex: "../../etc/passwd")

        XCTAssertNil(cache.load(endpointIdHex: "../../etc/passwd"))
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'DeviceSnapshotCache' in scope`.

- [ ] **Step 3: 구현**

`ios/Sources/Fleet/DeviceSnapshotCache.swift`:

```swift
import Foundation
import Wire
import os

public struct CachedSnapshot: Codable, Equatable, Sendable {
    public let snapshot: MirrorSnapshot
    public let fetchedAt: Date

    public init(snapshot: MirrorSnapshot, fetchedAt: Date) {
        self.snapshot = snapshot
        self.fetchedAt = fetchedAt
    }
}

/// 장치별 마지막 스냅샷. 앱 시작 시 probe 가 끝나기 전에도 목록을 채우는 용도라
/// 비밀이 아니고, 스트리밍 중 초당 갱신이라 Keychain 이 아닌 파일에 둔다.
public final class DeviceSnapshotCache {
    private static let logger = Logger(
        subsystem: "co.kr.wannypark.aiagentmonitor.multi", category: "DeviceSnapshotCache"
    )

    private let directory: URL

    public init(directory: URL) {
        self.directory = directory
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    /// Application Support 아래 기본 위치.
    public static func defaultDirectory() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("DeviceSnapshots", isDirectory: true)
    }

    public func load(endpointIdHex: String) -> CachedSnapshot? {
        guard let url = fileURL(endpointIdHex) else { return nil }
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(CachedSnapshot.self, from: data)
    }

    public func save(_ snapshot: MirrorSnapshot, fetchedAt: Date, endpointIdHex: String) {
        guard let url = fileURL(endpointIdHex) else {
            Self.logger.error("캐시 파일명으로 쓸 수 없는 식별자")
            return
        }
        let cached = CachedSnapshot(snapshot: snapshot, fetchedAt: fetchedAt)
        guard let data = try? JSONEncoder().encode(cached) else { return }
        try? data.write(to: url, options: .atomic)
    }

    /// `endpointIdHex` 가 그대로 파일명이 되므로 hex 문자만 허용한다 — 경로 조작 차단.
    private func fileURL(_ endpointIdHex: String) -> URL? {
        let allowed = CharacterSet(charactersIn: "0123456789abcdefABCDEF")
        guard !endpointIdHex.isEmpty,
              endpointIdHex.unicodeScalars.allSatisfy(allowed.contains) else { return nil }
        return directory.appendingPathComponent("\(endpointIdHex).json")
    }
}
```

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 4개 신규 포함 전부 통과.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/Fleet/DeviceSnapshotCache.swift ios/Tests/FleetTests/DeviceSnapshotCacheTests.swift
git commit -m "feat: 장치별 마지막 스냅샷 파일 캐시"
```

---

### Task 8: 장치 상태기계

**Files:**
- Create: `ios/Sources/Fleet/DeviceStatus.swift`
- Test: `ios/Tests/FleetTests/DeviceStatusTests.swift`

**Interfaces:**
- Produces: `public enum DeviceStatus: Equatable, Sendable { case idle, probing, online, unstable(failureCount: Int), offline, needsRepairing, versionMismatch }`
- Produces: `public enum DeviceEvent: Equatable, Sendable { case probeStarted, probeSucceeded, streamEstablished, connectionLost, probeFailed, authRejected, versionRejected, retriggered }`
- Produces: `public struct DeviceStatusMachine { public static let failureThreshold = 3; public static func next(_ status: DeviceStatus, on event: DeviceEvent) -> DeviceStatus }`

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/FleetTests/DeviceStatusTests.swift` 생성:

```swift
import XCTest
@testable import Fleet

final class DeviceStatusTests: XCTestCase {

    private func next(_ status: DeviceStatus, _ event: DeviceEvent) -> DeviceStatus {
        DeviceStatusMachine.next(status, on: event)
    }

    func testProbeSuccessGoesOnline() {
        XCTAssertEqual(next(.probing, .probeSucceeded), .online)
    }

    func testConnectionLossFromOnlineBecomesUnstable() {
        XCTAssertEqual(next(.online, .connectionLost), .unstable(failureCount: 1))
    }

    /// 연속 3회 실패해야 오프라인으로 떨어진다 — 일시적 끊김과 실제 꺼짐을 구분한다.
    func testOfflineOnlyAfterThreeConsecutiveFailures() {
        var status = next(.online, .connectionLost)      // 1
        XCTAssertEqual(status, .unstable(failureCount: 1))
        status = next(status, .probeFailed)               // 2
        XCTAssertEqual(status, .unstable(failureCount: 2))
        status = next(status, .probeFailed)               // 3
        XCTAssertEqual(status, .offline)
    }

    /// 중간에 한 번 성공하면 실패 카운터가 리셋돼야 한다.
    func testSuccessResetsFailureCount() {
        var status = next(.online, .connectionLost)
        status = next(status, .probeFailed)
        XCTAssertEqual(status, .unstable(failureCount: 2))

        status = next(status, .probeSucceeded)
        XCTAssertEqual(status, .online)

        XCTAssertEqual(next(status, .connectionLost), .unstable(failureCount: 1))
    }

    /// 인증 거부는 재시도로 풀리지 않는다. NetworkClient.swift:213-222 의 교훈 —
    /// 성공할 수 없는 재시도가 QR 스캐너를 깜빡이게 만든 버그가 있었다.
    func testAuthRejectionIsTerminalAndNotRetried() {
        XCTAssertEqual(next(.probing, .authRejected), .needsRepairing)
        // 재탐색 트리거가 와도 상태가 바뀌지 않는다.
        XCTAssertEqual(next(.needsRepairing, .retriggered), .needsRepairing)
        XCTAssertEqual(next(.needsRepairing, .probeFailed), .needsRepairing)
    }

    func testVersionMismatchIsTerminal() {
        XCTAssertEqual(next(.probing, .versionRejected), .versionMismatch)
        XCTAssertEqual(next(.versionMismatch, .retriggered), .versionMismatch)
    }

    /// 오프라인 탈출은 트리거(타이머/포어그라운드/당겨서 새로고침)로만 일어난다.
    func testOfflineLeavesOnlyOnRetrigger() {
        XCTAssertEqual(next(.offline, .probeFailed), .offline)
        XCTAssertEqual(next(.offline, .retriggered), .probing)
    }

    func testStreamEstablishedKeepsOnline() {
        XCTAssertEqual(next(.online, .streamEstablished), .online)
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'DeviceStatusMachine' in scope`.

- [ ] **Step 3: 구현**

`ios/Sources/Fleet/DeviceStatus.swift`:

```swift
import Foundation

public enum DeviceStatus: Equatable, Sendable {
    case idle
    case probing
    case online
    /// 연결이 끊겨 재시도 중. `failureCount` 가 임계값에 닿으면 offline 이 된다.
    case unstable(failureCount: Int)
    case offline
    /// 토큰 폐기·Mac 재설치. 재시도로는 절대 풀리지 않는다.
    case needsRepairing
    case versionMismatch
}

public enum DeviceEvent: Equatable, Sendable {
    case probeStarted
    case probeSucceeded
    case streamEstablished
    case connectionLost
    case probeFailed
    case authRejected
    case versionRejected
    /// 타이머(3분)/포어그라운드 복귀/당겨서 새로고침
    case retriggered
}

/// 장치 하나의 상태 전이 규칙. 순수 함수라 테스트로 고정한다.
public struct DeviceStatusMachine {
    public static let failureThreshold = 3

    public static func next(_ status: DeviceStatus, on event: DeviceEvent) -> DeviceStatus {
        // 종단 상태는 어떤 이벤트로도 벗어나지 않는다. 사용자가 다시 페어링해야만
        // 레지스트리가 바뀌고 새 세션이 생긴다.
        switch status {
        case .needsRepairing, .versionMismatch:
            return status
        default:
            break
        }

        switch event {
        case .authRejected:
            return .needsRepairing
        case .versionRejected:
            return .versionMismatch
        case .probeStarted:
            return .probing
        case .probeSucceeded, .streamEstablished:
            return .online
        case .connectionLost, .probeFailed:
            let failures = currentFailureCount(status) + 1
            return failures >= failureThreshold ? .offline : .unstable(failureCount: failures)
        case .retriggered:
            return status == .offline ? .probing : status
        }
    }

    private static func currentFailureCount(_ status: DeviceStatus) -> Int {
        switch status {
        case .unstable(let count): return count
        case .offline: return failureThreshold
        default: return 0
        }
    }
}
```

> 주의: `.offline` 상태에서 `.probeFailed` 가 오면 `currentFailureCount` 가 임계값이라 다시 `.offline` 이 된다 — 테스트 `testOfflineLeavesOnlyOnRetrigger` 가 이걸 고정한다.

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 8개 신규 포함 전부 통과.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/Fleet/DeviceStatus.swift ios/Tests/FleetTests/DeviceStatusTests.swift
git commit -m "feat: 장치 상태기계 (3회 실패 오프라인, 인증거부·버전불일치는 종단)"
```

---

### Task 9: 목록 표시 규칙

**Files:**
- Create: `ios/Sources/Fleet/DeviceListPresentation.swift`
- Test: `ios/Tests/FleetTests/DeviceListPresentationTests.swift`

**Interfaces:**
- Consumes: `Device`(Task 5), `DeviceStatus`(Task 8), `CachedSnapshot`(Task 7)
- Produces: `public struct DeviceRowModel: Equatable { public let id: String; public let title: String; public let statusText: String; public let freshnessText: String?; public let agents: [AgentRowModel]; public let hiddenAgentCount: Int }`
- Produces: `public struct AgentRowModel: Equatable { public let name: String; public let rateText: String?; public let fiveHour: QuotaWindowText?; public let weekly: QuotaWindowText? }`
- Produces: `public struct QuotaWindowText: Equatable { public let percentText: String; public let percent: Float }`
- Produces: `public enum DeviceListPresentation { public static let visibleAgentLimit = 2; public static func row(device:status:cached:now:) -> DeviceRowModel; public static func sorted(_ rows: [(Device, DeviceStatus)]) -> [Device] }`

핵심 규칙: **오프라인이면 tok/s 같은 순간값은 감추고 한도 %는 유지한다.** 한도는 몇 분 지나도 유효하지만 tok/s 는 지금 이 순간의 값이라 낡으면 의미가 없다.

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/FleetTests/DeviceListPresentationTests.swift` 생성:

```swift
import XCTest
import Wire
@testable import Fleet

final class DeviceListPresentationTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_758_000_600)

    private func device(_ hex: String, host: String? = nil, label: String? = nil) -> Device {
        Device(endpointIdHex: hex, token: "t", relayUrl: nil, addresses: [],
               macHostname: host, userLabel: label, sortIndex: 0)
    }

    private func cached(agentJSON: String, minutesAgo: Int = 0) throws -> CachedSnapshot {
        let json = #"{"v":1,"t":1758000000,"a":[\#(agentJSON)]}"#
        let snapshot = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        return CachedSnapshot(
            snapshot: snapshot,
            fetchedAt: now.addingTimeInterval(TimeInterval(-60 * minutesAgo))
        )
    }

    // MARK: - 이름

    func testTitlePrefersUserLabelOverHostname() {
        let row = DeviceListPresentation.row(
            device: device("aabbccdd", host: "호스트", label: "작업실"),
            status: .online, cached: nil, now: now
        )
        XCTAssertEqual(row.title, "작업실")
    }

    func testTitleFallsBackToEndpointPrefix() {
        let row = DeviceListPresentation.row(
            device: device("aabbccddee"), status: .online, cached: nil, now: now
        )
        XCTAssertEqual(row.title, "aabbccdd")
    }

    // MARK: - 오프라인 표시 규칙

    /// 오프라인이면 tok/s 는 감추고 한도 %는 남긴다.
    func testOfflineHidesRateButKeepsQuota() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"),
            status: .offline,
            cached: try cached(agentJSON: #"{"k":0,"r":12.5,"t5":100,"p5":40,"pw":80,"pj":[]}"#, minutesAgo: 12),
            now: now
        )

        XCTAssertNil(row.agents[0].rateText)
        XCTAssertEqual(row.agents[0].fiveHour?.percentText, "40%")
        XCTAssertEqual(row.agents[0].weekly?.percentText, "80%")
        XCTAssertEqual(row.freshnessText, "12분 전")
    }

    func testOnlineShowsRate() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"),
            status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":12.5,"t5":100,"p5":40,"pj":[]}"#),
            now: now
        )
        XCTAssertEqual(row.agents[0].rateText, "13 tok/s")
    }

    /// quotaError 가 있으면 한도 %를 감춘다 — 맥은 실패 중에도 마지막 %를 보내지만
    /// 그 숫자는 현재 상태를 말해주지 않는다(MirrorSnapshot 문서).
    func testQuotaErrorHidesPercent() throws {
        let row = DeviceListPresentation.row(
            device: device("aa"),
            status: .online,
            cached: try cached(agentJSON: #"{"k":0,"r":1,"t5":0,"p5":44,"e":1,"pj":[]}"#),
            now: now
        )
        XCTAssertNil(row.agents[0].fiveHour)
    }

    // MARK: - 에이전트 개수 제한

    func testShowsAtMostTwoAgentsAndCountsTheRest() throws {
        let agents = #"{"k":0,"r":1,"t5":0,"pj":[]},{"k":1,"r":2,"t5":0,"pj":[]},{"k":2,"r":3,"t5":0,"pj":[]}"#
        let row = DeviceListPresentation.row(
            device: device("aa"), status: .online, cached: try cached(agentJSON: agents), now: now
        )

        XCTAssertEqual(row.agents.count, 2)
        XCTAssertEqual(row.hiddenAgentCount, 1)
    }

    // MARK: - 정렬

    func testSortsOnlineThenUnstableThenOffline() {
        let sorted = DeviceListPresentation.sorted([
            (device("cc"), .offline),
            (device("aa"), .online),
            (device("bb"), .unstable(failureCount: 1)),
        ])
        XCTAssertEqual(sorted.map(\.endpointIdHex), ["aa", "bb", "cc"])
    }

    func testSortIndexBreaksTiesWithinAGroup() {
        var first = device("aa"); first.sortIndex = 5
        var second = device("bb"); second.sortIndex = 1

        let sorted = DeviceListPresentation.sorted([(first, .online), (second, .online)])

        XCTAssertEqual(sorted.map(\.endpointIdHex), ["bb", "aa"])
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'DeviceListPresentation' in scope`.

- [ ] **Step 3: 구현**

`ios/Sources/Fleet/DeviceListPresentation.swift`:

```swift
import Foundation
import MirrorFormat
import Wire

public struct QuotaWindowText: Equatable, Sendable {
    public let percentText: String
    public let percent: Float
}

public struct AgentRowModel: Equatable, Sendable {
    public let name: String
    /// 오프라인이면 nil — tok/s 는 지금 이 순간의 값이라 낡으면 의미가 없다.
    public let rateText: String?
    public let fiveHour: QuotaWindowText?
    public let weekly: QuotaWindowText?
}

public struct DeviceRowModel: Equatable, Sendable {
    public let id: String
    public let title: String
    public let statusText: String
    /// 오프라인일 때만 채운다("12분 전").
    public let freshnessText: String?
    public let agents: [AgentRowModel]
    public let hiddenAgentCount: Int
}

public enum DeviceListPresentation {
    public static let visibleAgentLimit = 2

    public static func row(
        device: Device, status: DeviceStatus, cached: CachedSnapshot?, now: Date
    ) -> DeviceRowModel {
        let isLive = (status == .online)
        let ordered = orderedForDisplay(cached?.snapshot.agents ?? [])
        let shown = ordered.prefix(visibleAgentLimit).map { agent in
            AgentRowModel(
                name: agentName(agent.kind),
                rateText: isLive ? "\(MirrorFormat.tokensPerSec(agent.ratePerSec)) tok/s" : nil,
                fiveHour: window(percent: agent.usedPct5h, hasError: agent.quotaError != nil),
                weekly: window(percent: agent.usedPctWeekly, hasError: agent.quotaError != nil)
            )
        }

        return DeviceRowModel(
            id: device.endpointIdHex,
            title: device.displayName,
            statusText: statusText(status),
            freshnessText: isLive ? nil : freshness(cached?.fetchedAt, now: now),
            agents: Array(shown),
            hiddenAgentCount: max(0, ordered.count - visibleAgentLimit)
        )
    }

    /// `온라인 → 불안정 → 오프라인` 그룹, 그룹 안에서는 sortIndex 순.
    public static func sorted(_ rows: [(Device, DeviceStatus)]) -> [Device] {
        rows.sorted { lhs, rhs in
            let l = groupRank(lhs.1), r = groupRank(rhs.1)
            if l != r { return l < r }
            return lhs.0.sortIndex < rhs.0.sortIndex
        }.map(\.0)
    }

    private static func groupRank(_ status: DeviceStatus) -> Int {
        switch status {
        case .online: return 0
        case .probing, .unstable: return 1
        case .idle, .offline: return 2
        case .needsRepairing, .versionMismatch: return 3
        }
    }

    private static func window(percent: Float?, hasError: Bool) -> QuotaWindowText? {
        guard !hasError, let percent else { return nil }
        let clamped = min(100, percent)
        return QuotaWindowText(
            percentText: MirrorFormat.toFixed(Double(clamped), 0) + "%",
            percent: clamped
        )
    }

    private static func statusText(_ status: DeviceStatus) -> String {
        switch status {
        case .idle: return "대기"
        case .probing: return "확인 중"
        case .online: return "연결됨"
        case .unstable: return "재연결 중"
        case .offline: return "오프라인"
        case .needsRepairing: return "재페어링 필요"
        case .versionMismatch: return "버전 불일치"
        }
    }

    private static func freshness(_ fetchedAt: Date?, now: Date) -> String? {
        guard let fetchedAt else { return nil }
        let minutes = max(0, Int(now.timeIntervalSince(fetchedAt) / 60))
        return minutes < 1 ? "방금 전" : "\(minutes)분 전"
    }

    private static func agentName(_ kind: AgentKindCode) -> String {
        switch kind {
        case .claude: return "Claude Code"
        case .codex: return "Codex"
        case .antigravity: return "Antigravity"
        case .unknown: return "Agent"
        }
    }

    /// 기존 앱 `MirrorViewController.orderedForDisplay` 와 같은 순서.
    private static func orderedForDisplay(_ agents: [MirrorAgent]) -> [MirrorAgent] {
        let claude = agents.filter { $0.kind == .claude }
        let codex = agents.filter { $0.kind == .codex }
        let others = agents.filter { $0.kind != .claude && $0.kind != .codex }
        return claude + codex + others
    }
}
```

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 9개 신규 포함 전부 통과.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/Fleet/DeviceListPresentation.swift ios/Tests/FleetTests/DeviceListPresentationTests.swift
git commit -m "feat: 목록 표시 규칙 (오프라인은 tok/s 감추고 한도 유지, 에이전트 2개 + 나머지 수)"
```

---

### Task 10: DeviceSession — 장치 하나의 수명주기

**Files:**
- Create: `ios/Sources/Fleet/DeviceTransport.swift`, `ios/Sources/Fleet/DeviceSession.swift`
- Test: `ios/Tests/FleetTests/DeviceSessionTests.swift`

**Interfaces:**
- Consumes: `DeviceStatusMachine`(Task 8), `Device`(Task 5)
- Produces: `public protocol DeviceTransport: Sendable { func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> }`
- Produces: `public enum DeviceTransportError: Error, Equatable { case unreachable, authRejected, versionMismatch }`
- Produces: `@MainActor public final class DeviceSession { public init(device: Device, transport: DeviceTransport); public private(set) var status: DeviceStatus; public var onChange: ((DeviceSession) -> Void)?; public private(set) var latest: MirrorSnapshot?; public func probeNow() async; public func retrigger() async; public func stop() }`

실제 iroh 연결은 단위 테스트가 불가능하므로 전송 계층을 프로토콜로 주입받는다.

**세대(generation) 카운터가 이 태스크의 핵심이다.** 오프라인 전환·정지 시점에 늦게 도착한 결과가 상태를 덮어쓰면 안 된다 — 2026-09-17 TunnelKit 리뷰에서 같은 클래스의 버그 3건이 하루에 발견됐다.

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/FleetTests/DeviceSessionTests.swift` 생성:

```swift
import XCTest
import Wire
@testable import Fleet

private func makeSnapshot(rate: Float) throws -> MirrorSnapshot {
    let json = #"{"v":1,"t":1758000000,"a":[{"k":0,"r":\#(rate),"t5":0,"pj":[]}]}"#
    return try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
}

/// 결과를 시험이 직접 정하는 가짜 전송.
private final class FakeTransport: DeviceTransport, @unchecked Sendable {
    enum Outcome {
        case stream([MirrorSnapshot])
        case fail(DeviceTransportError)
        case hang
    }
    var outcomes: [Outcome] = []
    private(set) var probeCount = 0

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        probeCount += 1
        let outcome = outcomes.isEmpty ? Outcome.fail(.unreachable) : outcomes.removeFirst()
        switch outcome {
        case .fail(let error):
            throw error
        case .hang:
            try await Task.sleep(nanoseconds: 10_000_000_000)
            throw DeviceTransportError.unreachable
        case .stream(let snapshots):
            return AsyncThrowingStream { continuation in
                for s in snapshots { continuation.yield(s) }
                continuation.finish()
            }
        }
    }
}

@MainActor
final class DeviceSessionTests: XCTestCase {

    private func makeDevice() -> Device {
        Device(endpointIdHex: "aa", token: "t", relayUrl: nil, addresses: [],
               macHostname: nil, userLabel: nil, sortIndex: 0)
    }

    func testSuccessfulProbeGoesOnlineAndPublishesSnapshot() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.stream([try makeSnapshot(rate: 3)])]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()

        XCTAssertEqual(session.status, .online)
        XCTAssertEqual(session.latest?.agents.first?.ratePerSec, 3)
    }

    func testThreeFailuresGoOffline() async {
        let transport = FakeTransport()
        transport.outcomes = [.fail(.unreachable), .fail(.unreachable), .fail(.unreachable)]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()
        await session.probeNow()
        await session.probeNow()

        XCTAssertEqual(session.status, .offline)
    }

    /// 인증 거부는 재시도하지 않는다 — 트리거가 와도 probe 를 다시 부르면 안 된다.
    func testAuthRejectionStopsRetrying() async {
        let transport = FakeTransport()
        transport.outcomes = [.fail(.authRejected)]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()
        XCTAssertEqual(session.status, .needsRepairing)

        await session.retrigger()
        XCTAssertEqual(session.status, .needsRepairing)
        XCTAssertEqual(transport.probeCount, 1, "종단 상태인데 probe 를 다시 불렀다")
    }

    func testVersionMismatchIsTerminal() async {
        let transport = FakeTransport()
        transport.outcomes = [.fail(.versionMismatch)]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        await session.probeNow()

        XCTAssertEqual(session.status, .versionMismatch)
    }

    /// stop() 이후 늦게 도착한 결과가 상태를 오염시키면 안 된다 —
    /// 2026-09-17 TunnelKit 리뷰의 버그 클래스.
    func testResultArrivingAfterStopIsIgnored() async throws {
        let transport = FakeTransport()
        transport.outcomes = [.stream([try makeSnapshot(rate: 9)])]
        let session = DeviceSession(device: makeDevice(), transport: transport)

        session.stop()
        await session.probeNow()

        XCTAssertEqual(session.status, .idle)
        XCTAssertNil(session.latest)
    }

    func testOfflineDeviceProbesAgainOnRetrigger() async throws {
        let transport = FakeTransport()
        transport.outcomes = [
            .fail(.unreachable), .fail(.unreachable), .fail(.unreachable),
            .stream([try makeSnapshot(rate: 1)]),
        ]
        let session = DeviceSession(device: makeDevice(), transport: transport)
        for _ in 0..<3 { await session.probeNow() }
        XCTAssertEqual(session.status, .offline)

        await session.retrigger()

        XCTAssertEqual(session.status, .online)
        XCTAssertEqual(transport.probeCount, 4)
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'DeviceSession' in scope`.

- [ ] **Step 3: 구현**

`ios/Sources/Fleet/DeviceTransport.swift`:

```swift
import Foundation
import Wire

public enum DeviceTransportError: Error, Equatable {
    /// 도달 불가(타임아웃·연결 실패). 재시도로 풀릴 수 있다.
    case unreachable
    /// 토큰이 거부됐다. 재시도로는 절대 풀리지 않는다.
    case authRejected
    case versionMismatch
}

/// 장치 하나에 붙어 스냅샷을 흘려보내는 전송 계층.
///
/// probe 성공 시 **그 연결을 그대로 유지한 채** 스트림을 돌려준다. 다시 dial 하면
/// 가장 비싼 부분(hole-punch·QUIC·인증 왕복)을 두 번 낸다.
public protocol DeviceTransport: Sendable {
    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error>
}
```

`ios/Sources/Fleet/DeviceSession.swift`:

```swift
import Foundation
import Wire

/// 장치 한 대의 연결 수명주기. 상태 전이는 `DeviceStatusMachine` 이 정하고,
/// 이 타입은 전송 호출과 세대 관리만 한다.
@MainActor
public final class DeviceSession {
    public static let probeTimeoutSeconds: Double = 3

    public let device: Device
    public private(set) var status: DeviceStatus = .idle
    public private(set) var latest: MirrorSnapshot?
    /// `latest` 가 도착한 시각. 없으면 목록이 오프라인 장치의 신선도를 계산할 수 없어
    /// 항상 "방금 전"으로 보인다.
    public private(set) var latestAt: Date?
    public var onChange: ((DeviceSession) -> Void)?

    private let transport: DeviceTransport
    private var runTask: Task<Void, Never>?
    /// 취소/정리 시점 이후에 도착한 결과가 상태를 덮어쓰는 걸 막는다.
    /// 완료 지점마다 이 값을 검사한다.
    private var generation = 0
    /// `stop()` 이후 세션은 재사용하지 않는다. 세대 카운터는 **비동기 경합**을 막는
    /// 장치라, `stop()` 바로 뒤에 `probeNow()` 를 부르는 **순차** 호출은 못 막는다
    /// (2026-09-18 확인). 정지된 장치가 다시 필요하면 새 DeviceSession 을 만든다.
    private var isStopped = false

    public init(device: Device, transport: DeviceTransport) {
        self.device = device
        self.transport = transport
    }

    /// 지금 한 번 붙어본다. 종단 상태(재페어링 필요·버전 불일치)면 아무것도 하지 않는다.
    public func probeNow() async {
        guard !isTerminal else { return }

        generation += 1
        let current = generation
        apply(.probeStarted, generation: current)

        do {
            let stream = try await transport.probe(
                device: device, timeoutSeconds: Self.probeTimeoutSeconds
            )
            guard current == generation else { return }
            apply(.probeSucceeded, generation: current)
            await consume(stream, generation: current)
        } catch let error as DeviceTransportError {
            guard current == generation else { return }
            switch error {
            case .authRejected: apply(.authRejected, generation: current)
            case .versionMismatch: apply(.versionRejected, generation: current)
            case .unreachable: apply(.probeFailed, generation: current)
            }
        } catch {
            guard current == generation else { return }
            apply(.probeFailed, generation: current)
        }
    }

    /// 타이머(3분)/포어그라운드 복귀/당겨서 새로고침이 부른다.
    public func retrigger() async {
        guard !isTerminal else { return }
        let nextStatus = DeviceStatusMachine.next(status, on: .retriggered)
        // 오프라인이 아니면 재탐색 대상이 아니다(이미 붙어 있거나 붙는 중).
        guard nextStatus == .probing || status == .idle else { return }
        await probeNow()
    }

    public func stop() {
        generation += 1
        runTask?.cancel()
        runTask = nil
        status = .idle
        latest = nil
        onChange?(self)
    }

    private var isTerminal: Bool {
        status == .needsRepairing || status == .versionMismatch
    }

    private func consume(_ stream: AsyncThrowingStream<MirrorSnapshot, Error>, generation current: Int) async {
        do {
            for try await snapshot in stream {
                guard current == generation else { return }
                latest = snapshot
                latestAt = Date()
                apply(.streamEstablished, generation: current)
            }
            // 스트림이 에러 없이 끝난 건 연결 단절이 아니다 — 실제 전송
            // (`NetworkClient.listenForSnapshots`)은 `while wantsRunning` 루프라
            // 정상 종료가 "중지 요청"이나 "버전 불일치"를 뜻하고, 진짜 끊김은
            // `recv.read` 가 throw 해서 아래 catch 로 온다. 여기서 connectionLost 를
            // 적용하면 성공 케이스가 곧바로 unstable 로 떨어진다(2026-09-18 확인).
            //
            // ⚠️ 그래서 Task 12 의 `snapshotStream` 은 연결 끊김과 **버전 불일치를
            // 반드시 에러로 던져야** 한다. 정상 finish 로 넘기면 장치가 영원히
            // online 에 고착돼 멈춘 데이터를 보여준다.
        } catch {
            guard current == generation else { return }
            apply(.connectionLost, generation: current)
        }
    }

    private func apply(_ event: DeviceEvent, generation current: Int) {
        guard current == generation else { return }
        status = DeviceStatusMachine.next(status, on: event)
        onChange?(self)
    }
}
```

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 6개 신규 포함 전부 통과.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/Fleet/DeviceTransport.swift ios/Sources/Fleet/DeviceSession.swift ios/Tests/FleetTests/DeviceSessionTests.swift
git commit -m "feat: DeviceSession — 세대 카운터로 늦은 결과 차단, 전송은 프로토콜 주입"
```

---

### Task 11: DeviceFleet — probe 라운드와 트리거

**Files:**
- Create: `ios/Sources/Fleet/DeviceFleet.swift`
- Test: `ios/Tests/FleetTests/DeviceFleetTests.swift`

**Interfaces:**
- Consumes: `DeviceSession`(Task 10), `DeviceRegistry`(Task 5), `DeviceSnapshotCache`(Task 7)
- Produces: `@MainActor public final class DeviceFleet { public static let probeConcurrency = 4; public static let reprobeInterval: TimeInterval = 180; public init(devices: [Device], transportFactory: @escaping (Device) -> DeviceTransport, cache: DeviceSnapshotCache?); public private(set) var sessions: [DeviceSession]; public var onChange: (() -> Void)?; public func startInitialRound() async; public func retriggerOffline() async }`

- [ ] **Step 1: 실패하는 테스트 작성**

`ios/Tests/FleetTests/DeviceFleetTests.swift` 생성:

```swift
import XCTest
import Wire
@testable import Fleet

/// 동시에 몇 개가 실행 중이었는지 관찰하는 가짜 전송.
private final class CountingTransport: DeviceTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var active = 0
    private(set) var maxConcurrent = 0

    func probe(device: Device, timeoutSeconds: Double) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        lock.lock(); active += 1; maxConcurrent = max(maxConcurrent, active); lock.unlock()
        try await Task.sleep(nanoseconds: 50_000_000)
        lock.lock(); active -= 1; lock.unlock()
        throw DeviceTransportError.unreachable
    }
}

@MainActor
final class DeviceFleetTests: XCTestCase {

    private func devices(_ count: Int) -> [Device] {
        (0..<count).map {
            Device(endpointIdHex: String(format: "%02x", $0), token: "t", relayUrl: nil,
                   addresses: [], macHostname: nil, userLabel: nil, sortIndex: $0)
        }
    }

    func testCreatesOneSessionPerDevice() {
        let fleet = DeviceFleet(
            devices: devices(5),
            transportFactory: { _ in CountingTransport() },
            cache: nil
        )
        XCTAssertEqual(fleet.sessions.count, 5)
    }

    /// 순차 probe 는 꺼진 Mac 의 타임아웃이 직렬로 쌓여 목록이 100초 넘게 안 잡힌다.
    /// 그렇다고 16개를 한꺼번에 던지면 릴레이에 몰린다.
    func testInitialRoundRespectsConcurrencyLimit() async {
        let transport = CountingTransport()
        let fleet = DeviceFleet(
            devices: devices(16),
            transportFactory: { _ in transport },
            cache: nil
        )

        await fleet.startInitialRound()

        XCTAssertLessThanOrEqual(transport.maxConcurrent, DeviceFleet.probeConcurrency)
        XCTAssertGreaterThan(transport.maxConcurrent, 1, "직렬로 돌았다")
    }

    func testAllSessionsEndUpOfflineWhenNothingIsReachable() async {
        let fleet = DeviceFleet(
            devices: devices(4),
            transportFactory: { _ in CountingTransport() },
            cache: nil
        )

        // 임계값(3회)만큼 라운드를 돌린다.
        for _ in 0..<DeviceStatusMachine.failureThreshold {
            await fleet.startInitialRound()
        }

        XCTAssertTrue(fleet.sessions.allSatisfy { $0.status == .offline })
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'DeviceFleet' in scope`.

- [ ] **Step 3: 구현**

`ios/Sources/Fleet/DeviceFleet.swift`:

```swift
import Foundation
import Wire

/// 장치 세션 전체를 소유하고 probe 라운드를 관리한다.
@MainActor
public final class DeviceFleet {
    /// 순차는 꺼진 Mac 의 타임아웃이 직렬로 쌓여 목록이 자리잡는 데 최악 100초가 넘고,
    /// 16개를 한꺼번에 던지면 릴레이에 몰린다.
    public static let probeConcurrency = 4
    /// 포어그라운드 상태에서 오프라인 장치를 다시 확인하는 주기.
    public static let reprobeInterval: TimeInterval = 180

    public private(set) var sessions: [DeviceSession] = []
    public var onChange: (() -> Void)?

    private let cache: DeviceSnapshotCache?

    public init(
        devices: [Device],
        transportFactory: @escaping (Device) -> DeviceTransport,
        cache: DeviceSnapshotCache?
    ) {
        self.cache = cache
        self.sessions = devices.map { device in
            let session = DeviceSession(device: device, transport: transportFactory(device))
            return session
        }
        for session in sessions {
            session.onChange = { [weak self] changed in
                self?.handleChange(changed)
            }
        }
    }

    /// 앱 시작·포어그라운드 복귀 시. 모든 세션을 동시성 제한 안에서 붙여본다.
    /// 포어그라운드 복귀 때 대상이 "오프라인이던 장치"가 아니라 전부인 이유는,
    /// iOS 가 앱을 suspend 하면 QUIC 연결이 전부 조용히 끊기기 때문이다.
    public func startInitialRound() async {
        await runRound(sessions)
    }

    /// 타이머(3분)/당겨서 새로고침. 오프라인인 것만 다시 본다.
    public func retriggerOffline() async {
        let targets = sessions.filter { $0.status == .offline || $0.status == .idle }
        await withTaskGroup(of: Void.self) { group in
            var iterator = targets.makeIterator()
            for _ in 0..<Self.probeConcurrency {
                guard let session = iterator.next() else { break }
                group.addTask { await session.retrigger() }
            }
            while await group.next() != nil {
                guard let session = iterator.next() else { continue }
                group.addTask { await session.retrigger() }
            }
        }
    }

    private func runRound(_ targets: [DeviceSession]) async {
        await withTaskGroup(of: Void.self) { group in
            var iterator = targets.makeIterator()
            // 처음 probeConcurrency 개를 띄우고, 하나 끝날 때마다 다음 것을 넣는다.
            for _ in 0..<Self.probeConcurrency {
                guard let session = iterator.next() else { break }
                group.addTask { await session.probeNow() }
            }
            while await group.next() != nil {
                guard let session = iterator.next() else { continue }
                group.addTask { await session.probeNow() }
            }
        }
    }

    private func handleChange(_ session: DeviceSession) {
        if let snapshot = session.latest, session.status == .online {
            cache?.save(snapshot, fetchedAt: Date(), endpointIdHex: session.device.endpointIdHex)
        }
        onChange?()
    }
}
```

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 3개 신규 포함 전부 통과.

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/Fleet/DeviceFleet.swift ios/Tests/FleetTests/DeviceFleetTests.swift
git commit -m "feat: DeviceFleet — 동시성 4 probe 라운드와 재탐색 트리거"
```

---

### Task 12: iroh 전송 구현체

**Files:**
- Create: `ios/Sources/Fleet/IrohDeviceTransport.swift`
- Test: `ios/Tests/FleetTests/IrohDeviceTransportTests.swift`

**Interfaces:**
- Consumes: `DeviceTransport`(Task 10), `NetworkClient.probe(...)`(Task 4), `IrohEndpointProvider`(Task 3)
- Produces: `public struct IrohDeviceTransport: DeviceTransport { public init(provider: IrohEndpointProvider) }`

- [ ] **Step 1: 실패하는 테스트 작성**

실제 연결은 단위 테스트가 불가능하다. 검증할 수 있는 건 **에러 매핑**이다 — 이게 틀리면 인증 거부가 단순 실패로 취급돼 무한 재시도에 빠진다.

`ios/Tests/FleetTests/IrohDeviceTransportTests.swift` 생성:

```swift
import XCTest
import NetworkTransport
@testable import Fleet

final class IrohDeviceTransportTests: XCTestCase {

    /// 인증 거부가 .unreachable 로 뭉개지면 재시도로 풀리지 않는 상태를 계속 재시도하게 된다
    /// (NetworkClient.swift:213-222 의 교훈).
    func testAuthFailureMapsToAuthRejected() {
        XCTAssertEqual(IrohDeviceTransport.mapError(NetworkClientError.authFailed), .authRejected)
        XCTAssertEqual(IrohDeviceTransport.mapError(NetworkClientError.needsPairing), .authRejected)
    }

    func testVersionMismatchMapsToVersionMismatch() {
        XCTAssertEqual(IrohDeviceTransport.mapError(NetworkClientError.versionMismatch), .versionMismatch)
    }

    func testTimeoutMapsToUnreachable() {
        XCTAssertEqual(IrohDeviceTransport.mapError(NetworkClientError.fetchTimedOut), .unreachable)
    }

    func testUnknownErrorMapsToUnreachable() {
        struct Boom: Error {}
        XCTAssertEqual(IrohDeviceTransport.mapError(Boom()), .unreachable)
    }
}
```

- [ ] **Step 2: 테스트 실행해서 실패 확인**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | tail -20
```

기대: `cannot find 'IrohDeviceTransport' in scope`.

- [ ] **Step 3: 구현**

`NetworkClientError` 가 `internal` 이면 `public` 으로 올린다(`NetworkClient.swift:403`). 그리고 `ios/Sources/Fleet/IrohDeviceTransport.swift`:

```swift
import Foundation
import NetworkTransport
import Wire

/// 실제 iroh 전송. probe 로 연결을 열고 **그 연결을 그대로** 스냅샷 스트림으로 잇는다.
public struct IrohDeviceTransport: DeviceTransport {
    private let provider: IrohEndpointProvider

    public init(provider: IrohEndpointProvider = .shared) {
        self.provider = provider
    }

    public func probe(
        device: Device, timeoutSeconds: Double
    ) async throws -> AsyncThrowingStream<MirrorSnapshot, Error> {
        let client = await NetworkClient(endpointProvider: provider)
        do {
            let result = try await client.probe(
                endpointIdHex: device.endpointIdHex,
                relayUrl: device.relayUrl,
                addresses: device.addresses,
                timeoutSeconds: timeoutSeconds,
                // 장치별 토큰. 전역 NetworkTokenStore 슬롯은 1:1 앱 소유라 읽거나 지우면 안 된다(Ruling 14).
                token: device.token
            )
            // 스트림 본문 에러도 DeviceTransportError 로 바꿔 던져야 DeviceSession 이 종단 상태를 알아본다(Ruling 13).
            return Self.mappingErrors(await client.snapshotStream(from: result))
        } catch {
            throw Self.mapError(error)
        }
    }

    /// 재시도로 풀리는 실패와 그렇지 않은 실패를 가른다. 이 매핑이 틀리면
    /// 인증 거부를 무한 재시도하게 된다.
    static func mapError(_ error: Error) -> DeviceTransportError {
        switch error {
        case NetworkClientError.authFailed, NetworkClientError.needsPairing:
            return .authRejected
        case NetworkClientError.versionMismatch:
            return .versionMismatch
        default:
            return .unreachable
        }
    }
}
```

`NetworkClient` 에 스트림 어댑터를 추가한다(`NetworkClient.swift`):

```swift
/// probe 로 연 연결을 그대로 스냅샷 스트림으로 잇는다. 첫 스냅샷을 먼저 흘리고,
/// 이후 `listenForSnapshots` 와 같은 방식으로 계속 읽는다.
public func snapshotStream(from result: ProbeResult) -> AsyncThrowingStream<MirrorSnapshot, Error> {
    AsyncThrowingStream { continuation in
        // self 를 강하게 잡는다 — probe 호출부는 이 클라이언트를 지역변수로만 들고 있어
        // weak 면 메인 액터가 양보하는 순간 해제돼 빈 스트림으로 끝난다(Ruling 12).
        let task = Task {
            continuation.yield(result.firstSnapshot)
            do {
                try await self.streamSnapshots(
                    conn: result.connection, channel: result.channel
                ) { snapshot in
                    continuation.yield(snapshot)
                }
                continuation.finish()
            } catch {
                continuation.finish(throwing: error)
            }
        }
        continuation.onTermination = { _ in
            task.cancel()
            Task { await result.connection.close() }
        }
    }
}
```

> 구현 주의: `streamSnapshots(conn:channel:onSnapshot:)` 는 기존 `listenForSnapshots(conn:channel:)`(`NetworkClient.swift:355`)에서 **퍼블리셔로 보내던 부분만** 콜백으로 바꾼 것이다. 읽기 루프·복호화·버전 검사 로직을 새로 쓰지 말고 그대로 옮긴 뒤, 기존 `listenForSnapshots` 는 이 함수를 호출해 `snapshotSubject.send` 하도록 남긴다(기존 앱 동작 유지).

- [ ] **Step 4: 테스트 재실행**

```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 4개 신규 포함 전부 통과.

- [ ] **Step 5: 기존 앱 회귀 확인**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD" && \
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFeatureTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: `BUILD SUCCEEDED`, 47개 통과.

- [ ] **Step 6: 커밋**

```bash
git add ios/Sources/Fleet/IrohDeviceTransport.swift ios/Sources/NetworkTransport/NetworkClient.swift ios/Tests/FleetTests/IrohDeviceTransportTests.swift
git commit -m "feat: iroh 전송 구현체와 스냅샷 스트림 어댑터"
```

---

### Task 13: AppMulti 타겟과 앱 셸

**Files:**
- Modify: `ios/Project.swift` (`AppMulti` 타겟 추가)
- Create: `ios/Sources/AppMulti/AppDelegate.swift`, `ios/Sources/AppMulti/SceneDelegate.swift`, `ios/Sources/AppMulti/AppEnvironment.swift`

**Interfaces:**
- Produces: `AppEnvironment` — 레지스트리·캐시·fleet 을 조립하는 단일 지점

- [ ] **Step 1: Tuist 타겟 추가**

`ios/Project.swift` 의 `targets:` 배열 끝(AIMonitorWidget 다음)에 추가:

```swift
        .target(
            name: "AppMulti",
            destinations: .iOS,
            product: .app,
            bundleId: "co.kr.wannypark.aiagentmonitor.multi",
            deploymentTargets: iOS,
            infoPlist: .extendingDefault(with: [
                "UILaunchScreen": [:],
                "CFBundleDisplayName": "AI Monitor Multi",
                "NSCameraUsageDescription":
                    "Mac 화면에 뜬 페어링 QR 코드를 스캔해 장치를 추가합니다.",
                // 없으면 iOS 가 로컬 네트워크(사설 IP) 소켓 연결마다 물어보는 권한
                // 팝업이 제대로 안 뜬다 — 기존 App 과 같은 이유다.
                "NSLocalNetworkUsageDescription":
                    "Mac과 같은 네트워크에서 QUIC(iroh)로 직접 연결하기 위해 필요합니다.",
                "ITSAppUsesNonExemptEncryption": false,
                "CFBundleShortVersionString": "$(MARKETING_VERSION)",
                "CFBundleVersion": "$(CURRENT_PROJECT_VERSION)",
                "UIApplicationSceneManifest": [
                    "UIApplicationSupportsMultipleScenes": false,
                    "UISceneConfigurations": [
                        "UIWindowSceneSessionRoleApplication": [[
                            "UISceneConfigurationName": "Default",
                            "UISceneDelegateClassName": "$(PRODUCT_MODULE_NAME).SceneDelegate",
                        ]]
                    ],
                ],
            ]),
            sources: ["Sources/AppMulti/**"],
            dependencies: [
                .target(name: "Fleet"),
                .target(name: "NetworkTransport"),
                .target(name: "DesignSystem"),
                .target(name: "MirrorFormat"),
                .target(name: "Wire"),
                .external(name: "SnapKit"),
            ],
            settings: .settings(base: [
                "DEVELOPMENT_TEAM": "LC8PY3D283",
                "CODE_SIGN_STYLE": "Automatic",
                "MARKETING_VERSION": marketingVersion,
                "CURRENT_PROJECT_VERSION": currentProjectVersion,
            ])
        ),
```

**엔타이틀먼트를 주지 않는다** — App Group·Keychain Sharing 이 필요 없다.

- [ ] **Step 2: 앱 셸 작성**

`ios/Sources/AppMulti/AppDelegate.swift`:

```swift
import UIKit

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        configurationForConnecting connectingSceneSession: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        UISceneConfiguration(name: "Default", sessionRole: connectingSceneSession.role)
    }
}
```

`ios/Sources/AppMulti/AppEnvironment.swift`:

```swift
import Fleet
import Foundation
import NetworkTransport

/// 앱 전역 조립 지점. 레지스트리·캐시·fleet 을 여기서만 만든다.
@MainActor
final class AppEnvironment {
    let registry = DeviceRegistry(store: KeychainRegistryStore())
    let cache = DeviceSnapshotCache(directory: DeviceSnapshotCache.defaultDirectory())

    /// 레지스트리가 손상된 경우. 빈 목록으로 조용히 시작하지 않고 화면에 알린다.
    private(set) var registryError: Error?

    func makeFleet() -> DeviceFleet {
        let devices: [Device]
        do {
            devices = try registry.load()
        } catch {
            registryError = error
            devices = []
        }
        return DeviceFleet(
            devices: devices,
            transportFactory: { _ in IrohDeviceTransport() },
            cache: cache
        )
    }
}
```

`ios/Sources/AppMulti/SceneDelegate.swift`:

```swift
import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private let environment = AppEnvironment()
    private var listViewController: DeviceListViewController?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }
        let list = DeviceListViewController(environment: environment)
        listViewController = list

        let window = UIWindow(windowScene: windowScene)
        window.rootViewController = UINavigationController(rootViewController: list)
        window.makeKeyAndVisible()
        self.window = window
    }

    /// iOS 가 앱을 suspend 하면 QUIC 연결이 전부 조용히 끊긴다 — 복귀 시 대상은
    /// "오프라인이던 장치"가 아니라 전부다.
    func sceneWillEnterForeground(_ scene: UIScene) {
        listViewController?.reconnectAll()
    }
}
```

> Task 14 에서 `DeviceListViewController` 를 만들기 전까지는 컴파일되지 않는다. 이 태스크의 검증은 Task 14 와 함께 한다 — 아래 Step 3 에서 **자리표시 목록 화면**을 최소 구현으로 같이 넣는다.

- [ ] **Step 3: 최소 목록 화면(자리표시)**

`ios/Sources/AppMulti/DeviceListViewController.swift`:

```swift
import Fleet
import UIKit

/// Task 14 에서 컬렉션 뷰로 채운다. 여기서는 타겟이 빌드·실행되는 것만 확인한다.
final class DeviceListViewController: UIViewController {
    private let environment: AppEnvironment
    private var fleet: DeviceFleet?

    init(environment: AppEnvironment) {
        self.environment = environment
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "장치"
        view.backgroundColor = .systemBackground
        fleet = environment.makeFleet()
    }

    func reconnectAll() {
        guard let fleet else { return }
        Task { await fleet.startInitialRound() }
    }
}
```

- [ ] **Step 4: 재생성 + 빌드 확인**

```bash
cd ios && mise exec -- tuist generate --no-open && \
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme AppMulti \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD"
```

기대: `BUILD SUCCEEDED`.

- [ ] **Step 5: 기존 앱 회귀 확인**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD"
```

기대: `BUILD SUCCEEDED`.

- [ ] **Step 6: 커밋**

```bash
git add ios/Project.swift ios/Sources/AppMulti
git commit -m "feat: AppMulti 앱 타겟과 앱 셸 추가"
```

---

### Task 14: 장치 목록 화면

**Files:**
- Modify: `ios/Sources/AppMulti/DeviceListViewController.swift` (Task 13 의 자리표시를 교체)
- Create: `ios/Sources/AppMulti/DeviceCell.swift`
- Modify: `ios/Sources/Fleet/DeviceSession.swift` (진행 중 probe Task 보관·`stop()` 에서 취소 — Ruling 17)
- Modify: `ios/Sources/Fleet/DeviceFleet.swift` (`stopAll()` 추가 — Ruling 17)
- Test: `ios/Tests/FleetTests/DeviceSessionTests.swift`, `ios/Tests/FleetTests/DeviceFleetTests.swift`

**Interfaces:**
- Consumes: `DeviceListPresentation.row(...)`, `DeviceRowModel`, `DeviceFleet`
- Produces: 셀 탭 → Task 16 의 상세 화면 진입점

- [ ] **Step 0: Fleet 정지 API (Ruling 17 — 먼저 한다)**

이 화면은 레지스트리가 바뀔 때마다 `startFleet()` 으로 fleet 을 **다시 만든다**. 옛 fleet 의 세션들은
`while true` 스트림을 계속 소비하므로, 멈추지 않으면 삭제된 장치의 QUIC 연결이 누수되고 남은 장치는
이중 연결이 된다. 그런데 지금 `DeviceSession.stop()` 은 generation 만 올리고 진행 중인 작업을 취소하지
않아 — 스트림은 **다음 스냅샷이 도착해야** 가드에 걸려 닫힌다(Mac 이 조용하면 영원히 안 닫힘).

`ios/Sources/Fleet/DeviceSession.swift`:

```swift
    /// 진행 중인 probe/consume 작업. `stop()` 과 재-probe 가 취소해 스트림을 **실제로** 닫는다 —
    /// 취소는 `for try await` 에서 CancellationError 로 튀어나오고, 그 취소가 `mappingErrors` →
    /// `snapshotStream` 의 onTermination 까지 내려가 QUIC 연결을 닫는다. generation 가드는
    /// 늦은 결과를 **무시**만 할 수 있고 연결을 닫지는 못한다.
    private var probeTask: Task<Void, Never>?

    public func probeNow() async {
        guard !isStopped, !isTerminal else { return }
        // 포어그라운드 복귀처럼 online 인 세션에 다시 probe 가 걸리면, 죽었을 옛 스트림을 기다리지
        // 말고 바로 끊는다.
        probeTask?.cancel()

        generation += 1
        let current = generation
        apply(.probeStarted, generation: current)

        let task = Task { [weak self] in await self?.runProbe(generation: current) }
        probeTask = task
        await task.value
    }

    /// 기존 `probeNow()` 의 do/catch 본문을 그대로 옮긴다(transport.probe → consume → 에러 분기).
    /// CancellationError 는 범용 catch 로 떨어지고, 취소한 쪽이 generation 을 이미 올렸으므로
    /// `guard current == generation` 에서 조용히 빠져나간다.
    private func runProbe(generation current: Int) async { /* 기존 본문 */ }

    public func stop() {
        generation += 1
        isStopped = true
        probeTask?.cancel()
        probeTask = nil
        status = .idle
        latest = nil
        latestAt = nil
        onChange?(self)
    }
```

`ios/Sources/Fleet/DeviceFleet.swift`:

```swift
    /// 화면이 이 fleet 을 버릴 때(레지스트리 변경으로 다시 만들 때) 부른다. 모든 세션의 진행 중
    /// 작업을 취소해 연결을 닫는다 — 안 부르면 옛 fleet 의 스트림이 계속 살아 연결이 누수된다.
    public func stopAll() {
        for session in sessions { session.stop() }
    }
```

**테스트가 스펙이다.** `DeviceSessionTests` 의 `FakeTransport.Outcome` 에 케이스를 하나 추가한다:

```swift
        /// 스냅샷 하나를 흘린 뒤 **끝나지 않는** 스트림. 소비자가 끊으면 `onTerminate` 가 불린다 —
        /// `stop()` 이 스트림을 실제로 닫는지(취소 연쇄) 고정하는 픽스처.
        case openStream(MirrorSnapshot, onTerminate: @Sendable () -> Void)
```
구현: `AsyncThrowingStream { c in c.onTermination = { _ in onTerminate() }; c.yield(snapshot) }` (finish 하지 않는다).

```swift
    func testStopCancelsInFlightStreamAndClosesIt() async {
        let terminated = expectation(description: "스트림 종료 콜백")
        transport.outcomes = [.openStream(makeSnapshot(), onTerminate: { terminated.fulfill() })]
        let session = DeviceSession(device: makeDevice(), transport: transport)
        let probing = Task { await session.probeNow() }
        // online 에 도달할 때까지 기다린다(기존 테스트들이 쓰는 대기 방식을 따른다)
        ...
        XCTAssertEqual(session.status, .online)
        session.stop()
        await fulfillment(of: [terminated], timeout: 1)   // ← 핵심 단언
        await probing.value                                // probeNow 가 반환해야 한다(매달리면 실패)
        XCTAssertEqual(session.status, .idle)
    }
```
뮤테이션: `stop()` 의 `probeTask?.cancel()` 을 지우면 이 테스트는 **타임아웃으로 실패**해야 한다(스냅샷이
더 오지 않으니 가드에 걸릴 기회가 없다). 실행해서 확인하고 보고서에 적는다. 복원 후 다시 초록.

`DeviceFleetTests`:
```swift
    func testStopAllStopsEverySession() async {
        // CountingTransport 가 끝나지 않는 스트림을 돌려주게 한 뒤 startInitialRound → 전원 online
        ...
        fleet.stopAll()
        XCTAssertTrue(fleet.sessions.allSatisfy { $0.status == .idle })
    }
```
뮤테이션: `stopAll()` 본문을 비우면 실패해야 한다.

기존 `DeviceSessionTests` 55개·`DeviceFleetTests` 는 전부 그대로 통과해야 한다 — 특히
`testInFlightProbeResultArrivingAfterStopIsIgnored`, `testRetriggerIgnoresRequestWhileProbeIsInFlight`.

- [ ] **Step 1: 셀 구현**

`ios/Sources/AppMulti/DeviceCell.swift`:

```swift
import DesignSystem
import Fleet
import SnapKit
import UIKit

/// Mac 한 대. 에이전트는 최대 2개까지 보여주고 나머지는 "+N" 으로 접는다 —
/// 16대까지 쓰므로 가변 높이면 한 화면에 3~4대밖에 못 넣는다.
final class DeviceCell: UICollectionViewListCell {
    static let reuseIdentifier = "DeviceCell"

    private let titleLabel = UILabel()
    private let statusLabel = UILabel()
    private let freshnessLabel = UILabel()
    private let agentStack = UIStackView()
    private let moreLabel = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        titleLabel.font = Typography.name
        titleLabel.textColor = Palette.primaryText
        statusLabel.font = Typography.label
        statusLabel.textColor = Palette.subtle
        freshnessLabel.font = Typography.label
        freshnessLabel.textColor = Palette.fainter
        moreLabel.font = Typography.label
        moreLabel.textColor = Palette.subtle

        agentStack.axis = .vertical
        agentStack.spacing = 6

        let header = UIStackView(arrangedSubviews: [titleLabel, statusLabel, UIView(), freshnessLabel])
        header.axis = .horizontal
        header.spacing = 6
        header.alignment = .firstBaseline

        let root = UIStackView(arrangedSubviews: [header, agentStack, moreLabel])
        root.axis = .vertical
        root.spacing = 8

        contentView.addSubview(root)
        root.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(UIEdgeInsets(top: 12, left: 16, bottom: 12, right: 16))
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    func configure(_ model: DeviceRowModel) {
        titleLabel.text = model.title
        statusLabel.text = model.statusText
        freshnessLabel.text = model.freshnessText
        freshnessLabel.isHidden = model.freshnessText == nil

        agentStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for agent in model.agents {
            agentStack.addArrangedSubview(makeAgentRow(agent))
        }

        moreLabel.text = model.hiddenAgentCount > 0 ? "+\(model.hiddenAgentCount)" : nil
        moreLabel.isHidden = model.hiddenAgentCount == 0
    }

    private func makeAgentRow(_ agent: AgentRowModel) -> UIView {
        let name = UILabel()
        name.font = Typography.strong
        name.textColor = Palette.primaryText
        name.text = agent.name

        let rate = UILabel()
        rate.font = Typography.rate
        rate.textColor = Palette.rate
        rate.text = agent.rateText          // 오프라인이면 nil → 빈 칸
        rate.textAlignment = .right

        let quota = QuotaBarView()
        quota.configure(
            tokens5h: 0,
            autoPct: agent.fiveHour?.percent,
            weeklyPct: agent.weekly?.percent,
            isReset5h: false,
            unreadable: agent.fiveHour == nil && agent.weekly == nil
        )

        let header = UIStackView(arrangedSubviews: [name, rate])
        header.axis = .horizontal
        header.spacing = 8

        let row = UIStackView(arrangedSubviews: [header, quota])
        row.axis = .vertical
        row.spacing = 4
        return row
    }
}
```

- [ ] **Step 2: 목록 화면 구현**

`ios/Sources/AppMulti/DeviceListViewController.swift` 를 교체:

```swift
import Fleet
import UIKit

final class DeviceListViewController: UIViewController {
    private enum Section { case main }

    private let environment: AppEnvironment
    private var fleet: DeviceFleet?
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Section, String>!
    private var reprobeTimer: Timer?

    init(environment: AppEnvironment) {
        self.environment = environment
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "장치"
        view.backgroundColor = Palette.windowBackground

        navigationItem.rightBarButtonItem = UIBarButtonItem(
            barButtonSystemItem: .add, target: self, action: #selector(addDeviceTapped)
        )

        configureCollectionView()
        startFleet()
        startReprobeTimer()
    }

    // MARK: - 구성

    private func configureCollectionView() {
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = Palette.windowBackground
        let layout = UICollectionViewCompositionalLayout.list(using: config)

        collectionView = UICollectionView(frame: .zero, collectionViewLayout: layout)
        collectionView.backgroundColor = Palette.windowBackground
        collectionView.delegate = self
        view.addSubview(collectionView)
        collectionView.frame = view.bounds
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]

        let registration = UICollectionView.CellRegistration<DeviceCell, String> { [weak self] cell, _, id in
            guard let self, let model = self.rowModel(for: id) else { return }
            cell.configure(model)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { view, indexPath, id in
            view.dequeueConfiguredReusableCell(using: registration, for: indexPath, item: id)
        }

        let refresh = UIRefreshControl()
        refresh.addTarget(self, action: #selector(pulledToRefresh), for: .valueChanged)
        collectionView.refreshControl = refresh
    }

    private func startFleet() {
        // 옛 fleet 의 스트림을 먼저 끊는다 — 안 그러면 삭제된 장치의 연결이 누수되고 남은 장치는
        // 이중 연결이 된다(Ruling 17).
        fleet?.stopAll()
        let fleet = environment.makeFleet()
        fleet.onChange = { [weak self] in self?.applySnapshot() }
        self.fleet = fleet
        applySnapshot()
        Task { await fleet.startInitialRound() }
    }

    /// 포어그라운드 상태에서만 돈다. 오프라인 장치만 다시 확인한다.
    private func startReprobeTimer() {
        reprobeTimer = Timer.scheduledTimer(
            withTimeInterval: DeviceFleet.reprobeInterval, repeats: true
        ) { [weak self] _ in
            guard let fleet = self?.fleet else { return }
            Task { await fleet.retriggerOffline() }
        }
    }

    // MARK: - 데이터

    private func rowModel(for id: String) -> DeviceRowModel? {
        guard let session = fleet?.sessions.first(where: { $0.device.endpointIdHex == id }) else {
            return nil
        }
        let cached = session.latest.map { CachedSnapshot(snapshot: $0, fetchedAt: Date()) }
            ?? environment.cache.load(endpointIdHex: id)
        return DeviceListPresentation.row(
            device: session.device, status: session.status, cached: cached, now: Date()
        )
    }

    private func applySnapshot() {
        guard let fleet else { return }
        let ordered = DeviceListPresentation.sorted(
            fleet.sessions.map { ($0.device, $0.status) }
        )
        var snapshot = NSDiffableDataSourceSnapshot<Section, String>()
        snapshot.appendSections([.main])
        snapshot.appendItems(ordered.map(\.endpointIdHex))
        snapshot.reloadItems(ordered.map(\.endpointIdHex))
        dataSource.apply(snapshot, animatingDifferences: true)
    }

    // MARK: - 동작

    func reconnectAll() {
        guard let fleet else { return }
        Task { await fleet.startInitialRound() }
    }

    @objc private func pulledToRefresh() {
        guard let fleet else { return }
        Task {
            await fleet.retriggerOffline()
            collectionView.refreshControl?.endRefreshing()
        }
    }

    @objc private func addDeviceTapped() {
        // Task 15 에서 채운다.
    }
}

extension DeviceListViewController: UICollectionViewDelegate {
    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        // Task 16 에서 채운다.
    }
}
```

`Palette` 를 쓰므로 파일 맨 위에 `import DesignSystem` 를 추가한다.

- [ ] **Step 3: 스와이프 삭제**

레지스트리에 `remove(endpointIdHex:)` 가 이미 있다(Task 5). 목록에서 지울 수 있어야
16대 상한에 걸렸을 때 자리를 비울 수 있다. `configureCollectionView()` 의 `config` 설정에 추가:

```swift
        config.trailingSwipeActionsConfigurationProvider = { [weak self] indexPath in
            guard let self, let id = self.dataSource.itemIdentifier(for: indexPath) else { return nil }
            let delete = UIContextualAction(style: .destructive, title: "삭제") { _, _, done in
                self.removeDevice(endpointIdHex: id)
                done(true)
            }
            return UISwipeActionsConfiguration(actions: [delete])
        }
```

그리고 메서드를 추가한다:

```swift
    private func removeDevice(endpointIdHex: String) {
        do {
            _ = try environment.registry.remove(endpointIdHex: endpointIdHex)
            // startFleet() 이 옛 fleet 전체를 stopAll() 하므로 지운 장치의 세션도 여기서 멈춘다.
            // 이 함수는 메인 액터에서 await 없이 이어지므로 그 사이에 늦은 결과가 끼어들 틈이 없다.
            startFleet()
        } catch {
            present(
                UIAlertController(title: nil, message: "삭제하지 못했습니다", preferredStyle: .alert),
                animated: true
            )
        }
    }
```

- [ ] **Step 4: 빌드 확인**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme AppMulti \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD"
```

기대: `BUILD SUCCEEDED`.

- [ ] **Step 5: 시뮬레이터 실행 확인**

```bash
cd ios && xcrun simctl boot B155999C-1CD2-4819-92E6-B354BC972D23 2>/dev/null; \
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme AppMulti \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' \
  -derivedDataPath /tmp/appmulti-dd 2>&1 | tail -3 && \
xcrun simctl install B155999C-1CD2-4819-92E6-B354BC972D23 \
  /tmp/appmulti-dd/Build/Products/Debug-iphonesimulator/AppMulti.app && \
xcrun simctl launch B155999C-1CD2-4819-92E6-B354BC972D23 co.kr.wannypark.aiagentmonitor.multi
```

기대: 앱이 실행되고 빈 "장치" 목록이 뜬다(페어링 전이라 비어 있는 게 정상).

- [ ] **Step 6: 커밋**

```bash
git add ios/Sources/AppMulti
git commit -m "feat: 장치 목록 화면 (diffable 컬렉션 뷰, 상태별 정렬, 당겨서 새로고침, 스와이프 삭제)"
```

> **이번 범위 밖**: 스펙 §6.1 이 언급한 **드래그로 순서 바꾸기**는 넣지 않는다. `sortIndex` 는
> 추가한 순서로 고정되고, 정렬은 `온라인 → 불안정 → 오프라인` 그룹이 주로 결정한다.
> 드래그 재정렬은 `UICollectionViewDragDelegate` + 레지스트리 일괄 갱신이 필요해 별도 작업으로 남긴다.

---

### Task 15: 공유 뷰를 DesignSystem 으로 이동 + 페어링 추가 화면

**Files:**
- Move: `ios/Sources/MirrorFeature/QRScannerViewController.swift`, `AgentCardView.swift`, `SessionListView.swift` → `ios/Sources/DesignSystem/` 로
- Modify: `ios/Project.swift` (`DesignSystem` 의존성에 `Wire` 추가)
- Modify: `ios/Sources/NetworkTransport/NetworkClient.swift` (`probe(..., code:)`, `ProbeResult.issuedToken`, `TokenSlot.ephemeral` — Ruling 16)
- Test: `ios/Tests/NetworkTransportTests/TokenSlotTests.swift` (`.ephemeral` 케이스 추가)
- Create: `ios/Sources/AppMulti/AddDeviceViewController.swift`
- Modify: `ios/Sources/AppMulti/DeviceListViewController.swift` (`addDeviceTapped` 채우기)

**Interfaces:**
- Consumes: `NetworkClient.parseQrPayload`(Task 2), `DeviceRegistry.upsert`(Task 5)
- Produces: `DesignSystem.AgentCardView.configure(agent: MirrorAgent, now: Date)` — Task 16 의 상세 화면이 쓴다
- Produces: `DesignSystem.QRScannerViewController.onScan: ((String) -> Void)?`

**이 이동이 필요한 이유**: `AgentCardView`·`SessionListView`·`QRScannerViewController` 는 현재
`MirrorFeature` 에 있는데, `MirrorFeature` 는 `BLETransport`·`WidgetShared` 까지 끌고 온다.
AppMulti 가 그걸 임포트하면 쓰지도 않는 BLE 스택과 위젯 캐시가 통째로 딸려온다.
세 파일 모두 전송 계층을 모르고 `Wire`/`MirrorFormat`/`Palette`/`Typography`/`QuotaBarView` 만
쓰므로 `DesignSystem` 으로 옮기는 게 맞다.

- [ ] **Step 1: DesignSystem 에 Wire 의존성 추가**

`ios/Project.swift` 에서 `framework("DesignSystem", ...)` 줄을 다음으로 바꾼다:

```swift
        framework("DesignSystem", deps: [.target(name: "MirrorFormat"), .target(name: "Wire"), .external(name: "SnapKit")]),
```

`AgentCardView` 가 `MirrorAgent`(Wire) 를 받기 때문이다. `Wire` 는 의존성이 없어 순환이 생기지 않는다.

- [ ] **Step 2: 세 파일 이동**

```bash
cd /Users/wannypark/Desktop/@Projects/2_App/4_AIAgentMonitor
git mv ios/Sources/MirrorFeature/QRScannerViewController.swift ios/Sources/DesignSystem/QRScannerViewController.swift
git mv ios/Sources/MirrorFeature/AgentCardView.swift ios/Sources/DesignSystem/AgentCardView.swift
git mv ios/Sources/MirrorFeature/SessionListView.swift ios/Sources/DesignSystem/SessionListView.swift
```

옮긴 파일 안에서 같은 모듈이라 생략돼 있던 import 를 채운다 — 각 파일 상단에 `import Wire`,
`import MirrorFormat` 이 필요한지 확인하고 없으면 추가한다. 반대로 `MirrorFeature` 쪽에서
이 타입들을 쓰는 파일에는 `import DesignSystem` 이 있어야 한다:

```bash
grep -rn "QRScannerViewController\|AgentCardView\|SessionListView" ios/Sources/MirrorFeature/
grep -n "^import" ios/Sources/MirrorFeature/MirrorViewController.swift
```

- [ ] **Step 3: 기존 앱 회귀 확인 (이동이 깨뜨리지 않았는지)**

```bash
cd ios && mise exec -- tuist generate --no-open && \
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD" && \
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme AppBLE \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD" && \
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme MirrorFeatureTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 빌드 둘 다 `BUILD SUCCEEDED`, MirrorFeatureTests 47개 통과.
⚠️ `MirrorFeatureTests` 가 `AgentCardView` 를 `@testable import MirrorFeature` 로 쓰고 있었다면
`import DesignSystem` 으로 바꿔야 한다(`AgentCardViewTests` 가 있다면 그 파일).

- [ ] **Step 3a: NetworkTransport 확장 — 코드 페어링으로 발급 토큰 받기 (Ruling 16)**

QR 의 `code` 는 6자리 **페어링 코드**이고 토큰이 아니다. 토큰은 CODE2 뒤에 Mac 이 봉인해 내려주는
값(`authenticate` 의 `.openSealedToken` 케이스)이다. AppMulti 는 이 토큰을 `Device.token` 에 저장해야
재연결(`TokenSlot.fixed`)이 성립한다. 코드를 토큰으로 저장하면 재연결이 항상 `needsPairing` 으로 거부된다.

`ios/Sources/NetworkTransport/NetworkClient.swift` — 모두 **하위호환** 확장이다(기본값 유지, 기존 호출부 무수정):

```swift
    enum TokenSlot {
        case shared
        case fixed(String)
        /// 코드 페어링 전용. 저장된 토큰이 없고(load → nil), Mac 이 발급한 토큰도 여기엔 **저장하지 않는다**
        /// (save/clear no-op) — 발급 토큰은 `ProbeResult.issuedToken` 으로 호출부에 돌려준다.
        /// 전역 슬롯(`.shared`)을 쓰면 1:1 앱의 페어링을 덮어쓴다.
        case ephemeral
    }

    public struct ProbeResult {
        public let connection: Connection
        public let channel: SealedChannel
        public let firstSnapshot: MirrorSnapshot
        /// 이번 인증에서 Mac 이 **새로 발급한** 토큰. 코드로 페어링했을 때(`.openSealedToken`)만 non-nil.
        /// 토큰으로 재연결했을 때는 nil — 프로토콜상 재연결 경로엔 토큰 회전이 없다.
        public let issuedToken: String?
    }

    /// `token` 이 있으면 그 토큰으로(.fixed), 없고 `code` 가 있으면 코드로 페어링(.ephemeral),
    /// 둘 다 없으면 전역 슬롯(.shared — 1:1 앱·위젯의 기존 동작).
    public func probe(
        endpointIdHex: String, relayUrl: String?, addresses: [String],
        timeoutSeconds: Double, token: String? = nil, code: String? = nil
    ) async throws -> ProbeResult
```

- `authenticate(conn:code:tokens:)` 는 `(channel: SealedChannel, issuedToken: String?)` 을 돌려준다.
  `.openSealedToken` 케이스에서 `tokens.save(token)` 을 그대로 부르고(`.shared` 는 저장, `.fixed`/`.ephemeral` 은 no-op)
  `issuedToken: token` 으로 반환한다. 다른 케이스는 `issuedToken: nil`.
- `dialAuthenticateAndOpen(..., tokens:, code:)` 가 `code` 를 `authenticate` 에 넘기고 `ProbeResult.issuedToken` 을 채운다.
- 1:1 경로(`runConnection`)의 `authenticate` 호출은 반환 튜플의 `.channel` 만 쓰도록 고친다 — 동작 불변.
- `TokenSlotTests` 에 추가: `testEphemeralSlotLoadsNil`, `testEphemeralSlotIgnoresSaveAndClear`(save → true, 이후 load 여전히 nil).
- `AIMonitorWidget`·`App` 빌드가 무수정으로 성공해야 한다(기본값 덕분).

- [ ] **Step 3b: 페어링 화면 구현**

`ios/Sources/AppMulti/AddDeviceViewController.swift`:

```swift
import DesignSystem
import Fleet
import NetworkTransport
import UIKit

/// QR 스캔 → 이름 확인 → **코드로 연결해 토큰 발급** → 레지스트리 저장. 저장까지 끝나면 `onAdded` 로 알린다.
/// 스펙 6.3 의 "저장 → 즉시 연결" 을 "연결 → 저장" 으로 한 칸 당긴 것 — 저장할 토큰이 연결 결과물이기 때문(Ruling 16).
final class AddDeviceViewController: UIViewController {
    private let registry: DeviceRegistry
    var onAdded: ((Device) -> Void)?

    private let scanner = QRScannerViewController()
    private var hasHandledScan = false

    init(registry: DeviceRegistry) {
        self.registry = registry
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "장치 추가"
        view.backgroundColor = Palette.windowBackground

        addChild(scanner)
        view.addSubview(scanner.view)
        scanner.view.frame = view.bounds
        scanner.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scanner.didMove(toParent: self)

        scanner.onScan = { [weak self] payload in
            // 스캐너는 같은 코드를 연속으로 여러 번 던진다.
            guard let self, !self.hasHandledScan else { return }
            self.hasHandledScan = true
            self.handle(payload)
        }
    }

    private func handle(_ payload: String) {
        guard let parsed = NetworkClient.parseQrPayload(payload) else {
            present(alert("QR 코드를 인식하지 못했습니다") { [weak self] in
                self?.hasHandledScan = false
            }, animated: true)
            return
        }
        askForName(defaultName: parsed.macName ?? "", parsed: parsed)
    }

    private func askForName(defaultName: String, parsed: NetworkClient.ParsedPairingPayload) {
        let sheet = UIAlertController(
            title: "장치 이름", message: "목록에 표시할 이름입니다.", preferredStyle: .alert
        )
        sheet.addTextField { field in
            field.text = defaultName
            field.placeholder = "예: 작업실 맥"
        }
        sheet.addAction(UIAlertAction(title: "취소", style: .cancel) { [weak self] _ in
            self?.hasHandledScan = false
        })
        sheet.addAction(UIAlertAction(title: "추가", style: .default) { [weak self] _ in
            let typed = sheet.textFields?.first?.text?.trimmingCharacters(in: .whitespaces)
            self?.pairAndSave(parsed: parsed, userLabel: (typed?.isEmpty == false) ? typed : nil)
        })
        present(sheet, animated: true)
    }

    /// 페어링 타임아웃. probe 의 3초는 "이미 아는 장치가 켜져 있나" 용이고, 페어링은 사용자가 Mac 앞에서
    /// 기다리는 1회성 작업이라 hole-punch 가 느린 네트워크를 더 참아준다.
    private static let pairingTimeoutSeconds: Double = 10

    private func pairAndSave(parsed: NetworkClient.ParsedPairingPayload, userLabel: String?) {
        let waiting = UIAlertController(title: nil, message: "Mac 에 연결하는 중…", preferredStyle: .alert)
        present(waiting, animated: true)
        Task { [weak self] in
            guard let self else { return }
            let outcome: Result<String, Error>
            do {
                let client = NetworkClient(endpointProvider: .shared)
                let result = try await client.probe(
                    endpointIdHex: parsed.endpointIdHex,
                    relayUrl: parsed.relayUrl,
                    addresses: parsed.addresses,
                    timeoutSeconds: Self.pairingTimeoutSeconds,
                    code: parsed.code
                )
                // 페어링용 연결은 여기서 닫는다. fleet 이 저장된 토큰으로 다시 붙는다 — hole-punch 1회가
                // 추가되지만 페어링은 장치당 한 번이다.
                try? result.connection.close(errorCode: 0, reason: Data())
                guard let token = result.issuedToken else {
                    // 코드로 인증했는데 토큰이 안 왔다 = Mac 이 이미 이 기기를 알고 있어 재연결 경로로 갔다는 뜻.
                    // AppMulti 는 그 토큰을 모르므로 저장할 수 없다.
                    throw NetworkClientError.needsPairing
                }
                outcome = .success(token)
            } catch {
                outcome = .failure(error)
            }
            waiting.dismiss(animated: true) { [weak self] in
                guard let self else { return }
                switch outcome {
                case .success(let token):
                    self.save(parsed: parsed, token: token, userLabel: userLabel)
                case .failure:
                    self.present(self.alert("Mac 화면의 코드가 만료됐거나 연결할 수 없습니다. QR 을 다시 스캔하세요") { [weak self] in
                        self?.hasHandledScan = false
                    }, animated: true)
                }
            }
        }
    }

    private func save(parsed: NetworkClient.ParsedPairingPayload, token: String, userLabel: String?) {
        let device = Device(
            endpointIdHex: parsed.endpointIdHex,
            token: token,   // Mac 이 발급한 토큰. QR 의 code 는 여기 오기 전에 소비됐다.
            relayUrl: parsed.relayUrl,
            addresses: parsed.addresses,
            macHostname: parsed.macName,
            userLabel: userLabel,
            sortIndex: 0    // upsert 가 max+1 로 덮어쓴다
        )
        do {
            _ = try registry.upsert(device)
            onAdded?(device)
            dismiss(animated: true)
        } catch DeviceRegistryError.deviceLimitReached {
            present(alert("장치는 최대 \(DeviceRegistry.maxDevices)대까지 추가할 수 있습니다") { [weak self] in
                self?.hasHandledScan = false
            }, animated: true)
        } catch {
            present(alert("저장하지 못했습니다: \(error.localizedDescription)") { [weak self] in
                self?.hasHandledScan = false
            }, animated: true)
        }
    }

    private func alert(_ message: String, onDismiss: @escaping () -> Void) -> UIAlertController {
        let controller = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        controller.addAction(UIAlertAction(title: "확인", style: .default) { _ in onDismiss() })
        return controller
    }
}
```

`DeviceListViewController.addDeviceTapped` 를 채운다:

```swift
    @objc private func addDeviceTapped() {
        let add = AddDeviceViewController(registry: environment.registry)
        add.onAdded = { [weak self] _ in
            // 레지스트리가 바뀌었으므로 fleet 을 다시 만든다(startFleet 이 옛 fleet 을 stopAll 한다 — Ruling 17).
            self?.startFleet()
        }
        present(UINavigationController(rootViewController: add), animated: true)
    }
```

- [ ] **Step 4: 빌드 확인**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme AppMulti \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD"
```

기대: `BUILD SUCCEEDED`.

- [ ] **Step 5: 전체 회귀 확인**

```bash
cd ios && for s in MirrorFeatureTests NetworkTransportTests WidgetSharedTests FleetTests; do \
  echo "--- $s ---"; \
  xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme $s \
    -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed [0-9]+ tests|TEST"; \
done
```

기대: 전부 통과.

- [ ] **Step 6: 커밋**

```bash
git add ios/Sources/DesignSystem/QRScannerViewController.swift ios/Sources/MirrorFeature ios/Sources/AppMulti
git commit -m "feat: 장치 추가(QR 스캔+이름 지정) 화면, 스캐너를 DesignSystem 으로 이동"
```

---

### Task 16: 상세 화면

**Files:**
- Create: `ios/Sources/AppMulti/DeviceDetailViewController.swift`
- Modify: `ios/Sources/AppMulti/DeviceListViewController.swift` (`didSelectItemAt` 채우기)
- Modify: `ios/Sources/Fleet/DeviceFleet.swift` (`onSessionChange` 추가 — Ruling 23)
- Test: `ios/Tests/FleetTests/DeviceFleetTests.swift`

**Interfaces:**
- Consumes: `DeviceSession`(Task 10), `DeviceFleet.onSessionChange`(이 태스크가 추가), `AgentCardView`·`QuotaBarView`(DesignSystem)

상세 진입 시 **다른 세션을 끊지 않는다** — 목록으로 돌아왔을 때 즉시 최신값이 보이고, 끊었다 다시 붙이면 probe 승격으로 아끼려던 비용을 그대로 낸다.

- [ ] **Step 1a: `DeviceFleet.onSessionChange` (Ruling 23 — 먼저 한다)**

`DeviceSession.onChange` 는 **fleet 의 것**이다 — `DeviceFleet.init` 이 걸어두고 `handleChange` 에서 캐시 쓰기와
`fleet.onChange` 를 돌린다. 상세 화면이 이걸 덮어쓰면(원래 계획) 상세를 한 번 다녀온 뒤 fleet 이 그 세션의 변화를
영영 못 본다. 상세는 fleet 이 제공하는 별도 훅으로 구독한다.

`ios/Sources/Fleet/DeviceFleet.swift`:
```swift
    /// 세션 **하나**의 변화를 보고 싶은 화면(상세)용. `onChange`(목록용, 인자 없음)와 별개로 어느 세션이
    /// 바뀌었는지 넘긴다. 상세 화면이 viewWillAppear 에서 걸고 viewWillDisappear 에서 nil 로 되돌린다.
    /// `DeviceSession.onChange` 자체는 fleet 소유라 화면이 건드리면 안 된다 — 캐시 쓰기가 끊긴다.
    public var onSessionChange: ((DeviceSession) -> Void)?
```
`handleChange(_:)` 끝의 `onChange?()` 앞에 `onSessionChange?(session)` 을 추가한다.

테스트(`DeviceFleetTests`): `testOnSessionChangeReportsChangedSessionAndOnChangeStillFires` — 세션 1대가 online 이 되면
`onSessionChange` 가 그 세션(`===`)으로 불리고 `onChange` 도 여전히 불린다. 뮤테이션: `onSessionChange?(session)` 삭제 → 실패.

- [ ] **Step 1b: 상세 화면 구현**

`ios/Sources/AppMulti/DeviceDetailViewController.swift`:

```swift
import DesignSystem
import Fleet
import SnapKit
import UIKit
import Wire

/// 기존 1:1 앱의 세부 화면과 같은 구성. 카드 렌더링은 `AgentCardView` 를 그대로 쓴다.
final class DeviceDetailViewController: UIViewController {
    private let session: DeviceSession
    private let fleet: DeviceFleet
    private let cache: DeviceSnapshotCache

    private let statusLabel = UILabel()
    private let cardStack = UIStackView()
    private let scrollView = UIScrollView()

    init(session: DeviceSession, fleet: DeviceFleet, cache: DeviceSnapshotCache) {
        self.session = session
        self.fleet = fleet
        self.cache = cache
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = session.device.displayName
        view.backgroundColor = Palette.windowBackground

        statusLabel.font = Typography.label
        statusLabel.textColor = Palette.subtle

        cardStack.axis = .vertical
        cardStack.spacing = 12

        let root = UIStackView(arrangedSubviews: [statusLabel, cardStack])
        root.axis = .vertical
        root.spacing = 12

        view.addSubview(scrollView)
        scrollView.addSubview(root)
        scrollView.snp.makeConstraints { $0.edges.equalTo(view.safeAreaLayoutGuide) }
        root.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(16)
            make.width.equalTo(scrollView).offset(-32)
        }

        // session.onChange 는 건드리지 않는다 — fleet 소유(캐시 쓰기·목록 갱신). 구독은 아래
        // viewWillAppear/viewWillDisappear 의 fleet.onSessionChange 로 한다(Ruling 23).
        render()

        // 오프라인 장치에 들어왔다면 사용자 의도가 명확하므로 즉시 1회 재시도한다.
        if session.status == .offline || session.status == .idle {
            Task { await session.retrigger() }
        }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        fleet.onSessionChange = { [weak self] changed in
            guard let self, changed === self.session else { return }
            self.render()
        }
        render()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        fleet.onSessionChange = nil
    }

    private func render() {
        statusLabel.text = statusText()

        let snapshot = session.latest ?? cache.load(endpointIdHex: session.device.endpointIdHex)?.snapshot
        let agents = snapshot?.agents ?? []

        cardStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for agent in agents {
            let card = AgentCardView()
            card.configure(agent: agent, now: Date())
            cardStack.addArrangedSubview(card)
        }
    }

    private func statusText() -> String {
        switch session.status {
        case .online: return "연결됨"
        case .probing: return "확인 중"
        case .unstable: return "재연결 중"
        case .offline: return "오프라인 — 마지막으로 받은 값"
        case .needsRepairing: return "재페어링이 필요합니다. QR 을 다시 스캔하세요."
        case .versionMismatch: return "Mac 앱 버전이 맞지 않습니다."
        case .idle: return "대기"
        }
    }
}
```

> 구현 주의: `AgentCardView.configure(...)` 의 실제 시그니처를 `ios/Sources/DesignSystem/AgentCardView.swift` 에서 확인하고 그대로 맞춘다. 기존 `MirrorViewController` 가 부르는 형태를 그대로 따라 쓰면 된다.

`DeviceListViewController` 의 선택 처리를 채운다:

```swift
extension DeviceListViewController: UICollectionViewDelegate {
    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        guard let id = dataSource.itemIdentifier(for: indexPath),
              let session = fleet?.sessions.first(where: { $0.device.endpointIdHex == id }) else {
            return
        }
        // 다른 세션은 끊지 않는다 — 목록으로 돌아왔을 때 즉시 최신값이 보여야 한다.
        let detail = DeviceDetailViewController(session: session, fleet: fleet, cache: environment.cache)   // fleet 은 여기서 non-nil — guard let 로 풀어 쓴다
        navigationController?.pushViewController(detail, animated: true)
    }
}
```

목록 쪽 `viewWillAppear` 는 **`session.onChange` 를 되돌리지 않는다** — 그 자리는 fleet 의 것이고 상세가 건드리지
않았으므로 되돌릴 게 없다. 목록은 이미 `fleet.onChange` 로 갱신을 받는다(Task 14).

- [ ] **Step 2: 빌드 확인**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme AppMulti \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD"
```

기대: `BUILD SUCCEEDED`.

- [ ] **Step 3: 전체 회귀 확인**

```bash
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD" && \
xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme AppBLE \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "error:|BUILD" && \
xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme FleetTests \
  -destination 'platform=iOS Simulator,id=B155999C-1CD2-4819-92E6-B354BC972D23' 2>&1 | grep -E "Executed|TEST"
```

기대: 빌드 둘 다 성공, FleetTests 전부 통과.

- [ ] **Step 4: 커밋**

```bash
git add ios/Sources/AppMulti
git commit -m "feat: 장치 상세 화면 (AgentCardView 재사용, 오프라인 진입 시 1회 재시도)"
```

---

### Task 17: 실기 통합 검증

이 태스크는 코드 변경이 없다. 시뮬레이터로는 여러 Mac 과의 실제 iroh 연결·오프라인 전환을 재현할 수 없다.

**준비**: Mac 2대 이상에서 AI Agent Monitor 를 켜고 네트워크 공유를 활성화한다.

- [ ] **Step 1: 실기기에 설치**

Xcode 에서 실기기를 선택해 `AppMulti` 스킴을 Run 한다.
새 App ID(`co.kr.wannypark.aiagentmonitor.multi`)라 프로비저닝 프로파일 발급이 필요하다 —
Xcode 가 자동으로 만든다. 위젯이 없어 App Group·Keychain Sharing 케이퍼빌리티는 필요 없다.

- [ ] **Step 2: 첫 Mac 페어링**

`+` → Mac 1 의 QR 스캔 → **이름 입력창에 Mac 의 실제 이름이 미리 채워져 있는지 확인**(Task 1 의 `name` 파라미터가 동작한다는 증거) → 추가 → 목록에 나타나고 사용량이 채워지는지 확인.

- [ ] **Step 3: 둘째 Mac 페어링**

Mac 2 의 QR 을 스캔해 추가한다. 두 대가 **동시에** 온라인으로 뜨고 각자 tok/s 가 갱신되는지 확인한다.

- [ ] **Step 4: 오프라인 전환**

Mac 2 의 네트워크 공유를 끄거나 Mac 을 잠근다. 3회 실패 뒤 해당 셀이 "오프라인"으로 바뀌고,
**한도 %와 막대는 남되 tok/s 는 사라지고** "N분 전" 이 뜨는지 확인한다.
Mac 1 은 영향을 받지 않아야 한다.

- [ ] **Step 5: 재탐색 트리거**

Mac 2 를 다시 켜고 **당겨서 새로고침** 한다. 다시 온라인이 되는지 확인한다.
(타이머는 3분이라 기다려도 되지만 수동 트리거로 즉시 확인할 수 있다.)

- [ ] **Step 6: 포어그라운드 복귀**

앱을 백그라운드로 보냈다가(홈 버튼) 30초 뒤 돌아온다. **두 대 모두** 자동으로 다시 연결되는지 확인한다.

- [ ] **Step 7: 상세 화면**

온라인 장치를 탭해 상세로 들어가 카드가 실시간으로 갱신되는지 보고, 뒤로 나왔을 때
목록이 여전히 실시간인지(다른 세션이 끊기지 않았는지) 확인한다.
오프라인 장치도 탭해서 "오프라인 — 마지막으로 받은 값"과 캐시된 카드가 보이는지 확인한다.

- [ ] **Step 8: 재페어링 필요 경로**

Mac 1 에서 앱을 껐다 켜 페어링 상태를 초기화한 뒤(또는 Mac 쪽에서 기기를 제거한 뒤)
iPhone 에서 재탐색한다. 해당 셀이 **"재페어링 필요"** 가 되고, **재시도를 반복하지 않는지**
(로그에 probe 가 계속 찍히지 않는지) 확인한다.

- [ ] **Step 9: 기존 앱 회귀**

기존 `App`(1:1 미러)을 같은 기기에 설치해 페어링·연결이 그대로 동작하는지 확인한다.
`NetworkTransport` 를 건드렸으므로 이 확인이 필요하다.

- [ ] **Step 10: 문제 없으면 커밋**

```bash
git add -A
git commit -m "chore: AI Monitor Multi 실기 통합 검증 완료"
```

---
