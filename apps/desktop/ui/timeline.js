// 時間軸。和字母人、設定頁一樣沒有打包步驟——這個檔案就是瀏覽器讀到的那個檔案。
//
// 這一頁回答的問題和搜尋不一樣。搜尋回答「我記得的那件事在哪」，時間軸回答
// **「她到底記了什麼」**——那是使用者決定要不要繼續讓她跑的依據。所以這裡的
// 每一段空白都必須有名字：她被暫停了，和畫面沒變過，是兩件不同的事。
//
// PHASES.md 的 v0 寫的是「縮圖 + OCR 摘錄」，這裡只有 OCR 摘錄加一顆「看當時
// 的畫面」。理由是量：圖是用 data URL 送過去的（見 `frame_image`），一天八百
// 筆就是八百張 base64 塞進這個 webview。縮圖要做得對得先在 Rust 那邊產一份
// 小圖並且存起來，那是另一件事——現在假裝有縮圖的做法只有一種，就是讓這一頁
// 在他真正用了一整天之後才變得打不開。

// `let` 而不是 `const`：`?demo=1` 會把它換成一個假後端，好讓這一頁在開不起
// Tauri 的機器上仍然走產品那一條路（見檔案最後的 `fakeBackend`）。
let invoke = globalThis.__TAURI__?.core?.invoke ?? null;

const DAY = 86_400_000;
/** 小於這個長度的空檔不畫。不然一整天會被分隔線切成兩百段，反而看不出真的斷點。 */
const QUIET = 20 * 60_000;
/** 一天最多抓幾筆。抓不完的話 `truncated` 會說，不會安靜地截掉。 */
const LIMIT = 800;

const el = {
  days: document.querySelector("[data-days]"),
  railSay: document.querySelector("[data-rail-say]"),
  title: document.querySelector("[data-day-title]"),
  sub: document.querySelector("[data-day-sub]"),
  moments: document.querySelector("[data-moments]"),
  from: document.querySelector("[data-from]"),
  to: document.querySelector("[data-to]"),
  say: document.querySelector("[data-say]"),
  forget: document.querySelector("[data-forget]"),
};

/** 現在攤開的是哪一天。忘掉那顆鍵要用它換算時間範圍。 */
let current = null;

// 日期一律用視窗自己的時區算，Rust 那邊拿到的也是同一個偏移量——
// 兩邊各自判斷「今天是哪天」的話，日光節約時間那天會對不起來。
const tzOffsetMs = () => -new Date().getTimezoneOffset() * 60_000;

const hhmm = new Intl.DateTimeFormat("zh-TW", {
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});
const shortDay = new Intl.DateTimeFormat("zh-TW", {
  month: "long",
  day: "numeric",
  weekday: "short",
});
const longDay = new Intl.DateTimeFormat("zh-TW", {
  year: "numeric",
  month: "long",
  day: "numeric",
  weekday: "long",
});

/** 和 Rust 那邊的 `fmt::duration_ms` 同一套規則：無條件捨去到看得懂的單位。 */
function lasted(ms) {
  if (ms < 60_000) return `${Math.floor(ms / 1000)} 秒`;
  if (ms < 3_600_000) return `${Math.floor(ms / 60_000)} 分鐘`;
  const h = Math.floor(ms / 3_600_000);
  const m = Math.floor((ms % 3_600_000) / 60_000);
  return m === 0 ? `${h} 小時` : `${h} 小時 ${m} 分`;
}

function say(message, bad = false) {
  el.sub.textContent = message;
  el.sub.classList.toggle("bad", bad);
}

function tell(message, bad = false) {
  el.say.textContent = message;
  el.say.classList.toggle("bad", bad);
}

// ---------- 把一天排成一列 ----------

/**
 * 把「她看到的東西」和「她被關掉的那幾段」併成同一條時間線。
 *
 * 游標（`cursor`）是這裡唯一的機關：暫停那幾段會把游標推過去，所以被暫停蓋住
 * 的空白**不會**再被算成一段「沒有新東西」。少了這一步，同一段空白會被講兩次，
 * 而且第二次講的是錯的原因。
 */
