#!/usr/bin/env node
/*
 * 字母人上那幾句「為什麼沒成」，活得過下一次輪詢嗎。
 *
 * `pollRecording` 每 5 秒跑一次，而它會呼叫兩次 `paint()`（`setRecording` 和
 * `setPaused` 各一次）。`paint()` **無條件**覆寫 `stateLine.textContent`。所以
 * 任何直接寫那一格的人，話的壽命是 0 到 5 秒，而且不由它自己決定——那個計時
 * 器不會因為他剛按了按鈕就重設，所以「0 秒」是真的會發生的。
 *
 * 這一族在 alpha.38 之前有五個（開始記錄失敗、問問題失敗、暫停切不動、時間軸
 * 開不起來、「還在翻…」），其中兩個的原始碼註解自己就在描述這個 bug：
 * `wakeFailed` 那個欄位上面整段講的就是它，而暫停那條寫著「寧可看起來沒反應，
 * 然後把原因寫出來」——輪詢一到，只剩下前半句。
 *
 * 外加一條不同形狀的：問題答不成的時候那句「這一題我沒答成」被塞進
 * `[data-hits]`，而那個 `<ul>` 開場是 `hidden`，只有 `renderHits` 會拿掉。
 * 第一題就失敗的人整個畫面一個字都不會多。
 *
 * 作法和 `check-settings-say.mjs` 一樣：載入 `apps/desktop/ui/app.js` **原檔**，
 * 不是一份抄過來的邏輯。重畫用的是產品自己的路（`pause-changed` 事件 →
 * `setPaused` → `paint()`），和輪詢走的是同一條，只是不必真的等五秒。
 */

import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { domOf, fakeDocument, hiddenIn, loader, read, watchNonsense } from "./fake-dom.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "app.js");
const LADDER_WORDS = [
  ["glimmer", "微光形式沒有獨立分支"],
  ["one_line", "一行形式沒有獨立分支"],
  ["card", "建議卡形式沒有獨立分支"],
  ["這張記憶不會再提了", "結案後沒有說記憶不再提"],
  ["先收起來，之後再說", "其他回饋沒有說之後再說"],
  ["收到你的回饋", "沒有 commitment 可改時沒有第三句"],
];
for (const [word, why] of LADDER_WORDS) {
  if (!read(SRC).includes(word)) {
    console.log(`✗ ${why}：缺少 ${word}`);
    process.exit(1);
  }
}
const boot = loader(read(SRC));

/*
 * 開場的 `hidden` 要跟 index.html 一樣，不能跟著假 DOM 的預設值走。詳細的
 * 理由在 fake-dom.mjs 的 `hiddenIn`——簡短版是：第一版寫死 false，於是這
 * 幾條測試在**真的壞掉的** app.js 上照樣綠。
 */
const HTML = read(join(UI, "index.html"));
const hiddenInHtml = (sel) => hiddenIn(HTML, sel);

// 前提本身也要驗一次。哪天 index.html 把那個 `hidden` 拿掉，這幾條測試會
// 悄悄變成「驗一個不存在的問題」——寧可在這裡就吵。
if (!hiddenInHtml("[data-hits]")) {
  console.log("✗ index.html 上的 [data-hits] 已經不是 hidden 了——底下那條測試的前提沒了");
  process.exit(1);
}

const tick = (ms = 20) => new Promise((r) => setTimeout(r, ms));

/**
 * `Answer` 的形狀，照 main.rs 那個 struct 抄的。
 *
 * 第一版寫的是 `{ hits: [], kind: "none", answers: [], blind: [], searched: [] }`
 * ——三個欄位的型別是錯的，而且 `"none"` 不是真的 kind（只有 `"keywords"` 和
 * `"recent"`）。`searched` 真的是 `Option<String>`，而 `[]` 在 JS 裡是 truthy，
 * 所以產品當場印出「我拿去比對的是「」——那是從你打的字黏出來的，不是一個
 * 詞」：一句只在她黏出非詞的時候才該出現的話，被一個空陣列叫了出來。沒有一條
 * 斷言問過它。
 */
function answer(over = {}) {
  return {
    kind: "keywords",
    searched: null,
    query_id: 7,
    answers: [],
    hits: [],
    truncated: false,
    answers_truncated: false,
    blind: null,
    ...over,
  };
}

/**
 * 一筆命中，照 main.rs 的 `Hit` 抄的。
 *
 * 到這一版為止沒有一個 case 送過真的命中（全是 `hits: []`），所以
 * `renderSnippet` 一次都沒跑過——而它第一行就在用 `document.createTextNode`，
 * 那是假 DOM 一直沒有的東西。少了它那個 TypeError 被 `ask()` 的 catch 接住，
 * 「答成了」於是走進「答不成」那條路。見 fake-dom.mjs 的 `fakeDocument`。
 */
