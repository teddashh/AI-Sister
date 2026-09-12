#!/usr/bin/env node
/*
 * Persona v2 的行為契約。
 *
 * 直接載入產品 app.js，而不是抄一份選台詞邏輯。這裡特別守五條容易各自看起來
 * 正確、湊起來卻說謊的縫：17 人都必須有隨程式提供的真角色圖，任何 pack 狀態都
 * 不准退回字母；開場／五秒輪詢不准自己開口；native button 的 click 才能播放固定
 * Ogg；一般答案才用明確標成 localService 的系統語音；日常與閒話兩包語音各自
 * fail closed，壞一包不可以連累另一包、也不可以互相背書。關掉角色不可以連搜尋與
 * 錄製狀態一起關。
 *
 * alpha.132 之後多守兩件事。一是她**沒有人碰也會開口**（`idle-giggle`／
 * `answer-beat`）：那是整個程式裡唯一一條自己出聲的路，所以底下每一道閘門都要有
 * 自己的斷言。二是上面那條膠囊平常收起來（②ᵈ）——那一格的規則橫跨 CSS、app.js、
 * HTML 和 Rust 的系統匣選單四個地方，而「看不見」和「點得穿」是分開寫的兩句話，
 * 只改一句不會有任何畫面上的症狀。
 */

import { createHash } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import { domOf, fakeDocument, fakeEl, loader, read, watchNonsense } from "./fake-dom.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const UI = join(ROOT, "apps/desktop/ui");
const HTML = read(join(UI, "index.html"));
const SETTINGS_HTML = read(join(UI, "settings.html"));
const SRC = read(join(UI, "app.js"));
const STYLES = read(join(UI, "styles.css"));
const SETTINGS = read(join(UI, "settings.js"));
const SETTINGS_STYLES = read(join(UI, "settings.css"));
const SETTINGS_CATALOG_SOURCE = read(join(UI, "personas/catalog.js"));
const SHOT = read(join(ROOT, "scripts/shot.mjs"));
const SHOOT_AVATAR = read(join(ROOT, "scripts/shoot-avatar.sh"));
const MAIN = read(join(ROOT, "apps/desktop/src-tauri/src/main.rs"));
const CONFIG = read(join(ROOT, "crates/sister-core/src/config.rs"));
const BUNDLED = JSON.parse(read(join(UI, "personas/manifest.json")));
const REELS = JSON.parse(read(join(UI, "persona-reels/manifest.json")));
const VOICES = JSON.parse(read(join(UI, "persona-voices/v1/manifest.json")));
const BANTER = JSON.parse(read(join(UI, "persona-banter-voices/v1/manifest.json")));
const FACE_CSS = STYLES.match(/(?:^|\n)\.face \{\n(?<body>[\s\S]*?)\n\}/u)?.groups?.body ?? "";
const PORTRAIT_CSS =
  STYLES.match(/(?:^|\n)\.portrait \{\n(?<body>[\s\S]*?)\n\}/u)?.groups?.body ?? "";
const REEL_CSS =
  STYLES.match(/(?:^|\n)\.persona-reel \{\n(?<body>[\s\S]*?)\n\}/u)?.groups?.body ?? "";
const PAUSED_MARKER_CSS =
  STYLES.match(
    /(?:^|\n)\.avatar\[data-state="paused"\] \.face::after \{\n(?<body>[\s\S]*?)\n\}/u,
  )?.groups?.body ?? "";
const ASLEEP_MARKER_CSS =
  STYLES.match(
    /(?:^|\n)\.avatar\[data-state="asleep"\] \.face::after \{\n(?<body>[\s\S]*?)\n\}/u,
  )?.groups?.body ?? "";
const PAUSED_SLASH_CSS =
  STYLES.match(
    /(?:^|\n)\.avatar\[data-state="paused"\]::after \{\n(?<body>[\s\S]*?)\n\}/u,
  )?.groups?.body ?? "";
const PERSONA_CHOICE_IMAGE_CSS =
  SETTINGS_STYLES.match(
    /(?:^|\n)\.persona-choice-image \{\n(?<body>[\s\S]*?)\n\}/u,
  )?.groups?.body ?? "";
const PERSONA_PREVIEW_IMAGE_CSS =
  SETTINGS_STYLES.match(
    /(?:^|\n)\.persona-preview-image \{\n(?<body>[\s\S]*?)\n\}/u,
  )?.groups?.body ?? "";
const settingsCatalogContext = {};
runInNewContext(SETTINGS_CATALOG_SOURCE, settingsCatalogContext);
const SETTINGS_CATALOG = settingsCatalogContext.__AI_SISTER_PERSONA_CATALOG__;
const ASSET_PROJECTION = JSON.parse(
  read(join(ROOT, "crates/sister-assets/tests/fixtures/public-manifest-selected-v2.json")),
);
const boot = loader(SRC);
const tick = () => new Promise((done) => setTimeout(done, 20));

const EMPTY_PACK = Object.freeze({
  phase: "unavailable",
  release_id: null,
  portrait: null,
  voice_lines: [],
});

function persona(id = "chatgpt", over = {}) {
  return {
    enabled: true,
    id,
    motion: true,
    tap_lines: true,
    voice_enabled: false,
    asset_pack: EMPTY_PACK,
    ...over,
  };
}

const EXPECTED = Object.freeze({
  chatgpt: {
    alias: "ChatGPT",
    tagline: "結構與驗證；固定台詞用「我在」開場。",
    palette: ["#0B1F33", "#F8FAFC", "#5EEAD4"],
    first: "我在，隨時可以開始。",
  },
  claude: {
    alias: "Claude",
    tagline: "論證與邊界；固定台詞用「慢慢來」開場。",
    palette: ["#12372A", "#F8FAFC", "#A7F3D0"],
    first: "慢慢來，隨時可以開始。",
  },
  gemini: {
    alias: "Gemini",
    tagline: "打開可能；固定台詞用「一起看看」開場。",
    palette: ["#172554", "#F8FAFC", "#A5B4FC"],
    first: "一起看看，隨時可以開始。",
  },
  grok: {
    alias: "Grok",
    tagline: "直球測試；固定台詞用「收到」開場。",
    palette: ["#3B1B0B", "#F8FAFC", "#FDE68A"],
    first: "收到，隨時可以開始。",
  },
  deepseek: { alias: "DeepSeek", tagline: "深挖證據與原理。", palette: ["#12213A", "#F8FAFC", "#60A5FA"], first: "我來往下挖，隨時可以開始。" },
  qwen: { alias: "Qwen", tagline: "布局、控場與收斂。", palette: ["#312E81", "#F8FAFC", "#C4B5FD"], first: "我來收斂，隨時可以開始。" },
  mistral: { alias: "Mistral", tagline: "俐落拆解，減少多餘協調。", palette: ["#451A03", "#FFFBEB", "#F59E0B"], first: "直接拆開來看，隨時可以開始。" },
  venice: { alias: "Llama", tagline: "自由、直接、不受拘束。", palette: ["#3F1D2E", "#FFF7ED", "#FB7185"], first: "先講最直接的，隨時可以開始。" },
  sakana: { alias: "Sakana", tagline: "保留變體，試另一條演化路徑。", palette: ["#164E63", "#ECFEFF", "#67E8F9"], first: "我們試另一條路，隨時可以開始。" },
  perplexity: { alias: "Perplexity", tagline: "先查證，再下結論。", palette: ["#134E4A", "#F0FDFA", "#5EEAD4"], first: "我先查證，隨時可以開始。" },
  glm: { alias: "GLM", tagline: "先做出可動的版本。", palette: ["#1E3A5F", "#EFF6FF", "#93C5FD"], first: "先做一版，隨時可以開始。" },
  kimi: { alias: "Kimi", tagline: "守住前文、脈絡與交接。", palette: ["#312E81", "#F7F7FF", "#C7D2FE"], first: "我接著前面，隨時可以開始。" },
  hunyuan: { alias: "Hunyuan", tagline: "把上下游與被漏掉的人接回來。", palette: ["#0C4A6E", "#F4F8FF", "#78B8FF"], first: "我把上下游接起來，隨時可以開始。" },
  minimax: { alias: "MiniMax", tagline: "先讓作品能看、能聽、能感受到。", palette: ["#4A0D24", "#FFF6F8", "#FB923C"], first: "先讓它活起來，隨時可以開始。" },
  nemotron: { alias: "Nemotron", tagline: "工程調度與可部署交付。", palette: ["#0B0F0A", "#F9FAFB", "#76B900"], first: "把交付路徑釘住，隨時可以開始。" },
  cohere: { alias: "Cohere", tagline: "多方溝通、引用與協議。", palette: ["#243C34", "#F7F8F3", "#D18EE2"], first: "我把每一方都放進來，隨時可以開始。" },
  mimo: { alias: "MiMo", tagline: "先看人用起來順不順。", palette: ["#431407", "#FFF8F1", "#FF6900"], first: "先看用起來順不順，隨時可以開始。" },
});

/*
 * 她挑台詞是隨機的（`pickBanter`），而「隨機」本身很難斷言：拿真的 `Math.random`
 * 去跑統計，等於在 CI 上擲骰子——這個 repo 已經被那種測試咬過兩次。所以這裡把
 * 骰子接管起來。產品碼一個字都不必為了測試改，是測試自己決定骰子擲出什麼。
 *
 * 預設仍是真的亂數；只有明確 `scriptRandom()` / `scriptRandomSequence()` 的區段
 * 才被接管，用完 `stopScriptingRandom()` 還回去。
 */
const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;
const realRandom = Math.random;
let scriptedRandom = null;
Math.random = () => (scriptedRandom === null ? realRandom() : scriptedRandom());

/** 每次都擲出同一個值的骰子。用來逼出「連兩下不准講同一句」那條規則。 */
function scriptRandom(value) {
  scriptedRandom = () => value;
}

/**
 * 固定種子的 PRNG（mulberry32）。它是「真的有鋪開」而不是「碰運氣」：同一顆種子
 * 在任何機器上算出來的序列一模一樣，所以「戳 40 下會出現幾種台詞」是個定值，
 * 不是機率。
 */
