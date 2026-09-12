#!/usr/bin/env node
/*
 * 游標進到她那扇窗的時候，renderer 有沒有把 Rust 那條輪詢執行緒叫起來。
 *
 * 這扇窗整片透明，作業系統卻照整個 340×560 的矩形做命中判定。Rust 一條執行緒
 * 輪詢游標位置、翻 `set_ignore_cursor_events`，游標在她 120px 以外的時候那條
 * 執行緒最久睡 160 毫秒（`hit::POLL_AWAY_MS`）。使用者從遠處一口氣把滑鼠甩到
 * 她旁邊的**空白**上按下去，那一下就會被這扇窗吃掉，底下的視窗收不到。
 *
 * 能救的只有 renderer：游標進來的那一瞬間窗還是可點的，webview 收得到
 * `pointermove`。所以這邊喊一聲，那條執行緒立刻跑一次它本來就會跑的判斷。
 *
 * 這支閘門守的是那一聲，以及它的**形狀**：
 *
 * - 喊了，而且不帶座標也不帶答案。renderer 送一份自己算的答案過去，就會變成
 *   第二個「上一次翻到哪一邊」的記錄者；兩邊各記各的，其中一邊翻完另一邊不
 *   知道，開關就會停在那裡不再送出。判斷整套留在 Rust，這邊只負責喊。
 * - 一幀只喊一次。`pointermove` 一秒來上千次，每一次都發 IPC 是純浪費，而且
 *   同一幀問到的是同一個游標位置。
 * - 沒有 Tauri（瀏覽器裡開 demo）的時候不准炸。
 * - renderer 自己不准碰 `setIgnoreCursorEvents`，Rust 那邊的
 *   `set_ignore_cursor_events` 也必須只有一個呼叫端。
 * - 這一聲是**加速**不是取代：輪詢不能被拿掉。反方向（已經穿透中、游標移到
 *   她身上）webview 是瞎的，只有輪詢救得回來。
 *
 * 斷言故意不綁節流的做法（rAF 還是時間戳都行）——這支閘門要活得過下一次
 * 換寫法。它問的是「喊了幾聲」，不是「用什麼東西數幀」。
 */

import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { domOf, fakeDocument, loader, read } from "./fake-dom.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "app.js");
const MAIN = join(UI, "../src-tauri/src/main.rs");
const HTML = read(join(UI, "index.html"));
const boot = loader(read(SRC));

let failed = 0;
function ok(cond, why) {
  if (cond) return;
  console.log(`✗ ${why}`);
  failed += 1;
}
const tick = () => new Promise((r) => setTimeout(r, 0));

/**
 * 開一次她那一頁，並且**留住**掛到 `globalThis` 上的事件處理器。
 *
 * 其他幾支閘門把 `globalThis.addEventListener` 換成空函式——它們不需要餵事件。
 * 這一支整條線就在那些處理器裡面，吞掉等於什麼都沒測。
 */
async function open({ withTauri = true } = {}) {
  const node = domOf(HTML);
  const handlers = new Map();
  const invokes = [];
  const frames = [];

  globalThis.document = fakeDocument(node, { visibilityState: "visible" });
  globalThis.location = { search: "" };
  globalThis.addEventListener = (ev, fn) => {
    (handlers.get(ev) ?? handlers.set(ev, []).get(ev)).push(fn);
  };
  globalThis.removeEventListener = () => {};
  globalThis.matchMedia = () => ({ matches: false, addEventListener() {} });
  globalThis.innerWidth = 340;
  globalThis.innerHeight = 560;
  globalThis.MutationObserver = class {
    observe() {}
    disconnect() {}
    takeRecords() {
      return [];
    }
  };
  globalThis.speechSynthesis = { getVoices: () => [], addEventListener() {}, cancel() {}, speak() {} };
  // 節流可能用 rAF、也可能用時間戳。兩種都要能跑，所以這支要在，而且要收得到
  // 排進來的 callback——底下 `pump()` 會把它們放掉。
  globalThis.requestAnimationFrame = (fn) => {
    frames.push(fn);
    return frames.length;
  };
  globalThis.cancelAnimationFrame = () => {};

  const tauri = {
    core: { invoke: async (cmd, arg) => { invokes.push({ cmd, arg }); return null; } },
    event: { listen: async () => () => {} },
    window: { getCurrentWindow: () => ({ startDragging: async () => {} }) },
  };
  if (withTauri) globalThis.__TAURI__ = tauri;
  else delete globalThis.__TAURI__;

  await boot();
  await tick();

  /** 把排進 rAF 的 callback 全部放掉，再讓 microtask 與 timer 跑完。 */
  const pump = async () => {
    for (let round = 0; round < 4; round += 1) {
      const due = frames.splice(0, frames.length);
      for (const fn of due) fn(round);
      await tick();
    }
  };
  const fire = async (ev, detail = {}) => {
    for (const fn of handlers.get(ev) ?? []) fn({ type: ev, buttons: 0, ...detail });
    await pump();
  };
  return {
    fire,
    pump,
    handlers,
    nudges: () => invokes.filter((i) => i.cmd === "pet_pointer_moved"),
  };
}

