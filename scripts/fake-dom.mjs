/*
 * 五支畫面測試共用的那幾十行：一個夠跑一頁的假 DOM，加上「怎麼把同一個
 * .js 檔載入兩次」。
 *
 * 為什麼是共用的：這裡面每一條都是被打臉打出來的，而五份抄本代表下一次
 * 只會有一份被修好。
 *
 * - `hidden` 的預設值要從 index.html 讀，不能寫死 `false`。第一版寫死了，
 *   於是那條測試在**真的壞掉的** app.js 上照樣綠——一個假 DOM 只要有一個
 *   欄位比真的寬鬆，它守的那條線就是假的。
 * - 同一條的下一半：假 DOM 不可以**生出 HTML 上沒有的元素**。第一版的
 *   `node(sel)` 對任何選擇器都回一個 `fakeEl()`，於是「『重新讀取』沒有跟著
 *   被灰掉」那條斷言，在那顆按鈕被從 HTML 刪掉的時候照樣綠——而它是那一頁
 *   唯一的出口。用 `domOf(html)`，別自己寫 `nodes.get(sel) ?? fakeEl()`。
 * - 假資料的**形狀**要照著 Rust 那邊的 struct 抄。抄漏一欄不會有人報錯：
 *   `check-timeline-forget.mjs` 的預覽用 `bytes`、真的叫 `image_bytes`，畫面
 *   於是印出「5 張畫面（NaN MB）」而那條斷言（`includes("刪掉了")`）照樣綠。
 *   `watchNonsense()` 就是為了這個——任何一次寫進畫面的 `NaN` /
 *   `undefined` / `[object Object]` 都要當場算輸，不必事先知道哪一欄會漏。
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
      // 真的 DOM 回的是**整棵子樹**的字，不是只有直接寫進去的那一段。少了
      // 後半截，一個用 `append` 組出來的句子在測試裡讀起來是空的。
      return this._text + this.children.map((k) => k.textContent).join("");
    },
    set(v) {
      const text = String(v);
      if (NONSENSE.test(text)) nonsense.push(`<${this.tag}> ${text}`);
      // 指派 `textContent` 會**清掉所有子節點**。以前只換那一段字，於是一個
      // 重畫過的容器在測試裡還掛著上一次的孩子。
      this.children = [];
      this._text = text;
    },
  });
  return el;
}

/** 一個文字節點。`renderSnippet` 每一筆命中都會生好幾個。 */
export function fakeText(data) {
  const node = fakeEl("#text");
  node.textContent = String(data);
  return node;
}

/**
 * 一個夠跑一頁的 `document`。
 *
 * 為什麼要共用：五支測試以前各自手捏一個，而**各自漏掉不一樣的東西**。
 * `createTextNode` 就是這樣漏的——五支裡一支都沒有，而 `renderSnippet`（每一
 * 筆命中都會走）第一行就在用它。少了它，那個 TypeError 被 `ask()` 的 catch
 * 接住，於是「答成了」和「答不成」在測試裡長得一模一樣（兩條路都會把
 * `hitList.hidden` 設成 false）。第一條真的去渲染一筆命中的斷言，就這樣紅在
 * 一個沒有壞的產品上。
 *
 * 假 DOM 缺一個方法，和假資料少一個欄位，是同一種錯：產品被推到一條它平常
 * 不會走的路上，而測試看到的是那條路的結果。
 */
export function fakeDocument(querySelector, extra = {}) {
  return {
    querySelector,
    querySelectorAll: () => [],
    createElement: (tag) => fakeEl(tag),
    createTextNode: (data) => fakeText(data),
    addEventListener() {},
    body: fakeEl("body"),
    documentElement: fakeEl("html"),
    ...extra,
  };
}

/*
 * 沒有一句給人看的話裡面該有這三個字串。
 *
 * 它們是**假資料形狀錯掉**的通用症狀：抄漏一欄 → 讀到 `undefined` → 算術得到
 * `NaN` → 畫面上出現「5 張畫面（NaN MB）」，而斷言只問了那句話裡有沒有「刪掉
 * 了」，所以是綠的。事先猜哪一欄會漏是猜不完的；直接不准畫面上出現這三個東西
 * 就夠了，而且它同時是一條真的產品規則。
 */
const NONSENSE = /NaN|undefined|\[object Object\]/;
let nonsense = [];

/** 把計數歸零，回傳一個「到目前為止畫面上出現過的胡言亂語」的取用函式。 */
export function watchNonsense() {
  nonsense = [];
  return () => nonsense;
}

/** 那個選擇器在這份 HTML 上指到的那個標籤（含屬性），沒有就是 `null`。 */
function tagFor(html, sel) {
  const attr = sel.startsWith("#") ? `id="${sel.slice(1)}"` : sel.replace(/[[\]]/g, "");
  const m = html.match(new RegExp(`<[^>]*${attr.replace(/[.*+?^${}()|]/g, "\\$&")}[^>]*>`));
  return m === null ? null : m[0];
}

/**
 * 那個選擇器指到的東西，在真的 HTML 上是不是 `hidden` 的。
 *
 * 開場狀態要跟真的一樣。這一條的反例是 `[data-hits]`：HTML 上寫著 `hidden`，
 * 假 DOM 預設 `false`，於是「第一題就失敗的人整個畫面一個字都不會多」這個
 * bug 在測試裡看不見。
 */
export function hiddenIn(html, sel) {
  const tag = tagFor(html, sel);
  return tag !== null && /\shidden(\s|>|=)/.test(tag);
}

/**
 * 一個**只生得出那份 HTML 上真的有的元素**的 `document.querySelector`。
 *
 * 這是上面那條「假 DOM 不可以比真的寬鬆」的另一半。第一版的工廠對任何選擇器
 * 都回一個新的 `fakeEl()`，於是斷言可以站在一個**不存在的元素**上而永遠是綠
 * 的——`check-settings-say.mjs` 那條守著「儲存被灰掉的時候『重新讀取』還按得
 * 動」的斷言就是這樣，而那顆按鈕是那一頁唯一的出口。
 *
 * 找不到就當場結束整支測試（而不是讓那一條紅）：這不是產品錯了，是**這支測試
 * 自己的前提垮了**，剩下的每一條都不算數。
 *
 * @param make 生元素的函式，收選擇器。要一個會壞掉的 `<img>` 之類的就從這裡換。
 */
export function domOf(html, make = () => fakeEl()) {
  const nodes = new Map();
  return (sel) => {
    if (!nodes.has(sel)) {
      if (tagFor(html, sel) === null) {
        console.log(`✗ 前提不成立：那份 HTML 上沒有 ${sel}，而假 DOM 生了一個給它`);
        console.log("   （這支測試站在一個不存在的元素上，剩下的每一條都不算數）");
        process.exit(1);
      }
      const el = make(sel);
      // 只在生出來的時候套一次。每次查詢都套的話，這個查詢函式會把畫面洗回
      // 開場狀態——而斷言正是透過它讀畫面的，於是每一條都在看開場。
      el.hidden = hiddenIn(html, sel);
      nodes.set(sel, el);
    }
    return nodes.get(sel);
  };
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
