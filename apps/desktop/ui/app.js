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
});

const avatar = document.querySelector("[data-avatar]");
const stateLine = document.querySelector("[data-state-line]");
const askInput = document.querySelector("[data-ask-input]");
const askSend = document.querySelector("[data-ask-send]");
const pinButton = document.querySelector("#pin");
const hideButton = document.querySelector("#hide");
const pauseButton = document.querySelector("#pause");

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

function paint() {
  // 暫停壓過一切。她沒在看的時候，畫面上絕不可以有一格看起來像在看。
  const shown = paused ? "paused" : state;
  avatar.dataset.state = shown;

  // 暫停時仍然答得出問題——停的是「記錄」，不是「記憶」。所以 thinking
  // 要講出來，只是講在文字上，不動那個灰掉的身體。
  const line =
    paused && state === "thinking" ? "想一下…（仍在暫停）" : STATE_LINES[shown];
  stateLine.textContent = line;
  // 讀螢幕的人也要知道她在忙，不然「想一下…」只是給看得見的人看的。
  avatar.setAttribute("aria-label", `AI-Sister：${line}`);
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

function renderHits(hits) {
  hitList.replaceChildren();

  if (hits.length === 0) {
    const empty = document.createElement("li");
    empty.className = "hits-empty";
    empty.textContent = "這件事我沒看到過。";
    hitList.append(empty);
  }

  for (const hit of hits) {
    const li = document.createElement("li");
    li.className = "hit";

    const text = document.createElement("p");
    text.className = "hit-text";
    renderSnippet(text, hit.snippet || hit.text);
    li.append(text);

    // 出處。她說的每一句都要指得回去——沒有這一行就只是另一個會唬爛的東西。
    const source = document.createElement("p");
    source.className = "hit-source";

    const time = document.createElement("span");
    time.className = "when";
    time.textContent = when(hit.ts);
    source.append(time);

    for (const part of [hit.app, hit.title, hit.url]) {
      if (!part) continue;
      const span = document.createElement("span");
      span.textContent = part;
      source.append(span);
    }

    // 文字保留 365 天、畫面 30 天，所以「字還在但圖沒了」是正常狀態。
    // 那時候要直說，不是留一個點不開的連結。
    if (hit.frame_id === null || hit.frame_id === undefined) {
      const gone = document.createElement("span");
      gone.className = "no-frame";
      gone.textContent = "畫面已過保留期";
      source.append(gone);
    } else {
      // 有圖的才點得開。「看起來能點但點了沒反應」比「看得出來不能點」差。
      li.classList.add("openable");
      li.tabIndex = 0;
      li.title = "點開看當時的畫面";
      const open = () => void invoke?.("open_frame", { frameId: hit.frame_id });
      li.addEventListener("click", open);
      li.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          open();
        }
      });
    }

    li.append(source);
    hitList.append(li);
  }

  hitList.hidden = false;
  document.body.classList.add("has-hits");
}

async function ask() {
  const question = askInput.value.trim();
  if (question === "") return;

  setState("thinking");
  try {
    if (invoke === null) throw new Error("這一頁不是在 AI-Sister 裡打開的");
    renderHits(await invoke("ask", { question }));
    setState("idle");
  } catch (err) {
    // 失敗要說出是什麼失敗。「沒有結果」跟「還沒錄過任何東西」跟「資料庫
    // 打不開」是三件不同的事，混成一句「查不到」等於把問題藏起來。
    setState("idle");
    stateLine.textContent = String(err?.message ?? err);
  }
}

askSend?.addEventListener("click", () => void ask());
askInput?.addEventListener("keydown", (event) => {
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
setState(wanted === "paused" ? "idle" : wanted);

// 開場先問一次磁碟。暫停**不會自己過期**，所以「上禮拜按了暫停」是一條真實
// 的路——開起來就該是灰的，而不是先亮一下再變灰。
if (invoke !== null) {
  invoke("pause_state").then(setPaused, () => {});
}

if (params.get("hits") === "demo") {
  renderHits([
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
  ]);
}