/*
 * 前提本身也要驗一次。這支閘門的每一條都站在「假瀏覽器收得到這一頁掛上去的
 * 全域指標事件」上面；接不到的話底下每一條都會變成在問一個沒人聽的問題，而
 * 且會安靜地綠——所以寧可在這裡就吵。
 */
{
  const page = await open();
  const listeners = (page.handlers.get("pointermove") ?? []).length;
  if (listeners === 0) {
    console.log("✗ 前提不成立：這一頁一個全域 pointermove 處理器都沒掛上來");
    console.log("   （底下每一條都在餵一個沒人聽的事件，剩下的不算數）");
    process.exit(1);
  }
}

/* ---------- 1. 游標動了就喊 ---------- */
{
  const page = await open();
  const before = page.nudges().length;
  ok(before === 0, `還沒有人動滑鼠就先喊了 ${before} 聲`);
  await page.fire("pointermove", { clientX: 12, clientY: 12 });
  const after = page.nudges();
  ok(
    after.length >= 1,
    "游標在她那扇窗裡動了，renderer 一聲都沒喊——那條執行緒要睡滿 160 毫秒才會發現",
  );
}

/* ---------- 2. 喊的那一聲不帶座標、不帶答案 ---------- */
{
  const page = await open();
  await page.fire("pointermove", { clientX: 170, clientY: 40 });
  // 一聲都沒喊的話底下那個迴圈是空的，這一條會憑「沒東西可看」變綠。
  ok(page.nudges().length >= 1, "沒有喊出任何一聲，這一條沒有東西可以檢查");
  for (const { arg } of page.nudges()) {
    const keys = arg === undefined || arg === null ? [] : Object.keys(arg);
    ok(
      keys.length === 0,
      `那一聲帶了 ${JSON.stringify(keys)} 過去。renderer 只能說「去看一下」，` +
        "不能送自己算的座標或答案——那會變成第二個翻開關的人",
    );
  }
}

/* ---------- 3. 一整串移動不會變成一整串 IPC ---------- */
{
  const page = await open();
  for (let i = 0; i < 60; i += 1) {
    for (const fn of page.handlers.get("pointermove") ?? []) {
      fn({ type: "pointermove", buttons: 0, clientX: 100 + i, clientY: 100 });
    }
  }
  await page.pump();
  const n = page.nudges().length;
  ok(n >= 1, "連續 60 次移動之後一聲都沒喊");
  ok(
    n <= 5,
    `連續 60 次移動喊了 ${n} 聲。同一幀裡問到的是同一個游標位置，多喊的每一聲都是白發的 IPC`,
  );
}

/* ---------- 4. 沒有 Tauri 的時候不准炸 ---------- */
{
  const page = await open({ withTauri: false });
  let threw = null;
  try {
    await page.fire("pointermove", { clientX: 5, clientY: 5 });
  } catch (err) {
    threw = err;
  }
  ok(threw === null, `瀏覽器裡開 demo（沒有 __TAURI__）動一下滑鼠就炸了：${threw}`);
  ok(page.nudges().length === 0, "沒有 Tauri 還是發了 IPC 出去");
}

