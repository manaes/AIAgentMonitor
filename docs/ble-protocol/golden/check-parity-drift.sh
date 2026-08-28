#!/usr/bin/env bash
#
# src/lib/format.ts 와 이 디렉토리의 골든 표(format-parity.json)가 여전히 맞는지 확인한다.
#
# ── 무엇을 검사하나 ────────────────────────────────────────────────────────
#   [1] format.ts 의 formatTokensPerSec / formatTokensTotal 이
#       generate-format-parity.mjs 안의 **복제본**과 실질적으로 같은가
#   [2] format.ts 의 relativeTime 이 이 스크립트에 고정해 둔 사본과 같은가
#       (생성기는 relativeTime 을 복제하지 않는다 — 골든 표에 들어가지 않는 함수라서다.
#        하지만 Swift 로는 포팅돼 있으므로(MirrorFormat.relativeTime) 바뀌면 알아야 한다)
#   [3] 생성기를 다시 돌린 결과가 체크인된 format-parity.json 과 같은가
#
# ── 무엇을 검사하지 **않나** (정직하게 적어 둔다) ──────────────────────────
#   * Swift 구현(MirrorFormat)이 JS 와 같은 답을 내는지 — 그건 MirrorFormatTests 가
#     골든 표로 검증한다. 이 스크립트는 그 **표가 최신인지**만 본다.
#   * format.ts 의 formatResetClock — Swift 로 포팅되지 않아 미러에 나가지 않는다.
#   * 공백/주석/줄바꿈만 다른 변경 — 일부러 정규화해서 무시한다("실질적으로 같은가").
#
# ── 왜 필요한가 ────────────────────────────────────────────────────────────
# 생성기는 format.ts 를 import 하지 않고 **복제**한다(생성기는 독립 실행 Node 스크립트라
# TypeScript 원본을 못 읽는다). 그래서 format.ts 가 바뀌고 아무도 생성기를 고치지 않으면
# 표가 낡은 채로 남는다. 이때 MirrorFormatTests 는 아무 도움이 되지 않는다 — 그 테스트는
# Swift 를 **체크인된 JSON** 과 대조하므로 낡은 기준으로 그대로 통과한다.
# [3] 만으로도 부족하다: [3] 은 생성기 복제본과 표만 보므로 format.ts 쪽 변경을 못 본다.
# **[1]+[2] 가 format.ts 를 실제로 읽는 유일한 부분이고, 이 스크립트의 핵심이다.**
#
# ── 언제 돌리나 ────────────────────────────────────────────────────────────
#   * src/lib/format.ts 를 고칠 때마다 (필수)
#   * generate-format-parity.mjs 안의 복제본을 고칠 때마다 (필수)
#   * format-parity.json 을 근거로 뭔가를 판단하기 전 (권장)
#
# ── 실행 ───────────────────────────────────────────────────────────────────
#   docs/ble-protocol/golden/check-parity-drift.sh
#
# 종료 코드: 0 = 이상 없음 / 1 = 드리프트 / 2 = 검사 자체를 못 함(생성기 실행 실패 등)
set -euo pipefail

cd "$(dirname "$0")"

format_ts="../../../src/lib/format.ts"
generator="generate-format-parity.mjs"
json="format-parity.json"

for f in "$format_ts" "$generator" "$json"; do
  [ -f "$f" ] || { echo "✗ 파일이 없다: $f" >&2; exit 2; }
done

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# ---------------------------------------------------------------------------
# 함수 하나를 뽑아 "실질" 형태로 정규화하는 도우미.
#
# 문자열/템플릿 리터럴/주석을 인식하는 간이 토크나이저로 함수 본문의 닫는 중괄호를
# 찾는다(`${...}` 안의 } 때문에 단순 카운팅은 안 된다). 그 뒤
#   - 주석 제거
#   - `export ` 제거
#   - TS 타입 표기 제거 (매개변수 `v: number`, 반환형 `): string`)
#   - 숫자 구분자 제거 (1_000_000 → 1000000)
#   - 모든 공백을 하나로 접기
# 를 거쳐 한 줄로 만든다. 즉 서식·주석 차이는 무시하고 **코드가 바뀌었을 때만** 다르다.
# ---------------------------------------------------------------------------
cat > "$tmp/extract.mjs" <<'JSEOF'
import { readFileSync } from "node:fs";