function build(view, dayStart, now) {
  const dayEnd = dayStart + DAY;
  const entries = view.moments.map((m) => ({ at: m.ts, moment: m }));
  for (const p of view.pauses) {
    // 裁到這一天之內是為了畫線；判斷「這一段其實從昨天就開始了」看的是原值。
    const start = Math.max(p.from ?? dayStart, dayStart);
    const end = Math.min(p.to ?? dayEnd, dayEnd);
    // 裁完之後長度是零或負的 = 這一段根本不在這一天裡。後端不該送這種東西
    // 過來（`pause_spans` 兩端都篩過），但少了這一行，一筆越界的資料會畫出
    // 「這一天裡佔了 -37800 秒」——第一次跑 demo 就是這樣露出來的。
    if (end <= start) continue;
    entries.push({ at: start, pause: { ...p, start, end } });
  }
  entries.sort((a, b) => a.at - b.at);

  const rows = [];
  let cursor = dayStart;
  for (const e of entries) {
    if (e.pause) {
      rows.push({ kind: "pause", ...e.pause });
      cursor = Math.max(cursor, e.pause.end);
      continue;
    }
    if (e.at - cursor >= QUIET) {
      rows.push({ kind: "quiet", start: cursor, end: e.at });
    }
    rows.push({ kind: "moment", m: e.moment });
    cursor = Math.max(cursor, e.at);
  }

  // 這一天已經過完了，最後那段空白也要交代。今天的話不畫——剩下的時間
  // 不是「沒有紀錄」，是還沒發生，而那兩件事看起來一樣就糟了。
  if (dayEnd <= now && dayEnd - cursor >= QUIET) {
    rows.push({ kind: "quiet", start: cursor, end: dayEnd });
  }
  return rows;
}

/** 暫停那一列要說的話。三種開頭、兩種結尾，每一種都是真的會發生的。 */
function pauseWords(p, dayStart) {
  let head;
  if (p.from === null) {
    head = "她那時候是關著的（按下去那一筆已經過了保留期）";
  } else if (p.from < dayStart) {
    // 不能寫「還在暫停中」：這一段可能早上就解除了，只是**開始**在昨天。
    head = "延續前一天按下的暫停";
  } else {
    head = "你按了暫停";
  }
  // 長度一律講**這一天裡**的那一段（`start`/`end` 是裁過的），所以只要真正的
  // 兩端有任何一端在窗外，就得說出來。少了這兩句，一段從昨晚 21:00 到今早
  // 09:12 的暫停，會在兩天各自顯示成「持續 3 小時」和「持續 9 小時」——
  // 兩個都是真的，加起來卻讓人以為他按了兩次。
  const dayEnd = dayStart + DAY;
  let tail;
  if (p.to === null) {
    tail = "之後沒有再解除";
  } else if (p.to > dayEnd) {
    tail = `跨過午夜；這一天裡佔了 ${lasted(p.end - p.start)}`;
  } else if (p.from !== null && p.from < dayStart) {
    tail = `這一天裡佔了 ${lasted(p.end - p.start)}`;
  } else {
    tail = `持續 ${lasted(p.end - p.start)}`;
  }
  return [head, tail];
}

// ---------- 畫 ----------

function chip(text, kind) {
  const span = document.createElement("span");
  span.className = kind ? `chip ${kind}` : "chip";
  span.textContent = text;
  return span;
}

function momentRow(m) {
  const li = document.createElement("li");
  li.className = "row moment";

  const at = document.createElement("span");
  at.className = "at";
  at.textContent = hhmm.format(m.ts);

  const body = document.createElement("div");
  body.className = "body";

  const where = document.createElement("div");
  where.className = "where";
  if (m.app) where.append(chip(m.app, "app"));
  if (m.title) where.append(chip(m.title));
  if (m.url) where.append(chip(m.url));
  if (where.childElementCount > 0) body.append(where);

  const text = document.createElement("div");
  text.className = "text";
  // textContent 而不是 innerHTML：這串字是從螢幕上 OCR 出來的，
  // 也就是說它的內容由**任何一個他打開過的網頁**決定。
  text.textContent = m.text;
  body.append(text);

  if (m.frame_id === null) {
    // 文字留 365 天、畫面 30 天，所以這是正常狀態，不是壞掉。
    const gone = document.createElement("p");
    gone.className = "gone";
    gone.textContent = "畫面已經過了保留期，只剩這些字";
    body.append(gone);
  } else {
    const see = document.createElement("button");
    see.className = "see";
    see.type = "button";
    see.textContent = "看當時的畫面";
    see.addEventListener("click", () => {
      void invoke?.("open_frame", { frameId: m.frame_id });
    });
    body.append(see);
  }

  li.append(at, body);
  return li;
}

