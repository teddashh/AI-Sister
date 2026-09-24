#!/usr/bin/env node
/*
 * 字母人上那幾句「為什麼沒成」，活得過下一次輪詢嗎。
 *
 * `pollRecording` 每 5 秒跑一次，而它會呼叫兩次 `paint()`（`setRecording` 和
 * `setPaused` 各一次）。`paint()` **無條件**覆寫 `stateLine.textContent`。所以
 * 任何直接寫那一格的人，話的壽命是 0 到 5 秒，而且不由它自己決定——那個計時
 * 器不會因為他剛按了按鈕就重設，所以「0 秒」是真的會發生的。
 *
 * 這一族在 alpha.38 之前有五個（開始記錄失敗、問問題失敗、暫停切不動、時間軸
 * 開不起來、那句「超過 4 秒」），其中兩個的原始碼註解自己就在描述這個 bug：
 * `wakeFailed` 那個欄位上面整段講的就是它，而暫停那條寫著「寧可看起來沒反應，
 * 然後把原因寫出來」——輪詢一到，只剩下前半句。
 *
 * 外加一條不同形狀的：問題答不成的時候那句「這一題我沒答成」被塞進
 * `[data-hits]`，而那個 `<ul>` 開場是 `hidden`，只有 `renderHits` 會拿掉。
 * 第一題就失敗的人整個畫面一個字都不會多。
 *
 * 作法和 `check-settings-say.mjs` 一樣：載入 `apps/desktop/ui/app.js` **原檔**，
 * 不是一份抄過來的邏輯。重畫用的是產品自己的路（`pause-changed` 事件 →
 * `setPaused` → `paint()`），和輪詢走的是同一條，只是不必真的等五秒。
 */

import { spawnSync } from "node:child_process";
import { runInNewContext } from "node:vm";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { domOf, fakeDocument, hiddenIn, loader, read, watchNonsense } from "./fake-dom.mjs";

// **這支閘門的時鐘釘死在 UTC，而且要釘在讀 app.js 之前。**
//
// `clock()` 和 `when()`（`app.js:4766`／`4774`）用的是 `getHours()`，吃的是這個
// 行程的時區。底下兩份 golden 快照裡有牆上的時鐘，所以不釘死的話，這支閘門
// 只在寫它的人那一個時區是綠的。
//
// 這不是假設，是量出來的：alpha.153 R1 的 golden 是在 EDT 上產生的，本機 53 條
// 閘門全綠推出去，CI（UTC）上 `938 passed; 2 failed` 紅了五個 commit 才被發現。
// 本機 `TZ=UTC` 跑同一支，逐字重現那兩條。
process.env.TZ = "UTC";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "app.js");
const MAIN = join(UI, "../src-tauri/src/main.rs");
const MASTER_STOP_DISPATCH = join(UI, "../src-tauri/src/master_stop_dispatch.rs");
const LADDER_WORDS = [
  ["glimmer", "微光形式沒有獨立分支"],
  ["one_line", "一行形式沒有獨立分支"],
  ["card", "建議卡形式沒有獨立分支"],
  ["這張記憶不會再提了", "結案後沒有說記憶不再提"],
  ["先收起來，之後再說", "其他回饋沒有說之後再說"],
  ["收到你的回饋", "沒有 commitment 可改時沒有第三句"],
];
for (const [word, why] of LADDER_WORDS) {
  if (!read(SRC).includes(word)) {
    console.log(`✗ ${why}：缺少 ${word}`);
    process.exit(1);
  }
}
// 只在測試副本觀察 renderHits 的例外，仍重拋給產品原本的 ask catch。
const boot = loader(read(SRC) + `
const renderForTest = renderHits;
const renderErrorsForTest = [];
globalThis.__renderErrorsForTest = renderErrorsForTest;
renderHits = (...args) => {
  try { return renderForTest(...args); }
  catch (error) { renderErrorsForTest.push(error); throw error; }
};
`);
const APP_SOURCE = read(SRC);

