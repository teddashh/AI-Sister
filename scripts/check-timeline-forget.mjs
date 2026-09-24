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
    told: 0,
    facts: 2,
    frames: 5,
    images: 5,
    image_bytes: 5 * 148_000,
    events: 4,
    queries: 0,
    sessions: 0,
    // 她替他動過的手（alpha.69）。後端這一欄不在資料庫裡，是 `forget_preview`
    // 和 `forget_range` 各自問 `ActionLog` 補上的——所以它正是最容易在這裡被
    // 漏抄的那一種。
    actions: 0,
    // 存著的那張授權書（alpha.74）。跟 `actions` 一樣不在資料庫裡，而且它
    // 比誰都容易被漏列——**它是唯一一個站在區間外面也會被刪掉的東西**。
    grant: false,
    failed: [],
    words_cleared: 0,
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
let passed = 0;
function check(name, ok, detail) {
  if (ok) passed++;
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

{
  const p = await open({ timeline_moments: { ...MOMENTS, moments: [moment({ source_kind: "told", app: null, title: null, url: null, frame_id: null, text: "紫色雨傘在玄關" })] } });
  check("A154-9 時間軸列出原話與來源", p.rows().join("").includes("紫色雨傘在玄關") && p.rows().join("").includes("你告訴她的話"), p.rows());
  check("A154-9 沒有宣稱是遺失的畫面", !p.rows().join("").includes("只剩這些字"), p.rows());
}
if (process.env.A154_ONLY === "1") {
  console.log(`${passed} passed; ${failed} failed`);
  process.exit(failed > 0 ? 1 : 0);
}

{
  const p = await open({ forget_preview: erasure({ told: 2 }), forget_range: erasure({ told: 2 }) });
  await p.press();
  check("預覽列出親口告訴她的話", p.say().includes("2 段你告訴她的話"), p.say());
  await p.press();
  check("刪除結果列出親口告訴她的話", p.say().includes("2 段你告訴她的話"), p.say());
}

{
  const source = read(SRC);
  const start = source.indexOf('function fakeBackend(');
  const end = source.indexOf('\n// `1` 是平常', start);
  const demo = new Function('DAY', 'tzOffsetMs', `${source.slice(start, end)}; return fakeBackend();`)(DAY, () => 0);
  const args = { fromTs: 0, toTs: Date.UTC(2027, 0, 1) };
  const preview = await demo('forget_preview', args);
  const actual = await demo('forget_range', args);
  check("假後端真的給非零親口記憶", preview.told > 0);
  check("假後端預覽與刪除親口記憶一致", preview.told === actual.told);
  check("假後端刪完不再列親口記憶", (await demo('forget_preview', args)).told === 0);
}

{
  const native = read(resolve(UI, "../src-tauri/src/main.rs"));
  const conversion = native.match(/impl From<sister_core::retention::PruneReport> for Erasure[\s\S]*?\n}/)?.[0] ?? "";
  check("native 刪除回條有 told 欄", conversion.includes("told: r.told_deleted,"));
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
    forget_preview: erasure({ missing: 12, actions: 3 }),
    forget_range: erasure({
      missing: 12,
      sessions: 2,
      actions: 3,
      sessions_left: 1,
      shell_beat: "booting",
    }),
  });
  await p.press();
  check("預覽就說得出那 12 列的圖早就不在磁碟上", p.say().includes("12 列"), p.say());
  // 她替他動過的手是這幾類裡最敏感的一種（完整的網址與檔案路徑），而它到
  // alpha.69 才第一次有人刪、有人列。「刪掉了卻沒有列出來」是這一支上面那
  // 三段註解各講過一次的同一件事，這是第四次。
  check("預覽就講得出那段時間她動過幾次手", p.say().includes("3 件"), p.say());
  await p.press();
  check("刪完那句也講同一件事", p.say().includes("12 列"), p.say());
  check("刪完也要講她動過的手，不可以只在預覽出現", p.say().includes("3 件"), p.say());
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

console.log("⑥ᵇ 想最後一段不是當掉");
{
  const p = await open({
    forget_range: erasure({ sessions_left: 1, shell_beat: "thinking" }),
  });
  await p.press();
  await p.press();
  check("說得出解釋層還在收尾", p.say().includes("還在把最後一段想完"), p.say());
  check("不可以說她當掉了", !p.say().includes("她當掉了"), p.say());
}

