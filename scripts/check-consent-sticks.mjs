#!/usr/bin/env node
/*
 * 三張同意書那一頁：勾勾上寫的東西，一定要是檔案裡的東西。
 *
 * 這一頁是這整個產品的隱私契約，而它只有一種真正嚴重的錯——**畫面說已同意、
 * 檔案裡沒有**（或反過來）。`sister record` 讀的是那個檔案，不是這個畫面。
 * 原始碼裡那段註解自己就是這樣寫的。
 *
 * 而那顆勾勾是原生的 `<input type="checkbox">`：`change` 事件跑到 JS 手上的
 * 時候，瀏覽器**早就把它翻過去了**。所以「寫失敗」的正確反應不是「不要改
 * 畫面」，是「把已經被改掉的畫面翻回去」——這兩件事聽起來一樣，做起來差一行。
 *
 * 最壞的一條是兩個失敗疊在一起：寫不進去 → 退回去讀一次來修正畫面 → 讀也
 * 讀不出來。那兩件事多半是同一個原因（同一個檔案、同一顆磁碟、同一個鎖），
 * 所以它們不是獨立事件，是**同時發生**的。那條路上 `paint()` 根本不會跑。
 *
 * 方向也重要：他把第三張**取消**勾選來停掉截圖，寫失敗、讀也失敗，勾勾留在
 * 取消的樣子——而她繼續寫圖。他關掉這一頁就不會再回來看了。
 */

import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { fakeEl, loader, read } from "./fake-dom.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "onboarding.js");
const boot = loader(read(SRC));

const KEYS = ["local-recording", "cloud-reading", "frame-storage"];

/** `ConsentView` 的形狀，照 main.rs 那個 struct 抄的。 */
function view(granted = [true, false, true]) {
  return {
    path: "C:\\Users\\ted\\AppData\\Roaming\\sister\\consent.toml",
    current: granted[0],
    allows_recording: granted[0],
    allows_frames: granted[2],
    store_images: true,
    capture_enabled: true,
    reset_by_version: false,
    sheets: KEYS.map((key, i) => ({
      key,
      wording: `第 ${i + 1} 張`,
      without: `沒有第 ${i + 1} 張會怎樣`,
      granted_at: granted[i] ? 1_755_000_000_000 : null,
      effective: granted[i],
    })),
  };
}

const tick = () => new Promise((r) => setTimeout(r, 20));

async function open({ onRead, onSet } = {}) {
  const nodes = new Map();
  const node = (sel) => {
    if (!nodes.has(sel)) nodes.set(sel, fakeEl());
    return nodes.get(sel);
  };
  let state = [true, false, true];

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

  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, arg) => {
        if (cmd === "consent_read") {
          if (onRead) return onRead(state);
          return view(state);
        }
        if (cmd === "consent_set") {
          if (onSet) return onSet(arg, state);
          state = state.map((v, i) => (KEYS[i] === arg.key ? arg.granted : v));
          return view(state);
        }
        return null;
      },
    },
    window: { getCurrentWindow: () => ({ close() {} }) },
  };

  await boot();
  await tick();
  return {
    node,
    disk: () => state,
    say: () => node("[data-say]").textContent,
    bad: () => node("[data-say]").classList.contains("bad"),
    /** 那三顆勾勾，順序和 KEYS 一樣。 */
    boxes: () => node("[data-cards]").children.map((li) => li.querySelector("input")),
    /**
     * 按第 i 張。**先自己翻**再送 change 事件——原生的 checkbox 就是這個順序，
     * 而這條測試守的正是「翻過去之後沒人翻回來」。
     */
    async toggle(i) {
      const box = node("[data-cards]").children[i].querySelector("input");
      box.checked = !box.checked;
      for (const fn of box.handlers.change ?? []) fn();
      await tick();
      return box;
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

console.log("① 一般狀態：勾勾照著檔案畫");
{
  const p = await open();
  const boxes = p.boxes();
  check("三張都在", boxes.length === 3, boxes.length);
  check(
    "勾的狀態和檔案一致",
    boxes.map((b) => b.checked).join() === p.disk().join(),
    boxes.map((b) => b.checked),
  );
}

console.log("② 勾第二張，寫得進去");
{
  const p = await open();
  await p.toggle(1);
  check("檔案裡真的變了", p.disk()[1] === true, p.disk());
  check("勾勾也是勾的", p.boxes()[1].checked === true, p.boxes()[1].checked);
}

console.log("③ 寫不進去（唯讀磁碟），但還讀得回來");
{
  const p = await open({
    onSet: () => {
      throw new Error("寫不進去：拒絕存取");
    },
  });
  const box = await p.toggle(1);
  check("說得出為什麼", p.say().includes("拒絕存取"), p.say());
  check("而且是紅的", p.bad(), p.bad());
  check("檔案沒有被改到", p.disk()[1] === false, p.disk());
  check("勾勾也彈回去了", p.boxes()[1].checked === false, p.boxes()[1].checked);
  void box;
}

console.log("④ 寫不進去，而且連讀都讀不回來（同一顆磁碟、同一個鎖）");
{
  // **先宣告再開頁。** 第一版寫成 `var reads` 擺在 `await open()` 底下，於是
  // closure 跑的時候它是 `undefined`，`undefined++` 是 NaN，`NaN > 0` 永遠
  // 假——那個「讀也失敗」從頭到尾沒有發生過，而測試只少紅兩條。假資料的
  // 控制流錯掉，跟假資料的形狀錯掉一樣，都會讓測試綠得沒有意義。
  let reads = 0;
  const p = await open({
    onSet: () => {
      throw new Error("寫不進去：拒絕存取");
    },
    onRead: (state) => {
      // 第一次是開場那次（好），之後都炸——寫失敗和讀失敗是同一個原因。
      if (reads++ > 0) throw new Error("讀不出同意書：拒絕存取");
      return view(state);
    },
  });
  const before = p.boxes()[0].checked;
  await p.toggle(0);
  check("開場那次是勾著的", before === true, before);
  // 這裡是這支測試存在的理由。
  check(
    "勾勾要翻回檔案裡的樣子，不可以停在他剛剛按的樣子",
    p.boxes()[0].checked === true,
    p.boxes()[0].checked,
  );
  check("留著「寫不進去」那一句", p.say().includes("寫不進去"), p.say());
  check("而且說了現在連讀都讀不出來", p.say().includes("讀不出來"), p.say());
  check("是兩行", p.say().split("\n").length === 2, p.say());
  check("紅的", p.bad(), p.bad());
}

console.log("⑤ 反方向：取消第三張來停掉截圖，兩邊都失敗");
{
  let reads = 0;
  const p = await open({
    onSet: () => {
      throw new Error("寫不進去：拒絕存取");
    },
    onRead: (state) => {
      if (reads++ > 0) throw new Error("讀不出同意書：拒絕存取");
      return view(state);
    },
  });
  await p.toggle(2);
  check(
    "勾勾不可以停在「取消」——她其實還在寫圖",
    p.boxes()[2].checked === true,
    p.boxes()[2].checked,
  );
  check("而且畫面上有話說", p.say().includes("寫不進去"), p.say());
}

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——同意書那一頁上的勾勾，和檔案裡的不是同一件事。`);
  process.exit(1);
}
console.log("✓ 勾勾上寫的東西就是檔案裡的東西，寫失敗的時候它會自己說");