function scriptRandomSequence(seed) {
  let state = seed >>> 0;
  scriptedRandom = () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function stopScriptingRandom() {
  scriptedRandom = null;
}

/**
 * 一個角色被戳的時候講得出來的每一句話：日常那包的兩句 + 閒話那包的每一句。
 *
 * 從兩份 manifest 算出來，不在這裡抄一份清單——抄的那份會漂，而且漂了還是綠的。
 */
function pokePool(id) {
  return [
    ...VOICES.clips.filter((clip) => clip.persona === id && clip.use === "avatar-tap"),
    ...BANTER.clips.filter((clip) => clip.persona === id && clip.use === "avatar-poke"),
  ].map((clip) => clip.text);
}

async function open(personaView = persona(), options = {}) {
  let plays = 0;
  let cancels = 0;
  const mediaTrace = [];
  const playedSources = [];
  const speaks = [];
  const reelImages = [];
  const deferredReelLoads = [];
  let finishPersonaRead = null;
  let finishVoiceRead = null;
  const initialPersonaRead = options.deferPersonaRead
    ? new Promise((done) => {
        finishPersonaRead = done;
      })
    : Promise.resolve(personaView);
  const node = domOf(HTML, (selector) => {
    const element = fakeEl(selector);
    if (selector === "[data-persona-audio]") {
      element.pause = () => mediaTrace.push("pause");
      element.play = () => {
        plays += 1;
        playedSources.push(element.src);
        return Promise.resolve();
      };
      element.currentTime = 0;
      element.src = "";
    }
    return element;
  });
  const root = fakeEl("html");
  // `document.body` 的 class 是三塊畫面共用的開關（上面那條膠囊、答案氣泡讓不讓
  // 位、她是不是停著），所以要拿得到同一顆節點才驗得到。`fakeDocument` 自己也會
  // 生一個，但那一顆沒有人能引用。
  const body = fakeEl("body");
  const css = new Map();
  root.style = {
    setProperty(name, value) {
      css.set(name, value);
    },
    removeProperty(name) {
      css.delete(name);
    },
  };
  const listeners = new Map();
  const intervals = [];
  const slowTimers = [];
  const calls = [];
  const diagnoseNotes = [];
  const solidPushes = [];
  const windowHandlers = {};
  const voiceReads = [];
  const nonsense = watchNonsense();

  if (Object.hasOwn(options, "reelManifest")) {
    globalThis.__AI_SISTER_PERSONA_REELS__ = options.reelManifest;
  } else {
    delete globalThis.__AI_SISTER_PERSONA_REELS__;
  }
  globalThis.__AI_SISTER_PERSONA_VOICES__ = Object.hasOwn(options, "voiceManifest")
    ? options.voiceManifest
    : VOICES;
  globalThis.__AI_SISTER_PERSONA_BANTER__ = Object.hasOwn(options, "banterManifest")
    ? options.banterManifest
    : BANTER;
  const reelLayers = new Map();
  for (const rig of Array.isArray(options.reelManifest?.rigs)
    ? options.reelManifest.rigs
    : []) {
    for (const layer of Array.isArray(rig?.layers) ? rig.layers : []) {
      reelLayers.set(`./persona-reels/${layer.file}`, { id: rig.id, layer });
    }
  }

  const createElement = (tag) => {
    const element = fakeEl(tag);
    if (tag !== "img") return element;
    let src = "";
    Object.defineProperty(element, "src", {
      configurable: true,
      get: () => src,
      set(value) {
        src = String(value);
        const declared = reelLayers.get(src);
        reelImages.push(element);
        element.reelId = declared?.id ?? null;
        element.naturalWidth = declared?.layer?.width ?? 0;
        element.naturalHeight = declared?.layer?.height ?? 0;
        element.decodeCalls = 0;
        element.decode = async () => {
          element.decodeCalls += 1;
          if (
            options.reelFailure?.kind === "decode" &&
            options.reelFailure?.file === declared?.layer?.file
          ) {
            throw new Error("fixture decode failed");
          }
        };
        const finish = () => {
          if (
            options.reelFailure?.kind === "load" &&
            options.reelFailure?.file === declared?.layer?.file
          ) {
            element.onerror?.(new Error("fixture load failed"));
          } else {
            element.onload?.();
          }
        };
        if (options.deferReelLoads) {
          deferredReelLoads.push({ id: declared?.id ?? null, finish });
        } else {
          queueMicrotask(finish);
        }
      },
    });
    const removeAttribute = element.removeAttribute.bind(element);
    element.removeAttribute = (name) => {
      if (name === "src") {
        src = "";
        mediaTrace.push("remove-src");
      }
      removeAttribute(name);
    };
    return element;
  };

  globalThis.document = fakeDocument(node, {
    visibilityState: options.visibilityState ?? "visible",
    documentElement: root,
    body,
    createElement,
  });
  globalThis.location = { search: options.search ?? "" };
  // 記下來而不是丟掉。行為沒變——沒有人去 fire 的話，記著和 no-op 一樣；
  // 但「拖完之後整扇窗要收回來」那條路只掛在 globalThis 上，丟掉就驗不到。
  globalThis.addEventListener = (ev, fn) => {
    (windowHandlers[ev] ??= []).push(fn);
  };
  globalThis.removeEventListener = () => {};
  globalThis.matchMedia = () => ({ matches: false, addEventListener() {} });
  // 她那扇窗是固定的 340×560（`resizable: false`，見 tauri.conf.json）。
  // 假瀏覽器要報得出視窗大小，app.js 才算得出「拖曳中整扇窗都算實心」那一塊。
  globalThis.innerWidth = 340;
  globalThis.innerHeight = 560;
  // app.js 掛了兩個：一個重算「哪裡是實心的」再送回 Rust（`pet_solid_set`），一個
  // 把上面那條膠囊的 `aria-hidden` 跟著 body 的 class 一起翻。這個假瀏覽器沒有版面
  // 引擎，不會自己偵測到任何變動，所以回呼記下來讓測試自己叫（`fireMutations()`）；
  // 這個類別本身則得**存在**，少了它 app.js 一載入就 ReferenceError，整支閘門連第
  // 一條斷言都跑不到。
  //
  // 記下 `observe()` 的目標而不是丟掉：掛錯節點的 observer 在真機器上永遠收不到
  // body 的 class 變化，而讀原始碼分不出「有掛」和「掛對地方」。
  const mutationWatchers = [];
  globalThis.MutationObserver = class {
    constructor(callback) {
      this.callback = callback;
    }
    observe(target, init) {
      mutationWatchers.push({ callback: this.callback, target, init });
    }
    disconnect() {
      for (let i = mutationWatchers.length - 1; i >= 0; i -= 1) {
        if (mutationWatchers[i].callback === this.callback) mutationWatchers.splice(i, 1);
      }
    }
    takeRecords() {
      return [];
    }
  };
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
    addEventListener() {},
    cancel() {
      cancels += 1;
      mediaTrace.push("cancel-local");
    },
    getVoices() {
      return options.systemVoices ?? [];
    },
    speak(utterance) {
      speaks.push(utterance);
    },
  };
  globalThis.setInterval = (fn, ms) => {
    intervals.push({ fn, ms });
    return intervals.length;
  };
  globalThis.clearInterval = () => {};
  // 「沒事的時候自己笑一下」排在兩到五分鐘之後，那一句在畫面上再留六秒；測試
  // 不可能真的等。所以慢的那些被記下來讓測試自己叫，快的（tick、抖那一下、
  // 4 秒那句話）照樣交給真的 setTimeout。
  //
  // 門檻取六秒是量出來的，不是猜的：app.js 裡比它慢的只有這兩個閒話計時器
  // （`grep -n "setTimeout(" app.js` 共七處，其餘最長 5000）。
  globalThis.setTimeout = (fn, ms, ...rest) => {
    if (typeof ms === "number" && ms >= 6_000) {
      slowTimers.push({ fn, ms, cancelled: false });
      return { slowTimer: slowTimers.length };
    }
    return realSetTimeout(fn, ms, ...rest);
  };
  globalThis.clearTimeout = (handle) => {
    if (handle !== null && typeof handle === "object" && "slowTimer" in handle) {
      const entry = slowTimers[handle.slowTimer - 1];
      if (entry) entry.cancelled = true;
      return;
    }
    realClearTimeout(handle);
  };
  const tauri = {
    core: {
      invoke: async (cmd, args) => {
        /* 診斷觀測不是產品指令。
         *
         * `diagnose_note` 在 Rust 那邊就只是 `Mutex<Notebook>` 上的一次 push：
         * 不回傳東西、不碰任何狀態、沒有人 await 它。而底下好幾條契約的形狀是
         * 「做完這個動作，`calls` 該是空的」——把一條純觀測混進同一條清單，會讓
         * 那些契約在產品行為一個字都沒變的情況下變紅。
         *
         * 記到另一條清單上，不是丟掉：`diagnoseNotes` 自己有斷言（見 ⑨），所以
         * 「濾掉」不會變成「沒人看」。 */
        if (cmd === "diagnose_note") {
          diagnoseNotes.push(args?.note);
          return;
        }
        calls.push(cmd);
        if (cmd === "pet_solid_set") solidPushes.push(args?.solid ?? []);
        switch (cmd) {
          case "persona_read":
            return initialPersonaRead;
          case "persona_voice_read":
            voiceReads.push(args?.lineId);
            if (options.deferVoiceRead) {
              return new Promise((done) => {
                finishVoiceRead = () =>
                  done({
                    line_id: args?.lineId,
                    data_url: "data:audio/wav;base64,AA==",
                  });
              });
            }
            if (Object.hasOwn(options, "voiceReadResult")) return options.voiceReadResult;
            return {
              line_id: args?.lineId,
              data_url: "data:audio/wav;base64,AA==",
            };
          case "recording_state":
            if (options.deferRecorderTruth) return new Promise(() => {});
            return "recording";
          case "recorder_supervisor_state":
            if (options.deferRecorderTruth) return new Promise(() => {});
            return { phase: "stopped", failures: 0, message: null };
          case "master_stop_state":
            return typeof options.masterStopState === "function"
              ? options.masterStopState()
              : (options.masterStopState ?? "clear");
          case "master_stop_presentation_begin":
            return typeof options.masterStopPresentationBegin === "function"
              ? options.masterStopPresentationBegin(args)
              : (options.masterStopPresentationBegin ?? true);
          case "master_stop_presentation_end":
            mediaTrace.push(`end:${args?.presentationId ?? "missing"}`);
            return null;
          case "answer_local_speech_admit":
            if (options.localSpeechAdmission instanceof Error) {
              throw options.localSpeechAdmission;
            }
            return options.localSpeechAdmission ?? { presentation_id: "local-answer" };
          case "persona_fixed_voice_admit":
            if (options.fixedSpeechAdmission instanceof Error) {
              throw options.fixedSpeechAdmission;
            }
            return options.fixedSpeechAdmission ?? { presentation_id: "persona-fixed" };
          case "pause_state":
            return options.pauseState === true;
          case "azure_tts_read":
            return options.azureStatus ?? null;
          case "last_recording_end":
            return null;
          case "gatekeeper_check":
            return { display: null, developer: null, action_log: ["沒有動作。"] };
          case "url_policy_read":
            return {
              question: "q",
              answered: "only-on-my-press",
              options: [],
              before_you_answer: "x",
              path: "C:\\config.toml",
            };
          case "ask":
            return (
              options.askResult ?? {
                presentation_id: null,
                hits: [],
                kind: "keywords",
                query_id: null,
                answers: [],
                blind: null,
                truncated: false,
                answers_truncated: false,
                searched: null,
                time_range: null,
                chapters: null,
                followup: null,
                closure_notice: null,
                overview: null,
                synthesis: null,
                brain: { state: "not_configured", provider: null },
              }
            );
          default:
            return null;
        }
      },
    },
    event: {
      listen: async (name, callback) => {
        listeners.set(name, callback);
        return () => {};
      },
    },
  };
  if (options.browserOnly) delete globalThis.__TAURI__;
  else globalThis.__TAURI__ = tauri;

  await boot();
  await tick();
  return {
    node,
    body,
    css,
    calls,
    diagnoseNotes,
    solidPushes,
    /* 模擬「`document.body` 的 class 換了一次」，叫醒真的會收到這一則的那幾個
     * observer。真瀏覽器會在當下自己跑，這裡沒有版面引擎，所以由測試在改完之後補
     * 一次；回傳跑了幾個，0 就是「沒有人在看 body 的 class」，那條線在真機器上也是
     * 死的。
     *
     * 要照 `attributeFilter` 過濾，不可以一律全叫：這份清單漏掉 `"class"` 的
     * observer 在真機器上收不到這一則，而一律全叫會讓它在這裡照樣跑，於是漏了也
     * 是綠的。 */
    fireMutations() {
      const watching = mutationWatchers.filter(
        (entry) =>
          entry.target === body &&
          entry.init?.attributes === true &&
          (entry.init.attributeFilter ?? ["class"]).includes("class"),
      );
      for (const entry of watching) entry.callback([], null);
      return watching.length;
    },
    /* 對著 globalThis 發事件——「手放開了」這件事只有那裡收得到。 */
    fireWindow(ev, arg) {
      for (const fn of windowHandlers[ev] ?? []) fn(arg);
    },
    intervals,
    slowTimers,
    /* 叫起還沒被取消的長計時器。她「沒事自己笑一下」和「那一句留六秒」都掛在
     * 那上面，而兩者會互相接龍（笑完會排下一次），所以要挑得出來。 */
    async fireSlowTimers(pick = () => true) {
      const pending = slowTimers.filter((timer) => !timer.cancelled && pick(timer));
      for (const timer of pending) {
        timer.cancelled = true;
        timer.fn();
      }
      await tick();
      return pending.length;
    },
    nonsense,
    voiceReads,
    speaks,
    reelImages,
    plays: () => plays,
    playedSources: () => [...playedSources],
    cancels: () => cancels,
    mediaTrace: () => [...mediaTrace],
    finishAudio() {
      node("[data-persona-audio]").onended?.();
    },
    failAudio() {
      node("[data-persona-audio]").onerror?.();
    },
    async clickAvatar() {
      const button = node("[data-avatar]");
      if (button.disabled) return false;
      for (const fn of button.handlers.click ?? []) fn({ type: "click", isTrusted: true });
      await tick();
      return true;
    },
    /* 一次完整的手勢：按下 → （可選）移動 → 放開 → 補一個 click。
     * 拖曳和戳她共用同一顆按鈕，所以這兩件事只能靠「移動了多少」分辨。 */
    async avatarGesture({ dx = 0, dy = 0 } = {}) {
      const button = node("[data-avatar]");
      if (button.disabled) return false;
      const fire = (ev, arg) => {
        for (const fn of button.handlers[ev] ?? []) fn(arg);
      };
      fire("pointerdown", { button: 0, clientX: 100, clientY: 100 });
      if (dx !== 0 || dy !== 0) {
        fire("pointermove", { clientX: 100 + dx, clientY: 100 + dy });
      }
      fire("pointerup", {});
      fire("click", { type: "click", isTrusted: true });
      await tick();
      return true;
    },
    /* 那顆「⋯」。`isTrusted` 是可選的，因為這一頁唯一的版面驗證工具
     * （`scripts/shot.mjs`）用的是 `element.click()`，那是合成事件。 */
    async clickChromeToggle({ isTrusted = true } = {}) {
      const button = node("[data-chrome-toggle]");
      if (button.disabled) return false;
      for (const fn of button.handlers.click ?? []) fn({ type: "click", isTrusted });
      await tick();
      return true;
    },
    async forceAvatarHandler() {
      for (const fn of node("[data-avatar]").handlers.click ?? []) {
        fn({ type: "click", isTrusted: true });
      }
      await tick();
    },
    async syntheticAvatarClick() {
      for (const fn of node("[data-avatar]").handlers.click ?? []) {
        fn({ type: "click", isTrusted: false });
      }
      await tick();
    },
    async ask(question) {
      node("[data-ask-input]").value = question;
      for (const fn of node("[data-ask-send]").handlers.click ?? []) fn({
        type: "click",
        isTrusted: true,
      });
      await tick();
    },
    async clickAnswerRead() {
      const button = node("[data-hits]")
        .querySelectorAll("button")
        .find((item) => item.className === "answer-read");
      if (!button || button.disabled) return false;
      for (const fn of button.handlers.click ?? []) fn({ type: "click", isTrusted: true });
      await tick();
      return true;
    },
    async poll() {
      for (const { fn } of intervals) fn();
      await tick();
    },
    async fromOutside(name, payload) {
      const listener = listeners.get(name);
      if (!listener) throw new Error(`沒有人在聽 ${name}`);
      listener({ payload });
      await tick();
    },
    async resolvePersonaRead(view = personaView) {
      finishPersonaRead?.(view);
      await tick();
    },
    async resolveVoiceRead() {
      finishVoiceRead?.();
      await tick();
    },
    async settleReel(id) {
      for (const pending of deferredReelLoads.filter((item) => item.id === id)) {
        pending.finish();
      }
      await tick();
    },
  };
}

let failures = 0;
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failures += 1;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

