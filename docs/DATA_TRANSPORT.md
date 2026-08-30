# iOS·CYD 데이터 송수신

이 문서는 Mac 앱이 만든 사용량 스냅샷이 iOS 앱과 ESP32 CYD 보드에 도착하기까지의 현재 구현을 설명한다. 보안 알고리즘과 키 수명은 [보안과 E2EE](SECURITY.md), 전체 컴포넌트 구성은 [아키텍처](ARCHITECTURE.md)를 함께 참고한다.

## 전체 흐름

Mac 앱은 Claude Code, Codex, Antigravity의 로컬 기록을 수집하고 하나의 `MirrorSnapshot`으로 정규화한다. 각 클라이언트는 먼저 Mac과 v2 페어링 또는 재인증을 완료해 연결 전용 암호 채널을 만든다. Mac은 인가된 클라이언트에만 최대 초당 한 번 스냅샷을 봉인해 보낸다.

```text
에이전트 로그 → Mac 수집기 → MirrorSnapshot → E2EE 봉인
                                              ├─ BLE Notify → iOS
                                              ├─ iroh QUIC  → iOS
                                              ├─ BLE Notify → CYD
                                              └─ WebSocket  → CYD
```

인증 메시지는 양방향이지만, 인증 후 사용량 데이터는 Mac에서 클라이언트로 흐르는 단방향 미러다. iOS나 CYD가 Mac의 에이전트 상태를 변경하거나 원본 로그를 요청하지 않는다.

## 공통 스냅샷 형식

전송 경로가 달라도 복호화 뒤의 JSON은 동일하다.

| 필드 | 의미 |
|---|---|
| `v` | 와이어 형식 버전. 클라이언트가 지원하지 않으면 스트리밍을 중단한다. |
| `t` | Mac이 스냅샷을 만든 Unix epoch 초 |
| `a[].k` | 에이전트 종류: `0` Claude, `1` Codex, `2` Antigravity |
| `a[].r` | 최근 처리 속도(tok/s) |
| `a[].t5` | 최근 5분 입력+출력 토큰 합계 |
| `a[].p5`, `a[].r5` | 5시간 사용률과 리셋 시각(있을 때만) |
| `a[].pw`, `a[].rw` | 주간 사용률과 리셋 시각(있을 때만) |
| `a[].pj[]` | 프로젝트별 표시 정보 |
| `pj[].id` | 전체 경로의 FNV-1a 32비트 해시 |
| `pj[].n` | 프로젝트 경로의 마지막 이름 |
| `pj[].m` | 최근 모델 이름 |
| `pj[].r`, `pj[].t` | 프로젝트 tok/s와 마지막 이벤트 시각 |
| `pj[].s` | 활동 상태: `0` Active, `1` Idle, `2` Dormant |

원본 프롬프트, 응답 내용, 전체 프로젝트 경로는 보내지 않는다. CYD는 메모리 사용량을 제한하기 위해 최대 4개 에이전트와 에이전트당 12개 프로젝트만 고정 배열에 보관하며, 초과 항목은 화면 표시에서 잘린다.

## 공통 인증과 E2EE

모든 전송은 연결마다 새 X25519 임시 키를 만들고 v2 핸드셰이크만 허용한다.

### 최초 페어링

1. 클라이언트가 공개키를 담은 `HELLO2`를 보낸다.
2. Mac이 공개키와 nonce를 담은 `AwaitingCode2`를 응답한다.
3. 사용자가 Mac에 표시된 6자리 코드를 iOS 또는 CYD에 입력한다.
4. 클라이언트는 코드 자체가 아니라 코드와 핸드셰이크 transcript로 만든 바인딩을 `CODE2`에 담아 보낸다.
5. Mac은 새 peer 토큰을 암호화해 돌려주고, 양쪽이 연결 전용 송수신 키와 카운터를 연다.

### 저장된 peer의 재연결

1. 클라이언트가 저장된 토큰의 peer ID와 새 공개키를 `AUTH2`에 담아 보낸다.
2. Mac은 공개키와 nonce를 응답한다.
3. 클라이언트가 저장된 토큰으로 `PROOF2`를 만들고 Mac이 검증한다.
4. 성공한 연결에서만 스냅샷 스트리밍을 시작한다.