function functionRanges(source) {
  const ranges = [];
  for (const match of source.matchAll(/\b(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\([^)]*\)\s*\{/gu)) {
    const open = source.indexOf("{", match.index);
    let depth = 0;
    for (let cursor = open; cursor < source.length; cursor++) {
      if (source[cursor] === "{") depth++;
      else if (source[cursor] === "}" && --depth === 0) {
        ranges.push({ name: match[1], start: match.index, end: cursor + 1, body: source.slice(open + 1, cursor) });
        break;
      }
    }
  }
  return ranges;
}
const APP_FUNCTIONS = functionRanges(APP_SOURCE);
const hasHitsWrites = [];
for (const pattern of [
  /document\.body\.classList\.(?:add|remove)\(["']has-hits["']\)/gu,
  /\b(?:showAnswerHits|hideAnswerHits)\(\)/gu,
]) {
  for (const match of APP_SOURCE.matchAll(pattern)) {
    const owner = APP_FUNCTIONS.find(fn => match.index >= fn.start && match.index < fn.end);
    if (!owner || owner.name === "showAnswerHits" ||
        (match[0] === "hideAnswerHits()" && owner.name === "hideAnswerHits")) continue;
    hasHitsWrites.push(owner);
  }
}
const uniqueHasHitsWriters = [...new Map(hasHitsWrites.map(fn => [fn.name, fn])).values()];
const writersWithoutPaint = uniqueHasHitsWriters.filter(fn => !/\bpaintConversation\(\)/u.test(fn.body));

/*
 * 開場的 `hidden` 要跟 index.html 一樣，不能跟著假 DOM 的預設值走。詳細的
 * 理由在 fake-dom.mjs 的 `hiddenIn`——簡短版是：第一版寫死 false，於是這
 * 幾條測試在**真的壞掉的** app.js 上照樣綠。
 */
const HTML = read(join(UI, "index.html"));
const hiddenInHtml = (sel) => hiddenIn(HTML, sel);

if (!/<button\b[^>]*data-hits-close[^>]*>收起<\/button>/u.test(HTML)) {
  console.log("  ✗ A150 回答裡有一顆逐字是「收起」的按鈕");
  process.exit(1);
}

// 前提本身也要驗一次。哪天 index.html 把那個 `hidden` 拿掉，這幾條測試會
// 悄悄變成「驗一個不存在的問題」——寧可在這裡就吵。
if (!hiddenInHtml("[data-hits]")) {
  console.log("✗ index.html 上的 [data-hits] 已經不是 hidden 了——底下那條測試的前提沒了");
  process.exit(1);
}

const tick = (ms = 20) => new Promise((r) => setTimeout(r, ms));

/**
 * 等輸入框上面那一格真的重畫一次，回傳重畫之後的字。
 *
 * 底下三條斷言以前是 `await tick(4300)` 然後看一眼。那個寫法藏著一個 race：
 * 那個秒數是 `setInterval(paintThinking, 1000)` 重畫出來的，**要連跳四次**才跨
 * 過四秒線；而取樣用的 `tick()` 是一個**只跳一次**的 `setTimeout`。兩支排在同
 * 一個 event loop 上，機器一忙，要跳四次的那支落後得多——取樣就落在第三跳和第
 * 四跳之間，讀到還沒有數字的「思考中…」。
 *
 * 餘裕是算得出來的，而且很小：⑪ 那一組取樣在第二題送出後 1200+3600 = 4800 ms，
 * 第四跳排在 4000 ms，所以**最多**容得下 800 ms 的落後；上面那一組取樣在
 * 60+4300 = 4360 ms，只容得下 **360 ms**。兩個都還要再扣掉「`p.type()` 被呼叫」
 * 到「`startThinking()` 真的跑到」之間那一段，所以是上界不是實測值。
 *
 * 這不是新的：`v0.1.0-alpha.150` 之前就會偶發紅，而 2026-09-23 一天之內讓三輪
 * 派工白跑（同一族三條都中過）。
 *
 * 改成等「重畫真的發生」：`from` 是重畫之前那一格的字，等到它**變了**才回來。
 * 等不到就逾時，回傳的還是 `from`，呼叫端的斷言照樣紅。原本守的三件事一件都
 * 沒少——數字是哪一題的（呼叫端自己斷言值）、計時器還活著（死了就逾時）、
 * 以及確實跨過了一次重畫（這個函式等的就是那一次）。
 *
 * **`from` 要傳現況，不要傳寫死的「思考中…」。** 起算點沒跟著新題目走的那個
 * bug，會讓畫面在取樣之前就已經有數字；寫死的話這個迴圈會立刻回來、把那個錯
 * 的數字當成「重畫過了」交出去，那一刀就再也打不紅——修法會把偵測器一起刪掉。
 */
const awaitRepaint = async (p, from, budgetMs = 8000) => {
  const deadline = Date.now() + budgetMs;
  let now = p.thinking();
  while (now === from && Date.now() < deadline) {
    await tick(50);
    now = p.thinking();
  }
  return now;
};
const nativeSetInterval = globalThis.setInterval.bind(globalThis);
const nativeClearInterval = globalThis.clearInterval.bind(globalThis);

/**
 * `Answer` 的形狀，照 main.rs 那個 struct 抄的。
 *
 * 第一版寫的是 `{ hits: [], kind: "none", answers: [], blind: [], searched: [] }`
 * ——三個欄位的型別是錯的，而且 `"none"` 不是真的 kind（只有 `"keywords"` 和
 * `"recent"`）。`searched` 真的是 `Option<String>`，而 `[]` 在 JS 裡是 truthy，
 * 所以產品當場印出「我拿去比對的是「」——那是從你打的字黏出來的，不是一個
 * 詞」：一句只在她黏出非詞的時候才該出現的話，被一個空陣列叫了出來。沒有一條
 * 斷言問過它。
 */
function answer(over = {}) {
  return {
    presentation_id: null,
    kind: "keywords",
    searched: null,
    query_id: null,
    answers: [],
    hits: [],
    // 她自己稍早想過、而這一題用得到的那幾段。出處鍵回查得到 `card:` 靠它。
    readings: [],
    truncated: false,
    answers_truncated: false,
    blind: null,
    time_range: null,
    chapters: null,
    followup: null,
    closure_notice: null,
    overview: null,
    synthesis: null,
    brain: { state: "not_configured", provider: null },
    ...over,
  };
}

/**
 * 一筆命中，照 main.rs 的 `Hit` 抄的。
 *
 * 到這一版為止沒有一個 case 送過真的命中（全是 `hits: []`），所以
 * `renderSnippet` 一次都沒跑過——而它第一行就在用 `document.createTextNode`，
 * 那是假 DOM 一直沒有的東西。少了它那個 TypeError 被 `ask()` 的 catch 接住，
 * 「答成了」於是走進「答不成」那條路。見 fake-dom.mjs 的 `fakeDocument`。
 */
function hit(over = {}) {
  return {
    chunk_id: 31,
    ts: 1_755_000_000_000,
    text: "客服專線 0800-080-123",
    snippet: "客服[專線] 0800-080-123",
    app: "chrome.exe",
    title: "帳單查詢",
    url: "https://example.com/bill",
    frame_id: null,
    ...over,
  };
}

function fact(over = {}) {
  return {
    fact_id: 9,
    value: "+886800080123",
    raw: "客服專線 0800-080-123",
    sightings: 2,
    ts: 1_755_000_000_000,
    chunk_id: 31,
    frame_id: 42,
    app: "chrome.exe",
    title: "帳單查詢",
    url: "https://source.example.invalid/private",
    ...over,
  };
}

function chapter(over = {}) {
  return {
    start_ts: 1_755_000_000_000,
    end_ts: 1_755_000_360_000,
    core_start_ts: 1_755_000_030_000,
    core_end_ts: 1_755_000_330_000,
    app: "Notion.exe",
    title: "Azure 主句章節",
    host: "source.example.invalid",
    cut_kinds: [],
    confidence: 0.8,
    edited: null,
    edit_id: null,
    segment_count: 2,
    core_ms: 300_000,
    ...over,
  };
}

function overviewCard(over = {}) {
  return {
    segment_started_at: 1_755_000_000_000,
    activity: "修好安裝更新",
    author: "interpreter",
    model_confidence: 0.31337,
    evidence: [{ frame_id: 4242, label: "SOURCE_LABEL_MUST_STAY_LOCAL" }],
    ...over,
  };
}

/** `Blind` 的形狀，同上。後端只在一筆都沒找到的時候送。 */
function blind(over = {}) {
  return {
    chunks: 0,
    ocr_is_dead: false,
    frames: 0,
    ever_recorded: false,
    ever_stored: false,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
    master_stopped_episodes: 0,
    master_stopped_ms: 0,
    master_stopped_open: false,
    master_stopped_truncated: 0,
    master_stop_state: "clear",
    scan_horizon_days: null,
    recording_now: false,
    booting_now: false,
    ...over,
  };
}

/** #42 那題的 IPC 形狀；兩個選項的字刻意像後端真的回傳，不在測試裡縮成好／不要。 */
function urlPolicy(over = {}) {
  return {
    question: "我一個人在跑的時候，可不可以自己按網址？",
    answered: null,
    options: [
      { key: "only-on-my-press", line: "等我在；網址要我當場按。" },
      { key: "when-you-can-name-the-origin", line: "可以，但你要說得出它從哪來。" },
    ],
    before_you_answer: "你還沒回答以前，我一個人跑時不會開任何網址。",
    path: "C:\\Users\\ted\\AppData\\Roaming\\AI-Sister\\config.toml",
    ...over,
  };
}

/** gatekeeper_check 的 IPC 形狀。`display: null` 是量過的沒有，不是 command 沒跑。 */
function gatekeeper(display = null, actionLog = ["還沒有任何動作紀錄。她從來沒有把一個動作端到你面前過。"]) {
  return { display, developer: null, action_log: actionLog, presentation_id: null };
}

function gateCard(over = {}) {
  return {
    utterance_id: 91,
    form: "card",
    text: "五點了，要不要看那份報告？",
    evidence: [],
    suggestion: null,
    ...over,
  };
}

/** recorder_supervisor_state 的 IPC/event 形狀。 */
function supervisor(phase = "stopped", message = null, failures = 0) {
  return { phase, failures, message };
}

/** `consent_read` 的 IPC 形狀；預設表示四張都已回答，避免每個既有案例被導覽接管。 */
function consentView(
  reviewed = [true, true, true, true],
  effective = [true, true, true, false],
) {
  const keys = ["local-recording", "cloud-reading", "frame-storage", "azure-tts"];
  return {
    path: "C:\\Users\\ted\\AppData\\Roaming\\AI-Sister\\consent.toml",
    current: true,
    allows_recording: effective[0],
    allows_frames: effective[2],
    store_images: true,
    capture_enabled: true,
    reset_by_version: false,
    sheets: keys.map((key, index) => ({
      key,
      wording: `第 ${index + 1} 張完整而且可讀的同意條文。`,
      without: `沒有第 ${index + 1} 張的直接後果`,
      granted_at: effective[index] ? 1_755_000_000_000 + index : null,
      effective: effective[index],
      reviewed: reviewed[index],
    })),
  };
}

function consentVoiceManifest() {
  const personas = [
    "chatgpt",
    "claude",
    "gemini",
    "grok",
    "deepseek",
    "qwen",
    "mistral",
    "venice",
    "sakana",
    "perplexity",
    "glm",
    "kimi",
    "hunyuan",
    "minimax",
    "nemotron",
    "cohere",
    "mimo",
  ];
  const view = consentView([false, false, false, false], [false, false, false, false]);
  const clips = [];
  for (const [personaIndex, persona] of personas.entries()) {
    for (const [sheetIndex, sheet] of view.sheets.entries()) {
      clips.push({
        persona,
        group: personaIndex < 4 ? "sister" : "bestie",
        sheet: sheet.key,
        text: sheet.wording,
        file: `${persona}/${sheet.key}.ogg`,
        bytes: 100 + sheetIndex,
        sha256: `${(personaIndex + 1).toString(16).padStart(2, "0")}${(sheetIndex + 1)
          .toString(16)
          .padStart(2, "0")}${"a".repeat(60)}`,
        durationMs: 1000 + sheetIndex,
        integratedLufs: -23.0,
        truePeakDbtp: -1.2,
      });
    }
  }
  return {
    schema: "ai-sister/persona-consent-voices/v1",
    // 出貨的 manifest 帶著整平的契約，`consentVoiceLibrary()` 會驗它。
    // 見 apps/desktop/ui/persona-consent-voices/v1/manifest.json。
    postProcessing: {
      loudness: {
        standard: "EBU R128",
        targetLufs: -23.0,
        ceilingDbtp: -1.0,
        toleranceLu: 0.6,
        method:
          "one constant gain per clip; no compression, limiting, or other dynamics processing",
      },
    },
    locale: "zh-TW",
    roster: "four-sisters-plus-thirteen-besties",
    engine: {
      name: "MediaTek-Research/BreezyVoice-300M",
      modelSnapshot: "e33b502e0ac21c16b0ee0d00df66ac3fa737393d",
      license: "Apache-2.0",
    },
    rightsReview: "approved-owner-grant",
    ownerGrant: {
      grantedOn: "2026-09-11",
      grantor: "Ted Huang",
      license: "excluded-from-Apache-2.0",
      scope:
        "Unmodified inclusion of the 68 generated consent-reading clips hash-listed by this manifest in the AI-Sister source tree and official builds",
    },
    notice: "NOTICE.md",
    sheets: view.sheets.map(({ key }) => key),
    clips,
    totals: {
      personas: 17,
      sheetsPerPersona: 4,
      clips: 68,
      oggBytes: clips.reduce((sum, clip) => sum + clip.bytes, 0),
      durationMs: clips.reduce((sum, clip) => sum + clip.durationMs, 0),
    },
  };
}

/**
 * 開一次字母人。`invoke` 收一張 `{ 指令: 回傳值或會丟出來的 Error }` 表；
 * 沒列到的指令回 `null`；每一扇新 desktop 都必定提供的 supervisor view 與
 * master-stop observation 各預設成乾淨的 `stopped`／`clear`。函式值會被呼叫
 * （要延遲、要丟例外的用這個）。
 *
 * `search` 是網址上那串 `?…`。**那幾條 demo 路徑不是裝飾。** 這台機器開不起
 * Tauri，所以 `?asleep=nobeat` 那幾條是這幾格畫面唯一長得出來的地方——他真的
 * 是照著那個網址用眼睛看版面的。以前這裡寫死 `""`，等於整條 demo 路徑沒有
 * 任何測試走過。
 */
async function open(
  table = {},
  {
    search = "",
    beforeListenerRegistered = null,
    browserOnly = false,
    autoFirstPersona = true,
    alterCatalog = null,
    consentVoices = null,
    systemVoices = null,
    holdPlay = false,
    holdFirstPlay = false,
    personaVoices = null,
  } = {},
) {
  // `domOf` 只生得出 index.html 上真的有的東西——見 fake-dom.mjs 開頭那段。
  const node = domOf(HTML);
  const listeners = new Map();
  const windowListeners = new Map();
  const calls = [];
  const diagnoseNotes = [];
  const invokes = [];
  const intervals = [];
  let audioPlays = 0;
  let audioPauses = 0;
  let localSpeaks = 0;
  const localSpeechTexts = [];
  const playbackTrace = [];

  // fake-dom 的 selector 子集刻意很小；這一頁新增的 Azure allowlist 是 attribute
  // selector。只在真正的 [data-hits] 子樹補上正文與出處屬性查詢，避免測試自己用 class
  // denylist 重抄產品邏輯。
  const hitsNode = node("[data-hits]");
  const basicQuerySelectorAll = hitsNode.querySelectorAll.bind(hitsNode);
  hitsNode.querySelectorAll = (selector) => {
    const evidence = /^\[data-evidence-ref="([^"]+)"\]$/u.exec(selector);
    if (evidence) return basicQuerySelectorAll("li").filter(n => n.dataset.evidenceRef === evidence[1]);
    if (selector !== "[data-azure-answer-body]") return basicQuerySelectorAll(selector);
    const selected = [];
    const walk = (parent) => {
      for (const child of parent.children ?? []) {
        if (Object.hasOwn(child.dataset ?? {}, "azureAnswerBody")) selected.push(child);
        walk(child);
      }
    };
    walk(hitsNode);
    return selected;
  };

  const audio = node("[data-persona-audio]");
  audio.pause = () => {
    audioPauses += 1;
    playbackTrace.push("pause");
  };
  const removeAudioAttribute = audio.removeAttribute.bind(audio);
  audio.removeAttribute = (name) => {
    if (name === "src") playbackTrace.push("remove-src");
    removeAudioAttribute(name);
  };
  let playHold = null;
  audio.play = async () => {
    audioPlays += 1;
    playbackTrace.push("play");
    if (!holdPlay && !(holdFirstPlay && audioPlays === 1)) return;
    await new Promise((resolve, reject) => {
      playHold = { resolve, reject };
    });
  };

  globalThis.document = fakeDocument(node, {
    // **要是 visible。** 開場那一段對 `recording` 寫死的是 `"recording"`，
    // 只有 `updatePollGate()` 看到視窗是開著的才會去問一次磁碟；hidden 的話
    // 這一頁會停在「她在錄」，而灰掉那條路上的話（`wakeFailed`）就永遠不會
    // 被畫出來——測試會綠，但綠的理由是它根本沒走到那裡。
    visibilityState: "visible",
  });
  globalThis.location = { search };
  globalThis.addEventListener = (name, cb) => {
    (windowListeners.get(name) ?? windowListeners.set(name, []).get(name)).push(cb);
  };
  globalThis.removeEventListener = () => {};
  globalThis.matchMedia = () => ({ matches: false, addEventListener() {} });
  // 她那扇窗是固定的 340×560（`resizable: false`，見 tauri.conf.json）。
  // 假瀏覽器要報得出視窗大小，app.js 才算得出「拖曳中整扇窗都算實心」那一塊。
  globalThis.innerWidth = 340;
  globalThis.innerHeight = 560;
  // app.js 靠它盯住畫面變化，好重算「哪裡是實心的」再送回 Rust（`pet_solid_set`）。
  // 這個假瀏覽器不模擬 DOM 變動，所以 observe 不必真的做事；但它得**存在**——
  // 少了它，app.js 一載入就 ReferenceError，整支閘門連第一條斷言都跑不到。
  globalThis.MutationObserver = class {
    observe() {}
    disconnect() {}
    takeRecords() {
      return [];
    }
  };
  // 真的 WebView2 一定有這個建構子。以前這個假瀏覽器沒有它，於是
  // `speakWithLocalSystemVoice` 第一行就回 false——好幾條「不 fallback 到本機」
  // 的斷言其實是靠「這個全域不存在」過的，而那件事在他的機器上是假的。補上之後
  // 那幾條改成靠真正的理由過關：底下 `getVoices()` 預設一支都不回。
  globalThis.SpeechSynthesisUtterance = class {
    constructor(text) {
      this.text = text;
      this.voice = null;
      this.lang = "";
      this.rate = 1;
      this.pitch = 1;
    }
  };
  globalThis.speechSynthesis = {
    // 預設一支都沒有，所以 `speakWithLocalSystemVoice` 直接回 false——這個檔案
    // 大部分的情境要的正是那個。要**驅動**本機朗讀那顆鍵的人得自己送一支
    // `localService` 的繁中 voice 進來（見第 88 節）。
    getVoices: () => systemVoices ?? [],
    addEventListener() {},
    cancel() {},
    speak(utterance) {
      localSpeechTexts.push(utterance.text);
      localSpeaks += 1;
    },
  };
  globalThis.setInterval = (fn, ms, ...args) => {
    const id = nativeSetInterval(fn, ms, ...args);
    intervals.push({ fn, ms, id });
    return id;
  };
  globalThis.clearInterval = (id) => nativeClearInterval(id);
  globalThis.__AI_SISTER_PERSONA_VOICES__ = personaVoices;
  if (consentVoices === null) delete globalThis.__AI_SISTER_CONSENT_VOICES__;
  else globalThis.__AI_SISTER_CONSENT_VOICES__ = consentVoices;

  const tauri = {
    core: {
      invoke: async (cmd, arg) => {
        /* 診斷觀測不是產品指令。
         *
         * `diagnose_note` 在 Rust 那邊就只是 `Mutex<Notebook>` 上的一次 push：
         * 不回傳東西、不碰任何狀態、沒有人 await 它。而底下好幾條契約的形狀是
         * 「做完這個動作，`calls` 該是空的」——把一條純觀測混進同一條清單，會讓
         * 那些契約在產品行為一個字都沒變的情況下變紅。
         *
         * 記到另一條清單上，不是丟掉。上一版這裡寫著「`diagnoseNotes` 自己有
         * 斷言」——**那句話在這個檔案裡是假的**：這支從頭到尾沒有一行讀它，所
         * 以「濾掉」在這一輪真的就是「沒人看」。守它的三道各在別處：
         *
         *   1. `check-persona.mjs` ⑨——執行期真的送出去的每一則，形狀和內容。
         *   2. `scripts/check-diagnose-carries-no-text.py`——`Note` 的型別，
         *      不看夾具，所以夾具產不出來的種類也蓋得到。
         *   3. 底下那一條——這一支自己的接線還活著。
         *
         * 第三條要留著：前兩道證明得了「送出去的東西乾淨」，證明不了「這一條
         * 路在這個夾具上還通」，而那正是這幾行過濾隨手就會弄斷的東西。 */
        if (cmd === "diagnose_note") {
          diagnoseNotes.push(arg?.note);
          everyDiagnoseNote.push(arg?.note);
          return;
        }
        calls.push(cmd);
        invokes.push({ cmd, arg });
        if (cmd === "master_stop_presentation_end") {
          playbackTrace.push(`end:${arg?.presentationId ?? "missing"}`);
        }
        const v = Object.hasOwn(table, cmd)
          ? table[cmd]
          : cmd === "recorder_supervisor_state"
            ? supervisor()
            : cmd === "master_stop_state"
              ? "clear"
              : cmd === "persona_select"
                ? { ...(table.persona_read ?? { id: "chatgpt" }), id: arg.id }
              : cmd === "persona_consent_speak_set"
                ? { id: "chatgpt", consent_read_aloud: arg.enabled }
              : cmd === "consent_read"
                ? consentView()
              : cmd === "master_stop_presentation_begin"
                ? true
              : null;
        if (typeof v === "function") return v(arg);
        if (v instanceof Error) throw v;
        return v;
      },
    },
    event: {
      listen: async (name, cb) => {
        if (beforeListenerRegistered !== null) await beforeListenerRegistered(name);
        listeners.set(name, cb);
        return () => {};
      },
    },
  };
  if (browserOnly) delete globalThis.__TAURI__;
  else globalThis.__TAURI__ = tauri;

  await loader(read(join(UI, "personas/catalog.js")))();
  if (alterCatalog) globalThis.__AI_SISTER_PERSONA_CATALOG__ = alterCatalog(globalThis.__AI_SISTER_PERSONA_CATALOG__);
  const nonsense = watchNonsense();
  await boot();
  await tick();
  if (autoFirstPersona && !node("[data-persona-first-run]").hidden) {
    for (const fn of node("[data-persona-first-save]").handlers.click ?? []) fn({ isTrusted: true });
    await tick();
  }
  return {
    node,
    renderErrors: globalThis.__renderErrorsForTest,
    calls,
    diagnoseNotes,
    invokes,
    nonsense,
    line: () => node("[data-state-line]").textContent,
    // 輸入框上面那一格。`null` = 它不在畫面上。**藏起來要回 `null` 不是
    // 空字串**：`hidden` 沒被拿掉而字還留著，讀 `textContent` 會拿到上一題
    // 的秒數，而那一格明明看不見——那正是這幾條要抓的錯。
    thinking: () => {
      const el = node("[data-ask-thinking]");
      return el.hidden ? null : el.textContent;
    },
    hits: () => node("[data-hits]"),
    hitTexts: () => node("[data-hits]").children.map((c) => c.textContent),
    consentGuide: () => node("[data-consent-guide]"),
    consentProgress: () => node("[data-consent-progress]").textContent,
    consentWording: () => node("[data-consent-wording]").textContent,
    consentResult: () => node("[data-consent-result]").textContent,
    consentListen: () => node("[data-consent-speak]"),
    input: () => node("[data-ask-input]"),
    azureButton: () => node("[data-hits]").querySelector(".answer-cloud"),
    // Azure 那顆的 class 是 `answer-read answer-cloud`，所以 `.answer-read`
    // 兩顆都會中；這個假 DOM 的 selector 子集沒有 `:not()`，要自己濾。
    localReadButton: () =>
      node("[data-hits]")
        .querySelectorAll(".answer-read")
        .find((el) => !String(el.className).split(/\s+/u).includes("answer-cloud")) ?? null,
    audioPlays: () => audioPlays,
    audioPauses: () => audioPauses,
    playbackTrace: () => [...playbackTrace],
    isSpeaking: () => node("[data-avatar]").classList.contains("speaking"),
    // 圖案和字是兩條路：`paint()` 先算 `shown` 餵給 `avatar.dataset.state`，
    // 再另外算 `line`。把兩個三元的順序改成不一樣，字會講全停而圖案是暫停的
    // 斜槓——而 `styles.css` 專門替 `[data-state="stopped"]` 畫了方點加叉號，
    // 就是為了讓這兩件事長得不一樣。實測過：只改 `shown` 那個三元的順序，
    // 這支閘門原本 74 個情境全綠。
    avatarState: () => node("[data-avatar]").dataset.state,
    finishAudio() {
      audio.onended?.();
    },
    failAudio() {
      audio.onerror?.();
    },
    audioSrc: () => audio.src,
    holdPlayPending: () => playHold !== null,
    async releasePlay(error) {
      const hold = playHold;
      playHold = null;
      if (!hold) return;
      if (error) hold.reject(error);
      else hold.resolve();
      await tick();
    },
    localSpeaks: () => localSpeaks,
    localSpeechTexts: () => [...localSpeechTexts],
    urlPolicy: () => node("[data-url-policy]"),
    urlPolicyQuestion: () => node("[data-url-policy-question]").textContent,
    urlPolicyActions: () => node("[data-url-policy-actions]"),
    urlPolicyResult: () => node("[data-url-policy-result]"),
    utterance: () => node("[data-utterance]"),
    body: () => globalThis.document.body,
    handsLog: () => node("[data-hands-log]"),
    /** 從這個視窗**以外**發生的事：系統匣的按鈕、熱鍵、她自己停掉。 */
    async fromOutside(name, payload) {
      const cb = listeners.get(name);
      if (!cb) throw new Error(`沒有人在聽 ${name}——這條測試的前提沒了`);
      cb({ payload });
      await tick();
    },
    async click(sel) {
      for (const fn of node(sel).handlers.click ?? []) fn();
      await tick();
    },
    async clickElement(element, { trusted = true } = {}) {
      if (!element || element.disabled || element.hidden) return false;
      for (const fn of element.handlers.click ?? []) fn({ isTrusted: trusted });
      await tick();
      return true;
    },
    async key(key) {
      for (const fn of windowListeners.get("keydown") ?? []) fn({ key });
      await tick();
    },
    async type(q) {
      node("[data-ask-input]").value = q;
      for (const fn of node("[data-ask-send]").handlers.click ?? []) fn();
      await tick();
    },
    async chooseUrlPolicy(index) {
      const button = node("[data-url-policy-actions]").children[index];
      if (!button) throw new Error(`URL 題沒有第 ${index + 1} 顆按鈕`);
      for (const fn of button.handlers.click ?? []) fn();
      await tick();
    },
    /**
     * 逼一次重畫，走的是產品自己那條路：系統匣按了暫停 → `pause-changed`
     * → `setPaused` → `paint()`。輪詢那條（`setRecording` + `setPaused`）
     * 打在同一個 `paint()` 上，差別只有要不要等五秒。
     */
    async repaint() {
      const cb = listeners.get("pause-changed");
      if (!cb) throw new Error("沒有人在聽 pause-changed——這條測試的前提沒了");
      cb({ payload: false });
      await tick();
    },
    async pollNow() {
      const interval = intervals.find(({ ms }) => ms === 5000);
      if (!interval) throw new Error("找不到五秒 recording poll——這條測試的前提沒了");
      interval.fn();
      await tick();
    },
  };
}

let failed = 0;
let passed = 0;
process.on("exit", () => console.log(`${passed} passed; ${failed} failed`));
/** 這一輪所有夾具送出去的診斷觀測，不分是哪一個 view。 */
const everyDiagnoseNote = [];

function check(name, ok, detail) {
  if (ok) passed++;
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

// 執行產品原碼的探針；假動畫在下一個 tick 才完成，CSS 在此前仍回 visible。
const observationSource = APP_FUNCTIONS.filter(fn => fn.name === "observation")
  .map(fn => APP_SOURCE.slice(fn.start, fn.end));
const chromeSources = [...APP_SOURCE.matchAll(
  /(?:let chromeBarObservationGeneration = 0;\s*)?const noteTheChromeBar = observation\([\s\S]*?\n\}\);/gu,
)].map(match => match[0]);
check("R8 observation 至少抽到一項且唯一", observationSource.length === 1);
check("R8 chrome 探針至少抽到一項且唯一", chromeSources.length === 1);
if (observationSource.length === 1 && chromeSources.length === 1) {
  const probe = (animations) => {
    const notes = [];
    let open = false;
    let hidden = false;
    const context = {
      invoke: () => {},
      document: { body: { classList: { contains: () => open } } },
      chromeBar: { getAnimations: animations },
      getComputedStyle: () => ({ visibility: hidden ? "hidden" : "visible" }),
      noteForDiagnosis: note => notes.push(note),
    };
    const sample = runInNewContext(
      observationSource[0] + "\n" + chromeSources[0] + "\nnoteTheChromeBar;", context,
    );
    return { notes, sample, setOpen: value => { open = value; },
      setHidden: value => { hidden = value; } };
  };
  let finish;
  const animation = { finished: new Promise(resolve => { finish = resolve; }) };
  const delayed = probe(() => [animation]);
  delayed.sample();
  check("R8 過場完成前不送觀測", delayed.notes.length === 0);
  await tick();
  delayed.setHidden(true);
  finish();
  await tick();
  check("R8 等過場後送出的 dragbar_hidden 是 true",
    delayed.notes.length === 1 && delayed.notes[0].dragbar_hidden === true, delayed.notes);

  let cancel;
  const interrupted = { finished: new Promise((_, reject) => { cancel = reject; }) };
  // 等待邏輯被突變拿掉時，夾具自己的取消也不能殺掉整支閘門。
  interrupted.finished.catch(() => {});
  const rapid = probe(() => [interrupted]);
  rapid.sample();
  rapid.setOpen(true);
  rapid.sample();
  cancel(new Error("transition cancelled"));
  await tick();
  check("R8 連按丟掉舊取樣且中斷過場仍送最新一則",
    rapid.notes.length === 1 && rapid.notes[0].open === true, rapid.notes);

  const legacy = probe(undefined);
  legacy.setHidden(true);
  legacy.sample();
  check("R8 沒有 getAnimations 仍當場送一則",
    legacy.notes.length === 1 && legacy.notes[0].dragbar_hidden === true, legacy.notes);

  // 真子行程用 strict 模式：拿掉 thenable 接手時，父測試仍能印出具名紅燈。
  const child = spawnSync("timeout", ["300", process.execPath, "--unhandled-rejections=strict", "-e",
    "const invoke = () => {};\n" + observationSource[0] + `
      observation(() => Promise.reject(new Error("measure rejected")))();
      observation(() => ({ then(resolve, reject) { reject(new Error("thenable rejected")); } }))();
      setTimeout(() => console.log("R8 main continued"), 20);
    `], { encoding: "utf8" });
  check("R8 observation 接住拒絕的 thenable，子行程主線跑完",
    child.status === 0 && child.stdout.includes("R8 main continued"),
    { status: child.status, stderr: child.stderr });
}

console.log("A151 R1. 先開口，再想；先開口不是答完了");
// 這兩條是 Rust 接線的原碼檢查，不是 native 執行或 SQLite 寫入次數的量測。
// renderer 夾具只會回 mock；單靠下面畫面測試抓不到 main.rs 接錯 stage/brain。
{
  const main = read(MAIN);
  const localStart = main.indexOf("fn ask_local(");
  const localEnd = main.indexOf("fn nothing_was_asked(");
  check("A151 R1 抽取前提：ask_local 與 nothing_was_asked 錨點存在且有序", localStart >= 0 && localEnd > localStart);
  if (localStart < 0 || localEnd <= localStart) throw new Error("A151 R1 抽取錨點不存在或順序錯誤");
  const local = main.slice(localStart, localEnd);
  check(
    "A151 native 接線：先開口不得接 Brain，否則題庫記了兩次／結案講了兩遍",
    /answer_from_memory\(&shell, &question, AskStage::Local, brain\)/u.test(local) &&
      !local.includes("AskStage::Brain"),
    local,
  );
  check(
    "A151 native 接線：先開口的 thinking 必須來自共用 brain_plan 與設定的 CLI",
    /let brain = match brain_plan\(&shell, &question\)/u.test(local) &&
      /BrainPlan::Skip\(b\) => b/u.test(local) &&
      /BrainPlan::Go \{ cli, \.\. \} => BrainAnswer::new\("thinking", Some\(cli.label\)\)/u.test(local),
    local,
  );
}
// Rust 原碼位置檢查：先遮掉字串／註解，括號才不會被文案干擾。
// 保留長度，抽取位置仍然對應原始檔；test module 不算產品呼叫。
{
  const main = read(MAIN);
  const code = main.replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*|r(#+)"[\s\S]*?"\1|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])'/gu,
    token => token.replace(/[^\n]/gu, " "));
  function blockEnd(start, name) {
    const open = code.indexOf("{", start);
    check(`A151 R2 抽取：${name} 開括號存在`, open >= start && start >= 0);
    if (start < 0 || open < start) throw new Error(`A151 R2 抽取失敗：${name}`);
    let depth = 0;
    for (let i = open; i < code.length; i++) {
      if (code[i] === "{") depth++;
      if (code[i] === "}" && --depth === 0) return i + 1;
    }
    check(`A151 R2 抽取：${name} 結尾存在`, false);
    throw new Error(`A151 R2 抽取沒有結尾：${name}`);
  }
  function extract(anchor) {
    const start = code.indexOf(anchor);
    check(`A151 R2 抽取錨點：${anchor}`, start >= 0);
    if (start < 0) throw new Error(`A151 R2 抽取錨點不存在：${anchor}`);
    const end = blockEnd(start, anchor);
    return { start, end, body: code.slice(start, end) };
  }
  const ask = extract("fn ask(");
  const local = extract("fn ask_local(");
  const memory = extract("fn answer_from_memory(");
  let production = code;
  const testRanges = [...code.matchAll(/#\[cfg\(test\)\]\s*(?:mod|fn)\s+\w+/gu)]
    .map(m => [m.index, blockEnd(m.index, m[0])]);
  for (const [start, end] of testRanges.reverse()) {
    production = production.slice(0, start) + " ".repeat(end - start) + production.slice(end);
  }
  const count = (body, word) => [...body.matchAll(new RegExp(`\\b${word}\\b`, "gu"))].length;
  check("A151 R2 抽取正例：ask 找得到 close_from_message", count(ask.body, "close_from_message") >= 1);
  for (const word of ["close_from_message", "record_followup", "log_query"]) {
    check(`A151 R2 寫入位置：${word} 在非測試區只出現於 ask 一次`,
      count(production, word) === 1 && count(ask.body, word) === 1,
      { production: count(production, word), ask: count(ask.body, word) });
    check(`A151 R2 無寫入：ask_local 與 answer_from_memory 沒有 ${word}`,
      count(local.body, word) === 0 && count(memory.body, word) === 0,
      { local: count(local.body, word), memory: count(memory.body, word) });
  }
}
{
  let finish;
  const pending = new Promise(resolve => { finish = resolve; });
  const p = await open({
    ask_local: answer({ presentation_id: "15101", hits: [hit({ snippet: "A151_LOCAL_MEMORY" })], brain: { state: "thinking", provider: "Grok CLI" } }),
    ask: () => pending,
  });
  await p.type("本機那幾筆");
  check("A151 1 前提：ask_local 已呼叫而 ask 正在 pending", p.calls.includes("ask_local") && p.calls.includes("ask"), p.calls);
  check("A151 1：正式回答 pending 時先看得到本機記憶", p.hitTexts().join("\n").includes("A151_LOCAL_MEMORY"), p.hitTexts());
  check("A151 1：本機記憶旁如實說選定 CLI 還在想", p.hitTexts().some(t => t.includes("Grok CLI 還在想怎麼把它們講成一句話")), p.hitTexts());
  check("A151 2：先開口不清輸入也不停思考中", p.input().value === "本機那幾筆" && /^思考中/u.test(p.thinking() ?? ""), { input: p.input().value, thinking: p.thinking() });
  check("A151 2：先開口不記答完觀測", !p.diagnoseNotes.some(n => n?.kind === "answered"), p.diagnoseNotes);
  finish(answer({ hits: [hit({ snippet: "A151_FINAL_MEMORY" })] }));
  await tick(40);
  const text = p.hitTexts().join("\n");
  check("A151 3：正式回答整份取代本機那份", text.includes("A151_FINAL_MEMORY") && !text.includes("A151_LOCAL_MEMORY"), text);
  check("A151 3：正式回答後才清輸入並停計時", p.input().value === "" && p.thinking() === null, { input: p.input().value, thinking: p.thinking() });
  const landed = p.diagnoseNotes.filter(n => n?.kind === "answered");
  check("A151 R2 接線：app.js 先畫再把 spoke_ms 數字送到 answered note",
    landed.length === 1 && Number.isInteger(landed[0].spoke_ms) && landed[0].spoke_ms >= 0 && landed[0].spoke_ms <= landed[0].took_ms, landed);

}
{
  const p = await open({ ask_local: new Error("R2_NO_EARLY"), ask: answer() });
  await p.type("本機那段沒畫出來");
  const notes = p.diagnoseNotes.filter(n => n?.kind === "answered");
  check("A151 R2 接線：先開口失敗送 null，不冒充零毫秒",
    notes.length === 1 && notes[0].spoke_ms === null, notes);
}
for (const [name, table] of [
  ["A151 4：ask_local throw 仍由 ask 完成且不顯示錯誤", { ask_local: new Error("A151_EARLY_THROW") }],
  // 不 mock ask_local：這條明確守住既有幾百個夾具為什麼還是綠的。
  ["A151 5：未 mock 的 null 保持既有幾百個夾具行為", {}],
]) {
  const p = await open({ ...table, ask: answer({ hits: [hit({ snippet: "A151_FALLBACK_FINAL" })] }) });
  await p.type("先開口沒開成");
  const text = p.hitTexts().join("\n");
  check(name, p.calls.includes("ask") && text.includes("A151_FALLBACK_FINAL") && !text.includes("沒答成") && !p.line().includes("A151_EARLY_THROW") && !p.line().includes("沒有回東西"), { text, line: p.line(), calls: p.calls });
}
{
  const p = await open({
    ask_local: answer({ hits: [hit({ snippet: "A151_KEEP_LOCAL" })] }),
    ask: new Error("A151 正式回答沒有完成"),
  });
  await p.type("保留這一題的本機記憶");
  const text = p.hitTexts().join("\n");
  check("A151 6：正式回答 throw 保留本機那份並說明，不冒充這一題我沒答成", text.includes("A151_KEEP_LOCAL") && !text.includes("這一題我沒答成") && p.line().includes("A151 正式回答沒有完成"), { text, line: p.line() });
  check("A151 6：有本機答案的失敗另記 brain_error", p.diagnoseNotes.some(n => n.why === "brain_error"), p.diagnoseNotes);
}
{
  const state = consentView();
  const p = await open({
    consent_read: () => structuredClone(state),
    ask_local: () => {
      state.sheets[1].reviewed = false;
      state.sheets[1].effective = false;
      state.sheets[1].granted_at = null;
      return answer({ presentation_id: "15107", hits: [hit({ snippet: "A151_CONSENT_HIDDEN" })], brain: { state: "consent_required", provider: "Grok CLI" } });
    },
    ask: answer({ brain: { state: "consent_required", provider: "Grok CLI" } }),
  });
  await p.type("先問條文");
  check("A151 7：consent_required 先開口不畫，只出同意書", !p.hitTexts().join("\n").includes("A151_CONSENT_HIDDEN") && p.consentProgress() === "同意書 2 / 4", { hits: p.hitTexts(), consent: p.consentProgress() });
  check("A151 7：不畫的先開口仍歸還 native lease", p.invokes.some(i => i.cmd === "master_stop_presentation_end" && i.arg.presentationId === "15107"), p.invokes);
  // 上面那題終止於同意書，沒有 answered；另讓同樣被擋的先開口走完正式回答。
  const finished = await open({
    ask_local: answer({ presentation_id: "15107", brain: { state: "consent_required", provider: "Grok CLI" } }),
    ask: answer(),
  });
  await finished.type("同意要求擋下先開口之後完成回答");
  const notes = finished.diagnoseNotes.filter(n => n?.kind === "answered");
  check("A151 7：consent_required 沒畫出來就不准報先開口毫秒數（報告會說謊）",
    notes.length === 1 && notes[0].spoke_ms === null, notes);
}
{
  let finish;
  const pending = new Promise(resolve => { finish = resolve; });
  const p = await open({
    ask_local: answer({ presentation_id: "15108", hits: [hit({ snippet: "A151_STOP_HIDDEN" })] }),
    ask: () => pending,
    master_stop_presentation_begin: false,
  });
  await p.type("全停先擋住");
  check("A151 8：native presentation begin 拒絕時先開口不畫", !p.hitTexts().join("\n").includes("A151_STOP_HIDDEN"), p.hitTexts());
  finish(answer());
  await tick(40);
  const notes = p.diagnoseNotes.filter(n => n?.kind === "answered");
  check("A151 8：native 拒絕、沒畫出來就不准報先開口毫秒數（報告會說謊）",
    notes.length === 1 && notes[0].spoke_ms === null, notes);
}

const AZURE_READY = {
  generation: 7,
  config_readable: true,
  enabled: true,
  region: "eastasia",
  voice: "zh-TW-HsiaoChenNeural",
  endpoint: "https://eastasia.tts.speech.microsoft.com/cognitiveservices/v1",
  credential: "present",
  consented: true,
  consent_at: 1_757_299_200_000,
  ready: true,
  config_error: null,
};

console.log("A151 R3. 先開口空手時不先斷言沒有");
{
  const thinking = { state: "thinking", provider: "Grok CLI" };
  const finalBrain = { state: "not_configured", provider: null };
  const fullBlind = blind({
    ever_recorded: true, ever_stored: true, chunks: 12, frames: 12,
    excluded: [["excluded 規則", 3]], paused_episodes: 2, paused_ms: 1200,
    scan_horizon_days: 30,
  });
  // 有掃描界線的終局原文是「我翻過的那幾段」；未限天數才是「我記得的東西」。
  const finalBlind = { ...fullBlind, scan_horizon_days: null };
  const line = "我好像不太知道你在說什麼耶，我再認真想一下。";
  const provisionalChecks = (p, name) => {
    const texts = p.hitTexts();
    const text = texts.join("\n");
    check(`${name}：有「我好像不太知道你在說什麼耶」`, text.includes("我好像不太知道你在說什麼耶"), text);
    check(`${name}：只有完整那一句，一字不變`, texts.length === 1 && texts[0] === line, texts);
    check(`${name}：沒有「我記得的東西裡沒有這件事」`, !text.includes("我記得的東西裡沒有這件事"), text);
    check(`${name}：沒有「我翻過的那幾段裡沒有這件事」`, !text.includes("我翻過的那幾段裡沒有這件事"), text);
    check(`${name}：沒有「還在想怎麼把它們講成一句話」`, !text.includes("還在想怎麼把它們講成一句話"), text);
    check(`${name}：.hits-why 元素數是 0`, p.hits().querySelectorAll(".hits-why").length === 0, texts);
    check(`${name}：brain-note 整塊不畫`, p.hits().querySelectorAll(".brain-note").length === 0, texts);
  };
  for (const [name, suppliedBlind] of [["R3 1", null], ["R3 2", fullBlind]]) {
    const p = await open({
      ask_local: answer({ hits: [], answers: [], brain: thinking, blind: suppliedBlind }),
      ask: () => new Promise(() => {}),
    });
    await p.type("本機查不到的問題");
    check(`${name} 前提：ask_local 已回而 ask pending`, p.calls.includes("ask_local") && p.calls.includes("ask"), p.calls);
    provisionalChecks(p, name);
  }
  const emptyOverview = await open({
    azure_tts_read: AZURE_READY,
    ask_local: answer({ kind: "memory_overview", overview: { kind: "empty" }, brain: thinking,
      followup: "R4_FOLLOWUP", closure_notice: "R4_CLOSURE" }),
    ask: () => new Promise(() => {}),
  });
  await emptyOverview.type("空的記憶總覽");
  provisionalChecks(emptyOverview, "R4 空總覽");
  check("R4 空總覽：不畫 followup、收尾或朗讀按鈕", emptyOverview.localReadButton() === null && emptyOverview.azureButton() === null && !emptyOverview.hitTexts().join("\n").includes("R4_"), emptyOverview.hitTexts());
  const readyOverview = await open({
    ask_local: answer({ kind: "memory_overview", overview: { kind: "ready", cards: [overviewCard()], truncated: false, evidence_unavailable: 0 }, brain: thinking, followup: "R4_READY_FOLLOWUP" }),
    ask: () => new Promise(() => {}),
  });
  await readyOverview.type("有內容的記憶總覽");
  check("R4 有內容總覽加 thinking：仍畫卡片、brain-note 與 followup", readyOverview.hits().querySelectorAll(".overview-card").length === 1 && readyOverview.hits().querySelectorAll(".brain-note").length === 1 && readyOverview.hitTexts().join("\n").includes("R4_READY_FOLLOWUP") && !readyOverview.hitTexts().join("\n").includes(line), readyOverview.hitTexts());
  const invalidEarly = await open({
    ask_local: answer({ brain: thinking, answers: [fact({ frame_id: 42 })], synthesis: {
      sentences: [{ text: "R4_INVALID_SYNTHESIS", sources: [{ ref: "fact:9", label: "畫面 #42", frame_id: 42 }] }],
    } }),
    ask: answer({ hits: [hit({ snippet: "R4_FINAL_AFTER_THROW" })] }),
  });
  await invalidEarly.type("故意讓先開口帶成句答案");
  check("R4 synthesis 不變式：ask_local 帶成句答案必須 throw 指定錯誤", invalidEarly.renderErrors.length === 1 && invalidEarly.renderErrors[0].message === "先開口那一趟不該帶成句答案", invalidEarly.renderErrors.map(e => e.message));
  check("R4 synthesis 不變式：throw 後正式回答仍完成且先開口計時為 null", invalidEarly.hitTexts().join("\n").includes("R4_FINAL_AFTER_THROW") && invalidEarly.diagnoseNotes.some(n => n.kind === "answered" && n.spoke_ms === null), invalidEarly.diagnoseNotes);
  // 真正的先開口也會帶 searched／空時間範圍；不能因此多出另一句「沒有」。
  const withContext = await open({
    ask_local: answer({
      kind: "range", brain: thinking, blind: fullBlind, searched: "個板",
      time_range: { from: 1000, to: 2000, said: "昨天" }, chapters: [],
    }),
    ask: () => new Promise(() => {}),
  });
  await withContext.type("昨天那個板");
  provisionalChecks(withContext, "R3 2 時間與黏詞");
  const withHit = await open({
    ask_local: answer({ hits: [hit()], brain: thinking }),
    ask: () => new Promise(() => {}),
  });
  await withHit.type("有本機記憶");
  check("R3 3：有東西時 brain-note 一字不變", withHit.hitTexts().includes("這些是我自己記得的；Grok CLI 還在想怎麼把它們講成一句話。"), withHit.hitTexts());
  check("R3 3：有東西時不說空手那句", !withHit.hitTexts().join("\n").includes("我好像不太知道你在說什麼耶"), withHit.hitTexts());

  const finalChecks = (p, name) => {
    const text = p.hitTexts().join("\n");
    check(`${name}：終局仍有「我記得的東西裡沒有這件事」`, text.includes("我記得的東西裡沒有這件事"), text);
    check(`${name}：終局 .hits-why 元素數 > 0`, p.hits().querySelectorAll(".hits-why").length > 0, text);
    check(`${name}：終局不留先開口那句`, !text.includes("我好像不太知道你在說什麼耶"), text);
  };
  const terminal = await open({
    ask_local: answer({ hits: [], brain: finalBrain, blind: finalBlind }),
    ask: () => new Promise(() => {}),
  });
  await terminal.type("沒有設定大腦");
  finalChecks(terminal, "R3 4");
  const limited = await open({
    ask_local: answer({ brain: finalBrain, blind: fullBlind }),
    ask: () => new Promise(() => {}),
  });
  await limited.type("只翻過三十天");
  check("R3 4：30 天終局仍說「我翻過的那幾段裡沒有這件事」", limited.hitTexts().includes("我翻過的那幾段裡沒有這件事。"), limited.hitTexts());

  let finish;
  const pending = new Promise(resolve => { finish = resolve; });
  const twoTrips = await open({
    ask_local: answer({ hits: [], brain: thinking, blind: fullBlind }),
    ask: () => pending,
  });
  await twoTrips.type("走完兩趟");
  provisionalChecks(twoTrips, "R3 5 第一張");
  finish(answer({ hits: [], brain: finalBrain, blind: finalBlind }));
  await tick(40);
  finalChecks(twoTrips, "R3 5 第二張");
}

console.log("A151 R5. 承諾必須有結果");
for (const [name, extra] of [
  ["空手", { hits: [] }],
  ["空總覽", { kind: "memory_overview", overview: { kind: "empty" } }],
]) {
  let rejectAsk;
  const p = await open({
    ask_local: answer({ ...extra, brain: { state: "thinking", provider: "Grok CLI" } }),
    ask: () => new Promise((_, reject) => { rejectAsk = reject; }),
  });
  await p.type("字面查不到的問題");
  check(`R5 ${name} 1 前提：先開口只有承諾且正式回答 pending`,
    typeof rejectAsk === "function" && p.hitTexts().length === 1 &&
    p.hitTexts()[0].includes("我再認真想一下"), p.hitTexts());
  rejectAsk(new Error("CLI 沒有回來"));
  await tick(40);
  const text = p.hitTexts().join("\n");
  check(`R5 ${name} 2：throw 解除承諾`, !text.includes("我再認真想一下"), text);
  check(`R5 ${name} 3：throw 明講這一題我沒答成`, text.includes("這一題我沒答成。"), text);
  // 不只排除長句：必須真的出現失敗結果，避免承諾未解除時真空通過。
  check(`R5 ${name} 4：失敗只用短句不冒稱上一題有幾筆`,
    p.hitTexts().length === 1 && text === "這一題我沒答成。" && !text.includes("底下原本那幾筆是上一題的"), text);
  check(`R5 ${name} 5：無本機答案記 error 而非 brain_error`,
    p.diagnoseNotes.some(n => n.why === "error") && !p.diagnoseNotes.some(n => n.why === "brain_error"), p.diagnoseNotes);
  check(`R5 ${name} 6：失敗後停止思考秒數`, p.thinking() === null, p.thinking());
}
{
  const p = await open({
    ask_local: answer({ hits: [hit({ snippet: "R5_KEEP_LOCAL" })], brain: { state: "thinking", provider: "Grok CLI" } }),
    ask: new Error("CLI 沒有回來"),
  });
  await p.type("有本機答案");
  check("R5 7：本機答案仍在、不冒充沒答成且記 brain_error",
    !p.hits().hidden && p.hitTexts().join("\n").includes("R5_KEEP_LOCAL") &&
    !p.hitTexts().join("\n").includes("這一題我沒答成") &&
    p.diagnoseNotes.some(n => n.why === "brain_error") && !p.diagnoseNotes.some(n => n.why === "error"),
    { texts: p.hitTexts(), notes: p.diagnoseNotes });
}
{
  let rejectAsk;
  const p = await open({
    ask_local: answer({ brain: { state: "thinking", provider: "Grok CLI" } }),
    ask: () => new Promise((_, reject) => { rejectAsk = reject; }),
  });
  await p.type("收起等待中的問題");
  await p.key("Escape");
  rejectAsk(new Error("R5 收起後 CLI 沒有回來"));
  await tick(40);
  check("R5 Esc：收起後失敗不彈回、內容換結果、秒數停且角落報錯",
    p.hits().hidden && !p.body().classList.contains("has-hits") &&
    p.hitTexts().join("\n") === "這一題我沒答成。" && p.thinking() === null &&
    p.line().includes("R5 收起後 CLI 沒有回來"),
    { hidden: p.hits().hidden, texts: p.hitTexts(), thinking: p.thinking(), line: p.line() });
}
{
  // 收起之後那一題**成功**了，氣泡要彈回來——和上面失敗那一條相反，而那是對的。
  // 理由與 `renderHits` 裡 `showAnswerHits()` 上面那段註解是同一個，第二條斷言
  // 守的就是那個理由本身：哪天有人加了「重新打開」的控制，它會紅，逼人回來
  // 重讀，而不是看到兩條分支不對稱就把它們「修」成一樣。
  let resolveAsk;
  const p = await open({
    ask_local: answer({ brain: { state: "thinking", provider: "Grok CLI" } }),
    ask: () => new Promise((resolve) => { resolveAsk = resolve; }),
  });
  await p.type("收起之後會成功的問題");
  await p.key("Escape");
  check("A152 收起後成功 1 前提：收起已生效而正式回答仍 pending",
    p.hits().hidden && typeof resolveAsk === "function",
    { hidden: p.hits().hidden, texts: p.hitTexts() });
  resolveAsk(answer({ hits: [hit({ snippet: "A152_ESC_THEN_OK" })] }));
  await tick(40);
  check("A152 收起後成功：氣泡彈回來，而且換成這一題真正的答案",
    !p.hits().hidden && p.body().classList.contains("has-hits") &&
    p.hitTexts().join("\n").includes("A152_ESC_THEN_OK") &&
    !p.hitTexts().join("\n").includes("我再認真想一下"),
    { hidden: p.hits().hidden, texts: p.hitTexts() });
}
{
  const opens = [...APP_SOURCE.matchAll(/hitList\.hidden\s*=\s*false/gu)]
    .map(m => APP_FUNCTIONS.find(fn => m.index >= fn.start && m.index < fn.end)?.name);
  check("A152 收起之後只有重畫救得回來：打開答案區的寫入端只有 showAnswerHits 一處",
    opens.length === 1 && opens[0] === "showAnswerHits", opens);
}

/*
 * 那個旗標的不變式，用原始碼結構守，不用情境守。
 *
 * `showingProvisional` 的規矩是「答案區的內容只剩那句承諾」。維持它靠三件事：
 * 唯一畫承諾的 helper 設真、每個**替換**答案區內容的地方清假、renderHits 開頭
 * 清假。前兩件事跑得到的情境測得出來，**第三個替換端測不到**——
 * `showConsentCompletion` 只有在「同意書不是被問題叫出來的」那條路才走得到，
 * 而那條路上沒有 in-flight 的 ask 可以丟例外。
 *
 * 我拿掉 `showConsentCompletion` 裡那行 `showingProvisional = false;` 跑過一次
 * 整支腳本：**880 綠、一條都沒紅**。那一行是對的，但沒有人守它——下一個把它
 * 當成多餘而刪掉的人不會收到任何警告。所以這裡改成數**寫入端**：哪天多一個
 * 替換答案區的函式，它就得自己表態。
 */
const CONTENT_REPLACERS = [...new Set(
  [...APP_SOURCE.matchAll(/hitList\.replaceChildren\(/gu)]
    .map(m => APP_FUNCTIONS.find(fn => m.index >= fn.start && m.index < fn.end)?.name)
    .filter(name => name !== undefined),
)];
const replacersNotClearing = CONTENT_REPLACERS.filter(name =>
  !/\bshowingProvisional\s*=\s*false\b/u.test(APP_FUNCTIONS.find(fn => fn.name === name).body));
const provisionalSetters = [...new Set(
  [...APP_SOURCE.matchAll(/\bshowingProvisional\s*=\s*true\b/gu)]
    .map(m => APP_FUNCTIONS.find(fn => m.index >= fn.start && m.index < fn.end)?.name),
)];

console.log("A151 R5b. 那個旗標的不變式由原始碼結構守著");
// 抽不到東西的掃描器和「全部都合格」長得一模一樣，所以前提自己先斷言一次。
check("R5b 真的抽到至少三個替換答案區內容的函式", CONTENT_REPLACERS.length >= 3, CONTENT_REPLACERS);
check("R5b 每個替換答案區內容的函式都在同一支裡清掉 showingProvisional",
  replacersNotClearing.length === 0, replacersNotClearing);
check("R5b 只有 appendProvisionalLine 把 showingProvisional 設成 true",
  provisionalSetters.length === 1 && provisionalSetters[0] === "appendProvisionalLine",
  provisionalSetters);

console.log("A150. has-hits 寫入端會在同一個函式重畫對話");
check("A150 has-hits 寫入端真的抽到至少一個", uniqueHasHitsWriters.length >= 1);
check("A150 每個 has-hits 寫入端同函式呼叫 paintConversation", writersWithoutPaint.length === 0,
  writersWithoutPaint.map(fn => fn.name).join(", "));

console.log("A149. 首次選角、同意按鈕、保存的朗讀開關");
{
  const fresh = () => consentView([false, false, false, false], [false, false, false, false]);
  const first = await open({ consent_read: fresh(), persona_select: new Error("disk full") }, { autoFirstPersona: false });
  check("A149 首次只顯示選角、不顯示同意書", !first.node("[data-persona-first-run]").hidden && first.consentGuide().hidden);
  check("A149 選角使用共享 catalog 的全部頭像與名字", first.node("[data-first-persona-choices]").children.length === 17 &&
    first.node("[data-first-persona-choices]").children.every((button, i) => button.children[0].src === globalThis.__AI_SISTER_PERSONA_CATALOG__.personas[i].portrait && button.children[1].textContent === globalThis.__AI_SISTER_PERSONA_CATALOG__.personas[i].alias));
  await first.clickElement(first.node("[data-persona-first-save]"));
  check("A149 選角保存失敗留在原頁且能重按", !first.node("[data-persona-first-run]").hidden && first.consentGuide().hidden && !first.node("[data-persona-first-save]").disabled && first.node("[data-persona-first-result]").textContent.includes("disk full"));
  await first.clickElement(first.node("[data-persona-first-save]"));
  check("A149 選角失敗重按真的再次寫入", first.invokes.filter(({cmd}) => cmd === "persona_select").length === 2);
  const selected = await open({ consent_read: fresh() }, { autoFirstPersona: false });
  await selected.clickElement(selected.node("[data-first-persona-choices]").children[1]);
  await selected.clickElement(selected.node("[data-persona-first-save]"));
  check("A149 就她保存所點角色後才顯示同意書", selected.invokes.some(({cmd,arg}) => cmd === "persona_select" && arg.id === "claude") && selected.node("[data-persona-first-run]").hidden && !selected.consentGuide().hidden);
  for (const [label, alterCatalog] of [
    ["missing", () => null],
    ["schema", (catalog) => ({ ...catalog, schema: "wrong" })],
    ["count", (catalog) => ({ ...catalog, personas: catalog.personas.slice(1) })],
    ["portrait", (catalog) => ({ ...catalog, personas: catalog.personas.map((p, i) => i ? p : { ...p, portrait: "wrong.webp" }) })],
  ]) {
    const next = fresh(); next.sheets[0].reviewed = true;
    const fallback = await open({ consent_read: fresh(), consent_set: next }, { autoFirstPersona: false, alterCatalog });
    check(`A149 catalog ${label} 失效跳過選角、預設角色可回答第一張`, fallback.node("[data-persona-first-run]").hidden && !fallback.consentGuide().hidden && !fallback.input().disabled && fallback.consentProgress().includes("1 / 4") && fallback.node("[data-avatar]").dataset.persona === "chatgpt" && !fallback.calls.includes("persona_select"));
    await fallback.type("不同意");
    check(`A149 catalog ${label} 失效仍能保存回答並走到第二張`, fallback.invokes.some(({cmd,arg}) => cmd === "consent_set" && arg.granted === false) && fallback.consentProgress().includes("2 / 4"));
  }
  const wrongId = await open({ consent_read: fresh(), persona_select: { id: "gemini" } }, { autoFirstPersona: false });
  await wrongId.clickElement(wrongId.node("[data-persona-first-save]"));
  check("A149 回傳別人角色留在選角並說明沒有讀回所選角色", !wrongId.node("[data-persona-first-run]").hidden && wrongId.consentGuide().hidden && !wrongId.node("[data-persona-first-save]").disabled && wrongId.node("[data-persona-first-result]").textContent.includes("沒有讀回所選角色") && wrongId.node("[data-avatar]").dataset.persona === "chatgpt");
  // 第一層 ID 相等之後，第二層 applyPersona 只會對未知 ID 回 false。
  // JSON 的穩定 ID 無法獨立走到這條；用 getter 做 seam 故障注入，不冒充 native JSON。
  let idReads = 0;
  const rejectedView = { get id() { return ++idReads === 1 ? "chatgpt" : "unknown-persona"; } };
  const unapplied = await open({ consent_read: fresh(), persona_select: rejectedView }, { autoFirstPersona: false });
  await unapplied.clickElement(unapplied.node("[data-persona-first-save]"));
  check("A149 回傳角色無法套用留在選角並說明沒有讀回所選角色", !unapplied.node("[data-persona-first-run]").hidden && unapplied.consentGuide().hidden && !unapplied.node("[data-persona-first-save]").disabled && unapplied.node("[data-persona-first-result]").textContent.includes("沒有讀回所選角色"));
  const returning = await open({ consent_read: consentView([true,false,false,false], [false,false,false,false]) }, { autoFirstPersona: false });
  check("A149 已回答過的人不再選角", returning.node("[data-persona-first-run]").hidden && !returning.consentGuide().hidden && !returning.calls.includes("persona_select"));
  for (const granted of [true, false]) {
    const answerView = fresh();
    answerView.sheets[0] = { ...answerView.sheets[0], reviewed: true, effective: granted, granted_at: granted ? 123 : null };
    const typed = await open({ consent_read: fresh(), consent_set: answerView });
    await typed.type(granted ? "同意" : "不同意");
    let complete;
    const clicked = await open({ consent_read: fresh(), consent_set: () => new Promise((resolve) => { complete = resolve; }) });
    await clicked.clickElement(clicked.node(granted ? "[data-consent-yes]" : "[data-consent-no]"));
    const writes = (p) => p.invokes.filter(({cmd}) => cmd === "consent_set").map(({arg}) => arg);
    check(`A149 按${granted ? "同意" : "不同意"}與打字送出完全相同參數`, writes(clicked).length === 1 && JSON.stringify(writes(clicked)) === JSON.stringify(writes(typed)));
    check(`A149 ${granted} 保存忙碌時兩顆按鈕與輸入一起停用`, clicked.node("[data-consent-yes]").disabled && clicked.node("[data-consent-no]").disabled && clicked.input().disabled);
    await clicked.clickElement(clicked.node("[data-consent-yes]"));
    check(`A149 ${granted} 忙碌重按不重複寫入`, writes(clicked).length === 1);
    complete?.(answerView);
    await tick();
    check(`A149 ${granted} 保存成功後兩顆按鈕放開`, !clicked.node("[data-consent-yes]").disabled && !clicked.node("[data-consent-no]").disabled);
  }
  const failedWrite = await open({consent_read: fresh(), consent_set: new Error("disk full")});
  await failedWrite.clickElement(failedWrite.node("[data-consent-yes]"));
  check("A149 同意寫入失敗兩鍵恢復、沒有假成功", !failedWrite.node("[data-consent-yes]").disabled && !failedWrite.node("[data-consent-no]").disabled && failedWrite.consentResult().includes("沒有保存") && failedWrite.consentProgress().includes("1 / 4"));
  await failedWrite.clickElement(failedWrite.node("[data-consent-no]"));
  check("A149 同意失敗後另一顆真的能再寫", failedWrite.invokes.filter(({cmd}) => cmd === "consent_set").length === 2);
  for (const enabled of [false, true]) {
    const next = fresh(); next.sheets[0].reviewed = true;
    const voice = await open({
      persona_read: { id: "chatgpt", consent_read_aloud: enabled },
      consent_read: fresh(), consent_set: next,
      persona_fixed_voice_admit: {presentation_id: "a149"},
    }, {consentVoices: consentVoiceManifest()});
    const plays = voice.audioPlays();
    check(`A149 重讀朗讀 ${enabled} 保留開關與首張播放意圖`, voice.consentListen().dataset["aria-pressed"] === String(enabled) && plays === (enabled ? 1 : 0));
    await voice.clickElement(voice.node("[data-consent-no]"));
    check(`A149 朗讀 ${enabled} 換張的音訊副作用`, enabled ? voice.audioPlays() === plays + 1 && voice.audioSrc() === "./persona-consent-voices/v1/chatgpt/cloud-reading.ogg" : voice.audioPlays() === 0 && !voice.calls.includes("persona_fixed_voice_admit"));
    const mismatched = consentView([true,true,false,false], [false,false,false,false]);
    mismatched.sheets[2].wording = "新版條文";
    // 以既有 consent_set 回覆推到不匹配條文。
    const noClip = await open({ persona_read: {id: "chatgpt", consent_read_aloud: enabled}, consent_read: mismatched }, {consentVoices: consentVoiceManifest()});
    check(`A149 ${enabled} 無相符錄音停用但保留開關`, noClip.consentListen().disabled && noClip.consentListen().dataset["aria-pressed"] === String(enabled) && noClip.audioPlays() === 0);
  }
  const toggle = await open({consent_read: fresh(), persona_fixed_voice_admit: {presentation_id:"a149"}}, {consentVoices: consentVoiceManifest()});
  await toggle.clickElement(toggle.consentListen());
  await toggle.clickElement(toggle.consentListen());
  check("A149 開關兩次各保存正確布林並停止音訊", JSON.stringify(toggle.invokes.filter(({cmd}) => cmd === "persona_consent_speak_set").map(({arg}) => arg)) === JSON.stringify([{enabled:true},{enabled:false}]) && toggle.audioPlays() === 1 && !toggle.isSpeaking() && toggle.consentListen().dataset["aria-pressed"] === "false");
  const failedToggle = await open({consent_read: fresh(), persona_consent_speak_set: new Error("disk full")}, {consentVoices: consentVoiceManifest()});
  await failedToggle.clickElement(failedToggle.consentListen());
  check("A149 開啟保存失敗保留關閉、不播放且可重按", failedToggle.audioPlays() === 0 && failedToggle.consentListen().dataset["aria-pressed"] === "false" && !failedToggle.consentListen().disabled && failedToggle.consentResult().includes("沒有保存"));
  for (const enabled of [false, true]) {
    const mismatch = await open({ consent_read: fresh(), persona_read: { id: "chatgpt", consent_read_aloud: enabled }, persona_consent_speak_set: { id: "chatgpt", consent_read_aloud: enabled } }, { consentVoices: consentVoiceManifest() });
    const playsBeforeMismatch = mismatch.audioPlays();
    await mismatch.clickElement(mismatch.consentListen());
    check(`A149 朗讀回傳不符 ${enabled} 保留原開關並說明沒有讀回朗讀設定`, mismatch.consentListen().dataset["aria-pressed"] === String(enabled) && !mismatch.consentListen().disabled && mismatch.consentResult().includes("沒有讀回朗讀設定") && mismatch.audioPlays() === playsBeforeMismatch, { pressed: mismatch.consentListen().dataset["aria-pressed"], result: mismatch.consentResult(), playsBeforeMismatch, plays: mismatch.audioPlays() });
  }
  const ended = await open({ consent_read: fresh(), persona_fixed_voice_admit: {presentation_id: "a149-ended"} }, { consentVoices: consentVoiceManifest() });
  await ended.clickElement(ended.consentListen());
  ended.finishAudio();
  await ended.clickElement(ended.consentListen());
  check("A149 已播完再關閉只說關閉、不宣稱停止朗讀", ended.audioPlays() === 1 && ended.consentResult() === "條文朗讀已關閉。" && ended.consentListen().dataset["aria-pressed"] === "false");
}
if (process.env.A149_ONLY === "1") process.exit(failed ? 1 : 0);

const CONSENT = "第一張同意書還沒簽——她不會開始記錄。在系統匣圖示上按右鍵，選「四張同意書…」簽好再回來";

console.log("A150. 回答氣泡收得起來，流程與獨立區塊不受影響");
{
  const p = await open({
    ask: answer({ hits: [hit()] }),
    recording_state: "recording",
  });
  await p.type("第一題");
  const close = p.node("[data-hits-close]");
  check(
    "A150 回答裡有一顆逐字是「收起」的按鈕",
    /<button\b[^>]*data-hits-close[^>]*>收起<\/button>/u.test(HTML) && !close.hidden,
  );

  await p.click("[data-hits-close]");
  check("A150 收起鍵拿掉 has-hits 並藏起回答", !p.body().classList.contains("has-hits") && p.hits().hidden);
  check("A150 收起後氣泡裡的控制不留在 Tab 順序", p.hits().hidden && close.hidden);

  await p.type("第二題");
  check("A150 收起後下一次回答重新顯示氣泡", p.body().classList.contains("has-hits") && !p.hits().hidden && !close.hidden);
  await p.key("Escape");
  check("A150 Esc 拿掉 has-hits 並藏起回答", !p.body().classList.contains("has-hits") && p.hits().hidden && close.hidden);
}
{
  let asks = 0;
  const collapsedThenFailed = await open({
    ask: () => ++asks === 1 ? answer({ hits: [hit()] }) : Promise.reject(new Error("fixture failed")),
    recording_state: "recording",
  });
  await collapsedThenFailed.type("先答成");
  await collapsedThenFailed.click("[data-hits-close]");
  await collapsedThenFailed.type("再失敗");
  check("A150 收起後下一題失敗只說這題沒答成", collapsedThenFailed.hitTexts().includes("這一題我沒答成。") &&
    collapsedThenFailed.hitTexts().every(text => !text.includes("先收起來了")), collapsedThenFailed.hitTexts().join(" | "));
}
{
  const waitingUrl = await open({
    ask: answer({ hits: [hit()] }),
    recording_state: "recording",
    url_policy_read: urlPolicy(),
  });
  await waitingUrl.type("先顯示回答");
  check("A150 回答顯示時網址政策題被壓住", waitingUrl.urlPolicy().hidden);
  await waitingUrl.click("[data-hits-close]");
  check("A150 收起回答當場重畫網址政策題", !waitingUrl.urlPolicy().hidden);
}
{
  const consent = await open(
    { consent_read: consentView([false, false, false, false], [false, false, false, false]) },
  );
  const close = consent.node("[data-hits-close]");
  check("A150 同意書顯示時收起鍵不出現", !consent.consentGuide().hidden && close.hidden);
  await consent.key("Escape");
  check("A150 Esc 不會關掉同意書", !consent.consentGuide().hidden && consent.body().classList.contains("has-consent-guide"));
}
{
  const first = await open(
    { consent_read: consentView([false, false, false, false], [false, false, false, false]) },
    { autoFirstPersona: false },
  );
  await first.key("Escape");
  check("A150 Esc 不會關掉第一次選角", !first.node("[data-persona-first-run]").hidden && first.body().classList.contains("has-consent-guide"));
}

console.log("① 按「開始記錄」，後端說同意書還沒簽");
{
  const p = await open({ start_recording: new Error(CONSENT), recording_state: "none" });
  await p.click("[data-wake]");
  check("當下說得出原因", p.line().includes("同意書"), p.line());
  await p.repaint();
  check("輪詢過後那句話還在", p.line().includes("同意書"), p.line());
  await p.repaint();
  check("再一輪也還在", p.line().includes("同意書"), p.line());
}

console.log("② 第一題就答不成（資料庫打不開）");
{
  const p = await open({
    ask: new Error("資料庫打不開：database is locked"),
    recording_state: "recording",
  });
  await p.type("剛剛發生什麼事");
  check("說得出是哪一種失敗", p.line().includes("database is locked"), p.line());
  check(
    "「我沒答成」那一列看得見（那個 ul 開場是 hidden）",
    p.hits().hidden === false,
    `hidden=${p.hits().hidden}`,
  );
  check(
    "而且真的有那一列",
    p.hits().children.some((c) => c.textContent.includes("沒答成")),
    p.hits().children.map((c) => c.textContent),
  );
  await p.repaint();
  check("輪詢過後那句原因還在", p.line().includes("database is locked"), p.line());
}

console.log("③ 送出去了他就看得到，等久了才多一個秒數");
{
  let finishAsk;
  const pendingAnswer = new Promise((resolve) => {
    finishAsk = resolve;
  });
  const p = await open({
    // 看完等待中的畫面才交回答案；runner 延遲不能讓答案先到、把秒數收掉。
    ask: () => pendingAnswer,
    recording_state: "recording",
  });
  void p.type("三天前那通電話");
  // 這一格的整個理由：**他按完 Enter 的下一刻**就要看得到，不是四秒之後。
  // 四秒是一段長到足夠讓人以為自己沒按到的時間。
  await tick(60);
  check("按下去就看得到「思考中…」", p.thinking() === "思考中…", p.thinking());
  check("而她的泡泡裡還是她自己的話", p.line().includes("想一下"), p.line());
  const late = await awaitRepaint(p, p.thinking());
  check("等久了會多一個秒數", /^思考中… \d+ 秒$/u.test(late ?? ""), late);
  check("那個秒數是真的在數", Number(/(\d+)/u.exec(late ?? "")?.[1] ?? 0) >= 4, late);
  await p.repaint();
  check("輪詢過後那一格還在", /^思考中…/u.test(p.thinking() ?? ""), p.thinking());
  // 她的泡泡從頭到尾都沒被拿去講儀表板。
  check("她從頭到尾沒解釋自己為什麼慢", !/秒|CLI|多半/u.test(p.line()), p.line());
  finishAsk(answer());
  await tick();
  check("答案回來那一格就不在了", p.thinking() === null, p.thinking());
}

console.log("④ 暫停鍵切不動");
{
  const p = await open({
    toggle_pause: new Error("找不到資料目錄，暫停鍵沒有作用"),
    recording_state: "recording",
  });
  await p.click("#pause");
  check("說得出原因", p.line().includes("暫停鍵沒有作用"), p.line());
  await p.repaint();
  check("輪詢過後還在", p.line().includes("暫停鍵沒有作用"), p.line());
}

console.log("⑤ 時間軸開不起來");
{
  const p = await open({
    open_timeline: new Error("開不了時間軸視窗：WebView2 沒裝"),
    recording_state: "recording",
  });
  await p.click("#timeline");
  check("說得出原因", p.line().includes("WebView2"), p.line());
  await p.repaint();
  check("輪詢過後還在", p.line().includes("WebView2"), p.line());
}

console.log("⑥ 下一個動作要蓋掉上一次那句話");
{
  const p = await open({
    toggle_pause: new Error("找不到資料目錄，暫停鍵沒有作用"),
    ask: answer(),
    recording_state: "recording",
  });
  await p.click("#pause");
  check("先有那句話", p.line().includes("暫停鍵沒有作用"), p.line());
  await p.type("剛剛發生什麼事");
  check("問了下一題就不該再掛著", !p.line().includes("暫停鍵沒有作用"), p.line());
}

console.log("⑦ 早上那句舊的，不可以擋住系統匣剛剛那句新的");
{
  // `paint()` 讀的是 `notice ?? (wakeFailed ?? asleepDetail())`。九點問問題失敗
  // 留下 `notice`，九點五分從系統匣按開始記錄失敗 → `recorder-failed`。後端還
  // 特地把視窗叫到他面前，而他看到的是早上那句。
  const p = await open({
    ask: new Error("資料庫打不開：database is locked"),
    recording_state: "none",
  });
  await p.type("剛剛發生什麼事");
  check("先有早上那句", p.line().includes("database is locked"), p.line());
  await p.fromOutside("recorder-failed", CONSENT);
  check("系統匣那句要看得到", p.line().includes("同意書"), p.line());
  check("而且早上那句要讓開", !p.line().includes("database is locked"), p.line());
}

console.log("⑧ 從系統匣暫停成功之後，「暫停鍵沒有作用」不可以還掛著");
{
  const p = await open({
    toggle_pause: new Error("找不到資料目錄，暫停鍵沒有作用"),
    recording_state: "recording",
  });
  await p.click("#pause");
  check("先有那句", p.line().includes("暫停鍵沒有作用"), p.line());
  await p.fromOutside("pause-changed", true);
  check("她真的暫停了", p.line().includes("已暫停"), p.line());
  // 兩行都曾經是真的，湊起來在說「暫停鍵壞了」——而她正暫停著。
  check("那句「沒有作用」要跟著走", !p.line().includes("沒有作用"), p.line());
}

console.log("⑨ 第一次問問題就失敗的人，底下沒有「上一題」");
{
  const p = await open({
    ask: new Error("資料庫打不開：database is locked"),
    recording_state: "recording",
  });
  await p.type("三天前那通電話");
  check("有說沒答成", p.hitTexts().some((t) => t.includes("沒答成")), p.hitTexts());
  check(
    "但不可以說「底下原本那幾筆是上一題的」——他底下從來沒有東西",
    !p.hitTexts().some((t) => t.includes("上一題")),
    p.hitTexts(),
  );
}

console.log("⑩ 答成過一次之後再失敗，才輪得到那句「先收起來了」");
{
  let fail = false;
  const p = await open({
    ask: () => {
      if (fail) throw new Error("資料庫打不開：database is locked");
      return answer({ hits: [hit()] });
    },
    recording_state: "recording",
  });
  await p.type("電話");
  check("先答成一次", p.hits().hidden === false, p.hitTexts());
  fail = true;
  await p.type("電話");
  check("這次說得出「上一題」", p.hitTexts().some((t) => t.includes("上一題")), p.hitTexts());
}

console.log("⑪ 那個秒數數的是這一題，不是上一題");
{
  // 時間軸（SLOW_MS = 4000）：
  //   t=0     第一題送出，永遠不回來
  //   t=3800  第二題送出（第一題還掛著，`state` 一直是 thinking）
  //   t=4300  看畫面：第二題才半秒大，那一格不可以印出一個四以上的數字
  //
  // 舊版這裡守的是一句話（「超過 4 秒」有沒有蓋上去）。換成秒數之後，同一個
  // 錯會長成另一個樣子：**數字繼續從第一題算**。那更難看得出來——它不是一句
  // 突然冒出來的話，是一個看起來很合理、只是講錯題目的數字。
  //
  // **兩個取樣點，不是一個。** 第一版只在 t=4300 看一眼，而那一刀（起算點不
  // 跟著新題目走）照樣是綠的——因為第二題送出的那一刻自己畫了一次「思考中…」，
  // 而下一次重畫要等它的計時器，落在 t=4800。畫面在那之間根本沒動過，所以
  // 「看起來對」量到的是**沒有重畫**，不是算對了。第二個取樣點跨過那一拍，
  // 而且斷言的是一個確切的數字：算錯起算點會多印，計時器死掉會少印。
  const p = await open({
    ask: (arg) =>
      arg.question === "第一題"
        ? new Promise(() => {})
        : new Promise((r) => setTimeout(() => r(answer()), 8000)),
    recording_state: "recording",
  });
  void p.type("第一題");
  await tick(3800);
  // 第二題送出的時刻。底下那條斷言的期望值是從這裡算出來的，不是寫死的——
  // 見那一段的理由。
  const secondAskedAt = Date.now();
  void p.type("第二題");
  await tick(1200); // t=5000：第二題才 1.2 秒大，還不到那條四秒線
  check("還在想第二題", p.line().includes("想一下") || p.line().includes("在聽"), p.line());
  check("第二題還沒到四秒，就不該有數字", p.thinking() === "思考中…", p.thinking());
  // 取樣點不是 `tick()` 猜的，是等重畫真的發生（見 `awaitRepaint`）。
  //
  // **期望值也不寫死。** 以前這裡是 `p.thinking() === "思考中… 4 秒"`，而那個
  // `4` 綁著一件沒有人承諾過的事：重畫**剛好**落在 4000–5000 ms 那一格裡。
  // 機器一卡，第四跳整格跳過去印 5——產品一個字都沒錯（它印的一直是真的秒數），
  // 紅的是測試自己多加的那個假設。
  //
  // 改成拿測試自己的碼錶對：`expected` 是**第二題**到現在的秒數，而畫面上那個
  // 數字必須跟它對得起來（差一格，因為讀到的那次重畫發生在取樣之前）。
  // 這樣就完全不管它是第幾跳：
  //
  //   - 起算點沒跟著新題目走 → 畫面印的是**第一題**的年紀，比 `expected` 多 3 秒以上 → 紅。
  //   - 計時器死掉 → 逾時，`crossed` 還是沒有數字的那串 → `NaN` → 紅。
  //   - 機器卡到第四跳晚了一整秒 → 畫面 5、`expected` 也 5 → 綠，本來就該綠。
  //
  // `>= 4` 保留的是另一件事：它真的跨過了那條四秒線才開始印數字。
  const crossed = await awaitRepaint(p, p.thinking());
  const crossedSecs = Number(/^思考中… (\d+) 秒$/u.exec(crossed ?? "")?.[1] ?? NaN);
  const expected = Math.floor((Date.now() - secondAskedAt) / 1000);
  check(
    "過線之後數的是第二題那 4 秒",
    crossedSecs >= 4 && Math.abs(crossedSecs - expected) <= 1,
    { crossed, expected },
  );
}

console.log("⑪b 全停下來的時候，那個秒數不可以繼續數");
{
  // 全停會把 `asking` 加一，讓還在飛的那一題失效。**於是它的 `finally` 那條
  // `mine === asking` 是假的，`stopThinking()` 不會跑**——那條判斷是為了「他
  // 又送了下一題」寫的，而全停不是下一題，是沒有下一題了。少了補的那一行，
  // 畫面會停在「思考中… 41 秒」一路數下去，而她已經停了。
  const p = await open({
    ask: () => new Promise(() => {}),
    recording_state: "recording",
  });
  void p.type("這一題永遠不會回來");
  await tick(60);
  check("前提：那一格真的在數", p.thinking() !== null, p.thinking());
  await p.fromOutside("master-stop-changed", "stopped");
  await tick(60);
  check("全停之後那一格就不在了", p.thinking() === null, p.thinking());
  check("而畫面講的是全停", p.line().includes("全停"), p.line());
}

console.log("⑫ 整場下來，畫面上沒有出現過 NaN / undefined");
{
  const p = await open({
    ask: answer({ blind: blind({ ever_recorded: true, frames: 12, chunks: 0 }) }),
    recording_state: "recording",
  });
  await p.type("三天前那通電話");
  // 假的 `blind` 少抄一欄的時候，那幾句「為什麼答不出來」會印出 undefined，
  // 而上面每一條斷言都不會發現——它們問的都是別的句子。
  check("那幾句「為什麼」是完整的", p.nonsense().length === 0, p.nonsense());
  check("而且真的說了為什麼", p.hitTexts().some((t) => t.includes("12 張畫面")), p.hitTexts());
}

console.log("⑬ 她正在錄，從系統匣按「停止記錄」失敗——那句話要看得到");
{
  // 系統匣那一格是**開關**（`main.rs` 的 `record_label`），所以 `recorder-failed`
  // 也會帶著「停不了」回來，而那一刻她正在錄。以前這句話走的是「叫不起來」那條
  // 路，而那條路在 `paint()` 裡被一道 `shown === "asleep"` 的閘門擋著——後端特地
  // `win.show()` + `set_focus()` 把視窗叫到他面前，然後那一格一個字都沒多。
  const p = await open({ recording_state: "recording" });
  check("先確認她真的在錄", p.line().includes("在聽"), p.line());
  await p.fromOutside("recorder-failed", "找不到資料目錄，停不了");
  check("那句話要出現在畫面上", p.line().includes("停不了"), p.line());
  // 反面：她在錄的時候不可以順便把狀態講成灰的。
  check("而且上面那行還是「她在錄」", p.line().includes("在聽"), p.line());
}

console.log("⑭ 叫她起來那幾秒中間問了一題失敗，真正的原因要贏");
{
  // `startRecording` 開頭清過一次，但那次清距離 `catch` 隔著一整段 `await`。
  // 中間他問一題、失敗了，於是接下來那句「第一張同意書還沒簽」被擋住——兩行
  // 都是真的，湊起來是「她沒起來，因為資料庫打不開」，而他會去查一顆好好的
  // 資料庫。真正的原因在右鍵選單裡，一下就簽得掉。
  const p = await open({
    recording_state: "none",
    start_recording: () =>
      new Promise((_, reject) => setTimeout(() => reject(new Error(CONSENT)), 700)),
    ask: new Error("資料庫打不開：database is locked"),
  });
  void p.click("[data-wake]");
  await tick(100);
  await p.type("剛剛發生什麼事");
  check("中間那句先在", p.line().includes("database is locked"), p.line());
  await tick(900);
  check("同意書那句要贏", p.line().includes("同意書"), p.line());
  check("而且中間那句要讓開", !p.line().includes("locked"), p.line());
}

console.log("⑮ 她開完資料庫的那一刻，「還在開資料庫，暫停鍵沒有作用」要走");
{
  // 這一條驗的是 `setRecording` 裡那一行 `overtakenByEvents()`——它是三個呼叫端
  // 裡唯一沒有測試的一個（拿掉它，五支閘門全綠）。
  //
  // 留著的話畫面是「在聽／她還在開資料庫，暫停鍵現在沒有作用」：上面那行剛說
  // 她開完了，下面那句說她還在開。兩行直接互相矛盾。
  let heartbeat = "booting";
  const p = await open({
    // 狀態由測試明確切換，不拿「第幾次 invoke」代替狀態。listener 註冊完成後
    // 也會補讀一次；若靠次數，那一次合法的 read 會憑空替 recorder 開完資料庫。
    recording_state: () => heartbeat,
    toggle_pause: new Error("她還在開資料庫，暫停鍵現在沒有作用"),
  });
  check("開場是正在起來", p.line().includes("正在開資料庫"), p.line());
  await p.click("#pause");
  check("先有那句", p.line().includes("暫停鍵現在沒有作用"), p.line());
  heartbeat = "recording";
  await p.pollNow();
  check("她開完了", p.line().includes("在聽"), p.line());
  check("那句「還在開資料庫」要跟著走", !p.line().includes("還在開資料庫"), p.line());
}

console.log("⑯ 反面：狀態沒變的那幾輪輪詢，那句話不可以自己消失");
{
  // ⑮ 的修法很容易做成「每次輪詢都清」，那就變回 alpha.38 那個「五句話壽命
  // 0 到 5 秒」的 bug。這兩條要一起看才有意義。
  const p = await open({
    recording_state: "recording",
    toggle_pause: new Error("找不到資料目錄，暫停鍵沒有作用"),
  });
  await p.click("#pause");
  check("先有那句", p.line().includes("沒有作用"), p.line());
  await tick(5400);
  check("兩輪輪詢過後它還在", p.line().includes("沒有作用"), p.line());
}

console.log("⑰ 上一題空手而回，下一題失敗的時候不可以說「底下原本那幾筆」");
{
  // 底下躺的是「我記得的東西裡沒有這件事。」加幾行理由——一筆都沒有。而空手
  // 而回正是他最可能連著問第二次的那一種結果。
  let n = 0;
  const p = await open({
    ask: () => (++n === 1 ? answer({ hits: [], facts: [] }) : Promise.reject(new Error("讀不到"))),
    recording_state: "recording",
  });
  await p.type("三天前那通電話");
  check("第一題真的空手", p.hitTexts().some((t) => t.includes("沒有這件事")), p.hitTexts());
  await p.type("再問一次");
  check("有說沒答成", p.hitTexts().some((t) => t.includes("沒答成")), p.hitTexts());
  check(
    "但底下從來沒有「那幾筆」可以收",
    !p.hitTexts().some((t) => t.includes("上一題")),
    p.hitTexts(),
  );
}

console.log("⑱ 答成過、然後連著失敗兩次——第二次底下躺的是同一句錯誤，不是「上一題」");
{
  // 順序要**先答成一次**：`showingAnswer` 的初始值就是 false，所以從一張新的
  // 頁面連失敗兩次是驗不到東西的（那是 ⑰ 那條路）。要先讓它變成 true，才問
  // 得出「失敗那一次有沒有把它放回去」——守的是 `ask()` 的 catch 裡那一行
  // `showingAnswer = false`。
  let n = 0;
  const p = await open({
    ask: () => (++n === 1 ? answer({ hits: [hit()] }) : Promise.reject(new Error("讀不到"))),
    recording_state: "recording",
  });
  await p.type("第一題");
  check("第一題真的列出東西", p.hitTexts().some((t) => t.includes("客服")), p.hitTexts());
  await p.type("第二題");
  check("第二題說得出「上一題」", p.hitTexts().some((t) => t.includes("上一題")), p.hitTexts());
  await p.type("第三題");
  check("兩次都說沒答成", p.hitTexts().some((t) => t.includes("沒答成")), p.hitTexts());
  check(
    "但第三次底下躺的是同一句錯誤，不是上一題的答案",
    !p.hitTexts().some((t) => t.includes("上一題")),
    p.hitTexts(),
  );
}

console.log("⑲ 她**真的正在起來**的那 25 秒裡問了一題失敗，那一行要自己帶主詞");
{
  // ⑭ 驗的是 `start_recording` **被 reject** 的那條路，而那是這個 bug 比較小的
  // 一半：`await` 那一瞬間過去就結束了。真正長的是它 **resolve** 之後——
  // `starting` 會一路真到輪詢看見 recording 為止，最久 25 秒（`WAKE_TIMEOUT_MS`）。
  // 而那正是他最可能去問一題的 25 秒，因為畫面剛剛叫他等一下。
  //
  //     正在把她叫起來…
  //     資料庫打不開：database is locked
  //
  // 兩行都是真的。湊起來只讀得出一個意思——她起不來，因為資料庫打不開——而
  // 她其實好好地正在起來，那顆資料庫也沒有壞：那一題會失敗，就是因為她正在
  // 開它。他於是去修一顆沒有壞的東西。
  // 兩個狀態都要驗。`booting` 那一半**更長**——他那顆一年份的資料庫要開好幾
  // 分鐘（app.js 自己的註解就是這樣寫的），比 `starting` 的 25 秒長得多。少
  // 掉它，`starting || booting` 砍成 `starting` 會全綠。
  const OOPS = "資料庫打不開：database is locked";
  //
  // **`booting` 那一格不可以按「叫她起來」。** 第一版按了，於是那一格 `starting`
  // 也是真的，而 `line` 先看 `booting`——標題長得一模一樣，前提斷言就這樣為了
  // 錯的理由過關。抓到它的是 `starting || booting` 砍成 `starting` 之後**全綠**。
  // 真實情況本來就不必按：她會 booting，是因為系統匣或上一場把 recorder 開起
  // 來了，而他在那幾分鐘裡問了一題。
  for (const [what, state, head, wake] of [
    ["還在叫（25 秒）", "none", "正在把她叫起來", true],
    ["心跳看到了、資料庫還在開（好幾分鐘）", "booting", "正在開資料庫", false],
  ]) {
    console.log(`  — ${what}`);
    const p = await open({
      // 從頭到尾停在同一個狀態：她起得來，只是還沒起完。這就是那段窗。
      recording_state: state,
      start_recording: () => Promise.resolve(null),
      ask: new Error(OOPS),
    });
    if (wake) await p.click("[data-wake]");
    check("前提：她正在起來", p.line().includes(head), p.line());
    await p.type("剛剛發生什麼事");
    check("那句錯誤要在", p.line().includes(OOPS), p.line());
    // **只斷言「沒說 X」是不夠的**——那一行整個消失也會綠，而那是另一個 bug。
    // 要問的是它有沒有**變成另一句話**：底下那一行不可以就是那句錯誤本身。
    const detail = p.line().split("\n")[1] ?? "";
    check("但它不可以就這樣貼在底下", detail !== OOPS, detail);
    check("要看得出來是另一件事", detail.includes("另一件事"), detail);
  }
}

console.log("⑳ 反面：她灰著、而且真的是叫不起來的時候，那一行不可以說「另一件事」");
{
  // ⑲ 的修法最容易做成「一律加前綴」，那就在另一個方向上說謊：這一次那句話
  // **就是**在講她為什麼沒起來，前綴會把唯一那條線索推開。三個會寫「叫不起
  // 來」的地方都先關掉 `starting` 才重畫，⑲ 那條路靠的就是這件事——這一條驗
  // 的是它還成立。
  const p = await open({ start_recording: new Error(CONSENT), recording_state: "none" });
  await p.click("[data-wake]");
  // 前提就是那件讓 ⑲ 成立的事：寫「叫不起來」的人先關掉了 `starting`。
  check("已經不是「正在把她叫起來」了", !p.line().includes("正在把她叫起來"), p.line());
  check("同意書那句在", p.line().includes("同意書"), p.line());
  check("而且它就是在講她——不可以推開", !p.line().includes("另一件事"), p.line());
}

console.log("㉑ 她以前暫停過，不可以把「我現在就是瞎的」那句話吃掉");
{
  // `renderBlind` 裡「我現在是暫停的」掛在 `else if (blind.paused_now)`，所以
  // 只有一段暫停紀錄都沒有的時候才說得出口。錄的時候暫停又解除過一次、後來
  // 在沒有人在錄的時候又按了一次暫停的人，讀到的是
  //
  //     我也被暫停過 1 次，那幾段是空的。
  //
  // 過去式，話說完了。而他此刻是瞎的——下一次按開始記錄會錄一整天的空白。
  // `ops.rs` 的 `blind_lines` 是同一個 bug（那邊那條 else 的註解自己寫著
  // 「這一條比上面那條更需要講」，然後坐在會被擋掉的位置上）。
  const p = await open({
    recording_state: "recording",
    ask: answer({
      blind: blind({
        ever_recorded: true,
        ever_stored: true,
        chunks: 10,
        paused_episodes: 1,
        paused_ms: 300_000,
        paused_now: true,
      }),
    }),
  });
  await p.type("上禮拜那通電話");
  const said = p.hitTexts().join("\n");
  check("以前那幾段還是要講", said.includes("被暫停過 1 次"), said);
  check("而現在瞎著才是要命的那一句", said.includes("我現在是暫停的"), said);
  check("沒有 undefined 混進去", p.nonsense().length === 0, p.nonsense());
}

console.log("㉒ 反面：她現在沒有暫停的時候，不可以憑空多一句說她瞎著");
{
  const p = await open({
    recording_state: "recording",
    ask: answer({
      blind: blind({
        ever_recorded: true,
        ever_stored: true,
        chunks: 10,
        paused_episodes: 1,
        paused_ms: 300_000,
        paused_now: false,
      }),
    }),
  });
  await p.type("上禮拜那通電話");
  const said = p.hitTexts().join("\n");
  check("以前那幾段要講", said.includes("被暫停過 1 次"), said);
  check("但不可以說她現在是暫停的", !said.includes("我現在是暫停的"), said);
}

console.log("㉓ `booting` 那幾分鐘從系統匣按下去，回來那句話**就是**在講她");
{
  // ⑲ 那個修法的反面，而且是危險的那一面。alpha.41 的第一版拿「她正在起來」
  // 當「所以這句話一定不是在講她」的證據，然後在 `booting` 整段時間裡對每一句
  // `recorder-failed` 貼上「這是另一件事：」。那個推論在這裡是**反的**：
  //
  //   · `booting` 的時候 `recording_state` 不是 `"none"`；
  //   · 系統匣那顆 handler 讀真相不讀標籤（`main.rs`：`let on = recording_state
  //     (...) != "none"`），所以它送的是 `stop_recording`；
  //   · 於是 `booting` 期間唯一送得出 `recorder-failed` 的路**就是停不掉**
  //     （`main.rs` 那一行是全 repo 唯一的 emitter）。
  //
  // 也就是說：那個前綴在這一格 100% 是假的，而它推開的是唯一一句「你按的停止
  // 沒有生效」。上面那行還寫著「這期間還沒開始記」——兩句合起來讀成「她好好
  // 的，另外有個檔案權限問題」，他走開，她開完資料庫錄一整天。
  const STOP_FAILED =
    "write C:\\Users\\ted\\AppData\\Roaming\\ted-h\\AI-Sister\\data\\stop.request: Access is denied. (os error 5)";
  const p = await open({ recording_state: "booting" });
  check("前提：她正在開資料庫", p.line().includes("正在開資料庫"), p.line());
  await p.fromOutside("recorder-failed", STOP_FAILED);
  check("那句話要在", p.line().includes("Access is denied"), p.line());
  // **前提要在事後再驗一次。** 少了這一條，任何一個把 `booting` 弄丟的改動都
  // 會讓底下那句「沒有前綴」為了錯的理由過關——不是 booting 了，本來就不加。
  check("而且她還在 booting（不然下面那條等於沒驗）", p.line().includes("正在開資料庫"), p.line());
  const detail = p.line().split("\n")[1] ?? "";
  check("不可以被推開成「另一件事」——他按的停止沒生效，那正是在講她", !detail.includes("另一件事"), detail);
}

console.log("㉔ 叫她起來的中途又從系統匣按了一次，不可以當場宣告她沒起來");
{
  // 他按了「叫她起來」，沒耐心，又從系統匣按一次「開始記錄」。那一刻心跳還沒
  // 蓋出來，`recording_state` 還是 `"none"`，所以那一顆走 `start_recording`、
  // 撞上 `spawned.try_wait()` 回 `Ok(None)`，回一句「還在起來，再等一下」。
  //
  // `recorder-failed` 那個 listener 以前無條件 `starting = false`，於是：
  //
  //     沒有人在記錄——從現在起發生的事，她不會知道
  //     上一次按的那個還在起來——…再等一下
  //
  // 兩行都是真的，而它們直接互相矛盾。而且「叫她起來」那顆會跟著跳回來，他再
  // 按一次拿到同一句話——正是 `booting` 三態當初要消滅的那個迴圈。
  const STILL = "上一次按的那個還在起來——第一次開資料庫要重建索引，大的資料庫可能要幾分鐘。再等一下";
  const p = await open({
    recording_state: "none",
    start_recording: () => Promise.resolve(null),
  });
  await p.click("[data-wake]");
  check("前提：她正在起來", p.line().includes("正在把她叫起來"), p.line());
  await p.fromOutside("recorder-failed", STILL);
  check("那句話要在", p.line().includes("再等一下"), p.line());
  check("而上面那行不可以翻成「沒有人在記錄」", p.line().includes("正在把她叫起來"), p.line());
  check("「叫她起來」那顆也不可以跳回來", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);
  // 這一句**就是**在講她（系統匣送的不是 start 就是 stop），所以不加前綴。
  const detail = p.line().split("\n")[1] ?? "";
  check("而且它不是「另一件事」", !detail.includes("另一件事"), detail);
}

console.log("㉖ 她好好地在錄的時候，那句前綴是純噪音——不可以出現");
{
  // 前綴存在的唯一理由，是「上面那行講的是一個**還沒完成的轉換**，所以底下
  // 那行會被讀成它的原因」。她已經在錄了的時候上面那行是「在聽」，沒有因果
  // 可以誤讀；而一句到處都貼的「這是另一件事」，會在真正需要它的那一格失去
  // 分量。少了這一條，`(starting || booting) && !aboutHer` 砍成 `!aboutHer`
  // 全綠——這是 ㉓/㉔ 寫完之後跑突變才發現的洞。
  const OOPS = "資料庫打不開：database is locked";
  const p = await open({ recording_state: "recording", ask: new Error(OOPS) });
  await p.type("剛剛發生什麼事");
  check("前提：她在錄，不是正在起來", p.line().startsWith("在聽"), p.line());
  check("那句錯誤要在", p.line().includes(OOPS), p.line());
  check("但不加前綴", !p.line().includes("另一件事"), p.line());
}

console.log("㉕ `notice` 只有那兩個具名函式寫得進去");
{
  // `paint()` 讀的是 `notice.text` / `notice.aboutHer`。哪天有人寫回一句裸字串
  // （這一頁沒有型別可以擋，而 alpha.41 之前那七個寫入點全是裸字串），畫面上
  // 會是「undefined」——沒有例外、沒有紅字，只有一格看起來像壞掉的字。
  //
  // 更重要的是**語意**那一半：新的寫入點不經過那兩個函式，就等於沒有人回答過
  // 「這句話在講誰」，而 `paint()` 會替它猜——那正是這一輪修掉的東西。
  const src = read(join(UI, "app.js"));
  const writes = [...src.matchAll(/^\s*notice = (.+)$/gm)].map((m) => m[1].trim());
  const stray = writes.filter((w) => w !== "null;" && !w.startsWith("{ text:"));
  check("每一句 notice = 不是 null 就是那兩個函式裡的物件", stray.length === 0, stray);
  // 活體：正規表示式挑不到東西的話上面那條永遠綠，而它守的線是一格空白。
  check(`而且真的掃到了（${writes.length} 句）`, writes.length >= 5, writes.length);
  const setters = src.match(/^function noticeAbout\w+\(/gm) ?? [];
  check("那兩個具名函式也還在", setters.length === 2, setters);
}

console.log("㉗ `?asleep=nobeat` 這條 demo 路徑，在 booting 上也不可以被推開");
{
  // 他在**這台機器**上看這幾格畫面只有一條路：那幾個 `?…` 網址。Tauri 開不
  // 起來，所以「等了 25 秒還沒有心跳」那句話長什麼樣、換行撐不撐得住版面，
  // 是靠那條路用眼睛看的。而那條路以前一次測試都沒走過（`location.search`
  // 在這支腳本裡是寫死的 `""`）。
  //
  // 這一條驗的是它和 ㉓ 的交叉：`?state=booting` 讓她停在「正在開資料庫」，
  // 而 `?asleep=nobeat` 那句話講的正是她起不來。兩個湊在一起的時候，那句話
  // 前面不可以冒出「這是另一件事」——那會讓他在一台**真的沒起來**的機器上，
  // 讀到一句叫他別在意的話。
  const p = await open(
    { recording_state: "booting" },
    { search: "?state=booting&asleep=nobeat", browserOnly: true },
  );
  check("前提：她停在正在開資料庫", p.line().includes("正在開資料庫"), p.line());
  check("那句話要在", p.line().includes("等了 25 秒還沒有心跳"), p.line());
  const detail = p.line().split("\n").slice(1).join("\n");
  check("不可以被推開成「另一件事」", !detail.includes("另一件事"), detail);
  // 這條 demo 路徑餵的是三行字。少一行就代表 app.js 那邊被改短了，而它撐不撐
  // 得住版面正是他要看的東西——「有出現」不等於「整段都在」。
  check("三行都還在", detail.split("\n").length === 3, detail.split("\n").length);
}

console.log("㉘ 錄製已停、腦還在想最後一段：不是「在聽」，也不是「沒有人在記錄」");
{
  // 按下停止之後那兩分鐘。心跳說沒在錄（她不抓畫面了），行程還握著資料庫。
  // 畫面若走 `"none"`，他會按開始，然後撞上佔著閘門——兩句對打。
  const p = await open({ recording_state: "thinking" });
  check("說她在想最後一段", p.line().includes("想最後一段"), p.line());
  check("不可以說在聽", !p.line().includes("在聽"), p.line());
  check("不可以說沒有人在記錄", !p.line().includes("沒有人在記錄"), p.line());
  const wake = p.node("[data-wake]");
  check("開始鍵要藏起來——按下去只會撞上佔著閘門", wake.hidden === true, `hidden=${wake.hidden}`);
  await p.repaint();
  check("輪詢過後還是同一句", p.line().includes("想最後一段"), p.line());
}

console.log("㉙ 錄過但一列都沒存：指到 doctor，不指不存在的設定段落");
{
  const p = await open({
    recording_state: "none",
    ask: answer({ blind: blind({ ever_recorded: true, ever_stored: false }) }),
  });
  await p.type("剛剛發生什麼事");
  const text = p.hitTexts().join("\n");
  check("下一步是 sister doctor", text.includes("sister doctor"), text);
  check("不再指向設定頁的開始記錄段落", !text.includes("設定頁的「開始記錄」"), text);
}

console.log("㉚ 拔手熱鍵按下去之後，那句話要真的出現在畫面上");
{
  // 熱鍵存在的理由是「她不在畫面上的時候我也想按」，所以按完他能讀到的只有
  // 系統匣那兩行（要點開選單）和這一行。少了這條路，按下去的後果是視窗被叫
  // 出來、然後一個字都沒多。
  //
  // 四種結局裡有兩種的下一步是**相反**的（「沒寫成，可是她已經停了」對上
  // 「沒寫成，手還接著」），而字母人灰不灰分不出它們——所以驗的是整句話進
  // 得了那一格，不是「有沒有被叫出來」。句子本身在 `kill_switch.rs` 那邊有
  // 自己的測試，這裡只證它送得到。
  const PULLED = "手拔掉了。她現在什麼都不會交給作業系統。";
  const p = await open({ recording_state: "recording" });
  await p.fromOutside("hands-pulled", PULLED);
  check("那句話在狀態行上", p.line().includes(PULLED), p.line());

  // 而且它是「他剛剛按下去的那一下」，所以下一件事要蓋得掉——不然明天早上
  // 那行字還寫著「手拔掉了」，而他中午就把手接回去了。
  await p.fromOutside("pause-changed", true);
  check("下一件事蓋得掉", !p.line().includes(PULLED), p.line());

  const booting = await open({ recording_state: "booting" });
  check("前提：她正在開資料庫", booting.line().includes("正在開資料庫"), booting.line());
  await booting.fromOutside("hands-pulled", PULLED);
  check("booting 時那句話也在", booting.line().includes(PULLED), booting.line());
  check("而且她還在 booting（不然下面那條等於沒驗）", booting.line().includes("正在開資料庫"), booting.line());
  const detail = booting.line().split("\n")[1] ?? "";
  check("拔手結果不可以被推開成另一件事", !detail.includes("另一件事"), detail);
}

console.log("㉛ 送出去的事件名字，另一邊要真的有人在聽");
{
  // 上面那一節證的是「事件到了，話就上得了畫面」。它證不到的是**事件會不會
  // 到**：native Rust 那個名字和兩扇 renderer 裡的名字是各自寫死的字串，中間
  // 沒有共用的常數。實測過——把 `app.emit("hands-pulled", …)` 改成
  // `"hands-pulled-x"`，這支腳本、`check-settings-say.mjs`、`check-windows.sh`
  // 全綠，而使用者按下熱鍵之後畫面一個字都不會多。
  //
  // **這一條擋不住什麼，先寫在這裡：** 名字對、送的值是空的（把
  // `announce_hands_pulled(app, &says)` 改成 `announce_hands_pulled(app, "")`）
  // 一樣全綠。要抓那一種得真的把 Tauri 跑起來；這裡只保證兩張名單對得上。
  //
  // 兩個方向都實測過，但它們的來路不一樣：改 `main.rs` 那個名字，底下兩條
  // 斷言同時紅（那是這一節唯一的偵測器）；改 `app.js` 那個名字，前面的
  // `fromOutside` 會先丟「沒有人在聽 ⋯」，這一節根本沒跑到。所以「聽的 X
  // 真的有人送」是給**還沒有人驅動的新 listener** 留的後備，不是主力。
  // `local-tts-changed`／`local-tts-stop` 寫在 local_tts.rs，跟
  // recorder_supervisor.rs 一樣要掃進來，不然這一節看不見。
  const rustSources = [
    read(join(UI, "../src-tauri/src/main.rs")),
    read(join(UI, "../src-tauri/src/recorder_supervisor.rs")),
    read(join(UI, "../src-tauri/src/local_tts.rs")),
  ].join("\n");
  const eventConstants = new Map(
    [...rustSources.matchAll(/const\s+(\w+)\s*:\s*&str\s*=\s*"([^"]+)"/g)].map((m) => [
      m[1],
      m[2],
    ]),
  );
  const emitted = [...rustSources.matchAll(/\.emit\(\s*(?:"([^"]+)"|(\w+))/g)]
    .map((m) => m[1] ?? eventConstants.get(m[2]))
    .filter((name) => name !== undefined);
  // `persona-assets-changed` 的接收者合理地是設定頁，不是 pet；只掃 app.js 會逼
  // 一扇不需要該事件的視窗掛假 listener。聚合兩份真正有 Tauri event 的 renderer。
  const rendererSources = `${read(SRC)}\n${read(join(UI, "settings.js"))}`;
  const heard = [...rendererSources.matchAll(/\.listen\?\.\(\s*"([^"]+)"/g)].map(
    (m) => m[1],
  );
  check("native backend 真的有在送事件", emitted.length > 0, `${emitted.length} 個`);
  check("renderer 真的有在聽事件", heard.length > 0, `${heard.length} 個`);
  for (const name of new Set(emitted)) {
    check(`送出去的 ${name} 有人在聽`, heard.includes(name), heard.join("、"));
  }
  for (const name of new Set(heard)) {
    check(`聽的 ${name} 真的有人送`, emitted.includes(name), emitted.join("、"));
  }
}

console.log("㉜ URL 題寫入中撞上五秒輪詢：同一格不可以同時冒出兩張卡");
{
  let finishWrite;
  const writing = new Promise((resolve) => { finishWrite = resolve; });
  let gateChecks = 0;
  const p = await open({
    recording_state: "recording",
    url_policy_read: urlPolicy(),
    url_policy_write: () => writing,
    gatekeeper_check: () => gatekeeper(gateChecks++ === 0 ? null : gateCard()),
    ask: answer(),
  });
  check("前提：還沒回答時兩個答案都在", p.urlPolicy().hidden === false, p.urlPolicyQuestion());
  await p.chooseUrlPolicy(1);
  check("開始存之後 URL 題仍在", p.urlPolicy().hidden === false, p.urlPolicy().hidden);
  await tick(5100);
  check("輪詢來了一張 gatekeeper 卡，URL 寫入仍優先", p.urlPolicy().hidden === false, p.urlPolicy().hidden);
  check("gatekeeper 卡沒有疊在同一格", p.utterance().hidden === true, p.utterance().textContent);

  finishWrite("記下了：可以，但你要說得出它從哪來。");
  await tick();
  check("寫成後顯示的是確實存好的回條", p.urlPolicyResult().textContent.includes("存到"), p.urlPolicyResult().textContent);
  check("回條期間 gatekeeper 仍讓開", p.utterance().hidden === true, p.utterance().textContent);

  await p.type("剛剛發生什麼事");
  check("使用者自己的答案優先於剛存好的回條", p.urlPolicy().hidden === true, p.urlPolicy().hidden);
  check("也不把等著的 gatekeeper 疊回來", p.utterance().hidden === true, p.utterance().textContent);
}

console.log("㉝ URL 設定讀不回來不是『沒答過』，錯誤要看得見也聽得見");
{
  const LONG = `config parse failed: ${"x".repeat(400)}`;
  const p = await open({
    recording_state: "recording",
    gatekeeper_check: gatekeeper(),
    url_policy_read: new Error(LONG),
  });
  check("沒有替他選、也沒有把錯誤藏起來", p.urlPolicyResult().textContent.includes(LONG), p.urlPolicyResult().textContent);
  check("錯誤是 alert，不只是一塊看得到的顏色", p.urlPolicyResult().dataset.role === "alert", p.urlPolicyResult().dataset);
  check("仍有重試出口", p.urlPolicyActions().children.length === 1, p.urlPolicyActions().children.length);
}

console.log("㉞ gatekeeper／action log 讀不到時，不准留舊卡或安靜吞掉");
{
  const p = await open({
    recording_state: "recording",
    gatekeeper_check: new Error("database is locked"),
    url_policy_read: urlPolicy({ answered: "only-on-my-press" }),
  });
  check("讀取錯誤出現在 action log 那格", p.handsLog().textContent.includes("database is locked"), p.handsLog().textContent);
  check("那格會被螢幕閱讀器宣布", p.handsLog().dataset.role === "alert", p.handsLog().dataset);
  check("沒有拿上一輪卡片冒充現在式", p.utterance().hidden === true, p.utterance().textContent);
}

console.log("㉟ 長網址與 action log 不可以橫向裁掉真正的目標");
{
  const css = read(join(UI, "styles.css"));
  for (const selector of [".asked-note", ".asked-result", ".utterance-text", ".utterance-result", ".hands-log"]) {
    const start = css.indexOf(selector);
    const end = css.indexOf("}", start);
    const rule = css.slice(start, end);
    check(`${selector} 允許沒有斷點的字換行`, start >= 0 && rule.includes("overflow-wrap: anywhere"), rule);
  }
}

console.log("㊱ recorder supervisor 冷啟動會讀，五秒輪詢也會重讀");
{
  const p = await open({
    recording_state: "none",
    recorder_supervisor_state: supervisor(),
  });
  const initialReads = p.calls.filter((cmd) => cmd === "recorder_supervisor_state").length;
  check("冷啟動已讀過 supervisor", initialReads >= 1, p.calls);
  await p.pollNow();
  const afterPoll = p.calls.filter((cmd) => cmd === "recorder_supervisor_state").length;
  check("下一輪 recording poll 會一起重讀", afterPoll > initialReads, p.calls);
}

console.log("㊲ Backoff 不可以借尚未過期的舊 heartbeat 說『在聽』");
{
  const message =
    "record 剛剛異常退出（exit code 7）。desktop 仍在；5 秒後做第 2/3 次自動重試。";
  const p = await open({
    // hard kill 後的舊 heartbeat 在安全期限內仍可能是 live；supervisor 已經看見
    // child 退出，這時不能讓那份舊拍蓋過來。
    recording_state: "recording",
    recorder_supervisor_state: supervisor("backoff", message, 2),
  });
  check("Backoff 原因完整可見", p.line().includes(message), p.line());
  check("沒有冒充仍在聽", !p.line().includes("在聽"), p.line());
  check("舊 heartbeat 仍佔位時不提供重試", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);

  const vacant = await open({
    recording_state: "none",
    recorder_supervisor_state: supervisor("backoff", message, 2),
  });
  check("確認不佔位後才顯示現在重試", vacant.node("[data-wake]").hidden === false, vacant.node("[data-wake]").hidden);
  check("空位時按鈕逐字是現在重試", vacant.node("[data-wake]").textContent === "現在重試", vacant.node("[data-wake]").textContent);
}

console.log("㊳ GaveUp 要把自動重試真的停了與唯一恢復出口一起端出來");
{
  const message =
    "record 連續失敗 4 次，已停止自動重試；從現在起發生的事她不會知道。按「再試一次」才會重新開始。";
  const p = await open({
    recording_state: "none",
    recorder_supervisor_state: supervisor("gave-up", message, 4),
  });
  check("GaveUp 原因可見", p.line().includes(message), p.line());
  check("沒有人錄時不說在聽", !p.line().includes("在聽"), p.line());
  check("恢復按鈕看得到", p.node("[data-wake]").hidden === false, p.node("[data-wake]").hidden);
  check("恢復按鈕逐字是再試一次", p.node("[data-wake]").textContent === "再試一次", p.node("[data-wake]").textContent);
}

console.log("㊴ supervisor 讀不出來是 Uncertain，不是假裝停止或繼續錄");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: new Error("worker channel closed"),
  });
  check("讀取失敗本身看得到", p.line().includes("worker channel closed"), p.line());
  check("不拿 heartbeat 冒充確定仍在聽", !p.line().includes("在聽"), p.line());
  check("畫面明講不能確認", p.line().includes("不能確認"), p.line());
}

console.log("㊵ supervisor event 不可以擦掉使用者剛按下去那一下的回條");
{
  const p = await open({
    recording_state: "none",
    recorder_supervisor_state: supervisor(),
    toggle_pause: new Error("暫停鍵這一下沒有作用"),
  });
  await p.click("#pause");
  check("先有按鍵回條", p.line().includes("這一下沒有作用"), p.line());
  await p.fromOutside(
    "recorder-supervisor-changed",
    supervisor("gave-up", "SUPERVISOR-GAVE-UP", 4),
  );
  check("event 已套用（按鈕立即換字）", p.node("[data-wake]").textContent === "再試一次", p.node("[data-wake]").textContent);
  check("但一行文字仍先讓給按鍵回條", p.line().includes("這一下沒有作用"), p.line());
  check("持續狀態沒有蓋掉 one-shot notice", !p.line().includes("SUPERVISOR-GAVE-UP"), p.line());
  await p.fromOutside("pause-changed", true);
  check("下一件事發生後才輪到 supervisor message", p.line().includes("SUPERVISOR-GAVE-UP"), p.line());
}

console.log("㊶ Running 只證明 child 還活著；沒有新鮮 heartbeat 時不能冒充已在錄");
{
  const waiting = await open({
    recording_state: "none",
    recorder_supervisor_state: supervisor("running"),
  });
  check(
    "沒有 heartbeat 時只講現在能驗證的事",
    waiting.line().includes("行程仍活著，但目前沒有可驗證的新鮮錄製心跳"),
    waiting.line(),
  );
  check("不假定這是第一拍", !waiting.line().includes("第一個"), waiting.line());
  check("沒有 heartbeat 時不說在聽", !waiting.line().includes("在聽"), waiting.line());
  check("child 已存在時開始鍵藏起來", waiting.node("[data-wake]").hidden === true, waiting.node("[data-wake]").hidden);

  const confirmed = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  check("Recording heartbeat 回來後才顯示在聽", confirmed.line().startsWith("在聽"), confirmed.line());

  const openingDb = await open({
    recording_state: "booting",
    recorder_supervisor_state: supervisor("running"),
  });
  check("Booting heartbeat 保留開資料庫的精確狀態", openingDb.line().includes("正在開資料庫"), openingDb.line());
}

console.log("㊷ 第一份 heartbeat 尚未量到時只說正在確認，也不提供可能撞車的開始鍵");
{
  const p = await open({
    recording_state: () => new Promise(() => {}),
    recorder_supervisor_state: supervisor(),
  });
  check("未量到不是在聽", !p.line().includes("在聽"), p.line());
  check("未量到時逐字說正在確認", p.line().includes("正在確認 recorder"), p.line());
  check("未量到時不開放開始", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);

  await p.fromOutside(
    "recorder-supervisor-changed",
    supervisor("uncertain", "SUPERVISOR-UNKNOWN"),
  );
  check("Uncertain 仍不開放開始", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);
}

console.log("㊸ heartbeat 先回來、supervisor 尚未量到時也不能短暫冒充在聽");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: () => new Promise(() => {}),
  });
  check("supervisor 未量到時不借 heartbeat 說在聽", !p.line().includes("在聽"), p.line());
  check("兩份證據未齊時逐字說正在確認", p.line().includes("正在確認 recorder"), p.line());
  check("supervisor 未量到時不開放開始", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);

  await p.fromOutside("recorder-supervisor-changed", supervisor("running"));
  check("event 補齊 supervisor 證據後才顯示在聽", p.line().startsWith("在聽"), p.line());
}

