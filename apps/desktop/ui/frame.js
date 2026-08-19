// 「當時的畫面」視窗。一張圖，加上它是什麼時候、在哪裡拍的。

const invoke = globalThis.__TAURI__?.core?.invoke ?? null;

const shot = document.querySelector("[data-shot]");
const trouble = document.querySelector("[data-trouble]");
const whenEl = document.querySelector("[data-when]");
const whereEl = document.querySelector("[data-where]");

function when(ts) {
  const d = new Date(ts);
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
    d.getHours(),
  )}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function say(message) {
  trouble.textContent = message;
  trouble.hidden = false;
  shot.hidden = true;
}

/**
 * 開發用：`frame.html?demo=1` 用一張現場畫出來的假截圖看版面。
 *
 * 理由跟 app.js 的 `?state=` 一樣——這台開發機開不起 Tauri 視窗，而這是
 * 使用者會盯著看最久的一個畫面。Tauri 載入時帶的 query 只有 `id`。
 */
function demoShot() {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="2560" height="1440">
    <rect width="2560" height="1440" fill="#eceff4"/>
    <rect y="0" width="2560" height="72" fill="#3b4252"/>
    <text x="40" y="48" font-family="sans-serif" font-size="30" fill="#eceff4">帳單查詢 — Chrome</text>
    <rect x="120" y="200" width="1500" height="900" fill="#ffffff" stroke="#d8dee9" stroke-width="4"/>
    <text x="170" y="300" font-family="sans-serif" font-size="46" fill="#2e3440">本期應繳金額 NT$13,450</text>
    <text x="170" y="390" font-family="sans-serif" font-size="46" fill="#2e3440">客服專線 0800-080-123</text>
    <text x="170" y="480" font-family="sans-serif" font-size="38" fill="#4c566a">帳單問題請按 2</text>
  </svg>`;
  return `data:image/svg+xml;base64,${btoa(unescape(encodeURIComponent(svg)))}`;
}

async function show() {
  const params = new URLSearchParams(globalThis.location.search);

  if (params.get("demo") === "1") {
    shot.src = demoShot();
    shot.hidden = false;
    whenEl.textContent = when(Date.now() - 3 * 86400 * 1000);
    whereEl.textContent = "chrome.exe — 帳單查詢 — https://example.com/bill";
    return;
  }

  const id = Number(params.get("id"));
  if (!Number.isInteger(id)) {
    say("沒有指定是哪一張畫面。");
    return;
  }
  if (invoke === null) {
    say("這一頁不是在 AI-Sister 裡打開的。");
    return;
  }

  try {
    const view = await invoke("frame_image", { frameId: id });
    shot.src = view.data_url;
    shot.hidden = false;
    whenEl.textContent = when(view.ts);
    // 出處是這個視窗的一半意義：一張沒有時間地點的截圖只是一張圖。
    whereEl.textContent = [view.app, view.title, view.url]
      .filter(Boolean)
      .join(" — ");
  } catch (err) {
    // 「過了保留期」不是錯誤，是設計。訊息由 Rust 那邊給，因為只有它分得出
    // 「圖被保留期清掉了」跟「檔案不見了」——後者才是真的出事。
    say(String(err?.message ?? err));
    whenEl.textContent = "";
  }
}

void show();
