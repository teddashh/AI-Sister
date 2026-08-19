// 字母人的行為。純 ES module，**沒有打包步驟**——這個檔案就是瀏覽器讀到的
// 那個檔案。Phase 1 的退場條件裡有一條「clone → 跑起來 < 10 分鐘」，而在一個
// Rust repo 裡塞一套 Node 工具鏈是最容易讓那條過不了的東西。
//
// 需要型別的時候再加 tsc，那是改一個檔案的事；現在還不需要。

/**
 * 她**正在做什麼**。注意 `paused` 不在這裡：它是另一個維度（見下面的
 * `paused`），因為她可以「暫停中、同時正在想一個問題的答案」。
 */
const STATES = Object.freeze(["idle", "thinking"]);

const STATE_LINES = Object.freeze({
  idle: "在聽",
  thinking: "想一下…",
  paused: "已暫停，沒有在看",
  // 「她今天不會記得任何事」是一句這一頁證明不了的話：這裡手上只有「現在
  // 沒有人在錄」。早上錄了四小時、中午按停的話，那四小時她記得清清楚楚——
  // 而底下的 `asleepDetail()` 正好會印著「上一次 12:00 停的：你按了停止」，
  // 自己打自己。改成只講從現在起，那句話對每一種過去都成立。
  asleep: "沒有人在記錄——從現在起發生的事，她不會知道",
});

const avatar = document.querySelector("[data-avatar]");
const stateLine = document.querySelector("[data-state-line]");
const askInput = document.querySelector("[data-ask-input]");
const askSend = document.querySelector("[data-ask-send]");
const pinButton = document.querySelector("#pin");
const hideButton = document.querySelector("#hide");
const pauseButton = document.querySelector("#pause");
const timelineButton = document.querySelector("#timeline");
const wakeButton = document.querySelector("[data-wake]");

/**
 * Tauri 的 IPC。**在瀏覽器裡打開時是 null**，而那是刻意支援的：字母人整個
 * 是 HTML/CSS，所以它可以在一般瀏覽器裡開發、截圖、比對，不必每次都去開一個
 * 桌面視窗。所有會用到 IPC 的地方都要能在 null 之下安靜地降級。
 */
const invoke = globalThis.__TAURI__?.core?.invoke ?? null;

// ---------- 狀態 ----------

/**
 * 兩個獨立的東西，不是三選一：
 *
 * - `state` 是她**正在做什麼**（在聽／想一下）。
 * - `paused` 是她**有沒有在看**，而且真相不在這個行程裡，是 data dir 裡的
 *   一個檔案（見 sister-core 的 `pause` 模組）。系統匣、上一次開機、甚至
 *   使用者自己去刪檔案，都能改變它。
 *
 * 混成一個變數的話會出現一個很難發現的 bug：暫停中問一句話 → 進 thinking →
 * 答完回 idle → **暫停的樣子不見了，但她其實還在暫停**。
 */
let state = "idle";
let paused = false;

/**
 * 現在到底有沒有人在錄。**這和 `paused` 是兩件事。**
 *
 * `sister record` 是另一個執行檔。沒有人把它跑起來的時候，暫停旗標是乾淨的
 * ——於是這個視窗以前會顯示「在聽」，而她其實什麼都沒在看。那是這個產品唯一
 * 不能說的那種謊：他照著那三個字相信她記得住今天，然後某天問「剛剛發生
 * 什麼事」，得到一片空白，然後以為是搜尋壞了。
 *
 * 兩件事要分開講，因為**下一步不一樣**：暫停要按「繼續」，沒在錄要去開
 * recorder。混成一句「她沒在看」等於把解法藏起來。
 *
 * 初值是 `true`：後端那一支已經是「不確定就回沒在錄」，這裡的初值只是第一次
 * 問到答案之前的那幾毫秒要畫什麼。開場閃一下「沒有人在記錄」比閃一下「在聽」
 * 更容易嚇到人，而它們一樣快就會被真正的答案蓋掉。
 */
let recording = true;

/**
 * 按了「開始記錄」之後、她真的開始之前的那一段。
 *
 * 這一段可能要好幾秒：另一個行程要載入、開資料庫、跑 migration。這期間畫面上
 * 不能還寫著「沒有人在記錄」配一顆按得下去的按鈕——他會再按一次，然後就有兩個
 * recorder 各錄一份，而唯一看得出來的症狀是磁碟用得比講好的快一倍。
 */
let starting = false;

/**
 * 上一場錄製是什麼時候、為什麼結束的（`null` = 還不知道／她沒錄過）。
 *
 * 「沒有人在記錄」後面永遠跟著同一個問題：那她是什麼時候停的？沒有這一句的
 * 話，同一句灰字既可能是「你十分鐘前自己按了停止」，也可能是「她昨天半夜
 * 當掉了，你今天一整天都不在」——而只有後者需要你做什麼。
 */
let lastRun = null;

/**
 * 上一次叫她起來、等到逾時都還沒看到心跳（`null` = 沒發生過，或後來好了）。
 *
 * 這一句以前是直接寫進 `stateLine.textContent` 的，而 5 秒後的下一輪輪詢會
 * 呼叫 `paint()` 把它蓋回「沒有人在記錄」——**唯一說得出原因的那一句，活不到
 * 他讀完**，剩下的畫面和「他根本沒按過」長得一模一樣。所以它得是狀態，跟著
 * 每一次重畫一起出現，直到她真的起來為止。
 */
