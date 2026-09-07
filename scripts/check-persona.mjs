#!/usr/bin/env node
/*
 * Persona v1 的行為契約。
 *
 * 直接載入產品 app.js，而不是抄一份選台詞邏輯。這裡特別守四條容易各自看起來
 * 正確、湊起來卻說謊的縫：開場／五秒輪詢不准自己開口；native button 的 click
 * 才能從固定 allowlist 取字；關掉角色不可以連搜尋與錄製狀態一起關；未來的本機
 * voice pack 也只能替同一次 click 選中的固定 `voiceLineId` 播放，不能把顯示台詞
 * 的版本 ID 當素材 ID，更不能朗讀私人答案。
 */

import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { domOf, fakeDocument, fakeEl, loader, read, watchNonsense } from "./fake-dom.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const UI = join(ROOT, "apps/desktop/ui");
const HTML = read(join(UI, "index.html"));
const SRC = read(join(UI, "app.js"));
const MAIN = read(join(ROOT, "apps/desktop/src-tauri/src/main.rs"));
const CONFIG = read(join(ROOT, "crates/sister-core/src/config.rs"));
const boot = loader(SRC);
const tick = () => new Promise((done) => setTimeout(done, 20));

const EMPTY_PACK = Object.freeze({
  phase: "unavailable",
  release_id: null,
  portrait: null,
  voice_lines: [],
});

function persona(id = "neutral", over = {}) {
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
  neutral: {
    alias: "Neutral",
    glyph: "S",
    tagline: "原來的字母人；安靜待著，只在你點她或問她時回應。",
    palette: ["#F6ECDF", "#2E2140", "#955572"],
    first: "我在。你可以直接問。",
  },
  chatgpt: {
    alias: "Aster",
    glyph: "T",
    tagline: "沉著務實，會把選項整理清楚，陪你照自己的步調決定。",
    palette: ["#0B1F33", "#F8FAFC", "#5EEAD4"],
    first: "我在。 隨時可以開始。",
  },
  claude: {
    alias: "Cedar",
    glyph: "C",
    tagline: "溫柔細膩，願意留白，也陪你慢慢想清楚每個細節。",
    palette: ["#12372A", "#F8FAFC", "#A7F3D0"],
    first: "慢慢來。 隨時可以開始。",
  },
  gemini: {
    alias: "Mira",
    glyph: "G",
    tagline: "好奇敏銳，喜歡發現日常模式，從不替你的節奏打分。",
    palette: ["#172554", "#F8FAFC", "#A5B4FC"],
    first: "一起看看。 隨時可以開始。",
  },
  grok: {
    alias: "Rook",
    glyph: "X",
    tagline: "直率活潑，帶點玩心，給你輕快但不催促的陪伴。",
    palette: ["#3B1B0B", "#F8FAFC", "#FDE68A"],
    first: "收到。 隨時可以開始。",
  },
});

async function open(personaView = persona(), options = {}) {
  let plays = 0;
  let finishPersonaRead = null;
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

  globalThis.document = fakeDocument(node, {
    visibilityState: "visible",
    documentElement: root,
  });
  globalThis.location = { search: "" };
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};
  globalThis.matchMedia = () => ({ matches: false, addEventListener() {} });
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
            return {
              line_id: args?.lineId,
              data_url: "data:audio/wav;base64,AA==",
            };
          case "recording_state":
            return "recording";
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
    plays: () => plays,
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
  check("第一句固定", p.node("[data-persona-line]").textContent === "我在。 隨時可以開始。", p.node("[data-persona-line]").textContent);
  check("點了才顯示", !p.node("[data-persona-line]").hidden);
  check("沒改錄製狀態字", p.node("[data-state-line]").textContent === stateBefore, p.node("[data-state-line]").textContent);
  check("沒叫 ask／CLI／Gatekeeper／hands", p.calls.length === before, p.calls.slice(before));
  await p.clickAvatar();
  check("第二句固定", p.node("[data-persona-line]").textContent === "我在。 可以照自己的步調探索。");
  await p.clickAvatar();
  check("第三句固定", p.node("[data-persona-line]").textContent === "我在。 安靜地開始也很好。");
  await p.clickAvatar();
  check("第四下確定性回到第一句", p.node("[data-persona-line]").textContent === "我在。 隨時可以開始。");
}