function gapRow(row, dayStart) {
  const li = document.createElement("li");
  li.className = `row ${row.kind}`;

  const at = document.createElement("span");
  at.className = "at";
  at.textContent = hhmm.format(row.start);

  const body = document.createElement("div");
  body.className = "body";
  const what = document.createElement("p");
  what.className = "what";

  const [head, tail] =
    row.kind === "pause"
      ? pauseWords(row, dayStart)
      : [
          // 「接下來」這三個字是必要的。左邊那格印的是這段空白的**開始**時間，
          // 而它通常和上一筆同一分鐘——不說出方向的話，畫面上會出現兩列
          // 09:12，讀的人得自己猜第二列指的是往前還是往後。
          `接下來 ${lasted(row.end - row.start)} 沒有新的東西進來`,
          // 這句是重點。一樣的畫面會被去重掉，所以「沒有新的一筆」
          // **不等於**她沒在跑——不講清楚的話這條線看起來像故障。
          "可能是離開了，也可能是畫面一直沒變",
        ];
  what.textContent = `${head}　`;
  const em = document.createElement("em");
  em.textContent = tail;
  what.append(em);

  body.append(what);
  li.append(at, body);
  return li;
}

function paint(view, day, now) {
  el.title.textContent = longDay.format(day.start_ts);
  const bits = [`${day.chunks} 筆`];
  if (day.chunks > 0) {
    bits.push(`${hhmm.format(day.first_ts)}–${hhmm.format(day.last_ts)}`);
  }
  if (view.pauses.length > 0) bits.push(`暫停 ${view.pauses.length} 段`);
  // 截斷了就說。安靜地少給幾百筆，會讓那一天看起來比實際短。
  if (view.truncated) bits.push(`只顯示前 ${LIMIT} 筆`);
  say(bits.join("・"), view.truncated);

  el.moments.replaceChildren();
  const rows = build(view, day.start_ts, now);
  for (const row of rows) {
    el.moments.append(
      row.kind === "moment" ? momentRow(row.m) : gapRow(row, day.start_ts),
    );
  }
  if (rows.length === 0) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "這一天沒有東西。";
    el.moments.append(li);
  }
}

// 一段 OCR 可以有兩千個字。預設收三行，點一下攤開。
el.moments?.addEventListener("click", (e) => {
  const text = e.target.closest?.(".text");
  if (text) text.closest(".row").classList.toggle("open");
});

// ---------- 忘掉這一段 ----------

/**
 * `"09:30"` 換成當天的毫秒偏移量。空的或看不懂就用 `fallback`。
 *
 * **看不懂的時候不可以當成 0。** 0 是午夜，而午夜是「整天」的開頭——
 * 一個打到一半的時間欄位會因此把刪除範圍往前擴張到那一天的最前面。
 */
function offsetOf(value, fallback) {
  const m = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  if (m === null) return fallback;
  const [h, min] = [Number(m[1]), Number(m[2])];
  if (h > 23 || min > 59) return fallback;
  return h * 3_600_000 + min * 60_000;
}

/** 這一刻按下去會刪掉哪一段。回 `null` = 這個範圍沒有意義。 */
function chosen() {
  if (current === null) return null;
  const from = current.start_ts + offsetOf(el.from.value, 0);
  // 「到」是**含**那一分鐘的：使用者寫 09:00–09:30 的意思是連 09:30 那一分鐘
  // 裡發生的事一起忘掉，不是刪到 09:29:59 為止留下一條尾巴。
  const to = current.start_ts + offsetOf(el.to.value, DAY - 60_000) + 60_000;
  return to > from ? { from, to } : null;
}

