// AI Agent Monitor — CYD 펌웨어: 에이전트별 사용량 카드 화면 (Task 15b).
//
// Mac QuotaBar.svelte 와 동일한 색상 및 규칙을 적용한다.
#include "ui_cards.h"

#include <Arduino.h>
#include <stdio.h>
#include "font_ko.h"
#include "snapshot.h"

namespace {

// 에이전트 카드 UI 위젯 묶음
struct AgentCardWidgets {
    lv_obj_t *card = nullptr;
    lv_obj_t *nameLabel = nullptr;
    lv_obj_t *rateLabel = nullptr;
    lv_obj_t *usage5hLabel = nullptr;
    lv_obj_t *bar5h = nullptr;
    lv_obj_t *usageWkLabel = nullptr;
    lv_obj_t *barWk = nullptr;
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

// 리셋 카운트다운 포맷팅
void formatResetCountdown(uint64_t resetEpochSec, uint64_t currentEpochSec, char *buf, size_t bufSize) {
    if (resetEpochSec <= currentEpochSec) {
        snprintf(buf, bufSize, "초기화 대기");
        return;
    }
    uint64_t diff = resetEpochSec - currentEpochSec;
    uint32_t hours = (uint32_t)(diff / 3600);
    uint32_t mins = (uint32_t)((diff % 3600) / 60);
    if (hours > 0) {
        snprintf(buf, bufSize, "%u시간 %u분뒤 초기화", (unsigned)hours, (unsigned)mins);
    } else {
        uint32_t secs = (uint32_t)(diff % 60);
        snprintf(buf, bufSize, "%u분 %u초뒤 초기화", (unsigned)mins, (unsigned)secs);
    }
}

}  // namespace

lv_obj_t *uiCardsCreate(lv_obj_t *parent) {
    if (parent == nullptr) {
        parent = lv_screen_active();
    }

    g_cardsRoot = lv_obj_create(parent);
    lv_obj_set_size(g_cardsRoot, lv_pct(100), 288);
    lv_obj_set_pos(g_cardsRoot, 0, 30);
    lv_obj_set_style_bg_opa(g_cardsRoot, LV_OPA_TRANSP, 0);
    lv_obj_set_style_border_width(g_cardsRoot, 0, 0);
    lv_obj_set_style_pad_all(g_cardsRoot, 4, 0);

    g_cardsContainer = lv_obj_create(g_cardsRoot);
    lv_obj_set_size(g_cardsContainer, lv_pct(100), lv_pct(100));
    lv_obj_set_flex_flow(g_cardsContainer, LV_FLEX_FLOW_COLUMN);
    lv_obj_set_style_bg_opa(g_cardsContainer, LV_OPA_TRANSP, 0);
    lv_obj_set_style_border_width(g_cardsContainer, 0, 0);
    lv_obj_set_style_pad_all(g_cardsContainer, 0, 0);
    lv_obj_set_style_pad_row(g_cardsContainer, 6, 0);

    g_noDataLabel = lv_label_create(g_cardsRoot);
    lv_obj_set_style_text_font(g_noDataLabel, &font_ko, 0);
    lv_label_set_text(g_noDataLabel, "데이터 수신 대기 중...");
    lv_obj_center(g_noDataLabel);

    // 에이전트 카드 위젯 사전 생성
    for (size_t i = 0; i < SNAPSHOT_MAX_AGENTS; i++) {
        AgentCardWidgets &cw = g_cards[i];

        cw.card = lv_obj_create(g_cardsContainer);
        lv_obj_set_width(cw.card, lv_pct(100));
        lv_obj_set_height(cw.card, LV_SIZE_CONTENT);
        lv_obj_set_style_bg_color(cw.card, lv_color_hex(0x2c2c2e), 0);
        lv_obj_set_style_bg_opa(cw.card, LV_OPA_COVER, 0);
        lv_obj_set_style_radius(cw.card, 6, 0);
        lv_obj_set_style_border_width(cw.card, 0, 0);
        lv_obj_set_style_pad_all(cw.card, 6, 0);
        lv_obj_set_flex_flow(cw.card, LV_FLEX_FLOW_COLUMN);
        lv_obj_set_style_pad_row(cw.card, 2, 0);

        // 상단 행 (이름 + tok/s)
        lv_obj_t *topRow = lv_obj_create(cw.card);
        lv_obj_set_size(topRow, lv_pct(100), LV_SIZE_CONTENT);
        lv_obj_set_style_bg_opa(topRow, LV_OPA_TRANSP, 0);
        lv_obj_set_style_border_width(topRow, 0, 0);
        lv_obj_set_style_pad_all(topRow, 0, 0);
        lv_obj_set_flex_flow(topRow, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(topRow, LV_FLEX_ALIGN_SPACE_BETWEEN, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);

        cw.nameLabel = lv_label_create(topRow);
        lv_obj_set_style_text_font(cw.nameLabel, &font_ko, 0);
        lv_obj_set_style_text_color(cw.nameLabel, lv_color_hex(0xffffff), 0);
        lv_label_set_text(cw.nameLabel, "");

        cw.rateLabel = lv_label_create(topRow);
        lv_obj_set_style_text_font(cw.rateLabel, &font_ko, 0);
        lv_obj_set_style_text_color(cw.rateLabel, lv_color_hex(0x0a84ff), 0);
        lv_label_set_text(cw.rateLabel, "");

        // 5시간 쿼터 통합 라벨 (예: "5h 20%남음 4시간 55분뒤 초기화")
        cw.usage5hLabel = lv_label_create(cw.card);
        lv_obj_set_style_text_font(cw.usage5hLabel, &font_ko, 0);
        lv_obj_set_style_text_color(cw.usage5hLabel, lv_color_hex(0x8e8e93), 0);
        lv_label_set_text(cw.usage5hLabel, "");

        // 5시간 쿼터 바 (0%여도 항상 노출)
        cw.bar5h = lv_bar_create(cw.card);
        lv_obj_set_size(cw.bar5h, lv_pct(100), 4);
        lv_obj_set_style_bg_color(cw.bar5h, lv_color_hex(0x1c1c1e), 0);
        lv_obj_set_style_radius(cw.bar5h, 2, 0);
        lv_bar_set_range(cw.bar5h, 0, 100);

        // 주간 쿼터 통합 라벨 (예: "주간 80%남음 2일뒤 초기화")
        cw.usageWkLabel = lv_label_create(cw.card);
        lv_obj_set_style_text_font(cw.usageWkLabel, &font_ko, 0);
        lv_obj_set_style_text_color(cw.usageWkLabel, lv_color_hex(0x8e8e93), 0);
        lv_label_set_text(cw.usageWkLabel, "");

        // 주간 쿼터 바
        cw.barWk = lv_bar_create(cw.card);
        lv_obj_set_size(cw.barWk, lv_pct(100), 4);
        lv_obj_set_style_bg_color(cw.barWk, lv_color_hex(0x1c1c1e), 0);
        lv_obj_set_style_radius(cw.barWk, 2, 0);
        lv_bar_set_range(cw.barWk, 0, 100);

        lv_obj_add_flag(cw.card, LV_OBJ_FLAG_HIDDEN);
    }

    return g_cardsRoot;
}

void uiCardsUpdate(const Transport &transport) {
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

    for (size_t i = 0; i < SNAPSHOT_MAX_AGENTS; i++) {
        AgentCardWidgets &cw = g_cards[i];
        if (i >= snap.agentCount) {
            lv_obj_add_flag(cw.card, LV_OBJ_FLAG_HIDDEN);
            continue;
        }

        lv_obj_clear_flag(cw.card, LV_OBJ_FLAG_HIDDEN);
        const SnapshotAgent &ag = snap.agents[i];

        // 이름
        lv_label_set_text(cw.nameLabel, getAgentName(ag.kind));

        // tok/s 속도
        char rateBuf[32];
        if (ag.rateTokPerSec > 0.05f) {
            snprintf(rateBuf, sizeof(rateBuf), "%.1f tok/s", ag.rateTokPerSec);
        } else {
            snprintf(rateBuf, sizeof(rateBuf), "0 tok/s");
        }
        lv_label_set_text(cw.rateLabel, rateBuf);

        // 5시간 쿼터
        float pct5h = ag.has5hUsagePct ? ag.usage5hPct : 0.0f;
        if (pct5h > 100.0f) pct5h = 100.0f;
        if (pct5h < 0.0f) pct5h = 0.0f;
        float rem5h = 100.0f - pct5h;

        char resetBuf[64] = "";
        if (ag.has5hResetAt) {
            formatResetCountdown(ag.reset5hEpochSec, currentEpochSec, resetBuf, sizeof(resetBuf));
        }

        char uBuf[128];
        if (resetBuf[0] != '\0') {
            snprintf(uBuf, sizeof(uBuf), "5h %.0f%%남음 %s", rem5h, resetBuf);
        } else {
            snprintf(uBuf, sizeof(uBuf), "5h %.0f%%남음", rem5h);
        }
        lv_label_set_text(cw.usage5hLabel, uBuf);
        lv_obj_set_style_text_color(cw.usage5hLabel, getPctColor(pct5h), 0);

        // 0%여도 그래프(바)는 항상 노출
        lv_bar_set_value(cw.bar5h, (int32_t)pct5h, LV_ANIM_OFF);
        lv_obj_set_style_bg_color(cw.bar5h, getPctColor(pct5h), LV_PART_INDICATOR);
        lv_obj_clear_flag(cw.bar5h, LV_OBJ_FLAG_HIDDEN);

        // 주간 쿼터
        if (ag.hasWeeklyUsagePct) {
            float pctWk = ag.usageWeeklyPct;
            if (pctWk > 100.0f) pctWk = 100.0f;
            if (pctWk < 0.0f) pctWk = 0.0f;
            float remWk = 100.0f - pctWk;

            char resetWkBuf[64] = "";
            if (ag.hasWeeklyResetAt) {
                formatResetCountdown(ag.resetWeeklyEpochSec, currentEpochSec, resetWkBuf, sizeof(resetWkBuf));
            }

            char wBuf[128];
            if (resetWkBuf[0] != '\0') {
                snprintf(wBuf, sizeof(wBuf), "주간 %.0f%%남음 %s", remWk, resetWkBuf);
            } else {
                snprintf(wBuf, sizeof(wBuf), "주간 %.0f%%남음", remWk);
            }
            lv_label_set_text(cw.usageWkLabel, wBuf);
            lv_obj_set_style_text_color(cw.usageWkLabel, getPctColor(pctWk), 0);
            lv_obj_clear_flag(cw.usageWkLabel, LV_OBJ_FLAG_HIDDEN);

            // 주간 쿼터 바 항상 노출
            lv_bar_set_value(cw.barWk, (int32_t)pctWk, LV_ANIM_OFF);
            lv_obj_set_style_bg_color(cw.barWk, getPctColor(pctWk), LV_PART_INDICATOR);
            lv_obj_clear_flag(cw.barWk, LV_OBJ_FLAG_HIDDEN);
        } else {
            lv_obj_add_flag(cw.usageWkLabel, LV_OBJ_FLAG_HIDDEN);
            lv_obj_add_flag(cw.barWk, LV_OBJ_FLAG_HIDDEN);
        }
    }
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
