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
    // 空白檢查對兩種 entry 都要做。以前只有 moment 那一支有，於是最後一筆
    // 09:12、下午 14:00 才按下暫停的話，中間那四小時四十八分一列都沒有——
    // 清單直接從 09:12 跳到 14:00。時間戳看得到，所以這不是一句假話，但它
    // 違反這個檔案自己那條「每一段空白都必須有名字」，而且讀的人會把整段
    // 空白算到那個暫停頭上，也就是把她**還在跑**的四小時算成她閉著眼。
    const at = e.pause ? e.pause.start : e.at;
    if (at - cursor >= QUIET) {
      rows.push({ kind: "quiet", start: cursor, end: at });
    }
    if (e.pause) {
      rows.push({ kind: "pause", ...e.pause });
      cursor = Math.max(cursor, e.pause.end);
      continue;
    }
    rows.push({ kind: "moment", m: e.moment });
    cursor = Math.max(cursor, e.at);
  }

  // 這一天已經過完了，最後那段空白也要交代。今天的話不畫——剩下的時間
  // 不是「沒有紀錄」，是還沒發生，而那兩件事看起來一樣就糟了。
  //
  // 被切掉的那一天也不畫，理由一樣：那一段不是「沒有新的東西進來」，是
  // 「我沒有送過來」。後端是**由早到晚**排序之後取前 800 筆，所以被切掉
  // 的永遠是尾巴——一個 4213 筆的忙日會停在 13:47，然後底下印
  //
  //     13:47　接下來 10 小時 13 分沒有新的東西進來
  //           可能是離開了，也可能是畫面一直沒變
  //
  // 那十個小時裡她記了三千多筆。這一列是這個檔案用來**替空白命名**的機制
  // （見開頭那句「每一段空白都必須有名字」），而這裡給了一個假名字，和真的
  // 「她在跑，只是畫面沒變」長得一模一樣。標題那行的「只顯示前 800 筆」
  // 救不了它：那句話在最上面，這一列在最下面，而人是照著最靠近的那句讀的。
  if (view.truncated) {
    // 尾巴要切在**現在**，不是午夜。今天 13:47 被切掉的話，13:47 到午夜是
    // 10 小時 13 分，可是其中只有到現在為止的那 13 分鐘可能藏著沒送過來的
    // 東西——剩下的十個小時還沒發生。而底下那句「不是那段時間沒有東西」會
    // 把那十個小時一起指控成「有東西，我沒給你」，於是他去找一個翻頁的方法，
    // 想把幾千筆根本不存在的紀錄叫出來。
    //
    // 這就是下面那條 `quiet` 分支上 `dayEnd <= now` 在守的同一件事（上面那段
    // 註解自己寫著「今天的話不畫——剩下的時間不是『沒有紀錄』，是還沒發生，
    // 而那兩件事看起來一樣就糟了」）。兩條分支，只守了一條。
    const end = Math.min(dayEnd, now);
    // `end <= cursor` 也還是要畫：`truncated` 是後端說的，代表真的有東西沒送
    // 過來，只是它和最後一筆落在同一分鐘。那時候不講長度，但不能不講。
    rows.push({
      kind: "cut",
      start: cursor,
      end: Math.max(end, cursor),
      unfinished: end < dayEnd,
    });
  } else if (dayEnd <= now && dayEnd - cursor >= QUIET) {
    rows.push({ kind: "quiet", start: cursor, end: dayEnd });
  }
  return rows;
}

