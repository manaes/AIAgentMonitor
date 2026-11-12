// AI Agent Monitor — CYD 펌웨어: Dash 화면 구현. 좌표·색·폰트는 docs/design/b-final.en.html 의 스펙 표 그대로다.
#include "ui_dash.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

#include "dashlogic.h"

namespace {

// ── 색 (스펙 표 1) ────────────────────────────────────────────────────────
constexpr uint32_t COL_BG = 0x0B0F14, COL_CARD = 0x151B23, COL_TX = 0xF2F5F8, COL_SUB = 0xA7B3C0,
                   COL_TRACK = 0x2A323D, COL_CLAUDE = 0xFF9E4F, COL_CODEX = 0xC084FC, COL_RUN = 0x38BDF8,
                   COL_OK = 0x3DDC84, COL_FAILBAR = 0xB3202A;

const lv_font_t *const F10 = &lv_font_montserrat_10;
const lv_font_t *const F12 = &lv_font_montserrat_12;
const lv_font_t *const F16 = &lv_font_montserrat_16;
const lv_font_t *const F20 = &lv_font_montserrat_20;

struct Gauge {
    lv_obj_t *arc, *value, *label, *rate, *reset;
    bool limited = false;
};
struct Row {
    lv_obj_t *card, *stripe, *disc, *sym, *runArc, *name, *time, *project, *detail, *bar, *pct;
    bool valid;
    DashRow last;
    SnapshotJobState lastState;
};

lv_obj_t *g_root = nullptr;
Gauge g_gauge[2];
Row g_row[SNAPSHOT_MAX_JOBS];
lv_obj_t *g_emptyCard = nullptr, *g_emptyLabel = nullptr;

uint64_t g_lastT = 0;      // 마지막으로 본 스냅샷의 t
uint32_t g_rxMs = 0;       // 그 스냅샷을 처음 본 millis (CYD 에는 RTC 가 없다)
uint32_t g_lastRenderMs = 0;
bool g_seenSnapshot = false;

// ── 위젯 도우미 ────────────────────────────────────────────────────────────
lv_obj_t *makeBox(lv_obj_t *parent, int x, int y, int w, int h, uint32_t color, int radius) {
    lv_obj_t *o = lv_obj_create(parent);
    lv_obj_remove_flag(o, (lv_obj_flag_t)(LV_OBJ_FLAG_SCROLLABLE | LV_OBJ_FLAG_CLICKABLE));
    lv_obj_set_style_pad_all(o, 0, 0);
    lv_obj_set_style_border_width(o, 0, 0);
    lv_obj_set_style_radius(o, radius, 0);
    lv_obj_set_style_bg_color(o, lv_color_hex(color), 0);
    lv_obj_set_style_bg_opa(o, LV_OPA_COVER, 0);
    lv_obj_set_pos(o, x, y);
    lv_obj_set_size(o, w, h);
    return o;
}

lv_obj_t *makeLabel(lv_obj_t *parent, int x, int y, int w, int h, const lv_font_t *font, uint32_t color,
                    lv_text_align_t align = LV_TEXT_ALIGN_LEFT) {
    lv_obj_t *l = lv_label_create(parent);
    lv_obj_remove_flag(l, LV_OBJ_FLAG_CLICKABLE);
    lv_label_set_long_mode(l, LV_LABEL_LONG_MODE_DOTS);  // 폭 고정, 넘치면 끝을 "..." 로
    lv_obj_set_pos(l, x, y);
    lv_obj_set_size(l, w, h);
    lv_obj_set_style_text_font(l, font, 0);
    lv_obj_set_style_text_color(l, lv_color_hex(color), 0);
    lv_obj_set_style_text_align(l, align, 0);
    lv_obj_set_style_pad_all(l, 0, 0);
    lv_label_set_text(l, "");
    return l;
}

/// 같은 글자면 건드리지 않는다(불필요한 invalidate 가 풀스크린 플러시로 번지는 것을 막는다).
void setText(lv_obj_t *l, const char *s) {
    if (strcmp(lv_label_get_text(l), s) != 0) lv_label_set_text(l, s);
}

void place(lv_obj_t *o, int x, int y, int w, int h) {
    if (lv_obj_get_x(o) != x || lv_obj_get_y(o) != y) lv_obj_set_pos(o, x, y);
    if (lv_obj_get_width(o) != w || lv_obj_get_height(o) != h) lv_obj_set_size(o, w, h);
}

void setHidden(lv_obj_t *o, bool hidden) {
    if (hidden != lv_obj_has_flag(o, LV_OBJ_FLAG_HIDDEN)) {
        if (hidden) lv_obj_add_flag(o, LV_OBJ_FLAG_HIDDEN);
        else lv_obj_remove_flag(o, LV_OBJ_FLAG_HIDDEN);
    }
}

void styleArc(lv_obj_t *arc, int width, uint32_t track, uint32_t indicator) {
    lv_obj_remove_flag(arc, LV_OBJ_FLAG_CLICKABLE);
    lv_obj_set_style_bg_opa(arc, LV_OPA_TRANSP, 0);
    lv_obj_set_style_border_width(arc, 0, 0);
    lv_obj_set_style_pad_all(arc, 0, 0);
    lv_obj_set_style_arc_width(arc, width, LV_PART_MAIN);
    lv_obj_set_style_arc_width(arc, width, LV_PART_INDICATOR);
    lv_obj_set_style_arc_color(arc, lv_color_hex(track), LV_PART_MAIN);
    lv_obj_set_style_arc_color(arc, lv_color_hex(indicator), LV_PART_INDICATOR);
    lv_obj_set_style_arc_rounded(arc, false, LV_PART_MAIN);
    lv_obj_set_style_arc_rounded(arc, false, LV_PART_INDICATOR);
    lv_obj_set_style_bg_opa(arc, LV_OPA_TRANSP, LV_PART_KNOB);  // 손잡이 제거
    lv_obj_set_style_pad_all(arc, 0, LV_PART_KNOB);
    lv_arc_set_range(arc, 0, 100);
}

// ── 상단 게이지 ────────────────────────────────────────────────────────────
void createGauge(Gauge &g, int i) {
    const int cx = i ? 208 : 48, cy = 36, tx = cx + 34;
    const uint32_t color = i ? COL_CODEX : COL_CLAUDE;
    g.arc = lv_arc_create(g_root);
    lv_obj_set_pos(g.arc, cx - 28, cy - 28);
    lv_obj_set_size(g.arc, 56, 56);
    lv_arc_set_rotation(g.arc, 0);
    lv_arc_set_bg_angles(g.arc, 135, 45);  // 270° 스윕, 시계방향
    styleArc(g.arc, 5, COL_TRACK, color);
    lv_arc_set_value(g.arc, 0);
    g.value = makeLabel(g_root, cx - 20, cy - 11, 40, 22, F20, COL_TX, LV_TEXT_ALIGN_CENTER);
    g.label = makeLabel(g_root, tx, 12, 78, 14, F12, COL_TX);
    g.rate = makeLabel(g_root, tx, 30, 78, 14, F12, COL_SUB);
    g.reset = makeLabel(g_root, tx, 47, 78, 12, F10, COL_SUB);
    setText(g.label, i ? "Codex" : "Claude");
    setText(g.value, "--");
    setText(g.rate, "-- tok/s");
}

const SnapshotAgent *findAgent(const Snapshot &s, SnapshotAgentKind kind) {
    for (size_t i = 0; i < s.agentCount; i++)
        if (s.agents[i].kind == kind) return &s.agents[i];
    return nullptr;
}

// 대기 중에는 게이지 자리를 카운트다운에 쓰고, 모드가 바뀔 때만 배치를 변경한다.
void layoutGauge(Gauge &g, int i, bool limited) {
    if (g.limited == limited) return;
    g.limited = limited;
    const int cx = i ? 208 : 48, x = i * 160 + 8;
    setHidden(g.arc, limited);
    setHidden(g.reset, limited);
    place(g.label, limited ? x : cx + 34, limited ? 8 : 12, limited ? 144 : 78, 14);
    place(g.value, limited ? x : cx - 20, limited ? 28 : 25, limited ? 144 : 40, 22);
    place(g.rate, limited ? x : cx + 34, limited ? 55 : 30, limited ? 144 : 78, 14);
    lv_obj_set_style_text_align(g.value, limited ? LV_TEXT_ALIGN_LEFT : LV_TEXT_ALIGN_CENTER, 0);
    lv_obj_set_style_text_font(g.rate, limited ? F10 : F12, 0);
    lv_obj_set_style_text_color(g.value, lv_color_hex(limited ? COL_CLAUDE : COL_TX), 0);
    lv_obj_set_style_text_color(g.rate, lv_color_hex(limited ? COL_CLAUDE : COL_SUB), 0);
}

void renderHeader(const Snapshot &s, uint64_t now) {
    const SnapshotAgentKind kinds[2] = {SnapshotAgentKind::Claude, SnapshotAgentKind::Codex};
    for (int i = 0; i < 2; i++) {
        Gauge &g = g_gauge[i];
        const SnapshotAgent *a = findAgent(s, kinds[i]);
        const DashQuotaWait wait = a ? dashQuotaWait(*a) : DashQuotaWait{};
        const bool limited = wait.fiveHour || wait.weekly;
        layoutGauge(g, i, limited);
        if (limited) {
            char value[24], detail[32];
            const bool pending = !wait.hasReset || wait.resetEpochSec <= now;
            if (!wait.hasReset) strcpy(value, "Reset unknown");
            else if (pending) strcpy(value, "Refreshing");
            else if (wait.resetEpochSec - now < 60) strcpy(value, "<1m");
            else dashFormatReset(wait.resetEpochSec, now, value, sizeof(value));
            const lv_font_t *font = pending ? F16 : F20;
            if (lv_obj_get_style_text_font(g.value, LV_PART_MAIN) != font)
                lv_obj_set_style_text_font(g.value, font, 0);
            setText(g.value, value);
            setText(g.label, i ? "Codex - wait" : "Claude - wait");
            const char *reason = wait.fiveHour ? (wait.weekly ? "5h+7d limit" : "5h limit") : "7d limit";
            if (a->hasWeeklyUsagePct) {
                int p = (int)lroundf(a->usageWeeklyPct);
                p = p < 0 ? 0 : (p > 100 ? 100 : p);
                snprintf(detail, sizeof(detail), "%s / 7d %d%%", reason, p);
            } else {
                snprintf(detail, sizeof(detail), "%s / 7d --", reason);
            }
            setText(g.rate, detail);
            continue;
        }
        setText(g.label, i ? "Codex" : "Claude");
        if (lv_obj_get_style_text_font(g.value, LV_PART_MAIN) != F20)
            lv_obj_set_style_text_font(g.value, F20, 0);
        // 주간 값이 없거나 조회 실패(e) 중이면 낡은 %를 그리지 않는다.
        const bool ok = a && a->hasWeeklyUsagePct && !a->hasQuotaError;
        char buf[24];
        if (ok) {
            int p = (int)lroundf(a->usageWeeklyPct);
            p = p < 0 ? 0 : (p > 100 ? 100 : p);
            snprintf(buf, sizeof(buf), "%d", p);
            if (lv_arc_get_value(g.arc) != p) lv_arc_set_value(g.arc, p);
            lv_obj_set_style_arc_opa(g.arc, LV_OPA_COVER, LV_PART_INDICATOR);
        } else {
            strcpy(buf, "--");
            lv_obj_set_style_arc_opa(g.arc, LV_OPA_TRANSP, LV_PART_INDICATOR);
        }
        setText(g.value, buf);
        if (a) {
            char r[16];
            dashFormatRate(a->rateTokPerSec, r, sizeof(r));
            snprintf(buf, sizeof(buf), "%s tok/s", r);
        } else {
            strcpy(buf, "-- tok/s");
        }
        setText(g.rate, buf);
        if (ok && a->hasWeeklyResetAt) {
            char r[16];
            dashFormatReset(a->resetWeeklyEpochSec, now, r, sizeof(r));
            snprintf(buf, sizeof(buf), "%s", r);
        } else {
            buf[0] = '\0';
        }
        setText(g.reset, buf);
    }
}

// ── job 행 ────────────────────────────────────────────────────────────────
void createRow(Row &r) {
    memset(&r, 0, sizeof(r));
    r.card = makeBox(g_root, 4, 80, 312, 36, COL_CARD, 4);
    r.stripe = makeBox(r.card, 0, 0, 4, 36, COL_TX, 0);
    r.disc = makeBox(r.card, 10, 8, 20, 20, COL_OK, LV_RADIUS_CIRCLE);
    r.sym = lv_label_create(r.disc);
    lv_obj_remove_flag(r.sym, LV_OBJ_FLAG_CLICKABLE);
    r.runArc = lv_arc_create(r.card);
    lv_arc_set_rotation(r.runArc, 270);
    lv_arc_set_bg_angles(r.runArc, 0, 360);
    styleArc(r.runArc, 2, COL_TRACK, COL_RUN);
    lv_arc_set_value(r.runArc, 25);  // 기본 25% 호, %를 알면 renderRows 가 갱신
    r.name = makeLabel(r.card, 0, 0, 100, 14, F12, COL_TX);
    r.time = makeLabel(r.card, 0, 0, 54, 14, F12, COL_SUB, LV_TEXT_ALIGN_RIGHT);
    r.project = makeLabel(r.card, 0, 0, 84, 12, F10, COL_SUB);
    r.detail = makeLabel(r.card, 0, 0, 100, 12, F10, COL_SUB);
    r.bar = lv_bar_create(r.card);
    lv_obj_remove_flag(r.bar, LV_OBJ_FLAG_CLICKABLE);
    lv_obj_set_style_radius(r.bar, 0, 0);
    lv_obj_set_style_radius(r.bar, 0, LV_PART_INDICATOR);
    lv_obj_set_style_bg_color(r.bar, lv_color_hex(COL_TRACK), 0);
    lv_obj_set_style_bg_color(r.bar, lv_color_hex(COL_RUN), LV_PART_INDICATOR);
    lv_bar_set_range(r.bar, 0, 100);
    r.pct = makeLabel(r.card, 0, 0, 46, 12, F10, COL_RUN, LV_TEXT_ALIGN_RIGHT);
    lv_obj_add_flag(r.card, LV_OBJ_FLAG_HIDDEN);
}

/// 행 하나의 배치(위치·크기·색·폰트). 행의 모양(상태/강조/y/h)이 바뀔 때만 부른다.
void layoutRow(Row &r, const SnapshotJob &j, const DashRow &d) {
    const bool emph = d.failEmphasis, run = j.state == SnapshotJobState::Running;
    const int h = d.h, y = d.y;
    if (emph) {
        place(r.card, 0, y, 320, h);
        lv_obj_set_style_radius(r.card, 0, 0);
        lv_obj_set_style_bg_color(r.card, lv_color_hex(COL_FAILBAR), 0);
        place(r.stripe, 0, 0, 4, h);
        setHidden(r.stripe, false);
        place(r.disc, 14, (h - 28) / 2, 28, 28);
        lv_obj_set_style_text_font(r.sym, F16, 0);
        place(r.name, 50, 8, 262, 18);
        lv_obj_set_style_text_font(r.name, F16, 0);
        place(r.detail, 50, 30, 262, 14);
        lv_obj_set_style_text_font(r.detail, F12, 0);
        lv_obj_set_style_text_color(r.detail, lv_color_hex(COL_TX), 0);
        setHidden(r.time, true);
        setHidden(r.project, true);
        setHidden(r.bar, true);
        setHidden(r.pct, true);
        setHidden(r.runArc, true);
    } else {
        const int oy = (h - 30) / 2, is = h >= 36 ? 20 : 16, ix = 6, tx = 6 + is + 6;  // 카드 기준(카드 x=4)
        place(r.card, 4, y, 312, h);
        lv_obj_set_style_radius(r.card, 4, 0);
        lv_obj_set_style_bg_color(r.card, lv_color_hex(COL_CARD), 0);
        setHidden(r.stripe, true);
        place(r.disc, ix, (h - is) / 2, is, is);
        lv_obj_set_style_text_font(r.sym, F12, 0);
        place(r.runArc, ix, (h - is) / 2, is, is);
        place(r.name, tx, oy + 2, 250 - tx, 14);
        lv_obj_set_style_text_font(r.name, F12, 0);
        place(r.time, 252, oy + 2, 54, 14);
        lv_obj_set_style_text_color(r.time, lv_color_hex(run ? COL_RUN : COL_SUB), 0);
        setHidden(r.time, false);
        place(r.project, tx, oy + 17, 84, 12);
        place(r.bar, 116, oy + 21, 138, 4);
        place(r.pct, 260, oy + 17, 46, 12);
        place(r.detail, tx, oy + 17, 274 - tx, 12);
        lv_obj_set_style_text_font(r.detail, F10, 0);
        lv_obj_set_style_text_color(r.detail, lv_color_hex(COL_SUB), 0);
        setHidden(r.project, !run);
        setHidden(r.bar, !run);
        setHidden(r.pct, !run);
        setHidden(r.detail, run);
        setHidden(r.runArc, !run);
    }
    // 아이콘: 진행중=링(runArc), 성공=초록 원+체크, 실패=흰 원+빨강 X (라벨 글리프, 내장 심볼)
    setHidden(r.disc, run);
    if (!run) {
        const bool ok = j.state == SnapshotJobState::Success;
        lv_obj_set_style_bg_color(r.disc, lv_color_hex(ok ? COL_OK : COL_TX), 0);
        lv_obj_set_style_text_color(r.sym, lv_color_hex(ok ? COL_BG : COL_FAILBAR), 0);
        lv_label_set_text(r.sym, ok ? LV_SYMBOL_OK : LV_SYMBOL_CLOSE);
        lv_obj_center(r.sym);
    }
    setHidden(r.card, false);
}

void renderRow(Row &r, const SnapshotJob &j, const DashRow &d, uint64_t now) {
    if (!r.valid || r.last.y != d.y || r.last.h != d.h || r.last.failEmphasis != d.failEmphasis ||
        r.lastState != j.state) {
        layoutRow(r, j, d);
        r.valid = true;
        r.last = d;
        r.lastState = j.state;
    }
    char ago[16] = "", dur[16] = "", buf[64];
    if (j.hasFinished) {
        dashFormatAgo(dashElapsedSec(now, j.finishedEpochSec), ago, sizeof(ago));
        dashFormatElapsed(dashElapsedSec(j.finishedEpochSec, j.startedEpochSec), dur, sizeof(dur));
    }
    setText(r.name, j.name);
    if (j.state == SnapshotJobState::Running) {
        dashFormatElapsed(dashElapsedSec(now, j.startedEpochSec), buf, sizeof(buf));
        setText(r.time, buf);
        setText(r.project, j.project);
        const int p = j.hasPercent ? (j.percent > 100 ? 100 : j.percent) : 0;
        if (lv_bar_get_value(r.bar) != p) lv_bar_set_value(r.bar, p, LV_ANIM_OFF);
        // 진행중 링도 실제 %를 따른다(%를 모르면 정적 25% 호 유지).
        const int ap = j.hasPercent ? p : 25;
        if (lv_arc_get_value(r.runArc) != ap) lv_arc_set_value(r.runArc, ap);
        snprintf(buf, sizeof(buf), "%d%%", p);
        setText(r.pct, buf);
    } else if (d.failEmphasis) {
        // 한 줄에 프로젝트 · 소요 · 경과를 담는다. "·"(U+00B7)는 내장 Montserrat 에 없어 ASCII 하이픈을 쓴다.
        snprintf(buf, sizeof(buf), "%s - failed in %s - %s", j.project, dur, ago);
        setText(r.detail, buf);
    } else {
        setText(r.time, ago);
        snprintf(buf, sizeof(buf), j.state == SnapshotJobState::Success ? "%s - took %s" : "%s - failed in %s",
                 j.project, dur);
        setText(r.detail, buf);
    }
}

void renderJobs(const Snapshot &s, uint64_t now) {
    DashRow rows[SNAPSHOT_MAX_JOBS];
    const size_t n = dashLayoutRows(s.jobs, s.jobCount, rows);
    for (size_t i = 0; i < SNAPSHOT_MAX_JOBS; i++) {
        if (i < n) {
            renderRow(g_row[i], s.jobs[rows[i].jobIndex], rows[i], now);
        } else if (g_row[i].valid) {
            setHidden(g_row[i].card, true);
            g_row[i].valid = false;
        }
    }
    setHidden(g_emptyCard, n != 0);
    setText(g_emptyLabel, "No recent jobs");
}

}  // namespace