console.log("㊹ 問答不可以把尚未量到的 recorder 狀態翻成『沒有人在記錄』");
{
  const p = await open({
    recording_state: () => new Promise(() => {}),
    recorder_supervisor_state: () => new Promise(() => {}),
    ask: () => new Promise(() => {}),
  });
  await p.type("昨天我在做什麼");
  check("仍說得出她正在回答", p.line().includes("想一下"), p.line());
  check("括號逐字說證據還在確認", p.line().includes("正在確認 recorder"), p.line());
  check("沒量到不冒充量到沒人錄", !p.line().includes("沒有人在記錄"), p.line());
}

console.log("㊺ Uncertain 時問答也必須保留『不能確認』");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("uncertain", "SUPERVISOR-UNKNOWN"),
    ask: () => new Promise(() => {}),
  });
  await p.type("昨天我在做什麼");
  check("問答仍明講不能確認 recorder", p.line().includes("不能確認 recorder"), p.line());
  check("Uncertain 沒被降成確定沒人錄", !p.line().includes("沒有人在記錄"), p.line());
}

console.log("㊻ listener 註冊前遺失的 transition 會由註冊後補讀追回來");
{
  let heartbeat = "none";
  let supervisorView = supervisor();
  let finishRecorderListener = () => {};
  const recorderListenerHeld = new Promise((resolve) => {
    finishRecorderListener = resolve;
  });
  const p = await open(
    {
      recording_state: () => heartbeat,
      recorder_supervisor_state: () => supervisorView,
    },
    {
      beforeListenerRegistered: (name) =>
        name === "recorder-supervisor-changed" ? recorderListenerHeld : undefined,
    },
  );
  check("前提：開場 IPC 讀到舊的停止 snapshot", p.node("[data-wake]").hidden === false, p.line());
  const readsBeforeRegistration = p.calls.filter(
    (cmd) => cmd === "recorder_supervisor_state",
  ).length;

  // transition 落在開場 read 之後、listener 真正註冊之前，所以 event 沒有人收到。
  heartbeat = "recording";
  supervisorView = supervisor("running");
  finishRecorderListener();
  await tick();

  const readsAfterRegistration = p.calls.filter(
    (cmd) => cmd === "recorder_supervisor_state",
  ).length;
  check("listener ready 後確實補讀 supervisor", readsAfterRegistration > readsBeforeRegistration, p.calls);
  check("補讀後顯示真實的 recording", p.line().startsWith("在聽"), p.line());
  check("不把遺失事件前的開始鍵留著", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);
}

