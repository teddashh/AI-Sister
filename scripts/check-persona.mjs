#!/usr/bin/env node
/*
 * Persona v2 的行為契約。
 *
 * 直接載入產品 app.js，而不是抄一份選台詞邏輯。這裡特別守四條容易各自看起來
 * 正確、湊起來卻說謊的縫：17 人都必須有隨程式提供的真角色圖，任何 pack 狀態都
 * 不准退回字母；開場／五秒輪詢不准自己開口；native button 的 click 才能播放固定
 * WAV 或明確標成 localService 的系統語音；關掉角色不可以連搜尋與錄製狀態一起關。
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
const SETTINGS_CATALOG_SOURCE = read(join(UI, "personas/catalog.js"));
const MAIN = read(join(ROOT, "apps/desktop/src-tauri/src/main.rs"));
const CONFIG = read(join(ROOT, "crates/sister-core/src/config.rs"));
const BUNDLED = JSON.parse(read(join(UI, "personas/manifest.json")));
const REELS = JSON.parse(read(join(UI, "persona-reels/manifest.json")));
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
  deepseek: { alias: "DeepSeek", tagline: "深挖證據與原理。", palette: ["#12213A", "#F8FAFC", "#60A5FA"], first: "我來往下挖。你可以直接問。" },
  qwen: { alias: "Qwen", tagline: "布局、控場與收斂。", palette: ["#312E81", "#F8FAFC", "#C4B5FD"], first: "我來收斂。你可以直接問。" },
  mistral: { alias: "Mistral", tagline: "俐落拆解，減少多餘協調。", palette: ["#451A03", "#FFFBEB", "#F59E0B"], first: "直接拆開來看。你可以直接問。" },
  venice: { alias: "Llama", tagline: "自由、直接、不受拘束。", palette: ["#3F1D2E", "#FFF7ED", "#FB7185"], first: "先講最直接的。你可以直接問。" },
  sakana: { alias: "Sakana", tagline: "保留變體，試另一條演化路徑。", palette: ["#164E63", "#ECFEFF", "#67E8F9"], first: "我們試另一條路。你可以直接問。" },
  perplexity: { alias: "Perplexity", tagline: "先查證，再下結論。", palette: ["#134E4A", "#F0FDFA", "#5EEAD4"], first: "我先查證。你可以直接問。" },
  glm: { alias: "GLM", tagline: "先做出可動的版本。", palette: ["#1E3A5F", "#EFF6FF", "#93C5FD"], first: "先做一版。你可以直接問。" },
  kimi: { alias: "Kimi", tagline: "守住前文、脈絡與交接。", palette: ["#312E81", "#F7F7FF", "#C7D2FE"], first: "我接著前面。你可以直接問。" },
  hunyuan: { alias: "Hunyuan", tagline: "把上下游與被漏掉的人接回來。", palette: ["#0C4A6E", "#F4F8FF", "#78B8FF"], first: "我把上下游接起來。你可以直接問。" },
  minimax: { alias: "MiniMax", tagline: "先讓作品能看、能聽、能感受到。", palette: ["#4A0D24", "#FFF6F8", "#FB923C"], first: "先讓它活起來。你可以直接問。" },
  nemotron: { alias: "Nemotron", tagline: "工程調度與可部署交付。", palette: ["#0B0F0A", "#F9FAFB", "#76B900"], first: "把交付路徑釘住。你可以直接問。" },
  cohere: { alias: "Cohere", tagline: "多方溝通、引用與協議。", palette: ["#243C34", "#F7F8F3", "#D18EE2"], first: "我把每一方都放進來。你可以直接問。" },
  mimo: { alias: "MiMo", tagline: "先看人用起來順不順。", palette: ["#431407", "#FFF8F1", "#FF6900"], first: "先看用起來順不順。你可以直接問。" },
});

async function open(personaView = persona(), options = {}) {
  let plays = 0;
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
      element.pause = () => {};
      element.play = () => {
        plays += 1;
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
      if (name === "src") src = "";
      removeAttribute(name);
    };
    return element;
  };

  globalThis.document = fakeDocument(node, {
    visibilityState: "visible",
    documentElement: root,
    createElement,
  });
  globalThis.location = { search: "" };
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
    cancel() {},
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
  globalThis.__TAURI__ = {
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
            return "recording";
          case "recorder_supervisor_state":
            return { phase: "stopped", failures: 0, message: null };
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
  const appScript = HTML.indexOf('<script type="module" src="./app.js"></script>');
  check(
    "本機 manifest 在 app.js 前預載",
    manifestScript >= 0 && appScript > manifestScript,
    { manifestScript, appScript },
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
    "layer 位置以 manifest viewport crop 換算，不把完整 1280 canvas 硬塞進頭像",
    firstImage.style.left ===
      `${(((firstLayer.x - chatgptRig.viewport.x) / chatgptRig.viewport.width) * 100).toFixed(5)}%` &&
      firstImage.style.top ===
        `${(((firstLayer.y - chatgptRig.viewport.y) / chatgptRig.viewport.height) * 100).toFixed(5)}%` &&
      firstImage.style.width ===
        `${((firstLayer.width / chatgptRig.viewport.width) * 100).toFixed(5)}%`,
    { left: firstImage.style.left, top: firstImage.style.top, width: firstImage.style.width },
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
    "56px 答案／URL 模式換回 WebP，不拆已 decode rig",
    STYLES.includes("body.has-hits .avatar.reel-ready .portrait") &&
      STYLES.includes("body.has-url-policy .avatar.reel-ready .portrait") &&
      STYLES.includes("body.has-hits .persona-reel") &&
      STYLES.includes("body.has-url-policy .persona-reel"),
  );
}

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

console.log("⑥ fixed-pack seam fail closed；即使假裝已安裝，語音也只從 click 播");
{
  const firstVoiceId = "chatgpt-greeting";
  const installed = persona("chatgpt", {
    voice_enabled: true,
    asset_pack: {
      phase: "installed",
      release_id: "fixture",
      portrait: { data_url: "data:image/webp;base64,AA==" },
      voice_lines: [
        { line_id: firstVoiceId, duration_ms: 1, spoken_text: "我在，隨時可以開始。" },
        // 私人答案不在目前 tap allowlist，renderer 必須丟掉。
        { line_id: "answer/private", duration_ms: 1 },
      ],
    },
  });
  const p = await open(installed);
  check(
    "舊 pack portrait 不覆蓋 current bundled portrait",
    !p.node("[data-persona-portrait]").hidden &&
      p.node("[data-persona-portrait]").src === "./personas/chatgpt.webp",
    p.node("[data-persona-portrait]").src,
  );
  await p.poll();
  check("開場／poll 不播 voice", p.plays() === 0, p.plays());
  check("開場沒有預先取得 WAV", p.voiceReads.length === 0, p.voiceReads);
  await p.clickAvatar();
  check("click 只向 Rust 取當句 fixed voice", JSON.stringify(p.voiceReads) === JSON.stringify([firstVoiceId]), p.voiceReads);
  check("明確 click 的固定 line 才播一次", p.plays() === 1, p.plays());
  check("播放來源只能是 exact WAV data URL", p.node("[data-persona-audio]").src.startsWith("data:audio/wav;base64,"), p.node("[data-persona-audio]").src);
  check(
    "fixed WAV 真正開始播放才進 speaking",
    p.node("[data-avatar]").classList.contains("speaking"),
  );
  p.finishAudio();
  check(
    "fixed WAV ended 清掉 speaking",
    !p.node("[data-avatar]").classList.contains("speaking"),
  );

  const fixedError = await open(installed);
  await fixedError.clickAvatar();
  fixedError.failAudio();
  check(
    "fixed WAV error 清掉 speaking",
    !fixedError.node("[data-avatar]").classList.contains("speaking"),
  );

  const fixedStopped = await open(installed);
  await fixedStopped.clickAvatar();
  await fixedStopped.ask("下一題");
  check(
    "換題 stop 清掉 fixed WAV speaking",
    !fixedStopped.node("[data-avatar]").classList.contains("speaking"),
  );
  check(
    "display line ID 與 manifest voice ID 分開",
    SRC.includes("voiceLineId: `${id}-greeting`") &&
      SRC.includes('invoke("persona_voice_read", { lineId: line.voiceLineId })') &&
      !SRC.includes('invoke("persona_voice_read", { lineId: line.id })'),
  );

  const off = await open({ ...installed, voice_enabled: false });
  await off.clickAvatar();
  check("voice gate 關著完全不播", off.plays() === 0, off.plays());
  check("voice gate 關著也不取 WAV", off.voiceReads.length === 0, off.voiceReads);

  const wrongTranscript = await open({
    ...installed,
    asset_pack: {
      ...installed.asset_pack,
      voice_lines: [
        { line_id: firstVoiceId, duration_ms: 1, spoken_text: "另一句話" },
      ],
    },
  });
  await wrongTranscript.clickAvatar();
  check("metadata 逐字稿不同就不取 WAV", wrongTranscript.voiceReads.length === 0, wrongTranscript.voiceReads);

  for (const id of ["chatgpt", "claude", "gemini", "grok"]) {
    const approved = ASSET_PROJECTION.selectedAssets
      .filter(
        (asset) =>
          asset.association.kind === "voice" && asset.association.characterId === id,
      )
      .map((asset) => ({
        line_id: asset.association.lineId,
        duration_ms: asset.output.durationMs,
        spoken_text: asset.spokenText,
      }));
    const view = await open(
      persona(id, {
        voice_enabled: true,
        asset_pack: {
          phase: "installed",
          release_id: ASSET_PROJECTION.manifest.releaseId,
          portrait: { data_url: "data:image/webp;base64,AA==" },
          voice_lines: approved,
        },
      }),
    );
    await view.clickAvatar();
    await view.clickAvatar();
    check(
      `${id} 兩句 UI copy 逐字對上 selected voice projection`,
      JSON.stringify(view.voiceReads) ===
        JSON.stringify([`${id}-greeting`, `${id}-quiet`]),
      view.voiceReads,
    );
  }

  const local = await open(persona("mimo", { voice_enabled: true }), {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
  });
  await local.clickAvatar();
  check("沒有固定錄音的角色用 localService 中文聲音", local.speaks.length === 1, local.speaks);
  check("系統語音不向 Rust 取任意文字", local.voiceReads.length === 0, local.voiceReads);
  check("系統語音拿到的正是畫面固定台詞", local.speaks[0]?.text === "先看用起來順不順。你可以直接問。", local.speaks[0]?.text);
  check(
    "localService 排入 queue 還不算 speaking",
    !local.node("[data-avatar]").classList.contains("speaking"),
  );
  local.speaks[0]?.onstart?.();
  check(
    "localService onstart 才進 speaking",
    local.node("[data-avatar]").classList.contains("speaking"),
  );
  local.speaks[0]?.onend?.();
  check(
    "localService 最後一段 ended 清掉 speaking",
    !local.node("[data-avatar]").classList.contains("speaking"),
  );

  const remoteOnly = await open(persona("mimo", { voice_enabled: true }), {
    systemVoices: [{ name: "Remote", lang: "zh-TW", localService: false }],
  });
  await remoteOnly.clickAvatar();
  check("只有 remote voice 時保持安靜", remoteOnly.speaks.length === 0, remoteOnly.speaks);

  const staleFixed = await open(installed, {
    voiceReadResult: null,
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
  });
  await staleFixed.clickAvatar();
  check("fixed voice 回 null 時改用 localService，不把同一次 click 吃掉", staleFixed.speaks.length === 1, staleFixed.speaks);

  const pending = await open(installed, { deferVoiceRead: true });
  await pending.clickAvatar();
  check(
    "fixed WAV native read 還在飛時不冒充 speaking",
    !pending.node("[data-avatar]").classList.contains("speaking"),
  );
  await pending.ask("新問題");
  await pending.resolveVoiceRead();
  check("新問題使較晚回來的 fixed WAV 失效", pending.plays() === 0, pending.plays());

  const answerStopsPending = await open(installed, {
    deferVoiceRead: true,
    systemVoices: [{ name: "Remote", lang: "zh-TW", localService: false }],
  });
  await answerStopsPending.ask("這一題");
  await answerStopsPending.clickAvatar();
  check("答案底下真的有獨立朗讀按鈕", await answerStopsPending.clickAnswerRead());
  await answerStopsPending.resolveVoiceRead();
  check(
    "答案朗讀找不到 local voice 仍先使 pending fixed WAV 失效",
    answerStopsPending.plays() === 0 &&
      answerStopsPending.node("[data-persona-line]").textContent.includes("沒有回報可用的本機中文語音"),
    { plays: answerStopsPending.plays(), line: answerStopsPending.node("[data-persona-line]").textContent },
  );

  const answerLocal = await open(persona("mimo", { voice_enabled: true }), {
    systemVoices: [{ name: "Hanhan", lang: "zh-TW", localService: true }],
    askResult: {
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
    },
  });
  await answerLocal.ask("電話");
  await answerLocal.clickAnswerRead();
  const spokenAnswer = answerLocal.speaks[0]?.text ?? "";
  check("本機答案朗讀收到畫面答案正文", spokenAnswer.includes("客服專線 0800-080-123"), spokenAnswer);
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
    "本機 TTS async 失敗會明講且不改用雲端",
    answerLocal.node("[data-persona-line]").textContent ===
      "本機聲音這次沒有播成；我沒有改用雲端。",
    answerLocal.node("[data-persona-line]").textContent,
  );
  check(
    "localService error 也清掉 speaking",
    !answerLocal.node("[data-avatar]").classList.contains("speaking"),
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

console.log("⑦ᵇ remove 一開始就能獨立停聲，但內建角色圖不撤掉");
{
  const installed = persona("chatgpt", {
    voice_enabled: true,
    asset_pack: {
      phase: "installed",
      release_id: "fixture",
      portrait: { data_url: "data:image/webp;base64,AA==" },
      voice_lines: [
        { line_id: "chatgpt-greeting", duration_ms: 1, spoken_text: "我在，隨時可以開始。" },
      ],
    },
  });
  const p = await open(installed, { deferVoiceRead: true });
  await p.clickAvatar();
  check("remove 前單條 WAV 還在飛", JSON.stringify(p.voiceReads) === JSON.stringify(["chatgpt-greeting"]), p.voiceReads);
  check("remove 前是 ChatGPT 內建角色圖", !p.node("[data-persona-portrait]").hidden);
  await p.fromOutside("persona-media-stop");
  check("固定台詞立刻清掉", p.node("[data-persona-line]").hidden && p.node("[data-persona-line]").textContent === "");
  check(
    "內建角色圖不受 remove 影響",
    !p.node("[data-persona-portrait]").hidden &&
      p.node("[data-persona-portrait]").src === "./personas/chatgpt.webp",
    p.node("[data-persona-portrait]").src,
  );
  check("asset 狀態先 fail closed 成 unavailable", p.node("[data-avatar]").dataset.assetPack === "unavailable", p.node("[data-avatar]").dataset.assetPack);
  await p.resolveVoiceRead();
  check("較晚回來的 WAV 已失效，不會復播", p.plays() === 0, p.plays());
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

console.log("⑧ Rust／JS 邊界沒有把 Persona 接進答案或安全路徑");
{
  const askBody = SRC.match(/async function ask\(\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  const gateBody = SRC.match(/function renderGatekeeper\(view\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  check("ask 不讀 activeProfile/persona catalog", !/activeProfile|PERSONA_CATALOG|personaLine/u.test(askBody), askBody);
  check("Gatekeeper 不會觸發 tap-line", !/sayPersonaLine|personaLine|personaAudio/u.test(gateBody), gateBody);
  check("後端有獨立 persona_read command", MAIN.includes("fn persona_read(") && MAIN.includes("persona_read,"));
  check("fixed voice 是 typed click-time command", MAIN.includes("enum PersonaVoiceLineId") && MAIN.includes("fn persona_voice_read("));
  const voiceRead = MAIN.match(/fn persona_voice_read\([\s\S]*?\n\}/u)?.[0] ?? "";
  check(
    "voice read 也服從角色、tap-line 與 voice 三道開關",
    voiceRead.includes("persona.visible().get()") &&
      voiceRead.includes("persona.tap_lines_enabled().get()") &&
      voiceRead.includes("persona.voice_enabled().get()"),
    voiceRead,
  );
  const localAssets = SRC.match(/function resolveLocalAssets\(view, persona\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  check(
    "未安裝／下載中／刪除中只關 fixed voice，不碰 bundled portrait",
    localAssets.includes('if (phase !== "installed")') &&
      localAssets.includes("voiceLineIds: new Set()") &&
      !localAssets.includes("portrait:"),
    localAssets,
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
    "答案朗讀是另一個 trusted click 且只走本機系統語音",
    answerRead.includes('button.textContent = "🔊 用本機聲音朗讀"') &&
      answerRead.includes("event?.isTrusted !== true") &&
      answerRead.includes("speakWithLocalSystemVoice(text)") &&
      !answerRead.includes("invoke("),
    answerRead,
  );
  const localSpeech = SRC.match(/function speakWithLocalSystemVoice\(text\) \{[\s\S]*?\n\}/u)?.[0] ?? "";
  const mediaStop =
    SRC.match(
      /^function stopPersonaMedia\(\{ cancelAzureNative = true \} = \{\}\) \{[\s\S]*?^\}$/mu,
    )?.[0] ?? "";
  check(
    "長答案按段依序播且整串有 revision cancel gate",
    SRC.includes("function chunkLocalSpeech(text, limit = 160)") &&
      localSpeech.includes("revision !== localSpeechRevision") &&
      localSpeech.includes("utterance.onend = speakNext"),
    localSpeech,
  );
  check(
    "fixed WAV 與系統 TTS 共用一顆完整 stop",
    mediaStop.includes("voiceRequest += 1") &&
      mediaStop.includes("personaAudio?.pause?.()") &&
      mediaStop.includes("stopLocalSpeech()") &&
      localSpeech.includes("stopPersonaMedia()") &&
      /async function ask\(\)[\s\S]*?stopPersonaMedia\(\)/u.test(SRC),
    mediaStop,
  );
}

console.log("");
if (failures > 0) {
  console.log(`✗ ${failures} 條 Persona 契約沒守住。`);
  process.exit(1);
}
console.log("✔ Persona v2 + Reel：17 位本機角色、active-only 分層與三路 speaking 都守住了。");
