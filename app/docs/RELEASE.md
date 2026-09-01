# 릴리즈 / 배포 가이드

AI Agent Monitor를 릴리즈(프로덕션) 빌드하고 배포하는 방법.

---

## TL;DR — 내 Mac에서만 쓸 때

```bash
cd <repo>
. "$HOME/.cargo/env"
pnpm install --frozen-lockfile
pnpm tauri build
open "src-tauri/target/release/bundle/macos/AI Agent Monitor.app"
```

- 로컬에서 빌드한 `.app`은 **ad-hoc 서명**되어 있고 quarantine 속성이 없으므로 **그대로 실행**된다(Developer ID·공증 불필요).
- 상시 사용하려면 **시스템 설정 → 일반 → 로그인 항목**에 `.app`을 추가.

> 다른 Mac에 **배포**하려면 아래 "배포(서명+공증)" 섹션 참고.

---

## 사전 요건

| 도구 | 비고 |
|---|---|
| Rust (rustup) | `~/.cargo/bin` — `cargo`/`rustc` |
| Node | mise 등으로 설치됨 |
| pnpm | lockfile 고정 설치 및 Tauri 빌드 |
| (배포 시) Xcode + Developer ID 인증서 | 서명·공증용 |

CI와 동일하게 Node 22와 lockfile에 맞는 pnpm 사용을 권장한다.

---

## 1. 릴리즈 빌드

```bash
cd <repo>
. "$HOME/.cargo/env"

pnpm install --frozen-lockfile
pnpm tauri build
```

**산출물** (`src-tauri/target/release/bundle/`):
- `macos/AI Agent Monitor.app` — 앱 번들
- `dmg/AI Agent Monitor_0.1.0_aarch64.dmg` — 설치 dmg (Apple Silicon)
- `macos/AI Agent Monitor.app.tar.gz` / `.sig` — 자동 업데이트용 macOS 번들
- Windows installer `.sig` — 자동 업데이트용 Windows 서명 파일

릴리즈 번들은 프론트엔드를 **임베드**하므로 dev 서버/`tauri dev` 없이 standalone 실행된다.

### 자동 업데이트

앱은 시작 후 백그라운드에서 GitHub Releases의 `latest.json`을 확인한다. 최신 서명 릴리즈가 있으면 자동으로 다운로드·검증·설치 후 재시작한다.

릴리즈 빌드 전 GitHub Secrets에 다음 값을 설정해야 한다.

| Secret | 값 |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | `/Users/wannypark/.tauri/ai-agent-monitor.key` 파일 내용 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 비워둠(현재 키는 password 없음) |

private key를 잃어버리면 기존 설치 앱이 새 업데이트를 신뢰할 수 없으므로 반드시 안전하게 보관한다.

---

## 2. 내 Mac에서 사용 (서명/공증 불필요)

1. **설치**: `.app`을 `/Applications`로 드래그(또는 `.dmg`를 열어 드래그). 그대로 실행해도 됨.
2. **실행**: 더블클릭. Dock 아이콘 없는 **메뉴바 앱** — 우상단 tray 아이콘 → **좌클릭 = popover**, 우클릭 메뉴 = Detail/로그/종료.
3. **혹시 Gatekeeper가 막으면**(예: 다운로드한 경우): 우클릭 → 열기, 또는
   ```bash
   xattr -dr com.apple.quarantine "/Applications/AI Agent Monitor.app"
   ```
4. **로그인 시 자동 실행**(상시 모니터 권장): 시스템 설정 → 일반 → 로그인 항목 → `+`로 추가.

> ad-hoc 서명(`Signature=adhoc`)은 **이 Mac에서만** 신뢰된다. 다른 Mac에 주면 "확인되지 않은 개발자" 경고가 뜬다 → 배포하려면 아래로.

---

## 3. 배포(다른 Mac 대상) — Developer ID 서명 + 공증

App Store 밖으로 배포하려면 **`Developer ID Application` 인증서로 서명 + Apple 공증(notarization)**이 필요하다.