console.log("㊼ Cooling 不借 owned child 的舊 heartbeat，也不提供啟動出口");
{
  const p = await open({
    // child 已退出之後，上一拍仍可能在 heartbeat 的安全期限內。
    recording_state: "recording",
    recorder_supervisor_state: supervisor("cooling"),
  });
  check(
    "逐字說正在確認 owned recorder 的最後 heartbeat 證據",
    p.line().includes("owned recorder 已退出，正在確認最後 heartbeat 證據"),
    p.line(),
  );
  check("舊拍不能讓畫面說在聽", !p.line().includes("在聽"), p.line());
  check("Cooling 不顯示 Start／Retry", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);
}

console.log("㊽ External 的現在式完整交給 heartbeat，但不提供 retry");
{
  const recording = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("external"),
  });
  check("External Recording 才說在聽", recording.line().startsWith("在聽"), recording.line());
  check("External Recording 不顯示 retry", recording.node("[data-wake]").hidden === true, recording.node("[data-wake]").hidden);

  const booting = await open({
    recording_state: "booting",
    recorder_supervisor_state: supervisor("external"),
  });
  check("External Booting 照實說正在開資料庫", booting.line().includes("正在開資料庫"), booting.line());
  check("External Booting 不說在聽", !booting.line().includes("在聽"), booting.line());
  check("External Booting 不顯示 retry", booting.node("[data-wake]").hidden === true, booting.node("[data-wake]").hidden);

  const thinking = await open({
    recording_state: "thinking",
    recorder_supervisor_state: supervisor("external"),
  });
  check("External Thinking 照實說正在想最後一段", thinking.line().includes("想最後一段"), thinking.line());
  check("External Thinking 不說在聽", !thinking.line().includes("在聽"), thinking.line());
  check("External Thinking 不顯示 retry", thinking.node("[data-wake]").hidden === true, thinking.node("[data-wake]").hidden);

  const gone = await open({
    recording_state: "none",
    recorder_supervisor_state: supervisor("external"),
  });
  check("External 沒有新鮮拍時照實說沒在記錄", gone.line().startsWith("沒有人在記錄"), gone.line());
  check("等 supervisor 確認離場前仍不顯示 retry", gone.node("[data-wake]").hidden === true, gone.node("[data-wake]").hidden);
  check(
    "隱藏按鈕也不殘留 retry 文案",
    !["現在重試", "再試一次"].includes(gone.node("[data-wake]").textContent),
    gone.node("[data-wake]").textContent,
  );
}

console.log("㊾ Backoff／GaveUp 要等三種 heartbeat 佔位證據全退掉才顯示 retry");
{
  for (const phase of ["backoff", "gave-up"]) {
    for (const heartbeat of ["recording", "booting", "thinking"]) {
      const p = await open({
        recording_state: heartbeat,
        recorder_supervisor_state: supervisor(phase),
      });
      check(
        `${phase} + ${heartbeat} 不顯示 retry`,
        p.node("[data-wake]").hidden === true,
        p.node("[data-wake]").hidden,
      );
    }
  }
}

console.log("㊿ heartbeat 讀壞與外部停止等待都不能冒充空房");
{
  for (const phase of ["backoff", "gave-up"]) {
    const unreadable = await open({
      recording_state: "unreadable",
      recorder_supervisor_state: supervisor(phase),
    });
    check(
      `${phase} + unreadable 明講不能確認`,
      unreadable.line().includes("讀不懂 recording.beat") &&
        !unreadable.line().includes("沒有人在記錄"),
      unreadable.line(),
    );
    check(
      `${phase} + unreadable 不顯示 retry`,
      unreadable.node("[data-wake]").hidden === true,
      unreadable.node("[data-wake]").hidden,
    );
  }

  for (const heartbeat of ["recording", "none"]) {
    const stopping = await open({
      recording_state: heartbeat,
      recorder_supervisor_state: supervisor("stopping-external"),
    });
    check(
      `stopping-external + ${heartbeat} 不說已退出`,
      stopping.line().includes("已請外部 recorder 收工") &&
        !stopping.line().includes("recorder 已退出"),
      stopping.line(),
    );
    check(
      `stopping-external + ${heartbeat} 不提供 Start／Retry`,
      stopping.node("[data-wake]").hidden === true,
      stopping.node("[data-wake]").hidden,
    );
  }
}

console.log("50a. StopUndelivered 保留可重送 Stop，不能退化成一般 Uncertain／Start");
{
  for (const heartbeat of ["recording", "thinking", "none", "unreadable"]) {
    const message =
      "真人已要求停止，但停止要求沒有送達磁碟；目前 recorder 仍可能在跑。修好後可再按一次停止。";
    const p = await open({
      recording_state: heartbeat,
      recorder_supervisor_state: supervisor("stop-undelivered", message),
      stop_recording: Promise.resolve(null),
    });
    check(
      `${heartbeat} 保留未送達原文`,
      p.line().includes(message) && !p.line().includes("在聽"),
      p.line(),
    );
    check(
      `${heartbeat} 顯示重送停止按鈕`,
      p.node("[data-wake]").hidden === false &&
        p.node("[data-wake]").textContent === "再送一次停止要求",
      {
        hidden: p.node("[data-wake]").hidden,
        text: p.node("[data-wake]").textContent,
      },
    );
    await p.click("[data-wake]");
    check(
      `${heartbeat} 點擊只送 stop_recording`,
      p.calls.includes("stop_recording") && !p.calls.includes("start_recording"),
      p.calls,
    );
  }

  const overtakesWake = await open({
    recording_state: "none",
    start_recording: () => new Promise(() => {}),
  });
  await overtakesWake.click("[data-wake]");
  check("前提：renderer 還在等真人 Start 回條", overtakesWake.line().includes("正在把她叫起來"), overtakesWake.line());
  await overtakesWake.fromOutside(
    "recorder-supervisor-changed",
    supervisor("stop-undelivered", "STOP-DELIVERY-FAILED"),
  );
  check(
    "StopUndelivered event 取代本機 starting 猜測",
    overtakesWake.line().includes("停止要求尚未送達") &&
      !overtakesWake.line().includes("正在把她叫起來"),
    overtakesWake.line(),
  );
  check(
    "取代後仍只提供重送 Stop",
    overtakesWake.node("[data-wake]").hidden === false &&
      overtakesWake.node("[data-wake]").textContent === "再送一次停止要求",
    {
      hidden: overtakesWake.node("[data-wake]").hidden,
      text: overtakesWake.node("[data-wake]").textContent,
    },
  );
}

console.log("50b. 第一張同意書撤回先取消 automatic supervisor，再碰 durable I/O");
{
  const native = read(join(UI, "../src-tauri/src/main.rs"));
  const consentSet = native.slice(
    native.indexOf("fn consent_set("),
    native.indexOf("fn open_onboarding_window", native.indexOf("fn consent_set(")),
  );
  const cancelAt = consentSet.indexOf("cancel_automatic_for_consent_revoke");
  const mutateAt = consentSet.indexOf("sister_core::consent::mutate");
  check(
    "LocalRecording uncheck 的 typed cancellation 排在 consent mutate 前",
    consentSet.includes("Sheet::LocalRecording") && cancelAt >= 0 && mutateAt > cancelAt,
    { cancelAt, mutateAt },
  );
  const revokeWrites = consentSet.match(/request_consent_revoke\s*\(/g) ?? [];
  const prepareAt = consentSet.indexOf("prepare_consent_revoke_barrier_clear");
  const committedAt = consentSet.indexOf("committed.map_err");
  const clearAt = consentSet.indexOf("clear_after_commit");
  const barrierWrittenAt = consentSet.indexOf("revoke_barrier_written = true");
  check(
    "revoke 只在 consent save 前寫一代 barrier，沒有 post-relatch 蓋過較新 regrant",
    revokeWrites.length === 1 && consentSet.indexOf("request_consent_revoke") < committedAt,
    revokeWrites.length,
  );
  check(
    "local regrant 鎖內取票、commit 後才清，Superseded 明確回錯",
    prepareAt > mutateAt && clearAt > committedAt && consentSet.includes("ConsentRevokeBarrierClear::Superseded") &&
      /ConsentRevokeBarrierClear::Superseded\)[\s\S]*?return Err/.test(consentSet),
    { prepareAt, committedAt, clearAt },
  );
  check(
    "只有 revoke barrier API 完整成功後，transaction 錯誤才可以聲稱 barrier 已可靠留下",
    barrierWrittenAt > consentSet.indexOf("request_consent_revoke") &&
      barrierWrittenAt < committedAt &&
      /if revoke_barrier_written \{[\s\S]*?撤回 barrier 已留下[\s\S]*?else if revoking_recording \{[\s\S]*?barrier 寫入沒有完整成功[\s\S]*?無法證明停止條件已可靠落地/.test(
        consentSet,
      ),
    { barrierWrittenAt, committedAt },
  );
  check(
    "regrant 只清獨立 barrier，不碰人工 Stop marker",
    !consentSet.includes("clear_stop_intent"),
    consentSet.match(/clear_[a-z_]+/g),
  );
}

console.log("51. 叫醒途中讀壞 heartbeat，不可繼續用本機 starting 掩蓋");
{
  let heartbeat = "none";
  const p = await open({
    recording_state: () => heartbeat,
    // 把本機 wake 留在飛行中；真實重現是 child 剛 spawn，下一次讀檔回 unreadable。
    start_recording: () => new Promise(() => {}),
  });
  await p.click("[data-wake]");
  check("前提：本機正在等叫醒", p.line().includes("正在把她叫起來"), p.line());

  heartbeat = "unreadable";
  await p.pollNow();
  check("讀壞狀態逐字看得見", p.line().includes("讀不懂 recording.beat"), p.line());
  check("不再用 starting 假裝還有進度", !p.line().includes("正在把她叫起來"), p.line());
  check("讀不懂時不提供第二次啟動", p.node("[data-wake]").hidden === true, p.node("[data-wake]").hidden);
}

console.log("52. 無關的 one-shot notice 不可被 heartbeat transition 擦掉");
{
  let heartbeat = "none";
  const p = await open({
    recording_state: () => heartbeat,
    open_timeline: new Error("時間軸這一下開不起來"),
  });
  await p.click("#timeline");
  check("前提：時間軸回條已顯示", p.line().includes("時間軸這一下"), p.line());

  heartbeat = "recording";
  await p.pollNow();
  check("狀態已切到在聽", p.line().startsWith("在聽"), p.line());
  check("但無關回條仍在最高優先度", p.line().includes("時間軸這一下"), p.line());
}

console.log("53. 拔手失敗不是 recorder 回條，heartbeat transition 不能淘汰");
{
  let heartbeat = "booting";
  const p = await open({ recording_state: () => heartbeat });
  const failedHands = "拔手開關這一下沒有生效；她的手仍可能會動";
  await p.fromOutside("hands-pulled", failedHands);
  check("前提：拔手失敗回條已顯示", p.line().includes("拔手開關"), p.line());

  heartbeat = "recording";
  await p.pollNow();
  check("錄製狀態改變後拔手失敗仍在", p.line().includes("拔手開關"), p.line());

  // Windows tray handler 本機跑不到；直接釘住它送的 event 不再混回 recorder-failed。
  const main = read(join(UI, "../src-tauri/src/main.rs"));
  const from = main.indexOf('"hands-stop" | "hands-resume" => {');
  const to = main.indexOf('"record" => {', from);
  const trayHands = from >= 0 && to > from ? main.slice(from, to) : "";
  check("tray 拔手分支存在", trayHands !== "", [from, to]);
  check(
    "tray 拔手失敗送 hands-pulled",
    /\.emit\(\s*"hands-pulled"/.test(trayHands) && !/\.emit\(\s*"recorder-failed"/.test(trayHands),
    trayHands,
  );
}



const AZURE_OFF = {
  ...AZURE_READY,
  generation: 6,
  enabled: false,
  region: null,
  endpoint: null,
  credential: "missing",
  consented: false,
  consent_at: null,
  ready: false,
};

function azureCalls(page, command = "azure_tts_speak") {
  return page.invokes.filter(({ cmd }) => cmd === command);
}

console.log("A152 R2. 內容命中的卡片就是一份本機答案");
{
  const card = { card_id: 152, frame_id: 42, at: 1_757_299_200_000, activity: "退款申請已送出" };
  const thinking = { state: "thinking", provider: "Codex" };
  const p = await open({
    azure_tts_read: AZURE_READY,
    ask_local: answer({ readings: [card], brain: thinking }),
    ask: () => new Promise(() => {}),
  });
  await p.type("退款");
  check("A152 foundNothing：卡片快答不被當成 provisional", p.hits().querySelector(".brain-thinking") !== null, p.hitTexts());
  check("A152 卡片原句在模型完成前可見", p.hits().querySelector(".reading-card")?.querySelector(".hit-text")?.textContent === card.activity, p.hitTexts());
  check("A152 快答不送 Azure", azureCalls(p).length === 0, azureCalls(p));
  const evidence = p.hits().querySelector(".reading-evidence");
  check("A152 卡片有時間和看畫面按鈕", p.hits().querySelector(".reading-card")?.querySelector(".hit-source")?.textContent.includes("2025") && evidence?.textContent === "看當時的畫面", p.hitTexts());
  await p.clickElement(evidence, { trusted: false });
  check("A152 合成 click 不開圖", !p.invokes.some(c => c.cmd === "open_frame"), p.invokes);
  await p.clickElement(evidence);
  check("A152 trusted click 開原卡畫面", p.invokes.some(c => c.cmd === "open_frame" && c.arg?.frameId === 42), p.invokes);

  let calls = 0;
  const terminal = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("capture A152 body"),
    ask: () => ++calls === 1 ? answer({ query_id: 7, readings: [card], blind: blind() }) : Promise.reject(new Error("A152 second failed")),
  });
  await terminal.type("退款");
  check("A152 空結果牆：卡片終局沒有空結果或盲點行", terminal.hits().querySelectorAll(".hits-empty, .hits-why").length === 0, terminal.hitTexts());
  check("A152 忘記標記：只有卡片仍可標我本來已經忘了", terminal.hits().querySelector(".mark-toggle") !== null, terminal.hitTexts());
  check("A152 Azure 正文只有一次開場與卡片原句且只送一次", azureCalls(terminal).length === 1 && azureCalls(terminal)[0].arg?.text === "你問的這個，我那時候看到的是——\n退款申請已送出", azureCalls(terminal));
  await terminal.type("下一題");
  check("A152 showingAnswer：下一題失敗指出上一題卡片", terminal.hitTexts().some(t => t.includes("上一題的，先收起來了")), terminal.hitTexts());

  const source = { ref: "card:152", label: "我的判讀", frame_id: null };
  const grounded = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("capture A152 synthesis"),
    ask: answer({ readings: [{ ...card, frame_id: null }], synthesis: { sentences: [{ text: "你已經送出退款申請。", sources: [source] }] } }),
  });
  await grounded.type("退款");
  const row = grounded.hits().querySelector('[data-evidence-ref="card:152"]');
  let scrolled = 0;
  if (row) row.scrollIntoView = () => { scrolled++; };
  const sourceButton = grounded.hits().querySelector(".grounded-source");
  check("A152 sourceTarget：無圖內容卡仍有出處鍵及列", row !== null && sourceButton !== null, grounded.hitTexts());
  if (sourceButton) await grounded.clickElement(sourceButton);
  check("A152 sourceTarget：出處鍵真的捲到卡片", scrolled === 1, scrolled);
  check("A152 無圖不提供看画面按鈕", grounded.hits().querySelector(".reading-evidence") === null);
  check("A152 有成句時 Azure 不重念卡片", azureCalls(grounded).length === 1 && azureCalls(grounded)[0].arg?.text === "你已經送出退款申請。", azureCalls(grounded));
}


{
  // 這條守的是「升級那一步吃的是真的那幾個集合，而且排在 prepare 前面」。
  //
  // **尾逗號不可以寫死。** 原本的針要求 `&mut readings,`，而那個逗號是 rustfmt
  // 把呼叫折成多行時才加的：把同一個呼叫排成一行（語意一模一樣）就紅，訊息和
  // 「真的搬到 prepare 後面」那種紅**一字不差**。實測兩刀各紅一條、同一條。
  // 寫死下限會把「產品變了」和「儀器壞了」混成同一則診斷，所以逗號收成 `,?`：
  // 現在只剩順序那一種紅得出來，而訊息講的正是順序。
  const memory = read(join(UI, "../src-tauri/src/main.rs"));
  check("A153 native promotion uses actual collections before prepare",
    /present_time_readings\(\s*asked_chapters\.as_ref\(\),\s*&facts,\s*&hits,\s*&mut readings\s*,?\s*\)/u.test(memory) &&
    memory.indexOf("answer_readings::present_time_readings(") < memory.indexOf("sister_core::grounded_answer::prepare(question"));
}
console.log("A153 R1. 先開口與終局分開記帳");
for (const state of ["thinking", "not_configured"]) {
  for (const [name, data] of [
    ["hit", { hits: [hit()] }],
    ["card", { time_range: { from: 1757290000000, to: 1757300000000, said: "昨天下午" }, chapters: [], readings: [{ card_id: 153, at: 1757299200000, frame_id: null, activity: "那段時間在處理退款" }] }],
  ]) {
    const early = answer({ ...data, brain: { state, provider: null } });
    check(`A153 early ${state} ${name} fixture default null`, early.query_id === null);
    const p = await open({ ask_local: early, ask: () => new Promise(() => {}) });
    await p.type("昨天下午在做什麼");
    check(`A153 early ${state} ${name} no accounting`,
      !p.hitTexts().join("\n").includes("這一題沒進題庫") && p.hits().querySelector(".mark-toggle") === null, p.hitTexts());
    if (name === "card") check(`A153 time card ${state} visible without empty wall`,
      p.hits().querySelector(".reading-card")?.textContent.includes("那段時間在處理退款") &&
      !p.hitTexts().join("\n").includes("我好像不太知道你在說什麼耶") &&
      p.hits().querySelectorAll(".hits-empty, .hits-why").length === 0, p.hitTexts());
  }
}
{
  const p = await open({ ask: answer({ hits: [hit()] }) });
  await p.type("退款");
  check("A153 terminal null retains exact explanation", p.hitTexts().includes("（這一題沒進題庫，所以「我本來已經忘了」標不了：可能是設定裡「你問過她什麼」關著，也可能是設定檔讀不回來，還可能是剛剛寫不進資料庫。）"), p.hitTexts());
  const q = await open({ ask: answer({ query_id: 153, hits: [hit()] }), mark_query: true });
  await q.type("退款");
  await q.clickElement(q.hits().querySelector(".mark-toggle"));
  check("A153 terminal id mark works", q.invokes.some(c => c.cmd === "mark_query" && c.arg?.queryId === 153 && c.arg?.marked === true), q.invokes);
}

// 同一組背景 DTO；golden 由本輪動手前的 app.js 擷取。
for (const [name, extra] of [
  ["chapters", { time_range: { from: 1000, to: 2000, said: "昨天" }, chapters: [chapter()] }],
  ["hits", { time_range: { from: 1000, to: 2000, said: "昨天" }, chapters: [], hits: [hit()] }],
  ["no-range", {}],
]) {
  const p = await open({ ask: answer({ query_id: 7, ...extra,
    readings: [{ card_id: 153, frame_id: null, at: 1757299200000, activity: null }] }) });
  await p.type("昨天");
  check(`A153 ${name} background invisible`, p.hits().querySelector(".reading-card") === null, p.hitTexts());
  const actual = JSON.stringify(a152Dom(p.hits()));
  const expected = read(join(UI, `../../../scripts/fixtures/a153-${name}.json`)).trim();
  // 時鐘沒釘住的話底下那條會紅，而它的訊息是一整棵 DOM——讀起來像「產品變了」。
  // 這一條先講出真正的原因，它自己就是那個釘子還在不在的證據。
  check(`A153 ${name} 前提：時鐘釘在 UTC`,
    new Date(0).getHours() === 0 && new Date(0).getDate() === 1,
    `getHours=${new Date(0).getHours()} getDate=${new Date(0).getDate()}（golden 是 UTC 產的）`);
  check(`A153 ${name} DOM bytes unchanged`, actual === expected, actual);
}

// Golden snapshots captured from base 759aa12, before content-card rendering existed.
// Serialize the entire answer DOM, including classes, datasets, state and all descendants.
function a152Dom(node) {
  return { tag: node.tag, text: node._text, className: node.className,
    classes: [...node.classList._s].sort(), dataset: node.dataset, hidden: node.hidden,
    disabled: node.disabled, type: node.type, title: node.title,
    children: node.children.map(a152Dom) };
}
for (const state of ["thinking", "not_configured"]) {
  const background = { card_id: 152, frame_id: null, at: 1_757_299_200_000, activity: null };
  const value = answer({ readings: [background], brain: { state, provider: null }, blind: blind() });
  const p = await open(state === "thinking"
    ? { ask_local: value, ask: () => new Promise(() => {}) }
    : { ask: value });
  await p.type("不存在");
  const actual = JSON.stringify(a152Dom(p.hits()));
  const expected = read(join(UI, `../../../scripts/fixtures/a152-background-${state}.json`)).trim();
  check(`A152 反向 ${state}：只有時間背景時整份答案 DOM 與 759aa12 逐字相同`, actual === expected, actual);
}
{
  const memory = read(MAIN).split("fn answer_from_memory(")[1]?.split("fn ")[0] ?? "";
  check("A152 接線：answer_from_memory 呼叫內容查詢合併並交給 prepare", /match_answer_readings\(\s*db,\s*&retrieval_questions,\s*reading_rows,\s*readings_truncated,/u.test(memory) && memory.indexOf("match_answer_readings") < memory.indexOf("::prepare("), memory);
  check("A152 接線：題庫命中與盲點使用同一組 readings", memory.includes("hits: answer_hit_count(&facts, &hits, &readings)") && memory.includes("needs_answer_blind_spots(&facts, &hits, &readings, &brain)"), memory);
}

console.log("A153 R2. 空手理由按需展開，正文與朗讀仍只有一句");
{
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("capture R2 body"),
    ask: answer({ blind: blind({ chunks: 10, ever_recorded: true,
      excluded: [["excluded app: private", 2]], paused_episodes: 3 }) }),
  });
  await p.type("沒有命中的問題");
  const root = p.hits();
  const empty = root.querySelector(".hits-empty");
  const toggle = root.querySelector(".hits-why-toggle");
  const lines = root.querySelector(".hits-why-lines");
  const reasons = root.querySelectorAll(".hits-why");
  check("R2.1 主句直接可見且在收合容器外", empty?.parentNode === root && empty?.hidden === false && root.hidden === false && empty?.textContent === "我記得的東西裡沒有這件事。", p.hitTexts());
  check("R2.2 按鈕存在且文字正好為什麼？", toggle?.textContent === "為什麼？", toggle?.textContent);
  check("R2.2k 原生鍵盤按鈕可用", toggle?.tag === "button" && toggle?.type === "button" && toggle?.disabled === false && toggle.hidden === false && toggle.parentNode?.hidden === false, toggle?.tag);
  check("R2.3 理由在 hidden 容器內且 aria 收起", lines?.hidden === true && toggle?.dataset["aria-expanded"] === "false" && reasons.length === 2 && reasons.every((line) => line.parentNode === lines), lines?.hidden);
  check("R2.3text 理由逐字保留且順序不變", JSON.stringify(reasons.map((line) => line.textContent)) === JSON.stringify([
    "不過你的排除規則（和自動防線）擋掉過東西（excluded app: private 2 段）——在那裡面的我本來就不會知道。",
    "我也被暫停過 3 次，那幾段是空的。",
  ]), reasons.map((line) => line.textContent));
  await p.clickElement(toggle);
  check("R2.4a 點一下展開且 aria 同步", lines?.hidden === false && toggle?.dataset["aria-expanded"] === "true", lines?.hidden);
  await p.clickElement(toggle);
  check("R2.4b 再點一次收回且 aria 同步", lines?.hidden === true && toggle?.dataset["aria-expanded"] === "false", lines?.hidden);
  await p.clickElement(toggle, { trusted: false });
  check("R2.5a 合成 click 也能展開（shot 契約）", lines?.hidden === false && toggle?.dataset["aria-expanded"] === "true", lines?.hidden);
  await p.clickElement(toggle, { trusted: false });
  check("R2.5b 合成 click 也能收回", lines?.hidden === true && toggle?.dataset["aria-expanded"] === "false", lines?.hidden);
  const bodies = root.querySelectorAll("[data-azure-answer-body]");
  check("R2.7 DOM 正文只有 hits-empty，理由及祖先沒有正文標記", bodies.length === 1 && bodies[0] === empty && reasons.length === 2 && reasons.every((line) => !Object.hasOwn(line.dataset, "azureAnswerBody")), bodies.map((line) => line.className));
  check("R2.7send 展開收回後 Azure 仍只送主句一次", azureCalls(p).length === 1 && azureCalls(p)[0].arg?.text === "我記得的東西裡沒有這件事。", azureCalls(p));
  await p.clickElement(toggle);
  await p.type("再問一次");
  check("R2.reset 新答案重新收起", root.querySelector(".hits-why-lines")?.hidden === true && root.querySelector(".hits-why-toggle")?.dataset["aria-expanded"] === "false");
}
for (const [name, supplied] of [["null", null], ["空陣列", blind({ chunks: 10, ever_recorded: true })]]) {
  const p = await open({ ask: answer({ blind: supplied }) });
  await p.type("空理由");
  check(`R2.6 ${name} 整顆按鈕及容器不出現`, p.hits().querySelectorAll(".hits-why-toggle, .hits-why-lines, .hits-why-disclosure").length === 0, p.hitTexts());
}
{
  const p = await open({ ask_local: answer({ brain: { state: "thinking", provider: "codex" }, blind: blind() }), ask: () => new Promise(() => {}) });
  await p.type("先開口");
  check("R2.provisional 只留先開口句、不畫收合區", p.hits().querySelectorAll(".hits-why-toggle, .hits-why-lines, .hits-why-disclosure, .hits-why").length === 0 && p.hits().querySelector(".hits-empty")?.textContent === "我好像不太知道你在說什麼耶，我再認真想一下。", p.hitTexts());
}

// 這一條守的是**整支假瀏覽器看不到的那一格**：它沒有版面引擎，
// `hidden` 在這裡永遠等於「收起來了」。真的瀏覽器不是——UA 的 `[hidden]`
// 是一條優先權極低的 `display: none`，任何一條自訂的 `display` 都壓得過它。
// 這棟房子被咬過一次（`styles.css` 的 `.avatar[hidden]` 那一段就是收據），
// 而那一次的症狀正是「每一條斷言都說收起來了，畫面上攤開著」。
//
// 判準看的是 CSS 原始碼，不是行為，所以它先證明自己不是一支空工具：
// 剖析得到夠多規則、而且那根針真的打得到東西。
console.log("A153 R2. 收合靠的是原生 hidden，所以沒有人可以給它 display");
{
  const css = read(join(UI, "styles.css")).replace(/\/\*[\s\S]*?\*\//gu, "");
  const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/gu)].map((m) => ({
    sel: m[1].trim().replace(/\s+/gu, " "),
    body: m[2].replace(/\s+/gu, " "),
  }));
  const setsDisplay = (r) => /(^|[\s;])display\s*:/u.test(r.body);
  const displayRules = rules.filter(setsDisplay);
  check("R2.css 前提：真的剖析到規則", rules.length > 100, rules.length);
  check("R2.css 前提：display 這根針打得到東西", displayRules.length > 5, displayRules.length);
  const couldHitCollapsed = (r) =>
    r.sel.split(",").some((part) => {
      const last = part.trim().split(/[\s>+~]+/u).filter(Boolean).pop() ?? "";
      return last === "ul" || last === "*" || last.includes("hits-why-lines");
    });
  const offenders = displayRules.filter(couldHitCollapsed);
  check("R2.css 收起來的那份清單沒有被任何 display 規則壓過",
    offenders.length === 0, offenders.map((r) => `${r.sel} { ${r.body} }`));
}

