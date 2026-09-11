// 開一個無頭 Chromium、（可選）先點幾下、然後截圖。
//
// 存在的理由：這台開發機開不起 Tauri 視窗（沒有 webkit2gtk-4.1、沒有 sudo），
// 所以 `apps/desktop/ui/` 裡那幾頁唯一的驗證方式就是 `?demo=1` 加一張截圖。
// 單純的 `--screenshot` 只看得到**初始畫面**，而那幾頁最要緊的東西是狀態變了
// 之後才長出來的——時間軸上那顆刪除鍵的第二段（紅色的「確定刪掉」）就是。
// 一個只驗證得了初始狀態的工具，會讓人以為沒截到的那半是好的。
//
//   node scripts/shot.mjs <url> <out.png> [w] [h] [selector ...]
//       [--init-js '在每次頁面載入前跑的 JS'] [--expect-js '回傳 truthy 的頁面內 JS']
//
// 最後那幾個參數是要依序點的 CSS selector，每一下之間等 250ms。
// 需要先自己起一個 http server（CSP 的 `script-src 'self'` 在 file:// 上會
// 擋掉 ES module）。

import { spawn } from "node:child_process";
import { readdirSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [url, out, w = "980", h = "720", ...actions] = process.argv.slice(2);
if (!url || !out) {
  console.error(
    "用法：node scripts/shot.mjs <url> <out.png> [w] [h] [selector...] [--init-js JS] [--expect-js JS]",
  );
  process.exit(2);
}

const clicks = [];
let initialPageScript = null;
let expectedPageState = null;
for (let i = 0; i < actions.length; i += 1) {
  if (actions[i] !== "--init-js" && actions[i] !== "--expect-js") {
    clicks.push(actions[i]);
    continue;
  }
  const option = actions[i];
  const alreadySet = option === "--init-js" ? initialPageScript !== null : expectedPageState !== null;
  if (alreadySet || i + 1 >= actions.length) {
    console.error(`${option} 必須剛好出現一次，後面接一段頁面內 JS`);
    process.exit(2);
  }
  const script = actions[(i += 1)];
  if (option === "--init-js") initialPageScript = script;
  else expectedPageState = script;
}

const viewportWidth = Number(w);
const viewportHeight = Number(h);
if (
  !Number.isInteger(viewportWidth) ||
  viewportWidth <= 0 ||
  !Number.isInteger(viewportHeight) ||
  viewportHeight <= 0
) {
  console.error(`寬高必須是正整數：${w}×${h}`);
  process.exit(2);
}

// `out` 是這次命令承諾要產生的 exact 檔案。失敗時留下上一輪同名 PNG 會讓呼叫端
// 把舊畫面當新證據；先只 unlink 這一個 non-recursive target，目錄則明確報錯。
try {
  unlinkSync(out);
} catch (error) {
  if (error?.code !== "ENOENT") throw error;
}

const root = join(process.env.HOME, ".cache/ms-playwright");
const dir = readdirSync(root).find((d) => d.startsWith("chromium-"));
if (!dir) throw new Error(`找不到 chromium：${root}`);
const bin = join(root, dir, "chrome-linux64/chrome");

const port = 9222 + (process.pid % 400);
const chrome = spawn(bin, [
  "--headless",
  "--disable-gpu",
  "--no-sandbox",
  "--hide-scrollbars",
  `--window-size=${viewportWidth},${viewportHeight}`,
  `--remote-debugging-port=${port}`,
  "about:blank",
]);
let ws = null;
const stopChrome = () => {
  if (ws?.readyState === WebSocket.OPEN) ws.close();
  if (!chrome.killed) chrome.kill();
};
process.once("exit", stopChrome);
chrome.on("error", (e) => {
  throw e;
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** DevTools 有時候要一秒才聽得到。硬等會在快的機器上浪費時間，所以用重試。 */
async function target() {
  for (let i = 0; i < 60; i++) {
    try {
      // 先只取得一個確定的 blank target；連上之後再由 Page.navigate 明確導航，
      // 不把 DevTools `/json/new?...` 各版本不同的 query parsing 當成已載入證據。
      const r = await fetch(`http://127.0.0.1:${port}/json/new?about%3Ablank`, {
        method: "PUT",
      });
      if (r.ok) return await r.json();
    } catch {
      // 還沒起來
    }
    await sleep(100);
  }
  throw new Error("DevTools 沒有在 6 秒內起來");
}

const { webSocketDebuggerUrl } = await target();
ws = new WebSocket(webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.addEventListener("open", res, { once: true });
  ws.addEventListener("error", rej, { once: true });
});

let seq = 0;
const waiting = new Map();
const pageExceptions = [];
ws.addEventListener("message", (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.method === "Runtime.exceptionThrown") {
    const details = msg.params?.exceptionDetails;
    pageExceptions.push(details?.exception?.description ?? details?.text ?? "頁面有未捕捉例外");
    return;
  }
  const slot = waiting.get(msg.id);
  if (!slot) return;
  waiting.delete(msg.id);
  msg.error ? slot.rej(new Error(msg.error.message)) : slot.res(msg.result);
});

function send(method, params = {}) {
  const id = ++seq;
  return new Promise((res, rej) => {
    waiting.set(id, { res, rej });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

// `--window-size` 設的是 browser window；用 DevTools 另開的 target 仍可能拿到 Chromium
// 預設的 500px viewport。截小視窗時那會讓 responsive 版面驗到另一個尺寸，所以連上
// target 後再把 CSS viewport 和輸出像素一起釘在 CLI 指定值。
await send("Emulation.setDeviceMetricsOverride", {
  width: viewportWidth,
  height: viewportHeight,
  deviceScaleFactor: 1,
  mobile: false,
});
await send("Page.enable");
await send("Runtime.enable");
if (initialPageScript !== null) {
  // DevTools 在 document script 之前注入。這條只有截圖 CLI 能用；產品
  // URL 不增加 query fixture，也不讓純瀏覽器網頁把假回覆當成 native 狀態。
  await send("Page.addScriptToEvaluateOnNewDocument", { source: initialPageScript });
}
const navigation = await send("Page.navigate", { url });
if (navigation.errorText) {
  throw new Error(`頁面無法載入：${navigation.errorText}`);
}

async function evaluate(expression) {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true });
  // 頁面裡丟出來的例外要往上報，不能只是回一個 undefined 然後截一張空圖。
  if (r.exceptionDetails) {
    throw new Error(r.exceptionDetails.exception?.description ?? "頁面丟出例外");
  }
  return r.result?.value;
}

async function waitUntil(expression, label, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      if (await evaluate(expression)) return;
    } catch (error) {
      // Navigation can replace the execution context between two polls. Keep the latest
      // detail and only fail once the same page had a fair chance to finish loading.
      lastError = String(error?.message ?? error);
    }
    await sleep(100);
  }
  throw new Error(`${label}（10 秒逾時${lastError ? `；最後錯誤：${lastError}` : ""}）`);
}