let wakeFailed = null;

/** 灰掉那一刻要說的第二句話。她在錄的時候不講——那是現在式，不是回顧。 */
function asleepDetail() {
  if (lastRun === null) return "";
  const at = (ts) => when(ts).slice(5); // 年份對這句話沒有用
  if (lastRun.ended_at === null || lastRun.ended_at === undefined) {
    return `上一次從 ${at(lastRun.started_at)} 開始，沒有好好結束`;
  }
  if (lastRun.why) return `上一次 ${at(lastRun.ended_at)} 停的：${lastRun.why}`;
  // 沒有理由有兩種：那一版還沒在記，或者記了、後來被保留期／`sister forget`
  // 清掉。後者要說「查不出來了」——把它講成沉默，等於默認前者。
  return lastRun.why_gone
    ? `上一次 ${at(lastRun.ended_at)} 停的（為什麼停已經跟著那段紀錄一起被清掉了）`
    : `上一次 ${at(lastRun.ended_at)} 停的`;
}

function paint() {
  // 順序就是嚴重程度。她沒在看的時候，畫面上絕不可以有一格看起來像在看，
  // 而「被你叫停」要壓過「根本沒人開她」——前者是他做的決定，後者只是狀態。
  const shown = paused ? "paused" : recording ? state : "asleep";
  avatar.dataset.state = shown;

  // 暫停時仍然答得出問題——停的是「記錄」，不是「記憶」。所以 thinking
  // 要講出來，只是講在文字上，不動那個灰掉的身體。沒在錄的時候同理：
  // 她答得出以前記下來的東西。
  const line = starting
    ? "正在把她叫起來…"
    : state === "thinking" && shown !== "thinking"
      ? `想一下…（${shown === "paused" ? "仍在暫停" : "但沒有人在記錄"}）`
      : STATE_LINES[shown];
  // 灰掉的時候多講一句「上一次是什麼時候、為什麼停的」。換行不換句：那是
  // 同一件事的後半段，而 `.state-line` 的 `pre-line` 讓它自己排。
  //
  // 剛剛才叫不起來的話，那一句蓋過「上一次是什麼時候停的」——他現在要處理的
  // 是眼前這一次沒起來，不是上禮拜那一場怎麼收的。
  const detail =
    !starting && shown === "asleep" ? (wakeFailed ?? asleepDetail()) : "";
  stateLine.textContent = detail === "" ? line : `${line}\n${detail}`;
  // 讀螢幕的人也要知道她在忙，不然「想一下…」只是給看得見的人看的。
  avatar.setAttribute("aria-label", `AI-Sister：${line}`);

  if (wakeButton) {
    // 只在真的沒人在錄的時候出現。暫停中不出現——那時候的下一步是按 ▶，
    // 而不是再開一個 recorder（那會變成兩個行程各錄一份）。
    wakeButton.hidden = shown !== "asleep" || starting;
  }
}

function setState(next) {
  if (!STATES.includes(next)) return;
  state = next;
  paint();
}

function setPaused(next) {
  paused = next === true;
  if (pauseButton) {
    pauseButton.textContent = paused ? "▶" : "⏸";
    pauseButton.title = paused ? "繼續記錄" : "暫停記錄";
    // 「按下去了」= 暫停中。CSS 會把沒按下的那顆調淡，所以暫停時它最亮——
    // 這正是我們要的：不正常的狀態要吵。
    pauseButton.setAttribute("aria-pressed", String(paused));
  }
  paint();
}

function setRecording(next) {
  const was = recording;
  recording = next === true;
  // 她自己起來了（或是從別的地方被開起來的），那個「正在叫她」的等待就結束了，
  // 而「上一次叫不起來」也就過期了——她現在人在這裡，那句話再留著只會嚇人。
  if (recording) {
    starting = false;
    wakeFailed = null;
  }
  // 剛剛才停下來：現在才有一場「上一次」可以講，而它跟開場時讀到的那一場
  // 不是同一場。停了才問，因為在錄的時候問到的會是**這一場**（沒有收尾），
  // 而畫面會把它讀成「她當掉了」。
  if (was && !recording) refreshLastRun();
  paint();
}

/**
 * 等她把第一個心跳蓋出來。
 *
 * 心跳是 5 秒一次，但 recorder **在開資料庫之前就先蓋一次**（`ops::BootBeat`），
 * 所以正常情況下一秒內就看得到——包括那顆存了一年文字、migration 要跑好幾分鐘
 * 的資料庫。用 400 ms 去問是因為這一段有人正盯著看。
 *
 * 25 秒到了不代表她起不來，只代表**這 25 秒裡沒有心跳**。以前那句「她沒有
 * 起來」是一個猜測，而它會連著一顆放回來的按鈕一起出現——他再按一下，第二個
 * `sister record` 就打同一顆資料庫。所以逾時只換句話：說我看到什麼，別說她
 * 怎麼了。真的起來了，那 5 秒一輪的輪詢會接住。
 */
const WAKE_POLL_MS = 400;
const WAKE_TIMEOUT_MS = 25000;