/** 還沒確認的那一次刪除。`null` = 下一下只是預覽。 */
let pending = null;

/**
 * 把按鈕退回第一段。
 *
 * 換天、改時間、刪完，都要回到這裡——一顆停在「確定刪掉」狀態的紅色按鈕，
 * 配上已經換掉的日期，會刪掉他完全沒看過的一段。
 */
function armReset() {
  pending = null;
  el.forget.className = "ghost";
  const range = chosen();
  if (range === null) {
    el.forget.disabled = true;
    el.forget.textContent = "忘掉這一段";
    return;
  }
  el.forget.disabled = current === null;
  const whole = range.from === current.start_ts && range.to >= current.start_ts + DAY;
  el.forget.textContent = whole
    ? "忘掉這一整天"
    : `忘掉 ${hhmm.format(range.from)}–${hhmm.format(range.to - 60_000)}`;
}

function scale(e) {
  const bits = [];
  if (e.chunks > 0) bits.push(`${e.chunks} 段文字`);
  if (e.facts > 0) bits.push(`${e.facts} 個事實`);
  if (e.images > 0) {
    bits.push(`${e.images} 張畫面（${Math.round(e.image_bytes / 104_857.6) / 10} MB）`);
  }
  if (e.events > 0) bits.push(`${e.events} 筆事件`);
  return bits;
}

async function forget() {
  const range = chosen();
  if (range === null || invoke === null) return;
  el.forget.disabled = true;
  try {
    if (pending === null) {
      // 第一下：只問，不動。
      const e = await invoke("forget_preview", {
        fromTs: range.from,
        toTs: range.to,
      });
      const bits = scale(e);
      if (bits.length === 0) {
        tell("這一段本來就是空的，沒有東西可以忘。");
        armReset();
        return;
      }
      pending = range;
      el.forget.className = "danger";
      el.forget.textContent = "確定刪掉";
      el.forget.disabled = false;
      // 「不可復原」要和數字擺在同一句話裡。分成兩行的話，看的人會看數字。
      tell(`會刪掉 ${bits.join("、")}——不可復原。`, true);
      return;
    }

    // 第二下：真的刪。用 `pending` 而不是重算一次 `chosen()`——他確認的是
    // 剛才那個範圍，中間欄位若被改過，那不是他點頭的那一段。
    const e = await invoke("forget_range", {
      fromTs: pending.from,
      toTs: pending.to,
    });
    const done = scale(e);
    const stay = current.start_ts;
    pending = null;
    // 先重讀再報告。順序反過來的話，`openDay` 裡的 `tell("")` 會把剛剛那句
    // 「刪掉了…」擦掉，於是他按下不可逆的按鈕之後什麼回音都沒有。
    await load(stay);
    if (e.failed.length > 0) {
      // 刪不掉的截圖還躺在磁碟上，而他以為它不在了。這句要蓋過成功那句。
      tell(`有 ${e.failed.length} 個畫面檔刪不掉：${e.failed[0]}`, true);
    } else {
      tell(done.length === 0 ? "沒有東西被刪掉。" : `刪掉了 ${done.join("、")}。`);
    }
  } catch (err) {
    tell(String(err?.message ?? err), true);
    armReset();
  }
}

el.forget?.addEventListener("click", () => void forget());
// 改了範圍就退回第一段。他確認過的是舊的那一段。
for (const input of [el.from, el.to]) {
  input?.addEventListener("input", () => {
    armReset();
    tell("");
  });
}

// ---------- 讀 ----------

async function openDay(day, button) {
  for (const b of el.days.querySelectorAll("button")) {
    b.setAttribute("aria-current", String(b === button));
  }
  current = day;
  // 換天一定要退回第一段，不然那顆紅色按鈕會刪掉他沒看過的一天。
  armReset();
  tell("");
  try {
    const view = await invoke("timeline_moments", {
      fromTs: day.start_ts,
      toTs: day.start_ts + DAY,
      limit: LIMIT,
    });
    paint(view, day, Date.now());
  } catch (err) {
    el.moments.replaceChildren();
    say(String(err?.message ?? err), true);
  }
}