console.log("⑥ᶜ heartbeat 讀不懂不是沒有 recorder");
{
  const p = await open({
    forget_range: erasure({ sessions_left: 1, shell_beat: "unreadable" }),
  });
  await p.press();
  await p.press();
  check("明講 recording.beat 讀不懂", p.say().includes("讀不懂 recording.beat"), p.say());
  check("不冒充確認沒有 recorder", !p.say().includes("沒有任何 recorder 佔著"), p.say());
  check("不猜留下的 session 已當掉", !p.say().includes("她當掉了"), p.say());
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
  check("解釋層那一列講得出自己是哪一層", filledText.includes("解釋層"), filledText);

  // **這一段釘的是 `outboundRole`。**
  //
  // 那三個中文層別在 `ops.rs` 裡也各有一份，所以「盯梢層這三個字在原始碼裡
  // 找得到」這種檢查對這個面板什麼都證不到——把 timeline.js 的那一行整行刪掉，
  // 它照樣綠。要釘住它，就得真的把一列 watcher 餵進面板、再讀畫面上的字。
  //
  // 另一半是「不認得的時候不要猜」。這個函式原本的收尾是 `value || "解釋層"`：
  // 一列它沒讀懂的資料，它會很有把握地宣布那是解釋層。
  const roles = await openOutbound({
    memory_outbound: {
      ever_sent: true,
      skips: [],
      outbound: [
        {
          ts: D1 + 3_650_000,
          command: "grok",
          args: ["--verbatim"],
          chars_sent: 91,
          truncated: false,
          outcome: "success",
          duration_ms: 45,
          error: null,
          role: "interpreter_history",
        },
        {
          ts: D1 + 3_600_000,
          command: "claude",
          args: ["-p"],
          chars_sent: 12,
          truncated: false,
          outcome: "success",
          duration_ms: 40,
          error: null,
          role: "watcher",
        },
        {
          ts: D1 + 3_550_000,
          command: "codex",
          args: ["exec"],
          chars_sent: 87,
          truncated: false,
          outcome: "cancelled",
          duration_ms: 25,
          error: null,
          role: "answer",
        },
        {
          ts: D1 + 3_525_000,
          command: "grok",
          args: ["--verbatim"],
          chars_sent: 42,
          truncated: false,
          outcome: "success",
          duration_ms: 21,
          error: null,
          role: "answer_search",
        },
        {
          ts: D1 + 3_500_000,
          command: "claude",
          args: ["-p"],
          chars_sent: 12,
          truncated: false,
          outcome: "success",
          duration_ms: 40,
          error: null,
          role: "future-role",
        },
        // 空值要單獨餵一列。壞掉的那一版是 `value || "解釋層"`：非空的怪值它
        // 會原封不動印出來（於是「畫面上有 future-role」兩版都成立，斷言在
        // 那上面等於沒斷），只有**空值**那一格會走進 `||` 右邊、被宣布成解釋層。
        {
          ts: D1 + 3_400_000,
          command: "claude",
          args: ["-p"],
          chars_sent: 12,
          truncated: false,
          outcome: "success",
          duration_ms: 40,
          error: null,
          role: "",
        },
      ],
    },
  });
  const rolesText = roles.node("[data-outbound]").textContent;
  check("歷史補讀有自己的中文層別", rolesText.includes("舊記憶重讀"), rolesText);
  check("歷史補讀不會印成英文代號", !rolesText.includes("interpreter_history"), rolesText);
  check("盯梢層那一列講得出自己是哪一層", rolesText.includes("盯梢層"), rolesText);
  check("盯梢層不會印成英文的 watcher", !rolesText.includes("watcher"), rolesText);
  check("答題層與取消結局都用產品文字顯示", rolesText.includes("答題層") && rolesText.includes("已取消"), rolesText);
  check("答題查詢有自己的中文層別", rolesText.includes("答題查詢"), rolesText);
  check("兩種答題層不會印成英文代號", !rolesText.includes("answer") && !rolesText.includes("cancelled"), rolesText);
  // 斷言要打在「它說了自己不認得」上，不是打在那個怪值有沒有印出來上——
  // 怪值兩版都會印出來。
  check("不認得的層別要說出自己不認得", rolesText.includes("不認得"), rolesText);
  // 這一份沒有餵任何 interpreter 的列，所以畫面上冒出「解釋層」只有一個來源：
  // 它替一列自己沒讀懂的資料猜了一個層別。
  check("沒讀懂的層別不可以被猜成解釋層", !rolesText.includes("解釋層"), rolesText);
}

console.log("⑧ 存著的授權書：唯一一個站在區間外面也會被刪掉的東西");
{
  // `grant` 這一欄和 `actions` 是同一種東西——不在資料庫裡，由 `forget_preview`
  // 和 `forget_range` 各自補上。差別是它**不看區間**：他選的是哪一天都無所謂，
  // 那張票一樣會死。所以「刪掉了卻沒有列出來」在它身上比誰都嚴重：他挑一天
  // 按下去，結果掉的是一個跟那一天無關的東西。
  const p = await open({
    forget_preview: erasure({ grant: true }),
    forget_range: erasure({ grant: true }),
  });
  await p.press();
  check("預覽就要講那張票會跟著走", p.say().includes("授權書"), p.say());
  check("而且要講它不看區間", p.say().includes("不看區間"), p.say());
  check("還要講裡面有他打的任務原文", p.say().includes("任務原文"), p.say());
  await p.press();
  check("刪完那一段也要列出來", p.say().includes("授權書"), p.say());
}