/** 暫停那一列要說的話。三種開頭、兩種結尾，每一種都是真的會發生的。 */
function pauseWords(p, dayStart) {
  let head;
  if (p.from === null) {
    // 兩種原因做得出同一個孤兒 resume，而這裡分不出是哪一種：保留期掃掉了那天
    // 的 `system_events`，或者他自己對那段時間按過「忘掉這一段」。`retention.rs`
    // 的 forget 明講它會留下這個形狀（見那裡 739-742 行的註解）。
    //
    // 只講保留期的代價不是「少講一句」，是**指控錯對象**：一個十分鐘前才自己
    // 按下忘掉的人，會以為是保留期在吃他的紀錄，於是跑去把 text_days 調長——
    // 一個和真正原因相反的下一步。這一頁 601 行的空狀態早就是兩種一起講的。
    head = "她那時候是關著的（按下去那一筆被忘掉了，或是過了保留期）";
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
  } else if (p.from === null) {
    // 起點不見的時候 `p.start` 是 `p.from ?? dayStart` **湊**出來的，不是他按
    // 下去的時間。掉進最底下那個 `持續 ${...}` 會拿午夜當起點：09:30 按、11:00
    // 解除的那一段印出來是「持續 11 小時」——一個看起來精確、而且大了七倍的
    // 數字。上面那句「只要真正的兩端有任何一端在窗外，就得說出來」講的正是這
    // 件事，而「不知道在哪」比「在窗外」更不能拿來算。
    //
    // 這個 null 是想過的：底下那條 `p.from < dayStart` 本來寫著
    // `p.from !== null &&`。想過之後讓它掉進 else，比沒想過更難看見——所以現在
    // null 自己一條分支，`p.from` 是不是 null 只在這裡問一次。
    //
    // 「算不出長度」這五個字照抄 `retention.rs:742` 的契約：那裡說刪掉
    // system_events 會做出孤兒 resume，而消費端「會說『這一段算不出長度』，
    // 不會少算一段」。`pause_audit` 用 `truncated` 守住了，這一頁沒有。
    tail = `解除在 ${hhmm.format(p.to)}，但按下去那一筆不見了，算不出長度`;
  } else if (p.to > dayEnd) {
    tail = `跨過午夜；這一天裡佔了 ${lasted(p.end - p.start)}`;
  } else if (p.from < dayStart) {
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
    // 正常狀態，不是壞掉——原因有好幾種（只記字、節流、額度、保留期），
    // 這裡分不出是哪一種。理由同 app.js 那一段。
    const gone = document.createElement("p");
    gone.className = "gone";
    gone.textContent = "這一筆沒有留下畫面，只剩這些字";
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
      : row.kind === "cut"
        ? [
            // 這一列的全部意義是「底下這段空白不是我造成的」。它取代了
            // 一句以前印在這裡的假話（「接下來 10 小時沒有新的東西進來」），
            // 而那句話和真的沒東西長得一模一樣。
            //
            // 「這一天還沒完」對過完的那一天是假的，而它印出來的長度一路算到
            // 午夜——今天看的時候，那個長度裡有大半是還沒發生的時間。所以現在
            // 分兩種說法，長度也由 `build` 切在 `now`。
            row.end > row.start
              ? row.unfinished
                ? `到現在為止還有 ${lasted(row.end - row.start)} 我沒有送過來`
                : `這一天剩下的 ${lasted(row.end - row.start)} 我沒有送過來`
              : "這之後還有沒送過來的，只是它和上一筆同一分鐘",
            `一次只讀前 ${LIMIT} 筆——不是那段時間沒有東西`,
          ]
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

/**
 * 這一段刪下去會帶走什麼，一項一項列。
 *
 * 題庫（`queries`）以前漏了——後端一直有這個欄位，`forget` 也一直真的把它
 * 刪掉，只有這裡沒讀。代價是：她停止記錄之後他還在問問題（那是設計，她靠
 * 記憶回答），那段時間選下去，`bits` 是空的，畫面說「這一段本來就是空的，
 * 沒有東西可以忘」——然後刪掉他那幾題。**紀錄裡唯一一張存著他自己打的字的
 * 表，是唯一一張沒被列出來的。**
 */
function scale(e) {
  const bits = [];
  if (e.chunks > 0) bits.push(`${e.chunks} 段文字`);
  if (e.facts > 0) bits.push(`${e.facts} 個事實`);
  // 沒有截圖的那幾列也是紀錄。text-only 模式下、或圖已經過了保留期之後，
  // `images` 是 0，而那幾列裡還留著時間、dHash 和當時的視窗標題——`forget`
  // 一直都真的把它們刪掉，只有這裡沒讀。和上面那段題庫的故事是同一個，
  // 只是隔壁一欄。
  if (e.frames > 0) bits.push(`${e.frames} 列畫面紀錄`);
  if (e.images > 0) {
    bits.push(`${e.images} 張畫面（${Math.round(e.image_bytes / 104_857.6) / 10} MB）`);
  }
  if (e.events > 0) bits.push(`${e.events} 筆事件`);
  if (e.queries > 0) bits.push(`${e.queries} 題你問過的話`);
  // 「那一場錄製」本身。它不帶內容，帶的是時間——「那天下午 13:02 到 17:44
  // 她在錄」。少了這一行，一段只剩那一列的時間刪下去，`bits` 是空的，畫面
  // 會說「這一段本來就是空的，沒有東西可以忘」——然後刪掉那份他在電腦前的
  // 紀錄。和上面題庫、畫面紀錄那兩段是同一個故事，第三次。
  if (e.sessions > 0) bits.push(`${e.sessions} 場錄製的紀錄`);
  return bits;
}

/**
 * 資料庫說有圖、磁碟上找不到那個檔的那幾列。
 *
 * 預覽和事後都要講**同一句**。以前只有事後那一句有它，而預覽那邊照著資料庫
 * 算，所以「會刪掉 12 張畫面（1.8 MB）」後面接的是「刪掉了 0 張」——他按下
 * 那顆不可逆的按鈕的理由，正是那 1.8 MB。後端兩支現在走同一套記帳，這裡就
 * 沒有理由只講一半：預覽說得出 0 張，也就說得出那 12 列去哪了。
 *
 * 不是 ⚠：東西確實不在了，隱私上沒有缺口。但他拿這個數字對帳。
 */
function ghosts(e) {
  return e.missing > 0 ? `（另外 ${e.missing} 列說自己有圖，但那個檔早就不在磁碟上了）` : "";
}

/**
 * 刪完之後**沒被帶走**的那幾列 `sessions`。
 *
 * 上一場當掉的話，那一列撐得過這一刀：守衛不准碰還沒收尾的最新一列，因為那
 * 可能是此刻正在錄的那一場，刪掉它會讓接下來每一筆紀錄指向一個不存在的東西。
 * 而當掉的那一場長得一模一樣。
 *
 * 少了這一句，這一頁只列得出刪掉的東西——他會在別的地方撞見一個「1 場錄製」
 * 站在一整排 0 旁邊，而那時候沒有東西把它接回這一刀。`sister forget` 那邊是
 * 同一句話。
 *
 * `sessions_left` 是 `null` 代表**沒問過**（預覽算不出「刪完之後」），不是 0。
 *
 * `shell_beat` 和它一起讀完（後端算的是 `heartbeat::phase`），所以這裡不必印
 * 「當掉了，**或是**她此刻正在錄」——分得出來還印一個「或」，是把自己的懶惰
 * 講成他的功課。三種的下一步都不一樣，所以三句話分開寫。
 *
 * **三個字串，不是一個布林。** 上一版收 `shell_is_live`（後端算
 * `is_occupied`），於是開機那幾分鐘印的是「此刻有人佔著這個資料目錄（她正在
 * 錄，或正在開機）」——那個「或」就是上一段罵過的東西，而且那句話在那幾分鐘
 * 是假的：她那一列要等 `Db::open` 回來才 INSERT，所以手上這一列一定是**上一
 * 次當機留下來的殼**，不是佔著目錄的那一個。`sister forget` 那邊是同一句話。
 *
 * 「下次開始錄就會清掉」是假的，不要寫回去：`record` 的開機清理跑在新的一場
 * 開始**之前**，那時候這一列還是最新的一列，正好是守衛保護的那一列。整場錄製
 * 期間它都還在。見 `retention.rs` 那條照著真實順序寫的測試。
 */
function leftover(e) {
  if (!e.sessions_left) return "";
  // 認不得的值走 `gone` 那一句：它是三句裡唯一不會把一場活的錄製講成別的東西
  // 的，而那是這幾句話裡唯一會嚇到人的錯。
  const [what, then] =
    e.shell_beat === "live"
      ? [
          "那一場還沒收尾——她此刻正在錄",
          "等她收工的時候，那一場如果還是一列都不剩，那一列就會跟著走。",
        ]
      : e.shell_beat === "booting"
        ? [
            "她當掉了；現在有一個 sister record 正在起來，但這一列不是它的",
            "等它開始錄，這一列就不再是最新的一列，接下來任何一次清理都會把它帶走。",
          ]
        : [
            "她當掉了（現在沒有任何 recorder 佔著這個資料目錄）",
            "她再開始錄之後，那一列就不再是最新的一列，接下來任何一次清理都會把它帶走。",
          ];
  return `（留著 ${e.sessions_left} 場錄製的紀錄本身：${what}，裡面一列都不剩。${then}）`;
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
        // 圖檔全都不在了、而那一段只有那幾列 frame 的話，`bits` 還是有
        // 「N 列畫面紀錄」，所以走不到這裡。真的走到這裡就是真的空的。
        tell("這一段本來就是空的，沒有東西可以忘。");
        armReset();
        return;
      }
      pending = range;
      el.forget.className = "danger";
      el.forget.textContent = "確定刪掉";
      el.forget.disabled = false;
      // 「不可復原」要和數字擺在同一句話裡。分成兩行的話，看的人會看數字。
      tell(`會刪掉 ${bits.join("、")}——不可復原。${ghosts(e)}`, true);
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
    } else if (done.length === 0) {
      // 這裡也要接 `leftover(e)`：一段只剩下那一列空殼的時間刪下去，`done`
      // 是空的（守衛不准碰它），而畫面說「沒有東西被刪掉」——那一列還在，
      // 卻沒有任何一句話提到它。
      tell(`沒有東西被刪掉。${leftover(e)}`);
    } else {
      tell(`刪掉了 ${done.join("、")}。${ghosts(e)}${leftover(e)}`);
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
      // 「一天都沒有」有兩種，而它們的下一步是相反的。這裡數的是**還活著
      // 的**列數，分不出「從來沒錄過」和「錄了、然後被忘掉／過保留期」——
      // 而按下「忘掉這一整天」之後看到的正是這個畫面，它會叫他去跑一個他
      // 剛剛才故意清空的東西（甚至是一個正在跑的東西）。
      //
      // 問的是 `meta` 裡那個位元，不是 `sessions` 有沒有列。那張表現在會
      // 跟著它記下來的東西一起消失（保留期和「忘掉」都刪），所以「上一場是
      // 什麼時候」在整顆資料庫被清空之後是 `null` ——而那正是這一行最需要
      // 答對的時候。
      //
      // 而且 `sessions` 也不是每次都乾淨：最後一場當掉的話，那一列撐得過
      // `forget`（`delete_empty_sessions` 不准碰還沒收尾的最新一列，因為那
      // 可能是此刻正在錄的那一場）。拿它當「她錄過嗎」的話，同一顆被清空的
      // 資料庫會因為「上一場有沒有當掉」給出兩種答案。那個位元不會。
      //
      // 兩個位元，三種空。上面那一個人答不出中間那一種——它在 `start_session`
      // 就翻成 true，**第一張畫面之前**。於是一台 `capture.enabled = false` 的
      // 機器（跑完、一個字都沒記到、`sister forget` 從來沒被執行過）會在這一頁
      // 上被告知它的紀錄「是被忘掉的」，一則指控，而且把下一步指到相反的方向。
      const ever = await invoke("has_ever_recorded").catch(() => false);
      const stored = await invoke("has_ever_stored").catch(() => false);
      if (ever && !stored) {
        el.railSay.textContent = "一天都沒有。";
        say("她錄過，但一列內容都沒存進來過——先到設定頁看「開始記錄」那一段。");
      } else if (ever) {
        el.railSay.textContent = "現在一天都沒有了。";
        say("她錄過——這些紀錄是被忘掉的，或是過了保留期。再錄一段就會有新的。");
      } else {
        el.railSay.textContent = "她還沒記得任何東西。";
        say("跑 sister record 之後再回來看。");
      }
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
function fakeBackend(mode = "1") {
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
      text: "（這一筆只剩文字，示範沒有畫面可以點的樣子）",
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
  // `nulldata` 演的是「她跑過，一列內容都沒存進來過」，所以這裡真的要空——
  // 帶著三天資料去演一頁空白，演的就不是那一頁。
  if (mode === "nulldata") {
    moments = [];
  }
  // 第一段是前一天晚上按的（from 落在 dayStart 之前），第二段是當天中午按的。
  // `cross` 換掉第二段：晚上按下、明天早上才解除——那是 `pauseWords` 裡唯一
  // 一條在後端修好之前選不到的分支。時間挑在最後一筆（16:40）之後，不然這一頁
  // 會同時畫出「她被暫停」和暫停期間的紀錄，看起來像另一個 bug。
  let pauses = [
    { from: day - 3 * 3_600_000, to: at(9, 12) },
    mode === "cross"
      ? { from: at(17, 0), to: at(33, 0) }
      : { from: at(10, 30), to: at(14, 30) },
  ];
  const inRange = (m, a, b) => m.ts >= a && m.ts < b;
  const hit = (a, b) => moments.filter((m) => inRange(m, a, b));
  const dayOf = (ts) =>
    Math.floor((ts + tzOffsetMs()) / DAY) * DAY - tzOffsetMs();

  return async (cmd, arg) => {
    switch (cmd) {
      // **`load()` 那兩句話裡，這個 demo 以前只演得出錯的那一句。**
      //
      // 沒有這一條的話，那邊的 `.catch(() => false)` 會吃掉「demo 沒有實作」
      // 的錯誤，於是把整份資料刪光之後，畫面說「她還沒記得任何東西。跑
      // sister record 之後再回來看。」——正好是那整段程式碼在修的那句謊，
      // 而它就長在示範它的那一頁上。
      //
      // 回 `true`：這個 demo 一開始就有三天的東西，所以「她錄過」永遠成立。
      // 另一邊（真的沒錄過）是一台全新機器，那一句不會出錯也沒什麼好演的。
      case "has_ever_recorded":
        return true;
      // **同一句話的第三種走法，而它以前也演不出來。**
      //
      // `?demo=nulldata`：她跑過，一列內容都沒存進來過（`capture.enabled =
      // false`）。少了這一條，`.catch(() => false)` 會讓每一個 demo 都走進
      // 「她錄過但沒存過」那一支——把上面那句「被忘掉了」整個藏起來。
      // 兩邊都要看得見，才知道它們講的是相反的下一步。
      case "has_ever_stored":
        return demo !== "nulldata";
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
          truncated: mode === "cut",
        };
      case "forget_preview":
      case "forget_range": {
        const gone = hit(arg.fromTs, arg.toTs);
        const withImage = gone.filter((m) => m.frame_id !== null);
        // 假裝每 3 張裡有 1 張的檔案早就被人手動清掉了：資料庫還指著它，磁碟
        // 上沒有。假後端不模擬的話，`missing` 那一句話沒有任何辦法在這台機器
        // 上被看見。
        //
        // **兩支都要看得見。**這裡本來寫 `cmd === "forget_range"`，理由是
        // 「預覽看不出來，它只會數資料庫」——那句話當時是真的，而它正是
        // core 那邊剛修掉的 bug：預覽照著 `image_bytes` 答應了一個放不出來
        // 的空間，而他按下那顆不可逆的按鈕的理由就是那個數字。現在兩支都去
        // stat 一次（`count_files` / `delete_files`），假後端跟著改，不然這
        // 一頁示範的還是舊的那個謊。
        //
        // 模數要小。第一版寫 `i % 7`，而 demo 那一天只有三張圖，於是它永遠是
        // 0——一個「跑過了、什麼都沒驗到」的假後端，正好是它自己要防的東西。
        const vanished = (m, i) => i % 3 === 2;
        const images = withImage.filter((m, i) => !vanished(m, i));
        const missing = withImage.length - images.length;
        // **刪完之後還剩幾筆。** 兩支都要在動手之前算，因為真後端的
        // `count_empty_sessions`（預覽）和 `delete_empty_sessions`（真的刪）
        // 用的是同一個條件，而且有一條測試釘著它們回同一個數字。這裡本來是
        // `cmd === "forget_range" && moments.length === 0`——`moments` 那時候
        // 已經被下面那一行改過了，於是預覽永遠說「會刪掉 1 場錄製的紀錄」，
        // 接著真的刪的時候說「留著 1 場」。一個真後端做不出來的組合，長在
        // 一顆不可逆的按鈕的第一段上。
        const left = moments.filter(
          (m) => !inRange(m, arg.fromTs, arg.toTs),
        ).length;
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
          // 後端一直有這一欄，`scale()` 上面那段註解講的就是它漏掉的那次
          // ——而假後端到現在都還沒給，所以修好之後那一行在這台機器上還是
          // 看不到。同一根釘子的第三個位置。
          queries: Math.ceil(gone.length / 4),
          failed: [],
          missing,
          // 「那一場錄製」本身，以及**沒被帶走的那一列**。
          //
          // 刪到整份 demo 一個 moment 都不剩的時候，模擬的是「最後一場當掉
          // 了」：那一列撐得過這一刀（`delete_empty_sessions` 不准碰還沒收尾
          // 的最新一列），於是刪掉 0 場、留下 1 場。刪一半的時候相反。
          //
          // 這兩行在開發機上唯一看得到的地方就是這裡——Tauri 起不來，真後端
          // 的那個狀態要一台 Windows 加一次當機才生得出來。
          sessions: left === 0 ? 0 : 1,
          sessions_left:
            cmd === "forget_preview"
              ? null // 預覽不動任何東西，沒有「刪完之後」可言。不是 0。
              : left === 0
                ? 1
                : 0,
          // 真後端問的是 `heartbeat::phase`——一個心跳檔，這裡沒有。所以照
          // `cut` / `cross` 的老規矩開 mode：`live` 演「她此刻正在錄」、
          // `booting` 演「有一個 recorder 正在起來，而這一列是上一次當機留下
          // 來的」，其餘演「她當掉了」。三句話的下一步都不一樣（等她收工／等
          // 它開始錄／等她再開始錄），演不出來的那一支就等於沒被人看過——而
          // `booting` 正是上一版整支缺掉的那一種。
          shell_beat: mode === "live" ? "live" : mode === "booting" ? "booting" : "gone",
        };
      }
      default:
        throw new Error(`demo 沒有實作 ${cmd}`);
    }
  };
}