console.log("54. Azure ready 時最新答案完成自動送一次；只有 trusted click 能手動重播");
{
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("Azure 測試拒絕"),
    ask: answer({ hits: [hit({ snippet: "AZURE_HIT_BODY" })] }),
    recording_state: "recording",
  });
  await p.type("讀這份答案");
  const button = p.azureButton();
  check("ready 狀態才長出 Azure 按鈕", button !== null, p.hitTexts());
  check(
    "回答完成自動恰好送一次正文與剛讀到的完整 gate snapshot",
    azureCalls(p).length === 1 &&
      azureCalls(p)[0].arg?.text === "AZURE_HIT_BODY" &&
      JSON.stringify(azureCalls(p)[0].arg?.expected) ===
        JSON.stringify({
          generation: AZURE_READY.generation,
          enabled: true,
          region: AZURE_READY.region,
          voice: AZURE_READY.voice,
          consentAt: AZURE_READY.consent_at,
          credentialPresent: true,
        }),
    azureCalls(p),
  );
  await p.clickElement(button, { trusted: false });
  check("script 合成 click 不會多送重播", azureCalls(p).length === 1, p.invokes);
  await p.clickElement(button);
  check(
    "自動失敗後真人 click 可以手動重播一次",
    azureCalls(p).length === 2 && azureCalls(p)[1].arg?.text === "AZURE_HIT_BODY",
    azureCalls(p),
  );
  check("Azure 失敗沒有自動改用本機聲音", p.audioPlays() === 0 && p.localSpeaks() === 0);
  check(
    "失敗回條直接給設定與重播出口",
    p.node("[data-persona-line]").textContent ===
      "Azure 朗讀失敗。請檢查語音設定後重播。",
    p.node("[data-persona-line]").textContent,
  );
}

console.log("55. Azure payload 是正文 allowlist：hit/chapter 進，所有提示、出處與控制不進");
{
  const answerWithEverything = answer({
    kind: "range",
    searched: "SEARCH_DIAGNOSTIC_MUST_STAY_LOCAL",
    query_id: 7007,
    hits: [
      hit({
        snippet: "HIT_BODY_MUST_LEAVE",
        app: "SOURCE_APP_MUST_STAY_LOCAL",
        title: "SOURCE_TITLE_MUST_STAY_LOCAL",
        url: "https://source-must-stay-local.invalid/private",
      }),
    ],
    truncated: true,
    time_range: {
      from: 1_755_000_000_000,
      to: 1_755_000_360_000,
      said: "DATE_SCOPE_MUST_STAY_LOCAL",
    },
    chapters: [chapter()],
    followup: "FOLLOWUP_MUST_STAY_LOCAL",
    closure_notice: "CLOSURE_MUST_STAY_LOCAL",
  });
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("stop after payload capture"),
    ask: answerWithEverything,
    recording_state: "recording",
  });
  await p.type("範圍題");
  const text = azureCalls(p)[0]?.arg?.text ?? "";
  check("hit 主句會送", text.includes("HIT_BODY_MUST_LEAVE"), text);
  check("chapter 的核心時間與主句會送", text.includes("5 分鐘") && text.includes("Azure 主句章節"), text);
  for (const localOnly of [
    "CLOSURE_MUST_STAY_LOCAL",
    "SEARCH_DIAGNOSTIC_MUST_STAY_LOCAL",
    "DATE_SCOPE_MUST_STAY_LOCAL",
    "FOLLOWUP_MUST_STAY_LOCAL",
    "SOURCE_APP_MUST_STAY_LOCAL",
    "SOURCE_TITLE_MUST_STAY_LOCAL",
    "Notion.exe",
    "source-must-stay-local.invalid",
    "7007",
    "這裡最多列 20 筆",
    "我本來已經忘了",
    "用本機聲音朗讀",
    "用 Azure 朗讀",
  ]) {
    check(`不送 ${localOnly}`, !text.includes(localOnly), text);
  }
}

console.log("56. Azure payload 的 fact 與 empty 只送各自主句，不送 source／blind 診斷");
{
  const facts = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("captured"),
    ask: answer({
      answers: [fact()],
      hits: [],
    }),
    recording_state: "recording",
  });
  await facts.type("電話");
  const factText = azureCalls(facts)[0]?.arg?.text ?? "";
  check(
    "fact 認知界線、值與原文會送",
    factText.includes("我最後看到的是") &&
      factText.includes("+886800080123") &&
      factText.includes("客服專線 0800-080-123"),
    factText,
  );
  check(
    "fact source 與控制仍留本機",
    !factText.includes("chrome.exe") &&
      !factText.includes("帳單查詢") &&
      !factText.includes("source.example.invalid") &&
      !factText.includes("我本來已經忘了"),
    factText,
  );

  const empty = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("captured"),
    ask: answer({
      query_id: 8008,
      hits: [],
      answers: [],
      truncated: true,
      blind: blind({
        ever_recorded: true,
        excluded: [["excluded app: BLIND_DIAGNOSTIC_MUST_STAY_LOCAL", 9]],
        paused_episodes: 3,
        paused_now: true,
        truncated: 4,
      }),
    }),
    recording_state: "recording",
  });
  await empty.type("沒有的事");
  const emptyText = azureCalls(empty)[0]?.arg?.text ?? "";
  check("empty 主句會送", emptyText === "我記得的東西裡沒有這件事。", emptyText);
  check(
    "blind／pause 診斷不會跟著送",
    !emptyText.includes("BLIND_DIAGNOSTIC_MUST_STAY_LOCAL") &&
      !emptyText.includes("暫停") &&
      !emptyText.includes("9 段") &&
      !emptyText.includes("這裡最多列 20 筆") &&
      !emptyText.includes("8008"),
    emptyText,
  );
}

console.log("56a. RAG 成句逐句帶本機出處，Azure 只收到成句正文");
{
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("captured"),
    ask: answer({
      query_id: 7007,
      answers: [fact({ frame_id: 42, chunk_id: 31 })],
      hits: [
        hit({
          chunk_id: 77,
          frame_id: 84,
          snippet: "RAW_OCR_MUST_STAY_LOCAL",
          app: "SOURCE_APP_MUST_STAY_LOCAL",
          title: "SOURCE_TITLE_MUST_STAY_LOCAL",
        }),
      ],
      synthesis: {
        sentences: [
          {
            text: "客服電話是 0800-080-123。",
            sources: [{ ref: "fact:9", label: "畫面 #42", frame_id: 42 }],
          },
          {
            text: "昨天的筆記也提到客服流程。",
            sources: [{ ref: "chunk:77", label: "畫面 #84", frame_id: 84 }],
          },
        ],
      },
    }),
    recording_state: "recording",
  });
  await p.type("客服電話和流程");
  const sentences = p.hits().querySelectorAll(".grounded-text");
  const sources = p.hits().querySelectorAll(".grounded-source");
  check(
    "兩句成句與兩顆本機來源都畫出來",
    sentences.map((node) => node.textContent).join("|") ===
      "客服電話是 0800-080-123。|昨天的筆記也提到客服流程。" &&
      sources.map((node) => node.textContent).join("|") === "畫面 #42|畫面 #84",
    p.hitTexts(),
  );
  const azureText = azureCalls(p)[0]?.arg?.text ?? "";
  check(
    "Azure 只收兩句成句正文",
    azureText === "客服電話是 0800-080-123。\n昨天的筆記也提到客服流程。",
    azureText,
  );
  for (const localOnly of [
    "RAW_OCR_MUST_STAY_LOCAL",
    "SOURCE_APP_MUST_STAY_LOCAL",
    "SOURCE_TITLE_MUST_STAY_LOCAL",
    "fact:9",
    "chunk:77",
    "畫面 #42",
    "畫面 #84",
  ]) {
    check(`RAG Azure payload 不送 ${localOnly}`, !azureText.includes(localOnly), azureText);
  }

  const frameCallsBefore = p.invokes.filter(({ cmd }) => cmd === "open_frame").length;
  await p.clickElement(sources[0], { trusted: false });
  check(
    "合成的 source click 不開圖也不記點擊",
    p.invokes.filter(({ cmd }) => cmd === "open_frame").length === frameCallsBefore &&
      p.invokes.filter(({ cmd }) => cmd === "log_click").length === 0,
    p.invokes,
  );
  await p.clickElement(sources[0]);
  const opened = p.invokes.filter(({ cmd }) => cmd === "open_frame").at(-1);
  const logged = p.invokes.filter(({ cmd }) => cmd === "log_click").at(-1);
  check(
    "真人按 fact source 開 exact frame 並記 exact query/chunk/rank",
    JSON.stringify(opened?.arg) === JSON.stringify({ frameId: 42 }) &&
      JSON.stringify(logged?.arg) ===
        JSON.stringify({ queryId: 7007, chunkId: 31, rank: 0 }),
    { opened, logged },
  );
}

console.log("56b. RAG IPC 來源對不上本機候選時整份拒絕");
{
  // 底下兩個壞夾具**都只有一句**，所以 `renderGrounded` 是在任何東西被畫進
  // `hitList` 之前就 throw 的——這一節證得了「壞的不畫」，證不了「整份」。
  // 「好的排在壞的前面」那一格在 56g，不要把它併回來。
  for (const synthesis of [
    {
      sentences: [
        {
          text: "不存在的來源。",
          sources: [{ ref: "chunk:999", label: "文字 #999", frame_id: null }],
        },
      ],
    },
    {
      sentences: [
        {
          text: "畫面編號被換掉。",
          sources: [{ ref: "fact:9", label: "畫面 #999", frame_id: 999 }],
        },
      ],
    },
  ]) {
    const p = await open({
      ask: answer({ answers: [fact()], synthesis }),
      recording_state: "recording",
    });
    await p.type("壞掉的合約");
    check(
      "不把來源不一致的內容畫成正常答案",
      p.hits().querySelectorAll(".grounded-text").length === 0 &&
        (p.line().includes("成句答案找不到") || p.line().includes("畫面來源不一致")),
      { line: p.line(), hits: p.hitTexts() },
    );
  }
}

console.log("56g. 前一句是好的、後一句壞掉：整份拒絕的意思是連那句好的也不留");
{
  // 56b 和 56ab 都寫著「整份拒絕」「不畫半份」，而**那個處境它們一次都沒造出來
  // 過**：56b 兩個壞夾具都只有一句，56ab 的第七句是被最前面那條長度檢查擋下來
  // 的——兩種都在任何東西被 append 之前就 throw 了。`renderGrounded` 是**邊畫邊
  // 檢查**的：第一句合格就 `hitList.append(li)`，第二句的 ref 對不上才 throw。
  // 所以「半份」這件事只有在「好的排在壞的前面」時才存在，而那正是沒人試過的
  // 那一格。見 `a-suite-that-varies-the-wrong-axis-is-one-test`。
  const GOOD = "第一句有真的出處，看起來完全正常。";
  const p = await open({
    azure_tts_read: AZURE_READY,
    ask: answer({
      // `answer()` 預設 `presentation_id: null`＝「這份沒有 native lease」，而
      // 產品 IPC 一定帶（見 `beginNativePresentation` 上的註解）。不帶的話底下
      // 那條「票還回去了」問的是一張從來沒發出來的票——紅得毫無意義。
      presentation_id: "56g-lease",
      answers: [fact({ frame_id: 42 })],
      synthesis: {
        sentences: [
          { text: GOOD, sources: [{ ref: "fact:9", label: "畫面 #42", frame_id: 42 }] },
          {
            text: "第二句引用了一個這一輪沒送過的來源。",
            sources: [{ ref: "chunk:999", label: "文字 #999", frame_id: null }],
          },
        ],
      },
    }),
    recording_state: "recording",
  });
  await p.type("把兩句一起講");

  check(
    "一句成句都沒有畫出來",
    p.hits().querySelectorAll(".grounded-text").length === 0,
    p.hitTexts(),
  );
  // 上面那條問的是 class，這一條問的是**那幾個字在不在畫面上**。分開兩條，因為
  // 「把 class 拿掉但字還留著」是一種修法，而使用者讀到的是字。
  check(
    "那句好的也不可以留在畫面上任何地方",
    !p.hitTexts().some((line) => line.includes(GOOD)),
    p.hitTexts(),
  );
  check(
    "說得出是哪一個來源對不上",
    p.line().includes("成句答案找不到") && p.line().includes("chunk:999"),
    p.line(),
  );
  check("而且明講這一題沒答成", p.hitTexts().some((line) => line.includes("沒答成")), p.hitTexts());
  // 拒絕掉的半份不可以出境。`answerTextForLocalSpeech` 取的是 `.grounded-text`，
  // 自動送 Azure 那條路走的是 `data-azure-answer-body`——半份留在 DOM 裡的話，
  // 這一句會在他還沒讀到錯誤訊息之前就已經送上雲端了。
  check("一個字都沒送去 Azure", azureCalls(p).length === 0, azureCalls(p).map(({ arg }) => arg?.text));
  check(
    "朗讀那兩顆鍵也不存在（沒有東西可以念）",
    p.hits().querySelectorAll(".answer-read").length === 0,
    p.hitTexts(),
  );
  // `renderHits` 是跑在 `commitNativePresentation` 的 callback 裡，而那一層只有
  // `try/finally` 沒有 `catch`。少了那個 `finally`，畫到一半炸掉就會把 native
  // 那張票永遠留在手上——畫面上看不出來，而她從此停不下來。
  check(
    "畫到一半炸掉，native 那張票仍然還回去了",
    p.playbackTrace().includes("end:56g-lease"),
    p.playbackTrace(),
  );
}

console.log("56ab. 有前因後果的六句仍是一份可驗來源的答案，第七句整份拒絕");
{
  const groundedSentence = (n) => ({
    text: `同一件事的第 ${n} 句。`,
    sources: [{ ref: "fact:9", label: "畫面 #42", frame_id: 42 }],
  });
  const six = await open({
    ask: answer({
      answers: [fact({ frame_id: 42 })],
      synthesis: { sentences: Array.from({ length: 6 }, (_, index) => groundedSentence(index + 1)) },
    }),
    recording_state: "recording",
  });
  await six.type("把前因後果講清楚");
  check(
    "六句與六份逐句來源完整畫出",
    six.hits().querySelectorAll(".grounded-text").length === 6 &&
      six.hits().querySelectorAll(".grounded-source").length === 6,
    six.hitTexts(),
  );

  const seven = await open({
    ask: answer({
      answers: [fact({ frame_id: 42 })],
      synthesis: { sentences: Array.from({ length: 7 }, (_, index) => groundedSentence(index + 1)) },
    }),
    recording_state: "recording",
  });
  await seven.type("不能塞成一篇文章");
  check(
    "第七句讓 renderer 拒絕整份，不畫半份",
    seven.hits().querySelectorAll(".grounded-text").length === 0 &&
      seven.line().includes("成句答案必須是 1 到 6 句"),
    { line: seven.line(), hits: seven.hitTexts() },
  );
}

console.log("56e. 一句判讀的出處說得出它是判讀");
{
  // 這一版她第一次講得出一句**螢幕上沒寫過**的話。那一句底下的出處鍵不可以
  // 印成「畫面 #42」——那顆鍵按下去確實會開第 42 張圖（她是看著它想的），但
  // 標籤講的是「這句話從哪來的」，而它是從她想的東西來的。
  const p = await open({
    ask: answer({
      query_id: 7007,
      answers: [fact({ frame_id: 42, chunk_id: 31 })],
      readings: [{ card_id: 5, frame_id: 42 }],
      synthesis: {
        sentences: [
          {
            text: "你早上在追一個天氣警報。",
            sources: [{ ref: "card:5", label: "我的判讀", frame_id: 42 }],
          },
        ],
      },
    }),
    recording_state: "recording",
  });
  await p.type("剛剛在幹嘛");
  const sources = p.hits().querySelectorAll(".grounded-source");
  check(
    "判讀那一句畫得出來",
    p
      .hits()
      .querySelectorAll(".grounded-text")
      .map((node) => node.textContent)
      .join("|") === "你早上在追一個天氣警報。",
    p.hitTexts(),
  );
  check("出處印的是「我的判讀」", sources[0]?.textContent === "我的判讀", p.hitTexts());
  check(
    "不是「畫面 #42」",
    !p.hitTexts().some((t) => t.includes("畫面 #42")),
    p.hitTexts(),
  );

  // 點得開那張圖（她是看著它想的），但**不記點擊**：判讀不是檢索排出來的
  // 一筆，沒有 chunk_id、也沒有名次。記一筆假的名次會污染題庫。
  await p.clickElement(sources[0]);
  const opened = p.invokes.filter(({ cmd }) => cmd === "open_frame").at(-1);
  check(
    "按下去開得到她看著想的那張畫面",
    JSON.stringify(opened?.arg) === JSON.stringify({ frameId: 42 }),
    opened,
  );
  check(
    "而且不記進題庫的點擊排名",
    p.invokes.filter(({ cmd }) => cmd === "log_click").length === 0,
    p.invokes.filter(({ cmd }) => cmd === "log_click"),
  );

  const azureText = azureCalls(p)[0]?.arg?.text ?? "";
  for (const localOnly of ["card:5", "我的判讀", "畫面 #42"]) {
    check(`判讀的 Azure payload 不送 ${localOnly}`, !azureText.includes(localOnly), azureText);
  }
}

console.log("56h. 沒有圖、又沒有自己那一列的判讀，不可以畫成一顆按不動的鍵");
{
  // 出處鍵按下去只有兩條路：有圖就開圖，沒圖就捲到底下自己那一列
  // （`data-evidence-ref`）。這份時間背景判讀兩條都沒有——`frames_with_image()` 會把只
  // 簽第一張同意書（只記字、不留圖）那個人的 frame_id 濾成 null，而那是
  // **每一筆**不是零星幾筆；這份時間背景 `Reading` 不帶文字，所以畫面上根本
  // 不存在一列可以捲過去。於是那顆鍵永遠什麼都不會發生，卻長得跟真的開得
  // 了圖的那幾顆一模一樣。`sourceLine()` 那一行早就寫著這條規則：「看起來
  // 能點但點了沒反應」比「看得出來不能點」差。
  //
  // 同一句話裡的 ★ 出處是對照組：它一樣沒有圖，但底下有自己那一列，所以
  // 它**還是**一顆鍵。修法不可以把沒有圖的出處一律拔成死字。
  //
  // 擋不住的兩條，各打過一刀 `want=綠`：
  //
  // 1. **樣式表**。這裡問得出那一格掛的是 `.no-frame` 不是 `.grounded-source`，
  //    問不出 `.no-frame` 在畫面上長什麼樣。實測往 `.hit-source .no-frame`
  //    加一行 `cursor: pointer`，這一節照樣全綠——死字又長得可以按了，而這
  //    道閘門不會知道。字的寬度和游標形狀是要用眼睛在真機上看的。
  // 2. **捲動那條路捲去哪**。★ 那一格「還是一顆鍵」只證明它還是鍵，沒證明
  //    按下去看得見反應：`scrollIntoView({ block: "nearest" })` 在那一列本來
  //    就整列露著的時候什麼都不做。實測把它改成 `center`／`auto`，這一節也
  //    全綠。那是另一個問題，這一輪不動它。
  const p = await open({
    ask: answer({
      query_id: 7008,
      answers: [fact({ frame_id: null })],
      readings: [{ card_id: 5, frame_id: null }],
      synthesis: {
        sentences: [
          {
            text: "你早上在追一個天氣警報。",
            sources: [
              { ref: "card:5", label: "我的判讀", frame_id: null },
              { ref: "fact:9", label: "事實 #9", frame_id: null },
            ],
          },
        ],
      },
    }),
    recording_state: "recording",
  });
  await p.type("剛剛在幹嘛");
  check(
    "那一句還是畫得出來（沒有圖不是拒收的理由）",
    p
      .hits()
      .querySelectorAll(".grounded-text")
      .map((node) => node.textContent)
      .join("|") === "你早上在追一個天氣警報。",
    p.hitTexts(),
  );
  // 一格一格問，而且每一條只問它自己那一件事：刀切在標籤上和刀切在樣式上
  // 要紅出不同的句子，否則「紅了」讀不出「哪裡錯了」。
  const line = p.hits().querySelector(".grounded-sources");
  const cells = line?.children ?? [];
  const cell = cells.find((node) => node.textContent === "我的判讀") ?? null;
  const classesOf = (node) => String(node?.className ?? "").split(/\s+/u);
  check(
    "「我的判讀」那四個字還在——那是這句話唯一說得出口的出處",
    cell !== null,
    cells.map((node) => node.textContent),
  );
  if (cell !== null) {
    check("而那一格不是一顆鍵", cell.tag !== "button", cell.tag);
    check(
      "也不沿用「可以按」那個樣式",
      !classesOf(cell).includes("grounded-source"),
      cell.className,
    );
    check(
      "用的是出處那一行講「沒有留下畫面」時的同一個樣式",
      classesOf(cell).includes("no-frame"),
      cell.className,
    );
  }
  const pressable = line?.querySelectorAll("button") ?? [];
  check(
    "★ 那一格一樣沒有圖，可是還是一顆鍵（它捲得到底下自己那一列）",
    pressable.some((node) => node.textContent === "事實 #9"),
    pressable.map((node) => node.textContent),
  );
  // 上面那條的前提：★ 按下去捲得到的那一列真的存在。少了這一條，「還是一
  // 顆鍵」守的可能是另一顆一樣按不動的鍵。
  const refs = p
    .hits()
    .querySelectorAll("li")
    .map((node) => node.dataset.evidenceRef)
    .filter(Boolean);
  check("而底下那一列真的掛著 fact:9", refs.includes("fact:9"), refs);
  check("**時間背景**判讀沒有自己那一列（所以上面那格才無處可去）", !refs.includes("card:5"), refs);
}