const [file, name] = process.argv.slice(2);
const src = readFileSync(file, "utf8");

const sig = new RegExp(`(?:^|\\n)[ \\t]*(?:export[ \\t]+)?function[ \\t]+${name}[ \\t]*\\(`);
const m = sig.exec(src);
if (!m) {
  process.stderr.write(`함수를 찾지 못했다: ${name} (${file})\n`);
  process.exit(3);
}
const start = m.index + (src[m.index] === "\n" ? 1 : 0);

// 본문 여는 중괄호를 찾고, 문자열/템플릿/주석을 건너뛰며 짝을 맞춘다.
let i = src.indexOf("{", m.index + m[0].length - 1);
if (i < 0) { process.stderr.write(`본문을 찾지 못했다: ${name}\n`); process.exit(3); }

let depth = 0;
const tmplStack = [];   // 템플릿 리터럴 안의 ${ } 중첩 추적
let end = -1;
for (; i < src.length; i++) {
  const c = src[i], n2 = src[i + 1];
  if (c === "/" && n2 === "/") { while (i < src.length && src[i] !== "\n") i++; continue; }
  if (c === "/" && n2 === "*") { i = src.indexOf("*/", i + 2) + 1; continue; }
  if (c === '"' || c === "'") {
    const q = c; i++;
    while (i < src.length && src[i] !== q) { if (src[i] === "\\") i++; i++; }
    continue;
  }
  if (c === "`") {
    // 템플릿 리터럴: ${ 를 만나면 다시 코드 모드로 돌아간다.
    i++;
    while (i < src.length) {
      if (src[i] === "\\") { i += 2; continue; }
      if (src[i] === "`") break;
      if (src[i] === "$" && src[i + 1] === "{") { tmplStack.push(depth); depth++; i += 2; break; }
      i++;
    }
    continue;
  }
  if (c === "{") { depth++; continue; }
  if (c === "}") {
    depth--;
    if (tmplStack.length && tmplStack[tmplStack.length - 1] === depth) {
      // ${ } 를 닫았으니 템플릿 문자열 모드로 복귀
      tmplStack.pop();
      i++;
      while (i < src.length) {
        if (src[i] === "\\") { i += 2; continue; }
        if (src[i] === "`") break;
        if (src[i] === "$" && src[i + 1] === "{") { tmplStack.push(depth); depth++; i += 1; break; }
        i++;
      }
      continue;
    }
    if (depth === 0) { end = i; break; }
  }
}
if (end < 0) { process.stderr.write(`닫는 중괄호를 찾지 못했다: ${name}\n`); process.exit(3); }

let text = src.slice(start, end + 1);

