#!/usr/bin/env node
/*
 * Windows 的 WebView2 有一條很窄、但會把整個 desktop 凍住的接線規則：
 * `WebviewWindowBuilder::build()` 不能從同步 IPC command 或 Tauri event handler
 * 直接跑。壞掉時原生 HWND 和標題已經出來了，controller 卻等不到 callback，
 * 所以使用者看到的是一扇有標題的白窗，接著每一扇窗一起 Not Responding。
 *
 * Tauri 自己在 `WebviewWindowBuilder::new` 的文件裡要求兩條路：command 要是
 * async，event handler 要把建立工作送到 separate thread。這支 gate 守的是
 * **產品接線**，不是另抄一份能綠的範例：五扇 auxiliary window 的 builder
 * 必須各自收在 `open_*_window`；有 IPC 入口的四扇要用真 `async fn`，tray-only
 * 的 metrics 不為了過 gate 多開一扇 command；系統匣只能經過內含
 * `std::thread::spawn` 的 `spawn_window`。
 *
 * onboarding 在 `.setup(...)` 裡的第一次自動開窗是 Tauri 文件列出的安全
 * 例外；那裡可以直接呼叫 `open_onboarding_window`。離開 setup 之後不靠這個
 * 例外放行系統匣。
 */

import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MAIN_PATH = join(ROOT, "apps/desktop/src-tauri/src/main.rs");
const APP_PATH = join(ROOT, "apps/desktop/ui/app.js");
const MAIN = readFileSync(MAIN_PATH, "utf8");
const APP = readFileSync(APP_PATH, "utf8");

let failed = 0;
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

/*
 * 找 Rust function body 前先把註解和字串抹成同長度空白。main.rs 的註解與
 * `format!` 裡都有大量 `{}`；直接數大括號會讓 function 在一句中文裡提早
 * 結束，然後一支理應會紅的 gate 安靜變綠。
 */