function hit(over = {}) {
  return {
    chunk_id: 31,
    ts: 1_755_000_000_000,
    text: "客服專線 0800-080-123",
    snippet: "客服[專線] 0800-080-123",
    app: "chrome.exe",
    title: "帳單查詢",
    url: "https://example.com/bill",
    frame_id: null,
    ...over,
  };
}

/** `Blind` 的形狀，同上。後端只在一筆都沒找到的時候送。 */
function blind(over = {}) {
  return {
    chunks: 0,
    ocr_is_dead: false,
    frames: 0,
    ever_recorded: false,
    ever_stored: false,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
    scan_horizon_days: null,
    recording_now: false,
    booting_now: false,
    ...over,
  };
}

/**
 * 開一次字母人。`invoke` 收一張 `{ 指令: 回傳值或會丟出來的 Error }` 表；
 * 沒列到的指令回 `null`。函式值會被呼叫（要延遲、要丟例外的用這個）。
 *
 * `search` 是網址上那串 `?…`。**那幾條 demo 路徑不是裝飾。** 這台機器開不起
 * Tauri，所以 `?asleep=nobeat` 那幾條是這幾格畫面唯一長得出來的地方——他真的
 * 是照著那個網址用眼睛看版面的。以前這裡寫死 `""`，等於整條 demo 路徑沒有
 * 任何測試走過。
 */
