/*
 * 三支畫面測試共用的那幾十行：一個夠跑一頁的假 DOM，加上「怎麼把同一個
 * .js 檔載入兩次」。
 *
 * 為什麼是共用的：這裡面每一條都是被打臉打出來的，而三份抄本代表下一次
 * 只會有一份被修好。
 *
 * - `hidden` 的預設值要從 index.html 讀，不能寫死 `false`。第一版寫死了，
 *   於是那條測試在**真的壞掉的** app.js 上照樣綠——一個假 DOM 只要有一個
 *   欄位比真的寬鬆，它守的那條線就是假的。
 * - `textContent` 要 `configurable`，不然第二個 case 重新定義會炸。
 * - 載入同一個檔案第二次拿不到新的 module：那幾個 ui/*.js 沒有 import /
 *   export，Node 當 CJS 載入，而 CJS 的 cache 只認**檔名**——`?v=2` 這種
 *   query 繞不開它。要抄到一個新檔名底下。這一條錯掉的時候症狀是「第二個
 *   case 之後 handlers 全是空的」，很容易被誤讀成產品的錯。
 */

import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/** 一個夠跑一頁的假元素。缺什麼就往這裡加，不要在呼叫端補丁。 */
export function fakeEl(tag = "div") {
  const el = {
    tag,
    _text: "",
    type: "",
    value: "",
    className: "",
    checked: false,
    disabled: false,
    placeholder: "",
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
    setAttribute(name, v) {
      this.dataset[name] = String(v);
    },
    removeAttribute(name) {
      delete this.dataset[name];
    },
    focus() {},
    scrollIntoView() {},
    get childElementCount() {
      return this.children.length;
    },
    /** 只認得 `"button"` 這種標籤選擇器，而且是**遞迴**的——`querySelectorAll` 本來就是。 */
    querySelectorAll(sel) {
      const out = [];
      const walk = (node) => {
        for (const kid of node.children ?? []) {
          if (kid.tag === sel) out.push(kid);
          walk(kid);
        }
      };
      walk(this);
      return out;
    },
    querySelector(sel) {
      return this.querySelectorAll(sel)[0] ?? null;
    },
  };
  Object.defineProperty(el, "textContent", {
    configurable: true,
    get() {
      return this._text;
    },
    set(v) {
      this._text = String(v);
    },
  });
  return el;
}

/**
 * 那個選擇器指到的東西，在真的 HTML 上是不是 `hidden` 的。
 *
 * 開場狀態要跟真的一樣。這一條的反例是 `[data-hits]`：HTML 上寫著 `hidden`，
 * 假 DOM 預設 `false`，於是「第一題就失敗的人整個畫面一個字都不會多」這個
 * bug 在測試裡看不見。
 */
export function hiddenIn(html, sel) {
  const attr = sel.startsWith("#") ? `id="${sel.slice(1)}"` : sel.replace(/[[\]]/g, "");
  const tag = html.match(new RegExp(`<[^>]*${attr.replace(/[.*+?^${}()|]/g, "\\$&")}[^>]*>`));
  return tag !== null && /\shidden(\s|>|=)/.test(tag[0]);
}

/**
 * 把一份 ui/*.js 載入成一個**全新的** module instance。
 *
 * 每叫一次抄一個新檔名。理由見檔案開頭那段 CJS cache 的說明——這是這幾支
 * 測試裡最容易錯、而且錯了會安靜地讓測試變綠的一件事。
 */
export function loader(source) {
  const dir = mkdtempSync(join(tmpdir(), "sister-ui-"));
  let n = 0;
  return async () => {
    const copy = join(dir, `page-${++n}.js`);
    writeFileSync(copy, source);
    await import(pathToFileURL(copy).href);
  };
}

export const read = (path) => readFileSync(path, "utf8");
