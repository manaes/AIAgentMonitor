#!/usr/bin/env bash
#
# 골든 표(format-parity.json)가 지금의 src/lib/format.ts 와 맞는지 확인한다.
#
# ── 왜 필요한가 ────────────────────────────────────────────────────────────
# generate-format-parity.mjs 는 format.ts 를 import 하지 않고 **복제**한다.
# 그래서 format.ts 가 바뀌고 아무도 생성기를 다시 돌리지 않으면 골든 표가 낡은
# 채로 남는다. 이때 MirrorFormatTests 는 아무 도움이 되지 않는다 — 그 테스트는
# Swift 를 **체크인된 JSON** 과 대조하므로, JSON 이 낡아 있으면 낡은 기준으로
# 통과한다("패리티 테스트가 드리프트를 잡는다"는 서술은 성립하지 않는다).
# 실제 안전장치는 이 스크립트뿐이다: 생성기를 다시 돌려 결과가 커밋된 것과
# 다르면 실패시킨다.
#
# ── 언제 돌리나 ────────────────────────────────────────────────────────────
# * src/lib/format.ts 를 고칠 때마다 (필수)
# * generate-format-parity.mjs 안의 복제본을 고칠 때마다 (필수)
# * format-parity.json 을 근거로 뭔가를 판단하기 전 (권장)
#
# ── 실행 ───────────────────────────────────────────────────────────────────
#   docs/ble-protocol/golden/check-parity-drift.sh
#
# 종료 코드: 0 = 일치, 1 = 드리프트(파일이 갱신된 채로 남는다 → 커밋할 것),
#            2 = 확인 불가(미리 존재하던 미커밋 변경 등)
set -euo pipefail

cd "$(dirname "$0")"
json="format-parity.json"

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "✗ git 저장소 안에서 실행해야 한다 (커밋본과 대조해야 하므로)." >&2
  exit 2
fi

# 이미 미커밋 변경이 있으면 이 스크립트의 판정이 흐려진다 — 그 변경이 드리프트
# 때문인지 사람이 손댄 것인지 구분할 수 없으므로 실행하지 않는다.
if ! git diff --quiet -- "$json" || ! git diff --cached --quiet -- "$json"; then
  echo "✗ $json 에 이미 미커밋 변경이 있다. 커밋하거나 되돌린 뒤 다시 실행할 것." >&2
  exit 2
fi

node generate-format-parity.mjs > "$json"

if git diff --quiet -- "$json"; then
  echo "✓ 드리프트 없음 — $json 이 지금의 format.ts 복제본과 일치한다."
  exit 0
fi

cat >&2 <<'MSG'
✗ 드리프트 발견 — 체크인된 format-parity.json 이 생성기의 현재 출력과 다르다.

  format.ts(또는 생성기 안의 복제본)가 바뀐 뒤 표가 갱신되지 않았다는 뜻이다.
  이 스크립트가 방금 표를 다시 만들어 두었으니, 아래를 확인한 뒤 함께 커밋한다:

    1) generate-format-parity.mjs 의 복제본이 src/lib/format.ts 와 글자 단위로 같은가
    2) git diff docs/ble-protocol/golden/format-parity.json 이 의도한 변화인가
    3) Swift 쪽 MirrorFormat 도 같이 고쳐야 하는가 (MirrorFormatTests 로 확인)
MSG
exit 1
