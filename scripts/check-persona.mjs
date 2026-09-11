#!/usr/bin/env node
/*
 * Persona v2 的行為契約。
 *
 * 直接載入產品 app.js，而不是抄一份選台詞邏輯。這裡特別守四條容易各自看起來
 * 正確、湊起來卻說謊的縫：17 人都必須有隨程式提供的真角色圖，任何 pack 狀態都
 * 不准退回字母；開場／五秒輪詢不准自己開口；native button 的 click 才能播放固定
 * Ogg；一般答案才用明確標成 localService 的系統語音。關掉角色不可以連搜尋與
 * 錄製狀態一起關。
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
  const calls = [];
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
    visibilityState: "visible",
    documentElement: root,
    createElement,
  });
  globalThis.location = { search: options.search ?? "" };
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};
  globalThis.matchMedia = () => ({ matches: false, addEventListener() {} });
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
  const tauri = {
    core: {
      invoke: async (cmd, args) => {
        calls.push(cmd);
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
            return false;
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
    css,
    calls,
    intervals,
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
  return [...STYLES.matchAll(/(?<selector>[^{}]+)\{(?<body>[^{}]*)\}/gu)]
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
      tauriWithDemoQuery.node("#pause").textContent === "⏸" &&
      tauriWithDemoQuery.node("#pause").title === "暫停記錄" &&
      tauriWithDemoQuery.node("#pause").dataset["aria-pressed"] === "false" &&
      tauriWithDemoQuery.node("[data-hits]").hidden === true &&
      tauriWithDemoQuery.node("[data-hits]").children.length === 0 &&
      !tauriWithDemoQuery.node("[data-state-line]").textContent.includes("等了 25 秒"),
    {
      state: tauriWithDemoQuery.node("[data-avatar]").dataset.state,
      line: tauriWithDemoQuery.node("[data-state-line]").textContent,
      pause: tauriWithDemoQuery.node("#pause").textContent,
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

console.log("② 每一下 click 只走固定順序的 allowlist，不呼叫其他能力");
{
  const p = await open(persona("chatgpt"));
  const before = p.calls.length;
  const stateBefore = p.node("[data-state-line]").textContent;
  await p.clickAvatar();
  check("第一句固定", p.node("[data-persona-line]").textContent === "我在，隨時可以開始。", p.node("[data-persona-line]").textContent);
  check("點了才顯示", !p.node("[data-persona-line]").hidden);
  check("沒改錄製狀態字", p.node("[data-state-line]").textContent === stateBefore, p.node("[data-state-line]").textContent);
  check("沒叫 ask／CLI／Gatekeeper／hands", p.calls.length === before, p.calls.slice(before));
  await p.clickAvatar();
  check("第二句固定", p.node("[data-persona-line]").textContent === "我在，安靜地開始也很好。");
  await p.clickAvatar();
  check("第三下確定性回到第一句", p.node("[data-persona-line]").textContent === "我在，隨時可以開始。");
  await p.clickAvatar();
  check("第四下回到第二句", p.node("[data-persona-line]").textContent === "我在，安靜地開始也很好。");
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
  await p.clickAvatar();
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
  const appScript = HTML.indexOf('<script type="module" src="./app.js"></script>');
  check(
    "角色圖與語音 manifest 都在 app.js 前預載",
    manifestScript >= 0 &&
      voiceManifestScript > manifestScript &&
      appScript > voiceManifestScript,
    { manifestScript, voiceManifestScript, appScript },
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
  await p.clickAvatar();
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
    await view.clickAvatar();
    check(`${id} 有自己的 bundled tap voice`, view.plays() === 1, view.playedSources());
    check(`${id} 第一段文字逐字一致`, view.node("[data-persona-line]").textContent === expected.first);
  }

  const off = await open(persona("mimo", { voice_enabled: false }), {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
  });
  await off.clickAvatar();
  check("voice 關閉仍顯示台詞但完全不取播放權", off.plays() === 0 && !off.calls.includes("persona_fixed_voice_admit"));
  check("voice 關閉不借系統聲音", off.speaks.length === 0, off.speaks);

  const daily = await open(persona("kimi", { voice_enabled: true }));
  await daily.ask("早安。");
  check("精確日常短句也交給 CLI／記憶路徑", daily.calls.includes("ask"), daily.calls);
  check("文字問題不再用固定角色台詞繞過大腦", !daily.calls.includes("answer_cli_cancel"), daily.calls);
  check("文字問題不播固定 Ogg 冒充大腦答案", daily.plays() === 0, daily.playedSources());

  const normal = await open(persona("kimi", { voice_enabled: true }));
  await normal.ask("早安，昨天我在做什麼");
  check("不是 exact trigger 的問題仍完整交給 CLI／記憶路徑", normal.calls.includes("ask"), normal.calls);
  check("一般記憶問題不播固定語音冒充動態答案", normal.plays() === 0, normal.playedSources());

  const malformed = { ...VOICES, clips: VOICES.clips.slice(0, 543) };
  const closed = await open(persona("chatgpt", { voice_enabled: true }), {
    voiceManifest: malformed,
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
  });
  await closed.clickAvatar();
  check("不完整 manifest 整包拒絕且不借系統聲音", closed.plays() === 0 && closed.speaks.length === 0);

  const unapprovedVoices = { ...VOICES, rightsReview: "pending" };
  const rightsClosed = await open(persona("chatgpt", { voice_enabled: true }), {
    voiceManifest: unapprovedVoices,
  });
  await rightsClosed.clickAvatar();
  check("未核准權利投影讓整份語音 manifest fail closed", rightsClosed.plays() === 0);

  const wrongTotal = { ...VOICES, totals: { ...VOICES.totals, oggBytes: 8918727 } };
  const totalClosed = await open(persona("chatgpt", { voice_enabled: true }), {
    voiceManifest: wrongTotal,
  });
  await totalClosed.clickAvatar();
  check("總 bytes 漂移讓 544 段全部停用", totalClosed.plays() === 0);

  const wrongGroup = JSON.parse(JSON.stringify(VOICES));
  wrongGroup.clips[0].group = "bestie";
  const groupClosed = await open(persona("chatgpt", { voice_enabled: true }), {
    voiceManifest: wrongGroup,
  });
  await groupClosed.clickAvatar();
  check("角色分組漂移不能只停一段，必須整份停用", groupClosed.plays() === 0);

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
  const localLeaseEnds = () =>
    answerLocal.calls.filter((cmd) => cmd === "master_stop_presentation_end").length;
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

console.log("");
if (failures > 0) {
  console.log(`✗ ${failures} 條 Persona 契約沒守住。`);
  process.exit(1);
}
console.log("✔ Persona v3：17 位本機角色、544 段 bundled 語音、文字題全走大腦與三路 speaking 都守住了。");