const expectedUrl = new URL(url).href;
await waitUntil(
  `document.readyState === "complete" && globalThis.location.href === ${JSON.stringify(expectedUrl)}`,
  `頁面沒有完成載入：${expectedUrl}`,
);
const actualUrl = await evaluate("globalThis.location.href");
if (actualUrl !== expectedUrl) {
  throw new Error(`截到的不是要求的頁面：預期 ${expectedUrl}，實際 ${actualUrl}`);
}

// 這支工具就是拿來看 desktop 的靜態 UI。不能讓 app.js 還沒跑、Persona 圖還沒
// decode，或 settings 還停在「正在讀角色…」時照樣印成功；那種 PNG 不是驗證。
await waitUntil(
  `(() => {
    const avatar = document.querySelector("[data-avatar]");
    if (avatar) {
      const portrait = document.querySelector("[data-persona-portrait]");
      const reel = document.querySelector("[data-persona-reel]");
      const reelImages = reel ? [...reel.querySelectorAll("img")] : [];
      const hasKnownState = ["idle", "thinking", "paused", "asleep"].includes(avatar.dataset.state);
      const hasReadyReel = avatar.classList.contains("reel-ready") &&
        avatar.dataset.reel === "ready" && reel && !reel.hidden &&
        reelImages.length >= 21 && reelImages.length <= 26 &&
        reelImages.every((picture) => picture.complete && picture.naturalWidth > 0);
      const hasFallback = !avatar.classList.contains("reel-ready") && portrait &&
        !portrait.hidden && getComputedStyle(portrait).visibility !== "hidden" &&
        portrait.complete && portrait.naturalWidth > 0;
      const hasFigure = hasReadyReel || hasFallback;
      return hasKnownState && hasFigure;
    }
    const picker = document.querySelector("[data-persona-choice-groups]");
    if (picker) {
      const cards = [...document.querySelectorAll(".persona-choice")];
      const pictures = [...document.querySelectorAll(".persona-choice-image")];
      const previewPictures = [...document.querySelectorAll(".persona-preview-image")];
      return cards.length === 17 && pictures.length === 17 &&
        pictures.every((picture) => picture.complete && picture.naturalWidth > 0) &&
        previewPictures.length === 1 &&
        previewPictures[0].complete && previewPictures[0].naturalWidth > 0 &&
        document.querySelector("[data-persona-preview-name]")?.textContent !== "正在讀角色…";
    }
    // 其他四扇 desktop 頁面未必有 Persona，但至少要能認出 checked-in HTML 的
    // 固定根節點。404、Chrome error page、空目錄與任意別頁都不能被截成成功。
    return [
      "[data-days]",           // timeline.html
      "[data-cards]",          // onboarding.html
      "[data-shot]",           // frame.html
      "[data-corpus]",         // metrics.html
    ].some((selector) => document.querySelector(selector));
  })()`,
  "沒有載入可辨識且已 ready 的 AI-Sister 畫面",
);

function assertNoPageExceptions() {
  if (pageExceptions.length > 0) {
    throw new Error(`頁面有未捕捉例外：${pageExceptions.join(" | ")}`);
  }
}

assertNoPageExceptions();

for (const sel of clicks) {
  const ok = await evaluate(
    `(() => { const e = document.querySelector(${JSON.stringify(sel)});
              if (!e) return false; e.click(); return true; })()`,
  );
  if (!ok) throw new Error(`點不到：${sel}`);
  await sleep(250);
  assertNoPageExceptions();
}

if (expectedPageState !== null) {
  await waitUntil(`Boolean(${expectedPageState})`, "畫面沒有到達 --expect-js 指定狀態");
}
assertNoPageExceptions();

const { data } = await send("Page.captureScreenshot", { format: "png" });
assertNoPageExceptions();
const png = Buffer.from(data, "base64");
const pngSignature = "89504e470d0a1a0a";
if (
  png.length < 24 ||
  png.subarray(0, 8).toString("hex") !== pngSignature ||
  png.readUInt32BE(16) !== viewportWidth ||
  png.readUInt32BE(20) !== viewportHeight
) {
  throw new Error(
    `截圖像素不是指定的 ${viewportWidth}×${viewportHeight}（實際 ${
      png.length >= 24 ? `${png.readUInt32BE(16)}×${png.readUInt32BE(20)}` : "不是完整 PNG"
    }）`,
  );
}
writeFileSync(out, png);
console.log(`${out}（點了 ${clicks.length} 下）`);

stopChrome();
process.removeListener("exit", stopChrome);