function maskRust(source) {
  const out = [...source];
  const wipe = (at) => {
    if (out[at] !== "\n") out[at] = " ";
  };
  let i = 0;
  while (i < source.length) {
    if (source.startsWith("//", i)) {
      while (i < source.length && source[i] !== "\n") wipe(i++);
      continue;
    }
    if (source.startsWith("/*", i)) {
      let depth = 0;
      while (i < source.length) {
        if (source.startsWith("/*", i)) {
          wipe(i++);
          wipe(i++);
          depth++;
        } else if (source.startsWith("*/", i)) {
          wipe(i++);
          wipe(i++);
          depth--;
          if (depth === 0) break;
        } else {
          wipe(i++);
        }
      }
      continue;
    }

    // r"..." / r#"..."#，以及 byte/C-string 的 br/cr 版本。
    const raw = source.slice(i).match(/^(?:b|c)?r(#{0,16})"/);
    if (raw) {
      const hashes = raw[1];
      const close = `"${hashes}`;
      for (let n = 0; n < raw[0].length; n++) wipe(i++);
      while (i < source.length && !source.startsWith(close, i)) wipe(i++);
      for (let n = 0; n < close.length && i < source.length; n++) wipe(i++);
      continue;
    }

    if (source[i] === '"') {
      wipe(i++);
      while (i < source.length) {
        if (source[i] === "\\") {
          wipe(i++);
          if (i < source.length) wipe(i++);
        } else if (source[i] === '"') {
          wipe(i++);
          break;
        } else {
          wipe(i++);
        }
      }
      continue;
    }
    // Rust char literal：`'{'`、`'\n'`。生命週期 `'a` 沒有收尾單引號，
    // 不會被這個式子誤抹。少這條的話，`'"'` 會打開一個假字串，
    // 之後的 function 邊界全會風向相反。
    const character = source.slice(i).match(/^'(?:\\.|[^'\\\n])'/);
    if (character) {
      for (let n = 0; n < character[0].length; n++) wipe(i++);
      continue;
    }
    i++;
  }
  return out.join("");
}

const CODE = maskRust(MAIN);

function closeBrace(open) {
  if (open < 0 || CODE[open] !== "{") return -1;
  let depth = 0;
  for (let i = open; i < CODE.length; i++) {
    if (CODE[i] === "{") depth++;
    if (CODE[i] === "}" && --depth === 0) return i + 1;
  }
  return -1;
}

function functionNamed(name) {
  const pattern = new RegExp(`\\b(?:async\\s+)?fn\\s+${name}\\b`, "g");
  const matches = [...CODE.matchAll(pattern)];
  if (matches.length !== 1) return { name, matches: matches.length, found: false };
  const start = matches[0].index;
  const open = CODE.indexOf("{", start);
  const end = closeBrace(open);
  if (end < 0) return { name, matches: 1, found: false };
  const signature = CODE.slice(start, open);
  const before = CODE.slice(Math.max(0, start - 160), start);
  return {
    name,
    found: true,
    start,
    open,
    end,
    signature,
    before,
    code: CODE.slice(start, end),
    source: MAIN.slice(start, end),
  };
}

const WINDOWS = [
  ["settings", "settings.html"],
  ["onboarding", "onboarding.html"],
  ["timeline", "timeline.html"],
  ["metrics", "metrics.html"],
  ["frame", "frame.html"],
];
const OPENING_SLOTS = new Map([
  ["settings", "SETTINGS_WINDOW_OPENING"],
  ["onboarding", "ONBOARDING_WINDOW_OPENING"],
  ["timeline", "TIMELINE_WINDOW_OPENING"],
  ["metrics", "METRICS_WINDOW_OPENING"],
  ["frame", "FRAME_WINDOW_OPENING"],
]);
const COMMANDS = ["settings", "onboarding", "timeline", "frame"];

console.log("① builder 只能在 internal helper");
const internal = new Map();
for (const [name, page] of WINDOWS) {
  const helperName = `open_${name}_window`;
  const helper = functionNamed(helperName);
  internal.set(name, helper);
  check(`${helperName} 唯一存在`, helper.found, helper);
  if (!helper.found) continue;
  const builders = [...helper.code.matchAll(/\btauri::WebviewWindowBuilder::new\s*\(/g)];
  check(`${helperName} 自己建立 WebView`, builders.length === 1, builders.length);
  check(
    `${helperName} 也在 helper 內完成 build`,
    [...helper.code.matchAll(/\.build\s*\(/g)].length === 1,
    helper.source,
  );
  // frame 會帶 query string，所以這裡只釘檔名，不釘 `.into()` 的寫法。
  check(`${helperName} 接的是 ${page}`, helper.source.includes(page), helper.source.slice(0, 500));
  const slot = OPENING_SLOTS.get(name);
  check(
    `${helperName} 先取得自己的 in-flight reservation`,
    new RegExp(`WindowOpening::claim\\s*\\(\\s*&${slot}\\s*\\)`).test(helper.code),
    helper.source.slice(0, 500),
  );
}

check(
  "in-flight reservation 是非阻塞 atomic claim",
  /compare_exchange\s*\(\s*false\s*,\s*true\b/.test(functionNamed("claim").code ?? "") &&
    /store\s*\(\s*false\s*,\s*Ordering::Release\s*\)/.test(CODE),
  undefined,
);

const allBuilders = [...CODE.matchAll(/\btauri::WebviewWindowBuilder::new\s*\(/g)];
check("五扇視窗的 builder 一個不少、一個不多", allBuilders.length === 5, allBuilders.length);
// 另外數一次不帶 `tauri::` 與 constructor 名的形狀；不然多加一個
// `use tauri::WebviewWindowBuilder; WebviewWindowBuilder::from_config(...)` 會整條漏掉。
const everyBuilderCall = [...CODE.matchAll(/\bWebviewWindowBuilder::[A-Za-z_][A-Za-z0-9_]*\s*\(/g)];
check(
  "沒有 alias 或另一種 constructor 繞過五扇視窗的計數",
  everyBuilderCall.length === 5,
  everyBuilderCall.length,
);
for (const match of everyBuilderCall) {
  const owners = [...internal.entries()]
    .filter(([, fn]) => fn.found && match.index >= fn.start && match.index < fn.end)
    .map(([name]) => name);
  check("builder 不在 wrapper、tray 或 setup 裡", owners.length === 1, {
    line: MAIN.slice(0, match.index).split("\n").length,
    owners,
  });
}

console.log("② IPC command 真的離開同步 handler");
for (const name of COMMANDS) {
  const wrapperName = `open_${name}`;
  const wrapper = functionNamed(wrapperName);
  check(`${wrapperName} command wrapper 唯一存在`, wrapper.found, wrapper);
  if (!wrapper.found) continue;
  check(
    `${wrapperName} 是真 async fn`,
    /\basync\s+fn\s+open_/.test(wrapper.signature),
    wrapper.signature.trim(),
  );
  check(
    `${wrapperName} 是 Tauri command`,
    /#\s*\[\s*tauri::command(?:\s*\([^)]*\))?\s*\]\s*$/.test(wrapper.before),
    wrapper.before.trim(),
  );
  check(
    `${wrapperName} 只接到 internal helper`,
    new RegExp(`\\bopen_${name}_window\\s*\\(`).test(wrapper.code) &&
      !/WebviewWindowBuilder|std::thread::spawn/.test(wrapper.code),
    wrapper.source,
  );
}

const handlerStart = CODE.indexOf("tauri::generate_handler![");
const handlerEnd = handlerStart < 0 ? -1 : CODE.indexOf("]", handlerStart);
const handler = handlerStart < 0 || handlerEnd < 0 ? "" : CODE.slice(handlerStart, handlerEnd);
for (const name of COMMANDS) {
  check(
    `open_${name} 真的註冊進 invoke handler`,
    new RegExp(`\\bopen_${name}\\b`).test(handler),
    handler === "" ? "generate_handler! 不存在" : undefined,
  );
}
check(
  "metrics 保持 tray-only，沒有為了過 gate 多開 IPC 入口",
  !/\bopen_metrics\b/.test(handler) && !functionNamed("open_metrics").found,
  undefined,
);

// function 定義本身也長得像 `open_*_window(`，所以期望值包含它。wrapper
// 各一個 call；onboarding 另外有 setup 的文件允許例外；metrics 只有 tray
// function pointer，沒有直接 call。這個總數把未來新增在另一個同步 handler
// 裡的漏網 direct call 也變成紅燈。
for (const [name, expected] of [
  ["settings", 2],
  ["onboarding", 3],
  ["timeline", 2],
  ["metrics", 1],
  ["frame", 2],
]) {
  const calls = [...CODE.matchAll(new RegExp(`\\bopen_${name}_window\\s*\\(`, "g"))];
  check(`open_${name}_window 沒有額外 direct call site`, calls.length === expected, calls.length);
}

console.log("③ tray event 只投遞到明確的 thread helper");
const spawner = functionNamed("spawn_window");
check("spawn_window helper 唯一存在", spawner.found, spawner);
if (spawner.found) {
  check(
    "spawn_window 明確使用 std::thread::spawn",
    /\bstd::thread::spawn\s*\(/.test(spawner.code),
    spawner.source,
  );
  check(
    "spawn_window 自己不建 WebView",
    !/WebviewWindowBuilder/.test(spawner.code),
    spawner.source,
  );
}

const menuStart = CODE.indexOf(".on_menu_event");
const menuEnd = menuStart < 0 ? -1 : CODE.indexOf(".on_tray_icon_event", menuStart);
check("tray menu event 範圍找得到", menuStart >= 0 && menuEnd > menuStart, [menuStart, menuEnd]);
if (menuStart >= 0 && menuEnd > menuStart) {
  const menuCode = CODE.slice(menuStart, menuEnd);
  const menuSource = MAIN.slice(menuStart, menuEnd);
  check(
    "tray event 本身沒有 WebviewWindowBuilder/build",
    !/WebviewWindowBuilder|\.build\s*\(/.test(menuCode),
    MAIN.slice(menuStart, menuEnd),
  );

  // Branch 可以是 `{ ... }`，也可以直接是 `=> spawn_window(...)`。以下一個
  // match arm 當邊界，不去猜大括號；第一版只認大括號，結果把
  // timeline 分支後面的 quit 整塊當成 timeline，是 checker 自己造的假紅。
  const arms = [...menuSource.matchAll(/^\s*"([^"]+)"\s*=>/gm)].map((match) => ({
    id: match[1],
    start: match.index,
  }));
  for (let i = 0; i < arms.length; i++) {
    arms[i].end = i + 1 < arms.length ? arms[i + 1].start : menuSource.length;
    arms[i].source = menuSource.slice(arms[i].start, arms[i].end);
    arms[i].code = menuCode.slice(arms[i].start, arms[i].end);
  }

  for (const [item, name] of [
    ["timeline", "timeline"],
    ["settings", "settings"],
    ["consent", "onboarding"],
    ["metrics", "metrics"],
  ]) {
    const branch = arms.find((arm) => arm.id === item) ?? null;
    check(`tray 有 ${item} branch`, branch !== null, undefined);
    if (branch === null) continue;
    check(
      `tray ${item} 經過 spawn_window`,
      /\bspawn_window\s*\(/.test(branch.code),
      branch.source,
    );
    check(
      `tray ${item} 投遞正確 internal helper`,
      new RegExp(`\\bopen_${name}_window\\b`).test(branch.code),
      branch.source,
    );
    check(
      `tray ${item} 沒有直接呼叫 builder path`,
      !new RegExp(`\\bopen_${name}(?:_window)?\\s*\\(`).test(branch.code) &&
        !/WebviewWindowBuilder|\.build\s*\(/.test(branch.code),
      branch.source,
    );
  }
}

// setup 是 Tauri 文件明列可以同步 build 的時機。守住自動打開同意書
// 的接線，也證明 gate 沒有把所有 direct internal call 一刀切成紅的。
const setupStart = CODE.indexOf(".setup(|app|");
const setupEnd = setupStart < 0 ? -1 : CODE.indexOf(".build(tauri::generate_context!())", setupStart);
const setupCode = setupStart < 0 || setupEnd < 0 ? "" : CODE.slice(setupStart, setupEnd);
check(
  "setup onboarding 保留文件允許的 direct internal helper 例外",
  /\bopen_onboarding_window\s*\(/.test(setupCode),
  setupCode === "" ? "setup 範圍找不到" : undefined,
);

console.log("④ 4 秒慢訊息只說它真的量到的事");
check(
  "app.js 不再宣稱第一次開資料庫要整理索引",
  !APP.includes("第一次打開資料庫要先整理索引"),
  undefined,
);
check("slow gate 仍然是 4 秒", /\bconst\s+SLOW_MS\s*=\s*4000\s*;/.test(APP), undefined);
const slowMessages = [...APP.matchAll(/\bslowNote\s*=\s*(["'`])([^\n]*?)\1\s*;/g)]
  .map((match) => match[2])
  .filter((message) => message !== "");
check("4 秒訊息只有一份", slowMessages.length === 1, slowMessages);
if (slowMessages.length === 1) {
  const message = slowMessages[0];
  check("4 秒訊息明說已經超過 4 秒", /超過\s*4\s*秒/.test(message), message);
  check(
    "4 秒訊息沒猜原因或處理階段",
    !/\b(?:because|index|database)\b|(?:因為|索引|資料庫|第一次|整理|重建|升級|移轉)/u.test(message),
    message,
  );
}

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——Windows 還有從同步 handler 建 WebView 的路，或畫面正在猜慢的原因。`);
  process.exit(1);
}
console.log("✓ 五扇 auxiliary window 都離開同步 handler，4 秒訊息只說已經量到的事");
