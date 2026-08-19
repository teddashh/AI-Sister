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

async function show() {
  const id = Number(new URLSearchParams(globalThis.location.search).get("id"));
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
