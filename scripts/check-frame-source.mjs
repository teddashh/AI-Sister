#!/usr/bin/env node
/*
 * 「當時的畫面」那個視窗：**表頭上那兩格，只能替真的看到的那張圖背書。**
 *
 * 這一頁是整個產品唯一一個「查證」的介面。她說「你三天前看過這個」，他點下
 * 去，看到的就是這一頁——所以這一頁上每一種說謊的方式都特別貴：
 *
 *   一、圖好好的，出處三格全是 NULL，那一行就是一片空白。空白在這裡有兩個
 *       意思（「沒有留下是哪個視窗」跟「這個程式忘了填」），而他分不出來。
 *       這不是假設：`insert_frame` 把 `frame.focus` 那三格直接寫下去，鎖定
 *       畫面和 UAC 那層黑底就是三格全空，而圖照樣留下來了。
 *
 *   二、圖打不開，但表頭寫得好好的。`fs::read` 對半截的檔案照樣成功，壞在
 *       瀏覽器解碼那一步——而那一步**非同步**，`show()` 的 try/catch 接不到。
 *       畫面上於是是一個破圖圖示，配上一行言之鑿鑿的時間和出處。
 *
 *   三、網址上沒有 id，卻去問了第 0 張。`Number(null)` 是 0，`Number("")` 也
 *       是 0，而 `Number.isInteger(0)` 是真的——於是「連結建錯了」被講成
 *       「找不到這張畫面」，他會去翻保留期設定找一個不存在的 bug。
 *
 * 所以這支測試裡的假 `<img>` 有一個真的 `src` setter：指派是同步的，成功或
 * 失敗都要等下一個 task。時序抄錯的話第二條根本不會發生，而測試會綠。
 */

import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { fakeEl, hiddenIn, loader, read } from "./fake-dom.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "frame.js");
const HTML = read(join(UI, "frame.html"));
const boot = loader(read(SRC));

let failed = 0;
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

// 開場狀態要跟真的 HTML 一樣，不然「圖藏起來了」這種斷言是憑空的。
for (const sel of ["[data-shot]", "[data-trouble]"]) {
  if (!hiddenIn(HTML, sel)) {
    console.log(`✗ 前提不成立：frame.html 上的 ${sel} 沒有 hidden，這支測試守的線是假的`);
    process.exit(1);
  }
}

/** `FrameView` 的形狀，照 main.rs 那個 struct 抄的（`app`/`title`/`url` 都是 Option）。 */
function frameView(over = {}) {
  return {
    data_url: "data:image/png;base64,AAAA",
    ts: 1_755_000_000_000,
    app: "chrome.exe",
    title: "帳單查詢",
    url: "https://example.com/bill",
    ...over,
  };
}

const tick = () => new Promise((r) => setTimeout(r, 20));

/**
 * 一個會像真的 `<img>` 那樣壞掉的假元素。
 *
 * `src` 指派是**同步**的，解碼失敗是**下一個 task** 才通知的。這支測試的第
 * 二條就是靠這個時序才存在——寫成同步 throw 的話，`show()` 的 try/catch 會
 * 接住它，整條路徑就變成一個這個產品裡不存在的東西。
 */
function fakeImg(decodes) {
  const el = fakeEl("img");
  let src = "";
  Object.defineProperty(el, "src", {
    configurable: true,
    get: () => src,
    set(v) {
      src = String(v);
      if (decodes(src)) return;
      setTimeout(() => {
        for (const fn of el.handlers.error ?? []) fn();
      }, 0);
    },
  });
  return el;
}