스냅샷 봉인 형식은 `counter(8바이트, big-endian) || ciphertext || Poly1305 tag(16바이트)`다. ChaCha20-Poly1305 인증에 실패하거나 재생된 카운터가 오면 해당 프레임을 버린다. v2 세션에서 평문 스냅샷을 받아들이는 v1 폴백은 없다.

## iOS의 송수신 경로

iOS는 사용자가 고른 연결 방식에 따라 BLE 또는 원격 네트워크 클라이언트를 실행한다. 둘은 같은 페어링 상태기계, peer 토큰 저장소, `SealedChannel`, `MirrorSnapshot` 디코더를 사용한다.

### BLE

1. CoreBluetooth Central이 서비스 UUID `07A98A35-16C7-4BBA-A296-E28B78B7E683`을 광고하는 Mac을 찾는다.
2. 연결 후 Auth 특성 `1403603A-4C78-4899-A2B8-FDA198101900`을 구독하고, 이 특성에 인증 요청을 쓰고 응답 Notify를 받는다.
3. 인증이 끝난 뒤 Snapshot 특성 `0AE789AA-EF38-4A35-9E72-A7CD7AD995D5`을 구독한다.
4. Mac은 봉인 프레임을 ATT MTU에 맞춰 나누어 Notify한다. 각 청크 앞에는 `[frame_id][chunk_index][chunk_count]` 3바이트 헤더가 붙는다.
5. iOS는 동일 프레임의 청크가 0부터 순서대로 모두 왔을 때만 합친다. 누락, 순서 변경, 프레임 ID 변경이 있으면 불완전 프레임을 버린다.
6. 완성된 봉인 프레임을 열고 JSON 버전을 확인한 뒤 UI에 게시한다.

BLE 알림 하나가 스냅샷 하나라는 보장은 없다. 화면 데이터가 갱신되지 않을 때는 GATT 연결뿐 아니라 Snapshot 구독 완료, 청크 재조립, E2EE 열기까지 순서대로 확인해야 한다.

### iroh 원격 네트워크

1. Mac의 QR 코드에 있는 `aim://pair` URL에서 endpoint ID, 페어링 코드, 선택적 relay/주소 힌트를 읽는다.
2. iroh QUIC 연결을 ALPN `aim/mirror/1`로 연다.
3. 인증 요청마다 양방향 stream 하나를 열어 요청을 쓰고 종료한 뒤, 같은 stream의 응답을 최대 4 KiB까지 읽는다.
4. 인가가 끝나면 Mac이 연 장수명 단방향 stream을 받는다.
5. 봉인 프레임은 임의의 이진 바이트를 안전하게 줄 단위로 나누기 위해 hex 문자열 한 줄로 전송된다. iOS는 newline으로 분리하고 hex를 이진 프레임으로 되돌린다.
6. 최대 64 KiB 프레임을 복호화하고 `MirrorSnapshot`을 디코딩해 UI에 게시한다.

연결된 v2 세션에서 `{`로 시작하는 평문 JSON 줄이 오면 다운그레이드로 간주해 버린다. 손상된 한 프레임은 연결 전체를 끊지 않고 버리며 다음 프레임에서 회복한다.

## CYD의 송수신 경로

CYD는 설정에 저장된 BLE 또는 Wi-Fi 모드 중 하나만 실행한다. 모드와 Wi-Fi 정보, Mac 주소, peer 토큰은 ESP32 NVS에 저장된다. 전송 계층은 공통 복호화·JSON 파서를 호출하며, 검증과 파싱에 성공한 최신 스냅샷만 화면 모델로 교체한다.

### Wi-Fi/LAN WebSocket

1. CYD가 mDNS `_aim._tcp`로 Mac을 찾는다. 찾지 못하면 저장된 호스트/IP를 사용한다.
2. TCP 4320의 `/mirror` WebSocket에 연결한다.
3. `HELLO2`/`CODE2` 또는 `AUTH2`/`PROOF2` 인증 메시지는 WebSocket text frame으로 주고받는다.
4. 인증 후 Mac은 봉인된 스냅샷 하나를 WebSocket binary frame 하나로 보낸다. BLE와 달리 별도 청크 헤더나 hex 변환이 없다.
5. CYD는 64 KiB 상한을 검사하고 봉인 프레임을 연 뒤 JSON을 고정 크기 화면 모델로 파싱한다.

