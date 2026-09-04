// AI Agent Monitor — CYD 펌웨어: 에이전트별 사용량 카드 화면 (Task 15b).
//
// Mac QuotaBar.svelte 와 동일한 색상 및 규칙을 적용한다.
#include "ui_cards.h"

#include <Arduino.h>
#include <stdio.h>
#include "snapshot.h"

// CPU 프로파일링용 스위치 — 빌드 플래그로만 켠다(코드 수정 없이
// `PLATFORMIO_BUILD_FLAGS="-D PROFILE_SKIP_BARS=1" pio run -e cyd -t upload`
// 식으로 변형마다 재빌드해서 lvglMaxUs 변화를 비교하려는 목적, 기본값은
// 전부 0(끄기)이라 아무 플래그 없이 빌드하면 원래 동작과 동일하다).
// 2026-09-01, CYD 간헐적 BLE 끊김 조사 — 더블버퍼링/SPI 클럭 다 효과
// 없어서 렌더링 자체(블렌딩/스타일 계산)가 병목이라는 가설을 검증하려고
// 위젯 종류별로 갱신을 하나씩 꺼서 lvglMaxUs 기여도를 측정한다.
#ifndef PROFILE_SKIP_BARS
#define PROFILE_SKIP_BARS 0
#endif
#ifndef PROFILE_SKIP_PCT_LABELS
#define PROFILE_SKIP_PCT_LABELS 0
#endif
#ifndef PROFILE_SKIP_RATE_LABEL
#define PROFILE_SKIP_RATE_LABEL 0
#endif
#ifndef PROFILE_SKIP_COUNTDOWN
#define PROFILE_SKIP_COUNTDOWN 0
#endif

namespace {

// 에이전트 카드 UI 위젯 묶음
struct AgentCardWidgets {
    lv_obj_t *card = nullptr;
    lv_obj_t *topRow = nullptr;
    lv_obj_t *nameLabel = nullptr;       // 14px, #8e8e93
    lv_obj_t *rateLabel = nullptr;       // 14px, #0a84ff

    // 5시간 쿼터 위젯
    lv_obj_t *row5h = nullptr;           // flex row (space-between)
    lv_obj_t *info5hLabel = nullptr;     // 14px, #8e8e93 (예: "5h (4h 55m)")
    lv_obj_t *pct5hLabel = nullptr;      // 16px, getPctColor (예: "0%")
    lv_obj_t *bar5h = nullptr;           // 높이 6px 게이지 바