console.log("⑨ 沒存過票的人不該看到那一句");
{
  // 一句對他不成立的警告，和一句沒講的警告一樣是假話——而這一欄預設是
  // `false`，所以少了這一條，上面那三條用一個寫死的 `true` 也會全過。
  const p = await open({ forget_preview: erasure(), forget_range: erasure() });
  await p.press();
  check("沒有票就不要提它", !p.say().includes("授權書"), p.say());
}

console.log("⑩ 字母人那一側真的有動手（不是只有畫面上寫著）");
{
  // **上面九條全部只證明了 `timeline.js` 會不會印。** 後端那兩支是 Tauri
  // command，本機編不到也跑不到——這個 repo 有一整層接線是零執行覆蓋的，
  // 而 `action-log.jsonl` 就是在那一層漏了好幾版沒人發現。
  //
  // 所以這裡退一步，讀原始碼：只問「那兩支函式的函式體裡有沒有這件事」。
  // **要框在函式體裡**，不是整份檔案裡找得到就算——同一個字串出現在隔壁
  // 函式或註解裡也會滿足一個鬆的檢查，而被守的那一行刪掉它照樣綠。
  const rust = read(
    resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/src-tauri/src/main.rs"),
  );
  const body = (name) => {
    const at = rust.indexOf(`fn ${name}(`);
    if (at < 0) return "";
    const open = rust.indexOf("{", rust.indexOf(") ->", at));
    let depth = 0;
    for (let i = open; i < rust.length; i++) {
      if (rust[i] === "{") depth++;
      else if (rust[i] === "}" && --depth === 0) return rust.slice(open, i);
    }
    return "";
  };
  const del = body("forget_range");
  const pre = body("forget_preview");
  check("找得到 forget_range 的函式體", del.length > 0, del.length);
  check("找得到 forget_preview 的函式體", pre.length > 0, pre.length);
  check(
    "forget_range 從完整 Presence 推出 shell_beat",
    del.includes("heartbeat::watching_word") && del.includes("heartbeat::presence"),
    del.slice(0, 400),
  );
  // 刪的那一支要真的呼叫共用的那支——CLI 走的是同一個函式。
  check("忘掉這一段要真的刪掉授權書", del.includes("forget_saved_grant"), del.slice(0, 200));
  // 預覽要問「有沒有」，不然畫面上那一句是憑空來的。
  check(
    "預覽要去看磁碟上有沒有那張票",
    pre.includes("grant_path") && pre.includes("grant_tmp_path"),
    pre.slice(0, 200),
  );
  // 兩支都要把答案放進回傳的那一欄，不然 `timeline.js` 收到的永遠是預設值。
  for (const [name, src] of [["forget_range", del], ["forget_preview", pre]]) {
    check(`${name} 要把答案填進 grant 那一欄`, /\bgrant,/.test(src), src.slice(0, 200));
  }
}

console.log("⑪ 錄過但一列都沒存：指到 doctor，不指不存在的設定段落");
{
  const source = read(SRC);
  check("下一步是 sister doctor", source.includes("一列內容都沒存進來過——跑 `sister doctor`"));
  check("不再指向設定頁的開始記錄段落", !source.includes("先到設定頁看「開始記錄」那一段"));
}