lv_obj_t *uiDashCreate(lv_obj_t *parent) {
    g_root = lv_obj_create(parent ? parent : lv_screen_active());
    lv_obj_set_size(g_root, lv_pct(100), lv_pct(100));
    lv_obj_set_pos(g_root, 0, 0);
    lv_obj_set_style_radius(g_root, 0, 0);
    lv_obj_set_style_bg_color(g_root, lv_color_hex(COL_BG), 0);
    lv_obj_set_style_bg_opa(g_root, LV_OPA_COVER, 0);
    lv_obj_set_style_border_width(g_root, 0, 0);
    lv_obj_set_style_pad_all(g_root, 0, 0);
    lv_obj_set_scrollbar_mode(g_root, LV_SCROLLBAR_MODE_OFF);
    lv_obj_remove_flag(g_root, LV_OBJ_FLAG_SCROLLABLE);
    for (int i = 0; i < 2; i++) createGauge(g_gauge[i], i);
    for (size_t i = 0; i < SNAPSHOT_MAX_JOBS; i++) createRow(g_row[i]);
    g_emptyCard = makeBox(g_root, 4, 80, 312, 36, COL_CARD, 4);
    g_emptyLabel = makeLabel(g_emptyCard, 0, 11, 312, 14, F12, COL_SUB, LV_TEXT_ALIGN_CENTER);
    setText(g_emptyLabel, "Waiting for data...");
    return g_root;
}

void uiDashSetVisible(bool visible) {
    if (g_root) setHidden(g_root, !visible);
}

void uiDashUpdate(const Transport &transport) {
    if (!g_root || !transport.hasSnapshot()) return;
    const Snapshot &s = transport.latestSnapshot();
    const uint32_t ms = millis();
    const bool fresh = !g_seenSnapshot || s.emittedAtEpochSec != g_lastT;
    if (fresh) {
        g_seenSnapshot = true;
        g_lastT = s.emittedAtEpochSec;
        g_rxMs = ms;
    } else if (ms - g_lastRenderMs < 1000) {
        return;  // 새 스냅샷이 없으면 1초에 한 번만(시간 글자 갱신)
    }
    g_lastRenderMs = ms;
    const uint64_t now = dashNowEpoch(g_lastT, g_rxMs, ms);
    renderHeader(s, now);  // 새 스냅샷이 없어도 한도 리셋 카운트다운은 갱신한다
    renderJobs(s, now);
}
