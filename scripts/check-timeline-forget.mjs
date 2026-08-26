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
import { domOf, fakeDocument, loader, read, watchNonsense } from "./fake-dom.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "timeline.js");
const HTML = read(join(UI, "timeline.html"));
const boot = loader(read(SRC));

const DAY = 86_400_000;
// 2026-08-17 00:00 +08:00 起算的兩天。用固定值而不是 `Date.now()`，
// 不然這支測試會在午夜前後給出不同的答案。
const D1 = 1_755_360_000_000;
const DAYS = [
  { start_ts: D1 + DAY, chunks: 12, first_ts: D1 + DAY + 3_600_000, last_ts: D1 + DAY + 7_200_000 },
  { start_ts: D1, chunks: 40, first_ts: D1 + 3_600_000, last_ts: D1 + 36_000_000 },
];
/**
 * 一列真的紀錄，照 main.rs 的 `Moment` 一欄一欄抄的。
 *
 * 這一版之前 `MOMENTS` 是 `{ moments: [], … }`——**一列都沒有**。於是 ④ 那條
 * 「有講右邊那一份是舊的（不然「刪掉了 3 段」配上一份還列著那 3 段的畫面）」
 * 是綠的，而它站著的畫面上根本沒有那 3 段：`build()` 對空的那一天也會補一列
 * 「接下來 24 小時沒有新的東西進來」，`childElementCount` 於是是 1。
 * **那條斷言命名的處境，一次都沒有被造出來過。**
 */
function moment(over = {}) {
  return {
    ts: D1 + 3_600_000,
    app: "chrome.exe",
    title: "帳單查詢",
    url: "https://example.com/bill",
    text: "中華電信客服專線 0800-080-123",
    frame_id: 57,
    ...over,
  };
}

const MOMENTS = {
  moments: [moment(), moment({ ts: D1 + 7_200_000, title: "繳費紀錄" })],
  pauses: [],
  truncated: false,
};

/** 讀成功、但那一天是空的。`build()` 照樣會補一列填空用的灰字。 */
const EMPTY_DAY = { moments: [], pauses: [], truncated: false };

/**
 * `Erasure` 的形狀，照 main.rs 那個 struct **一欄一欄**抄的。
 *
 * 第一版只寫了它「看起來會用到」的那幾欄，而且把 `image_bytes` 寫成 `bytes`。
 * 於是畫面上印出「5 張畫面（NaN MB）」，而那條斷言問的是「這句話裡有沒有
 * 『刪掉了』」——綠的。`missing`、`sessions`、`sessions_left`、`shell_beat`
 * 整個沒送，所以 `ghosts()` 和 `leftover()`（`timeline.js` 裡花最多字論證的
 * 那一段）從頭到尾一行都沒跑過。
 *
 * 少抄一欄不會有人報錯，這就是為什麼要照著抄，而不是照著「用得到的」抄。
 */
function erasure(over = {}) {
  return {
    chunks: 3,
    facts: 2,
    frames: 5,
    images: 5,
    image_bytes: 5 * 148_000,
    events: 4,
    queries: 0,
    sessions: 0,
    failed: [],
    missing: 0,
    // 預覽算不出「刪完之後」——`null` 是「沒問過」，不是 0。
    sessions_left: null,
    shell_beat: "gone",
    ...over,
  };
}
const PREVIEW = erasure();

const tick = () => new Promise((r) => setTimeout(r, 20));

