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
 * 開不起來、「還在翻…」），其中兩個的原始碼註解自己就在描述這個 bug：
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

import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { domOf, fakeDocument, hiddenIn, loader, read, watchNonsense } from "./fake-dom.mjs";

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
const boot = loader(read(SRC));

/*
 * 開場的 `hidden` 要跟 index.html 一樣，不能跟著假 DOM 的預設值走。詳細的
 * 理由在 fake-dom.mjs 的 `hiddenIn`——簡短版是：第一版寫死 false，於是這
 * 幾條測試在**真的壞掉的** app.js 上照樣綠。
 */
const HTML = read(join(UI, "index.html"));
const hiddenInHtml = (sel) => hiddenIn(HTML, sel);

// 前提本身也要驗一次。哪天 index.html 把那個 `hidden` 拿掉，這幾條測試會
// 悄悄變成「驗一個不存在的問題」——寧可在這裡就吵。
if (!hiddenInHtml("[data-hits]")) {
  console.log("✗ index.html 上的 [data-hits] 已經不是 hidden 了——底下那條測試的前提沒了");
  process.exit(1);
}

const tick = (ms = 20) => new Promise((r) => setTimeout(r, ms));
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
    query_id: 7,
    answers: [],
    hits: [],
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
      });
    }
  }
  return {
    schema: "ai-sister/persona-consent-voices/v1",
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
    consentVoices = null,
  } = {},
) {
  // `domOf` 只生得出 index.html 上真的有的東西——見 fake-dom.mjs 開頭那段。
  const node = domOf(HTML);
  const listeners = new Map();
  const calls = [];
  const invokes = [];
  const intervals = [];
  let audioPlays = 0;
  let audioPauses = 0;
  let localSpeaks = 0;
  const playbackTrace = [];

  // fake-dom 的 selector 子集刻意很小；這一頁新增的 Azure allowlist 是 attribute
  // selector。只在真正的 [data-hits] 子樹補上這一種，避免測試自己用 class
  // denylist 重抄產品邏輯。
  const hitsNode = node("[data-hits]");
  const basicQuerySelectorAll = hitsNode.querySelectorAll.bind(hitsNode);
  hitsNode.querySelectorAll = (selector) => {
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
  audio.play = async () => {
    audioPlays += 1;
  };

  globalThis.document = fakeDocument(node, {
    // **要是 visible。** 開場那一段對 `recording` 寫死的是 `"recording"`，
    // 只有 `updatePollGate()` 看到視窗是開著的才會去問一次磁碟；hidden 的話
    // 這一頁會停在「她在錄」，而灰掉那條路上的話（`wakeFailed`）就永遠不會
    // 被畫出來——測試會綠，但綠的理由是它根本沒走到那裡。
    visibilityState: "visible",
  });
  globalThis.location = { search };
  globalThis.addEventListener = () => {};
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
  globalThis.speechSynthesis = {
    getVoices: () => [],
    addEventListener() {},
    cancel() {},
    speak() {
      localSpeaks += 1;
    },
  };
  globalThis.setInterval = (fn, ms, ...args) => {
    const id = nativeSetInterval(fn, ms, ...args);
    intervals.push({ fn, ms, id });
    return id;
  };
  globalThis.clearInterval = (id) => nativeClearInterval(id);
  if (consentVoices === null) delete globalThis.__AI_SISTER_CONSENT_VOICES__;
  else globalThis.__AI_SISTER_CONSENT_VOICES__ = consentVoices;

  const tauri = {
    core: {
      invoke: async (cmd, arg) => {
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

  const nonsense = watchNonsense();
  await boot();
  await tick();
  return {
    node,
    calls,
    invokes,
    nonsense,
    line: () => node("[data-state-line]").textContent,
    hits: () => node("[data-hits]"),
    hitTexts: () => node("[data-hits]").children.map((c) => c.textContent),
    consentGuide: () => node("[data-consent-guide]"),
    consentProgress: () => node("[data-consent-progress]").textContent,
    consentWording: () => node("[data-consent-wording]").textContent,
    consentResult: () => node("[data-consent-result]").textContent,
    consentListen: () => node("[data-consent-listen]"),
    input: () => node("[data-ask-input]"),
    azureButton: () => node("[data-hits]").querySelector(".answer-cloud"),
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
    localSpeaks: () => localSpeaks,
    urlPolicy: () => node("[data-url-policy]"),
    urlPolicyQuestion: () => node("[data-url-policy-question]").textContent,
    urlPolicyActions: () => node("[data-url-policy-actions]"),
    urlPolicyResult: () => node("[data-url-policy-result]"),
    utterance: () => node("[data-utterance]"),
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
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

const CONSENT = "第一張同意書還沒簽——她不會開始記錄。在系統匣圖示上按右鍵，選「四張同意書…」簽好再回來";

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

console.log("③ 這一題翻很久（SLOW_MS）");
{
  const p = await open({
    // 5 秒才回，比 SLOW_MS（4 秒）久。
    ask: () => new Promise((r) => setTimeout(() => r(answer()), 5000)),
    recording_state: "recording",
  });
  void p.type("三天前那通電話");
  await tick(4300);
  check("換成「還在翻…」了", p.line().includes("還在翻"), p.line());
  await p.repaint();
  check("輪詢過後沒有被換回「想一下…」", p.line().includes("還在翻"), p.line());
  await tick(1200);
  check("答案回來就不講了", !p.line().includes("還在翻"), p.line());
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

console.log("⑪ 上一題的「還在翻…」不可以蓋到半秒前才送出的新題目上");
{
  // 時間軸（SLOW_MS = 4000）：
  //   t=0     第一題送出，永遠不回來
  //   t=3800  第二題送出（第一題還掛著，`state` 一直是 thinking）
  //   t=4000  **第一題**的計時器響。`state === "thinking"` 是真的——那是第二題的。
  //   t=4300  看畫面：第二題才半秒大，不可以說「這一題已經超過 4 秒」
  const p = await open({
    ask: (arg) =>
      arg.question === "第一題"
        ? new Promise(() => {})
        : new Promise((r) => setTimeout(() => r(answer()), 2200)),
    recording_state: "recording",
  });
  void p.type("第一題");
  await tick(3800);
  void p.type("第二題");
  await tick(500);
  check("還在想第二題", p.line().includes("想一下") || p.line().includes("在聽"), p.line());
  check("而且沒被上一題的計時器蓋掉", !p.line().includes("還在翻"), p.line());
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
  const rustSources = [
    read(join(UI, "../src-tauri/src/main.rs")),
    read(join(UI, "../src-tauri/src/recorder_supervisor.rs")),
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
    "我目前對最近幾段有這些理解。每張下面都有畫面出處按鈕；內容可能由模型整理、審閱層修訂，或由你修正，不是我量到的確定事實：\n修好安裝更新\n審閱後的更新流程\n這是我自己修正的說法";
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
      azure:
        "我有原始紀錄，但還沒有整理成能直接回答的理解記憶；這次不會拿 OCR 片段冒充答案。",
    },
    {
      overview: { kind: "empty" },
      wanted: "目前還沒有留下能回答這題的記憶",
      azure: "我目前還沒有留下能回答這題的記憶。",
    },
    {
      overview: { kind: "evidence_missing", cards: 3 },
      wanted: "目前沒有可點開的畫面出處",
      azure:
        "我有整理過的理解記憶，但最近這 3 張卡片目前沒有可點開的畫面出處；這裡不把它們當成答案。",
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
    "開場就是第一張完整條文，輸入框明講兩個答案",
    !p.consentGuide().hidden &&
      p.consentProgress() === "同意書 1 / 4" &&
      p.consentWording() === state.sheets[0].wording &&
      p.input().placeholder.includes("同意"),
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

console.log("85. 條文錄音只能由 trusted click 播，而且逐字稿必須等於 native 條文");
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

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——字母人上有話說不出口，或說了活不過下一次輪詢。`);
  process.exit(1);
}
console.log("✓ 那幾句「為什麼沒成」活得過輪詢，而且下一個動作蓋得掉");
// 每個 module instance 都留著自己那顆 5 秒輪詢的 setInterval（`visibilityState`
// 是 visible，那正是要驗的東西），所以要自己走。
process.exit(0);
