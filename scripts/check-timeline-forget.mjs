#!/usr/bin/env node
/*
 * 時間軸上那顆「忘掉這一段」的狀態機。
 *
 * 這一頁上有這整個程式裡**唯一一個不可逆的動作**，而它是兩段式的：第一下
 * 預覽（「會刪掉 3 段文字——不可復原」），第二下才真的刪。兩段式的意思是
 * 那顆按鈕帶著狀態，而帶狀態的按鈕只有兩種壞法，兩種都很安靜：
 *
 * - **停在第二段**：他換了一天、改了時間，按鈕還紅著寫「確定刪掉」——下一
 *   下刪掉的是他完全沒看過的一段。
 * - **停在灰的**：刪完之後 `load()` 重讀清單失敗，而解開那顆按鈕的
 *   `armReset()` 只掛在讀成功那條路上。它從此按不動，唯一的線索是側邊欄
 *   那行錯誤——而畫面上那句「刪掉了 3 段文字」還在，右邊也還完整列著那
 *   三段。他會以為刪除沒生效。
 *
 * 兩種在螢幕上都不會自己承認。這一支載入 `apps/desktop/ui/timeline.js`
 * **原檔**，用產品自己的路（點日期、改欄位、按按鈕）把它們走一遍。
 */

import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { fakeEl, loader, read } from "./fake-dom.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "timeline.js");
const boot = loader(read(SRC));

const DAY = 86_400_000;
// 2026-08-17 00:00 +08:00 起算的兩天。用固定值而不是 `Date.now()`，
// 不然這支測試會在午夜前後給出不同的答案。
const D1 = 1_755_360_000_000;
const DAYS = [
  { start_ts: D1 + DAY, chunks: 12, first_ts: D1 + DAY + 3_600_000, last_ts: D1 + DAY + 7_200_000 },
  { start_ts: D1, chunks: 40, first_ts: D1 + 3_600_000, last_ts: D1 + 36_000_000 },
];
const MOMENTS = { moments: [], pauses: [], truncated: false };
const PREVIEW = { chunks: 3, facts: 2, frames: 5, images: 5, queries: 0, failed: [], bytes: 0 };

const tick = () => new Promise((r) => setTimeout(r, 20));

/** `table` 是 `{ 指令: 值 / Error / 函式 }`；沒列到的照預設回。 */
async function open(table = {}) {
  const nodes = new Map();
  const node = (sel) => {
    if (!nodes.has(sel)) nodes.set(sel, fakeEl());
    return nodes.get(sel);
  };
  const calls = [];

  globalThis.document = {
    querySelector: node,
    querySelectorAll: () => [],
    createElement: (tag) => fakeEl(tag),
    addEventListener() {},
    body: fakeEl(),
  };
  globalThis.location = { search: "" };
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};

  const DEFAULTS = {
    timeline_days: DAYS,
    timeline_moments: MOMENTS,
    forget_preview: PREVIEW,
    forget_range: PREVIEW,
    has_ever_recorded: true,
    has_ever_stored: true,
  };
  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, arg) => {
        calls.push(cmd);
        const v = cmd in table ? table[cmd] : DEFAULTS[cmd];
        if (typeof v === "function") return v(arg);
        if (v instanceof Error) throw v;
        return v ?? null;
      },
    },
  };

  await boot();
  await tick();
  const forget = node("[data-forget]");
  return {
    node,
    calls,
    forget,
    label: () => forget.textContent,
    armed: () => forget.className === "danger",
    say: () => node("[data-say]").textContent,
    sub: () => node("[data-day-sub]").textContent,
    /** 按那顆鍵。回傳「按得動嗎」——真的瀏覽器不會把 click 送給灰掉的按鈕。 */
    async press() {
      if (forget.disabled) return false;
      for (const fn of forget.handlers.click ?? []) fn();
      await tick();
      return true;
    },
    /** 點左邊第 i 個日期。走的是 `listDays` 掛上去的那個 handler。 */
    async pickDay(i) {
      const buttons = node("[data-days]").querySelectorAll("button");
      for (const fn of buttons[i].handlers.click ?? []) fn();
      await tick();
    },
    async typeRange(from, to) {
      node("[data-from]").value = from;
      node("[data-to]").value = to;
      for (const fn of node("[data-from]").handlers.input ?? []) fn();
      await tick();
    },
  };
}

let failed = 0;
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

console.log("① 兩段式：第一下只問，第二下才刪");
{
  const p = await open();
  check("開場是預覽那一段", !p.armed(), p.forget.className);
  await p.press();
  check("第一下只是預覽", p.calls.includes("forget_preview"), p.calls);
  check("而且沒有真的刪", !p.calls.includes("forget_range"), p.calls);
  check("按鈕變成紅的「確定刪掉」", p.armed() && p.label() === "確定刪掉", p.label());
  check("數字和「不可復原」在同一句", p.say().includes("不可復原"), p.say());
  await p.press();
  check("第二下才真的刪", p.calls.includes("forget_range"), p.calls);
  check("刪完退回預覽那一段", !p.armed(), p.forget.className);
  check("而且說得出刪掉了什麼", p.say().includes("刪掉了"), p.say());
}

console.log("② 換一天要退回第一段");
{
  const p = await open();
  await p.press();
  check("先進到「確定刪掉」", p.armed(), p.label());
  await p.pickDay(1);
  check(
    "換天之後那顆鍵不可以還紅著——它會刪掉他沒看過的一天",
    !p.armed(),
    p.label(),
  );
}

console.log("③ 改時間也要退回第一段");
{
  const p = await open();
  check("空欄位的意思是整天", p.label() === "忘掉這一整天", p.label());
  await p.press();
  check("先進到「確定刪掉」", p.armed(), p.label());
  await p.typeRange("09:00", "10:00");
  check("改過範圍之後不算他點過頭", !p.armed(), p.label());
  // 標籤上那兩個時刻不比對字面：`hhmm` 走的是這台機器的時區，而 CI 的
  // runner 和開發機不在同一個時區。要驗的是**它不再說「整天」**——一顆寫著
  // 「整天」卻只會刪一小時的按鈕（或反過來）才是這裡真正的風險。
  check("標籤跟著縮小的範圍走，不再說整天", p.label() !== "忘掉這一整天", p.label());
  check("而且看得出是一段區間", p.label().includes("–"), p.label());
}

console.log("④ 刪成功了，但刪完那次重讀清單炸了");
{
  let reads = 0;
  const p = await open({
    // 第一次是開場那次（好），第二次是刪完之後那次（炸）。
    timeline_days: () => {
      if (++reads > 1) throw new Error("讀不到日期清單：database is locked");
      return DAYS;
    },
  });
  await p.press();
  await p.press();
  check("真的刪了", p.calls.includes("forget_range"), p.calls);
  check("說得出刪掉了什麼", p.say().includes("刪掉了"), p.say());
  // 這裡是這支測試存在的理由。
  check("那顆鍵沒有從此按不動", p.forget.disabled === false, `disabled=${p.forget.disabled}`);
  check("而且退回了預覽那一段", !p.armed(), p.forget.className);
  check("再按一次真的按得動", await p.press(), "按不下去");
  check(
    "有講右邊那一份是舊的（不然「刪掉了 3 段」配上一份還列著那 3 段的畫面）",
    p.sub().includes("上一次讀到"),
    p.sub(),
  );
}

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——那顆不可逆的按鈕停在一個它不該停的狀態。`);
  process.exit(1);
}
console.log("✓ 「忘掉這一段」的兩段式，在成功和失敗之後都退得回去");
