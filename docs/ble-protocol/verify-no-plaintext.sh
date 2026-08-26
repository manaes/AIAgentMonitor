#!/bin/bash
# DEVICE-TEST.md §7-7 — 캡처에 평문이 없는지 검사한다.
#
# 이 스크립트의 요점은 "못 찾았다" 가 아니라 **찾을 수 있었는데 없다** 를 보이는
# 것이다. 캡처가 비어 있거나 엉뚱한 인터페이스를 떴어도 "평문 없음" 은 항상
# 통과한다 — 그래서 먼저 양성 대조(positive control)를 돌린다. 대조가 실패하면
# 나머지 결과는 전부 무의미하므로 거기서 멈춘다.
#
# 사용법:
#   ./verify-no-plaintext.sh <캡처파일> [6자리코드]
#
# 6자리 코드는 페어링할 때 맥 화면에 뜬 값이다. 캡처 시점에 창이 열려 있었다면
# 반드시 넘겨라 — v2 가 코드를 보내지 않는다는 것이 이 브랜치의 핵심 주장이고,
# 코드를 넘기지 않으면 그 주장은 검사되지 않는다.

set -u

CAP="${1:-}"
CODE="${2:-}"
PEERS="$HOME/Library/Application Support/ai-agent-monitor/paired-peers.json"

if [ -z "$CAP" ] || [ ! -f "$CAP" ]; then
    echo "사용법: $0 <캡처파일> [6자리코드]" >&2
    exit 2
fi

fail=0
found() { echo "  ✗ FAIL — $1"; fail=1; }
ok()    { echo "  ✓ $1"; }

echo "캡처: $CAP ($(wc -c <"$CAP" | tr -d ' ') 바이트)"
echo

# ── 양성 대조 ───────────────────────────────────────────────────────────────
# 캡처가 실제로 무언가를 담았는가. 이게 아니면 아래는 전부 공허하다.
echo "[0] 양성 대조 — 캡처가 비어 있지 않은가"
bytes=$(wc -c <"$CAP" | tr -d ' ')
if [ "$bytes" -lt 1024 ]; then
    found "캡처가 ${bytes}바이트뿐이다. 인터페이스·필터를 확인하고 다시 떠라."
    echo
    echo "대조 실패 — 아래 검사는 돌리지 않는다. 통과했다고 읽으면 안 된다."
    exit 1
fi
ok "${bytes}바이트"

# 우리 트래픽이 잡혔는지. 봉인 프레임은 hex ASCII 로 나가므로(NDJSON),
# 최소한 연결이 있었다는 흔적은 남는다. 없으면 엉뚱한 걸 떴을 가능성이 높다.
echo "  (참고: 아래에서 하나도 안 잡히면 대상 트래픽이 캡처에 없을 수 있다)"
echo

# ── 1. 토큰 ────────────────────────────────────────────────────────────────
echo "[1] 128비트 토큰이 평문으로 나타나는가"
if [ ! -f "$PEERS" ]; then
    echo "  ? paired-peers.json 이 없다 — 페어링된 기기가 없으므로 검사 생략"
else
    tokens=$(python3 -c "
import json,sys
try:
    d=json.load(open(sys.argv[1]))
except Exception as e:
    sys.exit(0)
for p in d:
    t=p.get('token')
    if t: print(t)
" "$PEERS")
    if [ -z "$tokens" ]; then
        echo "  ? 저장된 토큰이 없다 — 검사 생략"
    else
        n=0
        while IFS= read -r tok; do
            n=$((n+1))
            # 토큰은 소문자 hex 이므로 대소문자 양쪽을 본다.
            if strings -a "$CAP" | grep -qiF "$tok"; then
                found "토큰 #$n 이 캡처에 평문으로 있다 (…${tok: -8})"
            else
                ok "토큰 #$n 없음 (…${tok: -8})"
            fi
        done <<<"$tokens"
    fi
fi
echo

# ── 2. 6자리 코드 ───────────────────────────────────────────────────────────
echo "[2] 6자리 페어링 코드가 나타나는가"
if [ -z "$CODE" ]; then
    echo "  ? 코드를 넘기지 않았다 — **이 항목은 검사되지 않았다.**"
    echo "    페어링을 캡처했다면 화면에 떴던 코드를 두 번째 인자로 넘겨라."
else
    # 6자리 숫자는 우연히도 나타날 수 있다. 그래서 히트가 나면 자동 FAIL 이
    # 아니라 사람이 봐야 할 것으로 표시한다 — 봉인 프레임은 hex ASCII 라
    # "482913" 같은 부분열이 확률적으로 섞일 수 있기 때문이다.
    hits=$(strings -a "$CAP" | grep -cF "$CODE")
    if [ "$hits" -gt 0 ]; then
        echo "  ! 코드 문자열이 ${hits}회 나타난다 — 사람이 확인해야 한다."
        echo "    봉인 프레임은 hex ASCII 라 6자리 숫자열이 우연히 섞일 수 있다."
        echo "    Auth 특성 페이로드 안에 필드로 들어 있으면 FAIL, hex 본문"
        echo "    한가운데의 우연한 부분열이면 통과다. 아래로 위치를 봐라:"
        echo "      strings -a '$CAP' | grep -nF '$CODE' | head"
    else
        ok "나타나지 않음"
    fi
fi
echo

# ── 3. 평문 스냅샷 ──────────────────────────────────────────────────────────
# MirrorSnapshot 은 { v, t, a } 순서로 직렬화되므로 평문이면 `{"v":` 로 시작한다.
# 봉인 프레임은 counter‖ciphertext‖tag 를 hex 로 인코딩한 것이라 `{` 가 없다.
echo "[3] 평문 스냅샷 JSON 이 나타나는가"
if strings -a "$CAP" | grep -qF '{"v":'; then
    found "평문 스냅샷(\`{\"v\":\`)이 캡처에 있다"
    strings -a "$CAP" | grep -oF '{"v":' | wc -l | xargs echo "     발생 횟수:"
else
    ok "평문 스냅샷 없음"
fi
echo

# ── 판정 ───────────────────────────────────────────────────────────────────
if [ "$fail" -eq 0 ]; then
    echo "판정: 평문 유출 없음."
    if [ -z "$CODE" ]; then
        echo "다만 [2] 는 검사되지 않았다 — §7-7 을 완료로 표시하지 마라."
        exit 3
    fi
    exit 0
else
    echo "판정: FAIL — 위 항목을 보라."
    exit 1
fi