function listDays(days) {
  el.days.replaceChildren();
  for (const day of days) {
    const li = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    const date = document.createElement("span");
    date.className = "date";
    date.textContent = shortDay.format(day.start_ts);
    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = `${day.chunks} 筆・${hhmm.format(day.first_ts)}–${hhmm.format(day.last_ts)}`;
    button.append(date, meta);
    button.addEventListener("click", () => void openDay(day, button));
    li.append(button);
    el.days.append(li);
  }
}

/**
 * 重讀整份清單。`keep` 是想停在哪一天（epoch 毫秒）。
 *
 * 刪完之後一定要重讀**清單**，不是只重畫右邊：把一整天忘光之後，那一天
 * 就不該再出現在左邊——一個點下去空空如也的日期，會讓人以為刪除失敗了。
 */
async function load(keep = null) {
  if (invoke === null) {
    el.railSay.textContent = "這一頁不是在 AI-Sister 裡打開的。";
    el.forget.disabled = true;
    return;
  }
  try {
    const days = await invoke("timeline_days", { tzOffsetMs: tzOffsetMs() });
    if (days.length === 0) {
      el.days.replaceChildren();
      el.moments.replaceChildren();
      el.railSay.textContent = "她還沒記得任何東西。";
      say("跑 sister record 之後再回來看。");
      current = null;
      armReset();
      return;
    }
    listDays(days);
    el.railSay.textContent = `${days.length} 天有紀錄`;
    // 停在原本那天；那天整個被忘光的話就退回最新的一天（清單是倒序的）。
    const i = Math.max(
      days.findIndex((d) => d.start_ts === keep),
      0,
    );
    await openDay(days[i], el.days.querySelectorAll("button")[i]);
  } catch (err) {
    el.railSay.textContent = String(err?.message ?? err);
  }
}

/**
 * 開發用：`timeline.html?demo=1`。
 *
 * 和 app.js 的 `?state=`、settings.js 的 `?demo=1` 同一個理由——這台開發機開不起
 * Tauri 視窗，而這一頁最需要看過的正是那幾種**空白**長什麼樣。假資料裡刻意有：
 * 一段跨夜的暫停、一段當天按的暫停、一段沒有新東西的安靜、一筆圖已經過期的
 * 紀錄。
 *
 * **它假的是後端，不是這一頁。** 上一次讓開發開關自己畫一份長得像的版面出來，
 * 截出來的圖就騙了我一次；而這一頁上有一顆不可逆的刪除鍵，它的兩段式狀態機
 * 正是最需要親眼看過的東西。所以這裡換掉的是 `invoke`，`load` / `openDay` /
 * `forget` 走的仍然是產品那一條路，連刪完之後重讀清單都是真的。
 */
