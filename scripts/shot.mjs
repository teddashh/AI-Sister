// 開一個無頭 Chromium、（可選）先點幾下、然後截圖。
//
// 存在的理由：這台開發機開不起 Tauri 視窗（沒有 webkit2gtk-4.1、沒有 sudo），
// 所以 `apps/desktop/ui/` 裡那幾頁唯一的驗證方式就是 `?demo=1` 加一張截圖。
// 單純的 `--screenshot` 只看得到**初始畫面**，而那幾頁最要緊的東西是狀態變了
// 之後才長出來的——時間軸上那顆刪除鍵的第二段（紅色的「確定刪掉」）就是。
// 一個只驗證得了初始狀態的工具，會讓人以為沒截到的那半是好的。
//
//   node scripts/shot.mjs <url> <out.png> [w] [h] ['selector' ...]
//
// 最後那幾個參數是要依序點的 CSS selector，每一下之間等 250ms。
// 需要先自己起一個 http server（CSP 的 `script-src 'self'` 在 file:// 上會
// 擋掉 ES module）。

import { spawn } from "node:child_process";
import { readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [url, out, w = "980", h = "720", ...clicks] = process.argv.slice(2);
if (!url || !out) {
  console.error("用法：node scripts/shot.mjs <url> <out.png> [w] [h] [selector...]");
  process.exit(2);
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
  `--window-size=${w},${h}`,
  `--remote-debugging-port=${port}`,
  "about:blank",
]);
chrome.on("error", (e) => {
  throw e;
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** DevTools 有時候要一秒才聽得到。硬等會在快的機器上浪費時間，所以用重試。 */
async function target() {
  for (let i = 0; i < 60; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, {
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
const ws = new WebSocket(webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.addEventListener("open", res, { once: true });
  ws.addEventListener("error", rej, { once: true });
});

let seq = 0;
const waiting = new Map();
ws.addEventListener("message", (ev) => {
  const msg = JSON.parse(ev.data);
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

async function evaluate(expression) {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true });
  // 頁面裡丟出來的例外要往上報，不能只是回一個 undefined 然後截一張空圖。
  if (r.exceptionDetails) {
    throw new Error(r.exceptionDetails.exception?.description ?? "頁面丟出例外");
  }
  return r.result?.value;
}

await sleep(600);
for (const sel of clicks) {
  const ok = await evaluate(
    `(() => { const e = document.querySelector(${JSON.stringify(sel)});
              if (!e) return false; e.click(); return true; })()`,
  );
  if (!ok) throw new Error(`點不到：${sel}`);
  await sleep(250);
}

const { data } = await send("Page.captureScreenshot", { format: "png" });
writeFileSync(out, Buffer.from(data, "base64"));
console.log(`${out}（點了 ${clicks.length} 下）`);

ws.close();
chrome.kill();
