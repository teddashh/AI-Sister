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

import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "app.js");

// app.js 沒有 import/export，Node 當 CJS 載入，而 CJS 的 cache 只認檔名。
// 每個 case 抄一份新檔名才拿得到乾淨的 module instance。
const SOURCE = readFileSync(SRC, "utf8");
const TMP = mkdtempSync(join(tmpdir(), "sister-pet-"));

function fakeEl() {
  const el = {
    _text: "",
    value: "",
    checked: false,
    disabled: false,
    hidden: false,
    title: "",
    dataset: {},
    style: { setProperty() {}, removeProperty() {} },
    children: [],
    handlers: {},
    classList: {
      _s: new Set(),
      add(...c) {
        c.forEach((x) => this._s.add(x));
      },
      remove(...c) {
        c.forEach((x) => this._s.delete(x));
      },
      toggle(c, on) {
        if (on) this._s.add(c);
        else this._s.delete(c);
      },
      contains(c) {
        return this._s.has(c);
      },
    },
    addEventListener(ev, fn) {
      (this.handlers[ev] ??= []).push(fn);
    },
    removeEventListener() {},
    append(...kids) {
      this.children.push(...kids);
    },
    appendChild(k) {
      this.children.push(k);
      return k;
    },
    replaceChildren(...kids) {
      this.children = kids;
    },
    setAttribute() {},
    removeAttribute() {},
    focus() {},
    scrollIntoView() {},
    querySelector() {
      return null;
    },
    querySelectorAll() {
      return [];
    },
  };
  Object.defineProperty(el, "textContent", {
    get() {
      return this._text;
    },
    set(v) {
      this._text = String(v);
    },
  });
  return el;
}

/*
 * 開場的 `hidden` 要跟 index.html 一樣，不能跟著假 DOM 的預設值走。
 *
 * 這一段是被打臉逼出來的：`[data-hits]` 在 HTML 上寫著 `hidden`，而
 * `fakeEl()` 的預設是 `false`——照預設值寫的那一版測試，在**真的壞掉的**
 * app.js 上照樣綠。一個假 DOM 只要有一個欄位比真的寬鬆，它守的那條線就是
 * 假的。
 */
const HTML = readFileSync(join(UI, "index.html"), "utf8");

function tagOf(sel) {
  const attr = sel.startsWith("#") ? `id="${sel.slice(1)}"` : sel.replace(/[[\]]/g, "");
  const m = HTML.match(new RegExp(`<[^>]*${attr.replace(/[.*+?^${}()|]/g, "\\$&")}[^>]*>`));
  return m?.[0] ?? null;
}

function hiddenInHtml(sel) {
  const tag = tagOf(sel);
  return tag !== null && /\shidden(\s|>|=)/.test(tag);
}

// 前提本身也要驗一次。哪天 index.html 把那個 `hidden` 拿掉，這幾條測試會
// 悄悄變成「驗一個不存在的問題」——寧可在這裡就吵。
if (!hiddenInHtml("[data-hits]")) {
  console.log("✗ index.html 上的 [data-hits] 已經不是 hidden 了——底下那條測試的前提沒了");
  process.exit(1);
}

const tick = (ms = 20) => new Promise((r) => setTimeout(r, ms));
let instances = 0;

/**
 * 開一次字母人。`invoke` 收一張 `{ 指令: 回傳值或會丟出來的 Error }` 表；
 * 沒列到的指令回 `null`。函式值會被呼叫（要延遲、要丟例外的用這個）。
 */
async function open(table = {}) {
  const nodes = new Map();
  const node = (sel) => {
    if (!nodes.has(sel)) {
      const el = fakeEl();
      el.hidden = hiddenInHtml(sel);
      nodes.set(sel, el);
    }
    return nodes.get(sel);
  };
  const listeners = new Map();

  globalThis.document = {
    querySelector: node,
    querySelectorAll: () => [],
    createElement: () => fakeEl(),
    addEventListener() {},
    body: fakeEl(),
    documentElement: fakeEl(),
    // **要是 visible。** 開場那一段對 `recording` 寫死的是 `"recording"`，
    // 只有 `updatePollGate()` 看到視窗是開著的才會去問一次磁碟；hidden 的話
    // 這一頁會停在「她在錄」，而灰掉那條路上的話（`wakeFailed`）就永遠不會
    // 被畫出來——測試會綠，但綠的理由是它根本沒走到那裡。
    visibilityState: "visible",
  };
  globalThis.location = { search: "" };
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

  const copy = join(TMP, `app-${++instances}.js`);
  writeFileSync(copy, SOURCE);
  await import(pathToFileURL(copy).href);
  await tick();
  return {
    node,
    line: () => node("[data-state-line]").textContent,
    hits: () => node("[data-hits]"),
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
    ask: () => new Promise((r) => setTimeout(() => r({ hits: [], kind: "none" }), 5000)),
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
    ask: { hits: [], kind: "none", answers: [], blind: [], searched: [] },
    recording_state: "recording",
  });
  await p.click("#pause");
  check("先有那句話", p.line().includes("暫停鍵沒有作用"), p.line());
  await p.type("剛剛發生什麼事");
  check("問了下一題就不該再掛著", !p.line().includes("暫停鍵沒有作用"), p.line());
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