console.log("③ 五個本機 catalog 身分、glyph、tagline、palette 都固定而且可讀");
check("鍵盤 focus ring 對固定 stage ≥ 4.5:1", contrast("#F6ECDF", "#955572") >= 4.5);
check(
  "focus ring 使用 stage theme，不借 persona accent",
  /\.avatar:focus-visible\s*\{[^}]*var\(--letter-accent\)/u.test(read(join(UI, "styles.css"))),
);
for (const [id, expected] of Object.entries(EXPECTED)) {
  const p = await open(persona(id));
  const avatar = p.node("[data-avatar]");
  check(`${id} 穩定 ID`, avatar.dataset.persona === id, avatar.dataset.persona);
  check(`${id} glyph`, p.node("[data-persona-glyph]").textContent === expected.glyph, p.node("[data-persona-glyph]").textContent);
  check(`${id} alias/tagline`, avatar.title === `${expected.alias}：${expected.tagline}`, avatar.title);
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
  const p = await open(persona("neutral", { tap_lines: false, motion: false }));
  check("角色仍看得到", !p.node("[data-avatar]").hidden);
  check("沒有功能的 button 不留 focus target", p.node("[data-avatar]").disabled);
  check("動畫 class 沒掛上", !globalThis.document.documentElement.classList.contains("motion"));
  await p.forceAvatarHandler();
  check("tap-lines 關掉就沒有台詞", p.node("[data-persona-line]").textContent === "");

  await p.fromOutside("persona-changed", persona("claude"));
  check("事件換成 Cedar", p.node("[data-avatar]").dataset.persona === "claude");
  check("glyph 同步換成 C", p.node("[data-persona-glyph]").textContent === "C");
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
        { line_id: firstVoiceId, duration_ms: 1 },
        // 私人答案不在目前 tap allowlist，renderer 必須丟掉。
        { line_id: "answer/private", duration_ms: 1 },
      ],
    },
  });
  const p = await open(installed);
  check("已驗過的 portrait 取代 glyph", !p.node("[data-persona-portrait]").hidden && p.node("[data-persona-glyph]").hidden);
  await p.poll();
  check("開場／poll 不播 voice", p.plays() === 0, p.plays());
  check("開場沒有預先取得 WAV", p.voiceReads.length === 0, p.voiceReads);
  await p.clickAvatar();
  check("click 只向 Rust 取當句 fixed voice", JSON.stringify(p.voiceReads) === JSON.stringify([firstVoiceId]), p.voiceReads);
  check("明確 click 的固定 line 才播一次", p.plays() === 1, p.plays());
  check("播放來源只能是 exact WAV data URL", p.node("[data-persona-audio]").src.startsWith("data:audio/wav;base64,"), p.node("[data-persona-audio]").src);
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
}

console.log("⑦ 較舊的開場讀取不會蓋掉較新的設定事件");
{
  const p = await open(persona("chatgpt"), { deferPersonaRead: true });
  await p.fromOutside("persona-changed", persona("grok"));
  check("新事件先換成 Rook", p.node("[data-avatar]").dataset.persona === "grok");
  await p.resolvePersonaRead();
  check("舊 initial read 回來仍是 Rook", p.node("[data-avatar]").dataset.persona === "grok");
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
  check("asset resolver 目前明講 unavailable", /fn resolve_local_persona_assets[\s\S]*?PersonaAssetPackPhase::Unavailable/u.test(MAIN));
  check("typed config 只有五個穩定 ID", /enum PersonaId \{[\s\S]*?Neutral,[\s\S]*?Chatgpt,[\s\S]*?Claude,[\s\S]*?Gemini,[\s\S]*?Grok,/u.test(CONFIG));
  check("voice gate 預設 false", /voice_enabled:\s*false/u.test(CONFIG));
}

console.log("");
if (failures > 0) {
  console.log(`✗ ${failures} 條 Persona 契約沒守住。`);
  process.exit(1);
}
console.log("✔ Persona v1：只在明確點擊後說固定台詞；本機 fallback、關閉與素材邊界都守住了。 ");
