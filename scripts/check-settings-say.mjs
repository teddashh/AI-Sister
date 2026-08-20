#!/usr/bin/env node
/*
 * 設定頁按下「儲存」之後，那一格到底有沒有說話。
 *
 * 這一支存在的理由：`save()` 成功的時候會講一句話，然後呼叫 `load()` 把剛
 * 寫進去的那一份讀回來畫上去——而 `load()` 成功的路徑上有一行 `say("")`。
 * 兩行各自都對（重讀一份新的不該留著上一件事的結果；存完要把剪過空白的版本
 * 畫回去），湊起來這一頁從出生到 alpha.37 為止，**每一句「存好了」都在幾毫秒
 * 後被自己抹成空白**。失敗那條路反而活得好好的（`catch` 走不到 `load()`），
 * 所以症狀是：存壞了會說話，存好了什麼都不說。對按下按鈕的人來說，沉默就是
 * 「按了沒反應」。
 *
 * 為什麼是這種寫法：它載入的是 `apps/desktop/ui/settings.js` **原檔**，不是
 * 一份抄過來的邏輯。抄過來的那種測試只證明抄本會動。假 DOM 只有幾十行，夠
 * 這一頁跑完 load / save 一輪。
 *
 * 為什麼要 CI 顧：這一頁沒有任何自動測試，開發機也開不起 Tauri 視窗，所以
 * 這一整類「畫面說了什麼」的錯，一直是靠 Ted 在 Windows 上用眼睛抓。這一條
 * 抓得到的那一種，不必等到他那邊。
 */

import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "settings.js");

// settings.js 沒有 import/export，Node 會把它當 CJS 載入，而 CJS 的 require
// cache 只認檔名——`?v=1` 這種 query 繞不開它。所以每個 case 把原始碼原封不動
// 抄到一個新檔名底下再載入，才拿得到乾淨的 module instance。
const SOURCE = readFileSync(SRC, "utf8");
const TMP = mkdtempSync(join(tmpdir(), "sister-ui-"));

function fakeEl() {
  const el = {
    _text: "",
    value: "",
    checked: false,
    disabled: false,
    placeholder: "",
    hidden: false,
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

const BASE = {
  path: "C:\\Users\\ted\\AppData\\Roaming\\sister\\config.toml",
  excluded_apps: ["keepassxc"],
  excluded_urls: ["*.bank.example"],
  excluded_titles: [],
  pause_on_screenshare: true,
  redact_clipboard_secrets: true,
  query_log: true,
  frames_days: 14,
  text_days: 90,
};

const tick = () => new Promise((r) => setTimeout(r, 20));
let instances = 0;

async function open({ config = BASE, onRead, onWrite } = {}) {
  const nodes = new Map();
  const node = (sel) => {
    if (!nodes.has(sel)) nodes.set(sel, fakeEl());
    return nodes.get(sel);
  };
  globalThis.document = {
    querySelector: node,
    querySelectorAll: () => [],
    createElement: () => fakeEl(),
    addEventListener() {},
    body: fakeEl(),
  };
  globalThis.location = { search: "" };
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};

  let state = { ...config };
  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, arg) => {
        switch (cmd) {
          case "settings_read":
            if (onRead) return onRead(state);
            return { ...state };
          case "settings_write":
            if (onWrite) return onWrite(arg.settings, (s) => (state = { ...state, ...s }));
            state = { ...state, ...arg.settings };
            return { watching: "recording" };
          case "lint_url_rules":
            return [];
          case "privacy_health":
            return { url_rules: { kind: "working", reads: 12 }, at: 1_755_000_000_000 };
          case "hotkey_state":
            return { kind: "held", combo: "Ctrl + Alt + P" };
          default:
            return null;
        }
      },
    },
  };

  const copy = join(TMP, `settings-${++instances}.js`);
  writeFileSync(copy, SOURCE);
  await import(pathToFileURL(copy).href);
  await tick();
  return {
    node,
    say: () => node("[data-say]").textContent,
    bad: () => node("[data-say]").classList.contains("bad"),
    async save() {
      for (const fn of node("[data-save]").handlers.click ?? []) fn();
      await tick();
    },
  };
}

// 「先前記下的那些不會消失」那一句的指紋。
const KEPT = "從現在起她不會再記你問過的問題";

let failed = 0;
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

console.log("① 題庫本來開著，關掉按儲存");
{
  const p = await open();
  p.node("[data-querylog]").checked = false;
  await p.save();
  check("那一格不是空的", p.say() !== "", p.say());
  check("說了「存好了」", p.say().includes("存好了"), p.say());
  check("說了「先前記下的那些不會消失」", p.say().includes(KEPT), p.say());
  check("是兩行，不是黏成一長條", p.say().split("\n").length === 2, p.say());

  // 存完 `load()` 會把 `queryLogWas` 換成剛存進去的那一份，所以第二次按下去
  // 沒有人「剛剛關掉」任何東西。少了這一條，那句話會變成每次儲存都出現的
  // 背景雜訊——而它是一句只在那一下成立的話。
  console.log("② 同一頁上，關著的狀態下再按一次儲存");
  await p.save();
  check("還是說了「存好了」", p.say().includes("存好了"), p.say());
  check("沒有再講一次「先前記下的那些」", !p.say().includes(KEPT), p.say());
}

console.log("③ 題庫一直開著，按儲存");
{
  const p = await open();
  await p.save();
  check("說了「存好了」", p.say().includes("存好了"), p.say());
  check("沒有那句多的", !p.say().includes(KEPT), p.say());
}

console.log("④ 題庫本來關著，打開按儲存");
{
  const p = await open({ config: { ...BASE, query_log: false } });
  p.node("[data-querylog]").checked = true;
  await p.save();
  check("說了「存好了」", p.say().includes("存好了"), p.say());
  check("沒有那句多的", !p.say().includes(KEPT), p.say());
}

// 寫進去了、再讀出來卻讀不出來，多半是我們剛剛把那個檔寫壞了。那件事比
// 「存好了」急，而且「存好了」在那個當下已經不是一句完整的真話。
console.log("⑤ 存成功，但存完那次重讀炸了");
{
  let reads = 0;
  const p = await open({
    onRead: (s) => {
      if (++reads > 1) throw new Error("retention.frames_days 不能是 0");
      return { ...s };
    },
  });
  await p.save();
  check("留著那則解析錯誤", p.say().includes("frames_days"), p.say());
  check("沒有被「存好了」蓋掉", !p.say().includes("存好了"), p.say());
  check("而且是紅的", p.bad(), p.bad());
}

console.log("⑥ 存不進去");
{
  const p = await open({
    onWrite: () => {
      throw new Error("寫不進去：拒絕存取");
    },
  });
  await p.save();
  check("留著那則寫入錯誤", p.say().includes("拒絕存取"), p.say());
  check("而且是紅的", p.bad(), p.bad());
}

// 三向那句話（#50）也是被同一行抹掉的，所以它也要有人顧。
console.log("⑦ 沒有人在錄的時候按儲存");
{
  const p = await open({
    onWrite: (s, commit) => {
      commit(s);
      return { watching: "idle" };
    },
  });
  await p.save();
  check("留著「等你按下『開始記錄』才會生效」", p.say().includes("開始記錄"), p.say());
}

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——設定頁在某一種情況下對按下按鈕的人什麼都沒說。`);
  process.exit(1);
}
console.log("✓ 設定頁按下儲存之後，成功和失敗都說得出話");