text = text
  .replace(/\/\*[\s\S]*?\*\//g, " ")            // 블록 주석
  .replace(/(^|[^:])\/\/[^\n]*/g, "$1")          // 줄 주석
  .replace(/^\s*export\s+/, "")                  // export
  .replace(/(\d)_(?=\d)/g, "$1");                // 1_000_000 → 1000000

// TS 타입 표기 제거: 시그니처의 매개변수와 반환형만 손댄다.
const head = text.indexOf("(");
const bodyAt = text.indexOf("{", head);
let signature = text.slice(0, bodyAt);
const body = text.slice(bodyAt);
const params = signature.slice(head + 1, signature.lastIndexOf(")"));
const bare = params
  .split(",")
  .map((p) => p.split(":")[0].trim())
  .filter((p) => p.length > 0)
  .join(", ");
signature = signature.slice(0, head) + "(" + bare + ") ";

process.stdout.write((signature + body).replace(/\s+/g, " ").trim() + "\n");
JSEOF

extract() { node "$tmp/extract.mjs" "$1" "$2"; }

fail=0

# ---------------------------------------------------------------------------
# [1] format.ts ↔ 생성기 복제본
# ---------------------------------------------------------------------------
echo "[1] format.ts ↔ 생성기 복제본"
for fn in formatTokensPerSec formatTokensTotal; do
  a="$(extract "$format_ts" "$fn")"
  b="$(extract "$generator" "$fn")"
  if [ "$a" = "$b" ]; then
    echo "    ✓ $fn"
  else
    fail=1
    echo "    ✗ $fn — format.ts 와 생성기 복제본이 다르다" >&2
    echo "      format.ts : $a" >&2
    echo "      생성기    : $b" >&2
  fi
done

# ---------------------------------------------------------------------------
# [2] format.ts ↔ 여기 고정해 둔 사본 (생성기에 복제본이 없는 함수)
#
# relativeTime 은 골든 표에 들어가지 않아 생성기가 복제하지 않는다. 그래도 Swift 로
# 포팅돼 화면에 나가므로(MirrorFormat.relativeTime), 바뀌면 Swift 쪽도 같이 고쳐야 한다.
# 대조할 복제본이 없으니 정규화된 본문을 여기에 직접 고정한다.
# 이 값이 틀렸다고 실패하면: (a) Swift 포팅을 같이 고쳤는지 확인하고
# (b) `node extract.mjs` 가 출력한 새 문자열로 아래 값을 갱신한다.
# ---------------------------------------------------------------------------
relative_time_pin='function relativeTime(secsSinceEpoch) { const elapsed = Math.floor(Date.now() / 1000) - secsSinceEpoch; if (elapsed < 5) return "방금 전"; if (elapsed < 60) return `${elapsed}초 전`; if (elapsed < 3600) return `${Math.floor(elapsed / 60)}분 전`; return `${Math.floor(elapsed / 3600)}시간 전`; }'

echo "[2] format.ts ↔ 고정 사본 (생성기가 복제하지 않는 함수)"
actual="$(extract "$format_ts" relativeTime)"
if [ "$actual" = "$relative_time_pin" ]; then
  echo "    ✓ relativeTime"
else
  fail=1
  echo "    ✗ relativeTime — format.ts 가 바뀌었다. Swift 의 MirrorFormat.relativeTime 도 같이 고쳐야 한다." >&2
  echo "      고정값  : $relative_time_pin" >&2
  echo "      현재값  : $actual" >&2
fi

# ---------------------------------------------------------------------------
# [3] 골든 표 신선도
#
# 임시 파일에 먼저 만들고 성공했을 때만 제자리로 옮긴다. `node gen > json` 로 바로 쓰면
# 리다이렉션이 node 실행 **전에** json 을 0바이트로 잘라버려, 생성기가 죽었을 때
# 골든 표가 빈 파일로 남고 그 실패가 진짜 드리프트와 구분되지 않는다.
# ---------------------------------------------------------------------------
echo "[3] 골든 표 신선도"
if ! node "$generator" > "$tmp/regen.json"; then
  echo "    ✗ 생성기 실행이 실패했다 — $json 은 손대지 않았다." >&2
  exit 2
fi
if [ ! -s "$tmp/regen.json" ]; then
  echo "    ✗ 생성기가 빈 출력을 냈다 — $json 은 손대지 않았다." >&2
  exit 2
fi

if cmp -s "$tmp/regen.json" "$json"; then
  echo "    ✓ $json 이 생성기의 현재 출력과 일치한다"
  # 표가 최신이어도 아직 커밋 전일 수 있다. 실패는 아니고 안내만 한다.
  if git rev-parse --git-dir >/dev/null 2>&1 && ! git diff --quiet -- "$json" 2>/dev/null; then
    echo "    ! $json 에 미커밋 변경이 있다 — 함께 커밋할 것."
  fi
else
  fail=1
  cp "$tmp/regen.json" "$json"
  echo "    ✗ $json 이 낡았다 — 방금 다시 만들어 두었으니 확인 후 커밋할 것." >&2
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "✓ 드리프트 없음 — format.ts · 생성기 복제본 · 골든 표가 모두 일치한다."
  exit 0
fi

cat >&2 <<'MSG'
✗ 드리프트 발견. 순서대로 확인한다:

  1) [1]/[2] 가 실패했다면 → format.ts 가 바뀌었는데 따라오지 않은 곳이 있다.
     - generate-format-parity.mjs 의 복제본을 format.ts 와 같게 고친다
     - Swift 의 MirrorFormat 도 같은 규칙인지 확인한다
     - relativeTime 이라면 이 스크립트의 relative_time_pin 도 갱신한다
  2) 그 뒤 이 스크립트를 다시 돌려 [3] 이 표를 갱신하게 한다
  3) git diff docs/ble-protocol/golden/format-parity.json 이 의도한 변화인지 보고
     MirrorFormatTests 를 돌린 뒤 함께 커밋한다
MSG
exit 1