// `1` 是平常那一天；`cut` 是被 LIMIT 切掉的那一天；`cross` 是那段解除時刻
// 落在明天的暫停；`live` 是「清空之後那一列空殼，是因為她正在錄」；
// `booting` 是同一列空殼，可是佔著目錄的那個 recorder 還在開資料庫——那一列
// 是**上一次當機**留下來的，不是它的；`nulldata` 是「她跑過，可是一列內容都
// 沒存進來過」——那一頁和「你把東西都忘掉了」以前逐字相同，而它們的下一步剛
// 好相反。六個各自對應一條在這之前**畫不出來**的列——`cut` 那一列以前印的是
// 「接下來 N 小時沒有新的東西進來」（假的），`cross` 那一句（「跨過午夜」）
// 則是寫在 `pauseWords` 裡但永遠選不到，因為後端的 SQL 把那筆 resume 篩掉
// 了，而 `leftover` 那三支要一台 Windows 加一次當機才分得出來——`booting` 那
// 一支還要那台機器的資料庫大到 `Db::open` 跑得完一杯咖啡。
// 開發機開不起 Tauri，看不到就等於沒做過。
const demo = new URLSearchParams(globalThis.location.search).get("demo");
if (
  demo === "1" ||
  demo === "cut" ||
  demo === "cross" ||
  demo === "live" ||
  demo === "booting" ||
  demo === "nulldata"
) {
  invoke = fakeBackend(demo);
  // 「現在」推到這一天之後，好讓收尾那段空白也畫出來——16:40 之後的七個
  // 小時是這一頁最容易被漏掉的一塊，看不到就等於沒做過。
  Date.now = () => Date.UTC(2026, 7, 19);
}
void load();