async function startRecording() {
  if (invoke === null || starting) return;
  starting = true;
  // 這一次的結果還沒出來，上一次的判斷就不算數了。
  wakeFailed = null;
  paint();
  try {
    await invoke("start_recording");
  } catch (err) {
    // 同意書沒簽、找不到 sister.exe、已經有一個在跑——這三句都是後端寫好的
    // 完整句子，直接放上去。
    starting = false;
    paint();
    stateLine.textContent = String(err?.message ?? err);
    return;
  }
  const deadline = Date.now() + WAKE_TIMEOUT_MS;
  while (starting && Date.now() < deadline) {
    await new Promise((done) => setTimeout(done, WAKE_POLL_MS));
    // `setRecording(true)` 會把 `starting` 關掉，迴圈自己就結束了。
    try {
      setRecording(await invoke("recording_state"));
    } catch {
      // 問不到就下一輪再問。這裡不該因為一次 IPC 失敗就宣告她沒起來。
    }
  }
  if (!starting) return;
  // 逾時了。如果她中途死了，理由已經寫在 record.log 裡——而那個檔案在
  // %APPDATA% 深處，一個看著沒反應的按鈕的人不會去翻它，所以直接端過來。
  let why = "";
  try {
    why = await invoke("recorder_log_tail");
  } catch {
    // 連記錄檔都讀不到，那就只剩下面那句話。
  }
  const waited = Math.round(WAKE_TIMEOUT_MS / 1000);
  wakeFailed = why
    ? `等了 ${waited} 秒還沒有心跳。record.log 最後說：\n${why}`
    : `等了 ${waited} 秒還沒有心跳，record.log 也還是空的。` +
      "她可能還在起來——再等一下，或去看那個檔案";
  starting = false;
  paint();
}

wakeButton?.addEventListener("click", startRecording);

/**
 * 有沒有人在錄，隨時可能變——他會在另一個終端機視窗裡把 `sister record`
 * 開起來或按 Ctrl+C。所以要一直問，而且**問的節奏要跟得上心跳**
 * （`heartbeat::BEAT_EVERY_MS` 是 5 秒）。
 *
 * 視窗看不到的時候不問。一個整天掛在角落的東西，在沒有人看的時候還每 5 秒
 * 醒來一次，就是 Phase 0 那份 CPU 預算的漏洞——和動畫閘門同一條紀律。
 */
const RECORDING_POLL_MS = 5000;
let pollTimer = null;

/**
 * 「有沒有在錄」和「有沒有被暫停」要一起問。
 *
 * 暫停旗標以前只在開場問一次，之後只靠 `pause-changed` 事件更新——而那個
 * 事件**只有這個行程自己按下去的時候才會發**。旗標是磁碟上的一個檔案，另外
 * 有三個人會動它：`sister pause`、`sister resume`，還有 recorder 自己印出來
 * 的那句「或刪掉 …\paused.flag」。
 *
 * 所以在終端機裡 `sister resume` 之後，她其實已經在錄了，而字母人會一直灰著
 * 說「已暫停，沒有在看」。更糟的是那顆 ▶：`toggle_pause` 讀的是磁碟，所以按
 * 下「繼續記錄」實際上是把她**暫停**——而畫面本來就畫成暫停的樣子，按完什麼
 * 都不會變。旁邊那句註解說得很清楚：顯示成已暫停、實際上還在錄，是這個產品
 * 能犯的最嚴重的一種謊。反過來這一種同樣是它。
 *
 * 磁碟上的旗標是真相，這個視窗只是鏡子——和 `ask` 每次重讀設定檔同一條紀律。
 */
function pollRecording() {
  if (invoke === null) return;
  invoke("recording_state").then(setRecording, () => {});
  invoke("pause_state").then(setPaused, () => {});
}

/**
 * 去問「上一場是怎麼結束的」。
 *
 * 只在她從「有在錄」掉到「沒在錄」的那一刻問，外加開場問一次——不是每 5 秒
 * 問一次。這是一個**不會再變**的事實（下一次改變的時候她已經在錄了，那時候
 * 這句話不會被顯示），而每一次呼叫都要開資料庫查一次。
 */
function refreshLastRun(retry = true) {
  if (invoke === null) return;
  invoke("last_recording_end").then(
    (run) => {
      lastRun = run ?? null;
      paint();
      // recorder **先收心跳、再寫收尾**（那個順序是對的：寫資料庫可能失敗，
      // 而失敗不該讓一個錯的「她還在錄」留在磁碟上）。所以有一個很窄的窗口，
      // 我們會在收尾寫進去之前就問到——而答案會是「沒有好好結束」，也就是
      // 說她當掉了。那是一句嚇人的話，不能用猜的。再問一次。
      if (retry && lastRun !== null && (lastRun.ended_at ?? null) === null) {
        setTimeout(() => refreshLastRun(false), 1500);
      }
    },
    () => {},
  );
}