/* ---------- 5. 翻開關的人只能有一個 ---------- */
{
  const js = read(SRC);
  ok(
    !js.includes("setIgnoreCursorEvents"),
    "renderer 自己在翻 `setIgnoreCursorEvents`。兩個寫入端各記各的「上一次翻到哪一邊」，" +
      "其中一邊翻完另一邊不知道，開關就會停在那裡不再送出",
  );
  const rust = read(MAIN);
  const sites = rust.split("\n").filter((l) => /\.set_ignore_cursor_events\(/u.test(l)).length;
  ok(
    sites === 1,
    `main.rs 有 ${sites} 個 \`set_ignore_cursor_events\` 呼叫端，應該只有輪詢執行緒那一個`,
  );
}

/* ---------- 6. 這一聲是加速，不是取代 ---------- */
{
  const rust = read(MAIN);
  ok(
    rust.includes("next_poll_ms"),
    "輪詢的節奏不見了。renderer 只在窗可點的時候收得到滑鼠事件；已經穿透中、" +
      "游標移到她身上的那個方向 webview 是瞎的，只有輪詢救得回來",
  );
}


/* ---------- 7. 喊的那個閘門，就是輪詢在等的那個 ---------- */
{
  /*
   * 這一條守的是接線，不是邏輯，因為 `apps/desktop` 是另一個 workspace——
   * 根目錄的 `cargo test --workspace` 走不到它，本機也編不起來（缺
   * libdbus-1-dev）。`hit::wait_for_next_poll` 和 `hit::nudge` 兩支在
   * sister-shell 裡有真的測試在跑；接不上來的話，那些測試會全綠著守一段沒有
   * 人在用的程式碼，而 renderer 每一次喊都掉進一個沒人等的閘門——**完全沒有
   * 症狀**，只是那 160 毫秒原封不動留著。
   */
  const rust = read(MAIN);
  const bodyOf = (name) => {
    const head = rust.indexOf(`fn ${name}(`);
    if (head < 0) return null;
    const end = rust.indexOf("\n}\n", head);
    return end < 0 ? rust.slice(head) : rust.slice(head, end);
  };

  const poll = bodyOf("spawn_click_through");
  ok(poll !== null, "main.rs 裡找不到 `spawn_click_through`");
  if (poll !== null) {
    ok(
      poll.includes("wait_for_next_poll"),
      "輪詢執行緒沒有在等那個閘門。renderer 喊的每一聲都掉進沒人等的地方，" +
        "而且一點症狀都沒有——她只是照舊最久睡 160 毫秒",
    );
    ok(
      !/thread::sleep/u.test(poll),
      "輪詢執行緒還在用 `thread::sleep`。睡著的時候 condvar 叫不醒它",
    );
    ok(poll.includes("hit_wake"), "輪詢執行緒等的不是 `hit_wake` 那個閘門");
  }

  const cmd = bodyOf("pet_pointer_moved");
  ok(cmd !== null, "main.rs 裡找不到 `pet_pointer_moved` 這條指令");
  if (cmd !== null) {
    ok(
      /hit::nudge\(/u.test(cmd),
      "那條指令沒有走 `hit::nudge`。手抄一份 condvar 協定的話，少一句 " +
        "`notify_one()` 也照樣編得過、也照樣沒有症狀",
    );
    ok(cmd.includes("hit_wake"), "那條指令立的不是 `hit_wake` 那個閘門");
  }

  ok(
    rust.includes("pet_pointer_moved,"),
    "`pet_pointer_moved` 沒有註冊進 `generate_handler!`——renderer 那一聲會被 " +
      "Tauri 直接回 error，而 app.js 那邊是 `.catch(() => {})`，安靜地什麼都不會發生",
  );
}

if (failed > 0) {
  console.log(`\n${failed} 條沒過`);
  process.exit(1);
}
console.log("✓ 游標進來的時候 renderer 會把輪詢執行緒叫起來，而且只喊一聲、不帶答案");
// 每個 module instance 都留著自己那顆 5 秒輪詢的 setInterval，所以要自己走
//（和 check-pet-says-why.mjs 同一個理由）。
process.exit(0);