console.log("⑫ 「看當時的畫面」開不起來的時候，這一頁要說一聲");
{
  // app.js 那四顆同名的鍵在 alpha.150 修過（`check-pet-says-why.mjs` §91），而
  // 這一頁的三顆當時被寫成「射不到的自白」。同一個病：`open_frame` 是
  // `Result<(), String>`，三個呼叫端各自 `void invoke?.(…)`，沒有人 `.catch`，
  // 這一頁也沒有 `unhandledrejection` 的接口——那個錯掉在地上，按下去什麼都
  // 沒發生，和「我根本沒按到」逐像素相同。
  //
  // 兩層。第一層從原始碼出發：`open_frame` 在 timeline.js 的非註解行只准出現
  // 一次，就是 `openFrame` 那支 helper 裡面。第四顆鍵自己去叫 invoke，這一條
  // 當場紅。第二層真的去按。
  const productLines = read(SRC)
    .split("\n")
    .map((line, i) => [i + 1, line])
    .filter(([, line]) => {
      const t = line.trim();
      return !t.startsWith("*") && !t.startsWith("//") && !t.startsWith("/*");
    });
  const callSites = productLines.filter(([, line]) => line.includes("open_frame"));
  check("timeline.js 的產品碼裡只有一個地方叫得到 open_frame", callSites.length === 1, callSites);
  check(
    "而那一行就在 openFrame 那支 helper 裡",
    /invoke\?\.\("open_frame"/u.test(callSites[0]?.[1] ?? "") &&
      read(SRC).includes("function openFrame(frameId) {"),
    callSites[0],
  );

  // 少了 `.catch`，那個 rejection 在 Node 底下會直接把這支殺掉——閘門是紅的，
  // 而沒有任何一條斷言抓到它，輸出還在半路斷掉。那個紅是 Node 的性質，不是產品
  // 的：真的 webview 裡沒有人會死，它只是安靜地什麼都不做，而那正是要抓的東西。
  const dropped = [];
  const onDropped = (err) => dropped.push(String(err?.message ?? err));
  process.on("unhandledRejection", onDropped);

  const BOOM = "TIMELINE_OPEN_FRAME_BLEW_UP";
  const press = async (button) => {
    for (const fn of button.handlers.click ?? []) fn();
    await tick();
  };
  const GUESS_CARD = {
    id: 9001,
    segment_ref: 1,
    activity: "在看帳單",
    model_confidence: 0.31,
    evidence: [{ kind: "frame", id: 4242, label: "畫面 #4242" }],
  };
  // driver 只負責「把畫面開起來、交出一顆鍵」，**斷言一條都不在 driver 裡**：
  // 補一個空殼 driver 要過不了關。
  const drivers = {
    "時間軸那一列的「看當時的畫面」": async (openFrameResult) => {
      const page = await open({ open_frame: openFrameResult });
      return { page, button: page.node("[data-moments]").querySelector(".see") };
    },
    "她猜的那張卡底下的「根據」": async (openFrameResult) => {
      const page = await open({
        open_frame: openFrameResult,
        memory_current_guess: { message: "上一段在看帳單", card: null },
        memory_guesses: [GUESS_CARD],
      });
      // 換到「她猜的」那一頁，走法和 ⑨ 的 `openOutbound` 一樣：那個 handler 掛
      // 在 `[data-views]` 上，靠 `ev.target.closest("[data-view]")` 認人。
      const views = page.node("[data-views]");
      const btn = { getAttribute: (name) => (name === "data-view" ? "guess" : null) };
      const ev = { target: { closest: (sel) => (sel === "[data-view]" ? btn : null) } };
      for (const fn of views.handlers.click ?? []) fn(ev);
      await tick();
      return { page, button: page.node("[data-memory]").querySelector(".see") };
    },
  };

  for (const [name, drive] of Object.entries(drivers)) {
    // 對照組先跑：開得起來的時候**不准**有話說。少了這一條，helper 寫成「每次都
    // 抱怨一句」也是綠的，而那比沉默更糟。
    const okRun = await drive(null);
    check(`${name}：這顆鍵找得到`, okRun.button != null, okRun.button);
    await press(okRun.button);
    check(`${name}：開得起來的時候不多嘴`, !okRun.page.say().includes("打不開"), okRun.page.say());
    check(
      `${name}：而且真的去開了`,
      okRun.page.calls.includes("open_frame"),
      okRun.page.calls,
    );

    const bad = await drive(new Error(BOOM));
    await press(bad.button);
    const said = bad.page.say();
    check(`${name}：開不起來要說一聲`, said.includes("打不開"), said);
    // 原話照抄，理由和 `frame.js` 那邊一樣：只有 Rust 分得出是哪一種開不起來，
    // 這一頁不准自己編一個成因。
    check(`${name}：而且照抄 Rust 給的理由`, said.includes(BOOM), said);
    // 說在回條那一格，不是蓋掉這一天的摘要——後者是一句持續為真的話。
    check(`${name}：沒有蓋掉這一天的摘要`, !bad.page.sub().includes("打不開"), bad.page.sub());
  }

  // 兩拍：rejection 是下一個 microtask 才送到 `unhandledRejection` 的。
  await tick();
  await tick();
  check("沒有任何 open_frame 的錯掉在地上", dropped.length === 0, dropped);
  process.off("unhandledRejection", onDropped);

  /* 這一節抓不到什麼，講清楚——不然下一輪會有人以為它守住了整條路：
   *
   *   承諾那一頁的 `pledgeRow` 也有一顆「根據」鍵，這一節沒有按過它。它走的是
   *   同一支 helper（第一層蓋得到「有沒有人繞過」），但「按下去真的說了話」在
   *   這裡沒有被按過一次——把那顆鍵的 click handler 整個拿掉，這一節照樣綠。
   *   拿 `want=綠` 的刀量過，不是推出來的。 */
}

console.log("⑫b 同一顆「看當時的畫面」失敗後重開，晚到的失敗不准蓋掉後來的成功");
{
  const press = async (button) => {
    for (const fn of button.handlers.click ?? []) fn();
    await tick();
  };
  const BOOM = "TIMELINE_OPEN_FRAME_FIRST_FAIL";
  let opens = 0;
  const page = await open({
    open_frame: () => {
      opens += 1;
      if (opens === 1) throw new Error(BOOM);
      return null;
    },
  });
  const button = page.node("[data-moments]").querySelector(".see");
  await press(button);
  check(
    "第一次開不起來說一聲",
    page.say().includes("打不開") && page.say().includes(BOOM),
    page.say(),
  );
  await press(button);
  check("再按成功後那句失敗收掉", page.say() === "" && opens === 2, {
    say: page.say(),
    opens,
  });

  let finishLate = null;
  let lateOpens = 0;
  const late = await open({
    open_frame: () => {
      lateOpens += 1;
      if (lateOpens === 1) {
        return new Promise((_, reject) => {
          finishLate = () => reject(new Error("TIMELINE_OPEN_FRAME_LATE_FAIL"));
        });
      }
      return null;
    },
  });
  const lateButton = late.node("[data-moments]").querySelector(".see");
  await press(lateButton);
  await press(lateButton);
  check("第二下已成功時不多嘴", late.say() === "", late.say());
  finishLate?.();
  await tick();
  await tick();
  check(
    "較早那一次晚到的失敗不准蓋掉後來的成功",
    late.say() === "" && !late.say().includes("LATE_FAIL"),
    late.say(),
  );

  for (const navigate of ["day", "view"]) {
    let rejectOld;
    const moved = await open({
      open_frame: () => new Promise((_, reject) => { rejectOld = reject; }),
    });
    await press(moved.node("[data-moments]").querySelector(".see"));
    if (navigate === "day") await moved.pickDay(1);
    else {
      const button = { getAttribute: (name) => name === "data-view" ? "guess" : null };
      const event = { target: { closest: (selector) => selector === "[data-view]" ? button : null } };
      for (const fn of moved.node("[data-views]").handlers.click ?? []) fn(event);
      await tick();
    }
    rejectOld(new Error("PREVIOUS_PAGE_FRAME_FAILED"));
    await tick();
    check(`換${navigate}後舊出處失敗不回到新頁`, !moved.say().includes("PREVIOUS_PAGE"), moved.say());
  }
}

console.log("⑬ 六顆寫入鍵：寫不進去的時候，不准長得像成功");
{
  // ⑫ 守的是**讀**失敗（那扇視窗沒開起來）。這六顆是**寫**，後果重得多：Rust 回
  // `Err` 的時候資料庫裡一個字都沒變，而舊的寫法 `void invoke?.(…).then(重畫)` 會
  // 讓 `.then` 整段不跑——沒有重畫、沒有話、沒有紅字。畫面和「我根本沒按到」逐像素
  // 相同，而他會當成已經改好了走掉。更正正是 SPEC §8.3 說「唯一拿得到的 ground
  // truth」的那個東西。
  //
  // **這一節第一版只收了三顆**（改成這樣／結案／其他一切），而章節那三顆（合併／
  // 切開／撤銷）比它們早出貨、走的是自己那條 `runChapterEdit`，`catch` 裡寫的是
  // `say(錯誤字串, true)`——`say` 那一格是**這一天的摘要**，於是一次按壞就把
  // 「5 段・312 筆・09:12–18:40」換成一句沒有主詞的 Rust 錯誤，要換一天才回得來。
  // 族規寫成橫的就要第一版把既有成員全接上跑；沒接的那幾顆不會因為規則存在而變好。
  //
  // 第一層從原始碼出發，問的是整族不是這六顆：這一頁只准剩下**一個**地方把 invoke
  // 的結果丟在地上，就是 `openFrame`（它自己接了 `.catch`）；而且只准有一支
  // `catch` 把錯誤寫進 `say`，就是 `openDay` 那一支——整天讀不起來的時候本來就沒有
  // 摘要可言。第七顆鍵照舊寫成 `void invoke?.("x").then(…)`，這一條當場紅。
  const productLines = read(SRC)
    .split("\n")
    .map((line, i) => [i + 1, line])
    .filter(([, line]) => {
      const t = line.trim();
      return !t.startsWith("*") && !t.startsWith("//") && !t.startsWith("/*");
    });
  const dropped = productLines.filter(([, line]) => line.includes("void invoke"));
  check("這一頁只剩一個地方把 invoke 的結果丟在地上", dropped.length === 1, dropped);
  check(
    "而那一個是 openFrame，它自己接了成功／失敗兩臂",
    (dropped[0]?.[1] ?? "").includes("open_frame") &&
      (dropped[0]?.[1] ?? "").includes(".then"),
    dropped[0],
  );
  // 章節那三個指令各有兩個呼叫端（活動級和分鐘級），所以是 2 不是 1——數字寫死在
  // 這裡，下一顆新鍵不接 `wrote` 就會把它撞紅。
  const wants = {
    correct_l2: 1,
    commitment_kill: 1,
    commitment_other: 1,
    timeline_merge_chapters: 2,
    timeline_undo_segment_edit: 2,
    timeline_split_chapter: 1,
  };
  for (const [cmd, want] of Object.entries(wants)) {
    // 要 `invoke` 和指令名在同一行：這一頁底下那個 `?demo=1` 的假 native 有一整排
    // 同名的 `case`，它是**被呼叫的那一端**，不是呼叫端。（它自己就會 `throw`
    // 「找不到還活著的承諾」——也就是說這個病在 demo 模式下本來就示範得出來。）
    const sites = productLines.filter(
      ([, line]) => line.includes(`"${cmd}"`) && line.includes("invoke"),
    );
    check(`產品碼裡叫得到 ${cmd} 的地方剛好 ${want} 處`, sites.length === want, sites);
  }
  check("而且都走同一支 wrote", read(SRC).includes("function wrote(call, label, after) {"));
  // `say` 是這一天的摘要。把一次性的失敗寫進去＝拿一句持續為真的話換一句一次性的，
  // 而且要換一天才換得回來。唯一的例外是整天讀不起來那一支（那時本來就沒有摘要）。
  const criers = productLines.filter(([, line]) => /say\(String\(err/u.test(line));
  check("只有一個地方把錯誤寫進這一天的摘要", criers.length === 1, criers);
  check(
    "而那一個在 openDay 裡——整天讀不起來的時候本來就沒有摘要",
    (criers[0]?.[0] ?? 0) > read(SRC).split("\n").findIndex((l) => l.includes("async function openDay(")),
    criers[0],
  );

  // 和 ⑫ 同一個理由：少了接口，rejection 在 Node 底下會直接把這支殺掉，整體是紅的
  // 而沒有任何一條斷言抓到它。真的 webview 裡沒有人會死，它只是安靜地什麼都不做。
  const onFloor = [];
  const onDropped = (err) => onFloor.push(String(err?.message ?? err));
  process.on("unhandledRejection", onDropped);

  const BOOM = "TIMELINE_WRITE_BLEW_UP";
  const CARD = {
    id: 9001,
    segment_ref: "segment:1755360000000",
    activity: "在看帳單",
    model_confidence: 0.31,
    evidence: [],
  };
  // 兩段章節：第一段才有「與下一段合併」（要有 next），有 `edit_id` 才有「撤銷這次
  // 修改」。三顆鍵都掛 `dataset`，所以選得到。
  const chapter = (over = {}) => ({
    start_ts: D1 + 9 * 3_600_000,
    end_ts: D1 + 10 * 3_600_000,
    core_start_ts: D1 + 9 * 3_600_000,
    core_end_ts: D1 + 10 * 3_600_000,
    core_ms: 3_600_000,
    segment_count: 1,
    app: "Firefox",
    title: "在看帳單",
    ...over,
  });
  /** 成功之後後端回的那一份。刻意和 `CHAPTERS` 不同，畫面沒動就看得出來。 */
  const MERGED = [
    {
      start_ts: D1 + 9 * 3_600_000,
      end_ts: D1 + 11 * 3_600_000,
      core_start_ts: D1 + 9 * 3_600_000,
      core_end_ts: D1 + 11 * 3_600_000,
      core_ms: 7_200_000,
      segment_count: 2,
      app: "Firefox",
      title: "在看帳單",
      edited: "merge",
      edit_id: 9,
    },
  ];
  const CHAPTERS = [
    chapter({ edit_id: 5 }),
    chapter({
      start_ts: D1 + 10 * 3_600_000,
      end_ts: D1 + 11 * 3_600_000,
      core_start_ts: D1 + 10 * 3_600_000,
      core_end_ts: D1 + 11 * 3_600_000,
      app: "Terminal",
      title: "在跑測試",
    }),
  ];
  const PLEDGE = {
    id: 77,
    text: "五點去接她",
    status: "open",
    due_hint: "17:00",
    due_source: "explicit",
    tombstoned: false,
    evidence: [],
  };
  const pressClick = (button) => async () => {
    for (const fn of button.handlers.click ?? []) fn();
    await tick();
  };
  /**
   * 用**鍵上的字**找那顆鍵。
   *
   * 不用 `[data-merge]`：假瀏覽器的元素級 `querySelectorAll` 只認得 tag 和
   * `.class`（fake-dom.mjs 那支 `matches`），屬性選擇器一律不命中，而回來的
   * `null` 讀起來就是「這顆鍵不存在」——和 `classList` 那個盲點同一族。用字找還多
   * 一個好處：它和底下那條「句子要指名鍵上的字」問的是同一個字串。
   */
  const buttonSaying = (page, where, text) =>
    page.node(where).querySelectorAll("button").find((b) => b.textContent === text) ?? null;
  /** 換到上面那排的某一頁。走法和 ⑨ 的 `openOutbound` 一樣。 */
  const goTo = async (page, name) => {
    const btn = { getAttribute: (k) => (k === "data-view" ? name : null) };
    const ev = { target: { closest: (sel) => (sel === "[data-view]" ? btn : null) } };
    for (const fn of page.node("[data-views]").handlers.click ?? []) fn(ev);
    await tick();
  };

  // driver 只負責「把畫面開起來、交出一顆鍵和按它的方法」，**斷言一條都不在
  // driver 裡**：補一個空殼 driver 要過不了關。
  const drivers = {
    "她猜的那張卡上的「改成這樣」": {
      cmd: "correct_l2",
      reread: "memory_guesses",
      async open(result) {
        const page = await open({
          correct_l2: result,
          memory_current_guess: { message: "上一段在看帳單", card: null },
          memory_guesses: [CARD],
        });
        await goTo(page, "guess");
        const form = page.node("[data-memory]").querySelector(".guess-fix");
        const button = form?.querySelectorAll("button")[0] ?? null;
        return {
          page,
          button,
          press: async () => {
            form.querySelectorAll("input")[0].value = "其實我在報稅";
            for (const fn of form.handlers.submit ?? []) fn({ preventDefault: () => {} });
            await tick();
          },
        };
      },
    },
    "承諾那一列的「結案」": {
      cmd: "commitment_kill",
      reread: "memory_commitments",
      async open(result) {
        const page = await open({ commitment_kill: result, memory_commitments: [PLEDGE] });
        await goTo(page, "commitments");
        const button = page.node("[data-pledges]").querySelectorAll("button")[0] ?? null;
        return {
          page,
          button,
          press: async () => {
            for (const fn of button.handlers.click ?? []) fn();
            await tick();
          },
        };
      },
    },
    "章節上的「與下一段合併」": {
      cmd: "timeline_merge_chapters",
      reread: "timeline_chapters",
      async open(result) {
        const page = await open({
          timeline_chapters: CHAPTERS,
          timeline_merge_chapters: result === null ? MERGED : result,
        });
        const button = buttonSaying(page, "[data-moments]", "與下一段合併");
        return { page, button, press: pressClick(button) };
      },
    },
    "章節上的「撤銷這次修改」": {
      cmd: "timeline_undo_segment_edit",
      reread: "timeline_chapters",
      async open(result) {
        const page = await open({
          timeline_chapters: CHAPTERS,
          timeline_undo_segment_edit: result === null ? MERGED : result,
        });
        const button = buttonSaying(page, "[data-moments]", "撤銷這次修改");
        return { page, button, press: pressClick(button) };
      },
    },
    "章節上的「在這個時間切開」": {
      cmd: "timeline_split_chapter",
      reread: "timeline_chapters",
      async open(result) {
        const page = await open({
          timeline_chapters: CHAPTERS,
          timeline_split_chapter: result === null ? MERGED : result,
        });
        // 那一列預設是收起來的，先按「切開」把它攤開——他也是這樣按的。
        await pressClick(buttonSaying(page, "[data-moments]", "切開"))();
        const button = buttonSaying(page, "[data-moments]", "在這個時間切開");
        return { page, button, press: pressClick(button) };
      },
    },
    "承諾那一列的「其他一切」": {
      cmd: "commitment_other",
      reread: "memory_commitments",
      async open(result) {
        const page = await open({ commitment_other: result, memory_commitments: [PLEDGE] });
        await goTo(page, "commitments");
        const button = page.node("[data-pledges]").querySelectorAll("button")[1] ?? null;
        return {
          page,
          button,
          press: async () => {
            for (const fn of button.handlers.click ?? []) fn();
            await tick();
          },
        };
      },
    },
  };

  for (const [name, d] of Object.entries(drivers)) {
    // 對照組先跑：寫成功的時候**不准**有話說。少了這一條，helper 寫成「每次都抱怨
    // 一句」也是綠的，而那比沉默更糟。
    const okRun = await d.open(null);
    check(`${name}：這顆鍵找得到`, okRun.button != null, okRun.button?.textContent);
    const before = okRun.page.calls.filter((c) => c === d.reread).length;
    const rowsBefore = okRun.page.rows().join("|");
    await okRun.press();
    check(`${name}：真的寫出去了`, okRun.page.calls.includes(d.cmd), okRun.page.calls);
    check(`${name}：寫成功不多嘴`, !okRun.page.say().includes("沒做成"), okRun.page.say());
    // 成功那一臂要讓畫面動。少了這一條，把 `after()` 整個拿掉也是綠的，而畫面上那一
    // 列就永遠停在他按之前的樣子——和失敗長得一模一樣。承諾／猜測那三顆是再問一次
    // 後端；章節那三顆是拿回傳的那份章節直接重畫（所以數「又讀了一次」對它們永遠
    // 是假的，要數畫面上那幾列有沒有換過）。
    check(
      `${name}：寫完了畫面要跟著動`,
      d.reread === "timeline_chapters"
        ? okRun.page.rows().join("|") !== rowsBefore
        : okRun.page.calls.filter((c) => c === d.reread).length > before,
      [okRun.page.calls, okRun.page.rows()],
    );

    const bad = await d.open(new Error(BOOM));
    await bad.press();
    const said = bad.page.say();
    check(`${name}：寫不進去要說一聲`, said.includes("沒做成"), said);
    // 指名他剛剛按的那顆鍵。`tell` 只有一格、這一頁好幾顆鍵共用——比的是**鍵上的字**
    // 本身，所以改了按鈕的字而忘了改句子，這一條會紅。
    check(
      `${name}：指名他按的是哪一顆`,
      said.includes(`「${bad.button.textContent}」`),
      [said, bad.button.textContent],
    );
    // 「沒做成」聽起來像慢了一點。要講的是它**沒有發生**。
    check(`${name}：明講什麼都沒有改到`, said.includes("什麼都沒有改到"), said);
    check(`${name}：而且照抄 Rust 給的理由`, said.includes(BOOM), said);
    check(`${name}：沒有蓋掉這一天的摘要`, !bad.page.sub().includes("沒做成"), bad.page.sub());

    // 他按第二次、這次成功了：上一句失敗的話要收掉。留著的話，它會和一列已經消失
    // 的承諾同時在畫面上，而它這時候是假的。
    let n = 0;
    const flaky = () => {
      n += 1;
      if (n === 1) throw new Error(BOOM);
      return null;
    };
    const retry = await d.open(flaky);
    await retry.press();
    check(`${name}：第一下失敗有說話`, retry.page.say().includes("沒做成"), retry.page.say());
    await retry.press();
    check(`${name}：第二下成功要把上一句收掉`, !retry.page.say().includes("沒做成"), retry.page.say());
  }

  // 兩拍：rejection 是下一個 microtask 才送到 `unhandledRejection` 的。
  await tick();
  await tick();
  check("沒有任何一次寫入的錯掉在地上", onFloor.length === 0, onFloor);
  process.off("unhandledRejection", onDropped);

  /* 這一節抓不到什麼，講清楚——三條都拿 `want=綠` 的刀量過，不是推出來的：
   *
   *   一、**它不看送出去的內容**。把 `activity: next` 換成 `activity: ""`（他打的
   *   那句話整個丟掉，而 invoke 照樣成功）這一節照樣綠。它守的是「失敗的時候畫面
   *   要說實話」，不是「成功的時候送對了東西」。
   *
   *   二、**換頁不會把那句話收掉**。按了「結案」失敗、再切到「外送」那一頁，那句
   *   紅字還留在底下——`setView` 裡沒有 `tell("")`（`openDay` 和那兩個時間輸入框才
   *   有）。那句話這時候仍然是真的（那一筆確實沒寫進去），所以我沒動它；但這一節
   *   從來沒換過頁，在 `setView` 裡補一行 `tell("")` 它也是綠的。
   *
   *   三、**分鐘級那兩顆鍵（`pieceRow` 裡的合併／撤銷）沒有被按過**。它們和活動級
   *   那兩顆共用 `runChapterEdit`，所以上面第一層數得到它們（那兩個指令各要 2 處）；
   *   但「按下去真的說了話」在這裡只驗過活動級那一份。把 `pieceRow` 裡那兩顆的
   *   click handler 整個拿掉，這一節照樣綠。 */
}

console.log("⑭ 圖刪不掉：字已經移除、檔還在、下一輪只再試刪檔");
{
  // 列數來自回傳的 `words_cleared`。寫死「字已經移除」而不帶這個數字，
  // 或者把 0 列也說成已經移除，下面兩段會有一段紅。
  const cleared = await open({
    forget_range: erasure({
      failed: ["frames/2026/08/a.png: Access is denied"],
      words_cleared: 5,
      frames: 0,
      images: 0,
    }),
  });
  await cleared.press();
  await cleared.press();
  const said = cleared.say();
  check("點名刪不掉的畫面檔", said.includes("畫面檔刪不掉"), said);
  check("用報告上的列數說字已經移除", said.includes("這 5 列的字已經移除"), said);
  check("講畫面檔還在", said.includes("畫面檔還在"), said);
  check("下一步只再試刪檔", said.includes("下一輪只會再試刪檔"), said);
  check("不猜防毒或權限", !said.includes("防毒") && !said.includes("權限"), said);

  const none = await open({
    forget_range: erasure({
      failed: ["frames/2026/08/b.png: Access is denied"],
      words_cleared: 0,
    }),
  });
  await none.press();
  await none.press();
  const quiet = none.say();
  check("數到 0 列就不說字已經移除", !quiet.includes("字已經移除"), quiet);
  check("數到 0 列要說字還在", quiet.includes("字還在"), quiet);
  check("0 列也講檔還在、下一輪只試刪檔", quiet.includes("畫面檔還在") && quiet.includes("下一輪只會再試刪檔"), quiet);

  const unknown = await open({
    forget_range: erasure({
      failed: ["frames/2026/08/c.png: Access is denied"],
      words_cleared: undefined,
    }),
  });
  await unknown.press();
  await unknown.press();
  const withheld = unknown.say();
  check("沒帶回列數就不說字已經移除", !withheld.includes("字已經移除"), withheld);
  check("沒帶回列數也不說字還在", !withheld.includes("字還在"), withheld);
  check("沒帶回列數仍講檔還在、下一輪只試刪檔", withheld.includes("畫面檔還在") && withheld.includes("下一輪只會再試刪檔"), withheld);
}

console.log("");
console.log(`${passed} passed; ${failed} failed`);
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——那顆不可逆的按鈕停在一個它不該停的狀態。`);
  process.exit(1);
}
console.log("✓ 「忘掉這一段」的兩段式，在成功和失敗之後都退得回去");
console.log("✓ 外送紀錄面板把兩種空、跳過原因和原文沒遮講開了");