### 3-1. 준비
1. **Apple Developer Program** 멤버십($99/년). Developer ID 인증서 생성은 **Account Holder/Admin** 권한 필요.
2. **`Developer ID Application` 인증서 생성·설치**
   - Xcode → Settings → Accounts → 팀 → **Manage Certificates → `+` → Developer ID Application** (가장 쉬움), 또는 developer.apple.com → Certificates에서 생성 후 더블클릭 설치.
   - 확인: `security find-identity -v -p codesigning | grep "Developer ID Application"`
3. **공증 자격증명**(둘 중 하나)
   - **앱 암호**: appleid.apple.com → 로그인·보안 → 앱 암호 생성. (`APPLE_ID` + `APPLE_PASSWORD` + `APPLE_TEAM_ID`)
   - **App Store Connect API 키**(.p8): (`APPLE_API_ISSUER` + `APPLE_API_KEY` + `APPLE_API_KEY_PATH`)

### 3-2. Tauri 설정 (`src-tauri/tauri.conf.json`의 `bundle`에 추가)
```jsonc
"macOS": {
  "signingIdentity": "Developer ID Application: <이름> (<TEAMID>)",
  "hardenedRuntime": true,
  "minimumSystemVersion": "11.0"
}
```
> `signingIdentity`는 생략하고 아래 `APPLE_SIGNING_IDENTITY` env로 줘도 된다. hardened runtime은 서명 시 Tauri가 기본 적용.

### 3-3. 서명+공증 빌드 (env 세팅 후 `tauri build` 한 번에 서명→공증→staple 자동)
```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: <이름> (<TEAMID>)"

# 공증 A안: Apple ID + 앱 암호
export APPLE_ID="<appleid 이메일>"
export APPLE_PASSWORD="<앱-암호>"
export APPLE_TEAM_ID="<TEAMID>"

# 공증 B안: App Store Connect API 키 (위 3개 대신)
# export APPLE_API_ISSUER="<issuer-uuid>"
# export APPLE_API_KEY="<KeyID>"
# export APPLE_API_KEY_PATH="/path/AuthKey_<KeyID>.p8"

pnpm tauri build
```
> CI/무인 빌드는 키체인 대신 인증서를 주입: `APPLE_CERTIFICATE`(base64 .p12) + `APPLE_CERTIFICATE_PASSWORD` + `APPLE_SIGNING_IDENTITY`.
> 공증은 Apple 서버 왕복이라 빌드가 수 분 더 걸린다.

### 3-4. 검증
```bash
APP="src-tauri/target/release/bundle/macos/AI Agent Monitor.app"
codesign -dv --verbose=4 "$APP"     # Authority: Developer ID Application ...
spctl -a -vvv "$APP"                # accepted, source=Notarized Developer ID
xcrun stapler validate "$APP"       # The validate action worked!
```

---

## 운영 메모 (이 앱 특성)

- **메뉴바 전용 앱**(Dock 아이콘 없음, `ActivationPolicy::Accessory`). tray 좌클릭 popover / 우클릭 메뉴.
- **포트**: `127.0.0.1:4319` = Claude quota 프록시(opt-in). 앱이 떠 있을 때만 동작.
- **Claude 사용량(%)**: opt-in. Claude Code 환경에 `ANTHROPIC_BASE_URL=http://localhost:4319` 설정 시 프록시가 `anthropic-ratelimit-*` 헤더로 실측 캡처. 또는 popover의 "🔄 동기화" 버튼 / 활동 중 10분마다 자동 핑(`claude -p ping`). ⚠️ 상시 `ANTHROPIC_BASE_URL`을 영구 설정하면 앱이 꺼졌을 때 Claude Code 요청이 실패하니, 영구 설정은 앱을 로그인 자동실행으로 항상 띄울 때만 권장.
- **Codex 사용량(%)**: zero-config. `~/.codex/sessions/.../rollout-*.jsonl`의 `rate_limits`를 읽어 5h·주간 사용률을 자동 표시(프록시·핑 불필요).
- **claude 핑**: 절대경로 `~/.local/bin/claude`를 우선 사용 → Finder 실행(빈약한 PATH)에서도 동작.
- **진단**: 현재 Rust `tracing` subscriber가 없어 파일 로그가 생성되지 않는다. Devices 탭의 오류와 CYD 시리얼 로그를 우선 확인한다.
- **자체 DB 없음**: 메모리 5h ring/bucket. 재시작 시 jsonl/rollout 다시 읽어 5h 복원.