function updatePollGate() {
  const visible = document.visibilityState === "visible";
  if (visible && pollTimer === null) {
    pollRecording();
    pollTimer = setInterval(pollRecording, RECORDING_POLL_MS);
  } else if (!visible && pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

// ---------- 動畫閘門 ----------

const reducedMotion = globalThis.matchMedia?.("(prefers-reduced-motion: reduce)");

/**
 * 所有動畫由 `<html>` 上的一個 class 控制。
 *
 * 視窗被蓋住的時候要停：一個常駐在最上層、整天都在的視窗，如果在沒有人看的
 * 時候還在跑 compositor，那就是 Phase 0 那份 CPU 預算的直接漏洞。
 */
function updateMotionGate() {
  const allowed =
    document.visibilityState === "visible" && reducedMotion?.matches !== true;
  document.documentElement.classList.toggle("motion", allowed);
}

document.addEventListener("visibilitychange", updateMotionGate);
document.addEventListener("visibilitychange", updatePollGate);
reducedMotion?.addEventListener?.("change", updateMotionGate);

// ---------- 相位 ----------

/**
 * 搖晃的起始相位。用負的 delay 讓動畫「已經跑到一半」，這樣每次開視窗她不會
 * 都從同一個姿勢開始——那種同步感會讓她看起來像一個剛被 render 出來的元件，
 * 而不是一個本來就在那裡的東西。
 */
function seedSwayPhase() {
  const seed = Math.floor(Math.random() * 4000);
  avatar.style.setProperty("--sway-delay", `${-seed}ms`);
}

// ---------- 視窗控制 ----------

let pinned = true;

function paintPin() {
  pinButton.textContent = pinned ? "●" : "○";
  pinButton.setAttribute("aria-pressed", String(pinned));
  pinButton.title = pinned ? "取消置頂" : "保持在最上層";
}

pinButton?.addEventListener("click", async () => {
  if (invoke === null) {
    pinned = !pinned;
    paintPin();
    return;
  }
  pinned = await invoke("toggle_pin");
  paintPin();
});

hideButton?.addEventListener("click", () => {
  void invoke?.("hide_to_tray");
});

timelineButton?.addEventListener("click", async () => {
  try {
    await invoke?.("open_timeline");
  } catch (err) {
    stateLine.textContent = String(err?.message ?? err);
  }
});

pauseButton?.addEventListener("click", async () => {
  if (invoke === null) {
    setPaused(!paused);
    return;
  }
  try {
    setPaused(await invoke("toggle_pause"));
  } catch (err) {
    // 切不動就**不要**改畫面。顯示成已暫停、實際上還在錄，是這個產品能犯的
    // 最嚴重的一種謊；寧可看起來沒反應，然後把原因寫出來。
    stateLine.textContent = String(err?.message ?? err);
  }
});

/**
 * 系統匣上也有同一顆暫停鍵，所以狀態可能從**這個視窗以外**改變。
 * 沒有這一段的話，從系統匣暫停之後，字母人會繼續一臉「我在聽」。
 */
globalThis.__TAURI__?.event
  ?.listen?.("pause-changed", (event) => setPaused(event.payload))
  ?.catch?.(() => {});

/**
 * 從系統匣按「開始記錄」失敗的時候，那句原因沒有地方可以寫。
 *
 * 同意書沒簽、找不到 `sister.exe`、已經有一個在跑——三句都是後端寫好的完整
 * 中文，而系統匣選單上沒有一格能放字。以前它們只進 `desktop.log`：按下去的
 * 後果是**什麼都沒發生**，而唯一說得出原因的那句話在一個他不會開的檔案裡。
 *
 * 借的是「叫不起來」那條路（[`wakeFailed`]）——對他來說是同一件事：她沒起來，
 * 這是為什麼。後端會順手把視窗叫出來，不然這句話還是沒有人看得到。
 */
globalThis.__TAURI__?.event
  ?.listen?.("recorder-failed", (event) => {
    wakeFailed = String(event.payload ?? "");
    starting = false;
    paint();
  })
  ?.catch?.(() => {});

// ---------- 答案 ----------

const hitList = document.querySelector("[data-hits]");

/**
 * 把 FTS 的片段標記（`[` `]`）變成 `<mark>`。
 *
 * **用 DOM 節點組，不用 `innerHTML`。** 這些字是從螢幕上 OCR 出來的，
 * 也就是說它們的內容完全由「使用者那天看了什麼」決定——一個瀏覽器分頁的
 * 標題就足以把 HTML 帶進來。她把看到的東西唸回來，不該順便把它執行掉。
 */
function renderSnippet(target, snippet) {
  let mark = false;
  for (const piece of snippet.split(/([[\]])/u)) {
    if (piece === "[") {
      mark = true;
    } else if (piece === "]") {
      mark = false;
    } else if (piece !== "") {
      const node = document.createTextNode(piece);
      if (mark) {
        const em = document.createElement("mark");
        em.append(node);
        target.append(em);
      } else {
        target.append(node);
      }
    }
  }
}

function when(ts) {
  const d = new Date(ts);
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
    d.getHours(),
  )}:${pad(d.getMinutes())}`;
}

/**
 * 出處那一行：時間、在哪個 app、哪個視窗、哪個網址，以及點不點得開。
 *
 * ★ 答案和原文共用同一份——她說的每一句都要指得回去，而「答案的出處長得跟
 * 原文的出處不一樣」只會讓人以為其中一種比較可信。
 *
 * @param li 那一列本身。點得開的時候要在它身上掛 class 和事件。
 * @param rank 這一筆在畫面上排第幾（從 0 起算）。
 */
function sourceLine(item, li, queryId, rank) {
  const source = document.createElement("p");
  source.className = "hit-source";

  const time = document.createElement("span");
  time.className = "when";
  time.textContent = when(item.ts);
  source.append(time);

  for (const part of [item.app, item.title, item.url]) {
    if (!part) continue;
    const span = document.createElement("span");
    span.textContent = part;
    source.append(span);
  }

  // 「字還在但沒有畫面」是正常狀態，不是壞掉：只簽了第一張同意書（只記字）、
  // 截圖節流、每日額度用完、或者圖過了保留期（文字 365 天、畫面 30 天）。
  // 這裡分不出是哪一種，所以就不猜——說得出口的只有「沒有」。
  if (item.frame_id === null || item.frame_id === undefined) {
    const gone = document.createElement("span");
    gone.className = "no-frame";
    gone.textContent = "沒有留下畫面";
    source.append(gone);
    return source;
  }

  // 有圖的才點得開。「看起來能點但點了沒反應」比「看得出來不能點」差。
  li.classList.add("openable");
  li.tabIndex = 0;
  li.title = "點開看當時的畫面";
  const open = () => {
    void invoke?.("open_frame", { frameId: item.frame_id });
    // 他點下去的那一刻，等於幫這一題標了正解——而 `rank` 說出排序把它放
    // 在第幾個。那是檢索品質唯一不必人工標註就拿得到的訊號（PHASES.md
    // Phase 2 的題庫要 ≥ 30 題來自這裡）。
    //
    // 失敗完全不理：他要的是那張畫面。一個因為記不了統計而不肯開圖的
    // 產品，把手段當成了目的。
    // 沒有題號、或這一筆說不出自己是從哪一段字來的，就只開圖不記帳。
    if (
      queryId !== null &&
      queryId !== undefined &&
      item.chunk_id !== null &&
      item.chunk_id !== undefined
    ) {
      void invoke?.("log_click", {
        queryId,
        chunkId: item.chunk_id,
        rank,
      })?.catch?.(() => {});
    }
  };
  li.addEventListener("click", open);
  li.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open();
    }
  });
  return source;
}

/**
 * 一筆都沒找到的時候，除了「我沒看過」還講得出什麼。
 *
 * 全部靠查得到的東西：排除稽核、暫停稽核、她到底記過幾段字。猜的一律不講——
 * 「可能是那時候沒在看吧」對他沒有任何用處，而且有一半機率是假的。
 *
 * 她還沒開始記的時候只講那一件事：後面兩句在那個情況下都是廢話。
 */
function blindLines(blind) {
  if (!blind) return [];
  if (blind.chunks === 0) {
    // 「一段字都沒有」有兩種，而它們的下一步是相反的。看過畫面卻一個字都
    // 沒讀出來，代表讀字那一段斷了（關掉了、或者裝了讀不到）——那是這個
    // 專案已知的主要故障形狀。和 `blind_lines`（ops.rs）同一條分法。
    // 「連畫面都沒有」也還有兩種：從來沒錄過，或者錄過的東西被忘掉了／
    // 過了保留期。`sessions` 不在任何保留期的射程內，所以問得出來。
    if (blind.frames > 0) {
      return [`（我看過 ${blind.frames} 張畫面，但一個字都沒讀出來——讀字那一段是斷的。）`];
    }
    return blind.sessions > 0
      ? ["（我錄過，但現在什麼都不剩了——被忘掉了，或是過了保留期。）"]
      : ["（我到現在還沒記過任何東西。）"];
  }
  const out = [];
  if (blind.excluded?.length) {
    const why = blind.excluded.map(([reason, n]) => `${reason} ${n} 段`).join("、");
    // 同一張稽核表裡還躺著兩道**自動**防線（`screenshare app:` 和
    // `password field focused`），那兩種他沒有寫過任何規則。講成他寫的，
    // 他會去三張排除清單裡找一條不存在的規則。理由字串帶著前綴，讓它自己說。
    // 和 `blind_lines`（ops.rs）同一句話。
    const his = blind.excluded.some(([reason]) => reason.startsWith("excluded "));
    const whose = his ? "你的排除規則（和自動防線）" : "自動防線";
    out.push(`不過${whose}擋掉過東西（${why}）——在那裡面的我本來就不會知道。`);
  }
  if (blind.paused_episodes > 0) {
    // 這裡本來就不印時間，所以躲過了 `paused_ms` 那個「一共 0 秒」的坑。
    // 但「還沒解除」是另一回事：那不是過去式，是**現在**——他問的東西如果
    // 是暫停之後發生的，她根本沒有機會看到。和 `blind_lines`（ops.rs）同一條。
    out.push(
      blind.paused_open
        ? `我也被暫停過 ${blind.paused_episodes} 次，而且最後那一次到現在都還沒解除——我此刻就是閉著眼睛的。`
        : `我也被暫停過 ${blind.paused_episodes} 次，那幾段是空的。`,
    );
  }
  return out;
}

/**
 * @param hits 一筆一筆的原文。
 * @param kind `"keywords"`（比對字找到的）或 `"recent"`（他問的是時間）。
 *   這個字是後端給的，不是這裡判斷的——同一句話在 `sister query` 和這一頁
 *   必須得到同一種答案，所以規則只有一份，在 sister-core 的 `question`。
 * @param facts L1 直接答得出來的那幾筆（★）。排在原文前面，因為那才是他問
 *   的東西本身：問「電話」要的是號碼，不是一段剛好提到電話的字。
 * @param blind 兩手空空時，她查得到的那幾個理由（後端給事實，句子在這裡組）。
 * @param truncated 原文底下還有，只是沒送過來。捲到底那一句要靠它。
 * @param factsTruncated ★ 那一半也被切掉了。分開一個參數是因為兩邊的下一步
 *   不一樣：原文要 `--limit`，★ 十個不同的答案代表問法太寬。
 */
function renderHits(
  hits,
  kind,
  queryId = null,
  facts = [],
  blind = null,
  truncated = false,
  factsTruncated = false,
) {
  hitList.replaceChildren();

  // 他打了「剛剛發生什麼事」，而底下這幾筆跟那七個字一個都對不上。不先講
  // 一句「我把它當成時間問題了」，看起來就只是她答非所問。
  if (kind === "recent" && hits.length > 0) {
    const note = document.createElement("li");
    note.className = "hits-note";
    note.textContent = "你問的是「剛剛」，所以我沒有去比對字——這是我最後看到的幾件事：";
    hitList.append(note);
  }

  // SPEC §8.2 的語氣規範：「我最後看到的是…」，不准講成斷言。★ 那幾筆最需要
  // 這一句——一個孤零零的號碼看起來像一句「這就是答案」，而她知道的只有
  // 「我在某個時間點的螢幕上看過它」。問「昨天的金額」而她給的是今天那筆的
  // 時候，這一行就是那個差別。
  if (facts.length > 0) {
    const note = document.createElement("li");
    note.className = "hits-note";
    note.textContent = "我最後看到的是：";
    hitList.append(note);
  }

  for (const [rank, fact] of facts.entries()) {
    const li = document.createElement("li");
    li.className = "hit fact";

    const value = document.createElement("p");
    value.className = "fact-value";
    value.textContent = fact.value;
    // 1 次和 12 次是強度不同的答案。她自己不下判斷，只把數字講出來。
    if (fact.sightings > 1) {
      const seen = document.createElement("span");
      seen.className = "fact-seen";
      seen.textContent = `看過 ${fact.sightings} 次`;
      value.append(seen);
    }
    li.append(value);

    // 正規化後的值認得出來，原文才認得出**場景**——`+886800080123` 是機器
    // 要的，`客服專線 0800-080-123` 才是他記得的那一行。兩個都給。
    const raw = document.createElement("p");
    raw.className = "hit-text fact-raw";
    raw.textContent = fact.raw;
    li.append(raw);

    li.append(sourceLine(fact, li, queryId, rank));
    hitList.append(li);
  }

  // ★ 那一半也會被切掉，而這裡以前什麼都沒說。理由曾經寫成「十個不同答案
  // 代表問題出在問法」——那句話對，但它把「她只知道這十個」和「她知道更多、
  // 只是沒送過來」壓成同一個畫面，而那正是隔壁那一句存在的全部理由。
  if (factsTruncated) {
    const more = document.createElement("li");
    more.className = "hits-note hits-more";
    more.textContent = "還有別的答案沒列出來——問得再具體一點，或用 sister facts 看全部。";
    hitList.append(more);
  }

  if (hits.length === 0 && facts.length === 0) {
    const empty = document.createElement("li");
    empty.className = "hits-empty";
    // 「我沒看過這件事」和「我什麼都還沒看過」是兩件不同的事。問時間卻空手
    // 而回，唯一的解釋是她根本還沒錄過東西——那時候該講的是下一步，不是
    // 「查無此資料」。
    //
    // 這一句本來是「這件事我沒看到過。」——一句關於**世界**的斷言，而她
    // 唯一有資格講的是關於**她自己的紀錄**的話（SPEC §8.2，和 ★ 上面那句
    // 「我最後看到的是：」同一條紀律）。那個差別不是措辭：東西可能就在螢幕
    // 上，只是被排除規則擋掉、被暫停跳過，或者 OCR 沒讀出來——最後這一種
    // 她連數都數不出來，所以下面那幾行理由永遠不會是完整的。
    empty.textContent =
      kind === "recent"
        ? "我什麼都還沒看到——要先跑 sister record 我才記得住。"
        : "我記得的東西裡沒有這件事。";
    hitList.append(empty);

    // 後端只給事實（排除過幾段、暫停過幾次），句子在這裡組。
    for (const line of blindLines(blind)) {
      const li = document.createElement("li");
      li.className = "hits-why";
      li.textContent = line;
      hitList.append(li);
    }
  }

  for (const [i, hit] of hits.entries()) {
    const li = document.createElement("li");
    li.className = "hit";

    const text = document.createElement("p");
    text.className = "hit-text";
    renderSnippet(text, hit.snippet || hit.text);
    li.append(text);

    // ★ 那幾筆排在上面，所以原文的第 0 筆在畫面上其實是第 facts.length 筆。
    // rank 要說的是**他在清單上往下看了多遠**，不是它在哪一個陣列裡的位置。
    li.append(sourceLine(hit, li, queryId, facts.length + i));
    hitList.append(li);
  }

  // 捲到底之後那一句。少了它，「底下沒有了」和「她只記得這些」長得一模一樣
  // ——而後者是他會下的結論，因為這個視窗就是拿來問她記得什麼的。
  //
  // 講得出下一步才有意義：她這裡沒有第二頁，`sister query` 有 `--limit`。
  if (truncated) {
    const more = document.createElement("li");
    more.className = "hits-note hits-more";
    // 反引號和角括號留給終端機。這一頁的規矩是直接寫 `sister record`
    // 那樣的裸指令（onboarding 和時間軸都是這樣寫的）。
    // 「看得到全部」是講不出口的：後端只知道「超過 20 筆」，沒有人數過總共
    // 幾筆。而 sister query 自己會印「100+ 筆（撈滿 100 筆就停了）」——一句
    // 剛剛才安慰過他「這樣就看得到全部了」的話，被下一個畫面當場打臉。
    more.textContent = "這裡最多列 20 筆，底下還有——sister query --limit 100 看得到更多。";
    hitList.append(more);
  }

  hitList.hidden = false;
  document.body.classList.add("has-hits");
}

/**
 * 慢到這個秒數還沒回來，就得換一句話講。
 *
 * 「想一下…」不動地停在那裡，跟她整個卡死長得一模一樣——這正是她第一次跑在
 * 真 Windows 上時發生的事：第一個問題觸發了資料庫升級（要把整張表重算一次
 * bigram），畫面就停在「想一下…」，看不出是還在跑還是死了。那個成因已經修掉
 * 了（命令離開了主執行緒，而且開機就先去開資料庫），但「久到沒話講」這件事
 * 本身仍然要有出口。
 */
const SLOW_MS = 4000;

/**
 * 最後一次發問的編號。**只有最新的那一份答案算數。**
 *
 * 慢的時候人會多按幾次 Enter，而兩次查詢回來的順序不保證跟送出去的一樣——
 * 先送的後回，畫面上就會留著舊問題的答案，配著新問題的輸入框。那種錯不會
 * 有任何症狀，只是她答錯了，而他不會知道。
 */
let asking = 0;

async function ask() {
  const question = askInput.value.trim();
  if (question === "") return;

  const mine = ++asking;
  setState("thinking");
  const slow = setTimeout(() => {
    if (state === "thinking") {
      stateLine.textContent = "還在翻…（第一次打開資料庫要先整理索引）";
    }
  }, SLOW_MS);

  try {
    if (invoke === null) throw new Error("這一頁不是在 AI-Sister 裡打開的");
    const answer = await invoke("ask", { question });
    // 這一份過期了。畫面歸還在跑的那一次管，這裡連 idle 都不要設。
    if (mine !== asking) return;
    renderHits(
      answer.hits,
      answer.kind,
      answer.query_id,
      answer.answers,
      answer.blind,
      answer.truncated,
      answer.answers_truncated,
    );
    setState("idle");
    // 答完才清掉。失敗的時候留著，他才不用把整句話重打一次。
    askInput.value = "";
  } catch (err) {
    if (mine !== asking) return;
    // 失敗要說出是什麼失敗。「沒有結果」跟「還沒錄過任何東西」跟「資料庫
    // 打不開」是三件不同的事，混成一句「查不到」等於把問題藏起來。
    setState("idle");
    stateLine.textContent = String(err?.message ?? err);
  } finally {
    clearTimeout(slow);
  }
}

askSend?.addEventListener("click", () => void ask());
askInput?.addEventListener("keydown", (event) => {
  // 選字中的 Enter 是「就選這個字」，不是「問出去」。注音打「剛剛發生什麼事」
  // 一路上會按好幾次 Enter，少了這一行，第一次選字就把半句話送出去了。
  // `keyCode === 229` 是舊的那條路，有些 IME 只給得出這個。
  if (event.isComposing || event.keyCode === 229) return;
  if (event.key === "Enter") void ask();
});

// ---------- 開場 ----------

// 開發用的兩個開關：`?state=paused` 直接看某一個狀態，`?hits=demo` 看
// 有答案時的版面。**Tauri 載入時沒有 query string**，所以這兩條路在產品裡
// 走不到——它們存在的理由是這台開發機開不起 Tauri 視窗（沒有 webkit2gtk、
// 沒有 sudo），而版面對不對不該等到上了 Windows 才第一次看到。
const params = new URLSearchParams(globalThis.location.search);

seedSwayPhase();
updateMotionGate();
paintPin();

// `?state=paused` 走的是**和產品一樣的那條路**（設 `paused` 旗標），不是另外
// 搬一個長得像暫停的樣子出來。這一點是被截圖抓到的：第一版讓它去設 `state`，
// 於是截出來的圖裡字母人是灰的、但拖曳條上的暫停鍵還是「⏸」——而截圖是這台
// 機器上唯一看得到 UI 的方式，一個走假路的開發開關會讓它騙我。
const wanted = params.get("state") ?? "idle";
setPaused(wanted === "paused");
// `?state=asleep`：沒有人在跑 `sister record`。和上面同一條紀律——走真正的
// 那個旗標，不另外搬一個長得像的樣子出來。
setRecording(wanted !== "asleep");
setState(wanted === "paused" || wanted === "asleep" ? "idle" : wanted);

// 開場先問一次磁碟。暫停**不會自己過期**，所以「上禮拜按了暫停」是一條真實
// 的路——開起來就該是灰的，而不是先亮一下再變灰。
//
// 之後由 `pollRecording` 每 5 秒接手（它同時問這兩件事）。這一行留著是因為
// 視窗如果一開始就縮在系統匣裡，那個輪詢是不跑的。
if (invoke !== null) {
  invoke("pause_state").then(setPaused, () => {});
}

// 有沒有人在錄要一直問下去，不是問一次就算了：他隨時可能在另一個終端機
// 裡把 recorder 開起來或按 Ctrl+C，而這個視窗是他判斷「她到底有沒有在看」
// 的唯一依據。
updatePollGate();

// 開場也問一次「上一場是怎麼結束的」：最常見的情況是他早上打開電腦、看到
// 灰掉的她，而昨晚那一場是怎麼結束的正是這時候要回答的問題。
//
// 排在 `updatePollGate()` 後面：那一行會同步問一次 `recording_state`，所以
// 現在正在錄的話，畫面已經不是灰的了，這一句根本不會被顯示。
refreshLastRun();

// `?asleep=stopped` / `?asleep=crashed`：那句灰字底下的第二行。這台機器開不起
// Tauri，而「她昨晚當掉了」和「你自己按了停止」長得該不一樣——那是要用眼睛看的。
if (params.get("asleep") === "stopped") {
  lastRun = {
    started_at: Date.now() - 4 * 3600 * 1000,
    ended_at: Date.now() - 90 * 60 * 1000,
    why: "你按了停止",
  };
  paint();
} else if (params.get("asleep") === "crashed") {
  lastRun = {
    started_at: Date.now() - 19 * 3600 * 1000,
    ended_at: null,
    why: null,
  };
  paint();
} else if (params.get("asleep") === "nobeat") {
  // 叫了、逾時了、還是沒有心跳。這一句要活得比一次輪詢久（以前它被下一個
  // `paint()` 蓋掉），而它換行、比另外兩句長——版面撐不撐得住要用眼睛看。
  wakeFailed =
    "等了 25 秒還沒有心跳。record.log 最後說：\n" +
    "（這一輪還沒寫出東西，以下是上一輪的 record.log）\n" +
    "第一張同意書還沒簽——她不會開始記錄。";
  paint();
}

if (params.get("hits") === "demo") {
  renderHits(
    [
      {
        ts: Date.now() - 2 * 3600 * 1000,
        snippet: "中華電信 [客服]專線 0800-080-123 帳單問題請按 2",
        text: "",
        app: "chrome.exe",
        title: "帳單查詢",
        url: "https://example.com/bill",
        frame_id: 41,
      },
      {
        ts: Date.now() - 3 * 86400 * 1000,
        snippet: "轉接 [客服]，等候時間約 4 分鐘",
        text: "",
        app: "Teams.exe",
        title: "通話中",
        url: null,
        frame_id: null,
      },
    ],
    "keywords",
    null,
    // ★ 那一層。這正是 `sister query 電話` 一直答得出、而她以前答不出來的
    // 東西：螢幕上寫的是「客服**專線**」，比對「電話」兩個字永遠接不起來。
    [
      {
        value: "+886800080123",
        raw: "客服專線 0800-080-123",
        sightings: 3,
        ts: Date.now() - 2 * 3600 * 1000,
        chunk_id: 12,
        frame_id: 41,
        app: "chrome.exe",
        title: "帳單查詢",
        url: "https://example.com/bill",
      },
      {
        value: "+886912345678",
        raw: "手機 0912-345-678",
        sightings: 1,
        ts: Date.now() - 5 * 86400 * 1000,
        chunk_id: 9,
        frame_id: null,
        app: "Teams.exe",
        title: "通話中",
        url: null,
      },
    ],
    null,
    // 捲到底那兩句也要看得到。假資料裡不放這一種，就等於少驗一種情況——
    // 而這一頁能驗的只有截圖。★ 那一句和原文那一句講的下一步不一樣。
    true,
    true,
  );
}

// `?hits=recent` 是「剛剛發生什麼事」那條路的版面。分開一個開關而不是共用
// 上面那組假資料，是因為要看的正是**兩者長得不一樣**：時間問題多了一句說明，
// 而且答案裡不會有任何一個被標起來的字。
if (params.get("hits") === "recent") {
  renderHits(
    [
      {
        ts: Date.now() - 4 * 60 * 1000,
        snippet: "docker compose up -d 正在啟動 3 個容器",
        text: "",
        app: "WindowsTerminal.exe",
        title: "pwsh",
        url: null,
        frame_id: 88,
      },
      {
        ts: Date.now() - 11 * 60 * 1000,
        snippet: "第 2 季預算表 — 行銷 NT$412,000",
        text: "",
        app: "EXCEL.EXE",
        title: "budget-q2.xlsx",
        url: null,
        frame_id: 87,
      },
      {
        ts: Date.now() - 26 * 60 * 1000,
        snippet: "會議改到下午三點，會議室 B",
        text: "",
        app: "Teams.exe",
        title: "行銷部",
        url: null,
        frame_id: null,
      },
    ],
    "recent",
  );
}

// `?hits=none` 是兩手空空那一版。要看的是「我沒看到過」底下那幾句——它們是
// 這個畫面上唯一會讓他知道「東西可能在，只是我不准看」的地方。
if (params.get("hits") === "none") {
  renderHits([], "keywords", null, [], {
    chunks: 8421,
    excluded: [
      ["excluded url", 12],
      ["excluded app: keepassxc", 3],
    ],
    paused_episodes: 2,
    paused_ms: 4 * 3600 * 1000,
    // 最後一段還沒解除：這一版要看的就是那句「我此刻就是閉著眼睛的」。
    paused_open: true,
    paused_truncated: 0,
  });
}