console.log("56h-src. 「底下有自己那一列」這句話，兩邊要對得起來");
{
  // 上面那一節守的是今天的行為，這一節守的是它的前提不會被單邊改掉：哪天
  // 有人給判讀也掛一列 `data-evidence-ref`，`row: false` 就變成假話，那一格
  // 會永遠是死字；反過來拿掉原文那一列，`row: true` 也會變成一顆真的按不動
  // 的鍵。兩份名單各自從產品的不同地方讀出來，誰先動都會被抓到。
  const src = read(join(UI, "app.js"));
  const head = src.indexOf("const sourceTarget = (reference) =>");
  const target = src.slice(head, src.indexOf("for (const sentence of synthesis.sentences)", head));
  const arms = [...target.matchAll(/reference\.startsWith\("(\w+):"\)/g)];
  const withRow = new Set();
  for (const [i, arm] of arms.entries()) {
    const end = arms[i + 1]?.index ?? target.length;
    if (target.slice(arm.index, end).includes("row: true")) withRow.add(arm[1]);
  }
  const rows = new Set([...src.matchAll(/dataset\.evidenceRef = `(\w+):/g)].map((m) => m[1]));
  // 活體：兩支正規表示式挑不到東西的話，底下那條比較永遠是「空 === 空」。
  check(`sourceTarget 三種出處都掃到了（${arms.length}）`, arms.length === 3, arms.map((m) => m[1]));
  check(`data-evidence-ref 真的掃到了（${rows.size} 種）`, rows.size >= 2, [...rows]);
  check(
    "說得出「底下有自己那一列」的那幾種，就是真的掛著 data-evidence-ref 的那幾種",
    [...withRow].sort().join(",") === [...rows].sort().join(","),
    { row: [...withRow].sort(), evidenceRef: [...rows].sort() },
  );
}

console.log("56f. 引用一張這一題沒送出去的判讀，整份拒絕");
{
  const p = await open({
    ask: answer({
      answers: [fact()],
      readings: [{ card_id: 5, frame_id: 42 }],
      synthesis: {
        sentences: [
          {
            text: "我覺得你在忙別的。",
            sources: [{ ref: "card:6", label: "我的判讀", frame_id: null }],
          },
        ],
      },
    }),
    recording_state: "recording",
  });
  await p.type("剛剛在幹嘛");
  check(
    "沒送出去的判讀不可以被引用",
    p.hits().querySelectorAll(".grounded-text").length === 0 &&
      p.line().includes("成句答案找不到"),
    { line: p.line(), hits: p.hitTexts() },
  );
}

console.log("56c. 畫面說出實際接手的 CLI，而且零命中也不能假裝沒交給它");
{
  const used = await open({
    ask: answer({
      brain: { state: "used", provider: "Grok CLI" },
      hits: [hit()],
    }),
    recording_state: "recording",
  });
  await used.type("昨天做了什麼");
  check(
    "成功時說出 Grok CLI 已使用本機記憶",
    used.hits().querySelector(".brain-note")?.textContent === "Grok CLI · 已使用本機記憶",
    used.hitTexts(),
  );

  const empty = await open({
    ask: answer({
      brain: { state: "no_sources", provider: "Codex CLI" },
      blind: blind({ ever_recorded: true, chunks: 12 }),
    }),
    recording_state: "recording",
  });
  await empty.type("沒有命中的題目");
  check(
    "零命中仍明講 Codex 已處理並查過",
    empty.hits().querySelector(".brain-note")?.textContent ===
      "Codex CLI 已查過本機記憶；目前沒有可引用的內容。",
    empty.hitTexts(),
  );
}

console.log("56d. 答題路由每題重讀最後選用的 CLI，沒有 Grok 專用分支");
{
  const main = read(MAIN);
  const selection = main.slice(
    main.indexOf("fn answer_cli_from_config("),
    main.indexOf("#[cfg(test)]\nmod answer_cli_selection_tests"),
  );
  const routing = main.slice(
    main.indexOf("fn plan_answer_searches("),
    main.indexOf("/// 時間軸上的一天。", main.indexOf("fn plan_answer_searches(")),
  );
  check(
    "每題從 config.brain.cli 取得目前選擇",
    selection.includes("config.brain.cli()") &&
      selection.includes("let config = sister_core::config::Config::load(&path)") &&
      routing.includes("plan_answer_searches(&shell, &question"),
    { selection, routing },
  );
  check(
    "同一份已選 CLI 同時規劃查詢與完成回答",
    routing.includes("Some(PlannedAnswerSearches { cli, queries })") &&
      routing.includes("&planned.cli"),
    routing,
  );
  check(
    "回答路徑沒有針對 Grok、Claude、Codex 或 Gemini 寫分支",
    !routing.includes("BrainProvider::Grok") &&
      !routing.includes("BrainProvider::Claude") &&
      !routing.includes("BrainProvider::Codex") &&
      !routing.includes("BrainProvider::Gemini"),
    routing,
  );
}

console.log("57. 新題會等舊自動朗讀 cancel settle；晚 response 不播，然後只送最新題");
{
  let finishA = null;
  let finishCancel = null;
  let speakNumber = 0;
  let azureStatus = AZURE_READY;
  const p = await open({
    azure_tts_read: () => azureStatus,
    azure_tts_speak: () => {
      speakNumber += 1;
      if (speakNumber > 1) throw new Error("new answer captured");
      return new Promise((resolveSpeak) => {
        finishA = resolveSpeak;
      });
    },
    azure_tts_cancel: () =>
      new Promise((resolveCancel) => {
        finishCancel = () => {
          // A 已 admission 會先把 7 消耗成 8；cancel A 再推成 9。只有 cancel
          // settle 後的 authoritative read 可以把 9 交給 B。
          azureStatus = { ...AZURE_READY, generation: 9 };
          resolveCancel(true);
        };
      }),
    ask: ({ question }) =>
      answer({
        hits: [
          hit({
            snippet:
              question === "第二題" ? "LATEST_AFTER_CANCEL_BODY" : "PENDING_AZURE_BODY",
          }),
        ],
      }),
    recording_state: "recording",
  });
  await p.type("第一題");
  const button = p.azureButton();
  check("前提：第一題的自動 speak 在飛", azureCalls(p).length === 1 && typeof finishA === "function");
  check("Azure POST pending 還不算 speaking", !p.isSpeaking());
  await p.clickElement(button);
  check("按著正在播的按鈕是 cancel，不是第二個 speak", azureCalls(p).length === 1, azureCalls(p));
  check(
    "cancel IPC 恰好一次且只取消這次 read 綁定的 generation",
    azureCalls(p, "azure_tts_cancel").length === 1 &&
      JSON.stringify(azureCalls(p, "azure_tts_cancel")[0].arg) ===
        JSON.stringify({ expectedGeneration: AZURE_READY.generation }),
    azureCalls(p, "azure_tts_cancel"),
  );
  check("前提：cancel IPC 被故意卡住", typeof finishCancel === "function");
  // 第二題可能快到 cancel IPC 都還沒回。她不能重用 A token，也不能
  // 就此吃掉已完成的最新答案；只能等 authoritative read 回來後送 B。
  await p.type("第二題");
  check("cancel native 尚未回來時，新答案不重用 A token", azureCalls(p).length === 1, azureCalls(p));
  finishA({
    generation: 8,
    content_type: "audio/mpeg",
    audio_bytes: 3,
    data_url: "data:audio/mpeg;base64,AQID",
    presentation_id: "5701",
  });
  await tick(40);
  check("取消後的晚 MP3 不播放", p.audioPlays() === 0, p.audioPlays());
  check("取消不 fallback 到本機", p.localSpeaks() === 0, p.localSpeaks());
  finishCancel(true);
  await tick(40);
  check(
    "cancel settle 並重讀後，自動用新 generation 只送第二題",
    azureCalls(p).length === 2 &&
      azureCalls(p)[1].arg?.expected?.generation === 9 &&
      azureCalls(p)[1].arg?.text === "LATEST_AFTER_CANCEL_BODY",
    azureCalls(p),
  );
}

console.log("58. 合法 MP3 會自動播、結束後可 trusted replay；malformed response 不 fallback");
{
  const ok = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: ({ expected }) => ({
      generation:
        expected.generation >= Number.MAX_SAFE_INTEGER ? 0 : expected.generation + 1,
      content_type: "audio/mpeg",
      audio_bytes: 3,
      data_url: "data:audio/mpeg;base64,AQID",
      presentation_id: `580${expected.generation}`,
    }),
    ask: answer({ hits: [hit({ snippet: "PLAY_ME" })] }),
    recording_state: "recording",
  });
  await ok.type("播放");
  check("最新答案的合法 MP3 自動播一次", ok.audioPlays() === 1, ok.audioPlays());
  check("Azure MP3 真正開始播放才進 speaking", ok.isSpeaking());
  check("自動朗讀只送一個 speak", azureCalls(ok).length === 1, azureCalls(ok));
  const azurePresentationEnds = () =>
    ok.invokes.filter(
      ({ cmd, arg }) =>
        cmd === "master_stop_presentation_end" &&
        typeof arg?.presentationId === "string" &&
        arg.presentationId.startsWith("580"),
    );
  check(
    "播放還活著時 native lease 尚未 end",
    azurePresentationEnds().length === 0,
    azurePresentationEnds(),
  );
  ok.finishAudio();
  check(
    "Azure ended 清掉 speaking 並 end playback lease",
    !ok.isSpeaking() && azurePresentationEnds().length === 1,
    azurePresentationEnds(),
  );
  await ok.clickElement(ok.azureButton(), { trusted: false });
  check("script 合成 replay 不送", azureCalls(ok).length === 1, azureCalls(ok));
  await ok.clickElement(ok.azureButton());
  check(
    "真人手動 replay 用下一代 token 再送、再播一次",
    azureCalls(ok).length === 2 &&
      azureCalls(ok)[1].arg?.expected?.generation === 8 &&
      ok.audioPlays() === 2,
    { calls: azureCalls(ok), plays: ok.audioPlays() },
  );
  check("Azure trusted replay 播放時回到 speaking", ok.isSpeaking());
  check("第二次播放中 lease 仍活著", azurePresentationEnds().length === 1);
  ok.failAudio();
  check(
    "Azure playback error 清掉 speaking 並 end lease",
    !ok.isSpeaking() && azurePresentationEnds().length === 2,
    azurePresentationEnds(),
  );
  await ok.clickElement(ok.azureButton());
  check("前提：再一次 trusted replay 已開始", azureCalls(ok).length === 3 && ok.isSpeaking());
  await ok.clickElement(ok.azureButton());
  check(
    "Azure 停止鍵先停本機 media、end lease，且不另送 speak",
    azureCalls(ok).length === 3 &&
      !ok.isSpeaking() &&
      azurePresentationEnds().length === 3 &&
      ok.audioPauses() > 0,
    { calls: azureCalls(ok), ends: azurePresentationEnds(), pauses: ok.audioPauses() },
  );

  const bad = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: {
      generation: 8,
      content_type: "audio/wav",
      audio_bytes: 3,
      data_url: "data:audio/wav;base64,AQID",
      presentation_id: "5808",
    },
    ask: answer({ hits: [hit({ snippet: "DO_NOT_PLAY" })] }),
    recording_state: "recording",
  });
  await bad.type("別播放壞回應");
  check("malformed audio 不播放", bad.audioPlays() === 0, bad.audioPlays());
  check("malformed audio 不改用本機", bad.localSpeaks() === 0, bad.localSpeaks());

  const staleGeneration = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: {
      generation: AZURE_READY.generation,
      content_type: "audio/mpeg",
      audio_bytes: 3,
      data_url: "data:audio/mpeg;base64,AQID",
      presentation_id: "5807",
    },
    ask: answer({ hits: [hit({ snippet: "STALE_GENERATION_MUST_NOT_PLAY" })] }),
    recording_state: "recording",
  });
  await staleGeneration.type("別播放舊代回應");
  check("不是 baseline 精確 +1 的 MP3 也不播放", staleGeneration.audioPlays() === 0);
  check("generation mismatch 仍不 fallback", staleGeneration.localSpeaks() === 0);
}

console.log("59. changed event 的新狀態不會被較早開始、較晚回來的 read 蓋掉");
{
  let finishOldRead = null;
  let reads = 0;
  const p = await open({
    azure_tts_read: () => {
      reads += 1;
      // 開場 read 與 listener-ready 補讀先一致回 off；接著故意讓第一個 event
      // 卡住，再讓第二個 event 的新 generation 先回來。
      if (reads === 3) {
        return new Promise((resolveRead) => {
          finishOldRead = () => resolveRead(AZURE_OFF);
        });
      }
      return reads >= 4 ? AZURE_READY : AZURE_OFF;
    },
    ask: answer({ hits: [hit({ snippet: "READ_REVISION_BODY" })] }),
    recording_state: "recording",
  });
  await p.type("狀態更新");
  check("前提：開場與 listener-ready 補讀都回 off，答案沒有 Azure 按鈕", reads === 2 && p.azureButton() === null, {
    reads,
    button: p.azureButton()?.textContent ?? null,
  });
  await p.fromOutside("azure-tts-changed");
  check("前提：第一個 changed read 還在飛", reads === 3 && typeof finishOldRead === "function", reads);
  await p.fromOutside("azure-tts-changed");
  check("第二個 event 只觸發一次新 read，最新 ready 長出按鈕", reads === 4 && p.azureButton() !== null, {
    reads,
    button: p.azureButton()?.textContent ?? null,
  });
  finishOldRead();
  await tick(40);
  check("舊的 off 回應晚到仍不能拿掉新按鈕", p.azureButton() !== null, p.hitTexts());
  await p.repaint();
  await p.pollNow();
  check("純 read、event、repaint 與 poll 都不會補送舊答案", azureCalls(p).length === 0 && p.audioPlays() === 0);
}

console.log("60. listener 註冊前遺失的 Azure change 由 ready 後補讀追回來");
{
  let azure = AZURE_OFF;
  let reads = 0;
  let finishAzureListener = () => {};
  const azureListenerHeld = new Promise((resolveListener) => {
    finishAzureListener = resolveListener;
  });
  const p = await open(
    {
      azure_tts_read: () => {
        reads += 1;
        return azure;
      },
      ask: answer({ hits: [hit({ snippet: "LISTENER_READY_BODY" })] }),
      recording_state: "recording",
    },
    {
      beforeListenerRegistered: (name) =>
        name === "azure-tts-changed" ? azureListenerHeld : undefined,
    },
  );
  await p.type("listener gap");
  check("前提：listener 未完成時只有開場舊 read", reads === 1 && p.azureButton() === null, {
    reads,
    button: p.azureButton()?.textContent ?? null,
  });
  // 這次 change 落在 read 和 listen ready 之間，沒有 event callback 可呼叫。
  azure = AZURE_READY;
  finishAzureListener();
  await tick(40);
  check("listener ready 後恰好多一次 native read", reads === 2, reads);
  check("補讀追回新 generation/ready，答案按鈕出現", p.azureButton() !== null, p.hitTexts());
  check("補讀本身沒有 speak 或 autoplay", azureCalls(p).length === 0 && p.audioPlays() === 0);
}

console.log("60a. listener gap + 未知 status 不能把重簽後的新授權借給舊答案");
{
  let finishInitialStatus = null;
  const heldInitialStatus = new Promise((resolveStatus) => {
    finishInitialStatus = resolveStatus;
  });
  let finishAzureListener = () => {};
  const azureListenerHeld = new Promise((resolveListener) => {
    finishAzureListener = resolveListener;
  });
  let reads = 0;
  let azure = AZURE_OFF;
  const p = await open(
    {
      azure_tts_read: () => {
        reads += 1;
        return reads === 1 ? heldInitialStatus : azure;
      },
      azure_tts_speak: new Error("only a post-grant new answer may leave"),
      ask: ({ question }) =>
        answer({
          hits: [
            hit({
              snippet:
                question === "重簽後的新題"
                  ? "NEW_AFTER_LISTENER_GAP_GRANT"
                  : "OLD_BEFORE_LISTENER_GAP_GRANT",
            }),
          ],
        }),
      recording_state: "recording",
    },
    {
      beforeListenerRegistered: (name) =>
        name === "azure-tts-changed" ? azureListenerHeld : undefined,
    },
  );
  await p.type("重簽前的舊題");
  check("前提：答案完成時 status 未知且 listener 尚未 ready", reads === 1 && azureCalls(p).length === 0, {
    reads,
    calls: azureCalls(p),
  });
  // 模擬設定啟用／重簽發生在 listener 空窗；event 遺失，只剩 ready 後補讀。
  azure = AZURE_READY;
  finishAzureListener();
  await tick(40);
  check(
    "listener-ready 補讀拿到新授權也不補送簽名前答案",
    reads === 2 && p.azureButton() !== null && azureCalls(p).length === 0,
    { reads, calls: azureCalls(p) },
  );
  finishInitialStatus(AZURE_OFF);
  await tick(40);
  check("較早的 initial read 晚回也不能復活舊題", azureCalls(p).length === 0, azureCalls(p));
  await p.type("重簽後的新題");
  check(
    "重簽後才完成的新答案可以自動送一次",
    azureCalls(p).length === 1 && azureCalls(p)[0].arg?.text === "NEW_AFTER_LISTENER_GAP_GRANT",
    azureCalls(p),
  );
}

console.log("61. 答案完成時 initial status 未知，不借稍後回來的授權；下一題才送");
{
  let finishStatus = null;
  const heldStatus = new Promise((resolveStatus) => {
    finishStatus = resolveStatus;
  });
  const p = await open({
    azure_tts_read: () => heldStatus,
    azure_tts_speak: new Error("post-status new answer captured"),
    ask: ({ question }) =>
      answer({
        hits: [
          hit({
            snippet:
              question === "狀態確認後的新題"
                ? "NEW_AFTER_STATUS_BODY"
                : "UNKNOWN_STATUS_OLD_BODY",
          }),
        ],
      }),
    recording_state: "recording",
  });
  await p.type("開窗立刻問");
  check("status 未知時不用假的預設值送", azureCalls(p).length === 0, p.invokes);
  finishStatus(AZURE_READY);
  await tick(40);
  check(
    "authoritative ready 稍後回來也不補送 status 未知時完成的舊題",
    azureCalls(p).length === 0,
    azureCalls(p),
  );
  await p.type("狀態確認後的新題");
  check(
    "狀態已知 ready 後完成的下一題才自動送",
    azureCalls(p).length === 1 && azureCalls(p)[0].arg?.text === "NEW_AFTER_STATUS_BODY",
    azureCalls(p),
  );
}

console.log("61a. initial status 未回時收到 Azure mutation event，舊答案失效；下一題才自動送");
{
  let finishInitialStatus = null;
  const heldInitialStatus = new Promise((resolveStatus) => {
    finishInitialStatus = resolveStatus;
  });
  let reads = 0;
  const p = await open({
    azure_tts_read: () => {
      reads += 1;
      // 開場 read 和 listener-ready 補讀都卡住；mutation event 後的
      // 第三次 read 才是新的 authoritative ready。
      return reads <= 2 ? heldInitialStatus : AZURE_READY;
    },
    azure_tts_speak: new Error("post-mutation new answer captured"),
    ask: ({ question }) =>
      answer({
        hits: [
          hit({
            snippet:
              question === "mutation 後的新題"
                ? "NEW_AFTER_MUTATION_BODY"
                : "OLD_PENDING_BEFORE_MUTATION",
          }),
        ],
      }),
    recording_state: "recording",
  });
  check("前提：兩份 initial status read 都還在飛", reads === 2, reads);
  await p.type("mutation 前的舊題");
  check("舊題完成時 status 未知，還沒有送", azureCalls(p).length === 0, p.invokes);

  await p.fromOutside("azure-tts-changed");
  check("變更事件只重讀新狀態、長出按鈕，不補送舊題", reads === 3 && p.azureButton() !== null && azureCalls(p).length === 0, {
    reads,
    calls: azureCalls(p),
    button: p.azureButton()?.textContent ?? null,
  });
  finishInitialStatus(AZURE_READY);
  await tick(40);
  check("變更前的舊 read 晚回 ready 也不能復活舊題", azureCalls(p).length === 0, azureCalls(p));

  await p.type("mutation 後的新題");
  check(
    "變更後新完成的題才自動送一次",
    azureCalls(p).length === 1 && azureCalls(p)[0].arg?.text === "NEW_AFTER_MUTATION_BODY",
    azureCalls(p),
  );
}

console.log("62. 兩題亂序回來時，過期答案不能說話，只最新題自動送一次");
{
  let finishFirst = null;
  let finishSecond = null;
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("latest answer captured"),
    ask: ({ question }) =>
      new Promise((resolveAsk) => {
        if (question === "先問的") finishFirst = resolveAsk;
        else finishSecond = resolveAsk;
      }),
    recording_state: "recording",
  });
  await p.type("先問的");
  await p.type("後問的");
  check("前提：兩題都被故意卡住", typeof finishFirst === "function" && typeof finishSecond === "function");
  finishSecond(answer({ hits: [hit({ snippet: "LATEST_ANSWER_BODY" })] }));
  await tick(40);
  check(
    "後問的最新題先回來，自動送一次",
    azureCalls(p).length === 1 && azureCalls(p)[0].arg?.text === "LATEST_ANSWER_BODY",
    azureCalls(p),
  );
  finishFirst(answer({ hits: [hit({ snippet: "STALE_ANSWER_MUST_NOT_LEAVE" })] }));
  await tick(40);
  check(
    "先問的舊題晚回來不會再送、也不會換掉最新答案",
    azureCalls(p).length === 1 &&
      !azureCalls(p).some(({ arg }) => arg?.text?.includes("STALE_ANSWER_MUST_NOT_LEAVE")) &&
      p.hitTexts().some((line) => line.includes("LATEST_ANSWER_BODY")),
    { calls: azureCalls(p), hits: p.hitTexts() },
  );
}

console.log("63. 純瀏覽器 screenshot demo 有答案外觀，但沒有 native Azure 能力");
{
  const p = await open(
    {},
    { search: "?hits=demo", browserOnly: true },
  );
  check("demo 確實畫出答案，不是拿空頁證明沒有送", !p.hits().hidden && p.hitTexts().length > 0, p.hitTexts());
  check(
    "browser-only demo 沒有 Azure IPC，也沒有 audio autoplay",
    azureCalls(p).length === 0 && p.audioPlays() === 0 && p.invokes.length === 0,
    p.invokes,
  );
}

// 走輸入框 → ask IPC → 正式 renderer，再讀實際掛進答案泡泡的 disclosure。
// open 是原生 details 的公開 DOM 屬性；瀏覽器的 summary 點擊另做真瀏覽器驗證。
function checkOverviewWhy(p, name, exact) {
  const details = p.hits().querySelectorAll("details").find(
    (node) => node.querySelector("p")?.textContent === exact,
  );
  check(`${name} 精確說明存在且預設收合`,
    !!details && !details.open && !details.hidden &&
      details.children[0]?.tag === "summary" &&
      details.children[0]?.textContent === "為什麼", p.hitTexts());
  if (details) details.open = true;
  check(`${name} 展開後保留完整原句`,
    details?.open === true && details.querySelector("p")?.textContent === exact,
    details?.textContent);
  if (details) details.open = false;
}

function checkOverviewVoice(p, name) {
  const voices = p.hits().querySelectorAll(".overview-voice");
  check(`${name} 收合時有人話且不是內部說明`,
    voices.length > 0 && voices.every((node) =>
      !node.hidden && node.textContent.trim().length > 0 &&
      !/OCR|理解卡|理解記憶|審閱層|模型|自報信心/.test(node.textContent)),
    voices.map((node) => node.textContent));
}

// 單獨執行正式函式，作者拒絕不能借 renderOverview 的呼叫順序成立。
const provenanceOf = runInNewContext(
  `(${read(SRC).match(/function overviewProvenance\(card\) \{[\s\S]*?\n\}/)[0]})`,
);
{
  let rejected = false;
  try { provenanceOf({ author: "future_author" }); }
  catch (error) { rejected = error.message === "不認得的記憶總覽作者：future_author"; }
  check("overviewProvenance 未知作者獨立呼叫仍 throw", rejected);
}
const provenanceCases = [
  ["interpreter", 0.31, "我的印象", "模型整理的假設 · 模型自報信心 0.31（不是量出來的）"],
  ["reviewer", 0.42, "重新想過的印象", "審閱層修訂 · 原模型自報信心 0.42（不是量出來的）"],
  ["user", 0.88, "你修正過的說法", "你修正過 · 不是她量出來的，也不是模型說的"],
];
for (const [author, model_confidence, voice, why] of provenanceCases) {
  const actual = provenanceOf({ author, model_confidence });
  check(`overviewProvenance ${author} 同時回傳短句與精確說明`,
    actual.voice === voice && actual.why === why, actual);
}

console.log("64. 記憶總覽只畫有證據的 L2 假設；證據要真人按才開");
{
  let asks = 0;
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("captured"),
    ask: () => {
      asks += 1;
      if (asks > 1) throw new Error("SECOND_QUERY_FAILED");
      return answer({
        kind: "memory_overview",
        query_id: 99123,
        searched: "SEARCHED_MUST_STAY_LOCAL",
        hits: [hit({ snippet: "OCR_MUST_STAY_LOCAL" })],
        answers: [fact({ value: "FACT_MUST_STAY_LOCAL" })],
        followup: "FOLLOWUP_MUST_STAY_LOCAL",
        closure_notice: "CLOSURE_MUST_STAY_LOCAL",
        overview: {
          kind: "ready",
          cards: [
            overviewCard(),
            overviewCard({
              segment_started_at: 1_755_000_001_000,
              activity: "審閱後的更新流程",
              author: "reviewer",
              model_confidence: 0.42,
              evidence: [{ frame_id: 4243, label: "REVIEW_SOURCE_MUST_STAY_LOCAL" }],
            }),
            overviewCard({
              segment_started_at: 1_755_000_002_000,
              activity: "這是我自己修正的說法",
              author: "user",
              model_confidence: 0.88,
              evidence: [{ frame_id: 4244, label: "USER_SOURCE_MUST_STAY_LOCAL" }],
            }),
          ],
          truncated: true,
          evidence_unavailable: 2,
        },
      });
    },
    recording_state: "recording",
  });
  await p.type("QUESTION_MUST_STAY_LOCAL");
  const text = azureCalls(p)[0]?.arg?.text ?? "";
  checkOverviewVoice(p, "ready");
  const renderedCards = p.hits().querySelectorAll(".overview-card");
  for (const [index, [author, , voice, why]] of provenanceCases.entries()) {
    const card = renderedCards[index];
    check(`overviewProvenance ${author} 兩句接到同一張卡片`,
      card?.querySelector(".overview-meta")?.textContent.endsWith(` · ${voice}`) &&
      card?.querySelector("details")?.querySelector("p")?.textContent === why,
      card?.textContent);
  }
  for (const [name, exact] of [
    ["ready", "我目前對最近幾段有這些理解。每張下面都有畫面出處按鈕；內容可能由模型整理、審閱層修訂，或由你修正，不是我量到的確定事實："],
    ["truncated", "這裡只列最近一部分有證據的理解。"],
    ["withheld", "另外有 2 張理解卡目前沒有可點開的畫面出處，這裡沒有列。"],
    ["interpreter", "模型整理的假設 · 模型自報信心 0.31（不是量出來的）"],
    ["reviewer", "審閱層修訂 · 原模型自報信心 0.42（不是量出來的）"],
    ["user", "你修正過 · 不是她量出來的，也不是模型說的"],
  ]) checkOverviewWhy(p, name, exact);

  check(
    "ready 明講是可修正的理解／假設，不冒充確定事實",
    p.hitTexts().some(
      (line) =>
        line.includes("模型整理") &&
        line.includes("審閱層修訂") &&
        line.includes("由你修正") &&
        line.includes("不是我量到的確定事實"),
    ),
    p.hitTexts(),
  );
  check(
    "interpreter 信心明講是模型自報，不是假裝量過",
    p
      .hitTexts()
      .some(
        (line) =>
          line.includes("模型整理的假設") && line.includes("模型自報信心 0.31（不是量出來的）"),
      ),
    p.hitTexts(),
  );
  check("L2 activity 畫成答案正文", p.hitTexts().some((line) => line.includes("修好安裝更新")), p.hitTexts());
  check(
    "closure、follow-up、截斷與 withheld 計數仍留在畫面",
    [
      "CLOSURE_MUST_STAY_LOCAL",
      "FOLLOWUP_MUST_STAY_LOCAL",
      "只列最近一部分",
      "另外有 2 張理解卡",
    ].every((wanted) => p.hitTexts().some((line) => line.includes(wanted))),
    p.hitTexts(),
  );
  check(
    "overview 不會再跑 generic FTS/fact/empty renderer",
    !p.hitTexts().some(
      (line) =>
        line.includes("OCR_MUST_STAY_LOCAL") ||
        line.includes("FACT_MUST_STAY_LOCAL") ||
        line.includes("我記得的東西裡沒有"),
    ),
    p.hitTexts(),
  );
  check(
    "reviewer 信心明講沿用原模型自報，不是假裝審閱層量過",
    p
      .hitTexts()
      .some(
        (line) =>
          line.includes("審閱層修訂") && line.includes("原模型自報信心 0.42（不是量出來的）"),
      ),
    p.hitTexts(),
  );
  check(
    "user 修正不顯示沿用的模型信心",
    p
      .hitTexts()
      .some(
        (line) =>
          line.includes("你修正過 · 不是她量出來的，也不是模型說的") &&
          !line.includes("0.88") &&
          !line.includes("模型自報"),
      ),
    p.hitTexts(),
  );
  check(
    "overview 不冒充 retrieval 題庫，不顯示題庫標記或寫入失敗提示",
    p.hits().querySelector(".mark-toggle") === null &&
      !p.hitTexts().some((line) => line.includes("這一題沒進題庫")),
    p.hitTexts(),
  );

  const evidence = p.hits().querySelector(".overview-evidence");
  check("證據按鈕顯示後端 label", evidence?.textContent === "SOURCE_LABEL_MUST_STAY_LOCAL", evidence?.textContent);
  await p.clickElement(evidence, { trusted: false });
  check(
    "script 合成的 evidence click 不會開畫面",
    !p.invokes.some(({ cmd }) => cmd === "open_frame"),
    p.invokes,
  );
  await p.clickElement(evidence);
  check(
    "真人 evidence click 只用 typed frame id 開畫面",
    p.invokes.some(({ cmd, arg }) => cmd === "open_frame" && arg?.frameId === 4242),
    p.invokes,
  );

  const expected =
    "最近這幾件事，我記得是這樣。\n修好安裝更新\n審閱後的更新流程\n這是我自己修正的說法";
  check("Azure overview payload 恰好只有正文", text === expected, text);
  for (const localOnly of [
    "QUESTION_MUST_STAY_LOCAL",
    "OCR_MUST_STAY_LOCAL",
    "FACT_MUST_STAY_LOCAL",
    "SOURCE_LABEL_MUST_STAY_LOCAL",
    "SEARCHED_MUST_STAY_LOCAL",
    "FOLLOWUP_MUST_STAY_LOCAL",
    "CLOSURE_MUST_STAY_LOCAL",
    "99123",
    "4242",
    "4243",
    "4244",
    "0.31",
    "0.42",
    "0.88",
    "interpreter",
    "reviewer",
    "user",
    "REVIEW_SOURCE_MUST_STAY_LOCAL",
    "USER_SOURCE_MUST_STAY_LOCAL",
    "另外有 2 張理解卡",
  ]) {
    check(`overview Azure 不送 ${localOnly}`, !text.includes(localOnly), text);
  }

  await p.type("第二題會失敗");
  check(
    "有證據 overview 算一份正在顯示的答案",
    p.hitTexts().some((line) => line.includes("上一題的，先收起來了")),
    p.hitTexts(),
  );
}

console.log("65. raw-only／empty／證據遺失各自說實話，不掉進一般空結果");
{
  const cases = [
    {
      overview: { kind: "raw_only" },
      wanted: "有原始紀錄，但還沒有整理成能直接回答的理解記憶",
      exact: "我有原始紀錄，但還沒有整理成能直接回答的理解記憶；這次不會拿 OCR 片段冒充答案。",
      azure: "有記下來，但還沒理清楚。",
    },
    {
      overview: { kind: "empty" },
      wanted: "目前還沒有留下能回答這題的記憶",
      exact: "我目前還沒有留下能回答這題的記憶。",
      azure: "這個我沒印象。",
    },
    {
      overview: { kind: "evidence_missing", cards: 3 },
      wanted: "目前沒有可點開的畫面出處",
      exact: "我有整理過的理解記憶，但最近這 3 張卡片目前沒有可點開的畫面出處；這裡不把它們當成答案。",
      azure: "我有點印象，但找不到畫面，所以不敢說。",
    },
  ];
  for (const test of cases) {
    const p = await open({
      azure_tts_read: AZURE_READY,
      azure_tts_speak: new Error("captured non-ready overview"),
      ask: answer({
        kind: "memory_overview",
        hits: [hit({ snippet: "RAW_OCR_MUST_NOT_RENDER" })],
        overview: test.overview,
      }),
      recording_state: "recording",
    });
    await p.type("你知道了什麼");
    checkOverviewWhy(p, test.overview.kind, test.exact);
    checkOverviewVoice(p, test.overview.kind);
    check(`${test.overview.kind} 沒有答案卡片或證據按鈕`,
      p.hits().querySelector(".hit, .overview-evidence") === null,
      p.hitTexts());

    check(`${test.overview.kind} 有自己的答案`, p.hitTexts().some((line) => line.includes(test.wanted)), p.hitTexts());
    check(
      `${test.overview.kind} 不顯示 OCR 或 generic empty`,
      !p.hitTexts().some(
        (line) => line.includes("RAW_OCR_MUST_NOT_RENDER") || line.includes("我記得的東西裡沒有這件事"),
      ),
      p.hitTexts(),
    );
    check(`${test.overview.kind} 沒有可標成答對的卡片`, p.hits().querySelector(".mark-toggle") === null, p.hitTexts());
    check(
      `${test.overview.kind} 的 Azure payload 恰好是自己的狀態正文`,
      azureCalls(p).length === 1 && azureCalls(p)[0].arg?.text === test.azure,
      azureCalls(p),
    );
  }
}

console.log("65b. 總覽的大腦狀態精確版可展開，本機朗讀不念內部說明");
for (const [state, exact] of [
  ["search_failed", "Codex 這次沒有完成查詢；下方是依原問題找到的本機記憶。到設定按「測試目前大腦」即可重測。"],
  ["consent_required", "這題只顯示本機結果；第二張「雲端解讀」目前沒有授權 Codex 接手。"],
  ["not_configured", "到設定選一個 CLI，大腦才會接手文字問題。"],
]) {
  const p = await open({
    ask: answer({ kind: "memory_overview", overview: { kind: "empty" }, brain: { state, provider: "Codex" } }),
    recording_state: "recording",
    persona_read: { id: "chatgpt", enabled: true, motion: true, tap_lines: true, voice_enabled: true },
  }, { systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }] });
  await p.type("你知道了什麼");
  checkOverviewWhy(p, `brain-${state}`, exact);
  checkOverviewVoice(p, `brain-${state}`);
  for (const details of p.hits().querySelectorAll("details")) details.open = true;
  await p.clickElement(p.localReadButton());
  check(`brain-${state} 本機朗讀有人話、不念展開說明`,
    p.localSpeechTexts().length === 1 &&
      p.localSpeechTexts()[0].includes("這個我沒印象。") &&
      !p.localSpeechTexts()[0].includes(exact) &&
      !p.localSpeechTexts()[0].includes("為什麼") &&
      !p.localSpeechTexts()[0].includes("我目前還沒有留下能回答這題的記憶。"),
    p.localSpeechTexts());
  const calls = p.invokes.filter(({ cmd }) => cmd === "ask").length;
  check(`brain-${state} 展開收合不重送 ask`, calls === 1, p.invokes);
}

console.log("66. 未知 overview kind 是 contract error，不偽裝成沒有記憶");
{
  const p = await open({
    ask: answer({ kind: "memory_overview", overview: { kind: "future_state" } }),
    recording_state: "recording",
  });
  await p.type("你知道了什麼");
  check("錯誤原因點名未知狀態", p.line().includes("future_state"), p.line());
  check(
    "畫面說這題沒答成，不說沒有記憶",
    p.hitTexts().some((line) => line.includes("沒答成")) &&
      !p.hitTexts().some((line) => line.includes("沒有留下能回答")),
    p.hitTexts(),
  );
}

console.log("67. kind、overview 與作者是封閉契約，錯線不能畫成正常答案");
{
  // 底下每個 ready 夾具都只有**一張**卡，而且那一張就是壞的——所以 throw
  // 發生在任何一張卡被畫進 `hitList` 之前。這一節證得了「壞的不畫」，證不了
  // 「已經畫出去的要收回」。「好的排在壞的前面」那一格在 67b。
  const cases = [
    answer({ kind: "memory_overview", overview: null }),
    answer({ kind: "keywords", overview: { kind: "raw_only" } }),
    answer({
      kind: "memory_overview",
      overview: {
        kind: "ready",
        cards: [overviewCard({ author: "future_author" })],
        truncated: false,
        evidence_unavailable: 0,
      },
    }),
    answer({
      kind: "memory_overview",
      overview: {
        kind: "ready",
        cards: [overviewCard({ model_confidence: null })],
        truncated: false,
        evidence_unavailable: 0,
      },
    }),
    answer({
      kind: "memory_overview",
      overview: {
        kind: "ready",
        cards: [overviewCard({ evidence: [{ frame_id: 0, label: "not-openable" }] })],
        truncated: false,
        evidence_unavailable: 0,
      },
    }),
  ];
  for (const payload of cases) {
    const p = await open({ ask: payload, recording_state: "recording" });
    await p.type("她知道了什麼");
    check(
      "contract 錯線明講這題沒答成",
      p.hitTexts().some((line) => line.includes("這一題我沒答成")),
      p.hitTexts(),
    );
    check(
      "contract 錯線不顯示成空記憶或正常 activity",
      !p.hitTexts().some((line) => line.includes("沒有留下能回答") || line.includes("修好安裝更新")),
      p.hitTexts(),
    );
  }
}

console.log("67b. 第一張理解卡是好的、第二張壞掉：整份收回，不留半份");
{
  // 和 56g 同一把刀，打在另一支 renderer 上。`renderOverview` 的 ready 那一臂和
  // `renderGrounded` 是**一樣的形狀**：先把開場白寫進 `hitList`，再邊驗邊
  // `hitList.append(li)`。而 67 那三個壞卡片夾具全是 `cards: [一張壞的]`——壞的
  // 排第一個，於是 throw 發生在任何一張卡被畫進去之前，「半份」那一格一樣沒人
  // 造過。見 `a-suite-that-varies-the-wrong-axis-is-one-test` 那條。
  //
  // 兩支 renderer 現在靠的是**同一個**機制得救：`ask()` 的 catch 把整張清單
  // `replaceChildren(failed)`。所以這一節真正守的不是「又一次一樣的行為」，是
  // 「這兩支不准各自長出自己的 try/catch 把例外吞掉」——吞掉的那一版，56g 看不
  // 見，因為它問的是另一支 renderer。底下的刀①就是那個。
  const GOOD_ACTIVITY = "修好安裝更新";
  const p = await open({
    azure_tts_read: AZURE_READY,
    ask: answer({
      kind: "memory_overview",
      // 見 56g：`answer()` 預設沒有 native lease，不帶 id 的話底下那條「票還
      // 回去了」問的是一張從來沒發出來的票。
      presentation_id: "67b-lease",
      overview: {
        kind: "ready",
        cards: [
          overviewCard(),
          overviewCard({
            segment_started_at: 1_755_000_001_000,
            activity: "第二張卡的出處是壞的",
            evidence: [{ frame_id: 0, label: "not-openable" }],
          }),
        ],
        truncated: false,
        evidence_unavailable: 0,
      },
    }),
    recording_state: "recording",
  });
  await p.type("你知道了什麼");

  check(
    "一張理解卡都沒有畫出來",
    p.hits().querySelectorAll(".overview-card").length === 0,
    p.hitTexts(),
  );
  // 上面那條問 class，這一條問**那幾個字在不在畫面上**：把 class 拿掉而字留著
  // 是一種修法，而使用者讀到的是字。
  check(
    "第一張卡那句話也不可以留著",
    !p.hitTexts().some((line) => line.includes(GOOD_ACTIVITY)),
    p.hitTexts(),
  );
  // 開場白是在迴圈**之前**就寫進去的，所以它比任何一張卡都早進 DOM。
  check(
    "連那句開場白都要收掉",
    !p.hitTexts().some((line) => line.includes("我目前對最近幾段有這些理解")),
    p.hitTexts(),
  );
  check("說得出壞在哪裡", p.line().includes("frame_id"), p.line());
  check("而且明講這一題沒答成", p.hitTexts().some((line) => line.includes("沒答成")), p.hitTexts());
  // 半份留在 DOM 裡的話，自動送 Azure 那條路（`data-azure-answer-body`）會在他
  // 讀到錯誤訊息之前就把那張卡送上雲端——而它是一張被拒絕的答案。
  check("一個字都沒送去 Azure", azureCalls(p).length === 0, azureCalls(p).map(({ arg }) => arg?.text));
  check(
    "畫到一半炸掉，native 那張票仍然還回去了",
    p.playbackTrace().includes("end:67b-lease"),
    p.playbackTrace(),
  );
}

console.log("68. 全停中不可以出現『在聽』");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  check("前提：recording 與 supervisor 證據齊全時顯示在聽", p.line().startsWith("在聽"), p.line());
  await p.fromOutside("master-stop-changed", "stopped");
  check(
    "全停後不再聲稱在聽或正在錄",
    !p.line().includes("在聽") && !p.line().includes("正在錄"),
    p.line(),
  );
  check("全停後顯示全停主句", p.line().includes("已全停：capture／brain／hands 都不會動"), p.line());
  check("全停後圖案也是 stopped，不是暫停的斜槓", p.avatarState() === "stopped", p.avatarState());
}

console.log("69. 全停壓過暫停");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  await p.fromOutside("pause-changed", true);
  check("前提：先顯示暫停", p.line().includes("已暫停，沒有在看"), p.line());
  await p.fromOutside("master-stop-changed", "stopped");
  check("全停主句壓過暫停主句", p.line().includes("已全停：capture／brain／hands 都不會動"), p.line());
  check("全停時不顯示暫停主句", !p.line().includes("已暫停，沒有在看"), p.line());
  check("暫停中再全停，圖案換成 stopped", p.avatarState() === "stopped", p.avatarState());
}

console.log("70. 解除暫停不可以讓全停畫面消失");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  await p.fromOutside("pause-changed", true);
  await p.fromOutside("master-stop-changed", "stopped");
  await p.fromOutside("pause-changed", false);
  check(
    "解除暫停後仍顯示全停主句",
    p.line().includes("已全停：capture／brain／hands 都不會動"),
    p.line(),
  );
  check("解除暫停後仍不聲稱在聽", !p.line().includes("在聽"), p.line());
  check("解除暫停後圖案仍是 stopped", p.avatarState() === "stopped", p.avatarState());
}

console.log("71. 全停 detail 講得出正確的解除入口");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  await p.fromOutside("master-stop-changed", "stopped");
  check("全停 detail 指向解除全停", p.line().includes("要恢復，請從系統匣按「解除全停」"), p.line());
  check("全停 detail 不指向 pause 恢復鍵", !p.line().includes("▶") && !p.line().includes("繼續"), p.line());
}

console.log("72. 解除全停之後，原本的暫停要回來");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  await p.fromOutside("pause-changed", true);
  await p.fromOutside("master-stop-changed", "stopped");
  check("前提：全停時顯示全停主句", p.line().includes("已全停：capture／brain／hands 都不會動"), p.line());
  await p.fromOutside("master-stop-changed", "clear");
  check("解除全停後回到原本的暫停主句", p.line().includes("已暫停，沒有在看"), p.line());
  check("解除全停後沒有直接跳回在聽", !p.line().includes("在聽"), p.line());
}

console.log("73. master-stop-failed 的字要出現在畫面上");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  const failedMasterStop = "全停沒有成功：master.stop 寫不進去";
  await p.fromOutside("master-stop-failed", failedMasterStop);
  check("全停失敗 payload 逐字出現在畫面", p.line().includes(failedMasterStop), p.line());
}

console.log("74. 四顆停止選單項的建立、每個 menu 分支、managed state 與 refresh 都接齊");
{
  const main = read(MAIN);
  const menuBranches = [...main.matchAll(/Menu::with_items\(\s*app,\s*&\[([\s\S]*?)\],\s*\)\?/g)].map(
    (match) => match[1],
  );
  check("找得到 Menu::with_items 分支", menuBranches.length > 0, `${menuBranches.length} 個`);

  const refreshStart = main.indexOf("fn refresh_tray(app: &tauri::AppHandle)");
  const refreshEnd = main.indexOf("\n}\n\nfn master_stop_phase", refreshStart);
  const refreshTray =
    refreshStart >= 0 && refreshEnd > refreshStart ? main.slice(refreshStart, refreshEnd) : "";
  check("找得到 refresh_tray 函式", refreshTray !== "", [refreshStart, refreshEnd]);

  const items = [
    ["master-stop", "master_stop_item", "MasterStopItem"],
    ["master-resume", "master_resume_item", "MasterResumeItem"],
    ["hands-stop", "hands_stop_item", "HandsStopItem"],
    ["hands-resume", "hands_resume_item", "HandsResumeItem"],
  ];
  for (const [id, variable, state] of items) {
    check(
      `${id} 有用 with_id 建立`,
      main.includes(`MenuItem::with_id(app, "${id}"`),
      `缺少 MenuItem::with_id(app, "${id}", …)`,
    );
    for (const [index, branch] of menuBranches.entries()) {
      check(
        `${id} 出現在第 ${index + 1} 個 Menu::with_items 分支`,
        branch.includes(`&${variable}`),
        `第 ${index + 1} 個 Menu::with_items 分支缺少 &${variable}`,
      );
    }
    check(
      `${id} 有 app.manage(${state})`,
      main.includes(`app.manage(${state}(${variable}));`),
      `缺少 app.manage(${state}(${variable}));`,
    );
    check(
      `refresh_tray 讀得到 ${id} 的 ${state}`,
      refreshTray.includes(`app.try_state::<${state}>()`),
      `refresh_tray 缺少 app.try_state::<${state}>()`,
    );
  }
}

console.log("75. latch 現在全停、資料庫歷史為零時，blind 說現在與可行入口");
{
  const p = await open({
    ask: answer({
      blind: blind({
        chunks: 10,
        ever_recorded: true,
        ever_stored: true,
        master_stop_state: "stopped",
      }),
    }),
    recording_state: "recording",
    master_stop_state: "clear",
  });
  await p.type("找不到的歷史");
  const said = p.hitTexts().join("\n");
  check("現在全停有自己的句子", said.includes("我現在正全停中"), said);
  check("現在全停指向系統匣解除入口", said.includes("系統匣 → 解除全停"), said);
  check("歷史為零不捏造全停 episode", !said.includes("被全停過"), said);
}

console.log("76. latch 已解除時，完整 historical episode 顯示次數與完整 duration");
{
  const p = await open({
    ask: answer({
      blind: blind({
        chunks: 10,
        ever_recorded: true,
        ever_stored: true,
        master_stopped_episodes: 2,
        master_stopped_ms: 5 * 60_000,
      }),
    }),
    recording_state: "recording",
    master_stop_state: "clear",
  });
  await p.type("找不到的歷史");
  const said = p.hitTexts().join("\n");
  check("released 歷史顯示 episode 數", said.includes("被全停過 2 次"), said);
  check("完整歷史顯示完整 duration", said.includes("一共 5 分鐘"), said);
  check("已解除不冒充現在全停", !said.includes("現在正全停中"), said);
}

console.log("77. historical open/truncated 在 latch 已解除時仍只講稽核歷史");
{
  const p = await open({
    ask: answer({
      blind: blind({
        chunks: 10,
        ever_recorded: true,
        ever_stored: true,
        master_stopped_episodes: 3,
        master_stopped_ms: 7 * 60_000,
        master_stopped_open: true,
        master_stopped_truncated: 1,
      }),
    }),
    recording_state: "recording",
    master_stop_state: "clear",
  });
  await p.type("找不到的歷史");
  const said = p.hitTexts().join("\n");
  check("未收尾歷史明講最後一段沒有收尾", said.includes("最後一段沒有收尾"), said);
  check("截斷歷史明講一段開頭被刪", said.includes("有 1 段的開頭已被保留期刪掉"), said);
  check("有缺口的 duration 明講只是下限", said.includes("所以這個數字算短了"), said);
  check("歷史 open 不冒充 live latch", !said.includes("現在正全停中"), said);
}

console.log("78. historical 與 live master stop 是兩句可同時成立的事");
{
  const p = await open({
    ask: answer({
      blind: blind({
        chunks: 10,
        ever_recorded: true,
        ever_stored: true,
        master_stopped_episodes: 4,
        master_stopped_ms: 9 * 60_000,
        master_stop_state: "stopped",
      }),
    }),
    recording_state: "recording",
    master_stop_state: "clear",
  });
  await p.type("找不到的歷史");
  const said = p.hitTexts().join("\n");
  check("同一份 blind 同時顯示歷史", said.includes("被全停過 4 次"), said);
  check("同一份 blind 同時顯示現在", said.includes("現在正全停中"), said);
}

console.log("79. native stop event 到達後，較早的 poll false 晚回不能蓋掉 stopped");
{
  let reads = 0;
  let finishStalePoll = null;
  const stalePoll = new Promise((resolvePoll) => {
    finishStalePoll = resolvePoll;
  });
  const p = await open({
    master_stop_state: () => {
      reads += 1;
      return reads === 3 ? stalePoll : "clear";
    },
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  check("前提：開場、visible poll 與 listener-ready 補讀都真的送出", reads === 3 && typeof finishStalePoll === "function", reads);
  await p.fromOutside("master-stop-changed", "stopped");
  check("event 先把畫面切成 stopped", p.avatarState() === "stopped", p.line());
  finishStalePoll("clear");
  await tick(40);
  check("舊 poll false 晚回後仍是 stopped", p.avatarState() === "stopped", p.line());
}

console.log("79a. listener 安裝缺口會補讀；event 後的舊 poll rejection 也不能倒退狀態");
{
  let phase = "clear";
  let releaseListener = () => {};
  const listenerHeld = new Promise((resolve) => {
    releaseListener = resolve;
  });
  const gap = await open(
    {
      master_stop_state: () => phase,
      recording_state: "recording",
      recorder_supervisor_state: supervisor("running"),
    },
    {
      beforeListenerRegistered: (name) =>
        name === "master-stop-changed" ? listenerHeld : undefined,
    },
  );
  const readsBeforeReady = gap.calls.filter((cmd) => cmd === "master_stop_state").length;
  phase = "stopped";
  releaseListener();
  await tick(40);
  const readsAfterReady = gap.calls.filter((cmd) => cmd === "master_stop_state").length;
  check("listener ready 後確實補讀 durable master-stop state", readsAfterReady > readsBeforeReady, gap.calls);
  check("缺口裡遺失的 stopped transition 被追回", gap.avatarState() === "stopped", gap.line());

  let reads = 0;
  let rejectOldPoll = () => {};
  const oldPoll = new Promise((_, reject) => {
    rejectOldPoll = reject;
  });
  const rejection = await open({
    master_stop_state: () => {
      reads += 1;
      return reads === 4 ? oldPoll : "clear";
    },
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  await rejection.pollNow();
  check("前提：較舊的 poll rejection 被 hold", reads === 4, reads);
  await rejection.fromOutside("master-stop-changed", "stopped");
  rejectOldPoll(new Error("old read failed"));
  await tick(40);
  check("event 後才回來的舊 rejection 不可改成 uncertain", rejection.avatarState() === "stopped", rejection.line());
}

console.log("80. 兩個 overlapping master-stop polls 只有最新 request 可 apply");
{
  let reads = 0;
  let finishOlderPoll = null;
  const olderPoll = new Promise((resolvePoll) => {
    finishOlderPoll = resolvePoll;
  });
  const p = await open({
    master_stop_state: () => {
      reads += 1;
      if (reads === 4) return olderPoll;
      return "clear";
    },
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
  });
  await p.pollNow();
  check("前提：較早的第四份 read 被 hold", reads === 4 && typeof finishOlderPoll === "function", reads);
  await p.pollNow();
  check("較新的第五份 false 已套用", reads === 5 && p.avatarState() !== "stopped", {
    reads,
    state: p.avatarState(),
  });
  finishOlderPoll("stopped");
  await tick(40);
  check("更舊的 true 晚回不能蓋掉最新 false", p.avatarState() !== "stopped", p.line());
}

function desktopTruthSourceErrors(main, dispatch, ui) {
  const errors = [];
  const callback = /"master-stop" \| "master-resume" => \{([\s\S]*?)\n\s*\}/.exec(main)?.[1] ?? "";
  if (!callback.includes("dispatch_master_stop_menu(app, event.id.as_ref());")) {
    errors.push("production callback 沒有呼叫唯一 dispatch helper");
  }

  const helperStart = main.indexOf("fn dispatch_master_stop_menu(");
  const helperEnd = main.indexOf("\n}\n\n#[cfg(test)]", helperStart);
  const helper = helperStart >= 0 && helperEnd > helperStart ? main.slice(helperStart, helperEnd) : "";
  if (!helper.includes("master_stop_action_for_menu_id(menu_id)")) errors.push("helper 沒有走 fixed-direction policy");
  if (!helper.includes("master_stop_queue()") || !helper.includes(".send(MasterStopJob")) {
    errors.push("helper 沒有把 click 依序送進單一背景 queue");
  }
  if (!main.includes("MASTER_STOP_QUEUE") || !main.includes("run_fifo(receiver, run_master_stop_job)")) {
    errors.push("背景全停不是 FIFO single worker");
  }
  if (!main.includes("set_master_stop_with_observer")) errors.push("worker 沒有在 pending publication 接 Stopping observer");
  if (!main.includes("run_on_main_thread")) errors.push("worker 結果沒有 marshal 回 tray 主迴圈");
  if (!main.includes("fn finish_master_stop_menu") || !main.includes("app.emit(MASTER_STOP_FAILED_EVENT, error)")) {
    errors.push("失敗沒有 emit master-stop-failed");
  }
  if (!main.includes("refresh_tray(app);")) errors.push("完成沒有 refresh tray/changed state");

  if (!dispatch.includes('"master-stop" => Some(MasterStopAction::Engage)')) errors.push("master-stop 方向不是 Engage");
  if (!dispatch.includes('"master-resume" => Some(MasterStopAction::Release)')) errors.push("master-resume 方向不是 Release");
  if (!dispatch.includes("_ => None")) errors.push("未知 menu id 沒有拒絕");

  const changedNative = /const MASTER_STOP_CHANGED_EVENT: &str = "([^"]+)";/.exec(main)?.[1];
  const failedNative = /const MASTER_STOP_FAILED_EVENT: &str = "([^"]+)";/.exec(main)?.[1];
  const listened = [...ui.matchAll(/\.listen\?\.\("([^"]+)"/g)].map((match) => match[1]);
  if (changedNative !== "master-stop-changed" || !listened.includes(changedNative)) {
    errors.push("native/renderer master-stop-changed 名稱不一致");
  }
  if (failedNative !== "master-stop-failed" || !listened.includes(failedNative)) {
    errors.push("native/renderer master-stop-failed 名稱不一致");
  }
  if (!main.includes("app.emit(MASTER_STOP_CHANGED_EVENT, phase)")) errors.push("native 沒有 emit changed state");

  const mappingStart = main.indexOf("impl From<sister_core::answer::BlindSpots> for Blind");
  const mappingEnd = main.indexOf("\n}\n\n#[cfg(test)]\nmod blind_dto_tests", mappingStart);
  const mapping = mappingStart >= 0 && mappingEnd > mappingStart ? main.slice(mappingStart, mappingEnd) : "";
  for (const field of [
    "master_stopped_episodes",
    "master_stopped_ms",
    "master_stopped_open",
    "master_stopped_truncated",
    "master_stop_state",
  ]) {
    if (!mapping.includes(`${field}: blind.${field}`)) errors.push(`Blind mapping 丟掉或接錯 ${field}`);
  }
  if (!main.includes("Some(Blind::from(b))")) errors.push("ask 沒有走 tested Blind mapping");

  if (!main.includes('admit_desktop_brain(shell.data_dir.as_deref(), "守門員這一輪")')) {
    errors.push("gatekeeper_check 沒有在 DB 判決／寫入前取得 master-stop admission");
  }
  if (!main.includes("view.presentation_id = Some(hold_presentation(master_stop_admission));")) {
    errors.push("gatekeeper native guard 沒有跨到 renderer presentation lease");
  }
  if (!main.includes("answer.presentation_id = Some(hold_presentation(master_stop_admission));")) {
    errors.push("ask native guard 沒有跨到 renderer presentation lease");
  }
  if (!main.includes('admit_desktop_brain(Some(data_dir), "這次 Azure 朗讀")')) {
    errors.push("Azure speak 沒有取得 master-stop activity admission");
  }
  if (!main.includes("master_stop_admission.boundary()") || !main.includes("Ok((bytes, master_stop_admission))")) {
    errors.push("Azure speak 沒有在排程前重驗 boundary 並把 activity guard 帶過 transport");
  }
  if (!main.includes("presentation_id: hold_presentation(master_stop_admission)")) {
    errors.push("Azure transport guard 沒有跨 IPC 變成 renderer presentation lease");
  }

  if ((ui.match(/readMasterStopState\(\)/g) ?? []).length < 4) errors.push("startup/poll/listener-ready 沒有共用 master-stop read helper");
  if (!ui.includes("masterStopRevision += 1;")) errors.push("event 沒有推進 master-stop revision");
  if (!ui.includes("request === masterStopReadRequest")) errors.push("poll 沒有只接受最新 request");
  if (ui.includes('invoke("master_stop_state").then(setMasterStopPhase')) errors.push("仍有第二條直連 master-stop read");
  for (const phase of ["clear", "stopping", "stopped", "uncertain"]) {
    if (!ui.includes(`"${phase}"`)) errors.push(`renderer 沒有處理 ${phase}`);
  }
  if (!main.includes("Result<sister_hands::master_stop::State, String>")) {
    errors.push("native master_stop_state 仍把四態壓成 bool");
  }
  const handlers = /\.invoke_handler\(tauri::generate_handler!\[([\s\S]*?)\]\)/.exec(main)?.[1] ?? "";
  if (!handlers.includes("master_stop_state")) errors.push("master_stop_state 沒有註冊進 Tauri handler");
  for (const command of ["master_stop_presentation_begin", "master_stop_presentation_end"]) {
    if (!handlers.includes(command)) errors.push(`${command} 沒有註冊進 Tauri handler`);
  }
  if (!ui.includes("commitNativePresentation(answer")) errors.push("ask Promise 沒有走 native presentation commit");
  if (!ui.includes("commitNativePresentation(view")) errors.push("gatekeeper Promise 沒有走 native presentation commit");
  if (!ui.includes("azurePlaybackPresentation = audio") || !ui.includes("releaseAzurePlaybackPresentation(audio)")) {
    errors.push("Azure presentation lease 沒有跨到 playback ended/error");
  }
  if (!ui.includes("gatekeeperReadRequest += 1;")) errors.push("非 clear master stop 沒有讓 gatekeeper poll 失效");
  return errors;
}

console.log("81. stopping／uncertain 不冒充已全停，也不准顯示在聽");
{
  const p = await open({
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
    master_stop_state: "clear",
  });
  await p.fromOutside("master-stop-changed", "stopping");
  check("停止中明講仍在排乾", p.line().includes("正在完成全停") && p.line().includes("仍在排乾"), p.line());
  check("停止中不冒充完成", !p.line().includes("已全停") && !p.line().includes("在聽"), p.line());
  await p.fromOutside("master-stop-changed", "uncertain");
  check("不確定明講讀不到", p.line().includes("無法確認全停狀態"), p.line());
  check("不確定也不冒充完成或在聽", !p.line().includes("已全停") && !p.line().includes("在聽"), p.line());

  const blindPending = await open({
    ask: answer({
      blind: blind({
        chunks: 10,
        ever_recorded: true,
        ever_stored: true,
        master_stop_state: "stopping",
      }),
    }),
    recording_state: "recording",
    master_stop_state: "clear",
  });
  await blindPending.type("找不到的歷史");
  const said = blindPending.hitTexts().join("\n");
  check("Blind pending 說新工作已拒絕且仍在排乾", said.includes("新工作已拒絕") && said.includes("仍在排乾"), said);
  check("Blind pending 沒說三層已停", !said.includes("現在正全停中"), said);
}

console.log("81a. 全停事件夾在 native Answer 與 renderer continuation 之間時，晚答案與 Azure 都失效");
{
  let finishAsk = () => {};
  const heldAsk = new Promise((resolve) => {
    finishAsk = resolve;
  });
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("不該送到這裡"),
    ask: () => heldAsk,
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
    master_stop_state: "clear",
  });
  void p.type("全停前開始的慢題");
  await tick(40);
  check("前提：native ask 已經在飛", p.calls.filter((cmd) => cmd === "ask").length === 1, p.calls);
  check("A151 前提：native ask_local 也剛好一次", p.calls.filter((cmd) => cmd === "ask_local").length === 1, p.calls);
  await p.fromOutside("master-stop-changed", "stopping");
  finishAsk(answer({ hits: [hit({ snippet: "LATE_MASTER_STOP_ANSWER" })] }));
  await tick(40);
  const rendered = p.hitTexts().join("\n");
  check("Stopping 之後回來的答案不 render", !rendered.includes("LATE_MASTER_STOP_ANSWER"), rendered);
  check("同一份晚答案不建立 Azure POST", azureCalls(p).length === 0, azureCalls(p));
  check("畫面仍是全停正在排乾，不退回 idle 答案", p.avatarState() === "stopped" && p.line().includes("正在完成全停"), p.line());
}

console.log("81b. 外部 CLI stop 沒有 event：native presentation boundary 拒絕就不能畫答案或送 Azure");
{
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: new Error("不該送到這裡"),
    ask: answer({
      presentation_id: "4101",
      hits: [hit({ snippet: "EXTERNAL_STOP_LATE_ANSWER" })],
    }),
    master_stop_presentation_begin: false,
    master_stop_presentation_end: null,
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
    master_stop_state: "clear",
  });
  await p.type("外部終端剛按全停");
  await tick(40);
  const rendered = p.hitTexts().join("\n");
  check("boundary 拒絕的 native answer 不 render", !rendered.includes("EXTERNAL_STOP_LATE_ANSWER"), rendered);
  check("boundary 拒絕的 answer 不建立 Azure POST", azureCalls(p).length === 0, azureCalls(p));
  check(
    "renderer 有 begin 也有 end native lease",
    p.calls.includes("master_stop_presentation_begin") && p.calls.includes("master_stop_presentation_end"),
    p.calls,
  );
}

console.log("81c. Gatekeeper 也是 brain：external pending 拒絕 presentation 就不能冒出新主動卡");
{
  const view = {
    ...gatekeeper(gateCard({ text: "EXTERNAL_STOP_GATEKEEPER_CARD" })),
    presentation_id: "4102",
  };
  const p = await open({
    gatekeeper_check: view,
    master_stop_presentation_begin: false,
    master_stop_presentation_end: null,
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
    master_stop_state: "clear",
  });
  await tick(40);
  check("boundary 拒絕後沒有 gatekeeper 卡", p.utterance().hidden === true, p.utterance().textContent);
  check(
    "Gatekeeper presentation lease 有收尾",
    p.calls.includes("master_stop_presentation_begin") && p.calls.includes("master_stop_presentation_end"),
    p.calls,
  );
}

console.log("81d. 正常 Answer／Gatekeeper 都在可見內容完成後才 end native lease");
{
  let gateSequence = 0;
  let answerVisibleAtEnd = false;
  let gateVisibleAtEnd = false;
  const p = await open({
    ask: answer({
      presentation_id: "normal-answer",
      hits: [hit({ snippet: "NORMAL_ANSWER_VISIBLE" })],
    }),
    gatekeeper_check: () => ({
      ...gatekeeper(gateCard({ text: "NORMAL_GATE_VISIBLE" })),
      presentation_id: `normal-gate-${++gateSequence}`,
    }),
    master_stop_presentation_begin: true,
    master_stop_presentation_end: ({ presentationId }) => {
      if (presentationId === "normal-answer") {
        answerVisibleAtEnd = globalThis.document
          .querySelector("[data-hits]")
          .textContent.includes("NORMAL_ANSWER_VISIBLE");
      }
      if (presentationId.startsWith("normal-gate-")) {
        gateVisibleAtEnd = globalThis.document
          .querySelector("[data-utterance-text]")
          .textContent.includes("NORMAL_GATE_VISIBLE");
      }
      return null;
    },
    recording_state: "recording",
    recorder_supervisor_state: supervisor("running"),
    master_stop_state: "clear",
  });
  await p.type("正常 commit");
  const answerLeaseCalls = p.invokes.filter(
    ({ cmd, arg }) =>
      ["master_stop_presentation_begin", "master_stop_presentation_end"].includes(cmd) &&
      arg?.presentationId === "normal-answer",
  );
  const gateLeaseCalls = p.invokes.filter(
    ({ cmd, arg }) =>
      ["master_stop_presentation_begin", "master_stop_presentation_end"].includes(cmd) &&
      arg?.presentationId?.startsWith("normal-gate-"),
  );
  check(
    "Answer begin=true 後真的顯示，且 end 當下內容已可見",
    p.hitTexts().join("\n").includes("NORMAL_ANSWER_VISIBLE") &&
      answerVisibleAtEnd &&
      answerLeaseCalls.map(({ cmd }) => cmd).join(",") ===
        "master_stop_presentation_begin,master_stop_presentation_end",
    { answerVisibleAtEnd, answerLeaseCalls },
  );
  check(
    "Gatekeeper begin=true 後真的顯示，且每份 lease 都有 begin/end",
    gateVisibleAtEnd &&
      gateLeaseCalls.length >= 2 &&
      gateLeaseCalls.filter(({ cmd }) => cmd === "master_stop_presentation_begin").length ===
        gateLeaseCalls.filter(({ cmd }) => cmd === "master_stop_presentation_end").length,
    { gateVisibleAtEnd, gateLeaseCalls },
  );
}

console.log("81e. Gatekeeper reaction 也要過 presentation boundary；拒絕不改卡，成功才收卡");
{
  const blocked = await open({
    gatekeeper_check: {
      ...gatekeeper(gateCard({ text: "REACTION_CARD_STAYS" })),
      presentation_id: "reaction-card-blocked",
    },
    gatekeeper_react: {
      message: "BLOCKED_REACTION_MUST_NOT_APPEAR",
      presentation_id: "reaction-blocked",
    },
    master_stop_presentation_begin: ({ presentationId }) =>
      presentationId !== "reaction-blocked",
    master_stop_presentation_end: null,
    recording_state: "recording",
    master_stop_state: "clear",
  });
  await blocked.click("[data-utterance-close]");
  check(
    "reaction boundary 拒絕後原卡仍在、結果沒冒出來",
    !blocked.utterance().hidden &&
      blocked.node("[data-utterance-text]").textContent.includes("REACTION_CARD_STAYS") &&
      !blocked.node("[data-utterance-result]").textContent.includes("BLOCKED_REACTION_MUST_NOT_APPEAR") &&
      !blocked.node("[data-utterance-actions]").hidden,
    {
      card: blocked.node("[data-utterance-text]").textContent,
      result: blocked.node("[data-utterance-result]").textContent,
    },
  );
  check(
    "被拒 reaction lease 仍有 end",
    blocked.invokes.some(
      ({ cmd, arg }) =>
        cmd === "master_stop_presentation_end" && arg?.presentationId === "reaction-blocked",
    ),
    blocked.invokes,
  );

  let reactionVisibleAtEnd = false;
  const allowed = await open({
    gatekeeper_check: {
      ...gatekeeper(gateCard({ text: "REACTION_CARD_CLOSES" })),
      presentation_id: "reaction-card-allowed",
    },
    gatekeeper_react: {
      message: "REACTION_COMMITTED",
      presentation_id: "reaction-allowed",
    },
    master_stop_presentation_begin: true,
    master_stop_presentation_end: ({ presentationId }) => {
      if (presentationId === "reaction-allowed") {
        reactionVisibleAtEnd = globalThis.document
          .querySelector("[data-utterance-result]")
          .textContent.includes("REACTION_COMMITTED");
      }
      return null;
    },
    recording_state: "recording",
    master_stop_state: "clear",
  });
  await allowed.click("[data-utterance-close]");
  check(
    "reaction begin=true 才寫結果並收起 actions，end 時結果已可見",
    allowed.node("[data-utterance-result]").textContent.includes("REACTION_COMMITTED") &&
      allowed.node("[data-utterance-actions]").hidden &&
      reactionVisibleAtEnd,
    { text: allowed.node("[data-utterance-result]").textContent, reactionVisibleAtEnd },
  );
}

console.log("81f. Azure response 也要過 native playback boundary；拒絕時一個 frame 都不播");
{
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: {
      generation: 8,
      content_type: "audio/mpeg",
      audio_bytes: 3,
      data_url: "data:audio/mpeg;base64,AQID",
      presentation_id: "azure-playback-blocked",
    },
    ask: answer({ hits: [hit({ snippet: "AZURE_BOUNDARY_BODY" })] }),
    master_stop_presentation_begin: ({ presentationId }) =>
      presentationId !== "azure-playback-blocked",
    master_stop_presentation_end: null,
    recording_state: "recording",
    master_stop_state: "clear",
  });
  await p.type("Azure boundary");
  await tick(40);
  check("Azure playback boundary=false 時沒有 play", p.audioPlays() === 0, p.audioPlays());
  check(
    "被拒的 Azure playback lease 有 begin/end",
    p.invokes.some(
      ({ cmd, arg }) =>
        cmd === "master_stop_presentation_begin" &&
        arg?.presentationId === "azure-playback-blocked",
    ) &&
      p.invokes.some(
        ({ cmd, arg }) =>
          cmd === "master_stop_presentation_end" &&
          arg?.presentationId === "azure-playback-blocked",
      ),
    p.invokes,
  );
}

console.log("81g. Azure 播放中觀察到外部 Stopping，先 pause/remove source 再 end lease");
{
  let masterStopState = "clear";
  const p = await open({
    azure_tts_read: AZURE_READY,
    azure_tts_speak: {
      generation: 8,
      content_type: "audio/mpeg",
      audio_bytes: 3,
      data_url: "data:audio/mpeg;base64,AQID",
      presentation_id: "azure-stop-event",
    },
    ask: answer({ hits: [hit({ snippet: "AZURE_STOP_EVENT_BODY" })] }),
    master_stop_presentation_begin: true,
    master_stop_presentation_end: null,
    recording_state: "recording",
    master_stop_state: () => masterStopState,
  });
  await p.type("播放中全停");
  check("前提：Azure 已開始播放且 lease 尚未 end", p.audioPlays() === 1 && p.isSpeaking());
  const pausesBefore = p.audioPauses();
  const traceBefore = p.playbackTrace().length;
  masterStopState = "stopping";
  await p.pollNow();
  const stopTrace = p.playbackTrace().slice(traceBefore);
  const pauseAt = stopTrace.indexOf("pause");
  const removeAt = stopTrace.indexOf("remove-src");
  const endAt = stopTrace.indexOf("end:azure-stop-event");
  check(
    "外部 CLI 的 poll 觀察到 Stopping，依 pause→remove→end 停播並交還 lease",
    p.audioPauses() > pausesBefore &&
      !p.isSpeaking() &&
      pauseAt >= 0 &&
      removeAt > pauseAt &&
      endAt > removeAt,
    { pauses: p.audioPauses(), stopTrace, invokes: p.invokes },
  );
}

console.log("82. desktop truth source contract 與三個 production callback self-mutations");
{
  const main = read(MAIN);
  const dispatch = read(MASTER_STOP_DISPATCH);
  const ui = read(SRC);
  const actual = desktopTruthSourceErrors(main, dispatch, ui);
  check("production source contract 全部接齊", actual.length === 0, actual);

  const withoutCallback = main.replace("dispatch_master_stop_menu(app, event.id.as_ref());", "");
  check(
    "self-mutation：刪 callback helper call 會紅",
    desktopTruthSourceErrors(withoutCallback, dispatch, ui).some((line) => line.includes("production callback")),
  );
  const swapped = dispatch
    .replace('"master-stop" => Some(MasterStopAction::Engage)', '"master-stop" => Some(MasterStopAction::Release)')
    .replace('"master-resume" => Some(MasterStopAction::Release)', '"master-resume" => Some(MasterStopAction::Engage)');
  check(
    "self-mutation：swap stop/resume 會紅",
    desktopTruthSourceErrors(main, swapped, ui).some((line) => line.includes("方向")),
  );
  const renamedNative = main.replace(
    'const MASTER_STOP_CHANGED_EVENT: &str = "master-stop-changed";',
    'const MASTER_STOP_CHANGED_EVENT: &str = "master-stop-renamed";',
  );
  check(
    "self-mutation：只改 native event 名會紅",
    desktopTruthSourceErrors(renamedNative, dispatch, ui).some((line) => line.includes("名稱不一致")),
  );
  const unregistered = main.replace("            master_stop_state,\n", "");
  check(
    "self-mutation：刪 Tauri command registration 會紅",
    desktopTruthSourceErrors(unregistered, dispatch, ui).some((line) => line.includes("沒有註冊")),
  );
  const ungatedGatekeeper = main.replace(
    '    let master_stop_admission = admit_desktop_brain(shell.data_dir.as_deref(), "守門員這一輪")?;\n',
    "",
  );
  check(
    "self-mutation：刪 Gatekeeper admission 會紅",
    desktopTruthSourceErrors(ungatedGatekeeper, dispatch, ui).some((line) => line.includes("gatekeeper_check")),
  );
  const noPresentationCommit = ui.replace("commitNativePresentation(answer", "commitWithoutFence(answer");
  check(
    "self-mutation：ask 繞過 renderer presentation commit 會紅",
    desktopTruthSourceErrors(main, dispatch, noPresentationCommit).some((line) => line.includes("ask Promise")),
  );
}

console.log("83. 第一次開啟直接在主對話逐張問；無效回答不寫入，不同意也不會下次再追問");
{
  let state = consentView([false, false, false, false], [false, false, false, false]);
  const writes = [];
  const p = await open({
    consent_read: () => structuredClone(state),
    consent_set: ({ key, granted }) => {
      writes.push({ key, granted });
      const sheet = state.sheets.find((candidate) => candidate.key === key);
      if (!sheet) throw new Error("不存在的同意書");
      sheet.reviewed = true;
      sheet.effective = granted;
      sheet.granted_at = granted ? 1_755_000_010_000 + writes.length : null;
      state.allows_recording = state.sheets[0].effective;
      state.allows_frames = state.sheets[2].effective;
      return structuredClone(state);
    },
  });
  check(
    "選角後就是第一張完整條文，提示明講按鈕與打字",
    !p.consentGuide().hidden &&
      p.consentProgress() === "同意書 1 / 4" &&
      p.consentWording() === state.sheets[0].wording &&
      p.node("[data-consent-prompt]").textContent.includes("同意") && p.node("[data-consent-prompt]").textContent.includes("打字"),
    {
      hidden: p.consentGuide().hidden,
      progress: p.consentProgress(),
      wording: p.consentWording(),
      placeholder: p.input().placeholder,
    },
  );
  await p.type("也許");
  check(
    "不是精確答案就不寫入任何權限",
    writes.length === 0 && p.consentResult().includes("不會改動同意書"),
    { writes, result: p.consentResult() },
  );
  await p.type("不同意");
  await p.type("同意");
  await p.type("我同意");
  await p.type("先不要");
  check(
    "四張依固定順序各寫一次，第一、第四張拒絕仍算已回答但權限保持關閉",
    JSON.stringify(writes) ===
      JSON.stringify([
        { key: "local-recording", granted: false },
        { key: "cloud-reading", granted: true },
        { key: "frame-storage", granted: true },
        { key: "azure-tts", granted: false },
      ]) &&
      state.sheets.every((sheet) => sheet.reviewed) &&
      state.sheets[0].effective === false &&
      state.sheets[3].effective === false,
    { writes, sheets: state.sheets },
  );
  check(
    "問完收起導覽並留下齒輪可再查看的完成訊息",
    p.consentGuide().hidden &&
      p.hitTexts().some((line) => line.includes("上方齒輪查看或更改")),
    { guideHidden: p.consentGuide().hidden, hits: p.hitTexts() },
  );
}

console.log("84. CLI 回 consent_required 時保留原問題；補答後同一句自動重送給 Grok");
{
  const state = consentView();
  const asked = [];
  let first = true;
  const p = await open({
    consent_read: () => structuredClone(state),
    consent_set: ({ key, granted }) => {
      const sheet = state.sheets.find((candidate) => candidate.key === key);
      sheet.reviewed = true;
      sheet.effective = granted;
      sheet.granted_at = granted ? 1_755_000_020_000 : null;
      return structuredClone(state);
    },
    ask: ({ question }) => {
      asked.push(question);
      if (first) {
        first = false;
        state.sheets[1].reviewed = false;
        state.sheets[1].effective = false;
        state.sheets[1].granted_at = null;
        return answer({ brain: { state: "consent_required", provider: "Grok CLI" } });
      }
      return answer({
        brain: { state: "used", provider: "Grok CLI" },
        hits: [hit({ snippet: "GROK_RETRIED_THE_ORIGINAL_QUESTION" })],
      });
    },
  });
  await p.type("我昨天在做什麼");
  check(
    "第二張失效時改問第二張，原問題沒有先被清掉或改寫",
    p.consentProgress() === "同意書 2 / 4" &&
      asked.length === 1 &&
      asked[0] === "我昨天在做什麼",
    { progress: p.consentProgress(), asked },
  );
  await p.type("同意");
  check(
    "答完後 Grok 收到完全相同的原問題第二次，畫面換成它完成的答案",
    JSON.stringify(asked) === JSON.stringify(["我昨天在做什麼", "我昨天在做什麼"]) &&
      p.hitTexts().some((line) => line.includes("GROK_RETRIED_THE_ORIGINAL_QUESTION")) &&
      p.hitTexts().some((line) => line.includes("Grok CLI · 已使用本機記憶")),
    { asked, hits: p.hitTexts() },
  );
}

console.log("85. 條文朗讀預設關閉，trusted click 開啟後同一顆開關停得下來");
{
  const p = await open(
    {
      consent_read: consentView([false, false, false, false], [false, false, false, false]),
      persona_fixed_voice_admit: { presentation_id: "consent-voice" },
      master_stop_presentation_begin: true,
      master_stop_presentation_end: null,
    },
    { consentVoices: consentVoiceManifest() },
  );
  await p.clickElement(p.consentListen(), { trusted: false });
  check(
    "合成 click 不取得 native admission、也不播放",
    p.audioPlays() === 0 &&
      !p.invokes.some(({ cmd }) => cmd === "persona_fixed_voice_admit"),
    { plays: p.audioPlays(), invokes: p.invokes },
  );
  await p.clickElement(p.consentListen());
  check(
    "使用者明確按朗讀才播放目前角色的第一張錄音",
    p.audioPlays() === 1 &&
      p.node("[data-persona-audio]").src ===
        "./persona-consent-voices/v1/chatgpt/local-recording.ogg" &&
      p.invokes.some(({ cmd }) => cmd === "persona_fixed_voice_admit"),
    { plays: p.audioPlays(), src: p.node("[data-persona-audio]").src, invokes: p.invokes },
  );
  check(
    "正在念的時候，按鈕自己說得出現在按下去會停",
    p.consentListen().textContent.includes("按下關閉") && p.consentListen().disabled !== true,
    p.consentListen().textContent,
  );

  // 合成 click 不准播，也不准停。停止本身不產生輸出，但這顆按鈕只有一條路，
  // 把 trusted 檢查留在最前面才不會多開一個「頁面上的腳本按得到」的分支。
  const playingPauses = p.audioPauses();
  await p.clickElement(p.consentListen(), { trusted: false });
  check(
    "播放中的合成 click 既不重播也不停",
    p.audioPlays() === 1 && p.audioPauses() === playingPauses,
    { plays: p.audioPlays(), pauses: p.audioPauses() },
  );

  // alpha.143 之後第二張是 32.8–46.0 秒（中位 38.1），第四張 19.1–28.8 秒。
  // 再按一次必須是**停止**，不是從 0 重播——重播的話他按了想中斷的那一下，
  // 反而讓自己又要從頭聽一遍，而畫面上沒有第二個出口（回答那張、換角色、
  // 全停、關視窗）。
  // lease 要數，不要 `includes`。重播那條路自己也會 end 一次舊的，所以
  // 「清單裡出現過 end」在壞的那一邊同樣是真的——要問的是**這一下**有沒有還。
  const endsOf = (view) =>
    view.playbackTrace().filter((step) => step === "end:consent-voice").length;
  const endsBefore = endsOf(p);
  await p.clickElement(p.consentListen());
  check(
    "播放中再按一次是停止，不是從頭重播",
    p.audioPlays() === 1 &&
      p.audioPauses() > playingPauses &&
      endsOf(p) > endsBefore &&
      !p.isSpeaking(),
    { plays: p.audioPlays(), pauses: p.audioPauses(), trace: p.playbackTrace() },
  );
  check(
    "停下來之後按鈕自己說得出現在按下去會播",
    p.consentListen().textContent.includes("朗讀關閉") &&
      !p.consentListen().textContent.includes("按下關閉"),
    p.consentListen().textContent,
  );

  // 反向那一刀。少了它，「停止＝把按鈕停用掉」也會是綠的，而那是另一種壞法：
  // 他停下來之後就再也念不了了。
  await p.clickElement(p.consentListen());
  check(
    "停止之後還能再按一次重新念",
    p.audioPlays() === 2,
    { plays: p.audioPlays(), trace: p.playbackTrace() },
  );

  // 自己念完只收掉播放狀態，不能把使用者保存的朗讀選擇關掉。
  // 關閉再開啟仍能重新播放。
  p.finishAudio();
  check(
    "自己念完之後開關仍開著",
    p.consentListen().textContent.includes("朗讀開啟") &&
      p.consentListen().dataset["aria-pressed"] === "true",
    p.consentListen().textContent,
  );
  await p.clickElement(p.consentListen());
  await p.clickElement(p.consentListen());
  check("念完之後關閉再開仍然播得出來", p.audioPlays() === 3, {
    plays: p.audioPlays(),
    trace: p.playbackTrace(),
  });

  // 條文改版、錄音還是舊的那一種。`consentClipFor()` 比的是逐字稿等不等於 native
  // 條文，對不上就不准播——而這顆按鈕**一畫出來**就要是灰的，不能等他按下去才
  // 發現沒事發生。這一刀補的是「換條文時畫一次」那個呼叫端：兩份重複的畫法收成
  // 一支之後，沒人守的呼叫端會安靜地從畫面上消失。
  const stale = consentVoiceManifest();
  for (const clip of stale.clips) clip.text = `${clip.text}（上一版的條文）`;
  const mismatched = await open(
    {
      consent_read: consentView([false, false, false, false], [false, false, false, false]),
      persona_fixed_voice_admit: { presentation_id: "consent-voice" },
      master_stop_presentation_begin: true,
      master_stop_presentation_end: null,
    },
    { consentVoices: stale },
  );
  check(
    "逐字稿對不上這一版條文時，朗讀鍵一出現就是灰的並且說明原因",
    mismatched.consentListen().disabled === true &&
      mismatched.consentListen().title.includes("沒有相符的本機錄音") &&
      mismatched.consentListen().textContent.includes("朗讀關閉"),
    {
      disabled: mismatched.consentListen().disabled,
      title: mismatched.consentListen().title,
      text: mismatched.consentListen().textContent,
    },
  );
  await mismatched.clickElement(mismatched.consentListen());
  check(
    "灰掉的朗讀鍵按下去不取 native admission、也不播",
    mismatched.audioPlays() === 0 &&
      !mismatched.invokes.some(({ cmd }) => cmd === "persona_fixed_voice_admit"),
    { plays: mismatched.audioPlays(), invokes: mismatched.invokes },
  );

  /*
   * 這兩個標籤和驗收清單上抄的那兩句，必須是同一個字串。
   *
   * `check-checklist-quotes-exist.py` 守不到它們：那支腳本的 `MIN = 8` 把短引號
   * 當名詞跳過，而「■ 停止朗讀」只有 6 個字。它自己的檔頭寫過為什麼這件事比
   * 假綠貴——他照著清單去找一顆寫著舊字的鍵，找不到，然後回報一個沒有壞的東西
   * 壞了。所以在這裡補：**從 app.js 讀出來比**，不要在測試裡另抄一份。
   */
  const labelOf = (name) =>
    new RegExp(`const ${name} = "([^"]+)";`).exec(read(SRC))?.[1] ?? null;
  const play = labelOf("CONSENT_LISTEN_PLAY");
  const stop = labelOf("CONSENT_LISTEN_STOP");
  const checklist = read(resolve(UI, "../../../docs/WINDOWS-CHECKLIST.md"));
  check(
    "兩個標籤都還在，而且驗收清單上逐字抄的是同一組",
    play !== null &&
      stop !== null &&
      play !== stop &&
      checklist.includes(`「${play}」`) &&
      checklist.includes(`「${stop}」`),
    { play, stop },
  );
  const labelled = await open(
    {
      consent_read: consentView([false, false, false, false], [false, false, false, false]),
      persona_fixed_voice_admit: { presentation_id: "consent-voice" },
      master_stop_presentation_begin: true,
      master_stop_presentation_end: null,
    },
    { consentVoices: consentVoiceManifest() },
  );
  const before = labelled.consentListen().textContent;
  await labelled.clickElement(labelled.consentListen());
  check(
    "畫面上那顆鍵的兩個樣子就是那兩個常數，沒有第三份文案",
    before === play && labelled.consentListen().textContent === stop,
    { before, playing: labelled.consentListen().textContent, play, stop },
  );

  // 停下來那句話寫在 `[data-consent-result]` 上，而那一格還有別人的訊息。
  // 「只收自己寫的那一句」是 `clearPlaybackStoppedLines()` 的承諾，所以要有牙齒：
  // 他打錯字那句是他還需要看的，不可以被下一次朗讀順手抹掉。
  await labelled.clickElement(labelled.consentListen());
  check(
    "按停之後那一格說得出它停了",
    labelled.consentResult().includes("已停止"),
    labelled.consentResult(),
  );
  await labelled.type("也許");
  check(
    "前提：現在那一格是輸入無效那句話",
    labelled.consentResult().includes("不會改動同意書"),
    labelled.consentResult(),
  );
  await labelled.clickElement(labelled.consentListen());
  check(
    "再開始朗讀不會把別人的訊息一起抹掉",
    labelled.consentResult().includes("不會改動同意書"),
    labelled.consentResult(),
  );
}

console.log("86. 主視窗齒輪直接開設定，不必再去系統匣找");
{
  const p = await open({ open_settings: null });
  await p.click("#settings");
  check(
    "齒輪只呼叫既有 open_settings command",
    p.invokes.filter(({ cmd }) => cmd === "open_settings").length === 1,
    p.invokes,
  );
}

console.log("87. 圖示不靠字型，拖她的時候不會順便讓她說話");
{
  // 起因：alpha.127 的 `⚙`（U+2699）在 Ted 的 WebView2 上整顆沒畫出來，而同一份
  // HTML 在無頭 Chromium 上看得到。CSP 是 `default-src 'none'` 又沒有 `font-src`，
  // 這個視窗一個字型都載不進來，所以符號顯不顯示得出來完全看那台機器的系統字型。
  // 改成 inline SVG 之後這個問題沒有了——但只要有人改回 `textContent`，SVG 會被
  // 整個洗掉、而且畫面在**這台**機器上看起來還是對的。所以要用原始碼守。
  const html = read(join(UI, "index.html"));
  const css = read(join(UI, "styles.css"));
  const js = read(SRC);
  const capabilities = read(join(UI, "../src-tauri/capabilities/pet-drag.json"));
  const errors = [];

  for (const id of ["pause", "timeline", "settings", "pin", "hide"]) {
    const button = new RegExp(`<button[^>]*id="${id}"[\\s\\S]*?</button>`).exec(html)?.[0] ?? "";
    if (!button) errors.push(`#${id} 這顆按鈕不見了`);
    else if (!button.includes("<svg")) errors.push(`#${id} 又變回字型符號了`);
  }
  for (const id of ["pause", "pin"]) {
    const button = new RegExp(`<button[^>]*id="${id}"[\\s\\S]*?</button>`).exec(html)?.[0] ?? "";
    if (!button.includes("icon-off") || !button.includes("icon-on")) {
      errors.push(`#${id} 是 toggle，兩個狀態的圖示都要在按鈕裡`);
    }
  }
  // 這兩行正是會把 SVG 洗掉的那一種寫法。
  if (/pauseButton\.textContent\s*=/.test(js)) errors.push("pauseButton 又用 textContent 換圖示了");
  if (/pinButton\.textContent\s*=/.test(js)) errors.push("pinButton 又用 textContent 換圖示了");
  // `aria-pressed` 是兩態的唯一真相來源，CSS 只是它的投影。
  if (!css.includes('.control[aria-pressed="true"] .icon-on')) {
    errors.push("CSS 沒有用 aria-pressed 選圖示");
  }

  // 拖曳。`core:window` 的 default 不含 start-dragging，少了這張白名單，
  // 角色身上的拖曳會安靜地什麼都不做。
  if (!capabilities.includes("core:window:allow-start-dragging")) {
    errors.push("pet-drag capability 沒有給 start-dragging");
  }
  if (!js.includes("DRAG_THRESHOLD_PX")) errors.push("拖曳沒有移動門檻，點一下就會被當成拖");
  if (!js.includes("startDragging")) errors.push("沒有人真的發動視窗拖曳");
  if (!css.includes("cursor: grab")) errors.push("沒有游標提示，使用者不會知道她拖得動");

  check("圖示與拖曳的接線都還在", errors.length === 0, errors);
}

// 拖曳／點擊的**行為**測試在 `check-persona.mjs` 的 ②b，不在這裡。這支 gate 的
// 夾具餵不出台詞：`dialogueTaps()` 要求語音 manifest 每位角色剛好 32 句（2 tap、
// 30 reply）且時長與位元組對得上，湊不齊就回空的，於是「拖她不會說話」會變成
// 兩個空字串相等的假綠。要加拖曳的行為斷言就去 `check-persona.mjs`，那邊的
// `persona("chatgpt")` 本來就有真台詞。上面第 87 節守的是接線還在。

console.log("88. 三顆播放鍵的停止規則一致，而下一顆躲不掉這條規則");
{
  // SPEC 現在宣稱「三顆播放鍵（同意書條文、本機答案、Azure 答案）的規則一致：
  // 第二下停止，不是重播」。三顆**各自**的行為在別處都有人守（同意書 §85、
  // Azure §58、本機朗讀在 `check-persona.mjs`）——缺的是橫的那一條：沒有任何
  // 東西擋得住「第四顆播放鍵不加停止就上線」，而 SPEC 那句話會替它作保。
  //
  // 所以這一節不從我手抄的清單出發，從 app.js 的**原始碼**出發：成對的
  // `const X_PLAY` ／ `const X_STOP` 就是播放鍵的花名冊，每一個前綴都得有一個
  // driver。driver 只負責「把畫面開起來，交出一顆閒著的按鈕和它的播放計數」，
  // **斷言一條都不在 driver 裡**——所以補一個空殼 driver 過不了關。
  // 見 `an-exhaustive-tripwire-makes-you-look-not-update`：窮舉 tripwire 只逼
  // 你看一眼，不逼你補；逼你補的是「那顆按鈕真的要被按兩下」。
  const labels = new Map();
  for (const [, prefix, role, text] of read(SRC).matchAll(
    /const ([A-Z][A-Z0-9_]*)_(PLAY|STOP) = "([^"]*)";/gu,
  )) {
    labels.set(prefix, { ...(labels.get(prefix) ?? {}), [role]: text });
  }
  const checklist = read(resolve(UI, "../../../docs/WINDOWS-CHECKLIST.md"));

  const ZH_TW_VOICE = [{ name: "Hanhan", lang: "zh-TW", localService: true }];
  // 藏起來要算「什麼都沒說」。`hidden` 沒拿掉而字還留著，讀 `textContent` 會拿到
  // 上一次的句子，而畫面上一個字都看不到。
  const personaLineOf = (view) => {
    const el = view.node("[data-persona-line]");
    return el.hidden ? "" : el.textContent;
  };
  const drivers = {
    CONSENT_LISTEN: async () => {
      const view = await open(
        {
          consent_read: consentView([false, false, false, false], [false, false, false, false]),
          persona_fixed_voice_admit: { presentation_id: "consent-voice" },
          master_stop_presentation_begin: true,
          master_stop_presentation_end: null,
        },
        { consentVoices: consentVoiceManifest() },
      );
      return {
        view,
        button: view.consentListen(),
        plays: () => view.audioPlays(),
        said: () => view.consentResult(),
      };
    },
    ANSWER_READ: async () => {
      const view = await open(
        {
          // 本機朗讀要兩個前提：設定裡「本機聲音」開著，以及這台機器真的有一支
          // `localService` 的中文 voice。少任何一個，那顆鍵會改口說為什麼不念。
          persona_read: { id: "chatgpt", enabled: true, motion: true, tap_lines: true, voice_enabled: true },
          ask: answer({ hits: [hit({ snippet: "READ_ME" })] }),
          recording_state: "recording",
        },
        { systemVoices: ZH_TW_VOICE },
      );
      await view.type("念這一段");
      return {
        view,
        button: view.localReadButton(),
        plays: () => view.localSpeaks(),
        said: () => personaLineOf(view),
      };
    },
    AZURE_READ: async () => {
      const view = await open({
        azure_tts_read: AZURE_READY,
        azure_tts_speak: ({ expected }) => ({
          generation: expected.generation + 1,
          content_type: "audio/mpeg",
          audio_bytes: 3,
          data_url: "data:audio/mpeg;base64,AQID",
          presentation_id: `880${expected.generation}`,
        }),
        ask: answer({ hits: [hit({ snippet: "PLAY_ME" })] }),
        recording_state: "recording",
      });
      await view.type("播放");
      // 新答案的 MP3 回來會自動播一次（§58），所以這顆鍵一開始就是停止鍵。
      // 這一節量的是「同一顆鍵按兩下」，先讓那一段自己播完交還成閒置狀態。
      view.finishAudio();
      await tick();
      return {
        view,
        button: view.azureButton(),
        plays: () => view.audioPlays(),
        said: () => personaLineOf(view),
      };
    },
  };

  const noDriver = [...labels.keys()].filter((prefix) => !Object.hasOwn(drivers, prefix));
  const noLabels = Object.keys(drivers).filter((prefix) => !labels.has(prefix));
  check(
    "每一對 *_PLAY／*_STOP 都有人真的去按它",
    noDriver.length === 0 && noLabels.length === 0,
    { 沒有driver: noDriver, 沒有常數: noLabels, 找到的: [...labels.keys()] },
  );
  // 「一致」是一句關於複數的話。只剩一顆的時候這一整節會全綠而什麼都沒證明。
  check("花名冊上不只一顆鍵", labels.size >= 3, [...labels.keys()]);

  // 花名冊是靠**命名**找到的，所以它有一個看得見的極限：寫成字面值的按鈕它看不
  // 到——Azure 那顆在這一版之前就正是那樣，三處字面值抄來抄去。這一條把極限補
  // 回大半：這三個記號只准出現在 `*_PLAY`／`*_STOP` 的值裡，有人手寫一顆新的
  // 播放鍵會先撞到這裡。
  //
  // **剩下的那一半仍然是假的**：一顆連記號都不用的播放鍵，這一節到現在還是看
  // 不到它。這句自白是打過刀量出來的，不是我猜的——同一顆按鈕加記號這一整節會
  // 紅，拿掉記號就整節全綠。
  const known = new Set([...labels.values()].flatMap((pair) => Object.values(pair)));
  const loose = [...read(SRC).matchAll(/"[^"\n]*[\u{1F50A}\u{2601}\u{25A0}][^"\n]*"/gu)]
    .map(([literal]) => literal.slice(1, -1))
    .filter((text) => !known.has(text));
  check("播放鍵的記號沒有散落在常數以外的地方", loose.length === 0, loose);

  for (const [prefix, pair] of labels) {
    const play = pair.PLAY ?? null;
    const stop = pair.STOP ?? null;
    check(
      `${prefix}：播放與停止是兩個不同的字串`,
      play !== null && stop !== null && play !== stop,
      pair,
    );
    // 兩臂吐出一模一樣的字串時，分辨它們的那個條件就是零覆蓋——而這裡的下游
    // 是他的眼睛，不是另一支程式。真機 checklist 要查得到這兩句話，他才知道
    // 該看到什麼。
    check(
      `${prefix}：兩句話都寫進真機 checklist`,
      play !== null && stop !== null && checklist.includes(play) && checklist.includes(stop),
      { play, stop, hasPlay: checklist.includes(play), hasStop: checklist.includes(stop) },
    );

    const drive = drivers[prefix];
    if (typeof drive !== "function") continue;
    const { view, button, plays, said } = await drive();
    if (!button) {
      check(`${prefix}：driver 交得出那顆按鈕`, false, null);
      continue;
    }
    check(`${prefix}：閒著的時候鍵面寫的是播放`, button.textContent === play, button.textContent);
    const idle = plays();
    const started = await view.clickElement(button);
    check(
      `${prefix}：一下 trusted click 開始播，鍵面翻成停止`,
      started && plays() === idle + 1 && button.textContent === stop,
      { started, before: idle, after: plays(), label: button.textContent },
    );
    const playing = plays();
    const stopped = await view.clickElement(button);
    check(
      `${prefix}：第二下是停止，不是重播`,
      stopped && plays() === playing && button.textContent === play,
      { stopped, before: playing, after: plays(), label: button.textContent },
    );
    // 鍵面翻回去是看得見，但他的眼睛不一定在按鈕上。三顆都要開口說它停了。
    check(`${prefix}：停下來會說一聲`, said().includes("已停止"), said());
    // 停止鍵不可以把自己鎖死——停完那顆鍵要能再播一次，不然「停止」就變成
    // 「這一題從此不能再聽」。
    const replayed = await view.clickElement(button);
    check(
      `${prefix}：停完還能再播一次`,
      replayed && plays() === playing + 1 && button.textContent === stop,
      { replayed, before: playing, after: plays(), label: button.textContent },
    );
    // 那句「已停止」不可以掛在一顆正在播的按鈕底下。
    check(`${prefix}：再播的時候那句「已停止」收掉了`, !said().includes("已停止"), said());
  }
}

