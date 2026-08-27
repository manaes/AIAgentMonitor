#!/usr/bin/env bash
# UI 라벨용 한글 서브셋 LVGL 폰트(lib/font_ko/font_ko.c)를 굽는다.
#
# 순서: ko-words.txt(사람이 관리하는 단어 목록) → gen-symbols.py(고유 한글
# 음절만 뽑아 --symbols 문자열을 만듦) → lv_font_conv(비트맵 폰트 .c 생성).
# --symbols 문자열을 손으로 옮겨 적지 않는 이유는 tools/gen-symbols.py 상단
# 주석(T13-A)에 적었다.
#
# 원본 서체는 Noto Sans KR(OFL-1.1, Google Fonts)의 가변 폰트에서 wght=400
# (Regular)만 고정 인스턴스로 뽑아 쓴다 — 이 저장소에는 macOS 시스템 폰트
# 말고는 한글을 지원하는 TTF가 없고, 시스템 폰트(AppleSDGothicNeo 등)는
# 재배포 라이선스가 없어 여기서 추출한 글립을 커밋물에 구울 수 없다. Noto Sans
# KR 은 OFL 이라 글립을 서브셋으로 구워 커밋해도 된다.
#
# 이 스크립트가 받는 것: node/npx(폰트 변환), python3(가변 폰트 → 고정
# 인스턴스, ko-words.txt 파싱). 둘 다 없으면 무엇이 없는지 바로 에러로 멈춘다.
#
# 결과물을 src/ 가 아니라 lib/font_ko/ 에 두는 이유: PlatformIO 는 src/ 밑의
# 모든 .c/.cpp 를 무조건 컴파일하는데, 이 파일은 최상단에서 무조건
# "lvgl/lvgl.h" 를 include 한다(FONT_KO 가드 밖). LVGL 은 아직 lib_deps 에
# 없다(Task 14 몫, platformio.ini 주석 참고) — src/ 에 두면 지금 당장
# `pio run` 이 "lvgl/lvgl.h: No such file" 로 깨진다(실제로 한 번 이렇게 깨져서
# 옮겼다). `lib/<name>/` 는 PlatformIO 의 chain 모드 LDF 가 실제로 #include
# 하는 것만 링크에 끌어오므로, 지금은 아무도 font_ko 를 참조하지 않아
# 빌드에서 완전히 빠진다 — authfsm/cryptov2/transportlogic 과 같은 배치다.
# Task 14 가 `extern const lv_font_t font_ko;` 로 참조하기 시작하면 자연히
# 빌드에 들어온다.
set -euo pipefail

CYD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLS_DIR="$CYD_DIR/tools"
CACHE_DIR="$TOOLS_DIR/.font-cache"      # .gitignore 처리됨 — 다운로드 캐시일 뿐
VENV_DIR="$TOOLS_DIR/.venv"             # .gitignore 처리됨 — fonttools 전용
WORDS_FILE="$TOOLS_DIR/ko-words.txt"
OUT_FILE="$CYD_DIR/lib/font_ko/font_ko.c"

FONT_SIZE=16
FONT_BUDGET_BYTES=$((50 * 1024))  # 브리프의 50KB 참고치. 넘어도 flash 예산(주석 참고)
                                    # 안에서 괜찮으면 계속 진행 — 이 스크립트는 강제로
                                    # 막지 않고 결과 크기를 알려주기만 한다.

command -v node >/dev/null 2>&1 || { echo "error: node 가 없다" >&2; exit 1; }
command -v npx  >/dev/null 2>&1 || { echo "error: npx 가 없다" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "error: python3 이 없다" >&2; exit 1; }

mkdir -p "$CACHE_DIR"

# --- 1) Noto Sans KR Regular 고정 인스턴스 준비 (없으면 만든다) ---------------
VF_TTF="$CACHE_DIR/NotoSansKR-VF.ttf"
REGULAR_TTF="$CACHE_DIR/NotoSansKR-Regular.ttf"

if [ ! -f "$REGULAR_TTF" ]; then
  echo "[build-font] Noto Sans KR Regular 고정 인스턴스가 캐시에 없다 — 만든다" >&2

  if [ ! -f "$VF_TTF" ]; then
    echo "[build-font] Noto Sans KR 가변 폰트 다운로드 (google/fonts, OFL-1.1)" >&2
    curl -sL --fail \
      "https://raw.githubusercontent.com/google/fonts/main/ofl/notosanskr/NotoSansKR%5Bwght%5D.ttf" \
      -o "$VF_TTF"
    curl -sL --fail \
      "https://raw.githubusercontent.com/google/fonts/main/ofl/notosanskr/OFL.txt" \
      -o "$CACHE_DIR/OFL.txt" || true
  fi

  if [ ! -d "$VENV_DIR" ]; then
    python3 -m venv "$VENV_DIR"
  fi
  # shellcheck source=/dev/null
  source "$VENV_DIR/bin/activate"
  python -m pip install --quiet --upgrade pip fonttools >&2

  # wght=400(Regular)만 고정해서 뽑는다 — lv_font_conv 는 가변 폰트의 축(axis)을
  # 모르므로, 안 뽑고 그대로 넘기면 어떤 두께가 나올지 보장이 없다.
  fonttools varLib.instancer -o "$REGULAR_TTF" "$VF_TTF" wght=400 >&2
  deactivate
fi

# --- 2) 단어 목록 → --symbols 문자열 ------------------------------------------
SYMBOLS="$(python3 "$TOOLS_DIR/gen-symbols.py" "$WORDS_FILE")"
echo "[build-font] 한글 음절 ${#SYMBOLS}자: $SYMBOLS" >&2

# --- 3) 폰트를 굽는다 (bpp 4 먼저, 예산 초과 시 bpp 2 로 재시도) --------------
run_font_conv() {
  local bpp="$1"
  # lv_font_conv 는 --font/-o 로 받은 경로를 그대로 생성물 맨 위 주석에 박아 넣는다.
  # 절대경로를 주면 실행한 사람 계정명까지 커밋물에 남고, 돌릴 때마다 diff가
  # 생긴다 — CYD_DIR 로 cd 한 뒤 상대경로만 넘겨서 그 주석을 재현 가능하게 만든다.
  mkdir -p "$CYD_DIR/lib/font_ko"
  (cd "$CYD_DIR" && npx --yes lv_font_conv \
    --font "tools/.font-cache/NotoSansKR-Regular.ttf" \
    --size "$FONT_SIZE" --bpp "$bpp" --format lvgl \
    --symbols "$SYMBOLS" \
    --range 0x20-0x7F \
    --lv-font-name font_ko \
    -o "lib/font_ko/font_ko.c")
}

run_font_conv 4
SIZE_BYTES=$(wc -c < "$OUT_FILE" | tr -d ' ')

if [ "$SIZE_BYTES" -gt "$FONT_BUDGET_BYTES" ]; then
  echo "[build-font] bpp=4 결과가 ${SIZE_BYTES}B로 ${FONT_BUDGET_BYTES}B 참고치를 넘는다 — bpp=2 로 다시 굽는다" >&2
  run_font_conv 2
  SIZE_BYTES=$(wc -c < "$OUT_FILE" | tr -d ' ')
fi

echo "[build-font] 완료: $OUT_FILE (${SIZE_BYTES} bytes)" >&2