    // 주간 쿼터 위젯
    lv_obj_t *rowWk = nullptr;           // flex row (space-between)
    lv_obj_t *infoWkLabel = nullptr;     // 14px, #8e8e93 (예: "Week (2d 8h)")
    lv_obj_t *pctWkLabel = nullptr;      // 16px, getPctColor (예: "100%")
    lv_obj_t *barWk = nullptr;           // 높이 6px 게이지 바
};

lv_obj_t *g_cardsRoot = nullptr;
lv_obj_t *g_cardsContainer = nullptr;
lv_obj_t *g_noDataLabel = nullptr;
AgentCardWidgets g_cards[SNAPSHOT_MAX_AGENTS];

// 쿼터 바 및 텍스트 색상 계산 (70% / 90% 임계값)
lv_color_t getPctColor(float pct) {
    if (pct >= 90.0f) {
        return lv_color_hex(0xff453a);  // 빨강
    } else if (pct >= 70.0f) {
        return lv_color_hex(0xff9f0a);  // 주황
    } else {
        return lv_color_hex(0x30d158);  // 초록
    }
}

// 에이전트 이름 문자열 변환
const char *getAgentName(SnapshotAgentKind kind) {
    switch (kind) {
        case SnapshotAgentKind::Claude: return "Claude";
        case SnapshotAgentKind::Codex: return "Codex";
        case SnapshotAgentKind::Antigravity: return "Antigravity";
        default: return "Agent";
    }
}

// 리셋 카운트다운 간소화 포맷팅 (예: "4h 55m", "2d 8h", "17m 55s")
void formatResetCountdown(uint64_t resetEpochSec, uint64_t currentEpochSec, char *buf, size_t bufSize) {
    if (resetEpochSec <= currentEpochSec) {
        snprintf(buf, bufSize, "0s");
        return;
    }
    uint64_t diff = resetEpochSec - currentEpochSec;
    uint32_t days = (uint32_t)(diff / 86400);
    uint32_t hours = (uint32_t)((diff % 86400) / 3600);
    uint32_t mins = (uint32_t)((diff % 3600) / 60);
    uint32_t secs = (uint32_t)(diff % 60);
    if (days > 0) {
        snprintf(buf, bufSize, "%ud %uh", (unsigned)days, (unsigned)hours);
    } else if (hours > 0) {
        snprintf(buf, bufSize, "%uh %um", (unsigned)hours, (unsigned)mins);
    } else if (mins > 0) {
        snprintf(buf, bufSize, "%um %us", (unsigned)mins, (unsigned)secs);
    } else {
        snprintf(buf, bufSize, "%us", (unsigned)secs);
    }
}

// tok/s 속도 단축 포맷팅 (예: 0 -> "0 tok/s", 45 -> "45 tok/s", 2100 -> "2.1k tok/s", 1500000 -> "1.5M tok/s")
void formatTokensPerSec(float v, char *buf, size_t bufSize) {
    if (v < 1.0f) {
        snprintf(buf, bufSize, "0 tok/s");
    } else if (v < 1000.0f) {
        snprintf(buf, bufSize, "%.0f tok/s", v);
    } else if (v < 1000000.0f) {
        snprintf(buf, bufSize, "%.1fk tok/s", v / 1000.0f);
    } else {
        snprintf(buf, bufSize, "%.1fM tok/s", v / 1000000.0f);
    }
}

}  // namespace