console.log("89. 條文錄音播不出來的時候，不准改用系統聲音把條文念掉");
{
  // SPEC：同意書朗讀出廠關閉，一次 trusted click 開啟後每張各播一次，不必逐張再按；
  // 關閉立即停止，選擇隨 config 保存至重開後，逐字稿不符就停用當張。
  // 不借 Azure 或 `localService`；開關由 A149 守著，這一節守不借其他聲音。
  // 隔壁那條路（角色點擊台詞）**故意**有 localService fallback（見真機清單
  // alpha.108 那一項：「single voice read 回 null／cache stale／audio play 拒絕，
  // 確認同一次 trusted click 會嘗試 localService fallback」）。兩條路現在是分開
  // 寫的，所以這句話是真的；擋不住的是「有人把它們合併」。
  //
  // 這裡最重要的是**夾具要讓 fallback 真的做得到**，否則 `localSpeaks() === 0`
  // 是一條沒有牙齒的斷言：角色的「本機聲音」要開著、而且這台機器要有一支
  // `localService` 的中文 voice。兩個前提都給足了，她仍然必須閉嘴。
  const voiced = {
    persona_read: {
      id: "chatgpt",
      enabled: true,
      motion: true,
      tap_lines: true,
      voice_enabled: true,
    },
    consent_read: consentView([false, false, false, false], [false, false, false, false]),
    persona_fixed_voice_admit: { presentation_id: "consent-voice" },
    master_stop_presentation_begin: true,
    master_stop_presentation_end: null,
  };
  const options = {
    consentVoices: consentVoiceManifest(),
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
  };

  // 鍵面那句話從 app.js 讀出來比，不要在這裡另抄一份（理由見 §85）。
  const playLabel = /const CONSENT_LISTEN_PLAY = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;

  const failed = await open(voiced, options);
  await failed.clickElement(failed.consentListen());
  check("前提：條文錄音真的開始播了", failed.audioPlays() === 1, failed.audioPlays());
  failed.failAudio();
  await tick();
  check(
    "錄音播放失敗不改用系統聲音把條文念掉",
    failed.localSpeaks() === 0,
    failed.localSpeaks(),
  );
  check(
    "失敗要講出來，而且指向「自己讀文字」這條出口",
    failed.consentResult().includes("播放失敗") && failed.consentResult().includes("讀文字"),
    failed.consentResult(),
  );
  // 針取 `azure_tts_speak` 不是 `azure_*`：開機本來就會讀一次 Azure 設定狀態
  // （`azure_tts_read`，純本機、不送東西），SPEC 那句話講的是**念**。
  check(
    "錄音播放失敗也不借 Azure 念條文",
    !failed.invokes.some(({ cmd }) => cmd === "azure_tts_speak"),
    failed.invokes.map(({ cmd }) => cmd),
  );
  check(
    "失敗之後開關仍開著，還按得動",
    failed.consentListen().textContent !== playLabel &&
      failed.consentListen().disabled !== true,
    { label: failed.consentListen().textContent, disabled: failed.consentListen().disabled },
  );

  // 「逐字稿對不上就灰掉」那一格別去那樣驗：`clickElement` 和真的瀏覽器一樣不會
  // 把 click 送進一顆 disabled 的按鈕，所以在那裡寫 `localSpeaks() === 0` 是一條
  // 永遠成立、什麼都沒看的斷言。那一格的牙齒在 §85（鍵真的是灰的）。
  //
  // 擋「有人把兩條路合併」要用原始碼守，因為那條路現在**還不存在**，行為測試看
  // 不到不存在的東西。`body !== ""` 是 fail-closed：錨點鏽掉要紅，不要安靜略過。
  const body = /async function playConsentSheet\(\) \{[\s\S]*?\n\}/u.exec(read(SRC))?.[0] ?? "";
  check(
    "條文朗讀那條路上只有 bundled Ogg 一種聲音",
    body !== "" &&
      !body.includes("speakWithLocalSystemVoice") &&
      !body.includes("azure_tts_speak"),
    { anchored: body !== "", length: body.length },
  );
}