async function open(table = {}, { search = "" } = {}) {
  // `domOf` 只生得出 index.html 上真的有的東西——見 fake-dom.mjs 開頭那段。
  const node = domOf(HTML);
  const listeners = new Map();

  globalThis.document = fakeDocument(node, {
    // **要是 visible。** 開場那一段對 `recording` 寫死的是 `"recording"`，
    // 只有 `updatePollGate()` 看到視窗是開著的才會去問一次磁碟；hidden 的話
    // 這一頁會停在「她在錄」，而灰掉那條路上的話（`wakeFailed`）就永遠不會
    // 被畫出來——測試會綠，但綠的理由是它根本沒走到那裡。
    visibilityState: "visible",
  });
  globalThis.location = { search };
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};
  globalThis.matchMedia = () => ({ matches: false, addEventListener() {} });

  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, arg) => {
        const v = table[cmd];
        if (typeof v === "function") return v(arg);
        if (v instanceof Error) throw v;
        return v ?? null;
      },
    },
    event: {
      listen: async (name, cb) => {
        listeners.set(name, cb);
        return () => {};
      },
    },
  };

  const nonsense = watchNonsense();
  await boot();
  await tick();
  return {
    node,
    nonsense,
    line: () => node("[data-state-line]").textContent,
    hits: () => node("[data-hits]"),
    hitTexts: () => node("[data-hits]").children.map((c) => c.textContent),
    /** 從這個視窗**以外**發生的事：系統匣的按鈕、熱鍵、她自己停掉。 */
    async fromOutside(name, payload) {
      const cb = listeners.get(name);
      if (!cb) throw new Error(`沒有人在聽 ${name}——這條測試的前提沒了`);
      cb({ payload });
      await tick();
    },
    async click(sel) {
      for (const fn of node(sel).handlers.click ?? []) fn();
      await tick();
    },
    async type(q) {
      node("[data-ask-input]").value = q;
      for (const fn of node("[data-ask-send]").handlers.click ?? []) fn();
      await tick();
    },
    /**
     * 逼一次重畫，走的是產品自己那條路：系統匣按了暫停 → `pause-changed`
     * → `setPaused` → `paint()`。輪詢那條（`setRecording` + `setPaused`）
     * 打在同一個 `paint()` 上，差別只有要不要等五秒。
     */
    async repaint() {
      const cb = listeners.get("pause-changed");
      if (!cb) throw new Error("沒有人在聽 pause-changed——這條測試的前提沒了");
      cb({ payload: false });
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

const CONSENT = "第一張同意書還沒簽——她不會開始記錄。在系統匣圖示上按右鍵，選「三張同意書…」簽好再回來";

console.log("① 按「開始記錄」，後端說同意書還沒簽");
{
  const p = await open({ start_recording: new Error(CONSENT), recording_state: "none" });
  await p.click("[data-wake]");
  check("當下說得出原因", p.line().includes("同意書"), p.line());
  await p.repaint();
  check("輪詢過後那句話還在", p.line().includes("同意書"), p.line());
  await p.repaint();
  check("再一輪也還在", p.line().includes("同意書"), p.line());
}

console.log("② 第一題就答不成（資料庫打不開）");
{
  const p = await open({
    ask: new Error("資料庫打不開：database is locked"),
    recording_state: "recording",
  });
  await p.type("剛剛發生什麼事");
  check("說得出是哪一種失敗", p.line().includes("database is locked"), p.line());
  check(
    "「我沒答成」那一列看得見（那個 ul 開場是 hidden）",
    p.hits().hidden === false,
    `hidden=${p.hits().hidden}`,
  );
  check(
    "而且真的有那一列",
    p.hits().children.some((c) => c.textContent.includes("沒答成")),
    p.hits().children.map((c) => c.textContent),
  );
  await p.repaint();
  check("輪詢過後那句原因還在", p.line().includes("database is locked"), p.line());
}

console.log("③ 這一題翻很久（SLOW_MS）");
{
  const p = await open({
    // 5 秒才回，比 SLOW_MS（4 秒）久。
    ask: () => new Promise((r) => setTimeout(() => r(answer()), 5000)),
    recording_state: "recording",
  });
  void p.type("三天前那通電話");
  await tick(4300);
  check("換成「還在翻…」了", p.line().includes("還在翻"), p.line());
  await p.repaint();
  check("輪詢過後沒有被換回「想一下…」", p.line().includes("還在翻"), p.line());
  await tick(1200);
  check("答案回來就不講了", !p.line().includes("還在翻"), p.line());
}

console.log("④ 暫停鍵切不動");
{
  const p = await open({
    toggle_pause: new Error("找不到資料目錄，暫停鍵沒有作用"),
    recording_state: "recording",
  });
  await p.click("#pause");
  check("說得出原因", p.line().includes("暫停鍵沒有作用"), p.line());
  await p.repaint();
  check("輪詢過後還在", p.line().includes("暫停鍵沒有作用"), p.line());
}

console.log("⑤ 時間軸開不起來");
{
  const p = await open({
    open_timeline: new Error("開不了時間軸視窗：WebView2 沒裝"),
    recording_state: "recording",
  });
  await p.click("#timeline");
  check("說得出原因", p.line().includes("WebView2"), p.line());
  await p.repaint();
  check("輪詢過後還在", p.line().includes("WebView2"), p.line());
}

console.log("⑥ 下一個動作要蓋掉上一次那句話");
{
  const p = await open({
    toggle_pause: new Error("找不到資料目錄，暫停鍵沒有作用"),
    ask: answer(),
    recording_state: "recording",
  });
  await p.click("#pause");
  check("先有那句話", p.line().includes("暫停鍵沒有作用"), p.line());
  await p.type("剛剛發生什麼事");
  check("問了下一題就不該再掛著", !p.line().includes("暫停鍵沒有作用"), p.line());
}

console.log("⑦ 早上那句舊的，不可以擋住系統匣剛剛那句新的");
{
  // `paint()` 讀的是 `notice ?? (wakeFailed ?? asleepDetail())`。九點問問題失敗
  // 留下 `notice`，九點五分從系統匣按開始記錄失敗 → `recorder-failed`。後端還
  // 特地把視窗叫到他面前，而他看到的是早上那句。
  const p = await open({
    ask: new Error("資料庫打不開：database is locked"),
    recording_state: "none",
  });
  await p.type("剛剛發生什麼事");
  check("先有早上那句", p.line().includes("database is locked"), p.line());
  await p.fromOutside("recorder-failed", CONSENT);
  check("系統匣那句要看得到", p.line().includes("同意書"), p.line());
  check("而且早上那句要讓開", !p.line().includes("database is locked"), p.line());
}

console.log("⑧ 從系統匣暫停成功之後，「暫停鍵沒有作用」不可以還掛著");
{
  const p = await open({
    toggle_pause: new Error("找不到資料目錄，暫停鍵沒有作用"),
    recording_state: "recording",
  });
  await p.click("#pause");
  check("先有那句", p.line().includes("暫停鍵沒有作用"), p.line());
  await p.fromOutside("pause-changed", true);
  check("她真的暫停了", p.line().includes("已暫停"), p.line());
  // 兩行都曾經是真的，湊起來在說「暫停鍵壞了」——而她正暫停著。
  check("那句「沒有作用」要跟著走", !p.line().includes("沒有作用"), p.line());
}

console.log("⑨ 第一次問問題就失敗的人，底下沒有「上一題」");
{
  const p = await open({
    ask: new Error("資料庫打不開：database is locked"),
    recording_state: "recording",
  });
  await p.type("三天前那通電話");
  check("有說沒答成", p.hitTexts().some((t) => t.includes("沒答成")), p.hitTexts());
  check(
    "但不可以說「底下原本那幾筆是上一題的」——他底下從來沒有東西",
    !p.hitTexts().some((t) => t.includes("上一題")),
    p.hitTexts(),
  );
}

console.log("⑩ 答成過一次之後再失敗，才輪得到那句「先收起來了」");
{
  let fail = false;
  const p = await open({
    ask: () => {
      if (fail) throw new Error("資料庫打不開：database is locked");
      return answer({ hits: [hit()] });
    },
    recording_state: "recording",
  });
  await p.type("電話");
  check("先答成一次", p.hits().hidden === false, p.hitTexts());
  fail = true;
  await p.type("電話");
  check("這次說得出「上一題」", p.hitTexts().some((t) => t.includes("上一題")), p.hitTexts());
}

console.log("⑪ 上一題的「還在翻…」不可以蓋到半秒前才送出的新題目上");
{
  // 時間軸（SLOW_MS = 4000）：
  //   t=0     第一題送出，永遠不回來
  //   t=3800  第二題送出（第一題還掛著，`state` 一直是 thinking）
  //   t=4000  **第一題**的計時器響。`state === "thinking"` 是真的——那是第二題的。
  //   t=4300  看畫面：第二題才半秒大，不可以說「這一題已經超過 4 秒」
  const p = await open({
    ask: (arg) =>
      arg.question === "第一題"
        ? new Promise(() => {})
        : new Promise((r) => setTimeout(() => r(answer()), 2200)),
    recording_state: "recording",
  });
  void p.type("第一題");
  await tick(3800);
  void p.type("第二題");
  await tick(500);
  check("還在想第二題", p.line().includes("想一下") || p.line().includes("在聽"), p.line());
  check("而且沒被上一題的計時器蓋掉", !p.line().includes("還在翻"), p.line());
}

console.log("⑫ 整場下來，畫面上沒有出現過 NaN / undefined");
{
  const p = await open({
    ask: answer({ blind: blind({ ever_recorded: true, frames: 12, chunks: 0 }) }),
    recording_state: "recording",
  });
  await p.type("三天前那通電話");
  // 假的 `blind` 少抄一欄的時候，那幾句「為什麼答不出來」會印出 undefined，
  // 而上面每一條斷言都不會發現——它們問的都是別的句子。
  check("那幾句「為什麼」是完整的", p.nonsense().length === 0, p.nonsense());
  check("而且真的說了為什麼", p.hitTexts().some((t) => t.includes("12 張畫面")), p.hitTexts());
}

console.log("⑬ 她正在錄，從系統匣按「停止記錄」失敗——那句話要看得到");
{
  // 系統匣那一格是**開關**（`main.rs` 的 `record_label`），所以 `recorder-failed`
  // 也會帶著「停不了」回來，而那一刻她正在錄。以前這句話走的是「叫不起來」那條
  // 路，而那條路在 `paint()` 裡被一道 `shown === "asleep"` 的閘門擋著——後端特地
  // `win.show()` + `set_focus()` 把視窗叫到他面前，然後那一格一個字都沒多。
  const p = await open({ recording_state: "recording" });
  check("先確認她真的在錄", p.line().includes("在聽"), p.line());
  await p.fromOutside("recorder-failed", "找不到資料目錄，停不了");
  check("那句話要出現在畫面上", p.line().includes("停不了"), p.line());
  // 反面：她在錄的時候不可以順便把狀態講成灰的。
  check("而且上面那行還是「她在錄」", p.line().includes("在聽"), p.line());
}

console.log("⑭ 叫她起來那幾秒中間問了一題失敗，真正的原因要贏");
{
  // `startRecording` 開頭清過一次，但那次清距離 `catch` 隔著一整段 `await`。
  // 中間他問一題、失敗了，於是接下來那句「第一張同意書還沒簽」被擋住——兩行
  // 都是真的，湊起來是「她沒起來，因為資料庫打不開」，而他會去查一顆好好的
  // 資料庫。真正的原因在右鍵選單裡，一下就簽得掉。
  const p = await open({
    recording_state: "none",
    start_recording: () =>
      new Promise((_, reject) => setTimeout(() => reject(new Error(CONSENT)), 700)),
    ask: new Error("資料庫打不開：database is locked"),
  });
  void p.click("[data-wake]");
  await tick(100);
  await p.type("剛剛發生什麼事");
  check("中間那句先在", p.line().includes("database is locked"), p.line());
  await tick(900);
  check("同意書那句要贏", p.line().includes("同意書"), p.line());
  check("而且中間那句要讓開", !p.line().includes("locked"), p.line());
}

console.log("⑮ 她開完資料庫的那一刻，「還在開資料庫，暫停鍵沒有作用」要走");
{
  // 這一條驗的是 `setRecording` 裡那一行 `overtakenByEvents()`——它是三個呼叫端
  // 裡唯一沒有測試的一個（拿掉它，五支閘門全綠）。
  //
  // 留著的話畫面是「在聽／她還在開資料庫，暫停鍵現在沒有作用」：上面那行剛說
  // 她開完了，下面那句說她還在開。兩行直接互相矛盾。
  let beats = 0;
  const p = await open({
    // 開場問一次（ 那一行是同步問的），之後 5 秒一輪。
    // 所以第一次答 booting，第二次——也就是第一輪輪詢——答 recording。
    recording_state: () => (++beats > 1 ? "recording" : "booting"),
    toggle_pause: new Error("她還在開資料庫，暫停鍵現在沒有作用"),
  });
  check("開場是正在起來", p.line().includes("正在開資料庫"), p.line());
  await p.click("#pause");
  check("先有那句", p.line().includes("暫停鍵現在沒有作用"), p.line());
  await tick(5400);
  check("她開完了", p.line().includes("在聽"), p.line());
  check("那句「還在開資料庫」要跟著走", !p.line().includes("還在開資料庫"), p.line());
}

console.log("⑯ 反面：狀態沒變的那幾輪輪詢，那句話不可以自己消失");
{
  // ⑮ 的修法很容易做成「每次輪詢都清」，那就變回 alpha.38 那個「五句話壽命
  // 0 到 5 秒」的 bug。這兩條要一起看才有意義。
  const p = await open({
    recording_state: "recording",
    toggle_pause: new Error("找不到資料目錄，暫停鍵沒有作用"),
  });
  await p.click("#pause");
  check("先有那句", p.line().includes("沒有作用"), p.line());
  await tick(5400);
  check("兩輪輪詢過後它還在", p.line().includes("沒有作用"), p.line());
}

console.log("⑰ 上一題空手而回，下一題失敗的時候不可以說「底下原本那幾筆」");
{
  // 底下躺的是「我記得的東西裡沒有這件事。」加幾行理由——一筆都沒有。而空手
  // 而回正是他最可能連著問第二次的那一種結果。
  let n = 0;
  const p = await open({
    ask: () => (++n === 1 ? answer({ hits: [], facts: [] }) : Promise.reject(new Error("讀不到"))),
    recording_state: "recording",
  });
  await p.type("三天前那通電話");
  check("第一題真的空手", p.hitTexts().some((t) => t.includes("沒有這件事")), p.hitTexts());
  await p.type("再問一次");
  check("有說沒答成", p.hitTexts().some((t) => t.includes("沒答成")), p.hitTexts());
  check(
    "但底下從來沒有「那幾筆」可以收",
    !p.hitTexts().some((t) => t.includes("上一題")),
    p.hitTexts(),
  );
}

console.log("⑱ 答成過、然後連著失敗兩次——第二次底下躺的是同一句錯誤，不是「上一題」");
{
  // 順序要**先答成一次**：`showingAnswer` 的初始值就是 false，所以從一張新的
  // 頁面連失敗兩次是驗不到東西的（那是 ⑰ 那條路）。要先讓它變成 true，才問
  // 得出「失敗那一次有沒有把它放回去」——守的是 `ask()` 的 catch 裡那一行
  // `showingAnswer = false`。
  let n = 0;
  const p = await open({
    ask: () => (++n === 1 ? answer({ hits: [hit()] }) : Promise.reject(new Error("讀不到"))),
    recording_state: "recording",
  });
  await p.type("第一題");
  check("第一題真的列出東西", p.hitTexts().some((t) => t.includes("客服")), p.hitTexts());
  await p.type("第二題");
  check("第二題說得出「上一題」", p.hitTexts().some((t) => t.includes("上一題")), p.hitTexts());
  await p.type("第三題");
  check("兩次都說沒答成", p.hitTexts().some((t) => t.includes("沒答成")), p.hitTexts());
  check(
    "但第三次底下躺的是同一句錯誤，不是上一題的答案",
    !p.hitTexts().some((t) => t.includes("上一題")),
    p.hitTexts(),
  );
}

console.log("⑲ 她**真的正在起來**的那 25 秒裡問了一題失敗，那一行要自己帶主詞");
{
  // ⑭ 驗的是 `start_recording` **被 reject** 的那條路，而那是這個 bug 比較小的
  // 一半：`await` 那一瞬間過去就結束了。真正長的是它 **resolve** 之後——
  // `starting` 會一路真到輪詢看見 recording 為止，最久 25 秒（`WAKE_TIMEOUT_MS`）。
  // 而那正是他最可能去問一題的 25 秒，因為畫面剛剛叫他等一下。
  //
  //     正在把她叫起來…
  //     資料庫打不開：database is locked
  //
  // 兩行都是真的。湊起來只讀得出一個意思——她起不來，因為資料庫打不開——而
  // 她其實好好地正在起來，那顆資料庫也沒有壞：那一題會失敗，就是因為她正在
  // 開它。他於是去修一顆沒有壞的東西。
  // 兩個狀態都要驗。`booting` 那一半**更長**——他那顆一年份的資料庫要開好幾
  // 分鐘（app.js 自己的註解就是這樣寫的），比 `starting` 的 25 秒長得多。少
  // 掉它，`starting || booting` 砍成 `starting` 會全綠。
  const OOPS = "資料庫打不開：database is locked";
  //
  // **`booting` 那一格不可以按「叫她起來」。** 第一版按了，於是那一格 `starting`
  // 也是真的，而 `line` 先看 `booting`——標題長得一模一樣，前提斷言就這樣為了
  // 錯的理由過關。抓到它的是 `starting || booting` 砍成 `starting` 之後**全綠**。
  // 真實情況本來就不必按：她會 booting，是因為系統匣或上一場把 recorder 開起
  // 來了，而他在那幾分鐘裡問了一題。
  for (const [what, state, head, wake] of [
    ["還在叫（25 秒）", "none", "正在把她叫起來", true],
    ["心跳看到了、資料庫還在開（好幾分鐘）", "booting", "正在開資料庫", false],
  ]) {
    console.log(`  — ${what}`);
    const p = await open({
      // 從頭到尾停在同一個狀態：她起得來，只是還沒起完。這就是那段窗。
      recording_state: state,
      start_recording: () => Promise.resolve(null),
      ask: new Error(OOPS),
    });
    if (wake) await p.click("[data-wake]");
    check("前提：她正在起來", p.line().includes(head), p.line());
    await p.type("剛剛發生什麼事");
    check("那句錯誤要在", p.line().includes(OOPS), p.line());
    // **只斷言「沒說 X」是不夠的**——那一行整個消失也會綠，而那是另一個 bug。
    // 要問的是它有沒有**變成另一句話**：底下那一行不可以就是那句錯誤本身。
    const detail = p.line().split("\n")[1] ?? "";
    check("但它不可以就這樣貼在底下", detail !== OOPS, detail);
    check("要看得出來是另一件事", detail.includes("另一件事"), detail);
  }
}

console.log("⑳ 反面：她灰著、而且真的是叫不起來的時候，那一行不可以說「另一件事」");
{
  // ⑲ 的修法最容易做成「一律加前綴」，那就在另一個方向上說謊：這一次那句話
  // **就是**在講她為什麼沒起來，前綴會把唯一那條線索推開。三個會寫「叫不起
  // 來」的地方都先關掉 `starting` 才重畫，⑲ 那條路靠的就是這件事——這一條驗
  // 的是它還成立。
  const p = await open({ start_recording: new Error(CONSENT), recording_state: "none" });
  await p.click("[data-wake]");
  // 前提就是那件讓 ⑲ 成立的事：寫「叫不起來」的人先關掉了 `starting`。
  check("已經不是「正在把她叫起來」了", !p.line().includes("正在把她叫起來"), p.line());
  check("同意書那句在", p.line().includes("同意書"), p.line());
  check("而且它就是在講她——不可以推開", !p.line().includes("另一件事"), p.line());
}

console.log("㉑ 她以前暫停過，不可以把「我現在就是瞎的」那句話吃掉");
{
  // `renderBlind` 裡「我現在是暫停的」掛在 `else if (blind.paused_now)`，所以
  // 只有一段暫停紀錄都沒有的時候才說得出口。錄的時候暫停又解除過一次、後來
  // 在沒有人在錄的時候又按了一次暫停的人，讀到的是
  //
  //     我也被暫停過 1 次，那幾段是空的。
  //
  // 過去式，話說完了。而他此刻是瞎的——下一次按開始記錄會錄一整天的空白。
  // `ops.rs` 的 `blind_lines` 是同一個 bug（那邊那條 else 的註解自己寫著
  // 「這一條比上面那條更需要講」，然後坐在會被擋掉的位置上）。
  const p = await open({
    recording_state: "recording",
    ask: answer({
      blind: blind({
        ever_recorded: true,
        ever_stored: true,
        chunks: 10,
        paused_episodes: 1,
        paused_ms: 300_000,
        paused_now: true,
      }),
    }),
  });
  await p.type("上禮拜那通電話");
  const said = p.hitTexts().join("\n");
  check("以前那幾段還是要講", said.includes("被暫停過 1 次"), said);
  check("而現在瞎著才是要命的那一句", said.includes("我現在是暫停的"), said);
  check("沒有 undefined 混進去", p.nonsense().length === 0, p.nonsense());
}

console.log("㉒ 反面：她現在沒有暫停的時候，不可以憑空多一句說她瞎著");
{
  const p = await open({
    recording_state: "recording",
    ask: answer({
      blind: blind({
        ever_recorded: true,
        ever_stored: true,
        chunks: 10,
        paused_episodes: 1,
        paused_ms: 300_000,
        paused_now: false,
      }),
    }),
  });
  await p.type("上禮拜那通電話");
  const said = p.hitTexts().join("\n");
  check("以前那幾段要講", said.includes("被暫停過 1 次"), said);
  check("但不可以說她現在是暫停的", !said.includes("我現在是暫停的"), said);
}

console.log("㉓ `booting` 那幾分鐘從系統匣按下去，回來那句話**就是**在講她");
{
  // ⑲ 那個修法的反面，而且是危險的那一面。alpha.41 的第一版拿「她正在起來」
  // 當「所以這句話一定不是在講她」的證據，然後在 `booting` 整段時間裡對每一句
  // `recorder-failed` 貼上「這是另一件事：」。那個推論在這裡是**反的**：
  //
  //   · `booting` 的時候 `recording_state` 不是 `"none"`；
  //   · 系統匣那顆 handler 讀真相不讀標籤（`main.rs`：`let on = recording_state
  //     (...) != "none"`），所以它送的是 `stop_recording`；
  //   · 於是 `booting` 期間唯一送得出 `recorder-failed` 的路**就是停不掉**
  //     （`main.rs` 那一行是全 repo 唯一的 emitter）。
  //
  // 也就是說：那個前綴在這一格 100% 是假的，而它推開的是唯一一句「你按的停止
  // 沒有生效」。上面那行還寫著「這期間還沒開始記」——兩句合起來讀成「她好好
  // 的，另外有個檔案權限問題」，他走開，她開完資料庫錄一整天。
  const STOP_FAILED =
    "write C:\\Users\\ted\\AppData\\Roaming\\ted-h\\AI-Sister\\data\\stop.request: Access is denied. (os error 5)";
  const p = await open({ recording_state: "booting" });
  check("前提：她正在開資料庫", p.line().includes("正在開資料庫"), p.line());
  await p.fromOutside("recorder-failed", STOP_FAILED);
  check("那句話要在", p.line().includes("Access is denied"), p.line());
  // **前提要在事後再驗一次。** 少了這一條，任何一個把 `booting` 弄丟的改動都
  // 會讓底下那句「沒有前綴」為了錯的理由過關——不是 booting 了，本來就不加。
  check("而且她還在 booting（不然下面那條等於沒驗）", p.line().includes("正在開資料庫"), p.line());
  const detail = p.line().split("\n")[1] ?? "";
  check("不可以被推開成「另一件事」——他按的停止沒生效，那正是在講她", !detail.includes("另一件事"), detail);
}

console.log("㉔ 叫她起來的中途又從系統匣按了一次，不可以當場宣告她沒起來");
{
  // 他按了「叫她起來」，沒耐心，又從系統匣按一次「開始記錄」。那一刻心跳還沒
  // 蓋出來，`recording_state` 還是 `"none"`，所以那一顆走 `start_recording`、
  // 撞上 `spawned.try_wait()` 回 `Ok(None)`，回一句「還在起來，再等一下」。
  //
  // `recorder-failed` 那個 listener 以前無條件 `starting = false`，於是：
  //
  //     沒有人在記錄——從現在起發生的事，她不會知道
  //     上一次按的那個還在起來——…再等一下
  //
  // 兩行都是真的，而它們直接互相矛盾。而且「叫她起來」那顆會跟著跳回來，他再
  // 按一次拿到同一句話——正是 `booting` 三態當初要消滅的那個迴圈。
  const STILL = "上一次按的那個還在起來——第一次開資料庫要重建索引，大的資料庫可能要幾分鐘。再等一下";
  const p = await open({
    recording_state: "none",
    start_recording: () => Promise.resolve(null),
  });
  await p.click("[data-wake]");
  check("前提：她正在起來", p.line().includes("正在把她叫起來"), p.line());
  await p.fromOutside("recorder-failed", STILL);
  check("那句話要在", p.line().includes("再等一下"), p.line());
  check("而上面那行不可以翻成「沒有人在記錄」", p.line().includes("正在把她叫起來"), p.line());
  check("「叫她起來」那顆也不可以跳回來", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);
  // 這一句**就是**在講她（系統匣送的不是 start 就是 stop），所以不加前綴。
  const detail = p.line().split("\n")[1] ?? "";
  check("而且它不是「另一件事」", !detail.includes("另一件事"), detail);
}

console.log("㉖ 她好好地在錄的時候，那句前綴是純噪音——不可以出現");
{
  // 前綴存在的唯一理由，是「上面那行講的是一個**還沒完成的轉換**，所以底下
  // 那行會被讀成它的原因」。她已經在錄了的時候上面那行是「在聽」，沒有因果
  // 可以誤讀；而一句到處都貼的「這是另一件事」，會在真正需要它的那一格失去
  // 分量。少了這一條，`(starting || booting) && !aboutHer` 砍成 `!aboutHer`
  // 全綠——這是 ㉓/㉔ 寫完之後跑突變才發現的洞。
  const OOPS = "資料庫打不開：database is locked";
  const p = await open({ recording_state: "recording", ask: new Error(OOPS) });
  await p.type("剛剛發生什麼事");
  check("前提：她在錄，不是正在起來", p.line().startsWith("在聽"), p.line());
  check("那句錯誤要在", p.line().includes(OOPS), p.line());
  check("但不加前綴", !p.line().includes("另一件事"), p.line());
}

console.log("㉕ `notice` 只有那兩個具名函式寫得進去");
{
  // `paint()` 讀的是 `notice.text` / `notice.aboutHer`。哪天有人寫回一句裸字串
  // （這一頁沒有型別可以擋，而 alpha.41 之前那七個寫入點全是裸字串），畫面上
  // 會是「undefined」——沒有例外、沒有紅字，只有一格看起來像壞掉的字。
  //
  // 更重要的是**語意**那一半：新的寫入點不經過那兩個函式，就等於沒有人回答過
  // 「這句話在講誰」，而 `paint()` 會替它猜——那正是這一輪修掉的東西。
  const src = read(join(UI, "app.js"));
  const writes = [...src.matchAll(/^\s*notice = (.+)$/gm)].map((m) => m[1].trim());
  const stray = writes.filter((w) => w !== "null;" && !w.startsWith("{ text:"));
  check("每一句 notice = 不是 null 就是那兩個函式裡的物件", stray.length === 0, stray);
  // 活體：正規表示式挑不到東西的話上面那條永遠綠，而它守的線是一格空白。
  check(`而且真的掃到了（${writes.length} 句）`, writes.length >= 5, writes.length);
  const setters = src.match(/^function noticeAbout\w+\(/gm) ?? [];
  check("那兩個具名函式也還在", setters.length === 2, setters);
}

console.log("㉗ `?asleep=nobeat` 這條 demo 路徑，在 booting 上也不可以被推開");
{
  // 他在**這台機器**上看這幾格畫面只有一條路：那幾個 `?…` 網址。Tauri 開不
  // 起來，所以「等了 25 秒還沒有心跳」那句話長什麼樣、換行撐不撐得住版面，
  // 是靠那條路用眼睛看的。而那條路以前一次測試都沒走過（`location.search`
  // 在這支腳本裡是寫死的 `""`）。
  //
  // 這一條驗的是它和 ㉓ 的交叉：`?state=booting` 讓她停在「正在開資料庫」，
  // 而 `?asleep=nobeat` 那句話講的正是她起不來。兩個湊在一起的時候，那句話
  // 前面不可以冒出「這是另一件事」——那會讓他在一台**真的沒起來**的機器上，
  // 讀到一句叫他別在意的話。
  const p = await open(
    { recording_state: "booting" },
    { search: "?state=booting&asleep=nobeat" },
  );
  check("前提：她停在正在開資料庫", p.line().includes("正在開資料庫"), p.line());
  check("那句話要在", p.line().includes("等了 25 秒還沒有心跳"), p.line());
  const detail = p.line().split("\n").slice(1).join("\n");
  check("不可以被推開成「另一件事」", !detail.includes("另一件事"), detail);
  // 這條 demo 路徑餵的是三行字。少一行就代表 app.js 那邊被改短了，而它撐不撐
  // 得住版面正是他要看的東西——「有出現」不等於「整段都在」。
  check("三行都還在", detail.split("\n").length === 3, detail.split("\n").length);
}

console.log("㉘ 錄製已停、腦還在想最後一段：不是「在聽」，也不是「沒有人在記錄」");
{
  // 按下停止之後那兩分鐘。心跳說沒在錄（她不抓畫面了），行程還握著資料庫。
  // 畫面若走 `"none"`，他會按開始，然後撞上佔著閘門——兩句對打。
  const p = await open({ recording_state: "thinking" });
  check("說她在想最後一段", p.line().includes("想最後一段"), p.line());
  check("不可以說在聽", !p.line().includes("在聽"), p.line());
  check("不可以說沒有人在記錄", !p.line().includes("沒有人在記錄"), p.line());
  const wake = p.node("[data-wake]");
  check("開始鍵要藏起來——按下去只會撞上佔著閘門", wake.hidden === true, `hidden=${wake.hidden}`);
  await p.repaint();
  check("輪詢過後還是同一句", p.line().includes("想最後一段"), p.line());
}

console.log("㉙ 錄過但一列都沒存：指到 doctor，不指不存在的設定段落");
{
  const p = await open({
    recording_state: "none",
    ask: answer({ blind: blind({ ever_recorded: true, ever_stored: false }) }),
  });
  await p.type("剛剛發生什麼事");
  const text = p.hitTexts().join("\n");
  check("下一步是 sister doctor", text.includes("sister doctor"), text);
  check("不再指向設定頁的開始記錄段落", !text.includes("設定頁的「開始記錄」"), text);
}

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——字母人上有話說不出口，或說了活不過下一次輪詢。`);
  process.exit(1);
}
console.log("✓ 那幾句「為什麼沒成」活得過輪詢，而且下一個動作蓋得掉");
// 每個 module instance 都留著自己那顆 5 秒輪詢的 setInterval（`visibilityState`
// 是 visible，那正是要驗的東西），所以要自己走。
process.exit(0);