function fakeBackend() {
  const day = Date.UTC(2026, 7, 17) - tzOffsetMs();
  const at = (h, m) => day + h * 3_600_000 + m * 60_000;
  const on = (d, h, m) => at(h, m) - d * DAY;

  let moments = [
    {
      ts: at(9, 12),
      app: "chrome.exe",
      title: "SPEC.md — AI-Sister",
      url: "github.com/ted-h/AI-Sister/blob/main/docs/SPEC.md",
      text: "L0 是原始訊號，L1 是從裡面抽出來的事實。兩層分開存的理由是：L1 抽錯了可以重跑，L0 沒了就真的沒了。",
      frame_id: 4021,
    },
    {
      ts: at(9, 41),
      app: "Code.exe",
      title: "db.rs — AI-Sister",
      url: null,
      text: "pub fn pause_spans(&self, from_ts: Millis, to_ts: Millis) -> Result<Vec<PauseSpan>>",
      frame_id: 4088,
    },
    {
      ts: at(14, 30),
      app: "chrome.exe",
      title: "健保署 — 就醫紀錄查詢",
      url: "nhi.gov.tw/records",
      text: "（這一筆只剩文字，示範圖已經過保留期的樣子）",
      frame_id: null,
    },
    {
      ts: at(16, 40),
      app: "Notion.exe",
      title: "週報",
      url: null,
      text: "把時間軸接起來了。空白處現在會自己說明原因。",
      frame_id: 4310,
    },
    // 另外兩天各放一點，這樣左邊那欄點下去不會是空的。
    {
      ts: on(1, 22, 14),
      app: "chrome.exe",
      title: "PHASES.md — AI-Sister",
      url: "github.com/ted-h/AI-Sister/blob/main/docs/PHASES.md",
      text: "Phase 1：字母人、搜尋框、時間軸瀏覽器 v0、onboarding 三張同意書。",
      frame_id: 3902,
    },
    {
      ts: on(4, 13, 5),
      app: "Terminal",
      title: "sister doctor",
      url: null,
      text: "排除規則　6 條 app、3 條網址、2 條標題\n保留期　畫面 30 天／文字 365 天",
      frame_id: null,
    },
  ];
  // 第一段是前一天晚上按的（from 落在 dayStart 之前），第二段是當天中午按的。
  let pauses = [
    { from: day - 3 * 3_600_000, to: at(9, 12) },
    { from: at(10, 30), to: at(14, 30) },
  ];
  const inRange = (m, a, b) => m.ts >= a && m.ts < b;
  const hit = (a, b) => moments.filter((m) => inRange(m, a, b));
  const dayOf = (ts) =>
    Math.floor((ts + tzOffsetMs()) / DAY) * DAY - tzOffsetMs();

  return async (cmd, arg) => {
    switch (cmd) {
      // 從自己手上的資料算出「哪幾天有東西」，而不是另外寫死一份清單。
      // 寫死的那一版會在刪掉之後對不起來——左邊說 1281 筆、右邊一片空白，
      // 而那正是這個 demo 應該替我抓到的那種錯。
      case "timeline_days": {
        const by = new Map();
        for (const m of moments) {
          const d = dayOf(m.ts);
          const e = by.get(d) ?? {
            start_ts: d,
            chunks: 0,
            first_ts: m.ts,
            last_ts: m.ts,
          };
          e.chunks += 1;
          e.first_ts = Math.min(e.first_ts, m.ts);
          e.last_ts = Math.max(e.last_ts, m.ts);
          by.set(d, e);
        }
        return [...by.values()].sort((a, b) => b.start_ts - a.start_ts);
      }
      case "timeline_moments":
        return {
          moments: hit(arg.fromTs, arg.toTs),
          // 兩端都要篩，和 Rust 那邊的 `pause_spans` 一樣：只篩一端的話會把
          // 一段「這一天之後」的暫停送進來。假後端偷懶就等於少驗一種情況。
          pauses: pauses.filter(
            (p) =>
              (p.to ?? Infinity) > arg.fromTs &&
              (p.from ?? -Infinity) < arg.toTs,
          ),
          truncated: false,
        };
      case "forget_preview":
      case "forget_range": {
        const gone = hit(arg.fromTs, arg.toTs);
        const images = gone.filter((m) => m.frame_id !== null);
        if (cmd === "forget_range") {
          moments = moments.filter((m) => !inRange(m, arg.fromTs, arg.toTs));
          pauses = pauses.filter(
            (p) => (p.from ?? -Infinity) < arg.fromTs || (p.to ?? Infinity) > arg.toTs,
          );
        }
        return {
          chunks: gone.length,
          facts: gone.length * 2,
          frames: gone.length,
          images: images.length,
          image_bytes: images.length * 148_000,
          events: gone.length * 3,
          failed: [],
        };
      }
      default:
        throw new Error(`demo 沒有實作 ${cmd}`);
    }
  };
}

if (new URLSearchParams(globalThis.location.search).get("demo") === "1") {
  invoke = fakeBackend();
  // 「現在」推到這一天之後，好讓收尾那段空白也畫出來——16:40 之後的七個
  // 小時是這一頁最容易被漏掉的一塊，看不到就等於沒做過。
  Date.now = () => Date.UTC(2026, 7, 19);
}
void load();