async function open({ search = "?id=57", view = frameView(), fail = null, decodes = () => true } = {}) {
  const nodes = new Map();
  const asked = [];
  const node = (sel) => {
    if (!nodes.has(sel)) {
      const el = sel === "[data-shot]" ? fakeImg(decodes) : fakeEl();
      // **只在生出來的時候套一次。** 每次查詢都套的話，這個查詢函式會把畫面
      // 洗回開場狀態——而斷言正是透過它讀畫面的，於是每一條都在看開場。
      el.hidden = hiddenIn(HTML, sel);
      nodes.set(sel, el);
    }
    return nodes.get(sel);
  };

  globalThis.document = { querySelector: node, querySelectorAll: () => [], body: fakeEl() };
  globalThis.location = { search };
  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, arg) => {
        asked.push([cmd, arg]);
        if (fail) throw new Error(fail);
        return view;
      },
    },
  };

  await boot();
  await tick();
  return {
    asked,
    when: () => node("[data-when]").textContent,
    where: () => node("[data-where]").textContent,
    unknown: () => node("[data-where]").classList.contains("unknown"),
    trouble: () => (node("[data-trouble]").hidden ? null : node("[data-trouble]").textContent),
    shown: () => node("[data-shot]").hidden === false,
  };
}

console.log("① 一般狀態：圖、時間、出處都在");
{
  const p = await open();
  check("圖看得到", p.shown(), p.shown());
  check("沒有錯誤訊息", p.trouble() === null, p.trouble());
  check("時間寫出來了", p.when() !== "", p.when());
  check("出處三段都在", p.where() === "chrome.exe — 帳單查詢 — https://example.com/bill", p.where());
  check("而且不是那句「沒有留下」", !p.unknown(), p.unknown());
}

console.log("② 圖留下了，但當時問不出是哪個視窗（鎖定畫面、UAC）");
{
  const p = await open({ view: frameView({ app: null, title: null, url: null }) });
  check("圖照樣看得到——這不是錯誤", p.shown(), p.shown());
  // 這裡是這支測試存在的第一個理由。
  check("出處那一格不可以是空白", p.where() !== "", p.where());
  check("它要說得出「沒有留下」", p.where().includes("沒有留下"), p.where());
  check("而且看得出來是這一頁在說話，不是視窗標題", p.unknown(), p.unknown());
  check("時間照樣寫出來", p.when() !== "", p.when());
}

console.log("③ 這一筆沒有留下畫面，只有文字（Rust 那邊給的話）");
{
  const p = await open({ fail: "這一筆沒有留下畫面，只有文字" });
  check("原話照抄", p.trouble() === "這一筆沒有留下畫面，只有文字", p.trouble());
  check("圖藏起來", !p.shown(), p.shown());
  check("表頭清乾淨", p.when() === "" && p.where() === "", [p.when(), p.where()]);
}

console.log("④ 網址上沒有 id");
{
  const p = await open({ search: "" });
  check("說的是「沒有指定」", p.trouble() === "沒有指定是哪一張畫面。", p.trouble());
  // `Number(null) === 0`。這一條沒守住的話，上面那句會變成「找不到這張畫面」
  // ——換了一個診斷，而他會照著那個診斷去查一個沒有壞的東西。
  check("而且根本沒去問第 0 張", p.asked.length === 0, p.asked);
}

console.log("⑤ 網址上的 id 是空的（?id=）");
{
  const p = await open({ search: "?id=" });
  check("一樣是「沒有指定」", p.trouble() === "沒有指定是哪一張畫面。", p.trouble());
  check("一樣沒去問", p.asked.length === 0, p.asked);
}

console.log("⑥ 檔案讀得出來，但那是半截的圖（解碼失敗）");
{
  const p = await open({ decodes: () => false });
  check("有話說", p.trouble() !== null, p.trouble());
  check("說得出是檔案打不開，不是圖不見了", p.trouble()?.includes("打不開"), p.trouble());
  check("破圖要藏起來", !p.shown(), p.shown());
  // 這裡是第二個理由：表頭在解碼失敗**之前**就寫滿了，所以「只清時間」不夠。
  check("時間要清掉", p.when() === "", p.when());
  check("出處也要清掉——它正在替一張沒打開的圖背書", p.where() === "", p.where());
}

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——那個視窗的表頭正在替它沒有真的看到的東西背書。`);
  process.exit(1);
}
console.log("✓ 表頭上那兩格，只替真的看到的那張圖說話");