lv_obj_t *uiCardsCreate(lv_obj_t *parent) {
    if (parent == nullptr) {
        parent = lv_screen_active();
    }
    lv_obj_set_style_bg_color(lv_screen_active(), lv_color_hex(0x000000), 0);
    lv_obj_set_style_bg_opa(lv_screen_active(), LV_OPA_COVER, 0);

    g_cardsRoot = lv_obj_create(parent);
    lv_obj_set_size(g_cardsRoot, lv_pct(100), lv_pct(100));
    lv_obj_set_pos(g_cardsRoot, 0, 0);
    lv_obj_set_style_radius(g_cardsRoot, 0, 0);
    lv_obj_set_style_bg_color(g_cardsRoot, lv_color_hex(0x000000), 0);
    lv_obj_set_style_bg_opa(g_cardsRoot, LV_OPA_COVER, 0);
    lv_obj_set_style_border_width(g_cardsRoot, 0, 0);
    lv_obj_set_style_pad_top(g_cardsRoot, 4, 0);
    lv_obj_set_style_pad_bottom(g_cardsRoot, 4, 0);
    lv_obj_set_style_pad_left(g_cardsRoot, 10, 0);
    lv_obj_set_style_pad_right(g_cardsRoot, 10, 0);
    lv_obj_set_scrollbar_mode(g_cardsRoot, LV_SCROLLBAR_MODE_OFF);
    lv_obj_remove_flag(g_cardsRoot, LV_OBJ_FLAG_SCROLLABLE);

    g_cardsContainer = lv_obj_create(g_cardsRoot);
    lv_obj_set_size(g_cardsContainer, lv_pct(100), lv_pct(100));
    lv_obj_set_flex_flow(g_cardsContainer, LV_FLEX_FLOW_COLUMN);
    lv_obj_set_style_bg_opa(g_cardsContainer, LV_OPA_TRANSP, 0);
    lv_obj_set_style_border_width(g_cardsContainer, 0, 0);
    lv_obj_set_style_pad_all(g_cardsContainer, 0, 0);
    lv_obj_set_style_pad_row(g_cardsContainer, 0, 0);
    lv_obj_set_scrollbar_mode(g_cardsContainer, LV_SCROLLBAR_MODE_OFF);
    lv_obj_remove_flag(g_cardsContainer, LV_OBJ_FLAG_SCROLLABLE);

    g_noDataLabel = lv_label_create(g_cardsRoot);
    lv_obj_set_style_text_font(g_noDataLabel, &lv_font_montserrat_16, 0);
    lv_obj_set_style_text_color(g_noDataLabel, lv_color_hex(0x8e8e93), 0);
    lv_label_set_text(g_noDataLabel, "Waiting for data...");
    lv_obj_center(g_noDataLabel);

    // 에이전트 카드 위젯 사전 생성 (단일 블랙 배경 + 밝은 1px 하단 라인)
    for (size_t i = 0; i < SNAPSHOT_MAX_AGENTS; i++) {
        AgentCardWidgets &cw = g_cards[i];

        cw.card = lv_obj_create(g_cardsContainer);
        lv_obj_set_width(cw.card, lv_pct(100));
        lv_obj_set_height(cw.card, LV_SIZE_CONTENT);
        lv_obj_set_style_bg_opa(cw.card, LV_OPA_TRANSP, 0);
        lv_obj_set_style_radius(cw.card, 0, 0);
        lv_obj_set_style_border_side(cw.card, LV_BORDER_SIDE_BOTTOM, 0);
        lv_obj_set_style_border_width(cw.card, 1, 0);
        lv_obj_set_style_border_color(cw.card, lv_color_hex(0x636366), 0);
        lv_obj_set_style_border_opa(cw.card, LV_OPA_COVER, 0);
        lv_obj_set_style_pad_top(cw.card, 8, 0);
        lv_obj_set_style_pad_bottom(cw.card, 9, 0);
        lv_obj_set_style_pad_left(cw.card, 0, 0);
        lv_obj_set_style_pad_right(cw.card, 0, 0);
        lv_obj_set_flex_flow(cw.card, LV_FLEX_FLOW_COLUMN);
        lv_obj_set_style_pad_row(cw.card, 3, 0);

        // 1행: 헤더 (이름 14px 회색 + 속도 14px 파랑)
        cw.topRow = lv_obj_create(cw.card);
        lv_obj_set_size(cw.topRow, lv_pct(100), LV_SIZE_CONTENT);
        lv_obj_set_style_bg_opa(cw.topRow, LV_OPA_TRANSP, 0);
        lv_obj_set_style_border_width(cw.topRow, 0, 0);
        lv_obj_set_style_pad_all(cw.topRow, 0, 0);
        lv_obj_set_flex_flow(cw.topRow, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(cw.topRow, LV_FLEX_ALIGN_SPACE_BETWEEN, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);

        cw.nameLabel = lv_label_create(cw.topRow);
        lv_obj_set_style_text_font(cw.nameLabel, &lv_font_montserrat_14, 0);
        lv_obj_set_style_text_color(cw.nameLabel, lv_color_hex(0x8e8e93), 0);
        lv_label_set_text(cw.nameLabel, "");

        cw.rateLabel = lv_label_create(cw.topRow);
        lv_obj_set_style_text_font(cw.rateLabel, &lv_font_montserrat_14, 0);
        lv_obj_set_style_text_color(cw.rateLabel, lv_color_hex(0x0a84ff), 0);
        lv_label_set_text(cw.rateLabel, "");

        // 2행: 5시간 쿼터 텍스트행 (좌: 5h 남은시간, 우: 사용량 %)
        cw.row5h = lv_obj_create(cw.card);
        lv_obj_set_size(cw.row5h, lv_pct(100), LV_SIZE_CONTENT);
        lv_obj_set_style_bg_opa(cw.row5h, LV_OPA_TRANSP, 0);
        lv_obj_set_style_border_width(cw.row5h, 0, 0);
        lv_obj_set_style_pad_all(cw.row5h, 0, 0);
        lv_obj_set_style_pad_top(cw.row5h, 6, 0);
        lv_obj_set_flex_flow(cw.row5h, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(cw.row5h, LV_FLEX_ALIGN_SPACE_BETWEEN, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);

        cw.info5hLabel = lv_label_create(cw.row5h);
        lv_obj_set_style_text_font(cw.info5hLabel, &lv_font_montserrat_14, 0);
        lv_obj_set_style_text_color(cw.info5hLabel, lv_color_hex(0x8e8e93), 0);
        lv_label_set_text(cw.info5hLabel, "");

        cw.pct5hLabel = lv_label_create(cw.row5h);
        lv_obj_set_style_text_font(cw.pct5hLabel, &lv_font_montserrat_16, 0);
        lv_label_set_text(cw.pct5hLabel, "");

        // 5시간 쿼터 바 (높이 6px, 0%여도 최소 1% 및 트랙 노출)
        cw.bar5h = lv_bar_create(cw.card);
        lv_obj_set_size(cw.bar5h, lv_pct(100), 6);
        lv_obj_set_style_bg_color(cw.bar5h, lv_color_hex(0x3a3a3c), 0);
        lv_obj_set_style_radius(cw.bar5h, 3, 0);
        lv_bar_set_range(cw.bar5h, 0, 100);

        // 3행: 주간 쿼터 텍스트행 (좌: Week 남은시간, 우: 사용량 %)
        cw.rowWk = lv_obj_create(cw.card);
        lv_obj_set_size(cw.rowWk, lv_pct(100), LV_SIZE_CONTENT);
        lv_obj_set_style_bg_opa(cw.rowWk, LV_OPA_TRANSP, 0);
        lv_obj_set_style_border_width(cw.rowWk, 0, 0);
        lv_obj_set_style_pad_all(cw.rowWk, 0, 0);
        lv_obj_set_style_pad_top(cw.rowWk, 2, 0);
        lv_obj_set_flex_flow(cw.rowWk, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(cw.rowWk, LV_FLEX_ALIGN_SPACE_BETWEEN, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);

        cw.infoWkLabel = lv_label_create(cw.rowWk);
        lv_obj_set_style_text_font(cw.infoWkLabel, &lv_font_montserrat_14, 0);
        lv_obj_set_style_text_color(cw.infoWkLabel, lv_color_hex(0x8e8e93), 0);
        lv_label_set_text(cw.infoWkLabel, "");

        cw.pctWkLabel = lv_label_create(cw.rowWk);
        lv_obj_set_style_text_font(cw.pctWkLabel, &lv_font_montserrat_16, 0);
        lv_label_set_text(cw.pctWkLabel, "");

        // 주간 쿼터 바 (높이 6px)
        cw.barWk = lv_bar_create(cw.card);
        lv_obj_set_size(cw.barWk, lv_pct(100), 6);
        lv_obj_set_style_bg_color(cw.barWk, lv_color_hex(0x3a3a3c), 0);
        lv_obj_set_style_radius(cw.barWk, 3, 0);
        lv_bar_set_range(cw.barWk, 0, 100);

        lv_obj_add_flag(cw.card, LV_OBJ_FLAG_HIDDEN);
    }

    return g_cardsRoot;
}

void uiCardsUpdate(const Transport &transport, size_t agentIndexToUpdate) {
    if (!transport.hasSnapshot()) {
        lv_obj_clear_flag(g_noDataLabel, LV_OBJ_FLAG_HIDDEN);
        for (size_t i = 0; i < SNAPSHOT_MAX_AGENTS; i++) {
            lv_obj_add_flag(g_cards[i].card, LV_OBJ_FLAG_HIDDEN);
        }
        return;
    }

    lv_obj_add_flag(g_noDataLabel, LV_OBJ_FLAG_HIDDEN);
    const Snapshot &snap = transport.latestSnapshot();
    uint64_t currentEpochSec = snap.emittedAtEpochSec;

    // 카드 표시/숨김은 매번 전부 훑는다 — 값이 그대로면 LVGL이 무시해서
    // 싸다(실측 확인, PROFILE_SKIP_* 프로파일링 때 남은 바닥값 ~19ms에
    // 이미 포함돼 있던 비용). 실제로 무거운 아래 본문(이름/퍼센트/바/
    // 카운트다운)만 카드 하나씩 순환하며 갱신한다 — 3카드를 한 loop()
    // 에서 동시에 건드리면 LVGL이 dirty 영역을 화면 세로 전체로 합쳐
    // 풀스크린급 플러시(~130ms대)를 유발한다는 게 2026-09-01 프로파일링
    // (PROFILE_SKIP_BARS/PCT_LABELS/RATE_LABEL/COUNTDOWN)으로 확인됐다.
    for (size_t i = 0; i < SNAPSHOT_MAX_AGENTS; i++) {
        if (i >= snap.agentCount) {
            lv_obj_add_flag(g_cards[i].card, LV_OBJ_FLAG_HIDDEN);
        } else {
            lv_obj_clear_flag(g_cards[i].card, LV_OBJ_FLAG_HIDDEN);
        }
    }

    if (agentIndexToUpdate < snap.agentCount) {
        AgentCardWidgets &cw = g_cards[agentIndexToUpdate];
        const SnapshotAgent &ag = snap.agents[agentIndexToUpdate];

        // 이름
        lv_label_set_text(cw.nameLabel, getAgentName(ag.kind));

#if !PROFILE_SKIP_RATE_LABEL
        // tok/s 속도 (k/M 단축 표기 적용)
        char rateBuf[32];
        formatTokensPerSec(ag.rateTokPerSec, rateBuf, sizeof(rateBuf));
        lv_label_set_text(cw.rateLabel, rateBuf);
#endif

        // 사용량을 지금 못 읽고 있으면 %·막대를 통째로 감추고 이유만 남긴다.
        //
        // 맥은 실패 중에도 마지막 %를 함께 보내준다(wire.rs `e` 주석) — 이 키를
        // 모르는 구버전 펌웨어가 "값 없음 → 0%"(아래 pct5h 폴백)로 그리는 것보다
        // 낡은 값이라도 보이는 편이 덜 틀리기 때문이지, 그 숫자가 지금 유효해서가
        // 아니다. 이 키를 아는 우리는 아예 가린다.
        //
        // 이 블록이 이 함수의 마지막이라 early return 이 안전하다 — 뒤에 정리할
        // 코드가 없다.
        if (ag.hasQuotaError) {
            lv_label_set_text(cw.info5hLabel, snapshotQuotaErrorText(ag.quotaErrorCode));
            lv_obj_set_style_text_color(cw.info5hLabel, lv_color_hex(0xff9f0a), 0);  // 주황
            lv_obj_clear_flag(cw.row5h, LV_OBJ_FLAG_HIDDEN);
            lv_label_set_text(cw.pct5hLabel, "");
            lv_obj_add_flag(cw.bar5h, LV_OBJ_FLAG_HIDDEN);
            lv_obj_add_flag(cw.rowWk, LV_OBJ_FLAG_HIDDEN);
            lv_obj_add_flag(cw.barWk, LV_OBJ_FLAG_HIDDEN);
            return;
        }
        // 정상 복귀 시 위에서 바꾼 색을 되돌린다 — 안 그러면 한 번 실패한 카드의
        // "5h (…)" 라벨이 계속 주황으로 남는다.
        lv_obj_set_style_text_color(cw.info5hLabel, lv_color_hex(0x8e8e93), 0);

        // 5시간 쿼터 (사용량 표시)
        float pct5h = ag.has5hUsagePct ? ag.usage5hPct : 0.0f;
        if (pct5h > 100.0f) pct5h = 100.0f;
        if (pct5h < 0.0f) pct5h = 0.0f;

        char resetBuf[64] = "";
        if (ag.has5hResetAt) {
            formatResetCountdown(ag.reset5hEpochSec, currentEpochSec, resetBuf, sizeof(resetBuf));
        }

        char info5hBuf[64];
        if (resetBuf[0] != '\0') {
            snprintf(info5hBuf, sizeof(info5hBuf), "5h (%s)", resetBuf);
        } else {
            snprintf(info5hBuf, sizeof(info5hBuf), "5h");
        }
#if !PROFILE_SKIP_COUNTDOWN
        lv_label_set_text(cw.info5hLabel, info5hBuf);
#endif

#if !PROFILE_SKIP_PCT_LABELS
        char pct5hBuf[32];
        snprintf(pct5hBuf, sizeof(pct5hBuf), "%.0f%%", pct5h);
        lv_label_set_text(cw.pct5hLabel, pct5hBuf);
        lv_obj_set_style_text_color(cw.pct5hLabel, getPctColor(pct5h), 0);
#endif

#if !PROFILE_SKIP_BARS
        // 0%여도 그래프(바)는 최소 1%로 항상 표시
        int32_t bar5hVal = (int32_t)pct5h;
        if (bar5hVal < 1) bar5hVal = 1;
        lv_bar_set_value(cw.bar5h, bar5hVal, LV_ANIM_OFF);
        lv_obj_set_style_bg_color(cw.bar5h, getPctColor(pct5h), LV_PART_INDICATOR);
        lv_obj_clear_flag(cw.row5h, LV_OBJ_FLAG_HIDDEN);
        lv_obj_clear_flag(cw.bar5h, LV_OBJ_FLAG_HIDDEN);
#endif

        // 주간 쿼터 (사용량 표시, Week)
        if (ag.hasWeeklyUsagePct) {
            float pctWk = ag.usageWeeklyPct;
            if (pctWk > 100.0f) pctWk = 100.0f;
            if (pctWk < 0.0f) pctWk = 0.0f;

            char resetWkBuf[64] = "";
            if (ag.hasWeeklyResetAt) {
                formatResetCountdown(ag.resetWeeklyEpochSec, currentEpochSec, resetWkBuf, sizeof(resetWkBuf));
            }

            char infoWkBuf[64];
            if (resetWkBuf[0] != '\0') {
                snprintf(infoWkBuf, sizeof(infoWkBuf), "Week (%s)", resetWkBuf);
            } else {
                snprintf(infoWkBuf, sizeof(infoWkBuf), "Week");
            }
#if !PROFILE_SKIP_COUNTDOWN
            lv_label_set_text(cw.infoWkLabel, infoWkBuf);
#endif

#if !PROFILE_SKIP_PCT_LABELS
            char pctWkBuf[32];
            snprintf(pctWkBuf, sizeof(pctWkBuf), "%.0f%%", pctWk);
            lv_label_set_text(cw.pctWkLabel, pctWkBuf);
            lv_obj_set_style_text_color(cw.pctWkLabel, getPctColor(pctWk), 0);
#endif

#if !PROFILE_SKIP_BARS
            // 주간 쿼터 바 항상 노출 (최소 1%)
            int32_t barWkVal = (int32_t)pctWk;
            if (barWkVal < 1) barWkVal = 1;
            lv_bar_set_value(cw.barWk, barWkVal, LV_ANIM_OFF);
            lv_obj_set_style_bg_color(cw.barWk, getPctColor(pctWk), LV_PART_INDICATOR);
            lv_obj_clear_flag(cw.rowWk, LV_OBJ_FLAG_HIDDEN);
            lv_obj_clear_flag(cw.barWk, LV_OBJ_FLAG_HIDDEN);
#endif
        } else {
            lv_obj_add_flag(cw.rowWk, LV_OBJ_FLAG_HIDDEN);
            lv_obj_add_flag(cw.barWk, LV_OBJ_FLAG_HIDDEN);
        }
    }
}

void uiCardsUpdateRates(const Transport &transport) {
#if PROFILE_SKIP_RATE_LABEL
    (void)transport;
    return;
#else
    if (!transport.hasSnapshot()) {
        return;
    }
    const Snapshot &snap = transport.latestSnapshot();
    for (size_t i = 0; i < SNAPSHOT_MAX_AGENTS && i < snap.agentCount; i++) {
        AgentCardWidgets &cw = g_cards[i];
        if (lv_obj_has_flag(cw.card, LV_OBJ_FLAG_HIDDEN)) {
            continue;
        }
        char rateBuf[32];
        formatTokensPerSec(snap.agents[i].rateTokPerSec, rateBuf, sizeof(rateBuf));
        lv_label_set_text(cw.rateLabel, rateBuf);
    }
#endif
}

void uiCardsSetVisible(bool visible) {
    if (g_cardsRoot != nullptr) {
        if (visible) {
            lv_obj_clear_flag(g_cardsRoot, LV_OBJ_FLAG_HIDDEN);
        } else {
            lv_obj_add_flag(g_cardsRoot, LV_OBJ_FLAG_HIDDEN);
        }
    }
}
