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
  // 出錯的時候**整個表頭**都要清掉，不是只清時間。這兩格是一組的：留著
  // 「chrome.exe — 網銀 — https://…」配上一句「圖不見了」，讀起來像「那張圖
  // 是網銀的，只是檔案掉了」——而這一頁其實沒能確認它是哪一張。
  //
  // 這一條在底下那個 `error` 事件加進來之前是空談：`show()` 一份文件只跑一
  // 次，走到 catch 的時候表頭本來就還是空的。是「圖解碼失敗」那條路讓它變成
  // 真的——那一刻表頭已經寫滿了。
  whenEl.textContent = "";
  whereEl.textContent = "";
  whereEl.classList.remove("unknown");
}

/*
 * 圖讀得出來，不代表圖打得開。
 *
 * `fs::read` 對一個只寫了一半的檔案照樣成功（她是常駐的，被工作管理員砍掉
 * 或斷電就會在磁碟上留下半截的 PNG），base64 也照樣編得出來——壞在瀏覽器
 * 解碼那一步，而那一步是**非同步**的，`show()` 的 try/catch 接不到。
 *
 * 少了這一段，畫面上是一個破圖圖示，配上一行寫得好好的時間和出處。這個視窗
 * 存在的理由是讓「你三天前看過這個」那句話可以被當場查證，而它會在那一刻替
 * 一張根本沒打開的圖背書。
 */
shot.addEventListener("error", () => {
  say("這個檔案還在，但打不開——可能是當時只寫了一半。");
});

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

  // `Number(null)` 是 0，`Number("")` 也是 0，`Number(" ")` 還是 0——而 0 是一個
  // 長得完全合法的 frame id。少了這一行，「網址上沒有 id」會走完整條正常流程、
  // 去問第 0 張，然後得到「找不到這張畫面」：一個「這個連結建錯了」的錯，被
  // 講成一個「你的資料不見了」的錯。他會去翻保留期設定找一個不存在的 bug。
  const raw = params.get("id")?.trim() ?? "";
  const id = raw === "" ? Number.NaN : Number(raw);
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
    //
    // 三個都是 NULL 是**真的會發生**的：`insert_frame` 直接把 `frame.focus` 那
    // 三格寫下去，不管它們是不是空的，而鎖定畫面、UAC 那層黑底、以及任何一個
    // 前景視窗問不出來的時刻，三格就都是空的。錄影照樣留下了那張圖。
    //
    // 那一格空著的時候不可以留白。留白在這一頁上有兩個意思——「這張畫面沒有
    // 留下是哪個視窗」和「這個程式忘了填」——而使用者分不出來，只會覺得排版
    // 壞了。這一整頁的用途是背書，而一個說不出自己出處的背書不算數。
    const where = [view.app, view.title, view.url].filter(Boolean).join(" — ");
    whereEl.classList.toggle("unknown", where === "");
    whereEl.textContent = where === "" ? "沒有留下是哪個視窗" : where;
  } catch (err) {
    // 「過了保留期」不是錯誤，是設計。訊息由 Rust 那邊給，因為只有它分得出
    // 「圖被保留期清掉了」跟「檔案不見了」——後者才是真的出事。
    say(String(err?.message ?? err));
  }
}

void show();