Wi-Fi 연결은 지수 백오프로 다시 시도한다. 인가 후 스냅샷이 45초 동안 오지 않으면 소켓을 끊고 검색·연결부터 다시 시작한다. Mac 서버도 ping과 유휴 연결 정리를 수행하며 동시 클라이언트 수와 인증 제한 시간을 둔다.

### BLE

1. NimBLE Central이 서비스 UUID 또는 `AIM` 장치 이름을 광고하는 Mac을 스캔한다.
2. GATT 연결 후 Auth와 Snapshot 특성을 구독하고 v2 핸드셰이크를 시작한다.
3. Snapshot Notify의 3바이트 헤더를 읽어 프레임을 순서대로 재조립한다.
4. 완성 크기가 64 KiB 이하일 때만 E2EE로 열고 JSON을 파싱한다.
5. 인가 후 45초 동안 새 스냅샷이 없으면 연결을 끊고 다시 스캔한다.

청크가 빠지거나 순서가 바뀐 프레임은 폐기한다. 이후 새 `frame_id`의 첫 청크가 오면 재조립을 새로 시작하므로 일시적인 BLE 손실이 영구적인 빈 화면으로 이어지지 않는다.

## 경로별 프레이밍 비교

| 대상/경로 | 인증 메시지 | 스냅샷 운반 | 수신 완료 기준 |
|---|---|---|---|
| iOS BLE | Auth 특성 write/notify | 봉인 바이너리를 MTU 청크로 분할 | 모든 `[frame_id, index, count]` 청크 수신 |
| iOS iroh | 요청별 QUIC bi-stream | uni-stream의 newline 구분 hex | 한 줄 수신 후 hex decode |
| CYD BLE | Auth 특성 write/notify | 봉인 바이너리를 MTU 청크로 분할 | 모든 `[frame_id, index, count]` 청크 수신 |
| CYD LAN | WebSocket text frame | WebSocket binary frame | binary message 하나 수신 |

## 오류 처리와 상태 표시

- 잘못된 코드에는 남은 시도 횟수를 표시하고, 시도가 소진되거나 peer가 취소되면 자동 인증 재전송을 멈춘다.
- 연결이 끊기면 임시 키, 세션 키, 수신 카운터와 불완전 프레임을 폐기한다. 저장된 peer 토큰만 다음 연결에 사용한다.
- 지원하지 않는 스냅샷 `v`는 재연결로 해결할 수 없으므로 버전 불일치 상태로 중단한다.
- 복호화 또는 JSON 파싱에 실패한 단일 프레임은 최신 정상 화면을 유지한 채 버린다.
- Mac BLE 송신은 central별 대기열을 사용해 backpressure를 처리한다. 느린 수신자 때문에 다른 인가된 수신자의 프레임을 평문으로 보내거나 인증을 생략하지 않는다.

## 구현 위치

| 역할 | 주요 코드 |
|---|---|
| 공통 스냅샷 생성 | `src-tauri/src/ble/wire.rs` |
| Mac BLE 송신 | `src-tauri/src/ble/` |
| Mac iroh 송신 | `src-tauri/src/network/` |
| Mac LAN WebSocket 송신 | `src-tauri/src/lan/` |
| iOS BLE 수신 | `ios/Sources/BLETransport/BLEClient.swift` |
| iOS iroh 수신 | `ios/Sources/NetworkTransport/NetworkClient.swift` |
| iOS 공통 인증/E2EE | `ios/Sources/BLETransport/PairingClient.swift`, `SealedChannel.swift` |
| CYD LAN 수신 | `firmware/cyd/src/transport.cpp` |
| CYD BLE 수신 | `firmware/cyd/src/transport_ble.cpp` |
| CYD E2EE/파싱 | `firmware/cyd/lib/cryptov2/cryptov2.cpp`, `firmware/cyd/lib/snapshot/snapshot.cpp` |