/** `table` 是 `{ 指令: 值 / Error / 函式 }`；沒列到的照預設回。 */
async function open(table = {}) {
  const node = domOf(HTML);
  const calls = [];

  // 和另外四支共用同一個假 `document`。五份手抄本的意思是下一次只會有一份被
  // 修好——`createTextNode` 就是那樣漏掉的，見 fake-dom.mjs 的 `fakeDocument`。
  globalThis.document = fakeDocument(node);
  globalThis.location = { search: "" };
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};

  const DEFAULTS = {
    timeline_days: DAYS,
    timeline_moments: MOMENTS,
    timeline_chapters: [],
    forget_preview: PREVIEW,
    // 真的刪完那次才答得出「刪完之後還剩什麼」。
    forget_range: erasure({ sessions_left: 0 }),
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

  const nonsense = watchNonsense();
  await boot();
  await tick();
  const forget = node("[data-forget]");
  return {
    node,
    calls,
    forget,
    nonsense,
    label: () => forget.textContent,
    armed: () => forget.className === "danger",
    say: () => node("[data-say]").textContent,
    /** 右邊那一片現在列出來的每一列。 */
    rows: () => node("[data-moments]").children.map((c) => c.textContent),
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
  // 這一條抓的是**這支測試自己**：`bytes` / `image_bytes` 抄錯一個字的時候，
  // 上面那句照樣有「刪掉了」，只是那句話裡多了一個「NaN MB」。
  check("那句話裡沒有 NaN / undefined", p.nonsense().length === 0, p.nonsense());
  check("MB 是算得出來的數字", /（0\.7 MB）/.test(p.say()), p.say());
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
  // **先證明那個處境真的成立。** 這條斷言命名的畫面是「右邊還完整列著那幾段」，
  // 而它上一版站在一個一列紀錄都沒有的畫面上（見 `moment()` 上面那段）。
  check(
    "右邊真的還列著那幾段",
    p.rows().some((t) => t.includes("客服專線")),
    p.rows(),
  );
  check(
    "有講右邊那一份是舊的（不然「刪掉了 3 段」配上一份還列著那 3 段的畫面）",
    p.sub().includes("上一次讀到"),
    p.sub(),
  );
}

console.log("⑤ 開頁就讀不到清單——右邊根本沒有「上一次讀到的那一份」");
{
  // ④ 驗的是 `forget()` 那個呼叫端。這一條驗**更常走到的**那個：檔案最底下的
  // `void load()`。同一句話在這兩條路上不能是同一句。
  const p = await open({
    timeline_days: new Error("讀不到日期清單：database is locked"),
  });
  check("左邊那行是原因", p.node("[data-rail-say]").textContent.includes("locked"), p.node("[data-rail-say]").textContent);
  check(
    "不可以說「右邊列的是上一次讀到的那一份」——右邊一筆都沒有",
    !p.sub().includes("上一次讀到"),
    p.sub(),
  );
  check("要說得出右邊沒有東西可以指", p.sub().includes("沒有一份"), p.sub());
  check("右邊真的是空的", p.node("[data-moments]").childElementCount === 0, p.node("[data-moments]").childElementCount);
}

console.log("⑥ 那份預覽要對得起帳：說不見的、說留下來的，各講各的");
{
  // 這三欄是 `timeline.js` 裡花最多字論證的那一段，而在這一版之前，這支測試
  // 一欄都沒送過——`ghosts()` 和 `leftover()` 從頭到尾沒跑過一行。
  const p = await open({
    forget_preview: erasure({ missing: 12 }),
    forget_range: erasure({ missing: 12, sessions: 2, sessions_left: 1, shell_beat: "booting" }),
  });
  await p.press();
  check("預覽就說得出那 12 列的圖早就不在磁碟上", p.say().includes("12 列"), p.say());
  await p.press();
  check("刪完那句也講同一件事", p.say().includes("12 列"), p.say());
  check("「錄製的紀錄」自己算一項", p.say().includes("2 場錄製的紀錄"), p.say());
  // `shell_beat` 三種的下一步不一樣，而它上一版是一個布林——分不出來的時候
  // 那句話會變成「她正在錄，**或**正在開機」，而開機那幾分鐘裡它是假的。
  check("留下來那一列說得出是誰的", p.say().includes("正在起來"), p.say());
  check(
    "而且不可以說成「她此刻正在錄」——那一列不是它的",
    !p.say().includes("她此刻正在錄"),
    p.say(),
  );
  check("整段話裡沒有 NaN / undefined", p.nonsense().length === 0, p.nonsense());
}

console.log("⑦ 那一天本來就是空的，重讀又失敗——右邊沒有「那一份」可以指");
{
  // ④ 和 ⑤ 中間還有一格：讀**成功**了，但那一天什麼都沒有。畫面上留下的是
  // `build()` 補的一列「接下來 24 小時沒有新的東西進來」，或者「這一天沒有
  // 東西。」——兩種都不是清單。
  //
  // 這一格是 `childElementCount > 0` 那個問法唯一會答錯的地方，而它答錯的方向
  // 正是這一批在修的：請他去看一個不存在的東西。
  let reads = 0;
  const p = await open({
    timeline_moments: EMPTY_DAY,
    timeline_days: () => {
      if (++reads > 1) throw new Error("讀不到日期清單：database is locked");
      return DAYS;
    },
  });
  check("先確認右邊只有填空用的那一列", p.rows().length <= 1, p.rows());
  check("而且那一列不是紀錄", !p.rows().some((t) => t.includes("客服專線")), p.rows());
  await p.press();
  await p.press();
  check("真的刪了", p.calls.includes("forget_range"), p.calls);
  check(
    "不可以說「右邊列的是上一次讀到的那一份」——右邊沒有一份清單",
    !p.sub().includes("上一次讀到"),
    p.sub(),
  );
  check("要說得出右邊沒有東西可以指", p.sub().includes("沒有一份"), p.sub());
  // **這三條是這一格真正的重點。** 這一句以前寫著「這一頁現在是空的」，而它是
  // 照著⑤（開頁就失敗，整頁真的空著）寫的。這一格不是那一格：左邊三天好好列
  // 著、標題還在、右邊還有那條填充列。每一行都是真的，湊起來說整頁是空的——
  // 而他剛按完一顆不可逆的按鈕，讀到的會是「刪掉的比我選的多」。
  check("左邊那幾天還好好列著", p.node("[data-days]").childElementCount === DAYS.length, p.node("[data-days]").childElementCount);
  check("標題也還在", p.node("[data-day-title]").textContent !== "", p.node("[data-day-title]").textContent);
  check(
    "所以那一句不可以說整頁是空的",
    !p.sub().includes("這一頁現在是空的"),
    p.sub(),
  );
}

console.log("⑧ 換到另一天讀失敗（右邊被清空了），接著重讀清單也失敗");
{
  // 承 ⑦：右邊變空的還有第二條路——`openDay` 自己那個 catch。它 `replaceChildren()`
  // 把右邊清光，而那時候 `listing` 還停在上一天的 true。守的是 `openDay` 的
  // catch 裡那一行 `listing = false`。
  let days = 0;
  let moments = 0;
  const p = await open({
    timeline_days: () => {
      if (++days > 1) throw new Error("讀不到日期清單：database is locked");
      return DAYS;
    },
    timeline_moments: () => {
      if (++moments > 1) throw new Error("讀不到這一天：database is locked");
      return MOMENTS;
    },
  });
  check("第一天真的列出東西", p.rows().some((t) => t.includes("客服專線")), p.rows());
  await p.pickDay(1);
  check("換過去那一天讀失敗，右邊被清光了", p.rows().length === 0, p.rows());
  await p.press();
  await p.press();
  check(
    "不可以說「右邊列的是上一次讀到的那一份」——它剛剛才被清光",
    !p.sub().includes("上一次讀到"),
    p.sub(),
  );
  // **只斷言「沒說 A」是不夠的**，那樣 B 講什麼都沒有人看。這一格和⑦一樣：
  // 左邊那三天還在，所以另一句也不可以說整頁空了。
  check("要說得出右邊沒有東西可以指", p.sub().includes("沒有一份"), p.sub());
  check("左邊那幾天還好好列著", p.node("[data-days]").childElementCount === DAYS.length, p.node("[data-days]").childElementCount);
  check("所以那一句不可以說整頁是空的", !p.sub().includes("這一頁現在是空的"), p.sub());
}

console.log("⑨ 外送紀錄：兩種空、沒送出去的原因、原文沒遮過");
{
  async function openOutbound(table) {
    const page = await open(table);
    const views = page.node("[data-views]");
    const btn = {
      getAttribute: (name) => (name === "data-view" ? "outbound" : null),
    };
    const ev = {
      target: {
        closest(sel) {
          return sel === "[data-view]" ? btn : null;
        },
      },
    };
    for (const fn of views.handlers.click ?? []) fn(ev);
    await tick();
    return page;
  }
  const never = await openOutbound({
    memory_outbound: { outbound: [], skips: [], ever_sent: false },
  });
  const neverText = never.node("[data-outbound]").textContent;
  check("從來沒送過要講出來", neverText.includes("還沒送過任何東西"), neverText);
  check("從來沒送過不能說被清掉了", !neverText.includes("清掉了"), neverText);
  check("面板講原文沒遮", neverText.includes("原文") && neverText.includes("沒有去識別化"), neverText);

  const pruned = await openOutbound({
    memory_outbound: { outbound: [], skips: [], ever_sent: true },
  });
  const prunedText = pruned.node("[data-outbound]").textContent;
  check("送過被清掉要講出來", prunedText.includes("清掉了") && prunedText.includes("不是從來沒送"), prunedText);
  check("兩種空不是同一句話", neverText !== prunedText, { neverText, prunedText });

  const filled = await openOutbound({
    memory_outbound: {
      ever_sent: true,
      outbound: [
        {
          ts: D1 + 3_600_000,
          command: "claude",
          args: ["-p"],
          chars_sent: 12,
          truncated: false,
          outcome: "success",
          duration_ms: 40,
          error: null,
          role: "interpreter",
        },
      ],
      skips: [
        {
          ts: D1 + 1_800_000,
          reason: "no_consent",
          detail: "還沒簽第二張同意書（上雲解讀）。",
        },
      ],
    },
  });
  const filledText = filled.node("[data-outbound]").textContent;
  check("成功的外送看得到命令", filledText.includes("claude"), filledText);
  check("沒送出去的原因要一起顯示", filledText.includes("還沒簽第二張同意書"), filledText);
  check("有列的時候不是那兩種空", !filledText.includes("還沒送過任何東西") && !filledText.includes("清掉了"), filledText);
}

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——那顆不可逆的按鈕停在一個它不該停的狀態。`);
  process.exit(1);
}
console.log("✓ 「忘掉這一段」的兩段式，在成功和失敗之後都退得回去");
console.log("✓ 外送紀錄面板把兩種空、跳過原因和原文沒遮講開了");