function luminance(hex) {
  const channels = hex
    .slice(1)
    .match(/.{2}/gu)
    .map((part) => Number.parseInt(part, 16) / 255)
    .map((channel) =>
      channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
    );
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrast(left, right) {
  const values = [luminance(left), luminance(right)].sort((a, b) => b - a);
  return (values[0] + 0.05) / (values[1] + 0.05);
}

function directElementRules(className) {
  const classPattern = new RegExp(`\\.${className}(?![-\\w])`, "u");
  // `[^{}]+` 會把規則前面那整段註解一起收進 selector，於是註解裡提到的任何
  // class 名字都算命中——`.avatar::before` 的說明寫了「會把 `.face` 擠開」，
  // 這道閘門就報它是 `.face` 加了底色。註解不是 selector，先剝掉再比。
  const withoutComments = STYLES.replaceAll(/\/\*[\s\S]*?\*\//gu, "");
  return [...withoutComments.matchAll(/(?<selector>[^{}]+)\{(?<body>[^{}]*)\}/gu)]
    .filter(({ groups }) =>
      groups.selector
        .split(",")
        .some((selector) => classPattern.test(selector) && !selector.includes(`.${className}::`)),
    )
    .map(({ groups }) => ({ selector: groups.selector.trim(), body: groups.body }));
}

console.log("⓪ 純瀏覽器明列的 state 才是截圖 fixture；產品冷啟動仍等兩份證據");
{
  for (const [requested, state, line] of [
    ["idle", "idle", "在聽"],
    ["thinking", "thinking", "想一下…"],
    ["paused", "paused", "已暫停，沒有在看"],
    ["asleep", "asleep", "沒有人在記錄"],
  ]) {
    const p = await open(persona("chatgpt"), {
      browserOnly: true,
      search: `?state=${requested}`,
    });
    check(
      `純瀏覽器 ?state=${requested} 保留指定外觀`,
      p.node("[data-avatar]").dataset.state === state &&
        p.node("[data-state-line]").textContent.includes(line),
      {
        state: p.node("[data-avatar]").dataset.state,
        line: p.node("[data-state-line]").textContent,
      },
    );
  }

  const browserWithoutFixture = await open(persona("chatgpt"), { browserOnly: true });
  check(
    "純瀏覽器沒有 state 仍不拿預設值冒充 recorder 證據",
    browserWithoutFixture.node("[data-avatar]").dataset.state === "asleep" &&
      browserWithoutFixture.node("[data-state-line]").textContent.includes("正在確認"),
    browserWithoutFixture.node("[data-state-line]").textContent,
  );

  const tauriColdStart = await open(persona("chatgpt"), { deferRecorderTruth: true });
  check(
    "Tauri 冷啟動在 heartbeat / supervisor 回來前仍只說正在確認",
    tauriColdStart.node("[data-avatar]").dataset.state === "asleep" &&
      tauriColdStart.node("[data-state-line]").textContent.includes("正在確認"),
    tauriColdStart.node("[data-state-line]").textContent,
  );

  const tauriWithDemoQuery = await open(persona("chatgpt"), {
    deferRecorderTruth: true,
    search: "?state=paused&asleep=nobeat&hits=demo&demo=1&marked=1",
  });
  check(
    "Tauri 不採信瀏覽器 screenshot query，也不會假裝已按暫停",
    tauriWithDemoQuery.node("[data-avatar]").dataset.state === "asleep" &&
      tauriWithDemoQuery.node("[data-state-line]").textContent.includes("正在確認") &&
      tauriWithDemoQuery.node("#pause").title === "暫停記錄" &&
      tauriWithDemoQuery.node("#pause").dataset["aria-pressed"] === "false" &&
      tauriWithDemoQuery.node("[data-hits]").hidden === true &&
      tauriWithDemoQuery.node("[data-hits]").children.length === 0 &&
      !tauriWithDemoQuery.node("[data-state-line]").textContent.includes("等了 25 秒"),
    {
      state: tauriWithDemoQuery.node("[data-avatar]").dataset.state,
      line: tauriWithDemoQuery.node("[data-state-line]").textContent,
      title: tauriWithDemoQuery.node("#pause").title,
      pressed: tauriWithDemoQuery.node("#pause").dataset["aria-pressed"],
      hitsHidden: tauriWithDemoQuery.node("[data-hits]").hidden,
      hitCount: tauriWithDemoQuery.node("[data-hits]").children.length,
    },
  );

  const browserWithUnknownFixture = await open(persona("chatgpt"), {
    browserOnly: true,
    search: "?state=garbage",
  });
  check(
    "純瀏覽器不採信白名單外的 state",
    browserWithUnknownFixture.node("[data-avatar]").dataset.state === "asleep" &&
      browserWithUnknownFixture.node("[data-state-line]").textContent.includes("正在確認") &&
      browserWithUnknownFixture.node("#pause").dataset["aria-pressed"] === "false",
    {
      state: browserWithUnknownFixture.node("[data-avatar]").dataset.state,
      line: browserWithUnknownFixture.node("[data-state-line]").textContent,
      pressed: browserWithUnknownFixture.node("#pause").dataset["aria-pressed"],
    },
  );
}

console.log("① Avatar 是 native button；冷啟動與輪詢都不會替使用者點它");
{
  const tag = HTML.replace(/<!--[\s\S]*?-->/gu, "").match(/<button[^>]*data-avatar[^>]*>/u)?.[0] ?? "";
  check("HTML 用 button", tag.startsWith("<button"), tag);
  check("button 明列 type=button", /\stype="button"/u.test(tag), tag);
  check("台詞格開場是 hidden", /<p[^>]*data-persona-line[^>]*hidden/u.test(HTML));

  const p = await open();
  check("冷啟動台詞是空的", p.node("[data-persona-line]").textContent === "");
  check("冷啟動台詞藏著", p.node("[data-persona-line]").hidden);
  await p.poll();
  await p.poll();
  check("跑兩輪 polling 仍是空的", p.node("[data-persona-line]").textContent === "");
  await p.syntheticAvatarClick();
  check("程式合成的 click 仍然是空的", p.node("[data-persona-line]").textContent === "");
  check("沒有自動播放", p.plays() === 0, p.plays());
  check("畫面上沒出現 undefined / NaN", p.nonsense().length === 0, p.nonsense());
}

console.log("②b 拖她的時候不會順便讓她說話，拖完之後還戳得動");
{
  // 這支夾具每次都送得出 pointerup，真機器上常常送不出——`startDragging()` 一發動
  // 作業系統就接管，webview 收不到後續事件。所以這裡比真實情況嚴格：把旗標改成
  // 清在 pointerup 會被第二條抓到，而那個寫法在真機器上反而看不出症狀（她只是不
  // 說話）。四刀突變實測會紅的組合：拿掉門檻／門檻改 0 → 第四條；pointerdown 不
  // 重設旗標 → 第三條；click 不看旗標 → 第二、三條。
  // 她是可以拖著走的桌寵，而拖曳和戳她共用同一顆按鈕。分辨錯了有兩種壞法：
  // 拖完她突然開口，或者從此再也戳不動。第三條就是在守後面那種——旗標必須在
  // **下一次 pointerdown** 清掉，不能清在 pointerup：`startDragging()` 一發動，
  // 作業系統就接管拖曳迴圈，webview 根本收不到 pointerup。
  // 這一段問的是「拖還是戳」，不是「講了哪一句」——她講的話現在是隨機的
  // （見 ② 那一段）。所以這裡只斷言「有沒有開口」，而且開的口必須是這位角色
  // 真的有的那幾句，不能是空字串或別人的台詞。
  const pool = pokePool("chatgpt");
  const p = await open(persona("chatgpt"));

  await p.avatarGesture();
  const poked = p.node("[data-persona-line]").textContent;
  check("按下去沒移動＝戳她一下，要說話", poked !== "" && pool.includes(poked), poked);

  const beforeDrag = p.node("[data-persona-line]").textContent;
  await p.avatarGesture({ dx: 40, dy: 60 });
  check(
    "移動超過門檻＝拖，補上來的那個 click 不可以讓她說話",
    p.node("[data-persona-line]").textContent === beforeDrag,
    { before: beforeDrag, after: p.node("[data-persona-line]").textContent },
  );

  await p.avatarGesture();
  const afterDrag = p.node("[data-persona-line]").textContent;
  check(
    "拖完之後，下一次真正的戳還要有效",
    afterDrag !== "" && pool.includes(afterDrag) && afterDrag !== beforeDrag,
    { beforeDrag, afterDrag },
  );

  // 手指抖一下不算拖。門檻底下的移動仍然是「戳她」。
  const p2 = await open(persona("chatgpt"));
  await p2.avatarGesture({ dx: 2, dy: 1 });
  const nudged = p2.node("[data-persona-line]").textContent;
  check("門檻以下的微小移動仍然算戳她", nudged !== "" && pool.includes(nudged), nudged);
}

console.log("②c 拖完之後，透明的地方要再變得點得過去");
{
  // 她那扇窗整片透明，可是作業系統照整個 340×560 的矩形收點擊，所以 renderer
  // 要一直回報「哪裡真的畫了東西」，Rust 才翻得動 `set_ignore_cursor_events`。
  //
  // 按住她的那一刻整扇窗都要算實心，不然拖到一半開關被翻成穿透就斷在半路。
  // 問題出在怎麼把它收回來：`startDragging()` 一發動，作業系統就接管拖曳迴圈，
  // webview **通常收不到 pointerup**（同一份檔案 `draggedThisPress` 那段註解講的
  // 是同一件事）。只掛 pointerup 的話，他拖她一次之後整扇窗就永遠留在實心，
  // 這條線從此靜悄悄地失效——而「拖她」正是他最先會做的那件事。
  //
  // 所以第二條刻意**不送 pointerup**，只送一個沒按著鍵的 pointermove。
  const settle = () => new Promise((done) => setTimeout(done, 200));
  const whole = (push) =>
    Array.isArray(push) && push.length === 1 && push[0].w >= 340 && push[0].h >= 560;

  const p = await open(persona("chatgpt"));
  await settle();
  check("開場就回報過一次（Rust 那邊空清單＝一律實心，會擋住整片桌面）", p.solidPushes.length > 0, p.solidPushes.length);
  check(
    "平常回報的不是整扇窗，透明的地方才點得過去",
    !whole(p.solidPushes.at(-1)),
    p.solidPushes.at(-1)?.length,
  );

  const avatarEl = p.node("[data-avatar]");
  for (const fn of avatarEl.handlers.pointerdown ?? []) fn({ button: 0, clientX: 10, clientY: 10 });
  check("按住她的時候整扇窗都算實心，拖曳才不會斷在半路", whole(p.solidPushes.at(-1)), p.solidPushes.at(-1));

  // 作業系統把 pointerup 吃掉了——只有滑鼠回到窗上這一個訊號。
  p.fireWindow("pointermove", { buttons: 0 });
  await settle();
  check(
    "就算 pointerup 被作業系統吃掉，整扇窗也要收得回來",
    !whole(p.solidPushes.at(-1)),
    p.solidPushes.at(-1)?.length,
  );

  // 還按著的時候不可以提早收——那會在拖曳途中把開關翻掉。
  for (const fn of avatarEl.handlers.pointerdown ?? []) fn({ button: 0, clientX: 10, clientY: 10 });
  p.fireWindow("pointermove", { buttons: 1 });
  await settle();
  check("手還按著就不算拖完，整扇窗要繼續實心", whole(p.solidPushes.at(-1)), p.solidPushes.at(-1));
}

console.log("②ᵈ 上面那條膠囊平常不在，也不擋滑鼠；她真的停下來才自己出現");
{
  /*
   * 他的原話：「上面那一槓 可不可以改成從下面按一個鍵才跑出來 平常上面也是
   * 透明的?」所以預設是「不在」，而「不在」是兩件事：看不到，而且點得穿。
   *
   * 這兩件事由**兩份檔案**合起來成立：CSS 收起來用的是 `visibility: hidden`，
   * 而 `paintedRects()` 明文跳過那一種。任何一邊自己改掉都編得過、畫面也看不
   * 出差別——留下的是她頭頂一塊看不見的擋板，底下的桌面點不下去，而使用者
   * 只會覺得「這裡怪怪的」，回報不出來。所以兩邊各釘一條。
   *
   * 另一半是那顆暫停鍵住在裡面。她整個產品的前提是「你隨時停得掉」，所以「藏
   * 起來」不可以連「她停著的時候也藏起來」一起藏：`body.she-is-stopped` 那一格
   * 不歸這顆鍵管，按下去也關不掉。
   */
  const dragbarRules = directElementRules("dragbar");
  const collapsed = dragbarRules.find((rule) => rule.selector.includes(":not(.chrome-open)"));
  check(
    "收起來的規則在，而且只在 body 兩個 class 都沒有的時候才套",
    collapsed?.selector === "body:not(.chrome-open):not(.she-is-stopped) .dragbar",
    collapsed?.selector ?? dragbarRules.map((rule) => rule.selector),
  );
  check(
    "收起來用的是 visibility: hidden，不是只把顏色調透明",
    /visibility:\s*hidden/u.test(collapsed?.body ?? ""),
    collapsed?.body,
  );
  const paintedRectsBody =
    SRC.match(/\nfunction paintedRects\(\) \{\n(?<body>[\s\S]*?)\n\}/u)?.groups?.body ?? "";
  check(
    "另一半：`paintedRects()` 明文跳過 visibility: hidden，「看不見」才等於「點得穿」",
    /style\.visibility === "hidden"/u.test(paintedRectsBody),
    paintedRectsBody.slice(0, 400),
  );

  const barMarkup = HTML.match(/<header class="dragbar"[\s\S]*?<\/header>/u)?.[0] ?? "";
  const toggleTag = HTML.match(/<button\b[^>]*data-chrome-toggle[^>]*>/u)?.[0] ?? "";
  check("暫停鍵住在那條膠囊裡", /id="pause"/u.test(barMarkup), barMarkup.slice(0, 120));
  check(
    "那顆「⋯」不可以也住在膠囊裡——收起來之後就沒有東西按得到了",
    toggleTag !== "" && !barMarkup.includes("data-chrome-toggle"),
    { toggleTag, inBar: barMarkup.includes("data-chrome-toggle") },
  );
  check(
    "出貨的開場值就是收起來的（那顆鍵不在開場路徑上，這一格只有 HTML 說得算）",
    /aria-expanded="false"/u.test(toggleTag),
    toggleTag,
  );
  check(
    "`aria-controls` 指到的就是那條膠囊",
    /aria-controls="chrome-bar"/u.test(toggleTag) && /id="chrome-bar"/u.test(barMarkup),
    { toggleTag, barId: /id="[^"]*"/u.exec(barMarkup)?.[0] },
  );
  // 把這一排藏起來的前提是「還有另一條路停得掉她」。那條路是系統匣，而那個選單
  // 有**兩種**組合（有沒有 metrics 那一項），所以要兩臂都數過——只改一臂是這個
  // 檔案裡犯過好幾次的形狀，而漏掉的那一臂在畫面上長得一模一樣。
  const trayArms = [
    ...MAIN.matchAll(/Menu::with_items\(\s*\n\s*app,\s*\n\s*&\[(?<items>[\s\S]*?)\],\s*\n\s*\)\?/gu),
  ].map(({ groups }) => groups.items);
  check(
    "系統匣那條保險：每一種選單組合都含著暫停（這才敢把上面那一排收起來）",
    trayArms.length >= 2 && trayArms.every((items) => items.includes("&pause_item")),
    { arms: trayArms.length, withPause: trayArms.filter((i) => i.includes("&pause_item")).length },
  );

  const bubbleShift = directElementRules("answer-bubble").find(
    (rule) => rule.selector.includes(".chrome-open") && rule.selector.includes(".she-is-stopped"),
  );
  check(
    "膠囊出來的時候氣泡要讓開，而且兩種出現的理由都要讓",
    bubbleShift !== undefined && /top:\s*\d/u.test(bubbleShift.body),
    bubbleShift ?? directElementRules("answer-bubble").map((rule) => rule.selector),
  );

  const settle = () => new Promise((done) => setTimeout(done, 200));
  const p = await open(persona("chatgpt"));
  const expanded = () => p.node("[data-chrome-toggle]").dataset["aria-expanded"];
  check("開機一律是收起來的", !p.body.classList.contains("chrome-open"), [
    ...p.body.classList._s,
  ]);
  check(
    "還在確認她起來沒的時候不算「停著」——不然她一開機就把膠囊掛在他桌面上",
    !p.body.classList.contains("she-is-stopped"),
    [...p.body.classList._s],
  );

  const beforeToggle = p.calls.length;
  await p.clickChromeToggle();
  check(
    "從下面按一下就跑出來",
    p.body.classList.contains("chrome-open") && expanded() === "true",
    { classes: [...p.body.classList._s], expanded: expanded() },
  );
  await p.clickChromeToggle();
  check(
    "再按一下收回去",
    !p.body.classList.contains("chrome-open") && expanded() === "false",
    { classes: [...p.body.classList._s], expanded: expanded() },
  );
  const wrote = p.calls.slice(beforeToggle).filter((cmd) => cmd !== "pet_solid_set");
  check(
    "開關它不存任何設定——重開一律回到收起來，不必去猜畫面會長什麼樣",
    wrote.length === 0,
    wrote,
  );

  // 開場自己排了一次推送（`scheduleSolidPush()` 在接線的最後一行），防手震
  // 80 毫秒。不先把它放掉的話，下面量到的那一次是**它**，而不是 class 換掉造成
  // 的那一次——實測就是這樣：把 `"class"` 從 `attributeFilter` 拿掉，這一條照樣
  // 綠。所以先等它落地，再取基準。
  await settle();
  const beforePushes = p.solidPushes.length;
  await p.clickChromeToggle();
  await settle();
  check(
    "前提：沒有人通知的話，光是改 class 不會自己推一次——下一條量到的才是那個 observer",
    p.solidPushes.length === beforePushes,
    { before: beforePushes, after: p.solidPushes.length },
  );
  const watching = p.fireMutations();
  await settle();
  check(
    "class 一換就重算一次實心區，不然膠囊出來了還是點不到",
    watching > 0 && p.solidPushes.length > beforePushes,
    { watching, before: beforePushes, after: p.solidPushes.length },
  );
  check(
    "叫出來之後，裡面那五顆鍵對讀螢幕的人也回到 Tab 順序上",
    p.node("[data-chrome-bar]").dataset["aria-hidden"] === "false",
    p.node("[data-chrome-bar]").dataset["aria-hidden"],
  );
  await p.clickChromeToggle();
  p.fireMutations();
  check(
    "收回去的時候它們也要一起離開 Tab 順序",
    p.node("[data-chrome-bar]").dataset["aria-hidden"] === "true",
    p.node("[data-chrome-bar]").dataset["aria-hidden"],
  );

  // `scripts/shot.mjs` 用的是 `element.click()`（`isTrusted === false`）。這顆
  // 擋了 `isTrusted` 的話，這一頁唯一的版面驗證工具就只看得到收起來那一種。
  const synthetic = await open(persona("chatgpt"));
  await synthetic.clickChromeToggle({ isTrusted: false });
  check(
    "合成的 click 也開得起來，不然唯一的版面驗證工具永遠只看得到收起來的樣子",
    synthetic.body.classList.contains("chrome-open"),
    [...synthetic.body.classList._s],
  );
  check(
    "`scripts/shot.mjs` 真的是用 element.click()（上一條的前提）",
    /\.click\(\)/u.test(SHOT),
    SHOT.match(/.*\.click\(\).*/u)?.[0],
  );

  // 她停下來的時候那條膠囊自己要出現，而且那顆鍵關不掉它。看不到她在錄和看不到
  // 她停著在畫面上是同一種樣子，而後者要一眼看得出來——「繼續」也要一下按得到。
  for (const [name, options] of [
    ["按了暫停", { pauseState: true }],
    ["整個停下來", { masterStopState: "stopped" }],
    ["正在停", { masterStopState: "stopping" }],
    ["停成什麼樣不確定", { masterStopState: "uncertain" }],
  ]) {
    const halted = await open(persona("chatgpt"), options);
    await halted.poll();
    check(
      `${name}的時候，那條膠囊自己出現——不用先找那顆點點鍵`,
      halted.body.classList.contains("she-is-stopped"),
      [...halted.body.classList._s],
    );
    await halted.clickChromeToggle();
    await halted.clickChromeToggle();
    check(
      `${name}的時候，那顆點點鍵關不掉它`,
      halted.body.classList.contains("she-is-stopped") &&
        !halted.body.classList.contains("chrome-open"),
      [...halted.body.classList._s],
    );
    halted.fireMutations();
    check(
      `${name}的時候，裡面那顆「繼續」對讀螢幕的人也要在`,
      halted.node("[data-chrome-bar]").dataset["aria-hidden"] === "false",
      halted.node("[data-chrome-bar]").dataset["aria-hidden"],
    );
  }

  const running = await open(persona("chatgpt"));
  await running.poll();
  check(
    "她好好在跑的時候它不在——不然「平常上面也是透明的」就沒了",
    !running.body.classList.contains("she-is-stopped"),
    [...running.body.classList._s],
  );
}

console.log("② 每一下 click 只走 allowlist；講哪一句是隨機的，而且不連兩次同一句");
{
  // 這一段以前守的是「第一句固定、第二句固定、第三下回到第一句」。那個契約
  // 就是他實際看到的毛病：「目前按來按去只會一直講兩句話」。桌寵被戳的時候
  // 該像個活人——阿唷、煩耶、幹嘛啦——所以契約換成三條：
  //
  //   1. 講出來的一定是這位角色自己有的那幾句（不會憑空生字，也不會借別人的）
  //   2. 連著兩下不准講同一句（`pickBanter` 的 filter）
  //   3. 骰子鋪開的時候，講得出來的句子要真的鋪開（不是兩句在那裡輪）
  //
  // 亂數在這裡是被接管的，所以上面三條都是定值，不是機率。
  const pool = pokePool("chatgpt");
  check(
    "被戳的時候她講得出來的話不只那兩句",
    pool.length >= 12,
    { pool: pool.length, sample: pool.slice(0, 3) },
  );

  const p = await open(persona("chatgpt"));
  const before = p.calls.length;
  const stateBefore = p.node("[data-state-line]").textContent;
  scriptRandom(0);
  await p.clickAvatar();
  check(
    "第一句是這位角色自己的日常台詞",
    p.node("[data-persona-line]").textContent === "我在，隨時可以開始。",
    p.node("[data-persona-line]").textContent,
  );
  check("點了才顯示", !p.node("[data-persona-line]").hidden);
  check("沒改錄製狀態字", p.node("[data-state-line]").textContent === stateBefore, p.node("[data-state-line]").textContent);
  check("沒叫 ask／CLI／Gatekeeper／hands", p.calls.length === before, p.calls.slice(before));

  // 骰子卡死在同一格——這是「不連兩次同一句」唯一逼得出來的情況。少了那道
  // filter，底下這 12 下會是同一句話講 12 次。
  const stuck = [];
  for (let i = 0; i < 12; i += 1) {
    await p.clickAvatar();
    stuck.push(p.node("[data-persona-line]").textContent);
  }
  check(
    "骰子擲出同一格，她也不會連著講同一句",
    stuck.every((line, i) => i === 0 || line !== stuck[i - 1]),
    stuck,
  );
  check("卡死的骰子講出來的仍然都是她的話", stuck.every((line) => pool.includes(line)), stuck);

  // 換成真的有鋪開的骰子（固定種子，所以這個數字在任何機器上都一樣）。
  const spread = await open(persona("chatgpt"));
  scriptRandomSequence(20260912);
  const heard = [];
  for (let i = 0; i < 40; i += 1) {
    await spread.clickAvatar();
    heard.push(spread.node("[data-persona-line]").textContent);
  }
  stopScriptingRandom();
  const distinct = new Set(heard);
  check("戳 40 下至少聽得到 8 種不同的話", distinct.size >= 8, [...distinct]);
  check("每一句都在她自己的清單裡", heard.every((line) => pool.includes(line)), [...distinct]);
  check(
    "任何一下都不會複誦上一下",
    heard.every((line, i) => i === 0 || line !== heard[i - 1]),
    heard,
  );
  check(
    "阿唷／煩耶／幹嘛啦這種話真的會出現",
    ["阿唷，會痛耶。", "煩耶。", "幹嘛啦。"].some((line) => distinct.has(line)),
    [...distinct],
  );

  // 戳下去要**看得到**她動一下，而且那一下不歸「固定台詞」那個開關管。那個開關
  // 管的是她說不說話，不是她理不理你——關掉台詞之後戳下去整個人一動也不動，
  // 那不是安靜，那是當掉。所以這兩條要分開問：一條問有沒有字，一條問有沒有動。
  const jolted = await open(persona("chatgpt"));
  await jolted.clickAvatar();
  check(
    "戳一下她整個人會抖一下",
    jolted.node("[data-avatar]").classList.contains("poked"),
    [...jolted.node("[data-avatar]").classList._s],
  );

  const quietButAlive = await open(persona("chatgpt", { tap_lines: false }));
  await quietButAlive.forceAvatarHandler();
  check(
    "關掉台詞她不說話，但還是理你",
    quietButAlive.node("[data-persona-line]").textContent === "" &&
      quietButAlive.node("[data-avatar]").classList.contains("poked"),
    {
      line: quietButAlive.node("[data-persona-line]").textContent,
      classes: [...quietButAlive.node("[data-avatar]").classList._s],
    },
  );
}

console.log("③ 17 個本機角色、圖檔、tagline、palette 都固定而且可讀");
check("鍵盤 focus ring 對固定 stage ≥ 4.5:1", contrast("#F6ECDF", "#955572") >= 4.5);
check(
  "focus ring 使用 stage theme，不借 persona accent",
  /\.avatar:focus-visible\s*\{[^}]*var\(--letter-accent\)/u.test(read(join(UI, "styles.css"))),
);
check("HTML 沒有字母 glyph fallback", !HTML.includes("data-persona-glyph"));
check("renderer 沒有字母 glyph 路徑", !SRC.includes("data-persona-glyph"));
const bundledBytes = BUNDLED.assets.reduce((total, asset) => total + asset.bytes, 0);
check(
  "bundled preview manifest 是透明 640² contain 全身 v2 契約",
  BUNDLED.schema === "ai-sister/bundled-personas/v2" &&
    BUNDLED.previewContract?.alpha === "transparent" &&
    BUNDLED.previewContract?.canvas?.width === 640 &&
    BUNDLED.previewContract?.canvas?.height === 640 &&
    BUNDLED.previewContract?.fit === "contain" &&
    BUNDLED.previewContract?.subject === "full-body",
  { schema: BUNDLED.schema, previewContract: BUNDLED.previewContract },
);
check(
  "manifest totals 是 exact 17 張／1,031,124 bytes，而且等於逐檔加總",
  BUNDLED.totals?.assets === 17 &&
    BUNDLED.totals?.webpBytes === 1_031_124 &&
    BUNDLED.totals.webpBytes === bundledBytes,
  { totals: BUNDLED.totals, bundledBytes },
);
const bundledManifestDigest = createHash("sha256")
  .update(readFileSync(join(UI, "personas/manifest.json")))
  .digest("hex");
const bundledNoticeDigest = createHash("sha256")
  .update(readFileSync(join(UI, "personas/NOTICE.md")))
  .digest("hex");
check(
  "bundled preview manifest／owner NOTICE 都是核准的 exact bytes",
  bundledManifestDigest === "cf8e6e1b22f90f09ba021c092c3e0e9f5ae0dd39cf5644ffdfeb457ff3dd69c0" &&
    bundledNoticeDigest === "981557ff2db030abf75a644fd6fea2a50e69e7aedb27197406fbc324e05712fc",
  { bundledManifestDigest, bundledNoticeDigest },
);
check(
  "設定選單與目前角色 preview 都用 contain，不把透明全身圖裁回頭像",
  PERSONA_CHOICE_IMAGE_CSS.includes("object-fit: contain;") &&
    PERSONA_PREVIEW_IMAGE_CSS.includes("object-fit: contain;") &&
    !PERSONA_CHOICE_IMAGE_CSS.includes("object-fit: cover;") &&
    !PERSONA_PREVIEW_IMAGE_CSS.includes("object-fit: cover;"),
  { choice: PERSONA_CHOICE_IMAGE_CSS, preview: PERSONA_PREVIEW_IMAGE_CSS },
);
check("bundle manifest 恰好 17 人", BUNDLED.assets.length === 17, BUNDLED.assets.length);
const expectedIds = Object.keys(EXPECTED).sort();
const manifestIds = BUNDLED.assets.map((asset) => asset.id).sort();
const personaSelect = SETTINGS_HTML.match(/<select[^>]*data-persona-id[^>]*>[\s\S]*?<\/select>/u)?.[0] ?? "";
const settingsIds = [...personaSelect.matchAll(/<option value="([^"]+)">/gu)]
  .map((match) => match[1])
  .sort();
const catalogIds = (SETTINGS_CATALOG?.personas ?? []).map((persona) => persona.id).sort();
check(
  "manifest 是 exact 17 IDs，不只剛好有 17 列",
  JSON.stringify(manifestIds) === JSON.stringify(expectedIds),
  manifestIds,
);
check(
  "設定選單也是同一組 exact 17 IDs",
  JSON.stringify(settingsIds) === JSON.stringify(expectedIds),
  settingsIds,
);
check(
  "圖像選角 projection 也是同一組 exact 17 IDs",
  SETTINGS_CATALOG?.schema === "ai-sister/persona-catalog/v1" &&
    JSON.stringify(catalogIds) === JSON.stringify(expectedIds),
  catalogIds,
);
const catalogById = Object.fromEntries(
  (SETTINGS_CATALOG?.personas ?? []).map((persona) => [persona.id, persona]),
);
const sisterIds = new Set(["chatgpt", "claude", "gemini", "grok"]);
check(
  "選角 projection 的 alias／group／tagline／WebP path 全部對回 runtime 與 manifest",
  expectedIds.every((id) => {
    const persona = catalogById[id];
    const bundled = BUNDLED.assets.find((asset) => asset.id === id);
    return (
      persona?.alias === EXPECTED[id].alias &&
      persona?.tagline === EXPECTED[id].tagline &&
      persona?.group === (sisterIds.has(id) ? "四姊妹" : "13 位閨密") &&
      persona?.portrait === `./personas/${bundled?.file}`
    );
  }),
  catalogById,
);
const catalogScript = SETTINGS_HTML.indexOf('<script src="./personas/catalog.js"></script>');
const settingsScript = SETTINGS_HTML.indexOf('<script type="module" src="./settings.js"></script>');
check(
  "設定頁先載純本機 catalog，而且完全不載 Reel manifest",
  catalogScript >= 0 &&
    settingsScript > catalogScript &&
    !SETTINGS_HTML.includes("persona-reels/manifest.js") &&
    !SETTINGS.includes("persona-reels") &&
    !/\bfetch\s*\(/u.test(SETTINGS),
  { catalogScript, settingsScript },
);
for (const [id, expected] of Object.entries(EXPECTED)) {
  const p = await open(persona(id));
  const avatar = p.node("[data-avatar]");
  check(`${id} 穩定 ID`, avatar.dataset.persona === id, avatar.dataset.persona);
  check(
    `${id} 使用 bundled portrait`,
    p.node("[data-persona-portrait]").src === `./personas/${id}.webp` &&
      !p.node("[data-persona-portrait]").hidden,
    p.node("[data-persona-portrait]").src,
  );
  check(`${id} alias/tagline`, avatar.title === `${expected.alias}：${expected.tagline}`, avatar.title);
  check(
    `${id} 設定頁 tagline 同步`,
    catalogById[id]?.tagline === expected.tagline,
    catalogById[id]?.tagline,
  );
  const actualPalette = [
    p.css.get("--persona-bg"),
    p.css.get("--persona-fg"),
    p.css.get("--persona-accent"),
  ];
  check(`${id} palette`, JSON.stringify(actualPalette) === JSON.stringify(expected.palette), actualPalette);
  check(`${id} 字與底 ≥ 7:1`, contrast(expected.palette[0], expected.palette[1]) >= 7);
  check(`${id} accent 與底 ≥ 4.5:1`, contrast(expected.palette[0], expected.palette[2]) >= 4.5);
  check(
    `${id} 不覆寫淺色 stage 的 UI theme`,
    !p.css.has("--letter-bg") && !p.css.has("--letter-fg") && !p.css.has("--letter-accent"),
    [...p.css.entries()],
  );
  // 骰子釘在第一格，才問得出「這位角色的日常台詞是不是她自己的」。她被戳的
  // 時候實際會隨機挑一句（見 ②）；這裡要驗的是 17 份逐字稿沒有互相串位。
  scriptRandom(0);
  await p.clickAvatar();
  stopScriptingRandom();
  check(`${id} 第一條 tap copy`, p.node("[data-persona-line]").textContent === expected.first, p.node("[data-persona-line]").textContent);

  const bundled = BUNDLED.assets.find((asset) => asset.id === id);
  const file = join(UI, "personas", bundled?.file ?? "missing");
  const bytes = statSync(file).size;
  const digest = createHash("sha256").update(readFileSync(file)).digest("hex");
  check(`${id} manifest size/hash 對上 shipped bytes`, bytes === bundled?.bytes && digest === bundled?.sha256, { bytes, digest, bundled });
}

console.log("③ᵇ Reel 只建 active rig；整組 decode 前與任何失敗都保留 WebP");
{
  const manifestScript = HTML.indexOf('<script src="./persona-reels/manifest.js"></script>');
  const voiceManifestScript = HTML.indexOf(
    '<script src="./persona-voices/v1/manifest.js"></script>',
  );
  const banterManifestScript = HTML.indexOf(
    '<script src="./persona-banter-voices/v1/manifest.js"></script>',
  );
  const appScript = HTML.indexOf('<script type="module" src="./app.js"></script>');
  check(
    "角色圖與兩包語音 manifest 都在 app.js 前預載",
    manifestScript >= 0 &&
      voiceManifestScript > manifestScript &&
      banterManifestScript > voiceManifestScript &&
      appScript > banterManifestScript,
    { manifestScript, voiceManifestScript, banterManifestScript, appScript },
  );
  check("HTML 有 hidden Reel 容器", /data-persona-reel[^>]*hidden/u.test(HTML));

  const noManifest = await open(persona("chatgpt"));
  check(
    "沒有 manifest 完全照舊，不建立動態 layer",
    noManifest.reelImages.length === 0 &&
      !noManifest.node("[data-persona-portrait]").hidden &&
      noManifest.node("[data-persona-reel]").hidden,
    noManifest.reelImages.map((image) => image.src),
  );

  const pending = await open(persona("chatgpt"), {
    reelManifest: REELS,
    deferReelLoads: true,
  });
  const chatgptRig = REELS.rigs.find((rig) => rig.id === "chatgpt");
  check(
    "只建立 active ChatGPT 的 21–26 層",
    pending.reelImages.length === chatgptRig.layers.length &&
      pending.reelImages.every(
        (image) => image.src.startsWith("./persona-reels/rigs/chatgpt/") && image.reelId === "chatgpt",
      ),
    pending.reelImages.map((image) => image.src),
  );
  const firstLayer = chatgptRig.layers[0];
  const firstImage = pending.reelImages[0];
  check(
    "layer 位置以 manifest 全畫布 viewport 換算，保留透明全身座標",
    firstImage.style.left ===
      `${(((firstLayer.x - chatgptRig.viewport.x) / chatgptRig.viewport.width) * 100).toFixed(5)}%` &&
      firstImage.style.top ===
        `${(((firstLayer.y - chatgptRig.viewport.y) / chatgptRig.viewport.height) * 100).toFixed(5)}%` &&
      firstImage.style.width ===
        `${((firstLayer.width / chatgptRig.viewport.width) * 100).toFixed(5)}%`,
    { left: firstImage.style.left, top: firstImage.style.top, width: firstImage.style.width },
  );
  check(
    "全身座標格沒有底色／邊框／圓角裁切，陰影跟著人物 alpha",
    FACE_CSS.includes("height: 100%;") &&
      FACE_CSS.includes("width: 100%;") &&
      !/(?:^|\n)\s*(?:background|border|box-shadow|overflow)\s*:/u.test(FACE_CSS) &&
      REEL_CSS.includes("filter: drop-shadow(") &&
      !/(?:^|\n)\s*(?:background|border|border-radius|overflow)\s*:/u.test(REEL_CSS),
    { face: FACE_CSS, reel: REEL_CSS },
  );
  check(
    "flatten WebP 與 rig 都撐滿同一座標格，回答出現也不改人物尺度",
    PORTRAIT_CSS.includes("height: 100%;") &&
      PORTRAIT_CSS.includes("width: 100%;") &&
      /\.avatar\s*\{[^}]*height:\s*300px;[^}]*width:\s*300px;/su.test(STYLES) &&
      !STYLES.includes("body.has-hits .avatar"),
    PORTRAIT_CSS,
  );
  check(
    "paused／asleep 的米色空心點都有固定深色外圈，不靠人物 palette 或動畫辨識",
    PAUSED_MARKER_CSS.includes("border-color: var(--letter-fg);") &&
      ASLEEP_MARKER_CSS.includes("border-color: var(--letter-fg);"),
    { paused: PAUSED_MARKER_CSS, asleep: ASLEEP_MARKER_CSS },
  );
  check(
    "paused 的斜線是獨立深色 marker；asleep 不借用同一條斜線",
    PAUSED_SLASH_CSS.includes("content: \"\";") &&
      PAUSED_SLASH_CSS.includes("background: var(--letter-fg);") &&
      PAUSED_SLASH_CSS.includes("height: 3px;") &&
      PAUSED_SLASH_CSS.includes("width: 27px;") &&
      PAUSED_SLASH_CSS.includes("transform: rotate(-45deg);") &&
      !STYLES.includes('.avatar[data-state="asleep"]::after'),
    PAUSED_SLASH_CSS,
  );
  const frameProperty =
    /(?:^|\n)\s*(?:background(?:-color)?|border(?:-radius)?|box-shadow|overflow(?:-[xy])?)\s*:/u;
  const frameRules = ["face", "portrait", "persona-reel"]
    .flatMap((className) =>
      directElementRules(className).map((rule) => ({ className, ...rule })),
    )
    .filter(({ body }) => frameProperty.test(body));
  check(
    "所有狀態／compact／reduced-motion selector 都不能把相框加回人物容器",
    frameRules.length === 0,
    frameRules,
  );
  const reorderedIndex = chatgptRig.layers.findIndex((layer) => layer.render_z !== layer.z);
  const reorderedLayer = chatgptRig.layers[reorderedIndex];
  const reorderedImage = pending.reelImages[reorderedIndex];
  check(
    "眼部用 canonical render_z 而不是 raw z，role 才能決定眨眼層",
    reorderedIndex >= 0 &&
      reorderedImage.style.zIndex === String(reorderedLayer.render_z) &&
      reorderedImage.dataset.reelRole === reorderedLayer.role &&
      Object.hasOwn(reorderedImage.dataset, "reelEye") === (reorderedLayer.role === "eye"),
    { layer: reorderedLayer, zIndex: reorderedImage?.style.zIndex },
  );
  check(
    "全組 load/decode 完成前 WebP 不消失也不露半套 rig",
    !pending.node("[data-persona-portrait]").hidden &&
      pending.node("[data-persona-reel]").hidden &&
      pending.node("[data-persona-reel]").children.length === 0,
  );
  await pending.settleReel("chatgpt");
  check(
    "每層都 decode 成功後才一次換成完整 rig",
      pending.reelImages.every((image) => image.decodeCalls === 1) &&
      pending.node("[data-persona-reel]").children.length === chatgptRig.layers.length &&
      !pending.node("[data-persona-reel]").hidden &&
      !pending.node("[data-persona-portrait]").hidden &&
      pending.node("[data-avatar]").classList.contains("reel-ready") &&
      pending.node("[data-avatar]").dataset.reel === "ready",
    {
      decodes: pending.reelImages.map((image) => image.decodeCalls),
      children: pending.node("[data-persona-reel]").children.length,
    },
  );

  const brokenLayer = chatgptRig.layers[3].file;
  const broken = await open(persona("chatgpt"), {
    reelManifest: REELS,
    reelFailure: { kind: "decode", file: brokenLayer },
  });
  check(
    "任一層 decode 失敗就整組丟掉並保留 WebP",
    broken.node("[data-persona-reel]").children.length === 0 &&
      broken.node("[data-persona-reel]").hidden &&
      !broken.node("[data-persona-portrait]").hidden &&
      !Object.hasOwn(broken.node("[data-avatar]").dataset, "reel"),
    brokenLayer,
  );

  const race = await open(persona("chatgpt"), {
    reelManifest: REELS,
    deferReelLoads: true,
  });
  await race.fromOutside("persona-changed", persona("claude"));
  const claudeRig = REELS.rigs.find((rig) => rig.id === "claude");
  check(
    "切換只再建立新 active Claude 的 layers",
    race.reelImages.filter((image) => image.reelId === "chatgpt").length ===
      chatgptRig.layers.length &&
      race.reelImages.filter((image) => image.reelId === "claude").length ===
        claudeRig.layers.length &&
      race.reelImages
        .filter((image) => image.reelId === "chatgpt")
        .every((image) => image.src === "" && image.onload === null && image.onerror === null),
    race.reelImages.map((image) => image.reelId),
  );
  await race.settleReel("claude");
  check(
    "新角色先完成就只顯示新角色",
    race.node("[data-persona-reel]").children.length === claudeRig.layers.length &&
      race.node("[data-persona-reel]").children.every((image) => image.reelId === "claude"),
  );
  await race.settleReel("chatgpt");
  check(
    "舊角色晚 load/decode 不會覆蓋新角色",
    race.node("[data-avatar]").dataset.persona === "claude" &&
      race.node("[data-persona-reel]").children.length === claudeRig.layers.length &&
      race.node("[data-persona-reel]").children.every((image) => image.reelId === "claude"),
  );

  const shippedRigResults = [];
  for (const id of Object.keys(EXPECTED)) {
    const page = await open(persona(id), { reelManifest: REELS });
    const rig = REELS.rigs.find((candidate) => candidate.id === id);
    shippedRigResults.push({
      id,
      expected: rig?.layers.length ?? null,
      created: page.reelImages.length,
      published: page.node("[data-persona-reel]").children.length,
      ready: page.node("[data-avatar]").dataset.reel ?? null,
    });
  }
  check(
    "出貨的 17 個 rig 都通過同一份 runtime 契約",
    shippedRigResults.every(
      ({ expected, created, published, ready }) =>
        expected >= 21 &&
        expected <= 26 &&
        created === expected &&
        published === expected &&
        ready === "ready",
    ),
    shippedRigResults,
  );

  const forged = JSON.parse(JSON.stringify(REELS));
  forged.rigs.find((rig) => rig.id === "chatgpt").layers[0].file =
    "rigs/chatgpt/../claude/00_back_hair.png";
  const rejected = await open(persona("chatgpt"), { reelManifest: forged });
  check(
    "manifest 路徑不能跨 persona，也不能讓 runtime 建 img",
    rejected.reelImages.length === 0 && !rejected.node("[data-persona-portrait]").hidden,
    rejected.reelImages.map((image) => image.src),
  );
  const unapproved = await open(persona("chatgpt"), {
    reelManifest: { ...REELS, rights_review: "pending" },
  });
  check(
    "runtime 只接受已核准的 owner grant 投影",
    unapproved.reelImages.length === 0 && !unapproved.node("[data-persona-portrait]").hidden,
  );
  check(
    "runtime 沒有 fetch，PNG URL 只能固定 prepend 本機 persona-reels",
    !/\bfetch\s*\(/u.test(SRC) &&
      SRC.includes('image.src = `./persona-reels/${layer.file}`') &&
      SRC.includes('const prefix = `rigs/${id}/`'),
  );
  check(
    "Reel 動畫只在 motion + idle/thinking；paused/asleep 有 blanket stop",
    STYLES.includes('.motion .avatar[data-state="idle"] .persona-reel-layer[data-reel-eye]') &&
      STYLES.includes('.motion .avatar[data-state="thinking"] .persona-reel-layer[data-reel-eye]') &&
      STYLES.includes('.avatar[data-state="paused"] .persona-reel-layer') &&
      STYLES.includes('.avatar[data-state="asleep"] .persona-reel-layer') &&
      !STYLES.includes("data-reel-drift") &&
      !STYLES.includes("data-reel-mouth"),
  );
  check(
    "答案／URL 氣泡仍保留同尺寸已 decode 全身 rig，不換回相框 WebP",
    STYLES.includes(".avatar.reel-ready .portrait {\n  visibility: hidden;") &&
      STYLES.includes("body.has-hits .answer-bubble") &&
      !STYLES.includes("body.has-hits .avatar.reel-ready .portrait") &&
      !STYLES.includes("body.has-url-policy .avatar.reel-ready .portrait") &&
      !STYLES.includes("body.has-hits .persona-reel") &&
      !STYLES.includes("body.has-url-policy .persona-reel"),
  );
  check(
    "回答氣泡出現時根頁不會捲動到把上下控制列推出視窗",
    /html,\s*\nbody\s*\{[^}]*overflow:\s*hidden;[^}]*overflow-anchor:\s*none;/su.test(STYLES),
  );
}

check(
  "截圖工具會導航、等頁面與角色圖 ready，再核對 exact PNG 像素",
  SHOT.includes('await send("Page.navigate", { url })') &&
    SHOT.includes(
      'await send("Page.addScriptToEvaluateOnNewDocument", { source: initialPageScript })',
    ) &&
    SHOT.includes('document.readyState === "complete"') &&
    SHOT.includes('cards.length === 17 && pictures.length === 17') &&
    SHOT.includes("previewPictures.length === 1") &&
    SHOT.includes("reelImages.length >= 21 && reelImages.length <= 26") &&
    SHOT.includes('msg.method === "Runtime.exceptionThrown"') &&
    SHOT.includes("unlinkSync(out)") &&
    SHOT.includes('"[data-days]"') &&
    SHOT.includes('"[data-cards]"') &&
    SHOT.includes('"[data-shot]"') &&
    SHOT.includes('"[data-corpus]"') &&
    SHOT.includes('png.readUInt32BE(16) !== viewportWidth') &&
    SHOT.includes('png.readUInt32BE(20) !== viewportHeight'),
);
check(
  "角色截圖涵蓋 idle／thinking／paused／asleep，而且每張等 exact state",
  SHOOT_AVATAR.includes("for state in idle thinking paused asleep; do") &&
    SHOOT_AVATAR.includes('--expect-js "document.querySelector(\'[data-avatar]\')?.dataset.state === \'$state\'"') &&
    SHOOT_AVATAR.includes('(\"127.0.0.1\", 0)') &&
    SHOOT_AVATAR.includes('kill -0 "$server"') &&
    ["idle", "thinking", "paused", "asleep"].every((state) =>
      SHOOT_AVATAR.includes(`"$OUT_DIR/${state}.png"`),
    ) &&
    !SHOOT_AVATAR.includes("PORT=8731"),
);

console.log("④ 關掉 Persona 只拿掉角色，不碰核心 UI");
{
  const p = await open(persona("grok", { enabled: false }));
  check("avatar 真的 hidden", p.node("[data-avatar]").hidden);
  check("avatar 不再是 focus target", p.node("[data-avatar]").disabled);
  check("搜尋框仍在", p.node("[data-ask-input]") !== null);
  check("錄製狀態仍可見", p.node("[data-state-line]").textContent !== "", p.node("[data-state-line]").textContent);
  await p.forceAvatarHandler();
  check("就算硬叫 handler 也不說話", p.node("[data-persona-line]").textContent === "");
}

console.log("⑤ 可以只關 tap-lines 或動畫，並在設定存好後即時換人");
{
  const p = await open(persona("deepseek", { tap_lines: false, motion: false }));
  check("角色仍看得到", !p.node("[data-avatar]").hidden);
  check("沒有功能的 button 不留 focus target", p.node("[data-avatar]").disabled);
  check("動畫 class 沒掛上", !globalThis.document.documentElement.classList.contains("motion"));
  await p.forceAvatarHandler();
  check("tap-lines 關掉就沒有台詞", p.node("[data-persona-line]").textContent === "");

  await p.fromOutside("persona-changed", persona("claude"));
  check("事件換成 Claude", p.node("[data-avatar]").dataset.persona === "claude");
  check("portrait 同步換成 Claude", p.node("[data-persona-portrait]").src === "./personas/claude.webp");
  check("重新啟用 keyboard/click target", !p.node("[data-avatar]").disabled);
}

console.log("⑥ 17 人固定語音與日常短句都走 bundled Ogg，不借系統聲音");
{
  check(
    "manifest 是每人基本 8＋擴充 24，共 544 段",
    VOICES.totals?.personas === 17 &&
      VOICES.totals?.baseLinesPerPersona === 8 &&
      VOICES.totals?.extensionLinesPerPersona === 24 &&
      VOICES.clips?.length === 544,
    VOICES.totals,
  );
  const installed = persona("chatgpt", { voice_enabled: true });
  const p = await open(installed, {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
  });
  await p.poll();
  check("開場／poll 不自行播放", p.plays() === 0, p.plays());
  scriptRandom(0);
  await p.clickAvatar();
  stopScriptingRandom();
  check(
    "trusted click 只播 manifest 裡的 exact bundled Ogg",
    JSON.stringify(p.playedSources()) ===
      JSON.stringify(["./persona-voices/v1/base/chatgpt/tap-general.ogg"]),
    p.playedSources(),
  );
  check(
    "固定語音先取得 native admission/begin，播放中 lease 尚未 end",
    p.calls.includes("persona_fixed_voice_admit") &&
      p.calls.includes("master_stop_presentation_begin") &&
      !p.calls.includes("master_stop_presentation_end"),
    p.calls,
  );
  check("不使用 localService fallback", p.speaks.length === 0, p.speaks);
  check("真正開始播放才進 speaking", p.node("[data-avatar]").classList.contains("speaking"));
  p.finishAudio();
  check(
    "ended 清掉 speaking 並歸還 lease",
    !p.node("[data-avatar]").classList.contains("speaking") &&
      p.calls.includes("master_stop_presentation_end"),
    p.calls,
  );

  for (const [id, expected] of Object.entries(EXPECTED)) {
    const view = await open(persona(id, { voice_enabled: true }));
    scriptRandom(0);
    await view.clickAvatar();
    stopScriptingRandom();
    check(`${id} 有自己的 bundled tap voice`, view.plays() === 1, view.playedSources());
    check(`${id} 第一段文字逐字一致`, view.node("[data-persona-line]").textContent === expected.first);
  }

  // 閒話那包也要真的播得出來：隨機挑到的如果是閒話那一段，一樣要走 bundled Ogg，
  // 不可以掉回系統語音，也不可以安靜地什麼都不播。
  for (const [id] of Object.entries(EXPECTED)) {
    const view = await open(persona(id, { voice_enabled: true }), {
      systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
    });
    scriptRandomSequence(20260912);
    const sources = new Set();
    for (let i = 0; i < 12; i += 1) {
      await view.clickAvatar();
      sources.add(view.playedSources().at(-1));
      view.finishAudio();
    }
    stopScriptingRandom();
    const banterPlayed = [...sources].filter((src) =>
      src?.startsWith(`./persona-banter-voices/v1/banter/${id}/`),
    );
    check(`${id} 閒話那包也走 bundled Ogg`, banterPlayed.length >= 4, [...sources]);
    check(`${id} 沒有借系統語音`, view.speaks.length === 0, view.speaks);
  }

  const off = await open(persona("mimo", { voice_enabled: false }), {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
  });
  await off.clickAvatar();
  check("voice 關閉仍顯示台詞但完全不取播放權", off.plays() === 0 && !off.calls.includes("persona_fixed_voice_admit"));
  check("voice 關閉不借系統聲音", off.speaks.length === 0, off.speaks);

  // 這兩條守的是「她不可以拿一句罐頭台詞頂替大腦的答案」。alpha.132 之後問完
  // 一題會多一聲墊話（`playAnswerBeat`），所以「什麼都不准播」這個寫法會誤傷
  // 它——但**不能因此就放寬成「隨便播什麼都行」**。收緊成兩句話：日常那 544 段
  // 和同意書那 68 段一段都不准出現，而唯一准播的那一聲必須真的是 answer-beat。
  const answerBeatFiles = new Set(
    BANTER.clips
      .filter((clip) => clip.use === "answer-beat")
      .map((clip) => `./persona-banter-voices/v1/${clip.file}`),
  );
  const onlyAnswerBeat = (sources) =>
    sources.every(
      (src) =>
        !src.startsWith("./persona-voices/v1/") &&
        !src.startsWith("./persona-consent-voices/v1/") &&
        answerBeatFiles.has(src),
    );

  const daily = await open(persona("kimi", { voice_enabled: true }));
  await daily.ask("早安。");
  check("精確日常短句也交給 CLI／記憶路徑", daily.calls.includes("ask"), daily.calls);
  check("文字問題不再用固定角色台詞繞過大腦", !daily.calls.includes("answer_cli_cancel"), daily.calls);
  check(
    "文字問題不播固定 Ogg 冒充大腦答案（那一聲只能是 answer-beat）",
    onlyAnswerBeat(daily.playedSources()),
    daily.playedSources(),
  );
  check("答案落地那一聲真的響了", daily.plays() === 1, daily.playedSources());

  const normal = await open(persona("kimi", { voice_enabled: true }));
  await normal.ask("早安，昨天我在做什麼");
  check("不是 exact trigger 的問題仍完整交給 CLI／記憶路徑", normal.calls.includes("ask"), normal.calls);
  check(
    "一般記憶問題不播固定語音冒充動態答案（那一聲只能是 answer-beat）",
    onlyAnswerBeat(normal.playedSources()),
    normal.playedSources(),
  );

  /*
   * 兩包語音各自 fail closed，而且**互相不背書**。
   *
   * 以前這四條寫成「戳下去什麼都不准播」，因為那時候戳她只播得到日常那一包。
   * 現在戳她的台詞是兩包合起來的，所以那個寫法會把「日常壞掉、閒話還好好的」
   * 誤判成迴歸。改成指名道姓：壞掉那一包的檔案一個都不准出現。
   *
   * 但只改一半就是把閘門放鬆。每一種壞法都要問兩次——壞日常的時候閒話還在，
   * 壞閒話的時候日常還在——不然「其中一包的完整性檢查其實沒接上」看起來會和
   * 「兩包都好」一模一樣。
   */
  const dialoguePlayed = (view) =>
    view.playedSources().filter((src) => src.startsWith("./persona-voices/v1/"));
  const banterPlayed = (view) =>
    view.playedSources().filter((src) => src.startsWith("./persona-banter-voices/v1/"));

  /*
   * 骰子釘在**最後**一格，不是第一格。這一格是踩過坑才知道的：`taps` 是
   * 「日常兩句 + 閒話十句」接起來的，第一格永遠是日常那句，所以拿第一格去問
   * 「閒話那包停用了沒有」，答案不管閘門在不在都是「沒播閒話」——那兩條斷言
   * 會**恆真**。兩刀突變（拿掉閒話 manifest 的權利審查、拿掉「每個人都要有
   * 整包」）當場示範了這件事：兩刀都活下來，而整支閘門是綠的。
   *
   * 釘在最後一格之後，同一顆骰子在四種狀態下指向不同的包：
   *   兩包都好      → 閒話（12 句裡的第 12 句）
   *   日常壞掉      → 閒話（只剩 10 句）
   *   閒話壞掉      → 日常（只剩 2 句，第 2 句）
   * 所以底下那條 control 不是裝飾，它證明這顆骰子真的走得到閒話那一包。
   */
  const pokeWithBrokenPack = async (name, override) => {
    const view = await open(persona("chatgpt", { voice_enabled: true }), {
      ...override,
      systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
    });
    scriptRandom(0.999);
    await view.clickAvatar();
    stopScriptingRandom();
    return { name, view };
  };

  const bothPacksFine = await pokeWithBrokenPack("兩包都好", {});
  check(
    "前提：這顆骰子在兩包都好的時候真的挑到閒話那一包",
    banterPlayed(bothPacksFine.view).length === 1 &&
      dialoguePlayed(bothPacksFine.view).length === 0,
    bothPacksFine.view.playedSources(),
  );

  const dialogueBreakage = [
    ["不完整 manifest", { voiceManifest: { ...VOICES, clips: VOICES.clips.slice(0, 543) } }],
    ["未核准權利投影", { voiceManifest: { ...VOICES, rightsReview: "pending" } }],
    ["總 bytes 漂移", { voiceManifest: { ...VOICES, totals: { ...VOICES.totals, oggBytes: 8918727 } } }],
    [
      "角色分組漂移",
      {
        voiceManifest: (() => {
          const wrong = JSON.parse(JSON.stringify(VOICES));
          wrong.clips[0].group = "bestie";
          return wrong;
        })(),
      },
    ],
  ];
  for (const [label, override] of dialogueBreakage) {
    const { view } = await pokeWithBrokenPack(label, override);
    check(
      `${label}讓 544 段整份停用，一段都不播`,
      dialoguePlayed(view).length === 0 && view.speaks.length === 0,
      { played: view.playedSources(), speaks: view.speaks },
    );
    check(
      `${label}不會連閒話那包一起判死`,
      banterPlayed(view).length === 1,
      view.playedSources(),
    );
  }

  const banterBreakage = [
    ["不完整 manifest", { banterManifest: { ...BANTER, clips: BANTER.clips.slice(0, 339) } }],
    ["未核准權利投影", { banterManifest: { ...BANTER, rightsReview: "pending" } }],
    ["總 bytes 漂移", { banterManifest: { ...BANTER, totals: { ...BANTER.totals, oggBytes: 1 } } }],
    [
      "角色分組漂移",
      {
        banterManifest: (() => {
          const wrong = JSON.parse(JSON.stringify(BANTER));
          wrong.clips[0].group = "bestie";
          return wrong;
        })(),
      },
    ],
    [
      // 這一份要**只**踩到「每個人都要有整包」那一格。第一版沒做到：它把搬走的
      // 那一句換成一句 lineId 對不上檔名的假貨，於是先被路徑檢查擋掉，而突變
      // 把完整性檢查整個拿掉之後閘門照樣是綠的。所以這裡連 bytes、durationMs
      // 和檔名全部湊成合法的——mimo 少一句、chatgpt 多一句，其餘一格不動。
      "少一個人整包不要",
      {
        banterManifest: (() => {
          const wrong = JSON.parse(JSON.stringify(BANTER));
          const victim = wrong.clips.findIndex((clip) => clip.persona === "mimo");
          const [moved] = wrong.clips.splice(victim, 1);
          wrong.clips.push({
            ...moved,
            persona: "chatgpt",
            group: "sister",
            lineId: "poke-extra",
            file: "banter/chatgpt/poke-extra.ogg",
          });
          return wrong;
        })(),
      },
    ],
    [
      // 兄弟條款：人數對、每個人句數也對，但有人整整少掉一種用途。那個人會
      // 「被戳有反應、答案落地卻沒有那一聲」，比十七個人都沒有難解釋得多。
      "有人少一整種用途",
      {
        banterManifest: (() => {
          const wrong = JSON.parse(JSON.stringify(BANTER));
          for (const clip of wrong.clips) {
            if (clip.persona === "mimo" && clip.use === "answer-beat") {
              clip.use = "avatar-poke";
            }
          }
          return wrong;
        })(),
      },
    ],
  ];
  for (const [label, override] of banterBreakage) {
    const { view } = await pokeWithBrokenPack(`閒話 ${label}`, override);
    check(
      `閒話那包${label}讓 340 段整份停用，一段都不播`,
      banterPlayed(view).length === 0 && view.speaks.length === 0,
      { played: view.playedSources(), speaks: view.speaks },
    );
    check(
      `閒話那包${label}不會連日常那 544 段一起判死`,
      dialoguePlayed(view).length === 1,
      view.playedSources(),
    );
  }

  const blocked = await open(persona("chatgpt", { voice_enabled: true }), {
    fixedSpeechAdmission: { presentation_id: "fixed-blocked" },
    masterStopPresentationBegin: ({ presentationId }) => presentationId !== "fixed-blocked",
  });
  await blocked.clickAvatar();
  check(
    "native begin=false 時不播放且收回 lease",
    blocked.plays() === 0 && blocked.calls.includes("master_stop_presentation_end"),
    { plays: blocked.plays(), calls: blocked.calls },
  );

  const answerAskResult = {
      hits: [
        {
          chunk_id: 31,
          ts: 1_755_000_000_000,
          text: "客服專線 0800-080-123",
          snippet: "客服[專線] 0800-080-123",
          app: "chrome.exe",
          title: "帳單查詢",
          url: "https://example.com/bill",
          frame_id: null,
        },
      ],
      kind: "keywords",
      query_id: 7,
      answers: [],
      blind: null,
      truncated: false,
      answers_truncated: false,
      searched: null,
      time_range: null,
      chapters: null,
      followup: null,
      closure_notice: null,
      overview: null,
      synthesis: null,
  };
  let localMasterStopState = "clear";
  const answerLocal = await open(persona("mimo", { voice_enabled: true }), {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
    askResult: answerAskResult,
    masterStopState: () => localMasterStopState,
  });
  await answerLocal.ask("電話");
  await answerLocal.clickAnswerRead();
  const spokenAnswer = answerLocal.speaks[0]?.text ?? "";
  // 只數**答案那一份**租約被還回去幾次。以前這裡數的是所有 `…_end`，而
  // alpha.132 之後每問一題會先響一聲墊話（`playAnswerBeat`），它自己借還一次，
  // 於是總數整個偏掉。改成看 presentation id——這比舊的寫法更嚴：舊的分不出
  // 「還回來的是哪一份租約」，還錯了它也數得到一。
  const localLeaseEnds = () =>
    answerLocal.mediaTrace().filter((event) => event === "end:local-answer").length;
  check("本機答案朗讀收到畫面答案正文", spokenAnswer.includes("客服專線 0800-080-123"), spokenAnswer);
  check(
    "本機答案朗讀先取得 native admission/begin，播放中 lease 尚未 end",
    answerLocal.calls.includes("answer_local_speech_admit") &&
      answerLocal.calls.includes("master_stop_presentation_begin") &&
      localLeaseEnds() === 0,
    answerLocal.calls,
  );
  check(
    "答案朗讀剔除來源與操作列",
    !spokenAnswer.includes("chrome.exe") &&
      !spokenAnswer.includes("帳單查詢") &&
      !spokenAnswer.includes("沒有留下畫面") &&
      !spokenAnswer.includes("我本來已經忘了"),
    spokenAnswer,
  );

  answerLocal.speaks[0]?.onstart?.();
  check(
    "前提：答案 localService 已開始 speaking",
    answerLocal.node("[data-avatar]").classList.contains("speaking"),
  );
  answerLocal.speaks[0]?.onerror?.({ error: "voice-unavailable" });
  check(
    "本機 TTS async 失敗給出直接重播出口",
    answerLocal.node("[data-persona-line]").textContent ===
      "本機朗讀失敗。再按一次重播。",
    answerLocal.node("[data-persona-line]").textContent,
  );
  check(
    "localService error 也清掉 speaking",
    !answerLocal.node("[data-avatar]").classList.contains("speaking") && localLeaseEnds() === 1,
    { calls: answerLocal.calls, ends: localLeaseEnds() },
  );

  await answerLocal.ask("再念一次");
  await answerLocal.clickAnswerRead();
  check("第二次播放中仍沒提早 end 新 lease", localLeaseEnds() === 1, answerLocal.calls);
  answerLocal.speaks[1]?.onend?.();
  check("本機答案最後一段 ended 才 end lease", localLeaseEnds() === 2, answerLocal.calls);

  await answerLocal.ask("播放中全停");
  await answerLocal.clickAnswerRead();
  answerLocal.speaks[2]?.onstart?.();
  const cancelsBeforeStop = answerLocal.cancels();
  const traceBeforeStop = answerLocal.mediaTrace().length;
  localMasterStopState = "stopping";
  await answerLocal.poll();
  const stopTrace = answerLocal.mediaTrace().slice(traceBeforeStop);
  const cancelAt = stopTrace.indexOf("cancel-local");
  const endAt = stopTrace.indexOf("end:local-answer");
  check(
    "外部 CLI 的 poll 觀察到 Stopping，先 cancel 本機答案 TTS 再 end lease",
    answerLocal.cancels() > cancelsBeforeStop &&
      localLeaseEnds() === 3 &&
      !answerLocal.node("[data-avatar]").classList.contains("speaking") &&
      cancelAt >= 0 &&
      endAt > cancelAt,
    {
      calls: answerLocal.calls,
      cancels: answerLocal.cancels(),
      ends: localLeaseEnds(),
      stopTrace,
    },
  );

  const groundedLocal = await open(persona("mimo", { voice_enabled: true }), {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
    askResult: {
      ...answerAskResult,
      hits: [
        {
          ...answerAskResult.hits[0],
          snippet: "RAW_OCR_MUST_NOT_BE_SPOKEN",
        },
      ],
      synthesis: {
        sentences: [
          {
            text: "只念這一句整理後的答案。",
            sources: [{ ref: "chunk:31", label: "文字 #31", frame_id: null }],
          },
        ],
      },
    },
  });
  await groundedLocal.ask("整理後再念");
  await groundedLocal.clickAnswerRead();
  check(
    "本機朗讀有 RAG 成句時只念成句正文",
    groundedLocal.speaks[0]?.text === "只念這一句整理後的答案。" &&
      !groundedLocal.speaks[0]?.text.includes("RAW_OCR_MUST_NOT_BE_SPOKEN"),
    groundedLocal.speaks[0]?.text,
  );

  const boundaryBlocked = await open(persona("mimo", { voice_enabled: true }), {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
    askResult: answerAskResult,
    localSpeechAdmission: { presentation_id: "local-blocked" },
    masterStopPresentationBegin: ({ presentationId }) => presentationId !== "local-blocked",
  });
  await boundaryBlocked.ask("邊界拒絕");
  await boundaryBlocked.clickAnswerRead();
  check(
    "本機答案 begin=false 時不 speak 且收回 lease",
    boundaryBlocked.speaks.length === 0 &&
      boundaryBlocked.calls.includes("master_stop_presentation_end"),
    { speaks: boundaryBlocked.speaks, calls: boundaryBlocked.calls },
  );
}

console.log("⑥ᵈ 沒事的時候她會自己笑一下；答案落地也有一聲。全停與每一個開關照樣壓得住");
{
  // 這一段守的是產品裡**唯一**一條「沒有人碰她、她自己開口」的路。
  //
  // `docs/PRODUCT.md` 原本寫的是「聲音不因 idle、capture、記憶或系統事件自己
  // 播放」，這一版把 idle 那一項換掉了——他要的就是這個（「沒事也可以 呵呵
  // 嘻嘻 笑幾下 比較有互動感」）。換掉一條寫在文件裡的界線，代價是這裡要把
  // 剩下每一道閘門逐條釘住：全停、暫停、關角色、關台詞、視窗看不見、他正在
  // 打字。少釘一條，那條界線就是真的鬆了，而不是被搬過。
  const GIGGLES = BANTER.clips.filter((clip) => clip.use === "idle-giggle");
  const BEATS = BANTER.clips.filter((clip) => clip.use === "answer-beat");
  check(
    "閒話那包 17 人各 20 句、三種用途齊全",
    BANTER.totals?.personas === 17 &&
      BANTER.totals?.linesPerPersona === 20 &&
      BANTER.clips.length === 340 &&
      GIGGLES.length === 85 &&
      BEATS.length === 85,
    BANTER.totals,
  );

  const quiet = await open(persona("chatgpt", { voice_enabled: true }));
  await quiet.poll();
  await quiet.poll();
  check(
    "開場與輪詢都不會讓她自己開口",
    quiet.node("[data-persona-line]").textContent === "" && quiet.plays() === 0,
    { line: quiet.node("[data-persona-line]").textContent, plays: quiet.plays() },
  );

  // 固定週期的東西兩次之後就變成節拍器。間隔要是隨機的，而且落在兩到五分鐘。
  scriptRandom(0);
  const soonest = await open(persona("chatgpt"));
  scriptRandom(0.999);
  const latest = await open(persona("chatgpt"));
  stopScriptingRandom();
  const gapOf = (view) => view.slowTimers.find((timer) => timer.ms >= 60_000)?.ms ?? null;
  check(
    "自己笑的間隔是兩到五分鐘之間的隨機值，不是節拍器",
    gapOf(soonest) === 120_000 && gapOf(latest) > 290_000 && gapOf(latest) <= 300_000,
    { soonest: gapOf(soonest), latest: gapOf(latest) },
  );

  const giggling = await open(persona("chatgpt", { voice_enabled: true }), {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
  });
  const fired = await giggling.fireSlowTimers((timer) => timer.ms >= 60_000);
  const giggled = giggling.node("[data-persona-line]").textContent;
  check("時間到了她真的自己笑一句", fired === 1 && GIGGLES.some((clip) => clip.text === giggled), {
    fired,
    giggled,
  });
  check(
    "自己笑那一句走 bundled Ogg，不借系統語音",
    giggling.playedSources().length === 1 &&
      giggling.playedSources()[0].startsWith("./persona-banter-voices/v1/banter/chatgpt/giggle-") &&
      giggling.speaks.length === 0,
    { played: giggling.playedSources(), speaks: giggling.speaks },
  );
  check(
    "笑一下也要先跟 native 拿播放權",
    giggling.calls.includes("persona_fixed_voice_admit") &&
      giggling.calls.includes("master_stop_presentation_begin"),
    giggling.calls,
  );
  check(
    "笑完會排下一次，不會只笑這一次",
    giggling.slowTimers.filter((timer) => timer.ms >= 60_000).length === 2,
    giggling.slowTimers.map((timer) => timer.ms),
  );
  giggling.finishAudio();
  await giggling.fireSlowTimers((timer) => timer.ms < 60_000);
  check(
    "那一句六秒後自己收掉，畫面不會留著一個「嘻嘻」",
    giggling.node("[data-persona-line]").textContent === "" &&
      giggling.node("[data-persona-line]").hidden,
    giggling.node("[data-persona-line]").textContent,
  );

  // 每一道閘門各關一次。這裡刻意一條一條開夾具而不是共用一個，因為「哪一條
  // 擋住的」才是這段要證的事——合成一個夾具的話，其中一條失效看不出來。
  const gated = [
    ["全停中", persona("chatgpt", { voice_enabled: true }), { masterStopState: "stopping" }],
    ["暫停中", persona("chatgpt", { voice_enabled: true }), { pauseState: true }],
    ["關掉角色", persona("chatgpt", { enabled: false, voice_enabled: true }), {}],
    ["關掉台詞", persona("chatgpt", { tap_lines: false, voice_enabled: true }), {}],
    ["視窗看不見", persona("chatgpt", { voice_enabled: true }), { visibilityState: "hidden" }],
  ];
  for (const [name, view, options] of gated) {
    const shut = await open(view, options);
    await shut.poll();
    const before = shut.slowTimers.length;
    await shut.fireSlowTimers((timer) => timer.ms >= 60_000);
    check(
      `${name}的時候她不會自己開口`,
      shut.node("[data-persona-line]").textContent === "" && shut.plays() === 0,
      { line: shut.node("[data-persona-line]").textContent, plays: shut.plays() },
    );
    check(
      `${name}只是跳過這一輪，之後仍然排得回來`,
      shut.slowTimers.length > before,
      { before, after: shut.slowTimers.length },
    );
  }

  // 他正在打字的時候插一句「嘻嘻」不是陪伴，是打斷。
  const typing = await open(persona("chatgpt", { voice_enabled: true }));
  globalThis.document.activeElement = typing.node("[data-ask-input]");
  await typing.fireSlowTimers((timer) => timer.ms >= 60_000);
  check(
    "他正在打字的時候不插嘴",
    typing.node("[data-persona-line]").textContent === "" && typing.plays() === 0,
    typing.node("[data-persona-line]").textContent,
  );

  // 答案落地那一聲。
  const beating = await open(persona("chatgpt", { voice_enabled: true }));
  await beating.ask("剛剛在幹嘛");
  const beatSources = beating
    .playedSources()
    .filter((src) => src.startsWith("./persona-banter-voices/v1/banter/chatgpt/beat-"));
  check("答案落地會有一聲「找到了」", beatSources.length === 1, beating.playedSources());
  check(
    "那一聲是 answer-beat 那一類，不是隨便挑一句閒話",
    BEATS.some((clip) => `./persona-banter-voices/v1/banter/${clip.persona}/${clip.lineId}.ogg` === beatSources[0]),
    beatSources,
  );

  const azureReady = await open(persona("chatgpt", { voice_enabled: true }), {
    azureStatus: {
      generation: 1,
      config_readable: true,
      enabled: true,
      region: "eastasia",
      voice: "zh-TW-HsiaoChenNeural",
      endpoint: "https://eastasia.tts.speech.microsoft.com/cognitiveservices/v1",
      credential: "present",
      consented: true,
      consent_at: 1_755_000_000_000,
      config_error: null,
      ready: true,
    },
  });
  await azureReady.ask("剛剛在幹嘛");
  check(
    "Azure 朗讀開著就把這一聲讓出去，不疊兩個聲音",
    azureReady
      .playedSources()
      .every((src) => !src.startsWith("./persona-banter-voices/v1/banter/")),
    azureReady.playedSources(),
  );

  const muted = await open(persona("chatgpt", { voice_enabled: false }));
  await muted.ask("剛剛在幹嘛");
  await muted.fireSlowTimers((timer) => timer.ms >= 60_000);
  check("關掉語音就完全不取播放權", muted.plays() === 0 && !muted.calls.includes("persona_fixed_voice_admit"), {
    plays: muted.plays(),
    calls: muted.calls,
  });
}

console.log("⑦ 較舊的開場讀取不會蓋掉較新的設定事件");
{
  const p = await open(persona("chatgpt"), { deferPersonaRead: true });
  await p.fromOutside("persona-changed", persona("grok"));
  check("新事件先換成 Grok", p.node("[data-avatar]").dataset.persona === "grok");
  await p.resolvePersonaRead();
  check("舊 initial read 回來仍是 Grok", p.node("[data-avatar]").dataset.persona === "grok");
}

console.log("⑦ᵇ Persona media-stop 會停 bundled voice，但內建角色圖不撤掉");
{
  const p = await open(persona("chatgpt", { voice_enabled: true }));
  await p.clickAvatar();
  check("event 前 bundled voice 正在播", p.plays() === 1 && p.node("[data-avatar]").classList.contains("speaking"));
  await p.fromOutside("persona-media-stop");
  check("固定台詞立刻清掉", p.node("[data-persona-line]").hidden && p.node("[data-persona-line]").textContent === "");
  check(
    "內建角色圖不受 remove 影響",
    !p.node("[data-persona-portrait]").hidden &&
      p.node("[data-persona-portrait]").src === "./personas/chatgpt.webp",
    p.node("[data-persona-portrait]").src,
  );
  check("asset 狀態先 fail closed 成 unavailable", p.node("[data-avatar]").dataset.assetPack === "unavailable", p.node("[data-avatar]").dataset.assetPack);
  check(
    "media-stop 清掉 speaking 並歸還 bundled lease",
    !p.node("[data-avatar]").classList.contains("speaking") &&
      p.calls.includes("master_stop_presentation_end"),
    p.calls,
  );
  check("這條事件不碰 persona 選擇", p.node("[data-avatar]").dataset.persona === "chatgpt", p.node("[data-avatar]").dataset.persona);
}

console.log("⑦ᶜ 未知 Persona event 整份拒絕，不能替舊角色打開聲音");
{
  const p = await open(persona("chatgpt"));
  await p.fromOutside("persona-changed", {
    ...persona("not-in-this-version", { voice_enabled: true, tap_lines: false }),
    asset_pack: {
      phase: "installed",
      release_id: "forged",
      voice_lines: [
        { line_id: "chatgpt-greeting", duration_ms: 1, spoken_text: "我在，隨時可以開始。" },
      ],
    },
  });
  check("未知 event 沒把已知角色換掉", p.node("[data-avatar]").dataset.persona === "chatgpt");
  await p.clickAvatar();
  check("未知 event 的 voice=true 沒有播放權", p.plays() === 0 && p.speaks.length === 0, {
    fixed: p.plays(),
    local: p.speaks.length,
  });
}

console.log("⑧ Persona 點擊台詞獨立；所有文字問題都走大腦與記憶路徑");
{
  const askBody = SRC.match(/async function ask\([^)]*\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  const gateBody = SRC.match(/function renderGatekeeper\(view\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  check(
    "ask 沒有日常短句旁路，每個文字問題都 invoke native ask",
    !askBody.includes("dailyDialogueReply") &&
      askBody.includes('const answer = await invoke("ask", { question })'),
    askBody,
  );
  check("Gatekeeper 不會觸發 tap-line", !/sayPersonaLine|personaLine|personaAudio/u.test(gateBody), gateBody);
  check("後端有獨立 persona_read command", MAIN.includes("fn persona_read(") && MAIN.includes("persona_read,"));
  check(
    "bundled fixed voice 有獨立 native admission 且只鑄 presentation id",
    MAIN.includes("fn persona_fixed_voice_admit(") &&
      MAIN.includes('admit_desktop_brain(shell.data_dir.as_deref(), "這次 Persona 本機固定語音")') &&
      MAIN.includes("persona_fixed_voice_admit,"),
  );
  const bundledPlayback = SRC.match(/async function playBundledPersonaLine\(line\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  check(
    "bundled 日常語音不依賴舊素材包或下載狀態",
    bundledPlayback.includes("line.file") &&
      !bundledPlayback.includes("localAssets") &&
      !bundledPlayback.includes("asset_pack") &&
      !bundledPlayback.includes("persona_asset"),
    bundledPlayback,
  );
  const enumBody = CONFIG.match(/pub enum PersonaId \{([\s\S]*?)\n\}/u)?.[1] ?? "";
  const enumIds = [...enumBody.matchAll(/^\s{4}([A-Z][A-Za-z]+),$/gmu)].map((match) => match[1]);
  const rustIds = enumIds.map((id) => id.toLowerCase()).sort();
  check(
    "typed config 是同一組 exact 17 IDs且沒有 Neutral",
    !enumIds.includes("Neutral") && JSON.stringify(rustIds) === JSON.stringify(expectedIds),
    rustIds,
  );
  check("voice gate 預設 false", /voice_enabled:\s*false/u.test(CONFIG));
  const answerRead = SRC.match(/function answerReadLine\(\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  check(
    "答案朗讀是 trusted click、本機系統語音，且先取得 master-stop presentation admission",
    answerRead.includes('button.textContent = "🔊 用本機聲音朗讀"') &&
      answerRead.includes("event?.isTrusted !== true") &&
      answerRead.includes('invoke("answer_local_speech_admit")') &&
      answerRead.includes("beginNativePresentation(presentation)") &&
      answerRead.includes("speakWithLocalSystemVoice(text, presentation)"),
    answerRead,
  );
  check(
    "native local-answer admission command 有註冊且只鑄 presentation id",
    MAIN.includes("fn answer_local_speech_admit(") &&
      MAIN.includes('admit_desktop_brain(shell.data_dir.as_deref(), "這次本機答案朗讀")') &&
      MAIN.includes("answer_local_speech_admit,") &&
      MAIN.includes("struct MasterStopPresentationView"),
  );
  const localSpeech = SRC.match(/function speakWithLocalSystemVoice\(text, presentation = null\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  const mediaStop =
    SRC.match(
      /^function stopPersonaMedia\(\{ cancelAzureNative = true \} = \{\}\) \{[\s\S]*?^\}$/mu,
    )?.[0] ?? "";
  check(
    "長答案按段依序播且整串有 revision cancel gate",
    SRC.includes("function chunkLocalSpeech(text, limit = 160)") &&
      localSpeech.includes("revision !== localSpeechRevision") &&
      localSpeech.includes("utterance.onend = speakNext") &&
      localSpeech.includes("releaseNativePresentation(presentation)"),
    localSpeech,
  );
  check(
    "bundled Ogg 與答案 TTS 共用一顆完整 stop",
    mediaStop.includes("voiceRequest += 1") &&
      mediaStop.includes("personaAudio?.pause?.()") &&
      mediaStop.includes("bundledVoicePresentation") &&
      mediaStop.includes("stopLocalSpeech()") &&
      localSpeech.includes("stopPersonaMedia()") &&
      /async function ask\([^)]*\)[\s\S]*?stopPersonaMedia\(\)/u.test(SRC),
    mediaStop,
  );
}

console.log("⑨ 診斷觀測送得出數字，送不出螢幕上的字");
{
  /* 這一節守的是上面那個 `invoke` stub 裡的過濾。
   *
   * 那幾行把 `diagnose_note` 從 `calls` 移到 `diagnoseNotes`，理由是「觀測不是
   * 產品指令」。但「濾掉」和「刪掉偵測器」在通過的測試上長得一模一樣，所以這
   * 一節要證兩件事：那條路還活著（正向），而且它送出去的東西真的只有數字。 */
  const p = await open(persona("chatgpt"));
  const beforePoke = p.diagnoseNotes.length;
  await p.forceAvatarHandler();
  const poked = p.diagnoseNotes.slice(beforePoke);
  check(
    "戳一下真的留下一則觀測——濾掉不等於沒人看",
    poked.length === 1 && poked[0]?.kind === "poke",
    JSON.stringify(poked),
  );
  const said = p.node("[data-persona-line]").textContent;
  check("前提：她真的講了一句話", said !== "", said);
  check(
    "那一則觀測裡沒有她剛剛講的那句話",
    said !== "" && !JSON.stringify(poked).includes(said),
    JSON.stringify(poked),
  );

  /* 語音開著走的是另一條臂（等 `playBundledPersonaLine` 回話才記），而那條臂
   * 和上面那條吐出來的 note 幾乎一模一樣——只差一個 `voiced`。兩臂長得像的時
   * 候，先假設分辨它們的那個條件沒人守，所以兩邊各跑一次。 */
  const loud = await open(persona("kimi", { voice_enabled: true }));
  const beforeLoudPoke = loud.diagnoseNotes.length;
  await loud.forceAvatarHandler();
  const loudPokes = loud.diagnoseNotes.slice(beforeLoudPoke).filter((note) => note?.kind === "poke");
  check("語音開著，戳一下一樣留得下觀測", loudPokes.length === 1, JSON.stringify(loudPokes));
  check(
    "語音開著時那一則說得出「真的出聲了」",
    loudPokes[0]?.voiced === true,
    JSON.stringify(loudPokes),
  );
  check(
    "語音關著時那一則說的是「沒出聲」——兩臂分得開",
    poked[0]?.voiced === false,
    JSON.stringify(poked),
  );

  const beforeBar = p.diagnoseNotes.length;
  await p.clickChromeToggle();
  check(
    "上面那一槓開合也留得下觀測",
    p.diagnoseNotes.slice(beforeBar).some((note) => note?.kind === "bar"),
    JSON.stringify(p.diagnoseNotes.slice(beforeBar)),
  );

  await p.ask("我的銀行密碼是 hunter2，昨天在幹嘛？");
  check(
    "他打的問題只留下字數，不留原文",
    !p.diagnoseNotes.some((note) => JSON.stringify(note).includes("hunter2")),
    JSON.stringify(p.diagnoseNotes.filter((note) => note?.kind === "answered")),
  );

  /* 機械掃一遍：除了 `answered`（她自己的答案，報告把那一節放在那條線的下面，
   * 他可以整段刪掉），沒有一則觀測帶得動自由文字。
   *
   * 這一條是給**還沒寫出來的** note 種類看的：以後誰在 `Note` 上加一個字串欄
   * 位，這裡當場紅，不必等到有人讀報告才發現螢幕上的字被送出去了。`clip` 是
   * 語音檔的 lineId，所以另外釘它的形狀，免得有人拿它夾帶。 */
  const CARRIES_HER_ANSWER = "answered";
  const smuggled = [];
  for (const note of [...p.diagnoseNotes, ...loud.diagnoseNotes]) {
    if (note?.kind === CARRIES_HER_ANSWER) continue;
    for (const [key, value] of Object.entries(note ?? {})) {
      if (typeof value !== "string") continue;
      if (key === "kind") continue;
      if (key === "clip" && /^[a-z0-9_-]+$/u.test(value)) continue;
      smuggled.push({ kind: note?.kind, key, value });
    }
  }
  check("除了她的答案，沒有一則觀測帶自由文字", smuggled.length === 0, JSON.stringify(smuggled));
}

console.log("");
if (failures > 0) {
  console.log(`✗ ${failures} 條 Persona 契約沒守住。`);
  process.exit(1);
}
console.log(
  "✔ Persona v3：17 位本機角色、544+68+340 段 bundled 語音、文字題全走大腦與三路 speaking 都守住了。",
);