console.log("90. 他照著那句話去打開「本機聲音」之後，那句話要消失");
{
  // 「先到設定打開『本機聲音』，我才會朗讀。」是一則**指示**，不是狀態描述。
  // 他照做，`persona-changed` 回來，那一刻這句話就變成假的了——而畫面還掛著
  // 一句「你還沒做」，他會以為沒生效、再去點一次設定。
  const voiceOff = (on) => ({
    id: "chatgpt",
    enabled: true,
    motion: true,
    tap_lines: true,
    voice_enabled: on,
  });
  const line = () => {
    const el = p.node("[data-persona-line]");
    return el.hidden ? "" : el.textContent;
  };
  const instruction = /const ANSWER_READ_VOICE_OFF = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
  const p = await open(
    {
      persona_read: voiceOff(false),
      ask: answer({ hits: [hit({ snippet: "READ_ME" })] }),
      recording_state: "recording",
    },
    // 機器上**有**一支繁中 voice：這樣「不念」的唯一理由就是那個設定關著，
    // 而不是這個夾具本來就念不出來。
    { systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }] },
  );
  await p.type("念這一段");
  await p.clickElement(p.localReadButton());
  check(
    "本機聲音關著時不念，而且說得出要去哪裡打開",
    p.localSpeaks() === 0 && instruction !== null && line() === instruction,
    { speaks: p.localSpeaks(), line: line(), instruction },
  );
  // 上面那條是從原始碼把那句話讀出來比的，所以它擋得住「文案漂掉」，擋不住
  // 「文案還在但不指路」——打一刀 `want=綠` 問過：把它換成「現在不能朗讀。」
  // 整節全綠。這一條補上那半：他要找的是**設定**裡的**本機聲音**。
  check(
    "那句話真的指得出路（設定 ＋ 本機聲音）",
    instruction !== null && instruction.includes("設定") && instruction.includes("本機聲音"),
    instruction,
  );
  // 真機清單逐字抄了同一句。`check-checklist-quotes-exist.py` 幫不上這一句——
  // 它的 `QUOTE` 是 `「[^「」]+」`，而這句話自己裡面就有一對「」，永遠配不出來
  // （內層那個「本機聲音」又只有四個字，被 `MIN = 8` 跳過）。所以在這裡綁。
  check(
    "驗收清單上逐字抄的是同一句",
    instruction !== null &&
      read(resolve(UI, "../../../docs/WINDOWS-CHECKLIST.md")).includes(instruction),
    instruction,
  );

  // 收乾淨它的是 `applyPersona()` 每次都跑的 `clearPersonaLine()`，不是一段只認
  // 這句話的程式——我原本寫了那一段，打一刀 `want=綠` 才問出來它是死碼。所以這
  // 裡守的是**結局**（他照做之後那句話不在了，而且真的念得出來），不是機制。
  await p.fromOutside("persona-changed", voiceOff(true));
  check("他照做之後那句話不見了", line() === "", line());
  await p.clickElement(p.localReadButton());
  check(
    "而且這一下真的念出來了（不是只有字消失）",
    p.localSpeaks() === 1 && p.localReadButton().textContent.includes("停止"),
    { speaks: p.localSpeaks(), label: p.localReadButton().textContent },
  );
}

console.log("91. 「去開當時的畫面」開不起來的時候，這一頁要說一聲");
{
  // SPEC §8.2 把出處 chip 寫成〔定案〕：「點開 = 嘗試讀當時畫面；檔案若在點擊前
  // 被外部移走會明確失敗」。那個「明確」分成兩段路，而只有前一段有人守：
  //
  //   視窗開起來之後 → 圖是半截的／過了保留期／檔案不見了 → `frame.js` 在那扇
  //     視窗裡照實說（`check-frame-source.mjs` ③⑥ 守著，連表頭都要清掉）。
  //   視窗**根本沒開起來** → `open_frame` 是 `Result<(), String>`，照實回錯 →
  //     以前四個呼叫端各自 `void invoke?.(…)`，沒有人 `.catch`，這一頁也沒有
  //     `unhandledrejection` 的接口。那個錯掉在地上，畫面上一個字都沒有。
  //
  // 第二段是唯一一種「沒有視窗可以拿來說話」的失敗，所以話只能說在他剛按下去的
  // 這一頁上。而既有的閘門一條都抓不到：它們數的是 `open_frame` 被叫了幾次、帶
  // 了哪個 frameId——**呼叫有發生、參數是對的、回傳值被丟掉**。
  // 見 `a-call-counting-gate-cannot-see-a-discarded-result`。
  //
  // 所以這一節兩層。第一層從原始碼出發：`open_frame` 這個字在 app.js 的產品碼
  // （非註解行）裡只准出現一次，就是 `openFrame` 那支 helper 裡面。新加第五顆
  // 「看當時的畫面」的鍵自己去叫 invoke，這一條當場紅。第二層真的去按，證明那
  // 支 helper 不是擺著好看的。
  const productLines = read(SRC)
    .split("\n")
    .map((line, i) => [i + 1, line])
    .filter(([, line]) => {
      const t = line.trim();
      return !t.startsWith("*") && !t.startsWith("//") && !t.startsWith("/*");
    });
  const callSites = productLines.filter(([, line]) => line.includes("open_frame"));
  check(
    "app.js 的產品碼裡只有一個地方叫得到 open_frame",
    callSites.length === 1,
    callSites,
  );
  check(
    "而那一行就在 openFrame 那支 helper 裡",
    /invoke\?\.\("open_frame"/u.test(callSites[0]?.[1] ?? "") &&
      read(SRC).includes("function openFrame(frameId) {"),
    callSites[0],
  );

  // 每一顆鍵：先把畫面開起來（`open_frame` 這一輪一定失敗），按下去，讀他看得到
  // 的那一格。driver 只負責「開畫面、交出一顆鍵」，**斷言一條都不在 driver 裡**
  // ——理由和 §88 一樣，補一個空殼 driver 要過不了關。
  const BOOM = "OPEN_FRAME_BLEW_UP_HERE";
  // 少了 `.catch`，那個 rejection 在 Node 底下會直接把這支殺掉——閘門是紅的，
  // 而**沒有任何一條斷言抓到它**：輸出在半路斷掉，剩下的節一條都沒跑。這個紅
  // 是 Node 的性質，不是產品的：真的 webview 裡沒有人會死，它只是安靜地什麼都
  // 不做，而那正是這一節要抓的東西。所以自己接住，把「那個錯掉在地上」變成一
  // 條有名字、印得出來的斷言。見 `a-red-run-can-be-red-for-the-wrong-reason`。
  const dropped = [];
  const onDropped = (err) => dropped.push(String(err?.message ?? err));
  process.on("unhandledRejection", onDropped);
  const ragFixture = (openFrameResult) => ({
    open_frame: openFrameResult,
    ask: answer({
      query_id: 7007,
      answers: [fact({ frame_id: 42, chunk_id: 31 })],
      hits: [hit({ chunk_id: 77, frame_id: 84, snippet: "隨便一段原文" })],
      synthesis: {
        sentences: [
          {
            text: "客服電話是 0800-080-123。",
            sources: [{ ref: "fact:9", label: "畫面 #42", frame_id: 42 }],
          },
        ],
      },
    }),
    recording_state: "recording",
  });
  const overviewFixture = (openFrameResult) => ({
    open_frame: openFrameResult,
    ask: answer({
      kind: "memory_overview",
      query_id: 99123,
      overview: {
        kind: "ready",
        cards: [overviewCard()],
        truncated: false,
        evidence_unavailable: 0,
      },
    }),
    recording_state: "recording",
  });
  const drivers = {
    "成句答案底下的本機出處": async (openFrameResult) => {
      const view = await open(ragFixture(openFrameResult));
      await view.type("客服電話");
      return { view, button: view.hits().querySelector(".grounded-source") };
    },
    "一般命中那一列（整列點得開）": async (openFrameResult) => {
      const view = await open(ragFixture(openFrameResult));
      await view.type("客服電話");
      // `openable` 是 `classList.add` 上去的，而這個假瀏覽器的 `.class` 選擇器
      // 只讀 `className`——`querySelector(".openable")` 在這裡永遠是 null。那不是
      // 「這一列點不開」，是儀器看不到，所以改問 classList 本人。
      return {
        view,
        button: view.hits().children.find((el) => el.classList.contains("openable")) ?? null,
      };
    },
    "記憶總覽卡片的畫面出處": async (openFrameResult) => {
      const view = await open(overviewFixture(openFrameResult));
      await view.type("你知道了什麼");
      return { view, button: view.hits().querySelector(".overview-evidence") };
    },
  };

  for (const [name, drive] of Object.entries(drivers)) {
    // 對照組先跑：開得起來的時候**不准**有話說。少了這一條，helper 寫成「每次都
    // 抱怨一句」也是綠的，而那比沉默更糟。
    const okRun = await drive(null);
    check(`${name}：這顆鍵找得到`, okRun.button !== null && okRun.button !== undefined, okRun.button);
    await okRun.view.clickElement(okRun.button);
    check(
      `${name}：開得起來的時候，這一頁不多嘴`,
      !okRun.view.line().includes("打不開"),
      okRun.view.line(),
    );
    check(
      `${name}：而且真的去開了`,
      okRun.view.invokes.some(({ cmd }) => cmd === "open_frame"),
      okRun.view.invokes.filter(({ cmd }) => cmd === "open_frame"),
    );

    const bad = await drive(new Error(BOOM));
    await bad.view.clickElement(bad.button);
    const said = bad.view.line();
    check(`${name}：開不起來要說一聲`, said.includes("打不開"), said);
    // 原話照抄，理由和 `frame.js` 那邊一樣：只有 Rust 分得出是哪一種開不起來，
    // 這一頁不准自己編一個成因。
    check(`${name}：而且照抄 Rust 給的理由`, said.includes(BOOM), said);
    // 主詞要對。開不起來的是那扇視窗，不是她——`noticeAboutHer` 會讓這句話變成
    // 她自己出了什麼事，而他下一步會去按「解除全停」。
    check(
      `${name}：說的不是她壞了`,
      !/她(現在)?(叫不起來|出了|壞)/u.test(said),
      said,
    );
  }

  // 兩拍：rejection 是下一個 microtask 才送到 `unhandledRejection` 的。
  await tick();
  await tick();
  check("沒有任何 open_frame 的錯掉在地上", dropped.length === 0, dropped);
  process.off("unhandledRejection", onDropped);

  /* 這一節抓不到什麼，講清楚——不然下一輪會有人以為它守住了整條路：
   *
   *   1. 泡泡底下那顆證據 chip（`.see`）這支夾具叫不出來，所以它只被上面第一層
   *      蓋到——而第一層管的是「有沒有人繞過 helper」，不是「這顆鍵還在不在」。
   *      把那顆 chip 的 click handler 整個拿掉，這一節照樣綠。（把它換回裸的
   *      invoke 倒是會紅，那是第一層抓的。）
   *   2. `timeline.js` 有它自己的三個呼叫端，不在這支的 SRC 裡，也不共用
   *      `noticeAboutSomethingElse`——那是另一頁、另一條路。第一層只掃 app.js，
   *      把那三個呼叫端全換掉，這一節一個字都不會說。那一頁後來補上了同一支
   *      helper 和它自己的回條格（`tell`），守它的是
   *      `check-timeline-forget.mjs` ⑫——**不是這一節**。
   *
   * 兩條都拿 `want=綠` 的刀量過，不是推出來的。 */
}

console.log("92. 停止、晚到的 play()、失敗後重按、重開出處、trusted 手勢");
{
  const consentFixture = {
    consent_read: consentView([false, false, false, false], [false, false, false, false]),
    persona_fixed_voice_admit: { presentation_id: "consent-voice" },
    master_stop_presentation_begin: true,
    master_stop_presentation_end: null,
  };
  const playLabel = /const CONSENT_LISTEN_PLAY = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
  const stopLabel = /const CONSENT_LISTEN_STOP = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
  const failedLabel = /const CONSENT_LISTEN_FAILED = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
  const personaLineOf = (view) => {
    const el = view.node("[data-persona-line]");
    return el.hidden ? "" : el.textContent;
  };

  {
    const pending = await open(consentFixture, {
      consentVoices: consentVoiceManifest(),
      holdPlay: true,
    });
    await pending.clickElement(pending.consentListen());
    check(
      "play() 還在飛的時候，鍵面已經是停止",
      pending.holdPlayPending() &&
        pending.consentListen().textContent === stopLabel &&
        !pending.isSpeaking(),
      {
        pending: pending.holdPlayPending(),
        label: pending.consentListen().textContent,
        speaking: pending.isSpeaking(),
      },
    );
    const pausesBefore = pending.audioPauses();
    await pending.clickElement(pending.consentListen(), { trusted: false });
    check(
      "pending 期間的合成 click 不停、不重播",
      pending.holdPlayPending() &&
        pending.audioPlays() === 1 &&
        pending.audioPauses() === pausesBefore &&
        pending.consentListen().textContent === stopLabel,
      {
        plays: pending.audioPlays(),
        pauses: pending.audioPauses(),
        label: pending.consentListen().textContent,
      },
    );
    await pending.clickElement(pending.consentListen());
    check(
      "pending 期間再按一下是停止，不是再開一段",
      pending.holdPlayPending() &&
        pending.audioPlays() === 1 &&
        pending.audioPauses() > pausesBefore &&
        pending.consentListen().textContent === playLabel &&
        pending.consentResult().includes("已停止"),
      {
        plays: pending.audioPlays(),
        pauses: pending.audioPauses(),
        label: pending.consentListen().textContent,
        said: pending.consentResult(),
        pendingPlay: pending.holdPlayPending(),
      },
    );
    const srcAfterStop = pending.audioSrc();
    await pending.releasePlay();
    check(
      "晚到的 play() 不准把已停止的錄音再點著",
      !pending.isSpeaking() &&
        pending.audioPlays() === 1 &&
        pending.consentListen().textContent === playLabel &&
        (pending.audioSrc() === "" || pending.audioSrc() === srcAfterStop),
      {
        speaking: pending.isSpeaking(),
        plays: pending.audioPlays(),
        src: pending.audioSrc(),
        label: pending.consentListen().textContent,
      },
    );
  }

  {
    const failed = await open(consentFixture, { consentVoices: consentVoiceManifest() });
    await failed.clickElement(failed.consentListen());
    failed.failAudio();
    await tick();
    check(
      "前提：失敗那句還在，開關保持開啟",
      failed.consentResult() === failedLabel &&
        failed.consentListen().textContent === stopLabel,
      { said: failed.consentResult(), label: failed.consentListen().textContent },
    );
    await failed.clickElement(failed.consentListen());
    await failed.clickElement(failed.consentListen());
    check(
      "失敗後關閉再開，那句失敗自己消失，而且真的在念",
      failed.audioPlays() === 2 &&
        failed.consentListen().textContent === stopLabel &&
        failed.consentResult() === "",
      {
        plays: failed.audioPlays(),
        label: failed.consentListen().textContent,
        said: failed.consentResult(),
      },
    );
  }

  {
    const ZH_TW_VOICE = [{ name: "Hanhan", lang: "zh-TW", localService: true }];
    const localFailed = /const ANSWER_READ_FAILED = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
    const localStop = /const ANSWER_READ_STOP = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
    let speaks = 0;
    const view = await open(
      {
        persona_read: { id: "chatgpt", enabled: true, motion: true, tap_lines: true, voice_enabled: true },
        ask: answer({ hits: [hit({ snippet: "READ_ME" })] }),
        recording_state: "recording",
      },
      { systemVoices: ZH_TW_VOICE },
    );
    const originalSpeak = globalThis.speechSynthesis.speak;
    globalThis.speechSynthesis.speak = function speakWithError(utterance) {
      speaks += 1;
      if (speaks === 1) {
        queueMicrotask(() => utterance?.onerror?.());
        return;
      }
      return originalSpeak.call(this, utterance);
    };
    await view.type("念這一段");
    await view.clickElement(view.localReadButton());
    await tick();
    check(
      "本機朗讀失敗要說出來",
      personaLineOf(view) === localFailed &&
        view.localReadButton().textContent.includes("用本機聲音朗讀"),
      { line: personaLineOf(view), label: view.localReadButton()?.textContent },
    );
    await view.clickElement(view.localReadButton());
    check(
      "失敗後再按，失敗句收掉，鍵面是停止",
      personaLineOf(view) === "" && view.localReadButton().textContent === localStop,
      { line: personaLineOf(view), label: view.localReadButton()?.textContent, speaks },
    );
  }

  {
    const azureFailed = /const AZURE_READ_FAILED = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
    const azureStop = /const AZURE_READ_STOP = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
    const azurePlay = /const AZURE_READ_PLAY = "([^"]+)";/u.exec(read(SRC))?.[1] ?? null;
    const pending = await open(
      {
        azure_tts_read: AZURE_READY,
        azure_tts_speak: ({ expected }) => ({
          generation: expected.generation + 1,
          content_type: "audio/mpeg",
          audio_bytes: 3,
          data_url: "data:audio/mpeg;base64,AQID",
          presentation_id: `880${expected.generation}`,
        }),
        ask: answer({ hits: [hit({ snippet: "PLAY_ME" })] }),
        recording_state: "recording",
      },
      { holdPlay: true },
    );
    await pending.type("播放");
    check(
      "新答案自動送出後，play() 未 resolve 前鍵面已是停止",
      pending.holdPlayPending() &&
        pending.azureButton()?.textContent === azureStop &&
        !pending.isSpeaking(),
      {
        pending: pending.holdPlayPending(),
        label: pending.azureButton()?.textContent,
        speaking: pending.isSpeaking(),
      },
    );
    await pending.clickElement(pending.azureButton());
    check(
      "pending Azure 再按是停止",
      pending.azureButton()?.textContent === azurePlay &&
        personaLineOf(pending).includes("已停止"),
      {
        label: pending.azureButton()?.textContent,
        line: personaLineOf(pending),
      },
    );
    await pending.releasePlay();
    check(
      "晚到的 Azure play() 不准再出聲",
      !pending.isSpeaking() && pending.azureButton()?.textContent === azurePlay,
      {
        speaking: pending.isSpeaking(),
        label: pending.azureButton()?.textContent,
        src: pending.audioSrc(),
      },
    );

    const replay = await open(
      {
        azure_tts_read: AZURE_READY,
        azure_tts_speak: ({ expected }) => ({
          generation: expected.generation + 1,
          content_type: "audio/mpeg",
          audio_bytes: 3,
          data_url: "data:audio/mpeg;base64,AQID",
          presentation_id: `990${expected.generation}`,
        }),
        ask: answer({ hits: [hit({ snippet: "PLAY_ME" })] }),
        recording_state: "recording",
      },
    );
    await replay.type("播放");
    replay.failAudio();
    await tick();
    check(
      "Azure 播放失敗要說出來",
      personaLineOf(replay) === azureFailed,
      personaLineOf(replay),
    );
    await replay.clickElement(replay.azureButton());
    check(
      "Azure 失敗後再按，失敗句收掉",
      personaLineOf(replay) === "" && replay.azureButton()?.textContent === azureStop,
      { line: personaLineOf(replay), label: replay.azureButton()?.textContent },
    );
  }

  for (const kind of ["consent", "fixed", "azure"]) {
    for (const replay of [false, true]) {
      const table = kind === "consent" ? consentFixture : {
        persona_read: { id: "chatgpt", enabled: true, motion: true, tap_lines: true, voice_enabled: true },
        persona_fixed_voice_admit: { presentation_id: "fixed-rejection" },
        master_stop_presentation_begin: true,
        master_stop_presentation_end: null,
        recording_state: "recording",
        ask: answer({ hits: [hit({ snippet: "LATE_REJECTION" })] }),
        ...(kind === "azure" ? {
          azure_tts_read: AZURE_READY,
          azure_tts_speak: ({ expected }) => ({
            generation: expected.generation + 1,
            content_type: "audio/mpeg", audio_bytes: 3,
            data_url: "data:audio/mpeg;base64,AQID",
            presentation_id: `rejection-${expected.generation}`,
          }),
        } : {}),
      };
      const view = await open(table, {
        holdFirstPlay: true,
        consentVoices: kind === "consent" ? consentVoiceManifest() : null,
        personaVoices: kind === "fixed"
          ? JSON.parse(read(join(UI, "persona-voices/v1/manifest.json"))) : null,
      });
      if (kind === "azure") await view.type("播放");
      else await view.clickElement(kind === "consent" ? view.consentListen() : view.node("[data-avatar]"));
      check(`${kind} 過期 rejection 前提：第一段 play 正在等`, view.holdPlayPending());
      if (kind === "consent") await view.clickElement(view.consentListen());
      else if (kind === "azure") await view.clickElement(view.azureButton());
      else await view.type("切換問題讓台詞停止");
      if (replay) {
        await view.clickElement(kind === "consent" ? view.consentListen()
          : kind === "azure" ? view.azureButton() : view.node("[data-avatar]"));
        check(`${kind} 第二段已開始`, view.audioPlays() === 2 && view.isSpeaking());
      }
      const before = {
        line: personaLineOf(view), consent: view.consentResult(),
        speaking: view.isSpeaking(), src: view.audioSrc(),
      };
      await view.releasePlay(new Error("OLD_PLAY_REJECTION"));
      check(`${kind} 舊 play 拒絕不改動${replay ? "後一段播放" : "停止狀態"}`,
        view.isSpeaking() === before.speaking && view.audioSrc() === before.src &&
        personaLineOf(view) === before.line && view.consentResult() === before.consent,
        { before, after: { line: personaLineOf(view), consent: view.consentResult(),
          speaking: view.isSpeaking(), src: view.audioSrc() } });
    }
  }

  {
    const sourceAsk = {
      ask: answer({
        query_id: 7007,
        answers: [fact({ frame_id: 42, chunk_id: 31 })],
        hits: [hit({ chunk_id: 77, frame_id: 84, snippet: "隨便一段原文" })],
        synthesis: {
          sentences: [
            {
              text: "客服電話是 0800-080-123。",
              sources: [{ ref: "fact:9", label: "畫面 #42", frame_id: 42 }],
            },
          ],
        },
      }),
      recording_state: "recording",
    };
    const BOOM = "OPEN_FRAME_FIRST_BLEW_UP";
    let opens = 0;
    const view = await open({
      ...sourceAsk,
      open_frame: () => {
        opens += 1;
        if (opens === 1) throw new Error(BOOM);
        return null;
      },
    });
    await view.type("客服電話");
    const button = view.hits().querySelector(".grounded-source");
    await view.clickElement(button);
    check(
      "出處第一次開不起來說一聲",
      view.line().includes("打不開") && view.line().includes(BOOM),
      view.line(),
    );
    await view.clickElement(button);
    check(
      "同一顆出處再按成功後，失敗句收掉",
      !view.line().includes("打不開") && opens === 2,
      { line: view.line(), opens },
    );

    let finishLate = null;
    let lateOpens = 0;
    const late = await open({
      ...sourceAsk,
      open_frame: () => {
        lateOpens += 1;
        if (lateOpens === 1) {
          return new Promise((_, reject) => {
            finishLate = () => reject(new Error("OPEN_FRAME_LATE_FAIL"));
          });
        }
        return null;
      },
    });
    await late.type("客服電話");
    const lateButton = late.hits().querySelector(".grounded-source");
    await late.clickElement(lateButton);
    await late.clickElement(lateButton);
    check("第二下已成功時畫面不多嘴", !late.line().includes("打不開"), late.line());
    finishLate?.();
    await tick();
    await tick();
    check(
      "較早那一次晚到的失敗不准蓋掉後來的成功",
      !late.line().includes("打不開") && !late.line().includes("LATE_FAIL"),
      late.line(),
    );
    let rejectOldFrame;
    const nextQuestion = await open({
      ...sourceAsk,
      open_frame: () => new Promise((_, reject) => { rejectOldFrame = reject; }),
    });
    await nextQuestion.type("第一題");
    await nextQuestion.clickElement(nextQuestion.hits().querySelector(".grounded-source"));
    await nextQuestion.type("第二題");
    rejectOldFrame(new Error("PREVIOUS_QUESTION_FRAME_FAILED"));
    await tick();
    check("換題後舊出處失敗不回到新答案", !nextQuestion.line().includes("PREVIOUS_QUESTION"), nextQuestion.line());

    const otherNotice = await open({
      ...sourceAsk,
      open_timeline: new Error("時間軸這一下開不起來"),
      open_frame: null,
    });
    await otherNotice.type("客服電話");
    await otherNotice.click("#timeline");
    check(
      "前提：畫面上是時間軸那句無關回條",
      otherNotice.line().includes("時間軸這一下"),
      otherNotice.line(),
    );
    await otherNotice.clickElement(otherNotice.hits().querySelector(".grounded-source"));
    check(
      "出處開成功不准清掉不是畫面的回條",
      otherNotice.line().includes("時間軸這一下") && !otherNotice.line().includes("打不開"),
      otherNotice.line(),
    );
  }
}

// 每次命中都丟。`paint` 是同步的，第一次丟到第二次之間沒有交錯點。
// 這裡沒有「只丟一次」的開關：補畫只在剛好只丟一次的夾具底下看起來會動。
function throwOnText(el, needle, { afterWrite = false, message = "repaint failed" } = {}) {
  const original = Object.getOwnPropertyDescriptor(el, "textContent");
  let thrown = false;
  let armed = true;
  Object.defineProperty(el, "textContent", {
    configurable: true,
    get() {
      return original.get.call(el);
    },
    set(value) {
      const text = String(value);
      if (armed && text.includes(needle)) {
        thrown = true;
        if (afterWrite) original.set.call(el, value);
        throw new Error(message);
      }
      original.set.call(el, value);
    },
  });
  return {
    seen: () => thrown,
    disarm() {
      armed = false;
    },
  };
}

console.log("93. 標記寫進去之後重畫丟例外，不可以說沒記進去");
{
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    const stored = new Map();
    const p = await open(
      {
        ask: answer({ query_id: 42, hits: [hit({ snippet: "MARK_BODY_STAYS" })] }),
        recording_state: "recording",
        mark_query: (arg) => {
          stored.set(arg?.queryId, arg?.marked);
          return arg?.marked;
        },
      },
    );
    await p.type("這題我本來忘了");
    const button = p.hits().querySelector(".mark-toggle");
    check(
      "前提：答案底下有標記鈕，而且現在是在聽",
      button !== null && button.disabled === false && p.line().includes("在聽"),
      { button: button?.textContent, line: p.line() },
    );
    const seen = throwOnText(p.node("[data-state-line]"), "在聽");
    const before = rejections.length;
    const clicked = await p.clickElement(button);
    await tick();
    const marks = p.invokes.filter(({ cmd }) => cmd === "mark_query");
    check(
      "前提：標記有送出且重畫有丟",
      clicked === true &&
        seen.seen() === true &&
        marks.length === 1 &&
        marks[0].arg?.queryId === 42 &&
        marks[0].arg?.marked === true &&
        stored.get(42) === true,
      { clicked, seen: seen.seen(), marks, stored: [...stored.entries()] },
    );
    check(
      "這次標記重畫例外留在函式裡",
      rejections.length === before,
      rejections.slice(before).map((reason) => String(reason?.message ?? reason)),
    );
    check("標記鈕沒有留在停用", button.disabled === false, button.disabled);
    seen.disarm();
    await p.repaint();
    check(
      "再畫一次也不說這一次標記沒記進去",
      !p.line().includes("這一次標記沒記進去") &&
        !p.line().includes("repaint failed") &&
        button.disabled === false &&
        button.classList.contains("on") === true,
      { line: p.line(), disabled: button.disabled, on: button.classList.contains("on"), text: button.textContent },
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }

  const failed = await open({
    ask: answer({ query_id: 42, hits: [hit({ snippet: "MARK_FAIL_BODY" })] }),
    recording_state: "recording",
    mark_query: () => {
      throw new Error("database is locked");
    },
  });
  await failed.type("這題寫不進去");
  const failButton = failed.hits().querySelector(".mark-toggle");
  await failed.clickElement(failButton);
  check(
    "標記真的沒寫進去時，仍說沒記進去和原因",
    failed.line().includes("這一次標記沒記進去") && failed.line().includes("database is locked"),
    failed.line(),
  );
  check(
    "標記失敗之後按鈕可以再按，而且沒有變成已記下",
    failButton.disabled === false && failButton.classList.contains("on") === false,
    { disabled: failButton.disabled, text: failButton.textContent },
  );
}

console.log("93b. 標記時 invoke 丟出空值，仍走失敗那一臂");
{
  // 拒絕的理由是 null，不是 Error。null 的真假是假的；走哪一臂要看旗標。
  const falsy = await open({
    ask: answer({ query_id: 42, hits: [hit({ snippet: "MARK_FALSY_BODY" })] }),
    recording_state: "recording",
    mark_query: () => {
      throw null;
    },
  });
  await falsy.type("這題丟出空值");
  const falsyButton = falsy.hits().querySelector(".mark-toggle");
  const clicked = await falsy.clickElement(falsyButton);
  check(
    "invoke 丟出空值時仍走失敗：按鈕沒有變成已記下、狀態列說這一次標記沒記進去、而且可以再按",
    clicked === true &&
      falsyButton.classList.contains("on") === false &&
      falsy.line().includes("這一次標記沒記進去") &&
      falsyButton.disabled === false,
    {
      clicked,
      on: falsyButton.classList.contains("on"),
      disabled: falsyButton.disabled,
      line: falsy.line(),
    },
  );
}

/* 上面那幾行把 `diagnose_note` 從 `calls` 濾掉了。濾掉和刪掉偵測器只差一步，
 * 所以這裡量一次那條路還在：實測這一輪會經過 started／persona／bar／answered
 * 四種。只斷言「有東西」不夠——四種裡剩一種也是「有東西」。 */
check(
  "診斷觀測那條路沒有被過濾掉",
  ["started", "persona", "bar", "answered"].every((kind) =>
    everyDiagnoseNote.some((note) => note?.kind === kind),
  ),
  [...new Set(everyDiagnoseNote.map((note) => note?.kind))].sort(),
);

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——字母人上有話說不出口，或說了活不過下一次輪詢。`);
  process.exit(1);
}
console.log("✓ 那幾句「為什麼沒成」活得過輪詢，而且下一個動作蓋得掉");
// 每個 module instance 都留著自己那顆 5 秒輪詢的 setInterval（`visibilityState`
// 是 visible，那正是要驗的東西），所以要自己走。
process.exit(0);
