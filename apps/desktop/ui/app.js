// 桌面姊妹的行為。純 ES module，**沒有打包步驟**——這個檔案就是瀏覽器讀到的
// 那個檔案。Phase 1 的退場條件裡有一條「clone → 跑起來 < 10 分鐘」，而在一個
// Rust repo 裡塞一套 Node 工具鏈是最容易讓那條過不了的東西。
//
// 需要型別的時候再加 tsc，那是改一個檔案的事；現在還不需要。

/**
 * 她**正在做什麼**。注意 `paused` 不在這裡：它是另一個維度（見下面的
 * `paused`），因為她可以「暫停中、同時正在想一個問題的答案」。
 */
const STATES = Object.freeze(["idle", "thinking"]);
// `checking` 只存在 renderer 冷啟動；native 的封閉契約是另外四態。沒量到不是
// `uncertain`（讀過但讀不懂），也不能先假設 clear。
const MASTER_STOP_PHASES = Object.freeze([
  "checking",
  "clear",
  "stopping",
  "stopped",
  "uncertain",
]);

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

const MASTER_STOP_LINES = Object.freeze({
  stopping: "正在完成全停：新工作已拒絕，先前開始的工作仍在排乾",
  stopped: "已全停：capture／brain／hands 都不會動",
  uncertain: "無法確認全停狀態：為安全起見不會開始新的 capture／brain／hands 工作",
});

const avatar = document.querySelector("[data-avatar]");
const personaPortrait = document.querySelector("[data-persona-portrait]");
const personaReel = document.querySelector("[data-persona-reel]");
const personaLine = document.querySelector("[data-persona-line]");
const personaAudio = document.querySelector("[data-persona-audio]");
const stateLine = document.querySelector("[data-state-line]");
const askInput = document.querySelector("[data-ask-input]");
const askSend = document.querySelector("[data-ask-send]");
const pinButton = document.querySelector("#pin");
const hideButton = document.querySelector("#hide");
const pauseButton = document.querySelector("#pause");
const timelineButton = document.querySelector("#timeline");
const settingsButton = document.querySelector("#settings");
const chromeBar = document.querySelector("[data-chrome-bar]");
const chromeToggle = document.querySelector("[data-chrome-toggle]");
const consentGuide = document.querySelector("[data-consent-guide]");
const consentProgress = document.querySelector("[data-consent-progress]");
const consentWording = document.querySelector("[data-consent-wording]");
const consentWithout = document.querySelector("[data-consent-without]");
const consentListen = document.querySelector("[data-consent-listen]");
const consentPrompt = document.querySelector("[data-consent-prompt]");
const consentResult = document.querySelector("[data-consent-result]");
const wakeButton = document.querySelector("[data-wake]");
const utterance = document.querySelector("[data-utterance]");
const utteranceText = document.querySelector("[data-utterance-text]");
const utteranceEvidence = document.querySelector("[data-utterance-evidence]");
const utteranceTargetProvenance = document.querySelector("[data-utterance-target-provenance]");
const utteranceActions = document.querySelector("[data-utterance-actions]");
const utteranceClose = document.querySelector("[data-utterance-close]");
const utteranceOther = document.querySelector("[data-utterance-other]");
const utteranceResult = document.querySelector("[data-utterance-result]");
const gateDebug = document.querySelector("[data-gate-debug]");
const suggestionButton = document.querySelector("[data-utterance-suggestion]");
const handsLog = document.querySelector("[data-hands-log]");
const urlPolicy = document.querySelector("[data-url-policy]");
const urlPolicyQuestion = document.querySelector("[data-url-policy-question]");
const urlPolicyActions = document.querySelector("[data-url-policy-actions]");
const urlPolicyNote = document.querySelector("[data-url-policy-note]");
const urlPolicyResult = document.querySelector("[data-url-policy-result]");

// ---------- Persona catalog ----------

/**
 * 17 位角色共用同一份封閉的本機語音契約：每人基本包 8 句、擴充包 24 句。
 * Manifest 在 build 前逐檔驗過 Ogg、bytes、SHA-256、時長、文字與人聲；renderer
 * 仍只接受 exact roster、exact path 與 exact trigger，不能把任意 URL 當語音來源。
 */
const DIALOGUE_PERSONA_IDS = Object.freeze([
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
]);
const DIALOGUE_PACK_LINES = Object.freeze({
  base: Object.freeze([
    "tap-general",
    "tap-quiet",
    "good-morning",
    "good-evening",
    "welcome-back",
    "choose-topic",
    "start-small",
    "good-night",
  ]),
  extension: Object.freeze([
    "good-afternoon",
    "how-are-you",
    "thanks",
    "apology",
    "compliment",
    "tired",
    "stressed",
    "stuck",
    "unfocused",
    "ready-to-start",
    "keep-going",
    "task-done",
    "drink-water",
    "hungry",
    "ate",
    "lonely",
    "miss-you",
    "bored",
    "bad-day",
    "good-day",
    "small-win",
    "back-from-break",
    "leaving",
    "see-you",
  ]),
});

function hasExactKeys(value, keys) {
  return (
    value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    Object.keys(value).sort().join("\0") === [...keys].sort().join("\0")
  );
}

function sameStrings(left, right) {
  return (
    Array.isArray(left) &&
    left.length === right.length &&
    left.every((value, index) => value === right[index])
  );
}

function normalizeDailyPhrase(value) {
  return String(value)
    .normalize("NFKC")
    .trim()
    .replace(/[。！？!?～~]+$/u, "")
    .trim();
}

function dialogueVoiceLibrary(raw) {
  const empty = Object.freeze({ byPersona: new Map(), exactReplies: new Map() });
  if (
    !hasExactKeys(raw, [
      "schema",
      "locale",
      "roster",
      "engine",
      "rightsReview",
      "ownerGrant",
      "notice",
      "packs",
      "clips",
      "totals",
    ]) ||
    raw?.schema !== "ai-sister/persona-voices/v1" ||
    raw?.locale !== "zh-TW" ||
    raw?.roster !== "four-sisters-plus-thirteen-besties" ||
    !hasExactKeys(raw?.engine, ["name", "modelSnapshot", "license"]) ||
    raw.engine.name !== "MediaTek-Research/BreezyVoice-300M" ||
    raw.engine.modelSnapshot !== "e33b502e0ac21c16b0ee0d00df66ac3fa737393d" ||
    raw.engine.license !== "Apache-2.0" ||
    raw?.rightsReview !== "approved-owner-grant" ||
    !hasExactKeys(raw?.ownerGrant, ["grantedOn", "grantor", "license", "scope"]) ||
    raw.ownerGrant.grantedOn !== "2026-09-09" ||
    raw.ownerGrant.grantor !== "Ted Huang" ||
    raw.ownerGrant.license !== "excluded-from-Apache-2.0" ||
    raw.ownerGrant.scope !==
      "Unmodified inclusion of the 544 generated fixed-dialogue clips hash-listed by this manifest in the AI-Sister source tree and official builds" ||
    raw?.notice !== "NOTICE.md" ||
    !hasExactKeys(raw?.totals, [
      "personas",
      "baseLinesPerPersona",
      "extensionLinesPerPersona",
      "clips",
      "oggBytes",
      "durationMs",
    ]) ||
    raw?.totals?.personas !== 17 ||
    raw?.totals?.baseLinesPerPersona !== 8 ||
    raw?.totals?.extensionLinesPerPersona !== 24 ||
    raw?.totals?.clips !== 544 ||
    raw?.totals?.oggBytes !== 8918728 ||
    raw?.totals?.durationMs !== 1780686 ||
    !Array.isArray(raw?.packs) ||
    raw.packs.length !== 2 ||
    !Array.isArray(raw?.clips)
  ) {
    return empty;
  }
  const packLines = new Map();
  for (const [index, id] of ["base", "extension"].entries()) {
    const pack = raw.packs[index];
    const expectedLines = DIALOGUE_PACK_LINES[id];
    if (
      !hasExactKeys(pack, ["id", "delivery", "lineIds"]) ||
      pack?.id !== id ||
      pack?.delivery !== "bundled" ||
      !sameStrings(pack?.lineIds, expectedLines)
    ) {
      return empty;
    }
    packLines.set(id, new Set(pack.lineIds));
  }
  if (raw.clips.length !== 544) return empty;

  const byPersona = new Map(DIALOGUE_PERSONA_IDS.map((id) => [id, new Map()]));
  const exactReplies = new Map();
  let totalBytes = 0;
  let totalDurationMs = 0;
  for (const value of raw.clips) {
    const personaLines = byPersona.get(value?.persona);
    const allowedLines = packLines.get(value?.pack);
    const expectedFile = `${value?.pack}/${value?.persona}/${value?.lineId}.ogg`;
    const personaIndex = DIALOGUE_PERSONA_IDS.indexOf(value?.persona);
    const expectedUse = ["tap-general", "tap-quiet"].includes(value?.lineId)
      ? "avatar-tap"
      : "exact-intent-reply";
    if (
      !hasExactKeys(value, [
        "persona",
        "group",
        "lineId",
        "pack",
        "use",
        "text",
        "triggers",
        "file",
        "bytes",
        "sha256",
        "durationMs",
      ]) ||
      personaLines === undefined ||
      value?.group !== (personaIndex < 4 ? "sister" : "bestie") ||
      allowedLines === undefined ||
      !allowedLines.has(value?.lineId) ||
      personaLines.has(value.lineId) ||
      value?.use !== expectedUse ||
      typeof value?.text !== "string" ||
      value.text.length < 2 ||
      value.text.length > 100 ||
      !Array.isArray(value?.triggers) ||
      value?.file !== expectedFile ||
      !Number.isSafeInteger(value?.bytes) ||
      value.bytes < 1 ||
      typeof value?.sha256 !== "string" ||
      !/^[a-f0-9]{64}$/u.test(value.sha256) ||
      !Number.isSafeInteger(value?.durationMs) ||
      value.durationMs < 700 ||
      value.durationMs > 12000
    ) {
      return empty;
    }
    totalBytes += value.bytes;
    totalDurationMs += value.durationMs;
    const clip = Object.freeze({
      id: `persona-dialogue/v1/${value.persona}/${value.lineId}`,
      persona: value.persona,
      lineId: value.lineId,
      pack: value.pack,
      use: value.use,
      text: value.text,
      triggers: Object.freeze([...value.triggers]),
      file: `./persona-voices/v1/${value.file}`,
    });
    personaLines.set(value.lineId, clip);
    if (value.use === "avatar-tap") {
      if (value.triggers.length !== 0) return empty;
      continue;
    }
    if (value.triggers.length === 0) return empty;
    for (const trigger of value.triggers) {
      const normalized = normalizeDailyPhrase(trigger);
      if (
        typeof trigger !== "string" ||
        trigger !== trigger.trim() ||
        normalized === "" ||
        (exactReplies.has(normalized) && exactReplies.get(normalized) !== value.lineId)
      ) {
        return empty;
      }
      exactReplies.set(normalized, value.lineId);
    }
  }
  for (const lines of byPersona.values()) {
    if (lines.size !== 32) return empty;
    const taps = [...lines.values()].filter((clip) => clip.use === "avatar-tap");
    const replies = [...lines.values()].filter((clip) => clip.use === "exact-intent-reply");
    if (taps.length !== 2 || replies.length !== 30) return empty;
  }
  if (totalBytes !== raw.totals.oggBytes || totalDurationMs !== raw.totals.durationMs) {
    return empty;
  }
  return Object.freeze({ byPersona, exactReplies });
}

const DIALOGUE_VOICES = dialogueVoiceLibrary(globalThis.__AI_SISTER_PERSONA_VOICES__);

const CONSENT_SHEETS = Object.freeze([
  "local-recording",
  "cloud-reading",
  "frame-storage",
  "azure-tts",
]);

/**
 * 同意書錄音和 544 句日常台詞分成兩包：條文改版時只換這 68 段，也不會讓
 * 「早安」之類的 exact route 誤觸法律文字。真正顯示的條文仍只讀 native；
 * 播放前還會逐字比對，任何版本不一致都保持靜音。
 */
function consentVoiceLibrary(raw) {
  const empty = Object.freeze({ byPersona: new Map() });
  if (
    !hasExactKeys(raw, [
      "schema",
      "locale",
      "roster",
      "engine",
      "rightsReview",
      "ownerGrant",
      "notice",
      "sheets",
      "clips",
      "totals",
    ]) ||
    raw?.schema !== "ai-sister/persona-consent-voices/v1" ||
    raw?.locale !== "zh-TW" ||
    raw?.roster !== "four-sisters-plus-thirteen-besties" ||
    !hasExactKeys(raw?.engine, ["name", "modelSnapshot", "license"]) ||
    raw.engine.name !== "MediaTek-Research/BreezyVoice-300M" ||
    raw.engine.modelSnapshot !== "e33b502e0ac21c16b0ee0d00df66ac3fa737393d" ||
    raw.engine.license !== "Apache-2.0" ||
    raw?.rightsReview !== "approved-owner-grant" ||
    !hasExactKeys(raw?.ownerGrant, ["grantedOn", "grantor", "license", "scope"]) ||
    raw.ownerGrant.grantedOn !== "2026-09-11" ||
    raw.ownerGrant.grantor !== "Ted Huang" ||
    raw.ownerGrant.license !== "excluded-from-Apache-2.0" ||
    raw.ownerGrant.scope !==
      "Unmodified inclusion of the 68 generated consent-reading clips hash-listed by this manifest in the AI-Sister source tree and official builds" ||
    raw?.notice !== "NOTICE.md" ||
    !sameStrings(raw?.sheets, CONSENT_SHEETS) ||
    !hasExactKeys(raw?.totals, [
      "personas",
      "sheetsPerPersona",
      "clips",
      "oggBytes",
      "durationMs",
    ]) ||
    raw.totals.personas !== 17 ||
    raw.totals.sheetsPerPersona !== 4 ||
    raw.totals.clips !== 68 ||
    !Array.isArray(raw?.clips) ||
    raw.clips.length !== 68
  ) {
    return empty;
  }

  const byPersona = new Map(DIALOGUE_PERSONA_IDS.map((id) => [id, new Map()]));
  let totalBytes = 0;
  let totalDurationMs = 0;
  for (const value of raw.clips) {
    const personaSheets = byPersona.get(value?.persona);
    const personaIndex = DIALOGUE_PERSONA_IDS.indexOf(value?.persona);
    const expectedFile = `${value?.persona}/${value?.sheet}.ogg`;
    if (
      !hasExactKeys(value, [
        "persona",
        "group",
        "sheet",
        "text",
        "file",
        "bytes",
        "sha256",
        "durationMs",
      ]) ||
      personaSheets === undefined ||
      value?.group !== (personaIndex < 4 ? "sister" : "bestie") ||
      !CONSENT_SHEETS.includes(value?.sheet) ||
      personaSheets.has(value.sheet) ||
      typeof value?.text !== "string" ||
      value.text.length < 10 ||
      value.text.length > 320 ||
      value?.file !== expectedFile ||
      !Number.isSafeInteger(value?.bytes) ||
      value.bytes < 1 ||
      typeof value?.sha256 !== "string" ||
      !/^[a-f0-9]{64}$/u.test(value.sha256) ||
      !Number.isSafeInteger(value?.durationMs) ||
      value.durationMs < 700 ||
      value.durationMs > 90000
    ) {
      return empty;
    }
    totalBytes += value.bytes;
    totalDurationMs += value.durationMs;
    personaSheets.set(
      value.sheet,
      Object.freeze({
        persona: value.persona,
        sheet: value.sheet,
        text: value.text,
        file: `./persona-consent-voices/v1/${value.file}`,
      }),
    );
  }
  if (
    [...byPersona.values()].some((sheets) => sheets.size !== 4) ||
    totalBytes !== raw.totals.oggBytes ||
    totalDurationMs !== raw.totals.durationMs
  ) {
    return empty;
  }
  return Object.freeze({ byPersona });
}

const CONSENT_VOICES = consentVoiceLibrary(globalThis.__AI_SISTER_CONSENT_VOICES__);

/*
 * 第三包：閒話。
 *
 * 前兩包（544 句日常 + 68 段同意書）都是「她要說一件事」；這一包不是。
 * 「阿唷」「煩耶」「呵呵」不是答案，是反應——**聲音不必等於回答**。所以它
 * 自成一包、自己一套 use，而且刻意不帶 `{lead}`／`{approach}` 模板：一句
 * 感嘆詞在誰嘴裡都是同一個字，讓它變成她的是那個聲音，不是那句話。
 *
 * 三種用途各有各的出口：
 *   avatar-poke  戳她的時候（`sayPersonaLine`）
 *   idle-giggle  沒事的時候自己笑一下（`scheduleIdleGiggle`）
 *   answer-beat  答案落地那一刻的墊話（`playAnswerBeat`）
 *
 * 時長下限是 250 毫秒，不是日常那包的 700——整包實測最短的是 perplexity 的
 * 「不要戳了。」424 毫秒，用日常那個門檻會把整包最想要的那幾句全擋掉。上限
 * 4 秒：閒話講超過 4 秒就不是閒話，而是模型跑掉了（實測最長 3548 毫秒）。
 *
 * 三句原本只有一聲笑的（「呵呵。」「嘻嘻。」「哈哈。」）刻意都補上一句話。理由
 * 不是文案，是驗得出來或驗不出來：光一聲笑的說話人嵌入幾乎不帶身分——17 個角色
 * 各錄那三句、51 段裸笑量下去，她自己那份參考音的相似度中位數只有 0.296，其中
 * 10 段最近的一份是別人，32 段贏不了第二名 0.10。也就是說一句裸的「哈哈。」是誰
 * 笑的，機器分不出來，那一格就沒有人在守。補上幾個字之後同一個度量中位數回到
 * 0.675、最低 0.483，51 段的領先幅度最少 0.190。
 */
const BANTER_USES = Object.freeze(["avatar-poke", "idle-giggle", "answer-beat"]);

function banterVoiceLibrary(raw) {
  const empty = Object.freeze({ byPersona: new Map() });
  if (
    !hasExactKeys(raw, [
      "schema",
      "locale",
      "roster",
      "engine",
      "rightsReview",
      "ownerGrant",
      "notice",
      "pack",
      "clips",
      "totals",
    ]) ||
    raw?.schema !== "ai-sister/persona-banter-voices/v1" ||
    raw?.locale !== "zh-TW" ||
    raw?.roster !== "four-sisters-plus-thirteen-besties" ||
    !hasExactKeys(raw?.engine, ["name", "modelSnapshot", "license"]) ||
    raw.engine.name !== "MediaTek-Research/BreezyVoice-300M" ||
    raw.engine.modelSnapshot !== "e33b502e0ac21c16b0ee0d00df66ac3fa737393d" ||
    raw.engine.license !== "Apache-2.0" ||
    raw?.rightsReview !== "approved-owner-grant" ||
    !hasExactKeys(raw?.ownerGrant, ["grantedOn", "grantor", "license", "scope"]) ||
    raw.ownerGrant.grantor !== "Ted Huang" ||
    raw.ownerGrant.license !== "excluded-from-Apache-2.0" ||
    raw?.notice !== "NOTICE.md" ||
    raw?.pack !== "banter" ||
    !hasExactKeys(raw?.totals, [
      "personas",
      "linesPerPersona",
      "clips",
      "oggBytes",
      "durationMs",
    ]) ||
    raw?.totals?.personas !== DIALOGUE_PERSONA_IDS.length ||
    !Number.isSafeInteger(raw?.totals?.linesPerPersona) ||
    raw.totals.linesPerPersona < 1 ||
    raw?.totals?.clips !== raw.totals.personas * raw.totals.linesPerPersona ||
    !Array.isArray(raw?.clips) ||
    raw.clips.length !== raw.totals.clips
  ) {
    return empty;
  }

  const byPersona = new Map(DIALOGUE_PERSONA_IDS.map((id) => [id, new Map()]));
  let totalBytes = 0;
  let totalDurationMs = 0;
  for (const value of raw.clips) {
    const personaLines = byPersona.get(value?.persona);
    const personaIndex = DIALOGUE_PERSONA_IDS.indexOf(value?.persona);
    const expectedFile = `banter/${value?.persona}/${value?.lineId}.ogg`;
    if (
      !hasExactKeys(value, [
        "persona",
        "group",
        "lineId",
        "pack",
        "use",
        "text",
        "file",
        "bytes",
        "sha256",
        "durationMs",
      ]) ||
      personaLines === undefined ||
      value?.group !== (personaIndex < 4 ? "sister" : "bestie") ||
      value?.pack !== "banter" ||
      personaLines.has(value.lineId) ||
      !BANTER_USES.includes(value?.use) ||
      typeof value?.text !== "string" ||
      value.text.length < 2 ||
      value.text.length > 20 ||
      value?.file !== expectedFile ||
      !Number.isSafeInteger(value?.bytes) ||
      value.bytes < 1 ||
      typeof value?.sha256 !== "string" ||
      !/^[a-f0-9]{64}$/u.test(value.sha256) ||
      !Number.isSafeInteger(value?.durationMs) ||
      value.durationMs < 250 ||
      value.durationMs > 4000
    ) {
      return empty;
    }
    totalBytes += value.bytes;
    totalDurationMs += value.durationMs;
    personaLines.set(
      value.lineId,
      Object.freeze({
        id: `persona-banter/v1/${value.persona}/${value.lineId}`,
        persona: value.persona,
        lineId: value.lineId,
        pack: "banter",
        use: value.use,
        text: value.text,
        triggers: Object.freeze([]),
        file: `./persona-banter-voices/v1/${value.file}`,
      }),
    );
  }
  // 每個角色都要有整包。少一個人就整包不要——「其他十六個人會扭會笑，換到她
  // 就變回木頭」比十七個人都不會動難解釋得多。
  for (const lines of byPersona.values()) {
    if (lines.size !== raw.totals.linesPerPersona) return empty;
  }
  for (const use of BANTER_USES) {
    for (const lines of byPersona.values()) {
      if (![...lines.values()].some((clip) => clip.use === use)) return empty;
    }
  }
  if (totalBytes !== raw.totals.oggBytes || totalDurationMs !== raw.totals.durationMs) {
    return empty;
  }
  return Object.freeze({ byPersona });
}

const BANTER_VOICES = banterVoiceLibrary(globalThis.__AI_SISTER_PERSONA_BANTER__);

function banterClips(id, use) {
  return Object.freeze(
    [...(BANTER_VOICES.byPersona.get(id)?.values() ?? [])].filter(
      (clip) => clip.use === use,
    ),
  );
}

function dialogueTaps(id) {
  return Object.freeze(
    [...(DIALOGUE_VOICES.byPersona.get(id)?.values() ?? [])].filter(
      (clip) => clip.use === "avatar-tap",
    ),
  );
}

function profile(value) {
  return Object.freeze({
    ...value,
    portrait: `./personas/${value.id}.webp`,
    palette: Object.freeze({ ...value.palette }),
    // 戳她的時候可以講的話 = 日常那包的兩句 + 閒話那包的每一句。合成一份
    // 是刻意的：`sayPersonaLine` 只要隨機挑一句，不必知道它從哪一包來。
    taps: Object.freeze([...value.taps, ...banterClips(value.id, "avatar-poke")]),
    giggles: banterClips(value.id, "idle-giggle"),
    beats: banterClips(value.id, "answer-beat"),
  });
}

const PERSONA_CATALOG = Object.freeze({
  chatgpt: profile({
    id: "chatgpt",
    alias: "ChatGPT",
    group: "四姊妹",
    tagline: "結構與驗證；固定台詞用「我在」開場。",
    palette: { background: "#0B1F33", foreground: "#F8FAFC", accent: "#5EEAD4" },
    voiceRate: 1.02,
    voicePitch: 1.05,
    taps: dialogueTaps("chatgpt"),
  }),
  claude: profile({
    id: "claude",
    alias: "Claude",
    group: "四姊妹",
    tagline: "論證與邊界；固定台詞用「慢慢來」開場。",
    palette: { background: "#12372A", foreground: "#F8FAFC", accent: "#A7F3D0" },
    voiceRate: 0.94,
    voicePitch: 1.0,
    taps: dialogueTaps("claude"),
  }),
  gemini: profile({
    id: "gemini",
    alias: "Gemini",
    group: "四姊妹",
    tagline: "打開可能；固定台詞用「一起看看」開場。",
    palette: { background: "#172554", foreground: "#F8FAFC", accent: "#A5B4FC" },
    voiceRate: 1.08,
    voicePitch: 1.1,
    taps: dialogueTaps("gemini"),
  }),
  grok: profile({
    id: "grok",
    alias: "Grok",
    group: "四姊妹",
    tagline: "直球測試；固定台詞用「收到」開場。",
    palette: { background: "#3B1B0B", foreground: "#F8FAFC", accent: "#FDE68A" },
    voiceRate: 1.12,
    voicePitch: 0.96,
    taps: dialogueTaps("grok"),
  }),
  deepseek: profile({
    id: "deepseek", alias: "DeepSeek", group: "13 位閨密", tagline: "深挖證據與原理。",
    palette: { background: "#12213A", foreground: "#F8FAFC", accent: "#60A5FA" },
    voiceRate: 0.96, voicePitch: 0.98, taps: dialogueTaps("deepseek"),
  }),
  qwen: profile({
    id: "qwen", alias: "Qwen", group: "13 位閨密", tagline: "布局、控場與收斂。",
    palette: { background: "#312E81", foreground: "#F8FAFC", accent: "#C4B5FD" },
    voiceRate: 1.0, voicePitch: 1.02, taps: dialogueTaps("qwen"),
  }),
  mistral: profile({
    id: "mistral", alias: "Mistral", group: "13 位閨密", tagline: "俐落拆解，減少多餘協調。",
    palette: { background: "#451A03", foreground: "#FFFBEB", accent: "#F59E0B" },
    voiceRate: 1.1, voicePitch: 1.0, taps: dialogueTaps("mistral"),
  }),
  venice: profile({
    id: "venice", alias: "Llama", group: "13 位閨密", tagline: "自由、直接、不受拘束。",
    palette: { background: "#3F1D2E", foreground: "#FFF7ED", accent: "#FB7185" },
    voiceRate: 1.12, voicePitch: 1.04, taps: dialogueTaps("venice"),
  }),
  sakana: profile({
    id: "sakana", alias: "Sakana", group: "13 位閨密", tagline: "保留變體，試另一條演化路徑。",
    palette: { background: "#164E63", foreground: "#ECFEFF", accent: "#67E8F9" },
    voiceRate: 0.96, voicePitch: 1.12, taps: dialogueTaps("sakana"),
  }),
  perplexity: profile({
    id: "perplexity", alias: "Perplexity", group: "13 位閨密", tagline: "先查證，再下結論。",
    palette: { background: "#134E4A", foreground: "#F0FDFA", accent: "#5EEAD4" },
    voiceRate: 1.06, voicePitch: 1.0, taps: dialogueTaps("perplexity"),
  }),
  glm: profile({
    id: "glm", alias: "GLM", group: "13 位閨密", tagline: "先做出可動的版本。",
    palette: { background: "#1E3A5F", foreground: "#EFF6FF", accent: "#93C5FD" },
    voiceRate: 1.08, voicePitch: 1.02, taps: dialogueTaps("glm"),
  }),
  kimi: profile({
    id: "kimi", alias: "Kimi", group: "13 位閨密", tagline: "守住前文、脈絡與交接。",
    palette: { background: "#312E81", foreground: "#F7F7FF", accent: "#C7D2FE" },
    voiceRate: 0.94, voicePitch: 1.06, taps: dialogueTaps("kimi"),
  }),
  hunyuan: profile({
    id: "hunyuan", alias: "Hunyuan", group: "13 位閨密", tagline: "把上下游與被漏掉的人接回來。",
    palette: { background: "#0C4A6E", foreground: "#F4F8FF", accent: "#78B8FF" },
    voiceRate: 1.0, voicePitch: 1.04, taps: dialogueTaps("hunyuan"),
  }),
  minimax: profile({
    id: "minimax", alias: "MiniMax", group: "13 位閨密", tagline: "先讓作品能看、能聽、能感受到。",
    palette: { background: "#4A0D24", foreground: "#FFF6F8", accent: "#FB923C" },
    voiceRate: 1.14, voicePitch: 1.12, taps: dialogueTaps("minimax"),
  }),
  nemotron: profile({
    id: "nemotron", alias: "Nemotron", group: "13 位閨密", tagline: "工程調度與可部署交付。",
    palette: { background: "#0B0F0A", foreground: "#F9FAFB", accent: "#76B900" },
    voiceRate: 1.02, voicePitch: 0.94, taps: dialogueTaps("nemotron"),
  }),
  cohere: profile({
    id: "cohere", alias: "Cohere", group: "13 位閨密", tagline: "多方溝通、引用與協議。",
    palette: { background: "#243C34", foreground: "#F7F8F3", accent: "#D18EE2" },
    voiceRate: 0.98, voicePitch: 1.08, taps: dialogueTaps("cohere"),
  }),
  mimo: profile({
    id: "mimo", alias: "MiMo", group: "13 位閨密", tagline: "先看人用起來順不順。",
    palette: { background: "#431407", foreground: "#FFF8F1", accent: "#FF6900" },
    voiceRate: 1.04, voicePitch: 1.1, taps: dialogueTaps("mimo"),
  }),
});

function profileFor(id) {
  return typeof id === "string" && Object.hasOwn(PERSONA_CATALOG, id)
    ? PERSONA_CATALOG[id]
    : null;
}

/**
 * Tauri 的 IPC。**在瀏覽器裡打開時是 null**，而那是刻意支援的：桌面姊妹整個
 * 是 HTML/CSS，所以它可以在一般瀏覽器裡開發、截圖、比對，不必每次都去開一個
 * 桌面視窗。所有會用到 IPC 的地方都要能在 null 之下安靜地降級。
 */
const invoke = globalThis.__TAURI__?.core?.invoke ?? null;

// ---------- 狀態 ----------

/**
 * 三個獨立的東西，不是四選一：
 *
 * - `state` 是她**正在做什麼**（在聽／想一下）。
 * - `paused` 是她**有沒有在看**，而且真相不在這個行程裡，是 data dir 裡的
 *   一個檔案（見 sister-core 的 `pause` 模組）。系統匣、上一次開機、甚至
 *   使用者自己去刪檔案，都能改變它。
 * - `masterStopPhase` 是 capture／brain／hands 共用的最重開關。它分開 clear、
 *   stopping、stopped、uncertain；後三種都壓過 paused，但只有 stopped 能說
 *   「三層都停了」。
 *
 * 混成一個變數的話會出現一個很難發現的 bug：暫停中問一句話 → 進 thinking →
 * 答完回 idle → **暫停的樣子不見了，但她其實還在暫停**。
 */
let state = "idle";
let paused = false;
let masterStopPhase = "checking";
// Event 是比已送出 poll 更新的 observation；request id 則讓兩份重疊 poll 只有
// 最後送出的那份能落地。兩個軸分開，否則舊 false 可以蓋掉剛收到的全停 true。
let masterStopRevision = 0;
let masterStopReadRequest = 0;
let gatekeeperReadRequest = 0;

// Persona 是表達層，不是上面的錄製狀態。關掉她、換顏色或停動畫都不可以改
// `state` / `paused`，也不可以走 ask、Gatekeeper、hands 或 CLI。
let activeProfile = PERSONA_CATALOG.chatgpt;
let personaEnabled = true;
let personaMotion = true;
let personaTapLines = true;
/** 上一句講過的 lineId。見 `pickBanter`：不連兩次同一句。 */
let lastSpokenLineId = null;
let personaVoiceEnabled = false;
let voiceRequest = 0;
let personaRevision = 0;
// Desktop 開場先用 HTML 的 ChatGPT WebP 當可見 fallback；native persona_read 回來前
// 還不知道真正 active ID，不能先把 25 張 ChatGPT PNG 解碼、隨即又整組丟掉。
let personaSelectionKnown = invoke === null;
let personaReelRevision = 0;
let personaReelLoadingId = null;
let personaReelReadyId = null;
let personaReelPendingImages = [];
const personaReelFailedIds = new Set();

function fallbackLocalAssets() {
  return Object.freeze({ phase: "unavailable", voiceLineIds: new Set() });
}

let localAssets = fallbackLocalAssets();

/**
 * Renderer 只接受後端已驗過的 fixed-voice availability。17 張角色圖隨程式提供，
 * 不讀 cache，也不會因下載、損毀或移除舊素材包而退回字母。
 *
 * `phase` 刻意保留 fixed-pack 的六態（含未提供與刪除中）。遠端 URL／任意路徑
 * 仍過不了這裡。開場不拿 WAV bytes；voice 只回可用
 * line ID，等 trusted click 當下再向 Rust 取那一條。顯示台詞的版本 ID 與公開素材
 * manifest ID 是兩個命名空間，不能混成一格，更不能拿語音路徑朗讀私人文字。
 */
function resolveLocalAssets(view, persona) {
  const phase = [
    "unavailable",
    "available",
    "installing",
    "removing",
    "repair-needed",
    "installed",
  ].includes(view?.phase)
    ? view.phase
    : "unavailable";
  if (phase !== "installed") {
    return Object.freeze({ phase, voiceLineIds: new Set() });
  }
  const allowed = new Set(
    persona.taps.map((line) => line.voiceLineId).filter((lineId) => lineId !== null),
  );
  const voiceLineIds = new Set();
  for (const voice of Array.isArray(view?.voice_lines) ? view.voice_lines : []) {
    const tap = persona.taps.find((line) => line.voiceLineId === voice?.line_id);
    if (
      allowed.has(voice?.line_id) &&
      Number.isInteger(voice?.duration_ms) &&
      voice?.spoken_text === tap?.text
    ) {
      voiceLineIds.add(voice.line_id);
    }
  }
  return Object.freeze({ phase, voiceLineIds });
}

function clearPersonaLine() {
  if (personaLine) {
    personaLine.textContent = "";
    personaLine.hidden = true;
  }
  stopPersonaMedia();
}

/**
 * Reel manifest 是 index.html 在 app.js 前載入的純本機資料。這裡不 fetch，也不把
 * manifest 裡的字串直接當 URL：只有目前 typed persona 的 exact 目錄、單層 PNG
 * basename 能被固定接到 frontendDist 的 `./persona-reels/` 下面。
 */
function personaReelRig(id) {
  const manifest = globalThis.__AI_SISTER_PERSONA_REELS__;
  if (
    manifest?.schema !== "ai-sister/persona-reels/v1" ||
    manifest?.rights_review !== "approved-owner-grant" ||
    manifest?.notice !== "NOTICE.md" ||
    manifest?.format?.kind !== "layered-png" ||
    manifest?.format?.canvas_contract !== "see_through_center_pad_v1" ||
    manifest?.format?.png !== "rgba8-noninterlaced" ||
    !Array.isArray(manifest?.rigs)
  ) {
    return null;
  }
  const matching = manifest.rigs.filter((rig) => rig?.id === id);
  if (matching.length !== 1) return null;
  const rig = matching[0];
  if (
    rig?.theme !== "workplace" ||
    rig?.canvas?.width !== 1280 ||
    rig?.canvas?.height !== 1280 ||
    rig?.viewport?.x !== 0 ||
    rig?.viewport?.y !== 0 ||
    rig?.viewport?.width !== 1280 ||
    rig?.viewport?.height !== 1280 ||
    !/^[0-9a-f]{64}$/u.test(rig?.canvas_sha256 ?? "") ||
    !Array.isArray(rig?.layers) ||
    rig.layers.length < 21 ||
    rig.layers.length > 26
  ) {
    return null;
  }

  const tags = new Set();
  const files = new Set();
  const renderOrder = new Set();
  const prefix = `rigs/${id}/`;
  for (const [index, layer] of rig.layers.entries()) {
    const basename = typeof layer?.file === "string" ? layer.file.slice(prefix.length) : "";
    const integers = [
      layer?.z,
      layer?.render_z,
      layer?.x,
      layer?.y,
      layer?.width,
      layer?.height,
      layer?.center_x,
      layer?.center_y,
      layer?.bytes,
    ];
    if (
      !integers.every(Number.isSafeInteger) ||
      layer.z !== index ||
      layer.render_z < 0 ||
      layer.render_z >= rig.layers.length ||
      renderOrder.has(layer.render_z) ||
      layer.x < 0 ||
      layer.y < 0 ||
      layer.width < 1 ||
      layer.height < 1 ||
      layer.x + layer.width > 1280 ||
      layer.y + layer.height > 1280 ||
      layer.center_x < layer.x ||
      layer.center_x > layer.x + layer.width ||
      layer.center_y < layer.y ||
      layer.center_y > layer.y + layer.height ||
      layer.bytes < 1 ||
      layer.bytes > 8 * 1024 * 1024 ||
      !/^[a-z][a-z0-9_]*$/u.test(layer?.tag ?? "") ||
      !["body", "eye", "brow", "mouth"].includes(layer?.role) ||
      layer.role !== personaReelLayerRole(layer.tag) ||
      tags.has(layer.tag) ||
      typeof layer.file !== "string" ||
      !layer.file.startsWith(prefix) ||
      !/^[A-Za-z0-9][A-Za-z0-9._-]*\.png$/u.test(basename) ||
      layer.file !== `${prefix}${basename}` ||
      files.has(layer.file) ||
      !/^[0-9a-f]{64}$/u.test(layer?.sha256 ?? "")
    ) {
      return null;
    }
    tags.add(layer.tag);
    files.add(layer.file);
    renderOrder.add(layer.render_z);
  }
  if (
    ![
      "face",
      "mouth",
      "eyewhite_l",
      "eyewhite_r",
      "irides_l",
      "irides_r",
      "eyelash_l",
      "eyelash_r",
      "eyebrow_l",
      "eyebrow_r",
      "source_residual",
    ].every((tag) => tags.has(tag))
  ) {
    return null;
  }
  return rig;
}

function personaReelLayerRole(tag) {
  if (tag.includes("mouth")) return "mouth";
  if (tag.includes("brow")) return "brow";
  if (
    (tag.includes("eye") && !tag.includes("wear")) ||
    tag.includes("irid") ||
    tag.includes("lash") ||
    tag.includes("pupil")
  ) {
    return "eye";
  }
  return "body";
}

function resetPersonaReel() {
  personaReelRevision += 1;
  personaReelLoadingId = null;
  personaReelReadyId = null;
  disposePersonaReelImages(personaReelPendingImages);
  personaReelPendingImages = [];
  disposePersonaReelImages(personaReel?.children ?? []);
  personaReel?.replaceChildren?.();
  if (personaReel) personaReel.hidden = true;
  avatar.classList.remove("reel-ready");
  delete avatar.dataset.reel;
}

function disposePersonaReelImages(images) {
  for (const image of images) {
    image.onload = null;
    image.onerror = null;
    image.removeAttribute?.("src");
  }
}

function reelLayerImage(layer, viewport) {
  const image = document.createElement("img");
  image.className = "persona-reel-layer";
  image.alt = "";
  image.draggable = false;
  image.decoding = "async";
  image.dataset.reelTag = layer.tag;
  image.dataset.reelRole = layer.role;
  if (layer.role === "eye") image.dataset.reelEye = "";
  image.style.left = `${(((layer.x - viewport.x) / viewport.width) * 100).toFixed(5)}%`;
  image.style.top = `${(((layer.y - viewport.y) / viewport.height) * 100).toFixed(5)}%`;
  image.style.width = `${((layer.width / viewport.width) * 100).toFixed(5)}%`;
  image.style.height = `${((layer.height / viewport.height) * 100).toFixed(5)}%`;
  image.style.zIndex = String(layer.render_z);
  image.style.transformOrigin = `${(((layer.center_x - layer.x) / layer.width) * 100).toFixed(3)}% ${(((layer.center_y - layer.y) / layer.height) * 100).toFixed(3)}%`;

  const ready = new Promise((resolve, reject) => {
    let settled = false;
    const fail = () => {
      if (settled) return;
      settled = true;
      reject(new Error("persona reel layer unavailable"));
    };
    image.onerror = fail;
    image.onload = async () => {
      if (settled) return;
      try {
        if (typeof image.decode !== "function") throw new Error("decode unavailable");
        await image.decode();
        if (image.naturalWidth !== layer.width || image.naturalHeight !== layer.height) {
          throw new Error("persona reel layer dimensions changed");
        }
        settled = true;
        image.onload = null;
        image.onerror = null;
        resolve();
      } catch {
        fail();
      }
    };
    image.src = `./persona-reels/${layer.file}`;
  });
  return { image, ready };
}

function loadPersonaReel(id) {
  if (!personaReel || personaReelFailedIds.has(id)) return;
  const rig = personaReelRig(id);
  if (rig === null) return;
  const revision = ++personaReelRevision;
  personaReelLoadingId = id;
  const pending = rig.layers.map((layer) => reelLayerImage(layer, rig.viewport));
  personaReelPendingImages = pending.map(({ image }) => image);
  Promise.all(pending.map(({ ready }) => ready)).then(
    () => {
      if (
        revision !== personaReelRevision ||
        personaReelLoadingId !== id ||
        activeProfile.id !== id ||
        !personaEnabled
      ) {
        disposePersonaReelImages(pending.map(({ image }) => image));
        return;
      }
      personaReelPendingImages = [];
      personaReel.replaceChildren(...pending.map(({ image }) => image));
      personaReelLoadingId = null;
      personaReelReadyId = id;
      personaReel.hidden = false;
      personaPortrait.hidden = false;
      avatar.classList.add("reel-ready");
      avatar.dataset.reel = "ready";
    },
    () => {
      disposePersonaReelImages(pending.map(({ image }) => image));
      if (revision !== personaReelRevision || personaReelLoadingId !== id) return;
      personaReelFailedIds.add(id);
      resetPersonaReel();
      personaPortrait.hidden = !personaEnabled;
    },
  );
}

function paintPersonaPortrait() {
  if (!personaPortrait) return;
  personaPortrait.alt = `${activeProfile.alias} 角色圖`;
  if (personaPortrait.src !== activeProfile.portrait) {
    personaPortrait.src = activeProfile.portrait;
  }
  personaPortrait.hidden = !personaEnabled;
  avatar.classList.add("has-portrait");
  if (!personaSelectionKnown) {
    resetPersonaReel();
    return;
  }
  if (!personaEnabled) {
    resetPersonaReel();
    return;
  }
  if (personaReelReadyId === activeProfile.id) {
    personaPortrait.hidden = false;
    if (personaReel) personaReel.hidden = false;
    avatar.classList.add("reel-ready");
    return;
  }
  if (personaReelLoadingId === activeProfile.id) return;
  resetPersonaReel();
  loadPersonaReel(activeProfile.id);
}

function applyPersona(view) {
  const requestedProfile = profileFor(view?.id);
  // Rust 會拒絕未知 ID；renderer 仍要自己守住 IPC seam。不能一邊保留舊角色，
  // 一邊把同一份壞 payload 的 voice/tap/asset 欄位套上去——那會讓一個未知身分
  // 替已知角色打開聲音。整份拒絕、停掉聲音，等下一份完整有效的設定。
  if (requestedProfile === null) {
    personaVoiceEnabled = false;
    localAssets = fallbackLocalAssets();
    clearPersonaLine();
    avatar.dataset.assetPack = localAssets.phase;
    return false;
  }
  activeProfile = requestedProfile;
  // 缺 bool 欄位只代表舊後端／瀏覽器 demo；聲音仍只接受 exact true。
  personaEnabled = view?.enabled !== false;
  personaMotion = view?.motion !== false;
  personaTapLines = view?.tap_lines !== false;
  personaVoiceEnabled = view?.voice_enabled === true;
  lastSpokenLineId = null;
  localAssets = resolveLocalAssets(view?.asset_pack, activeProfile);

  // Persona 只改 avatar 自己的三色。`--letter-*` 是整個淺色 stage 的 UI theme；
  // 把深底角色的白色 foreground 寫進那組變數，會連錄製狀態、答案與安全卡片
  // 一起變成淺底白字。角色外觀不能改壞核心 UI。
  document.documentElement.style.setProperty("--persona-bg", activeProfile.palette.background);
  document.documentElement.style.setProperty("--persona-fg", activeProfile.palette.foreground);
  document.documentElement.style.setProperty("--persona-accent", activeProfile.palette.accent);
  avatar.dataset.persona = activeProfile.id;
  avatar.dataset.assetPack = localAssets.phase;
  avatar.title = `${activeProfile.alias}：${activeProfile.tagline}`;
  avatar.hidden = !personaEnabled;
  avatar.disabled = !personaEnabled || !personaTapLines;
  clearPersonaLine();
  paintPersonaPortrait();
  if (consentGuideSheet !== null && consentListen) {
    const clip = consentClipFor(consentGuideSheet);
    consentListen.disabled = clip === null;
    consentListen.title =
      clip === null ? "這一版條文沒有相符的本機錄音" : "用目前角色的聲音朗讀";
  }
  updateMotionGate();
  paint();
  return true;
}

let localSystemVoices = [];
let localSpeechRevision = 0;
let localSpeechPresentation = null;
let bundledVoicePresentation = null;
let azureSpeechRevision = 0;
let azureSpeechRequestPending = false;
let azureSpeechEnabled = false;
let azureSpeechReady = false;
let azureStatusKnown = false;
let azureAnswerLine = null;
let azureAnswerButton = null;
let azureStatusReadRevision = 0;
let azureNativeGeneration = null;
let azureNativeExpected = null;
let azurePendingGeneration = null;
let azureCancelPending = false;
let pendingAzureAutoAsk = null;
let azurePlaybackPresentation = null;

// 一個是第四張同意 + 設定開關授權的新答案，一個是使用者當下按的
// 重播。用物件 identity，不讓一個拼錯的字串想當哪一種就當哪一種。
const AZURE_AUTO_ANSWER = Object.freeze({});
const AZURE_TRUSTED_REPLAY = Object.freeze({});

// 說話微動只跟著「已經開始播放」的那條聲音走，不跟 request、答案完成或 thinking
// 狀態走。owner identity 讓舊 utterance/audio 的晚 end 不能清掉後來的新聲音。
const PERSONA_SPEAKING_FIXED = Object.freeze({});
const PERSONA_SPEAKING_LOCAL = Object.freeze({});
const PERSONA_SPEAKING_AZURE = Object.freeze({});
const PERSONA_SPEAKING_OWNERS = Object.freeze([
  PERSONA_SPEAKING_FIXED,
  PERSONA_SPEAKING_LOCAL,
  PERSONA_SPEAKING_AZURE,
]);
let personaSpeakingOwner = null;

function setPersonaSpeaking(owner, speaking) {
  if (!PERSONA_SPEAKING_OWNERS.includes(owner)) return;
  if (speaking) {
    personaSpeakingOwner = owner;
  } else if (personaSpeakingOwner === owner) {
    personaSpeakingOwner = null;
  }
  avatar.classList.toggle("speaking", personaSpeakingOwner !== null);
}

function clearPersonaSpeaking() {
  personaSpeakingOwner = null;
  avatar.classList.remove("speaking");
}

function resetAzureAnswerButton() {
  if (azureAnswerButton) {
    azureAnswerButton.disabled = false;
    azureAnswerButton.textContent = "☁ 用 Azure 朗讀／重播（送出這段文字）";
  }
  azureAnswerButton = null;
}

/**
 * 取消的是播放意圖：晚到的 native response 會被丟掉，已經開始的 blocking HTTPS
 * POST 可能仍跑到 timeout。只有真的有 request 在飛時才送 cancel IPC；冷啟動與
 * persona 重畫不會因此製造一個假的「使用者按過停止」。
 */
function stopAzureSpeech({ cancelNative = true } = {}) {
  azureSpeechRevision += 1;
  setPersonaSpeaking(PERSONA_SPEAKING_AZURE, false);
  const hadPendingRequest = azureSpeechRequestPending;
  const pendingGeneration = azurePendingGeneration;
  const hadAzureMedia = hadPendingRequest || azureAnswerButton !== null;
  azureSpeechRequestPending = false;
  azurePendingGeneration = null;
  resetAzureAnswerButton();
  if (hadAzureMedia && personaAudio) {
    personaAudio.pause?.();
    personaAudio.removeAttribute?.("src");
    personaAudio.onended = null;
    personaAudio.onerror = null;
  }
  // Native master-stop activity guard 跨完整播放；先確實停掉本機 media，再交還
  // presentation lease。外部 CLI stop 沒有 Tauri event，五秒 poll 走到這裡時
  // stop-all 會等這個 end，而不會先回成功、聲音才在後面繼續。
  releaseAzurePlaybackPresentation();
  if (
    cancelNative &&
    hadPendingRequest &&
    Number.isSafeInteger(pendingGeneration) &&
    pendingGeneration >= 0 &&
    invoke !== null
  ) {
    // 先拿掉可重用的 native token。A 的 cancel 是 fire-and-forget；在它回來、重讀
    // 新 generation 前，不能讓快速第二按 B 帶著 A 的 token 排進去。
    azureNativeGeneration = null;
    azureNativeExpected = null;
    azureCancelPending = true;
    // 讓取消前已送出的 status read 全部過期。Cancel settle 以前，任何 event/read
    // 都不能把同一代 token 填回來，否則快速連點會讓延遲的 A cancel 誤殺 B。
    azureStatusReadRevision += 1;
    const finishCancel = () => {
      azureCancelPending = false;
      readAzureTts();
    };
    try {
      Promise.resolve(
        invoke("azure_tts_cancel", { expectedGeneration: pendingGeneration }),
      ).then(finishCancel, finishCancel);
    } catch {
      finishCancel();
    }
  } else if (!cancelNative) {
    // native mutation/revoke 已經先 bump；不要再排一個全新的 cancel。只把 renderer
    // token 作廢並在下面補讀，避免事件後的第一個 click 還拿舊 generation。
    azureNativeGeneration = null;
    azureNativeExpected = null;
    azureSpeechReady = false;
    azureStatusKnown = false;
  }
}

function stopLocalSpeech() {
  localSpeechRevision += 1;
  globalThis.speechSynthesis?.cancel?.();
  setPersonaSpeaking(PERSONA_SPEAKING_LOCAL, false);
  if (localSpeechPresentation !== null) {
    const presentation = localSpeechPresentation;
    localSpeechPresentation = null;
    releaseNativePresentation(presentation);
  }
}

/** 三條聲音共用同一顆 stop；新意圖不能讓 bundled Ogg、系統 TTS 與 Azure 疊在一起。 */
function stopPersonaMedia({ cancelAzureNative = true } = {}) {
  stopAzureSpeech({ cancelNative: cancelAzureNative });
  voiceRequest += 1;
  personaAudio?.pause?.();
  personaAudio?.removeAttribute?.("src");
  if (personaAudio) {
    personaAudio.onended = null;
    personaAudio.onerror = null;
  }
  if (bundledVoicePresentation !== null) {
    const presentation = bundledVoicePresentation;
    bundledVoicePresentation = null;
    releaseNativePresentation(presentation);
  }
  stopLocalSpeech();
  clearPersonaSpeaking();
}

/**
 * 借 AIRI speech pipeline 的「先分段、依序播放、整串可取消」形狀，但保留零依賴。
 * 讓長答案不用等整段中文先合成完才出第一句，也避開 Windows voice 對超長 utterance
 * 的不同行為。這裡只切已經在畫面上的文字，不接 streaming 模型或遠端 provider。
 */
function chunkLocalSpeech(text, limit = 160) {
  const sentences = String(text).match(/[^。！？!?；;\n]+[。！？!?；;\n]?/gu) ?? [];
  const chunks = [];
  let current = "";
  for (const raw of sentences) {
    const sentence = raw.trim();
    if (sentence === "") continue;
    if (current !== "" && current.length + sentence.length > limit) {
      chunks.push(current);
      current = "";
    }
    if (sentence.length <= limit) {
      current += sentence;
      continue;
    }
    if (current !== "") chunks.push(current);
    current = "";
    for (let start = 0; start < sentence.length; start += limit) {
      chunks.push(sentence.slice(start, start + limit));
    }
  }
  if (current !== "") chunks.push(current);
  return chunks;
}

function refreshLocalSystemVoices() {
  const synth = globalThis.speechSynthesis;
  if (!synth || typeof synth.getVoices !== "function") {
    localSystemVoices = [];
    return;
  }
  localSystemVoices = synth
    .getVoices()
    .filter((voice) => voice?.localService === true)
    .map((voice) => {
      const lang = String(voice.lang ?? "").toLowerCase();
      const score = lang === "zh-tw" ? 3 : lang.startsWith("zh-hant") ? 2 : lang.startsWith("zh") ? 1 : 0;
      return { voice, score };
    })
    .filter(({ score }) => score > 0)
    .sort((a, b) => b.score - a.score)
    .map(({ voice }) => voice);
}

refreshLocalSystemVoices();
globalThis.speechSynthesis?.addEventListener?.("voiceschanged", refreshLocalSystemVoices);

/**
 * 只接受瀏覽器明確標成 `localService` 的繁中／中文聲音。找不到就保持安靜；絕不
 * 因為系統 voice 缺席而選 remote voice。呼叫端必須仍在 trusted click 那條路上。
 */
function speakWithLocalSystemVoice(text, presentation = null) {
  if (!personaVoiceEnabled || typeof globalThis.SpeechSynthesisUtterance !== "function") {
    return false;
  }
  refreshLocalSystemVoices();
  if (localSystemVoices.length === 0) return false;
  const synth = globalThis.speechSynthesis;
  const offset = Object.keys(PERSONA_CATALOG).indexOf(activeProfile.id);
  const voice = localSystemVoices[offset % localSystemVoices.length];
  const rate = activeProfile.voiceRate;
  const pitch = activeProfile.voicePitch;
  const chunks = chunkLocalSpeech(text);
  if (chunks.length === 0) return false;
  // 答案朗讀要停掉 bundled Ogg，也要讓 pending 固定語音失效。共用 stop 後才拿
  // revision，這一串才是目前唯一可繼續的播放意圖。
  stopPersonaMedia();
  localSpeechPresentation = presentation;
  const revision = localSpeechRevision;
  let next = 0;
  const speakNext = () => {
    if (revision !== localSpeechRevision) return;
    if (next >= chunks.length) {
      setPersonaSpeaking(PERSONA_SPEAKING_LOCAL, false);
      if (localSpeechPresentation === presentation) {
        localSpeechPresentation = null;
        releaseNativePresentation(presentation);
      }
      return;
    }
    const utterance = new globalThis.SpeechSynthesisUtterance(chunks[next]);
    next += 1;
    utterance.voice = voice;
    utterance.lang = voice.lang;
    utterance.rate = rate;
    utterance.pitch = pitch;
    utterance.onstart = () => {
      if (revision === localSpeechRevision) {
        setPersonaSpeaking(PERSONA_SPEAKING_LOCAL, true);
      }
    };
    utterance.onend = speakNext;
    utterance.onerror = () => {
      if (revision !== localSpeechRevision) return;
      localSpeechRevision += 1;
      setPersonaSpeaking(PERSONA_SPEAKING_LOCAL, false);
      if (localSpeechPresentation === presentation) {
        localSpeechPresentation = null;
        releaseNativePresentation(presentation);
      }
      personaLine.textContent = "本機朗讀失敗。再按一次重播。";
      personaLine.hidden = false;
    };
    try {
      synth.speak(utterance);
    } catch {
      utterance.onerror?.();
    }
  };
  speakNext();
  return true;
}

/*
 * 隨機挑一句，而且不會連兩次同一句。
 *
 * 舊版是 `taps[nextTap++ % taps.length]`，而那時候整包只有兩句——所以「按來按去
 * 只會一直講兩句話」不是感覺，是規格寫死的。句子變多之後順序也不該再是固定的：
 * 一個照順序輪的東西，按到第三下就露餡。
 *
 * 「不連兩次同一句」比「純隨機」重要。純隨機在十二句裡連兩次的機率是十二分之一，
 * 一分鐘按十下就會遇到一次，而那一下讀起來就是她壞掉了。
 */

function pickBanter(clips) {
  if (clips.length === 0) return null;
  const pool = clips.filter((clip) => clip.lineId !== lastSpokenLineId);
  const choices = pool.length > 0 ? pool : clips;
  return choices[Math.floor(Math.random() * choices.length)] ?? choices[0];
}

/** 被戳之後那一下抖動有多久。CSS 的 `poke-jolt` 是同一個數字。 */
const POKE_JOLT_MS = 460;
let pokeJoltTimer = null;

/*
 * 戳一下，她扭一下。
 *
 * 動畫掛在 `.avatar` 上而不是 `.face`：`.face` 的 `transform` 是呼吸、`rotate`
 * 是搖晃，兩個通道都有人了，第三段疊上去會把前兩段整組換掉（`animation` 是同一
 * 個屬性）。`.avatar` 本身沒有動畫，而 `scale`／`translate` 是獨立屬性，不會
 * 互相覆蓋。
 *
 * 動畫期間 `getBoundingClientRect()` 會跟著變，而剪影是照那個框算的——最多偏
 * 六個百分點，`FIGURE_DILATE_PX`（16px）吃得下，而且動畫結束再推一次就準了。
 *
 * 清 class 用計時器不用 `animationend`：`prefers-reduced-motion` 之下那段動畫
 * 根本不會跑，`animationend` 永遠不來，class 就會黏在上面。
 */
function joltHer() {
  if (avatar === null) return;
  // 要重播同一段動畫，得先讓瀏覽器看到一次「沒有這個 class」的樣子。少了中間
  // 那一次 reflow，連按兩下的第二下不會動。
  avatar.classList.remove("poked");
  void avatar.offsetWidth;
  avatar.classList.add("poked");
  if (pokeJoltTimer !== null) clearTimeout(pokeJoltTimer);
  pokeJoltTimer = setTimeout(() => {
    pokeJoltTimer = null;
    avatar.classList.remove("poked");
  }, POKE_JOLT_MS);
}

async function sayPersonaLine(event) {
  // `.click()` / `dispatchEvent()` 也能走進同一個 DOM handler，但那不是「使用者
  // 當下操作」。鍵盤在原生 button 上產生的 click 仍是 trusted，所以 Enter／Space
  // 可用；程式合成的事件則連文字都不說，更不可能沿這條路取得語音播放權。
  if (event?.isTrusted !== true) return;
  if (!personaEnabled) return;
  // 抖那一下不看 `personaTapLines`。那個開關管的是「她說不說話」，不是「她理不
  // 理你」——戳下去整個人一動也不動，那不是安靜，那是當掉。
  joltHer();
  if (!personaTapLines) return;
  const line = pickBanter(activeProfile.taps);
  if (line === null) return;
  lastSpokenLineId = line.lineId;
  personaLine.textContent = line.text;
  personaLine.hidden = false;

  stopPersonaMedia();
  if (personaVoiceEnabled) void playBundledPersonaLine(line);
}

/**
 * 這段錄音真的是這個角色的、而且真的來自我們自己驗過的那兩包嗎。
 *
 * 比的是**物件本身**不是 lineId：外面遞進來一個長得很像的字面量也過不了。
 * 日常那包和閒話那包各查一次；兩包的 lineId 不重疊，但就算重疊，`=== line`
 * 也讓它無害。
 */
function bundledClipOf(personaId, line) {
  if (line === null || line === undefined) return null;
  for (const library of [DIALOGUE_VOICES, BANTER_VOICES]) {
    if (library.byPersona.get(personaId)?.get(line.lineId) === line) return line;
  }
  return null;
}

async function playBundledPersonaLine(line) {
  if (
    !personaVoiceEnabled ||
    line?.persona !== activeProfile.id ||
    bundledClipOf(activeProfile.id, line) === null ||
    invoke === null ||
    !personaAudio ||
    typeof personaAudio.play !== "function"
  ) {
    return false;
  }
  const request = voiceRequest;
  let presentation;
  try {
    presentation = await invoke("persona_fixed_voice_admit");
    const allowed = await beginNativePresentation(presentation);
    if (
      !allowed ||
      request !== voiceRequest ||
      line.persona !== activeProfile.id ||
      masterStopPhase !== "clear"
    ) {
      releaseNativePresentation(presentation);
      return false;
    }
  } catch {
    releaseNativePresentation(presentation);
    return false;
  }
  bundledVoicePresentation = presentation;
  personaAudio.currentTime = 0;
  let playbackFinished = false;
  const finishPlayback = () => {
    if (playbackFinished) return;
    playbackFinished = true;
    if (request === voiceRequest) setPersonaSpeaking(PERSONA_SPEAKING_FIXED, false);
    personaAudio.onended = null;
    personaAudio.onerror = null;
    personaAudio.removeAttribute?.("src");
    if (bundledVoicePresentation === presentation) bundledVoicePresentation = null;
    releaseNativePresentation(presentation);
  };
  personaAudio.onended = finishPlayback;
  personaAudio.onerror = () => {
    finishPlayback();
    if (request === voiceRequest) {
      personaLine.textContent = "語音播放失敗。再按一次重播。";
      personaLine.hidden = false;
    }
  };
  personaAudio.src = line.file;
  try {
    await personaAudio.play();
    if (request === voiceRequest && !playbackFinished) {
      setPersonaSpeaking(PERSONA_SPEAKING_FIXED, true);
      return true;
    }
  } catch {
    personaAudio.onerror?.();
  }
  return false;
}

/* ---------- 沒事的時候，和答案落地的那一刻 ---------- */

/*
 * 這一段做的是 Ted 說的那件事：「其實出的語音不見得要跟回答的答案一樣，
 * 語音是情境用的，回答歸回答。」
 *
 * 所以閒話那一包有兩個出口和答案完全無關：
 *   - `idle-giggle`：沒事的時候自己笑一下。
 *   - `answer-beat`：答案落地那一刻的一聲墊話（「找到了。」），文字答案照舊
 *     用讀的，不是用念的。
 *
 * **這改了一條寫在 PRODUCT.md 上的界線。** 舊版寫「聲音不因 idle、capture、
 * 記憶或系統事件自己播放」，而「沒事也可以呵呵嘻嘻笑幾下」正是那條禁止的事。
 * 文件已經跟著改（PRODUCT.md／PHASES.md），不是偷偷放寬。真正還在守的是這些：
 * 全停、`personaEnabled`／`personaTapLines`、靜音、視窗看不見的時候不出聲，
 * 以及「一次只有一個聲音」。
 */

/** 兩次自己笑之間的隨機間隔。固定週期的東西兩次之後就變成節拍器。 */
const IDLE_GIGGLE_MIN_MS = 120_000;
const IDLE_GIGGLE_MAX_MS = 300_000;
/** 自己笑那一句在畫面上留多久。戳出來的台詞不清，這一句要清。 */
const IDLE_GIGGLE_LINGER_MS = 6_000;
let idleGiggleTimer = null;
let idleGiggleClearTimer = null;

/** 現在適不適合出聲。任何一條不成立就跳過這一輪，不重排也不補。 */
function idleEnoughToGiggle() {
  return (
    personaEnabled &&
    personaTapLines &&
    document.visibilityState === "visible" &&
    state === "idle" &&
    !paused &&
    masterStopPhase === "clear" &&
    consentGuideSheet === null &&
    bundledVoicePresentation === null &&
    // 他正在打字。這時候插一句「嘻嘻」不是陪伴，是打斷。
    document.activeElement !== askInput
  );
}

function scheduleIdleGiggle() {
  if (idleGiggleTimer !== null) clearTimeout(idleGiggleTimer);
  const span = IDLE_GIGGLE_MAX_MS - IDLE_GIGGLE_MIN_MS;
  idleGiggleTimer = setTimeout(
    () => {
      idleGiggleTimer = null;
      giggleIfNothingIsHappening();
      scheduleIdleGiggle();
    },
    IDLE_GIGGLE_MIN_MS + Math.floor(Math.random() * span),
  );
}

function giggleIfNothingIsHappening() {
  if (!idleEnoughToGiggle()) return;
  const line = pickBanter(activeProfile.giggles);
  if (line === null) return;
  lastSpokenLineId = line.lineId;
  personaLine.textContent = line.text;
  personaLine.hidden = false;
  if (idleGiggleClearTimer !== null) clearTimeout(idleGiggleClearTimer);
  idleGiggleClearTimer = setTimeout(() => {
    idleGiggleClearTimer = null;
    // 只清掉自己那一句。這段期間他要是戳了她，那一句是他要的，不要蓋掉。
    if (personaLine.textContent !== line.text) return;
    personaLine.textContent = "";
    personaLine.hidden = true;
  }, IDLE_GIGGLE_LINGER_MS);
  if (personaVoiceEnabled) void playBundledPersonaLine(line);
}

scheduleIdleGiggle();

/**
 * 答案落地那一刻的一聲。
 *
 * Azure 朗讀開著的時候不出這一聲——兩個聲音疊在一起，兩個都聽不清楚。
 * 那條路念的是答案本文，這一聲只是「找到了」，本來就該讓給它。
 */
function playAnswerBeat() {
  if (!personaEnabled || !personaVoiceEnabled || azureSpeechReady) return;
  if (masterStopPhase !== "clear") return;
  const line = pickBanter(activeProfile.beats);
  if (line === null) return;
  lastSpokenLineId = line.lineId;
  void playBundledPersonaLine(line);
}

// ---------- 主對話裡的四張同意書 ----------

let consentGuideView = null;
let consentGuideSheet = null;
let consentGuideBusy = false;
let pendingConsentQuestion = null;

function usableConsentView(raw) {
  if (
    raw === null ||
    typeof raw !== "object" ||
    !Array.isArray(raw.sheets) ||
    raw.sheets.length !== 4
  ) {
    return null;
  }
  for (const [index, sheet] of raw.sheets.entries()) {
    if (
      sheet?.key !== CONSENT_SHEETS[index] ||
      typeof sheet?.wording !== "string" ||
      sheet.wording.trim() === "" ||
      typeof sheet?.without !== "string" ||
      sheet.without.trim() === "" ||
      typeof sheet?.effective !== "boolean" ||
      typeof sheet?.reviewed !== "boolean" ||
      (sheet.granted_at !== null && !Number.isSafeInteger(sheet.granted_at))
    ) {
      return null;
    }
  }
  return raw;
}

function nextConsentSheet(view) {
  return view?.sheets?.find((sheet) => sheet.reviewed !== true) ?? null;
}

function consentClipFor(sheet) {
  const clip = CONSENT_VOICES.byPersona.get(activeProfile.id)?.get(sheet?.key) ?? null;
  // 顯示文字只信 native；錄音逐字稿不同就不播，不能讓舊錄音替新條文說話。
  return clip?.text === sheet?.wording ? clip : null;
}

function setConsentGuideInput(enabled) {
  if (!askInput || !askSend) return;
  askInput.disabled = !enabled;
  askSend.disabled = !enabled;
  askInput.placeholder = consentGuideSheet
    ? "輸入「同意」或「不同意」…"
    : "問我一件事…";
}

function showConsentGuide(view) {
  consentGuideView = view;
  consentGuideSheet = nextConsentSheet(view);
  if (consentGuideSheet === null) return false;
  const index = view.sheets.indexOf(consentGuideSheet);
  consentProgress.textContent = `同意書 ${index + 1} / 4`;
  consentWording.textContent = consentGuideSheet.wording;
  consentWithout.textContent = consentGuideSheet.without;
  consentPrompt.textContent = "請在下面輸入「同意」或「不同意」。";
  consentResult.textContent = "";
  consentResult.classList.remove("bad");
  consentGuide.hidden = false;
  hitList.hidden = true;
  document.body.classList.remove("has-hits");
  document.body.classList.add("has-consent-guide");
  const clip = consentClipFor(consentGuideSheet);
  consentListen.disabled = clip === null;
  consentListen.title = clip === null ? "這一版條文沒有相符的本機錄音" : "用目前角色的聲音朗讀";
  setConsentGuideInput(!consentGuideBusy);
  paintConversation();
  return true;
}

function hideConsentGuide() {
  consentGuideView = null;
  consentGuideSheet = null;
  consentGuideBusy = false;
  consentGuide.hidden = true;
  document.body.classList.remove("has-consent-guide");
  setConsentGuideInput(true);
}

function showConsentCompletion(view) {
  const message = document.createElement("li");
  message.className = "persona-dialogue";
  message.textContent = view.allows_recording
    ? "四張都問完了。日後可從上方齒輪查看或更改；按「開始記錄」後，我才會開始看。"
    : "四張都問完了。沒有同意的功能維持關閉；日後可從上方齒輪查看或更改。";
  hitList.replaceChildren(message);
  hitList.hidden = false;
  document.body.classList.add("has-hits");
  showingAnswer = false;
  paintConversation();
}

async function finishConsentGuide(view) {
  const queued = pendingConsentQuestion;
  pendingConsentQuestion = null;
  hideConsentGuide();
  if (queued !== null) {
    askInput.value = queued;
    await ask();
    return;
  }
  showConsentCompletion(view);
}

async function handleConsentReply() {
  if (consentGuideSheet === null || consentGuideBusy || invoke === null) return;
  const reply = askInput.value
    .normalize("NFKC")
    .trim()
    .replace(/[。！？!?]+$/u, "")
    .trim();
  const granted = reply === "同意" || reply === "我同意";
  const declined = reply === "不同意" || reply === "我不同意" || reply === "先不要";
  if (!granted && !declined) {
    consentResult.textContent = "請只輸入「同意」或「不同意」；其他文字不會改動同意書。";
    consentResult.classList.add("bad");
    askInput.select?.();
    return;
  }

  stopPersonaMedia();
  const answeredKey = consentGuideSheet.key;
  consentGuideBusy = true;
  askInput.value = "";
  consentResult.textContent = "正在保存…";
  consentResult.classList.remove("bad");
  setConsentGuideInput(false);
  try {
    const next = usableConsentView(
      await invoke("consent_set", { key: answeredKey, granted }),
    );
    if (next === null) throw new Error("保存後沒有讀回完整的四張同意書");
    const answered = next.sheets.find((sheet) => sheet.key === answeredKey);
    if (
      answered?.reviewed !== true ||
      answered.effective !== granted ||
      (granted && answered.granted_at === null)
    ) {
      throw new Error("同意書保存結果和剛才的回答不同");
    }
    consentGuideBusy = false;
    if (!showConsentGuide(next)) await finishConsentGuide(next);
  } catch (error) {
    consentGuideBusy = false;
    consentResult.textContent = `這一張沒有保存：${String(error?.message ?? error)}`;
    consentResult.classList.add("bad");
    setConsentGuideInput(true);
    askInput.focus?.();
  }
}

async function readConsentGuide() {
  if (invoke === null) return;
  setConsentGuideInput(false);
  try {
    const view = usableConsentView(await invoke("consent_read"));
    if (view === null) throw new Error("沒有讀回完整的四張同意書");
    if (!showConsentGuide(view)) hideConsentGuide();
  } catch (error) {
    hideConsentGuide();
    noticeAboutSomethingElse(`同意書讀不到；這一輪不會替你做任何授權：${String(error?.message ?? error)}`);
    paint();
  }
}

async function playConsentSheet(event) {
  if (event?.isTrusted !== true || consentGuideSheet === null || consentGuideBusy) return;
  const clip = consentClipFor(consentGuideSheet);
  if (
    clip === null ||
    !personaAudio ||
    typeof personaAudio.play !== "function" ||
    invoke === null
  ) {
    return;
  }
  stopPersonaMedia();
  const request = voiceRequest;
  let presentation;
  try {
    presentation = await invoke("persona_fixed_voice_admit");
    const allowed = await beginNativePresentation(presentation);
    if (
      !allowed ||
      request !== voiceRequest ||
      clip !== consentClipFor(consentGuideSheet) ||
      masterStopPhase !== "clear"
    ) {
      releaseNativePresentation(presentation);
      return;
    }
  } catch (error) {
    releaseNativePresentation(presentation);
    consentResult.textContent = `現在不能朗讀：${String(error?.message ?? error)}`;
    consentResult.classList.add("bad");
    return;
  }
  bundledVoicePresentation = presentation;
  personaAudio.currentTime = 0;
  let finished = false;
  const finish = () => {
    if (finished) return;
    finished = true;
    if (request === voiceRequest) setPersonaSpeaking(PERSONA_SPEAKING_FIXED, false);
    personaAudio.onended = null;
    personaAudio.onerror = null;
    personaAudio.removeAttribute?.("src");
    if (bundledVoicePresentation === presentation) bundledVoicePresentation = null;
    releaseNativePresentation(presentation);
  };
  personaAudio.onended = finish;
  personaAudio.onerror = () => {
    finish();
    if (request === voiceRequest) {
      consentResult.textContent = "這段本機錄音播放失敗；可以直接讀文字後回答。";
      consentResult.classList.add("bad");
    }
  };
  personaAudio.src = clip.file;
  try {
    await personaAudio.play();
    if (request === voiceRequest && !finished) {
      setPersonaSpeaking(PERSONA_SPEAKING_FIXED, true);
    }
  } catch {
    personaAudio.onerror?.();
  }
}

consentListen?.addEventListener("click", (event) => void playConsentSheet(event));

/*
 * 把她拖來拖去。
 *
 * 這扇窗是 340×560、`decorations: false` 的透明視窗，所以「拖她」就是拖視窗；
 * 位置的記憶已經是免費的——`main.rs` 的 `WindowEvent::Moved` 會把座標寫進
 * pet state，重開時 `bounds::nudge_onto` 再把她拉回看得見的螢幕上。
 *
 * 不能直接在角色上掛 `data-tauri-drag-region`：那個屬性在 mousedown 當下就把
 * 事件交給作業系統，點一下會說話的那條路（`sayPersonaLine`）就再也不會發生。
 * 所以這裡自己分辨——**按下去之後移動超過門檻才算拖**，沒超過就還是戳她一下。
 *
 * `startDragging()` 一旦發動，作業系統接管整個拖曳迴圈，webview 通常收不到
 * 後續的 pointerup／click。所以旗標是在**下一次 pointerdown** 清掉的，不是在
 * pointerup：留著一個沒人清的 true，下一次真正的點擊就會被無聲吃掉。
 */
const DRAG_THRESHOLD_PX = 4;
const petWindow = globalThis.__TAURI__?.window?.getCurrentWindow?.() ?? null;
let dragOrigin = null;
let draggedThisPress = false;

function startWindowDrag() {
  avatar?.classList.add("dragging");
  // 沒有 native 的時候（瀏覽器裡開 demo）就只是不會動，不要炸掉整條 UI。
  Promise.resolve(petWindow?.startDragging?.()).catch(() => {
    avatar?.classList.remove("dragging");
  });
}

avatar?.addEventListener("pointerdown", (event) => {
  // 只收主鍵。右鍵和中鍵不是「抓住她」。
  if (event.button !== undefined && event.button !== 0) return;
  if (avatar.disabled) return;
  dragOrigin = { x: event.clientX ?? 0, y: event.clientY ?? 0 };
  draggedThisPress = false;
});

avatar?.addEventListener("pointermove", (event) => {
  if (dragOrigin === null || draggedThisPress) return;
  const dx = (event.clientX ?? 0) - dragOrigin.x;
  const dy = (event.clientY ?? 0) - dragOrigin.y;
  if (Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
  draggedThisPress = true;
  startWindowDrag();
});

avatar?.addEventListener("pointerup", () => {
  dragOrigin = null;
  avatar?.classList.remove("dragging");
});

avatar?.addEventListener("pointercancel", () => {
  dragOrigin = null;
  draggedThisPress = false;
  avatar?.classList.remove("dragging");
});

avatar?.addEventListener("click", (event) => {
  // 拖完手放開，瀏覽器仍可能補一個 click。那一下是「我剛把她搬到這裡」，
  // 不是「我想聽她說話」。
  if (draggedThisPress) {
    event?.preventDefault?.();
    return;
  }
  // 事件要原封不動傳下去：`sayPersonaLine` 用 `event.isTrusted` 擋掉程式合成的
  // 點擊，那是「角色台詞只在真人點擊後出聲」那條產品規則的唯一守衛。這裡少寫
  // 一個參數，等於把它整個關掉。
  return sayPersonaLine(event);
});

/* ---------- 讓看不見的地方點得過去 ---------- */

/*
 * 這扇窗 340×560、整片透明，可是作業系統是照**整個矩形**做命中判定的：她頭頂
 * 上方那塊一百多像素高、什麼都沒畫的帶狀區域仍然會把點擊吃掉，底下的視窗收不
 * 到。對一個永遠置頂、整天掛在角落的東西來說，那是一塊跟著她移動的隱形擋板。
 *
 * Rust 那邊用 `set_ignore_cursor_events` 翻整扇窗的開關，但「現在哪裡是實心的」
 * 只有這裡知道——泡泡冒出來、換角色、輸入列長高，每一件都會改。所以真相從這邊
 * 推過去。
 *
 * 兩條原則：
 *
 * - **寧可多報實心。** 兩邊壞掉的代價不對稱：多報一塊只是多擋一點桌面，使用者
 *   看得見自己在點什麼；少報一塊是畫面上明明有她、滑鼠卻穿過去，那看起來就是
 *   當掉了（系統匣救得回來，但那要先想到是這扇窗的問題）。所以「有沒有畫東西」用的是
 *   一個寬鬆的規則（底色、背景圖、邊框，或本來就是控制項），而且每一塊都再往
 *   外撐 `SOLID_DILATE_PX`，蓋住文字陰影和反鋸齒。
 * - **不要每一幅都算。** 她一直在呼吸和搖晃（`breathe` + `sway`），拿她當下的
 *   外框去量會變成 60fps 的 IPC。所以量的是 `.avatar` 這個**不動**的座標格，
 *   剪影再往外撐一圈蓋住動畫走得到的範圍。
 */

/** 文字陰影、反鋸齒、focus ring 都會畫到 `getBoundingClientRect()` 外面一點。 */
const SOLID_DILATE_PX = 2;

/*
 * 剪影往外撐多少。動畫最遠走到哪是算得出來的，不是猜的：
 * `breathe` 往上 6px、放大 1.015（半徑 150px ⇒ 2.3px），`sway` 轉 ±2.2°
 * （150 × 2.2° 的弧度 ⇒ 5.8px）。加起來約 14px，取 16 留一點餘裕。
 */
const FIGURE_DILATE_PX = 16;

/** 剪影取樣的格子大小。300px 的框切成 4px 一格＝75×75，夠細也夠便宜。 */
const FIGURE_CELL_PX = 4;

/** 這些本來就是要給人點的，就算它自己沒有底色也算實心。 */
const SOLID_TAGS = new Set(["BUTTON", "INPUT", "TEXTAREA", "SELECT", "A", "AUDIO"]);

let solidPushTimer = null;
let figureMask = null;
let solidWholeWindow = false;

/** `rgba(…)` 的第四個數字；`rgb(…)` 沒有第四個就是不透明。 */
function alphaOf(color) {
  if (typeof color !== "string" || color === "transparent") return 0;
  const parts = color.match(/[\d.]+/gu);
  if (parts === null) return 0;
  return parts.length >= 4 ? Number(parts[3]) : 1;
}

function hasVisibleBorder(style) {
  return (
    style.borderStyle !== "none" &&
    style.borderStyle !== "" &&
    Number.parseFloat(style.borderTopWidth || "0") +
      Number.parseFloat(style.borderBottomWidth || "0") +
      Number.parseFloat(style.borderLeftWidth || "0") +
      Number.parseFloat(style.borderRightWidth || "0") >
      0
  );
}

/**
 * 畫面上所有「畫了東西」的長方形，不含她本人。
 *
 * 用一條寬鬆的通則掃整棵 DOM，而不是列一張選擇器清單：清單漏掉一個新泡泡，
 * 那個泡泡就點不到，而且是安靜地點不到。通則多報幾塊的代價只是多擋一點桌面。
 */
function paintedRects() {
  const out = [];
  for (const el of document.body.querySelectorAll("*")) {
    // 她自己走剪影那條路；`.avatar` 是 button，會被 `SOLID_TAGS` 收進來，
    // 那就等於把 300×300 的空框整塊算成實心，這條線就白做了。
    if (avatar !== null && (el === avatar || avatar.contains(el))) continue;
    // SVG 的內部節點跟著它的 <svg>／按鈕走，不必各自報一次。
    if (el.closest("svg") !== null) continue;
    const style = getComputedStyle(el);
    if (
      style.display === "none" ||
      style.visibility === "hidden" ||
      Number.parseFloat(style.opacity || "1") === 0
    ) {
      continue;
    }
    const painted =
      alphaOf(style.backgroundColor) > 0 ||
      style.backgroundImage !== "none" ||
      hasVisibleBorder(style) ||
      SOLID_TAGS.has(el.tagName);
    if (!painted) continue;
    const box = el.getBoundingClientRect();
    if (box.width > 0 && box.height > 0) out.push(box);
  }
  return out;
}

/**
 * 她的剪影，切成一列一列的橫條（格子座標）。
 *
 * 量的是 bundled WebP，不是完整 rig：兩者依契約撐滿同一個座標格（`check-persona`
 * 有一條在守這件事），所以 WebP 的輪廓對 rig 也夠準，而且它一定在——rig 還沒
 * load 完、或整組 decode 失敗的時候都還是它在畫。這樣換 rig 也不必重算。
 *
 * 量不到就回 `null`，呼叫端會退回整個外框：canvas 被 taint（`getImageData` 丟
 * SecurityError）、圖還沒 decode、瀏覽器不給 2d context 都算量不到。
 */
function figureMaskOf(img) {
  if (img === null || !img.complete || !img.naturalWidth) return null;
  if (figureMask !== null && figureMask.src === img.currentSrc) return figureMask;
  try {
    // `drawImage(img, 0, 0, cells, cells)` 是**拉伸**，CSS 那邊卻是
    // `object-fit: contain`（會留黑邊）。兩者只有在來源是正方形、而且容器也是
    // 正方形的時候才會對齊——現在兩邊都成立：17 張 bundled preview 都是 640×640
    // （`check-persona.mjs` 的「bundled preview manifest 是透明 640² contain 全身
    // v2 契約」和 `select-persona-previews.py` 的 640x640 RGBA contract 兩道在守
    // 它），而 `.avatar` 是 300×300。**哪一邊變成非正方形，這張遮罩就會歪**，而歪
    // 掉的方向是「她有一塊點不到」。真要改的話這裡得先照 contain 自己算出留白。
    const cells = Math.max(1, Math.round(300 / FIGURE_CELL_PX));
    const canvas = document.createElement("canvas");
    canvas.width = cells;
    canvas.height = cells;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (ctx === null) return null;
    ctx.drawImage(img, 0, 0, cells, cells);
    const { data } = ctx.getImageData(0, 0, cells, cells);
    const spans = [];
    for (let row = 0; row < cells; row += 1) {
      let start = -1;
      for (let col = 0; col <= cells; col += 1) {
        // alpha 門檻壓低：立繪邊緣是半透明的，切太高會把輪廓削掉一圈。
        const solid = col < cells && data[(row * cells + col) * 4 + 3] > 16;
        if (solid && start < 0) start = col;
        if (!solid && start >= 0) {
          spans.push([row, start, col]);
          start = -1;
        }
      }
    }
    figureMask = { src: img.currentSrc, cells, spans };
    return figureMask;
  } catch {
    // taint 或任何一種量不到，都退回外框——多擋一點桌面，不讓她點不到。
    return null;
  }
}

function figureRects() {
  if (avatar === null || avatar.hidden) return [];
  const box = avatar.getBoundingClientRect();
  if (box.width <= 0 || box.height <= 0) return [];
  const mask = figureMaskOf(document.querySelector("[data-persona-portrait]"));
  if (mask === null) return [box];
  const cellW = box.width / mask.cells;
  const cellH = box.height / mask.cells;
  return mask.spans.map(([row, start, end]) => ({
    left: box.left + start * cellW - FIGURE_DILATE_PX,
    top: box.top + row * cellH - FIGURE_DILATE_PX,
    width: (end - start) * cellW + FIGURE_DILATE_PX * 2,
    height: cellH + FIGURE_DILATE_PX * 2,
  }));
}

function toSolid(box, grow) {
  return {
    x: Math.floor(box.left - grow),
    y: Math.floor(box.top - grow),
    w: Math.ceil(box.width + grow * 2),
    h: Math.ceil(box.height + grow * 2),
  };
}

function pushSolid() {
  if (invoke === null) return;
  // 拖曳期間整扇窗都算實心。`startDragging()` 之後作業系統接管，這邊看不到
  // 游標；萬一那一瞬間輪詢把開關翻成穿透，拖到一半會斷在半路。
  const solid = solidWholeWindow
    ? [{ x: 0, y: 0, w: Math.ceil(globalThis.innerWidth), h: Math.ceil(globalThis.innerHeight) }]
    : [
        ...paintedRects().map((box) => toSolid(box, SOLID_DILATE_PX)),
        ...figureRects().map((box) => toSolid(box, 0)),
      ];
  invoke("pet_solid_set", { solid }).catch(() => {
    // 推不過去就維持上一次的答案；Rust 那邊空清單＝一律實心，不會變成點不到。
  });
}

function scheduleSolidPush() {
  if (solidPushTimer !== null) return;
  solidPushTimer = setTimeout(() => {
    solidPushTimer = null;
    pushSolid();
  }, 80);
}

if (invoke !== null) {
  // 畫面上任何一塊東西出現、消失、換位置都要重算。用通用的 observer 而不是在
  // 每個顯示／隱藏的地方各補一行：後者漏掉一處就是一塊點不到的泡泡。
  new MutationObserver(scheduleSolidPush).observe(document.body, {
    attributes: true,
    attributeFilter: ["hidden", "class", "style"],
    childList: true,
    characterData: true,
    subtree: true,
  });
  globalThis.addEventListener("resize", scheduleSolidPush);
  document.querySelector("[data-persona-portrait]")?.addEventListener("load", () => {
    figureMask = null;
    scheduleSolidPush();
  });
  avatar?.addEventListener("pointerdown", () => {
    solidWholeWindow = true;
    pushSolid();
  });
  // 拖曳結束之後要把「整扇窗都實心」收回來，但**不能只靠 pointerup**：
  // `startDragging()` 之後作業系統接管拖曳迴圈，webview 通常收不到後續的
  // pointerup（上面 `draggedThisPress` 那段註解講的是同一件事）。只掛 pointerup
  // 的話，他拖她一次之後這個旗標就再也沒有人清，整條線從此靜悄悄地失效——而
  // 「拖她」正是他最先會做的那件事。
  //
  // 所以真正把它收回來的是**下一個沒有按著鍵的 pointermove**：那代表事件又回到
  // 這個 webview、而且手已經放開了。滑鼠沒回到她身上的時候旗標留著也無妨，那段
  // 期間整扇窗是實心的，pointermove 一定收得到，自己會好。
  const releaseWholeWindow = () => {
    if (!solidWholeWindow) return;
    solidWholeWindow = false;
    scheduleSolidPush();
  };
  for (const done of ["pointerup", "pointercancel"]) {
    globalThis.addEventListener(done, releaseWholeWindow);
  }
  globalThis.addEventListener("pointermove", (event) => {
    if ((event?.buttons ?? 0) !== 0) return;
    releaseWholeWindow();
  });

  /*
   * 游標一進到這扇窗，就把 Rust 那條輪詢執行緒叫起來。
   *
   * 它平常最久睡 160 毫秒（`hit::POLL_AWAY_MS`，游標離她 120 像素以外時用的
   * 那一段）。從遠處一口氣把滑鼠甩到她旁邊的**空白**上按下去，那一下就會在
   * 這扇窗裡被吃掉，底下的視窗收不到。可是那個瞬間 webview 是收得到
   * `pointermove` 的——窗那時候還是可點的，事件就是送到這裡來的——所以這邊喊
   * 一聲，比等它睡飽快兩個數量級。
   *
   * 只喊一聲，不送座標也不送答案。判斷整套留在 Rust，它醒來之後自己去問游標
   * 在哪。所以這邊多喊、少喊、喊錯時機，都只會讓那一拍早一點或晚一點發生，
   * 不會讓答案變錯——這條線壞掉的上限是「退回原本的輪詢」。
   *
   * 節流用的是**前緣**：第一次動就喊，之後 `NUDGE_EVERY_MS` 內的不喊。
   * `requestAnimationFrame` 會等到下一幀才喊，而那最多就是 16 毫秒——正好是
   * 這條線要省掉的東西；它在視窗隱藏時還會整個停掉。時間戳相減會被系統時鐘
   * 往回跳卡住，所以這裡連時鐘都不看。
   */
  const NUDGE_EVERY_MS = 16;
  let nudgeCoolingDown = false;
  globalThis.addEventListener("pointermove", () => {
    if (nudgeCoolingDown) return;
    nudgeCoolingDown = true;
    setTimeout(() => {
      nudgeCoolingDown = false;
    }, NUDGE_EVERY_MS);
    invoke("pet_pointer_moved").catch(() => {
      // 叫不醒就等它自己睡飽。那是慢，不是壞。
    });
  });
  scheduleSolidPush();
}

function readPersona() {
  if (invoke === null) return;
  const revisionWhenStarted = personaRevision;
  invoke("persona_read").then(
    (view) => {
      // 設定頁事件若先到，它代表比這次 initial read 更新的設定；舊回應不能蓋回去。
      if (personaRevision === revisionWhenStarted) {
        personaSelectionKnown = true;
        applyPersona(view);
      }
    },
    () => {
      // Persona 是選配表達層；設定檔暫時讀不出來不可以連搜尋框一起拖垮。
    },
  );
}

function usableAzureTtsStatus(raw) {
  const endpoints = {
    eastasia: "https://eastasia.tts.speech.microsoft.com/cognitiveservices/v1",
    southeastasia: "https://southeastasia.tts.speech.microsoft.com/cognitiveservices/v1",
    japaneast: "https://japaneast.tts.speech.microsoft.com/cognitiveservices/v1",
  };
  const voices = [
    "zh-TW-HsiaoChenNeural",
    "zh-TW-HsiaoYuNeural",
    "zh-TW-YunJheNeural",
  ];
  return typeof raw === "object" &&
    raw !== null &&
    Number.isSafeInteger(raw.generation) &&
    raw.generation >= 0 &&
    raw.config_readable === true &&
    typeof raw.enabled === "boolean" &&
    (raw.region === null || Object.hasOwn(endpoints, raw.region)) &&
    voices.includes(raw.voice) &&
    raw.endpoint === (raw.region === null ? null : endpoints[raw.region]) &&
    ["present", "missing", "unreadable", "unsupported"].includes(raw.credential) &&
    (raw.consented === null || typeof raw.consented === "boolean") &&
    (raw.consent_at === null ||
      (Number.isSafeInteger(raw.consent_at) && raw.consent_at >= 0)) &&
    (raw.consented === true ? raw.consent_at !== null : raw.consent_at === null) &&
    raw.config_error === null &&
    raw.ready ===
      (raw.enabled &&
        raw.region !== null &&
        raw.credential === "present" &&
        raw.consented === true &&
        raw.consent_at !== null)
    ? raw
    : null;
}

function syncAzureAnswerLine() {
  if (!azureSpeechEnabled) {
    if (azureAnswerLine) azureAnswerLine.remove?.();
    azureAnswerLine = null;
    stopAzureSpeech();
    return;
  }
  if (azureAnswerLine === null && hitList?.hidden === false) {
    azureAnswerLine = answerAzureLine();
    hitList.append(azureAnswerLine);
  }
}

function applyAzureTtsStatus(raw) {
  if (azureCancelPending) {
    // Cancel Promise 尚未 settle 時不能重發 token；狀態會在 settle 後權威重讀。
    azureNativeGeneration = null;
    azureNativeExpected = null;
    return;
  }
  const status = usableAzureTtsStatus(raw);
  azureStatusKnown = true;
  azureNativeGeneration = status?.generation ?? null;
  azureNativeExpected = status
    ? Object.freeze({
        generation: status.generation,
        enabled: status.enabled,
        region: status.region,
        voice: status.voice,
        consentAt: status.consent_at,
        credentialPresent: status.credential === "present",
      })
    : null;
  azureSpeechEnabled = status?.enabled === true;
  azureSpeechReady = status?.ready === true;
  syncAzureAnswerLine();
  settlePendingAzureAutoAnswer();
}

function readAzureTts() {
  if (azureCancelPending) {
    azureStatusReadRevision += 1;
    return;
  }
  const revision = ++azureStatusReadRevision;
  if (invoke === null) {
    applyAzureTtsStatus(null);
    return;
  }
  invoke("azure_tts_read").then(
    (status) => {
      if (revision === azureStatusReadRevision) applyAzureTtsStatus(status);
    },
    () => {
      if (revision === azureStatusReadRevision) applyAzureTtsStatus(null);
    },
  );
}

/**
 * 她有沒有一件事想讓人看見，是第四個維度：不改「正在做什麼」、不假裝錄製
 * 狀態，也不改暫停真相。Glimmer 只把這個位元翻起來；所以仍可同時是
 * thinking + paused + has-something，而 `paint()` 的前三個合成規則完全不變。
 */
let hasSomething = false;
let activeUtteranceId = null;
let latestGatekeeperView = null;
let gatekeeperReadError = null;

function renderGatekeeper(view) {
  const item = view?.display ?? null;
  const debug = view?.developer ?? null;
  if (handsLog) {
    if (gatekeeperReadError !== null) {
      handsLog.setAttribute("role", "alert");
      handsLog.textContent = `守門員／行動紀錄這一輪讀不出來：${gatekeeperReadError}`;
    } else {
      handsLog.removeAttribute("role");
      const lines = Array.isArray(view?.action_log) ? view.action_log : [];
      handsLog.textContent = lines.length === 0
        ? "守門員回傳了一份沒有說明的空 action log。"
        : `行動紀錄\n${lines.join("\n")}`;
    }
  }
  if (gateDebug) {
    gateDebug.hidden = debug === null;
    if (debug !== null) {
      gateDebug.textContent = `今天用了 ${debug.points_spent} 點 / 上限 ${debug.points_limit} 點\n${debug.holds.join("\n")}`;
    }
  }
  // 後端說「現在沒有要講的」就要**收掉**，不是把上一句留在畫面上。
  // 輪詢起來以後這一條才有牙齒：上一版只呼叫一次，所以「不再顯示」這件事
  // 從來沒有發生過，`return` 看起來是對的。
  if (item === null) {
    activeUtteranceId = null;
    hasSomething = false;
    avatar.classList.remove("has-something");
    utterance.hidden = true;
    utteranceActions.hidden = true;
    utteranceEvidence.replaceChildren();
    utteranceTargetProvenance.hidden = true;
    utteranceTargetProvenance.textContent = "";
    suggestionButton.hidden = true;
    suggestionButton.removeAttribute("data-commitment-id");
    return;
  }
  // 同一句話重畫一次不要把他讀到一半的回條擦掉。
  if (item.utterance_id === activeUtteranceId) {
    // URL 題正在存的幾秒會暫借同一個文字 slot。還回來時，同一句 gatekeeper
    // 不會重畫內容，但必須重新露出來。
    if (item.form !== "glimmer") utterance.hidden = false;
    return;
  }
  activeUtteranceId = item.utterance_id;
  hasSomething = true;
  avatar.classList.add("has-something");
  utteranceResult.textContent = "";
  utteranceEvidence.replaceChildren();
  if (item.suggestion === null) {
    utteranceTargetProvenance.hidden = true;
    utteranceTargetProvenance.textContent = "";
    suggestionButton.hidden = true;
    suggestionButton.removeAttribute("data-commitment-id");
  } else {
    utteranceTargetProvenance.textContent = item.suggestion.target_provenance;
    utteranceTargetProvenance.hidden = false;
    suggestionButton.textContent = `要我幫你${item.suggestion.label}嗎`;
    // 帶回去的是「哪一張承諾」，不是「要執行什麼」。要做什麼由 Rust 那邊
    // 重讀一次資料庫決定——畫面說了不算。
    suggestionButton.dataset.commitmentId = String(item.suggestion.commitment_id);
    suggestionButton.hidden = false;
  }
  for (const evidence of item.evidence ?? []) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "see";
    chip.textContent = evidence.label;
    chip.addEventListener("click", () => void invoke?.("open_frame", { frameId: evidence.frame_id }));
    utteranceEvidence.append(chip);
  }
  // 三個 form 是封閉集合；沒有 default。後端多一個字串，這裡會直接報 contract 壞掉。
  switch (item.form) {
    case "glimmer":
      utterance.hidden = true;
      utteranceText.textContent = "";
      utteranceActions.hidden = true;
      return;
    case "one_line":
      utterance.hidden = false;
      utterance.dataset.form = "one_line";
      utteranceText.textContent = item.text;
      utteranceEvidence.hidden = true;
      utteranceActions.hidden = false;
      return;
    case "card":
      utterance.hidden = false;
      utterance.dataset.form = "card";
      utteranceText.textContent = item.text;
      // 一格都沒有的時候要收起來。日終那種卡的出處是 `reviewer_run:` 和
      // `daysummary:`，兩種都不是畫面，所以 chip 一顆都做不出來——留一個空
      // 框在那裡，讀起來會是「這裡有出處」，而它下面什麼都沒有。
      utteranceEvidence.hidden = utteranceEvidence.childElementCount === 0;
      utteranceActions.hidden = false;
      return;
  }
  throw new Error(`不知道的 gatekeeper form：${item.form}`);
}

function reactToGatekeeper(close) {
  if (invoke === null || activeUtteranceId === null) return;
  const reactedUtterance = activeUtteranceId;
  invoke("gatekeeper_react", { utteranceId: reactedUtterance, close }).then(
    (view) => {
      if (activeUtteranceId !== reactedUtterance || masterStopPhase !== "clear") {
        releaseNativePresentation(view);
        return;
      }
      void commitNativePresentation(view, () => {
        // 三個後端結果分別是「這張記憶不會再提了」、「先收起來，之後再說」、
        // 「收到你的回饋；這一則沒有可結案或延後的承諾」。不在前端猜類別。
        utteranceResult.textContent = view.message;
        utteranceActions.hidden = true;
        hasSomething = false;
        avatar.classList.remove("has-something");
      }).catch((error) => {
        if (activeUtteranceId === reactedUtterance && masterStopPhase === "clear") {
          utteranceResult.textContent = String(error);
        }
      });
    },
    (error) => {
      if (activeUtteranceId === reactedUtterance && masterStopPhase === "clear") {
        utteranceResult.textContent = String(error);
      }
    },
  );
}

utteranceClose?.addEventListener("click", () => reactToGatekeeper(true));
utteranceOther?.addEventListener("click", () => reactToGatekeeper(false));
suggestionButton?.addEventListener("click", () => {
  const raw = suggestionButton.dataset.commitmentId;
  if (invoke === null || raw === undefined) return;
  const commitmentId = Number(raw);
  if (!Number.isInteger(commitmentId)) return;
  suggestionButton.disabled = true;
  invoke("hands_execute", { commitmentId }).then(
    (message) => { utteranceResult.textContent = message; },
    (error) => { utteranceResult.textContent = String(error); },
  ).finally(() => { suggestionButton.disabled = false; });
});

/**
 * 她問的那一題：「你不在的時候，要我自己按下去嗎？」（PHASES #42）
 *
 * **這一格不是設定頁裡的一個開關，是她開口問的一個問題。** 差別在預設值：
 * 開關有一邊是預設的，而預設的那一邊等於產品替他選了。這裡沒有預設值——
 * 後端回 `answered === null` 就是**還沒問過**，而那和「他說了不要」是兩句
 * 不同的話（她一個人在跑的時候兩種都不會開網址，但講的理由不一樣）。
 *
 * 每一句話都是後端給的。前端一個字都不寫：同一題 `sister url-policy` 也在
 * 問，文案分兩份的話他答的是哪一個沒人說得準。
 *
 * **答過就不再問。** 這個視窗整天掛在螢幕角落，一個問過的問題再問一次是
 * 騷擾。改主意的路是 `sister url-policy`——那條路寫在他按完之後那一句裡。
 */
let urlPolicyView = null;
let urlPolicyReadError = null;
let urlPolicyWriteState = "idle";
let urlPolicyWriteMessage = "";
let urlPolicyConfirmationTimer = null;

function gatekeeperClaimsConversation() {
  const form = latestGatekeeperView?.display?.form;
  return form === "one_line" || form === "card";
}

function showUrlPolicy(shown) {
  urlPolicy.hidden = !shown;
  document.body.classList.toggle("has-url-policy", shown);
}

/** 一個文字 slot：gatekeeper 已經開口時，這題等下一輪，不在她臉上疊第二句。 */
function paintConversation() {
  if (!urlPolicy) return;
  if (latestGatekeeperView !== null) renderGatekeeper(latestGatekeeperView);

  // 同意書是使用者此刻正在回答的問題；這四張沒走完以前，不把守門員或 URL
  // 題疊在同一顆氣泡裡。它們都沒有消失，完成後下一輪會照原狀回來。
  if (consentGuideSheet !== null) {
    if (gatekeeperClaimsConversation()) utterance.hidden = true;
    showUrlPolicy(false);
    return;
  }

  const ownsWhileWriting = ["writing", "failed", "confirmed"].includes(urlPolicyWriteState);
  const userIsTalking = state === "thinking" || document.body.classList.contains("has-hits");
  // 只有一格能說話，優先序是：使用者正在問／讀答案 > 正在存的回答與回條 >
  // gatekeeper > 還沒作答的 URL 題。URL 存到一半不能被五秒輪詢蓋掉；反過來，
  // 使用者剛問的那一題也不能被一張寫入回條蓋掉。
  if (userIsTalking) {
    if (gatekeeperClaimsConversation()) utterance.hidden = true;
    showUrlPolicy(false);
    return;
  }
  if (ownsWhileWriting && gatekeeperClaimsConversation()) utterance.hidden = true;
  if (!ownsWhileWriting && gatekeeperClaimsConversation()) {
    showUrlPolicy(false);
    return;
  }

  if (urlPolicyWriteState === "confirmed") {
    // 成功後只剩一張短回條。問題、未回答說明、兩個答案都已經不是現在式。
    urlPolicyQuestion.textContent = "";
    urlPolicyNote.textContent = "";
    urlPolicyActions.replaceChildren();
    urlPolicyResult.setAttribute("role", "status");
    urlPolicyResult.textContent = urlPolicyWriteMessage;
    if (gatekeeperClaimsConversation()) utterance.hidden = true;
    showUrlPolicy(true);
    return;
  }

  if (urlPolicyReadError !== null) {
    urlPolicyQuestion.textContent =
      "這題現在讀不出來；沒有把你算成答過，也沒有替你選。";
    urlPolicyNote.textContent = "";
    urlPolicyResult.setAttribute("role", "alert");
    urlPolicyResult.textContent = urlPolicyReadError;
    urlPolicyActions.replaceChildren();
    const retry = document.createElement("button");
    retry.type = "button";
    retry.textContent = "再讀一次";
    retry.addEventListener("click", readUrlPolicy);
    urlPolicyActions.append(retry);
    showUrlPolicy(true);
    return;
  }

  const view = urlPolicyView;
  if (!view || (view.answered !== null && view.answered !== undefined)) {
    showUrlPolicy(false);
    return;
  }
  urlPolicyQuestion.textContent = view.question;
  urlPolicyNote.textContent = view.before_you_answer;
  urlPolicyResult.removeAttribute("role");
  urlPolicyResult.textContent = urlPolicyWriteState === "failed" ? urlPolicyWriteMessage : "";
  if (urlPolicyWriteState === "failed") urlPolicyResult.setAttribute("role", "alert");
  urlPolicyActions.replaceChildren();
  for (const option of view.options ?? []) {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = option.line;
    button.dataset.urlPolicyAnswer = option.key;
    button.disabled = urlPolicyWriteState === "writing";
    button.addEventListener("click", () => answerUrlPolicy(option.key, view.path));
    urlPolicyActions.append(button);
  }
  showUrlPolicy(urlPolicyActions.childElementCount > 0);
}

function receiveGatekeeper(view) {
  gatekeeperReadError = null;
  latestGatekeeperView = view;
  paintConversation();
}

function presentationIdOf(view) {
  return typeof view?.presentation_id === "string" ? view.presentation_id : null;
}

function releaseNativePresentation(view) {
  const presentationId = presentationIdOf(view);
  if (invoke !== null && presentationId !== null) {
    void invoke("master_stop_presentation_end", { presentationId }).catch(() => {});
  }
}

function releaseAzurePlaybackPresentation(view = azurePlaybackPresentation) {
  if (view === null) return;
  if (azurePlaybackPresentation === view) azurePlaybackPresentation = null;
  releaseNativePresentation(view);
}

async function beginNativePresentation(view) {
  const presentationId = presentationIdOf(view);
  if (presentationId === null) {
    // 純瀏覽器與舊 fixture 沒有 native lease；產品 IPC 回覆一定帶。
    return masterStopPhase === "clear";
  }
  const allowed = await invoke("master_stop_presentation_begin", { presentationId });
  return allowed === true && masterStopPhase === "clear";
}

/**
 * Native IPC 回傳與這段 JS 真正畫完之間還有一個排程縫。外部 CLI 的 stop-all
 * 沒有 Tauri event；只看五秒 poll，會讓 stop 成功後才跑到晚 Promise。Native
 * lease 把 activity guard 留在 Rust：begin 在 turnstile 內重驗，render 同步完成
 * 後 end。尚未 begin 的回覆五秒後回收；begun lease 只由 end 或 window/process
 * teardown 釋放，timeout 不能插在 stop success 與晚 render 中間。
 */
async function commitNativePresentation(view, render) {
  try {
    const allowed = await beginNativePresentation(view);
    if (!allowed) {
      readMasterStopState();
      return false;
    }
    render();
    return true;
  } finally {
    releaseNativePresentation(view);
  }
}

function readGatekeeper() {
  if (invoke === null || masterStopPhase !== "clear") return;
  const request = ++gatekeeperReadRequest;
  invoke("gatekeeper_check").then(
    (view) => {
      if (request !== gatekeeperReadRequest || masterStopPhase !== "clear") {
        releaseNativePresentation(view);
        return;
      }
      void commitNativePresentation(view, () => receiveGatekeeper(view)).catch((error) => {
        if (request === gatekeeperReadRequest && masterStopPhase === "clear") {
          failToReceiveGatekeeper(error);
        }
      });
    },
    (error) => {
      if (request === gatekeeperReadRequest && masterStopPhase === "clear") {
        failToReceiveGatekeeper(error);
      }
    },
  );
}

function failToReceiveGatekeeper(error) {
  // 不留上一輪的卡片或 action log 假裝仍是現在式。錯誤本身要看得見，也要讓
  // 螢幕閱讀器聽得見；「讀不到」不是「現在沒有任何事」。
  gatekeeperReadError = String(error);
  latestGatekeeperView = null;
  renderGatekeeper(null);
  paintConversation();
}

function receiveUrlPolicy(view) {
  urlPolicyView = view;
  urlPolicyReadError = null;
  urlPolicyWriteState = "idle";
  paintConversation();
}

function failToReadUrlPolicy(error) {
  urlPolicyView = null;
  urlPolicyReadError = String(error);
  urlPolicyWriteState = "idle";
  paintConversation();
}

function readUrlPolicy() {
  if (invoke === null) return;
  invoke("url_policy_read").then(receiveUrlPolicy, failToReadUrlPolicy);
}

function answerUrlPolicy(key, path) {
  if (invoke === null) return;
  urlPolicyWriteState = "writing";
  urlPolicyWriteMessage = "";
  paintConversation();
  invoke("url_policy_write", { key }).then(
    (message) => {
      urlPolicyView = { ...urlPolicyView, answered: key };
      urlPolicyWriteState = "confirmed";
      urlPolicyWriteMessage =
        `${message}\n存到 ${path}。改主意的話跑 sister url-policy。`;
      paintConversation();
      clearTimeout(urlPolicyConfirmationTimer);
      // 回條只留一小段時間；之後這題已經答完，讓正在等的 gatekeeper 接手。
      urlPolicyConfirmationTimer = setTimeout(() => {
        urlPolicyWriteState = "idle";
        paintConversation();
      }, 5000);
    },
    (error) => {
      // 存不進去的時候**不可以**顯示成他答過了：下一次她一個人在跑，
      // 擋下來的理由仍然會是「我還沒問過你」，而畫面說他選過了。
      urlPolicyWriteState = "failed";
      urlPolicyWriteMessage = `沒有存進去，所以這一題還是沒答：${String(error)}`;
      paintConversation();
    },
  );
}

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
// 開場那個 `true` 只保留既有 state shape；在第一次 `recording_state` 回來前，
// 它不是一份 heartbeat 證據，畫面只能說正在確認，不能借它宣布在聽。
let recordingStateKnown = false;
// IPC 有回不等於 heartbeat 可讀；`unreadable` 必須和真的 `none` 分開。
let recordingStateReadable = false;
let recordingStateReadRequest = 0;

/**
 * 有一個 `sister record` 起來了，但它還在開資料庫（心跳的第二欄是 `boot`）。
 *
 * **這是第三種，不是「沒有人在錄」的一種。** `recording_state` 上一版回的是
 * 一個布林（`is_recording`），而它把這幾分鐘歸進 false，於是：底下那個等她起
 * 來的迴圈等滿 25 秒然後說「還沒有心跳」（心跳從第一秒就在），那顆「叫她起
 * 來」的按鈕跟著放回來，而他再按一次只會拿到一句「已經有一個 sister record
 * 在跑了」。一顆存了一年的資料庫 `Db::open` 要跑好幾分鐘，所以那不是罕見的
 * 幾百毫秒，是他每天早上都會看到的那一段。
 */
let booting = false;

/**
 * 錄製迴圈已經停了，解釋層還在把最後一段想完。
 *
 * **這也不是「沒有人在錄」的一種。** 心跳的 `is_recording` 是 false（她不抓
 * 畫面了），但行程還握著資料庫。畫面若說「沒有人在記錄」配一顆開始鍵，他
 * 按下去會拿到一句「解釋層還在想最後一段」——兩句都是這台機器印的，直接
 * 對打。
 */
let thinkingLast = false;

/**
 * desktop 自己開的 recorder 現在走到哪裡。
 *
 * 這和上面的 heartbeat 是兩份不同證據：heartbeat 說資料目錄最近有沒有拍，
 * supervisor 說它握著的 child 是否在啟動、等候重試、或已經放棄。尤其 hard kill
 * 之後，舊 heartbeat 在安全期限內仍可能看起來是 live；這幾秒若只看 heartbeat
 * 就會印出「在聽」，而真正的 child 已經不在了。
 *
 * `null` 只代表這扇 renderer 還沒讀到第一份 view。讀壞不是 null，而是
 * `uncertain`：沒量到和量到「不確定」不能共用同一個空值。
 */
const SUPERVISOR_PHASES = Object.freeze([
  "stopped",
  "cooling",
  "starting",
  "running",
  "backoff",
  "gave-up",
  "external",
  "stopping-external",
  "stop-undelivered",
  "uncertain",
  "quitting",
]);

const SUPERVISOR_FALLBACKS = Object.freeze({
  stopped: "",
  cooling:
    "desktop 自己開的 recorder 已退出；正在等那一輪最後的 heartbeat 證據退場，這期間不會啟動另一個 recorder。",
  starting: "正在啟動 recorder；這期間還沒有開始記錄。",
  running: "",
  backoff: "record 剛剛異常退出；desktop 正在等候下一次自動重試。",
  "gave-up": "record 已停止自動重試；從現在起發生的事她不會知道。",
  external: "最近一拍辨識為另一個 recorder；desktop 只照 heartbeat 顯示，不接管或自動重試。",
  "stopping-external":
    "已留下停止外部 recorder 的要求；正在等它寫出停止墓碑，desktop 不接管或重啟它。",
  "stop-undelivered":
    "真人要求的停止尚未送達磁碟；recorder 仍可能在跑，desktop 不會自動重開。可以再送一次停止要求。",
  uncertain: "現在讀不出 recorder supervisor 的狀態；不能確認自動重試是否仍在運作。",
  quitting: "AI-Sister 正在結束；不會再啟動 recorder。",
});

let recorderSupervisor = null;
// heartbeat 與 supervisor 是平行 IPC。第一份 heartbeat 先回來時，null 仍只代表
//「另一份還沒量到」，不能把它當成 stopped，短暫宣布在聽或開放第二次 start。
let recorderSupervisorStateKnown = false;
let recorderSupervisorRevision = 0;
let recorderSupervisorReadRequest = 0;

function normalizeRecorderSupervisor(view) {
  const phase = SUPERVISOR_PHASES.includes(view?.phase) ? view.phase : "uncertain";
  const supplied = typeof view?.message === "string" ? view.message.trim() : "";
  return Object.freeze({
    phase,
    failures: Number.isInteger(view?.failures) && view.failures >= 0 ? view.failures : 0,
    message: supplied || SUPERVISOR_FALLBACKS[phase],
  });
}

function receiveRecorderSupervisor(view) {
  recorderSupervisorStateKnown = true;
  recorderSupervisor = normalizeRecorderSupervisor(view);
  if (recorderSupervisor.phase === "stop-undelivered") {
    // 這是一份比本機 click 猜出的 `starting` 更新、而且方向相反的 supervisor
    // 證據。保留 starting 會讓 headline 繼續說正在叫醒，掩住真人 Stop 未送達。
    starting = false;
  }
  paint();
}

function failToReadRecorderSupervisor(error) {
  const reason = error === null || error === undefined ? "這次 IPC 沒有附原因" : String(error);
  recorderSupervisorStateKnown = true;
  recorderSupervisor = normalizeRecorderSupervisor({
    phase: "uncertain",
    message: `recorder supervisor 狀態讀不出來：${reason}`,
  });
  paint();
}

function readRecorderSupervisor() {
  if (invoke === null) return;
  const revisionWhenStarted = recorderSupervisorRevision;
  const request = ++recorderSupervisorReadRequest;
  invoke("recorder_supervisor_state").then(
    (view) => {
      // event 比這次磁碟／worker read 晚發生；舊回應不能把新的 Backoff／GaveUp
      // 蓋回去。下一輪 polling 會用新的 revision 再讀一次。
      if (
        recorderSupervisorRevision === revisionWhenStarted &&
        recorderSupervisorReadRequest === request
      ) {
        receiveRecorderSupervisor(view);
      }
    },
    (error) => {
      if (
        recorderSupervisorRevision === revisionWhenStarted &&
        recorderSupervisorReadRequest === request
      ) {
        failToReadRecorderSupervisor(error);
      }
    },
  );
}

function supervisorRunningBeforeHeartbeat() {
  return (
    recorderSupervisor?.phase === "running" &&
    (!recordingStateKnown || (!recording && !booting && !thinkingLast))
  );
}

/**
 * 這幾態都不能拿初值或一份可能尚未過期的舊 heartbeat 宣布「在聽」。
 * `external` 刻意不在裡面：supervisor 不擁有它，現在有沒有錄只由 heartbeat 作證。
 */
function supervisorBlocksListeningClaim() {
  return (
    !recordingStateKnown ||
    !recordingStateReadable ||
    !recorderSupervisorStateKnown ||
    supervisorRunningBeforeHeartbeat() ||
    [
      "cooling",
      "starting",
      "backoff",
      "gave-up",
      "stopping-external",
      "stop-undelivered",
      "uncertain",
      "quitting",
    ].includes(recorderSupervisor?.phase)
  );
}

function supervisorHeadline() {
  if (!recordingStateKnown || !recorderSupervisorStateKnown) {
    return "正在確認 recorder 是否已開始記錄";
  }
  if (!recordingStateReadable) {
    return "讀不懂 recording.beat；現在不能確認 recorder 是否正在記錄";
  }
  switch (recorderSupervisor?.phase) {
    case "cooling":
      return "owned recorder 已退出，正在確認最後 heartbeat 證據";
    case "starting":
      return "正在啟動 recorder";
    case "running":
      return supervisorRunningBeforeHeartbeat()
        ? "recorder 行程仍活著，但目前沒有可驗證的新鮮錄製心跳"
        : null;
    case "backoff":
      return "record 剛剛中斷，正在等候重試";
    case "gave-up":
      return "record 已停止自動重試";
    case "stopping-external":
      return "已請外部 recorder 收工，正在等停止墓碑";
    case "stop-undelivered":
      return "停止要求尚未送達；現在不能確認 recorder 已停止";
    case "uncertain":
      return "現在不能確認 recorder 是否正在記錄";
    case "quitting":
      return "AI-Sister 正在結束";
    default:
      return null;
  }
}

/**
 * 她可以在 recorder 沒有作證時回答舊記憶，但括號裡只能講目前真的知道的事。
 * 第一份 IPC 尚未回來、或 supervisor 明說 uncertain，都不是「沒有人在記錄」。
 */
function thinkingRecordingQualifier(shown) {
  if (shown === "paused") return "仍在暫停";
  return supervisorHeadline() ?? "但沒有人在記錄";
}

/**
 * 按了「開始記錄」之後、她真的開始之前的那一段。
 *
 * 這一段可能要好幾秒：另一個行程要載入、開資料庫、跑 migration。這期間畫面上
 * 不能還寫著「沒有人在記錄」配一顆按得下去的按鈕——他會再按一次，然後就有兩個
 * recorder 各錄一份，而唯一看得出來的症狀是磁碟用得比講好的快一倍。
 *
 * 心跳一出現這個就關掉，接手的是 `booting`／`recording`——那兩個是**看到的**，
 * 這一個是**猜的**（我按了，所以她大概在起來）。
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
 * 剛剛那一下為什麼沒成（`null` = 沒有這回事）。
 *
 * 這幾句話以前是直接寫進 `stateLine.textContent` 的，而 `pollRecording` 每 5 秒
 * 會呼叫兩次 `paint()`（`setRecording` 和 `setPaused` 各一次），`paint()` 又
 * 無條件覆寫那一格——**所以它們的壽命是 0 到 5 秒，而且不由它們自己決定**。
 * 暫停那一條的註解說得最清楚：「寧可看起來沒反應，然後把原因寫出來」。輪詢
 * 一到，只剩下前半句。所以它得是狀態，跟著每一次重畫一起出現。
 *
 * **這裡以前是兩個欄位。** `wakeFailed`（叫不起來）和 `notice`（問問題、暫停、
 * 時間軸），而 `paint()` 讀的是 `notice ?? (只有灰著的時候 ? wakeFailed : "")`
 * ——一個固定的優先序，加上一道只讓其中一個出得了聲的閘門。兩邊都咬人：
 *
 * 一、系統匣那一格是**開關**（見 `main.rs` 的 `record_label`），所以
 *     `recorder-failed` 也會帶著「停不了」回來——而那一刻她正在錄，`wakeFailed`
 *     在那個狀態下根本不顯示。後端特地 `win.show()` + `set_focus()` 把視窗叫到
 *     他面前，然後那一格一個字都沒多。
 *
 * 二、反過來：`startRecording` 那幾秒 `await` 中間他問了一題、失敗了，那句
 *     `notice` 會把接下來那句「第一張同意書還沒簽」擋掉。**兩行都是真的，湊
 *     起來說的是「她沒起來，因為資料庫打不開」**——而他會去查一顆好好的資料庫，
 *     真正的原因在右鍵選單裡，一下就簽得掉。
 *
 * 兩個欄位餵同一行字，就是在替「誰先誰後」造一個沒有人會去想的規則。收成一個
 * 之後規則只剩一條：**最後寫的人贏**，而清掉它的是下一件事（見
 * [`overtakenByEvents`]），不是五秒。
 *
 * **但收成一個欄位並沒有解掉二。** 上面那段只描述了 `start_recording` 被
 * reject 的那一瞬間，而那是這個 bug 比較小的一半：真正長的是它 **resolve**
 * 之後——`starting` 最久真 25 秒，`booting` 更是好幾分鐘——而那整段時間裡
 * 任何一句 `notice` 都會貼在「正在把她叫起來…」底下，讀起來就是她起不來的
 * 原因。收欄位收掉的是「誰蓋掉誰」，蓋不掉的是「兩句話並排會被讀成因果」。
 * 那一半修在 `paint()` 裡：她正在起來的時候，那一行要自己帶主詞。
 *
 * **而主詞不可以用狀態去猜。** alpha.41 那一版寫的是
 * `starting || booting ? "這是另一件事：" + notice : notice`，也就是拿「她正在
 * 起來」當「所以這句話一定不是在講她」的證據。那個推論是假的，而且假在**最
 * 危險的方向**：`booting` 那幾分鐘他從系統匣按下去，`recording_state` 不是
 * `"none"`，所以那一顆送的是 `stop_recording`（`main.rs` 的 handler 讀真相不讀
 * 標籤）——於是 `booting` 期間唯一送得出 `recorder-failed` 的路**就是停不掉**：
 *
 *     她起來了，正在開資料庫…（大的記憶要等一下，這期間還沒開始記）
 *     這是另一件事：write …\stop.request: Access is denied. (os error 5)
 *
 * 他按的停止沒有生效，她開完資料庫就會開始錄一整天。而那行前綴宣告「跟上面
 * 無關」，把唯一一句「你那一下沒有生效」推開——上面那行還寫著「這期間還沒開始
 * 記」。兩句合起來是「她好好的，另外有個檔案權限問題」，他於是走開。
 *
 * 所以主詞由**寫的人**帶，不由讀的人猜：[`noticeAboutHer`] 和
 * [`noticeAboutSomethingElse`]。這不是把 alpha.40 收掉的那個欄位加回來——那次
 * 兩個欄位餵**同一行字**，誰蓋掉誰沒有人定義過；這裡是一行字加上一個「它在講
 * 誰」，而且沒有第三種選擇：`notice` 只有那兩個函式寫得進去。
 *
 * 不確定的那一邊往「在講她」倒（＝不加前綴）。兩個方向的代價不對稱：少了前綴
 * 他會多按一次「叫她起來」，多了前綴他會**放著一台正在錄的機器走開**。
 */
let notice = null;

/**
 * 這句話在講她：她起不來、停不掉、不會開始。
 *
 * 她正在起來的時候**不加**前綴——這一句就是在解釋上面那一行，兩句並排讀成
 * 因果是對的。
 */
function noticeAboutHer(text, expiresOnRecordingChange = false) {
  notice = { text: String(text ?? ""), aboutHer: true, expiresOnRecordingChange };
}

/**
 * 這句話在講他**同時**做的別的事：問一題、按暫停、開時間軸。
 *
 * 這三個是唯一會在她起來的那 25 秒（`booting` 更久）裡寫字進來、而且真的和她
 * 起不起得來無關的來源。而那正是他最可能去問問題的那 25 秒——畫面剛剛叫他
 * 等一下。
 */
function noticeAboutSomethingElse(text, expiresOnRecordingChange = false) {
  notice = { text: String(text ?? ""), aboutHer: false, expiresOnRecordingChange };
}

/**
 * 從這個視窗以外發生了一件事，所以他上一下按了什麼沒成那句話到此為止。
 *
 * [`notice`] 講的是「他手指剛剛按下去的那一下」，所以它的死期是**下一件事發生**
 * ——不是五秒後（那是 alpha.38 修掉的那個 bug），也不是永遠。按下去的地方本來
 * 就自己清（`startRecording`、時間軸、暫停、`ask`、答案底下那顆標記），漏掉的是
 * **從這個視窗以外**發生的那幾件：系統匣的開始記錄、系統匣的暫停、她自己停掉。
 * 那幾件在這裡是事件不是點擊，所以沒有人替它們清。
 *
 * 不寫數字：上一版寫「四個」，然後 alpha.43 加上標記那一顆的時候，數字沒跟著
 * 改——而那顆也真的忘了清。一個要靠人記得同步的計數，遲早會變成一句假話。
 *
 * 漏掉的代價：他在視窗裡按暫停，切不動（「找不到資料目錄，暫停鍵沒有作用」）；
 * 再從系統匣按暫停，成功了。畫面於是是「已暫停，沒有在看／找不到資料目錄，
 * 暫停鍵沒有作用」——兩行都曾經是真的，讀起來是「暫停鍵壞了」，而她正暫停著。
 *
 * 只在狀態**真的變了**的時候清：那五秒一次的輪詢每次都用同一個值呼叫
 * `setPaused`／`setRecording`，跟著清的話就變回 alpha.38 那個 bug 了。
 *
 * 反過來那一半（狀態真的變了、而那句話留著）也要清，理由不同：留著的話它會和
 * 上面那行**直接互相矛盾**。「她起來了，正在開資料庫…／她還在開資料庫，暫停鍵
 * 現在沒有作用」是一致的；開完之後上面那行換成「在聽」，下面那句就變成一句
 * 當場被打臉的話。
 */
function overtakenByEvents() {
  notice = null;
}

/**
 * Heartbeat transition 只能淘汰「這一次開／停／暫停記錄」的回條。問答、時間軸或
 * 標記失敗是另一件事；新的 recording shape 不能把它們擦掉。
 */
function overtakenByRecordingChange() {
  if (notice?.expiresOnRecordingChange === true) notice = null;
}

/**
 * 這一題等很久了（`null` = 沒有／已經回來了）。見 [`SLOW_MS`]。
 *
 * 一樣是被 `paint()` 蓋掉的那一種：它以前直接寫 `stateLine.textContent`，於是
 * 那句話在畫面上閃一下就被輪詢換回「想一下…」——而「想一下…」不動地停在
 * 那裡，正是這一句話當初要修的那個畫面。修法變成看起來像 glitch，比不修
 * 更糟。（那句話的內容 alpha.132 改過，見 `SLOW_MS` 那一段。）
 */
let slowNote = null;

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
  // 全停要壓過 pause，因為解除 pause 不是解除全停；而「被你叫停」要壓過
  // 「根本沒人開她」——前者是他做的決定，後者只是狀態。
  const masterStopHasHeadline = ["stopping", "stopped", "uncertain"].includes(masterStopPhase);
  const shown = masterStopHasHeadline
    ? "stopped"
    : masterStopPhase === "checking"
      ? "asleep"
      : paused
      ? "paused"
      : recording && !supervisorBlocksListeningClaim()
        ? state
        : "asleep";
  avatar.dataset.state = shown;
  // 她**被你停下來**的時候，上面那條膠囊自己要出現——不管使用者有沒有按過那顆
  // 點點鍵。整片透明的桌寵上，「看不到她在錄」和「看不到她停著」是同一種畫面，
  // 而後者要一眼看得出來，「繼續」也要一下按得到。這一格不歸那顆鍵管（見
  // `setChromeOpen`）。
  //
  // `asleep` 刻意不算：那不是他做的決定，而且沒有什麼「繼續」好按——那一格的
  // 話已經寫在她底下那句狀態列了。加進來的話，她開機還在確認的那幾秒就會先把
  // 一條膠囊掛在他桌面上，而「平常上面也是透明的」正是他要的。
  document.body.classList.toggle(
    "she-is-stopped",
    shown === "paused" || shown === "stopped",
  );

  // 暫停時仍然答得出問題——停的是「記錄」，不是「記憶」。所以 thinking
  // 要講出來，只是講在文字上，不動那個灰掉的身體。沒在錄的時候同理：
  // 她答得出以前記下來的東西。
  // 「正在起來」排在 `starting` 前面：兩個都是「還沒開始錄」，但這一個是**看
  // 到心跳**才說的，而且說得出她卡在哪裡。他那顆一年份的資料庫要開好幾分鐘，
  // 一句「正在把她叫起來…」在第三分鐘讀起來像當掉了。
  //
  // `slowNote` 插在「想一下…」前面而不是接在後面：它要換掉的就是那三個字。
  // 只在 `state === "thinking"` 的時候看它——`ask()` 回來會把它清成 null，
  // 但清跟重畫之間仍然有順序問題，多這一個條件就不必去猜那個順序。
  const supervisedLine = !paused && state !== "thinking" ? supervisorHeadline() : null;
  const trustHeartbeat = !supervisorBlocksListeningClaim();
  const heartbeatUnreadable = recordingStateKnown && !recordingStateReadable;
  const line = masterStopHasHeadline
    ? MASTER_STOP_LINES[masterStopPhase]
    : masterStopPhase === "checking"
      ? "正在確認錄製與全停狀態…"
      : booting && trustHeartbeat
      ? "她起來了，正在開資料庫…（大的記憶要等一下，這期間還沒開始記）"
      : thinkingLast && trustHeartbeat
        ? "錄製已停，解釋層還在想最後一段"
        : heartbeatUnreadable && supervisedLine !== null
          ? supervisedLine
          : starting
            ? "正在把她叫起來…"
            : supervisedLine !== null
              ? supervisedLine
              : state === "thinking" && slowNote !== null
                ? slowNote
                : state === "thinking" && shown !== "thinking"
                  ? `想一下…（${thinkingRecordingQualifier(shown)}）`
                  : STATE_LINES[shown];
  // 灰掉的時候多講一句「上一次是什麼時候、為什麼停的」。換行不換句：那是
  // 同一件事的後半段，而 `.state-line` 的 `pre-line` 讓它自己排。
  //
  // 剛剛才叫不起來的話，那一句蓋過「上一次是什麼時候停的」——他現在要處理的
  // 是眼前這一次沒起來，不是上禮拜那一場怎麼收的。
  //
  // `notice` 排在最前面，而且**不看現在是哪一個狀態**：他剛按的那一下沒成，
  // 那句話在她正在錄、正在暫停、還是灰著的時候都一樣該出現。
  //
  // 那道 `shown === "asleep"` 的閘門只管 `asleepDetail()`——它講的是上一場錄製，
  // 只有灰著的時候有意義。**以前 `wakeFailed` 也被掃進這道閘門底下**，於是一句
  // 從系統匣按「停止記錄」失敗的原因，在她正在錄的時候一個字都不顯示。見
  // [`notice`] 上面那段。
  //
  // **她正在起來的時候，底下那一行要自己補一個主詞。** `starting` / `booting`
  // 的時候上面那句講的是「她走到哪了」，而底下那一行沒有主詞——兩行並排，唯一
  // 讀得出來的意思是「她起不來，因為 X」：
  //
  //     正在把她叫起來…
  //     資料庫打不開
  //
  // 而這正是他最可能去問問題的那 25 秒：畫面剛剛叫他等一下。更糟的是那一題
  // 失敗的原因（她正在開那顆一年份的資料庫）和她還沒起來的原因是同一個，所以
  // 那兩句話讀起來會像同一件事——他於是去修一顆沒有壞的資料庫，而她其實
  // 好好地正在起來。
  //
  // 上一版只補了 `await` 那一瞬間（catch 那條路），而**這 25 秒是同一個 bug
  // 比較大的那一半**，那時候還寫著「已經修好了」。
  //
  // **「是不是在講她」由寫的人帶進來，這裡不猜。** alpha.41 那一版在這裡寫
  // `starting || booting`，而那個推論在 `booting` 那幾分鐘是反的——那一段的
  // 完整重現寫在 [`notice`] 上面。這裡只讀 `aboutHer`。
  //
  // 前綴不寫成「剛剛那一下：」，雖然那才是 [`notice`] 的定義。因為他**剛剛那
  // 一下正好就是按了「叫她起來」**，那五個字會被讀成在講那一下——在唯一需要它
  // 的狀態下最模糊。「另一件事」講的是關係（跟上面那句無關），不是時序。
  let detail = "";
  if (masterStopHasHeadline) {
    const resume =
      masterStopPhase === "uncertain"
        ? "狀態讀不到；可從系統匣按「解除全停」嘗試重設"
        : masterStopPhase === "stopping"
          ? "排乾完成前不會宣稱三層已停；要恢復新工作，請從系統匣按「解除全停」"
          : "要恢復，請從系統匣按「解除全停」";
    detail = notice === null ? resume : `${resume}\n${notice.text}`;
  } else if (notice !== null) {
    detail =
      (starting || booting || thinkingLast) && !notice.aboutHer
        ? `這是另一件事：${notice.text}`
        : notice.text;
  } else if (recorderSupervisor?.message) {
    // supervisor view 是持續狀態，不借 `notice`。後者講的是使用者剛按的那一下，
    // 應該永遠先被看見；輪詢／事件只更新這一格，不能把那句回條擦掉。
    detail = recorderSupervisor.message;
  } else if (
    !starting &&
    !booting &&
    !thinkingLast &&
    shown === "asleep" &&
    recorderSupervisor?.phase !== "running"
  ) {
    detail = asleepDetail();
  }
  stateLine.textContent = detail === "" ? line : `${line}\n${detail}`;
  // 讀螢幕的人也要知道她在忙，不然「想一下…」只是給看得見的人看的。角色名
  // 只加在 label，不改狀態那一格的事實；能點時也說明這顆 button 會做什麼。
  const tapHint = personaEnabled && personaTapLines ? "；按下會說一條固定台詞" : "";
  avatar.setAttribute("aria-label", `${activeProfile.alias}（AI-Sister）：${line}${tapHint}`);

  if (wakeButton) {
    // 只在真的沒人在錄的時候出現。暫停中不出現——那時候的下一步是按 ▶，
    // 而不是再開一個 recorder（那會變成兩個行程各錄一份）。
    //
    // `booting` 也不出現，而且理由是同一個：那幾分鐘目錄已經有人佔著，按下去
    // 撞的是 `start_recording` 那道 `is_occupied` 閘門。
    const phase = recorderSupervisor?.phase;
    wakeButton.textContent =
      phase === "stop-undelivered"
        ? "再送一次停止要求"
        : phase === "gave-up"
          ? "再試一次"
          : phase === "backoff"
            ? "現在重試"
            : "開始記錄";
    const stopCanBeResent = phase === "stop-undelivered";
    wakeButton.hidden = stopCanBeResent
      ? false
      : shown !== "asleep" ||
        masterStopPhase !== "clear" ||
        starting ||
        // Backoff／GaveUp 會刻意壓掉舊 heartbeat 的現在式，但 occupancy gate 仍會
        // 擋住它。顯示不能採信那顆拍說「在聽」，操作也不能假裝它已經不佔位。
        recording ||
        booting ||
        thinkingLast ||
        phase === "starting" ||
        phase === "running" ||
        phase === "cooling" ||
        phase === "external" ||
        phase === "stopping-external" ||
        phase === "uncertain" ||
        !recordingStateKnown ||
        !recordingStateReadable ||
        !recorderSupervisorStateKnown ||
        phase === "quitting";
  }
}

function setState(next) {
  if (!STATES.includes(next)) return;
  state = next;
  paint();
  paintConversation();
}

function setPaused(next) {
  const was = paused;
  paused = next === true;
  // 從系統匣（或熱鍵）切過來的那一下，也算「下一件事發生了」。見
  // [`overtakenByEvents`]——這一格漏掉的時候，「暫停鍵沒有作用」會掛在
  // 一個已經暫停了的桌面姊妹底下。
  if (was !== paused) overtakenByEvents();
  if (pauseButton) {
    // 圖示不在這裡換。兩個狀態的 SVG 都在按鈕裡，由底下那行 `aria-pressed`
    // 經 CSS 選一個顯示——寫 `textContent` 會把 SVG 整個洗掉，而且會讓「現在
    // 是哪個狀態」同時存在字形和 aria 兩份。
    pauseButton.title = paused ? "繼續記錄" : "暫停記錄";
    // 「按下去了」= 暫停中。CSS 會把沒按下的那顆調淡，所以暫停時它最亮——
    // 這正是我們要的：不正常的狀態要吵。
    pauseButton.setAttribute("aria-pressed", String(paused));
  }
  paint();
}

function normalizeMasterStopPhase(next) {
  if (MASTER_STOP_PHASES.includes(next)) return next;
  if (next === true) return "stopped";
  if (next === false) return "clear";
  return "uncertain";
}

function setMasterStopPhase(next) {
  const was = masterStopPhase;
  masterStopPhase = normalizeMasterStopPhase(next);
  if (masterStopPhase !== "clear" && was !== masterStopPhase) {
    // native admission 保到 Answer 回傳為止；但 Promise continuation 還沒畫答案、
    // 還沒決定 Azure 自動朗讀。全停事件若插在這個縫裡，讓既有 ask generation
    // 當場失效，晚答案連 render 都不能進，更不能在 Stopped 之後才 POST。
    asking += 1;
    pendingAzureAutoAsk = null;
    slowNote = null;
    stopPersonaMedia();
    if (state === "thinking") state = "idle";
    // Gatekeeper 是 brain 的產品寫入／主動說話面。外部 CLI stop 沒有 renderer
    // event 時由 poll 補上；一旦觀察到非 clear，舊 poll 失效、卡片立即撤掉。
    gatekeeperReadRequest += 1;
    latestGatekeeperView = null;
    gatekeeperReadError = null;
    renderGatekeeper(null);
  }
  if (was !== masterStopPhase) overtakenByEvents();
  paint();
  paintConversation();
  // 冷啟動的 visible poll 可能發生在 master-stop read 還是 checking 時；那一輪
  // Gatekeeper 刻意不進。第一次確定 clear（或解除全停）就在這裡補問，不必等
  // 下一個五秒 tick，也不會在 stopping／uncertain 下寫 utterance 帳。
  if (masterStopPhase === "clear" && was !== "clear") readGatekeeper();
}

function readMasterStopState() {
  if (invoke === null) return;
  const revisionWhenStarted = masterStopRevision;
  const request = ++masterStopReadRequest;
  invoke("master_stop_state").then(
    (next) => {
      if (masterStopRevision === revisionWhenStarted && request === masterStopReadRequest) {
        setMasterStopPhase(next);
      }
    },
    () => {
      if (masterStopRevision === revisionWhenStarted && request === masterStopReadRequest) {
        setMasterStopPhase("uncertain");
      }
    },
  );
}

/**
 * 心跳說什麼：`"recording"`／`"booting"`／`"thinking"`／`"none"`（見後端的
 * `recording_state`）。
 *
 * **收五個字串，不是一個布林。** `"unreadable"` 和認不得的值都保留成讀不懂，
 * 不能折成 `"none"`。`"thinking"` 是錄製已停、腦還在想最後一段：說「在聽」
 * 是謊，說「沒有人在記錄」配一顆開始鍵也是謊。
 */
function setRecording(next, observed = true) {
  const was = recording;
  const wasBooting = booting;
  const wasThinking = thinkingLast;
  if (observed) {
    recordingStateKnown = true;
    recordingStateReadable = ["recording", "booting", "thinking", "none"].includes(next);
  }
  // 讀不懂時撤掉證據，但保留上一個 shape，等下一份可讀回覆才判斷是否真的
  // transition。否則 event 刻意撤證再讀回同一態，也會被誤當成下一件事，擦掉
  // 使用者剛按完的 one-shot notice。
  if (!observed || recordingStateReadable) {
    recording = next === "recording";
    booting = next === "booting";
    thinkingLast = next === "thinking";
  }
  // 她從別的地方被開起來、或是自己停掉了：一樣是「下一件事發生了」。見
  // [`overtakenByRecordingChange`]。問答／時間軸這些無關回條不跟著消失。
  if (was !== recording || wasBooting !== booting || wasThinking !== thinkingLast) {
    overtakenByRecordingChange();
  }
  // 她起來了（或是從別的地方被開起來的），那個「正在叫她」的等待就結束了，
  // 而「上一次叫不起來」也就過期了——她現在人在這裡，那句話再留著只會嚇人。
  //
  // **`booting` 也算數。** 上一版只認 `recording`，於是那 25 秒的等待在一顆
  // 開得慢的資料庫上一定走到逾時那一句「還沒有心跳」——而心跳就在磁碟上，是
  // 這一支自己看不見它。
  if (recording || booting) starting = false;
  // 剛剛才停下來：現在才有一場「上一次」可以講，而它跟開場時讀到的那一場
  // 不是同一場。停了才問，因為在錄的時候問到的會是**這一場**（沒有收尾），
  // 而畫面會把它讀成「她當掉了」。
  //
  // **想最後一段不算停完。** 那一場的 `end_session` 還沒寫。這時候去問
  // 「上一次」會拿到正在收尾的這一場，讀成「沒有好好結束」。
  const fullyStopped = recordingStateReadable && !recording && !booting && !thinkingLast;
  const wasFullyStopped = !was && !wasBooting && !wasThinking;
  if (!wasFullyStopped && fullyStopped) refreshLastRun();
  paint();
}

/**
 * 每次 IPC 都有世代；較早的慢回應不能蓋掉 supervisor event 之後重問的新真相。
 * 失敗也不是「沿用上一拍」：heartbeat 會過期，舊的 recording=true 不能永久
 * 替現在作證。
 */
async function readRecordingState() {
  if (invoke === null) return null;
  const request = ++recordingStateReadRequest;
  try {
    const next = await invoke("recording_state");
    if (request === recordingStateReadRequest) setRecording(next);
    return next;
  } catch (error) {
    if (request === recordingStateReadRequest) {
      recordingStateKnown = false;
      recordingStateReadable = false;
      paint();
    }
    throw error;
  }
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
 *
 * 而上面那段承諾要到這一版才是真的：`recording_state` 以前回 `is_recording`，
 * 把那個開機心跳過濾掉了，於是這個迴圈在一顆一年份的資料庫上**每一次**都走到
 * 逾時——一句「等了 25 秒還沒有心跳」，印在一個從第一秒就在的心跳旁邊。
 */
const WAKE_POLL_MS = 400;
const WAKE_TIMEOUT_MS = 25000;

async function startRecording() {
  if (invoke === null || starting) return;
  starting = true;
  // 這一次的結果還沒出來，上一次的判斷就不算數了。
  notice = null;
  paint();
  try {
    await invoke("start_recording");
  } catch (err) {
    // 同意書沒簽、找不到 sister.exe、已經有一個在跑——這三句都是後端寫好的
    // 完整句子，直接放上去。
    //
    // **放進 `notice`，不是直接寫那一格。** 底下那條逾時路徑早就是這樣寫的，
    // 系統匣那條（`recorder-failed`）也是；只有這裡還在直接寫，於是 5 秒後的
    // 輪詢把它蓋回「沒有人在記錄」、還順手把「開始記錄」那顆按鈕放回來——畫面
    // 和他根本沒按過逐像素相同。[`notice`] 那個欄位上面的註解講的就是這件事，
    // 而這一條是它漏掉的那個收件人。
    //
    // 直接指派（而不是「只有在還空著的時候才寫」）：上面開頭是清過的，但那次
    // 清距離這裡隔著一整段 `await`，中間他問一題失敗就會再填一句進去。這一句
    // 才是他此刻在等的答案。
    noticeAboutHer(err?.message ?? err, true);
    starting = false;
    paint();
    return;
  }
  const deadline = Date.now() + WAKE_TIMEOUT_MS;
  while (starting && Date.now() < deadline) {
    await new Promise((done) => setTimeout(done, WAKE_POLL_MS));
    // `setRecording(true)` 會把 `starting` 關掉，迴圈自己就結束了。
    try {
      await readRecordingState();
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
  const unreadable = recordingStateKnown && !recordingStateReadable;
  const statusUnknown = !recordingStateKnown;
  const timeoutHeadline = unreadable
    ? `等了 ${waited} 秒仍讀不懂 recording.beat`
    : statusUnknown
      ? `等了 ${waited} 秒仍讀不到 recorder 狀態`
      : `等了 ${waited} 秒還沒有心跳`;
  noticeAboutHer(
    why
      ? `${timeoutHeadline}。record.log 最後說：\n${why}`
      : `${timeoutHeadline}，record.log 也還是空的。` +
          (unreadable || statusUnknown
            ? "現在不能確認她是否正在記錄——請先看那個檔案"
            : "她可能還在起來——再等一下，或去看那個檔案"),
    true,
  );
  starting = false;
  paint();
}

async function runWakeAction() {
  if (recorderSupervisor?.phase !== "stop-undelivered") {
    return startRecording();
  }
  notice = null;
  paint();
  try {
    await invoke?.("stop_recording");
  } catch (err) {
    noticeAboutHer(err?.message ?? err, true);
    paint();
  }
}

wakeButton?.addEventListener("click", runWakeAction);

/**
 * 有沒有人在錄，隨時可能變——他會在另一個終端機視窗裡把 `sister record`
 * 開起來或按 Ctrl+C。所以要一直問，而且**問的節奏要跟得上心跳**
 * （`heartbeat::BEAT_EVERY_MS` 是 5 秒）。
 *
 * 視窗看不到的時候不問。一個整天掛在角落的東西，在沒有人看的時候還每 5 秒
 * 醒來一次，就是長期 CPU 目標的一筆固定成本——和動畫閘門同一條紀律。
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
 * 所以在終端機裡 `sister resume` 之後，她其實已經在錄了，而桌面姊妹會一直灰著
 * 說「已暫停，沒有在看」。更糟的是那顆 ▶：`toggle_pause` 讀的是磁碟，所以按
 * 下「繼續記錄」實際上是把她**暫停**——而畫面本來就畫成暫停的樣子，按完什麼
 * 都不會變。旁邊那句註解說得很清楚：顯示成已暫停、實際上還在錄，是這個產品
 * 能犯的最嚴重的一種謊。反過來這一種同樣是它。
 *
 * 磁碟上的旗標是真相，這個視窗只是鏡子——和 `ask` 每次重讀設定檔同一條紀律。
 */
function pollRecording() {
  if (invoke === null) return;
  void readRecordingState().catch(() => {});
  readRecorderSupervisor();
  invoke("pause_state").then(setPaused, () => {});
  // CLI 也能改 master.stop；renderer 只是鏡子，不能只相信這個 desktop 發的 event。
  readMasterStopState();
  // 守門員也要一直問下去。**只在開場問一次的話，五點才到期的那張承諾
  // 永遠不會被看到**——而 a 類（顯式時間承諾）正是整個 Phase 5 冷啟動期
  // 唯一放行的兩類之一，它不動就等於守門員沒上線。
  //
  // 每 5 秒問一次不會把預算燒掉：後端那一側同一件事今天只記一次帳，
  // 已經開口而人還沒回應的那一句是繼續顯示、不重扣。理由寫在
  // `main.rs` 的 `gatekeeper_check` 上面。
  readGatekeeper();
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
 * 時候還在跑 compositor，那就是長期 CPU 目標的直接漏洞。
 */
function updateMotionGate() {
  const allowed =
    personaEnabled &&
    personaMotion &&
    document.visibilityState === "visible" &&
    reducedMotion?.matches !== true;
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
  // 同 `setPaused`：實心／空心兩顆 SVG 都在按鈕裡，`aria-pressed` 決定顯示哪一顆。
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
  notice = null;
  try {
    await invoke?.("open_timeline");
    paint();
  } catch (err) {
    // 時間軸開不起來和她起不起得來是兩件事——她正在起來的時候這一句要自己
    // 帶主詞，不然會被讀成「她就是因為這個沒起來」。
    noticeAboutSomethingElse(err?.message ?? err);
    paint();
  }
});

settingsButton?.addEventListener("click", async () => {
  try {
    await invoke?.("open_settings");
  } catch (err) {
    noticeAboutSomethingElse(err?.message ?? err);
    paint();
  }
});

pauseButton?.addEventListener("click", async () => {
  notice = null;
  if (invoke === null) {
    setPaused(!paused);
    return;
  }
  try {
    setPaused(await invoke("toggle_pause"));
  } catch (err) {
    // 切不動就**不要**改畫面。顯示成已暫停、實際上還在錄，是這個產品能犯的
    // 最嚴重的一種謊；寧可看起來沒反應，然後把原因寫出來。
    //
    // 「寫出來」要寫進 `notice`：直接寫那一格的話，下一輪輪詢（5 秒內，而且
    // 這顆按鈕本身不會重設那個計時器，所以可能是 0 秒）會把它蓋掉，留下的
    // 剛好只有前半句「看起來沒反應」。
    noticeAboutSomethingElse(err?.message ?? err, true);
    paint();
  }
});

/**
 * 系統匣上也有同一顆暫停鍵，所以狀態可能從**這個視窗以外**改變。
 * 沒有這一段的話，從系統匣暫停之後，桌面姊妹會繼續一臉「我在聽」。
 */
globalThis.__TAURI__?.event
  ?.listen?.("pause-changed", (event) => setPaused(event.payload))
  ?.catch?.(() => {});

globalThis.__TAURI__?.event
  ?.listen?.("master-stop-changed", (event) => {
    masterStopRevision += 1;
    setMasterStopPhase(event.payload);
  })
  // 開場 read 和 listener 真正 ready 中間，CLI 可能剛好切了 durable latch；那一個
  // event 沒有 listener 可收。註冊完成後重讀一次磁碟，才封得住這個缺口。
  ?.then?.(() => readMasterStopState())
  ?.catch?.(() => {});

globalThis.__TAURI__?.event
  ?.listen?.("master-stop-failed", (event) => {
    noticeAboutHer(event.payload, true);
    paint();
  })
  ?.catch?.(() => {});

/**
 * worker transition 不等下一輪五秒 polling。特別是第四次失敗的 GaveUp：那一刻
 * 「再試一次」要立即出現，不能讓舊的「正在等候重試」多活五秒。
 */
const recorderSupervisorListener = globalThis.__TAURI__?.event?.listen?.(
  "recorder-supervisor-changed",
  (event) => {
    recorderSupervisorRevision += 1;
    // transition 與上一輪 heartbeat 不是同一個 snapshot。先撤銷舊證據，再立刻
    // 重讀；正常收工 event 絕不能和五秒前的 recording=true 湊成「在聽」。
    recordingStateKnown = false;
    recordingStateReadable = false;
    receiveRecorderSupervisor(event.payload);
    void readRecordingState().catch(() => {});
  },
);
recorderSupervisorListener?.then?.(
  () => {
    // `listen` 本身是 async：開場兩份 IPC 可能先讀到 stopped/none，接著 worker
    // 在 listener 真正註冊前切成 running。那個 event 已經丟了，不能讓舊 snapshot
    // 一直留到五秒 polling。註冊完成後撤掉舊證據，再補讀同一對狀態。
    recordingStateKnown = false;
    recordingStateReadable = false;
    recorderSupervisorStateKnown = false;
    recorderSupervisor = null;
    paint();
    readRecorderSupervisor();
    void readRecordingState().catch(() => {});
  },
  () => {},
);

/** 設定頁是另一扇 WebView；存成功後立即換成本機 persona，不等整支程式重開。 */
globalThis.__TAURI__?.event
  ?.listen?.("persona-changed", (event) => {
    personaRevision += 1;
    personaSelectionKnown = true;
    if (!applyPersona(event.payload)) readPersona();
  })
  ?.catch?.(() => {});

/**
 * 刪除素材的第一步，不等檔案真的刪完，也不依賴 config.toml 還讀得出來。
 *
 * 後端隨後仍會送完整的 `persona-changed`；這個窄事件只負責 fail closed：讓飛行中
 * 的單條 WAV 回來也失效、停掉正在播的固定錄音或系統語音，並保留 activeProfile
 * 的內建角色圖。它不改 persona 選擇，更不碰 ask／Gatekeeper／hands。
 */
globalThis.__TAURI__?.event
  ?.listen?.("persona-media-stop", () => {
    personaRevision += 1;
    clearPersonaLine();
    localAssets = fallbackLocalAssets();
    avatar.dataset.assetPack = localAssets.phase;
    paintPersonaPortrait();
  })
  ?.catch?.(() => {});

// 設定頁與第四張同意書只送「狀態變了」；這一扇窗重新讀 native 真相。事件本身
// 不帶答案、不播放，也不會啟動 Azure request。
const azureTtsChangedListener = globalThis.__TAURI__?.event?.listen?.(
  "azure-tts-changed",
  () => {
    // 這是設定／credential／同意書 mutation 的通知，不是某一題答案完成的證據。
    // 即使剛好有一題在等冷啟動 read，也不能在事後開啟或重簽時補送那份舊答案。
    pendingAzureAutoAsk = null;
    readAzureTts();
  },
);
azureTtsChangedListener?.then?.(
  () => {
    // listen 本身是 async：開場 read 與真正註冊之間若剛好換設定，event 會丟掉。
    // 這段空窗無法證明沒有發生 mutation；先丟掉尚未授權的舊答案，再補讀。
    // 否則簽名前完成的答案可能借到補讀才看見的新同意。
    pendingAzureAutoAsk = null;
    readAzureTts();
  },
  () => {},
);

// 關開關、刪金鑰或撤回第四張時立即停止播放／丟掉晚回應。後端同時把 generation
// 推進；已開始的 blocking POST 仍可能跑到 timeout，所以畫面不宣稱 socket 已中止。
globalThis.__TAURI__?.event
  ?.listen?.("azure-tts-stop", () => {
    pendingAzureAutoAsk = null;
    stopPersonaMedia({ cancelAzureNative: false });
    readAzureTts();
  })
  ?.catch?.(() => {});

/**
 * 拔手熱鍵按下去之後那一句。
 *
 * 這條路存在的理由和熱鍵本身一樣：**她不在畫面上的時候他也要按得到。** 而
 * 「按到了沒」只有兩個地方看得出來——系統匣那兩行字（要點開選單才讀得到）
 * 和這裡。少了這一行，按下去的後果是一個字都沒有：桌面姊妹視窗被叫出來，然後
 * 什麼都沒說。
 *
 * 為什麼不借 `recorder-failed`：那一句**多半不是失敗**（最常見的結果是手真的
 * 拔掉了），借那個名字會讓事件名自己說謊，而下一個讀這個檔案的人只看得到名字。
 *
 * 為什麼是 `noticeAboutHer`：這一句從頭到尾都在講她——她的手拔掉了沒、她還
 * 會不會把東西交給作業系統。整句話是後端算好的（`kill_switch::hands_hotkey_message`），
 * 這裡不加工，因為那四種結局的差別正是那句話本身。
 */
globalThis.__TAURI__?.event
  ?.listen?.("hands-pulled", (event) => {
    noticeAboutHer(event.payload);
    paint();
  })
  ?.catch?.(() => {});

/**
 * 從系統匣按「開始記錄」失敗的時候，那句原因沒有地方可以寫。
 *
 * 後端把開始／停止記錄失敗的完整中文放進 payload，而系統匣選單上沒有一格
 * 能放字。拔手結果走 `hands-pulled`，因為它不能被無關的 recording transition 淘汰。
 * 以前記錄開關失敗只進 `desktop.log`：按下去的
 * 後果是**什麼都沒發生**，而唯一說得出原因的那句話在一個他不會開的檔案裡。
 *
 * 寫進 [`notice`]，因為那是**每一個狀態下都出得了聲**的那一格。
 *
 * 這裡以前借的是「叫不起來」那條路（`wakeFailed`），理由寫著「對他來說是同一
 * 件事：她沒起來，這是為什麼」。那句話是假的：系統匣那一格是**開關**（見
 * `main.rs` 的 `record_label`），她在錄的時候按下去送的是 `stop_recording`，
 * 所以這裡也會收到「找不到資料目錄，停不了」——而那一刻 `wakeFailed` 在
 * `paint()` 那道 `shown === "asleep"` 閘門後面，一個字都不顯示。後端剛剛才特地
 * `win.show()` + `set_focus()` 把視窗叫到他面前，然後那一格什麼都沒多。
 *
 * 一段解釋為什麼可以不分的註解，就是那裡沒分過。
 */
globalThis.__TAURI__?.event
  ?.listen?.("recorder-failed", (event) => {
    // 直接指派就把上一句蓋掉了：一句早上留下的話不可以擋住這一句。
    //
    // 這個 event 只回答記錄開關，所以真的 recording transition 可以淘汰它。
    noticeAboutHer(event.payload, true);
    // **這裡以前還會 `starting = false`，而那一下是在說謊。** 他按了「叫她
    // 起來」、等不及又從系統匣按了一次：那一刻心跳還沒蓋出來，
    // `recording_state` 還是 `"none"`，所以那一顆走 `start_recording`、撞上
    // `spawned.try_wait()` 回 `Ok(None)`，回一句「上一次按的那個還在起來
    // ——…再等一下」。第一次那個 wake **還在飛**，而這一行把它記成沒了：
    //
    //     沒有人在記錄——從現在起發生的事，她不會知道
    //     上一次按的那個還在起來——第一次開資料庫要重建索引…再等一下
    //
    // 兩行都是真的，而它們直接互相矛盾。附帶那顆「叫她起來」會跟著跳回來，
    // 他再按一次拿到同一句話——正是 `booting` 那個三態當初要消滅的迴圈。
    //
    // `starting` 的生死歸 wake 自己那一圈管：心跳出現（`setRecording`）、
    // spawn 被 reject（catch）、25 秒到了（逾時）。從系統匣按壞的另外一下，
    // 不是那三件事裡的任何一件。
    paint();
  })
  ?.catch?.(() => {});

// ---------- 答案 ----------

const hitList = document.querySelector("[data-hits]");

/**
 * 底下那個 `<ul>` 現在裝的是不是**一份答案**（而不是一句錯誤，或者什麼都沒有）。
 *
 * 只有一個讀者：問題答不成的時候那句「底下原本那幾筆是上一題的，先收起來了」。
 * 那後半句是在描述它剛剛做掉的事，而那件事不一定發生過——見 [`ask`] 的 catch。
 */
let showingAnswer = false;

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

function clock(ts) {
  const d = new Date(ts);
  const pad = (n) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 和終端機 `fmt::duration_ms`、時間軸 `lasted` 同一套：無條件捨去。 */
function lasted(ms) {
  if (ms < 60_000) return `${Math.floor(ms / 1000)} 秒`;
  if (ms < 3_600_000) return `${Math.floor(ms / 60_000)} 分鐘`;
  const h = Math.floor(ms / 3_600_000);
  const m = Math.floor((ms % 3_600_000) / 60_000);
  return m === 0 ? `${h} 小時` : `${h} 小時 ${m} 分`;
}

function chapterHit(ch, speak = true) {
  const li = document.createElement("li");
  li.className = "hit chapter";
  const whenEl = document.createElement("p");
  whenEl.className = "chapter-when";
  if (speak) whenEl.dataset.azureAnswerBody = "";
  // 答案講的是核心時間。start_ts／end_ts 在時間軸上含 5 秒 margin，
  // 相加會把相鄰段的邊界算兩次。
  const start = ch.core_start_ts ?? ch.start_ts;
  const end = ch.core_end_ts ?? ch.end_ts;
  const durMs =
    typeof ch.core_ms === "number" ? ch.core_ms : Math.max(0, end - start);
  const dur = lasted(Math.max(0, durMs));
  const n = ch.segment_count;
  const howLong =
    typeof n === "number" && n > 1 ? `${dur}，${n} 段併成` : dur;
  whenEl.textContent = `${clock(start)}–${clock(end)}　${howLong}`;
  const what = document.createElement("p");
  what.className = "chapter-what";
  const visibleTitle = ch.title || ch.host;
  if (ch.app) {
    const app = document.createElement("span");
    app.className = "chapter-source-app";
    app.textContent = ch.app;
    what.append(app);
    if (visibleTitle) what.append(document.createTextNode(" · "));
  }
  if (visibleTitle) {
    const title = document.createElement("span");
    title.className = "chapter-answer-title";
    title.textContent = visibleTitle;
    // Window title 是這段答案的主句；host 只是沒有 title 時的出處 fallback，
    // 畫面照常顯示但不因 Azure click 出境。
    if (speak && ch.title) title.dataset.azureAnswerBody = "";
    what.append(title);
  }
  if (!ch.app && !visibleTitle) {
    const fallback = document.createElement("span");
    if (speak) fallback.dataset.azureAnswerBody = "";
    fallback.textContent = "一段紀錄";
    what.append(fallback);
  }
  li.append(whenEl, what);
  return li;
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
  const out = [];
  // 讀字斷掉要**單獨先問**，不能掛在 chunks === 0 底下。
  //
  // 那一支本來寫在下面那個 if 裡，於是它守的是「一段字都沒有，而且看過畫面」。
  // 可是 OCR 全死的機器上 chunks 不是 0：`insert_focus` 每次換視窗就寫一列
  // 視窗標題、一列網址進 text_chunks，兩種都不經過 OCR。真正壞掉的那台機器
  // 於是掉到最後那句「我記得的東西裡沒有這件事」，和一台一切正常、那件事真的
  // 沒發生過的機器一模一樣——而這是這個專案已知的主要故障形狀。
  //
  // 提早收工的理由和舊版一樣：畫面明明留下來了，暫停和排除都解釋不了「這幾張
  // 畫面上沒有字」。門檻（幾張畫面才算數）在 Rust 那邊，這裡只讀結論。
  if (blind.ocr_is_dead) {
    return [`（我看過 ${blind.frames} 張畫面，但一個字都沒讀出來——讀字那一段是斷的。）`];
  }
  if (blind.chunks === 0) {
    // 「連畫面都沒有」有**四種**，而這裡以前只講得出一種：從頭暫停到尾
    // 的那一小時、被一條排除規則整段擋掉的那一小時，走到的都是同樣這組
    // 數字，然後被告知「被忘掉了，或是過了保留期」——四個裡唯一假的那個，
    // 也是唯一一個會讓他以為東西被刪了的。底下 excluded / paused 兩段本來
    // 就會講出真正的原因，所以這裡不再提早 return。
    const blocked =
      blind.paused_episodes > 0 ||
      blind.master_stopped_episodes > 0 ||
      blind.excluded?.length > 0;
    if (blind.frames > 0) {
      // 上面那道 ocr_is_dead 已經把「夠多張畫面、一行字都沒有」攔走了，所以
      // 走到這裡的是張數還太少的時候。三張畫面上剛好都沒有字是完全正常的事
      // ——這裡不指控 OCR。
      out.push(`（我留下了 ${blind.frames} 張畫面，但還沒有任何一段字——多半是才剛開始。）`);
    } else if (blind.ever_recorded && blocked) {
      out.push("（我錄過，但那段時間一張畫面都沒留下來——底下是我查得出來的原因。）");
    } else if (blind.recording_now && !blind.ever_stored) {
      // **「我正開著」和「一列都沒存過」同時成立的那一格。** 底下那句攤開三
      // 種可能，而其中一種在這台機器上證得出來是假的：一列都沒進來過，就沒有
      // 東西可以被忘掉或過期。
      //
      // 這一格是上一版自己造出來的：`recording_now` 問在 `!ever_stored` 前
      // 面，於是後者永遠輪不到一台正在錄的機器，而前者那句話裡帶著一則它證得
      // 出來是假的指控。攤開可能性也要先把不可能的那幾種扣掉。
      out.push("（我正開著，可是到現在一列內容都還沒落地——多半是剛開始，再等一下。）");
    } else if (blind.ever_recorded && blind.recording_now) {
      // 她**正在**錄。「被忘掉了，或是過了保留期」少了一種可能，而且正好是
      // 最常見的那一種：他三秒前才按下「開始記錄」。第一次用的人問的第一個
      // 問題就落在這裡，然後被告知他的紀錄被忘掉了。
      //
      // 不挑一邊：清空過的資料庫上她照樣可能正在錄，那時候兩件事都成立。
      out.push("（我正開著，但手上一段字都沒有——可能是剛開始，也可能是之前的被忘掉了或過期了。）");
    } else if (blind.booting_now) {
      // **開機那幾分鐘。** `recording_now` 在這裡是 false（我一拍都還沒跑），
      // 所以這一格以前掉到底下那兩句：一台什麼都還沒開始的機器被送去看設定，
      // 或被告知東西被忘掉了。排在 `ever_*` 那幾條前面，因為它們講的是過去，
      // 而他問這一題的時候在等現在。
      out.push("（我正在起來（多半在開資料庫），還沒開始記東西——再等一下。）");
    } else if (blind.ever_recorded && !blind.ever_stored) {
      // 我跑過，而一列內容都沒進來過。底下那句「被忘掉了」在這台機器上是
      // 指控一件沒發生的事——他一次都沒刪過東西。四種空手變五種，而這第五
      // 種以前是被「被忘掉了」吃掉的。
      out.push("（我錄過，但一列內容都沒存進來過——跑 `sister doctor`，它會直接說是哪一段擋住了。）");
    } else if (blind.ever_recorded) {
      out.push("（我錄過，但現在什麼都不剩了——被忘掉了，或是過了保留期。）");
    } else {
      // 下一步只掛在這一條上。它是四種空手裡唯一一種「去按開始記錄」真的
      // 是對的答案的——被忘掉的、過保留期的、被規則擋掉的、OCR 讀不出來
      // 的，再錄一天都還是一樣。標題那一行以前把這句話對四種人一起講。
      //
      // 講按鈕不講指令：這一頁右上角就有那顆鍵（index.html 的 `.wake`），
      // 系統匣裡也有一個。叫一個開著視窗的人去找終端機，是在描述一個更早
      // 的版本。
      out.push("（我到現在還沒記過任何東西——右上角那顆「開始記錄」按下去我才開始。）");
    }
  }
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
  // 「我以前暫停過幾段」和「我**現在**閉不閉得了眼」是兩件事。這裡以前把它們
  // 串成一條 if/else 鏈，而「現在」那一句掛在 `else if`——只有一段暫停紀錄都
  // 沒有的時候才說得出口。錄的時候暫停又解除過一次、後來在沒人錄的時候又按了
  // 一次暫停的人，讀到的是「我也被暫停過 1 次，那幾段是空的。」：過去式，話說
  // 完了。而他此刻是瞎的，下一次按開始記錄會錄一整天的空白。
  //
  // `ops.rs` 的 `blind_lines` 是同一個 bug、同一個修法——那邊的 else 分支註解
  // 自己寫著「這一條比上面那條更需要講」，然後坐在會被擋掉的位置上。
  //
  // 這裡本來就不印時間，所以躲過了 `paused_ms` 那個「一共 0 秒」的坑。
  if (blind.paused_episodes > 0) {
    // 「最後那一段沒收尾」不是「我現在閉著眼睛」。暫停中關掉 recorder、事後才
    // 解除的人，`CaptureResumed` 沒有人寫，資料庫從此永遠掛著一段配不到對的
    // ——把它印成「現在」就是一則再也不會消失的假警報。只有旗標答得出「現在」。
    if (blind.paused_open) {
      out.push(
        `我也被暫停過 ${blind.paused_episodes} 次。最後那一段沒有收尾，所以那幾段其實比記下來的更長。`,
      );
    } else {
      out.push(`我也被暫停過 ${blind.paused_episodes} 次，那幾段是空的。`);
    }
  }
  if (blind.paused_now) {
    // 講的是**接下來**：再錄也記不到東西。所以它和上面那句同時成立，不是二選一。
    // 這一頁是 `textContent`，不是 markdown——`**粗體**` 會原樣印出星號。
    // 強調用字本身，不用符號。
    out.push(
      `${out.length ? "而且" : ""}我現在是暫停的（右上角那顆鍵）——這樣繼續錄也不會記到東西。`,
    );
  }
  // 稽核歷史和目前 latch 是兩個答案：recorder 不在跑時 engage／release 不會補
  // 稽核列，所以兩句可以同時成立，也可以只成立其中一句。不能用 else-if。
  if (blind.master_stopped_episodes > 0) {
    const why = [];
    if (blind.master_stopped_open) why.push("最後一段沒有收尾");
    if (blind.master_stopped_truncated > 0) {
      why.push(`有 ${blind.master_stopped_truncated} 段的開頭已被保留期刪掉`);
    }
    const howLong =
      blind.master_stopped_ms === 0 && why.length > 0
        ? `長度還算不出來（${why.join("、")}）`
        : why.length === 0
          ? `一共 ${lasted(blind.master_stopped_ms)}`
          : `算得出來的加起來 ${lasted(blind.master_stopped_ms)}（${why.join("、")}，所以這個數字算短了）`;
    out.push(
      `我也被全停過 ${blind.master_stopped_episodes} 次、${howLong}，那幾段 recorder、解釋層和手都停著。`,
    );
  }
  if (blind.master_stop_state === "stopped") {
    out.push(
      `${out.length ? "而且" : ""}我現在正全停中（系統匣 → 解除全停）——recorder、解釋層和手都不會工作。`,
    );
  } else if (blind.master_stop_state === "stopping") {
    out.push(
      `${out.length ? "而且" : ""}我正在完成全停——新工作已拒絕，先前開始的工作仍在排乾；完成前我不會說三層都停了。`,
    );
  } else if (blind.master_stop_state === "uncertain") {
    out.push(
      `${out.length ? "而且" : ""}我現在讀不到可靠的全停狀態——為安全起見不會開始新的 recorder、解釋或手部工作。`,
    );
  }
  // 「我找不到」和「我沒去找」是兩件事。每個詞都短到索引比不出來的問題
  // （一個中文字，或兩個以內的英數），只剩那條夾在 30 天內的掃描——而保留期
  // 預設 365 天。見 `BlindSpots::scan_horizon_days` 和 `covered_by_index`。
  //
  // 這裡以前寫「單獨一個字」，而後端的條件其實對**每一個純英數查詢**都成立。
  // 於是他打了 21 個字元的錯誤碼，讀到的是「這種問法（單獨一個字）」——一句
  // 在描述他沒打過的東西的話，附在一個其實看完了整顆資料庫的搜尋底下。
  if (blind.scan_horizon_days) {
    out.push(
      `——不過這種問法（每個詞都太短，我的索引比不出來）我只翻得動最近 ${blind.scan_horizon_days} 天，更早的這次沒翻到。多打一個字我就找得比較遠。`,
    );
  }
  return out;
}

/**
 * 答案底下那一個開關：「這一題我本來已經忘了」。
 *
 * PHASES.md Phase 1 的**第一條**退場條件是「自用 7 天內 ≥ 3 次答對我自己都忘掉
 * 的東西」，而那件事只有他知道。題庫記得住他問了什麼、她給了幾筆、他點開了哪
 * 一個出處——記不住他當時知不知道那個答案。
 *
 * **點開出處不是它。** 那件事最常發生在她答錯、或他在查核的時候。
 *
 * 而它補不回來：那是他看到答案那一刻腦袋裡的狀態，一個禮拜之後回頭翻題庫翻不
 * 出來。所以它是一個當下按的按鈕，不是一份事後的問卷——長得小、就在答案底下、
 * 按一下就好、按錯了再按一下收回。
 *
 * 失敗要說出來，這一點和 [`sourceLine`] 裡的 `log_click` 相反：那邊他要的是那
 * 張畫面，記不記得到帳是次要的；這邊他要的**就是**記這一筆。畫面裝作記進去了
 * 而其實沒有，等於在退場條件的證據上說謊——所以按鈕先不變，回來了才變。
 */
function markLine(queryId) {
  const li = document.createElement("li");
  li.className = "hits-note hits-mark";

  const button = document.createElement("button");
  button.type = "button";
  button.className = "mark-toggle";
  // 開發用：`?hits=demo&marked=1` 直接看按下去之後長什麼樣。**兩個狀態都要
  // 看得到版面**——按下去那一個字比較長、還多一顆星，而這一頁只有 340 像素
  // 寬。沒有這個開關的話，無頭瀏覽器那一遍只驗得到其中一半。
  let marked =
    invoke === null && new URLSearchParams(globalThis.location.search).get("marked") === "1";

  const paintButton = () => {
    button.textContent = marked ? "★ 記下來了：這件事你本來已經忘了" : "這件事我本來已經忘了";
    button.classList.toggle("on", marked);
    button.title = marked ? "再按一次收回" : "她答對了一件你早就忘掉的事？按一下記下來";
    button.setAttribute("aria-pressed", String(marked));
  };
  paintButton();

  button.addEventListener("click", async () => {
    const want = !marked;
    // **這也是一個「按下去的地方」，所以它也要自己清。** 理由寫在
    // [`overtakenByEvents`]：`notice` 講的是「他手指剛剛按下去的那一下」，
    // 而他現在按的是這一顆。（那份註解剛把「四個」這種數字拿掉了，理由是
    // 沒人會記得同步——所以這裡也不寫成第幾個。）
    //
    // 少了這一行最容易走到的那條路，是**同一顆按鈕失敗過一次、他再按一次**：
    // 第二次成功了，按鈕變成「★ 記下來了」，而上一次那句「這一次標記沒記進
    // 去：database is locked」還留在下面。畫面同時說記進去了和沒記進去——而
    // 驗收清單上「沒變色 = 沒記進去（會另外有一句話說為什麼）」正是靠這兩件
    // 事分得開才成立的。
    overtakenByEvents();
    // 這一下屬於**現在螢幕上這一題**。慢的那一次回來的時候他可能已經問了下
    // 一題（`ask()` 每次寫畫面之前都拿 `asking` 問同一件事），而那時候這句話
    // 會貼在一個他從來沒標過的答案底下。
    const mine = asking;
    // 連按的時候不要送出兩筆互相打架的請求。回來之前先關起來。
    button.disabled = true;
    try {
      // 在瀏覽器裡打開這一頁的時候要**講出來**，不是安靜地什麼都不做——那正
      // 是 `ask()` 對同一件事的做法。一顆按了沒反應的按鈕，和一顆按了有記進
      // 去的按鈕，在畫面上長得一模一樣。
      if (invoke === null) throw new Error("這一頁不是在 AI-Sister 裡打開的");
      // 照後端回的畫，不要照 `want` 畫——**寫進去了才算數**。
      //
      // 那個值就是傳過去的那個參數（後端沒有再讀一次表，見 `MarkOutcome`），
      // 所以它證明的是「這一次真的寫成功了」，不是「另一個視窗剛剛也改過」。
      // 上一版這裡寫著後者，而沒有任何東西提供它。
      marked = await invoke("mark_query", { queryId, marked: want });
      if (mine !== asking) return;
      paintButton();
      paint();
    } catch (err) {
      // 沒成就不要改樣子。加上一句話，不然「我按了，它沒反應」和「我按了，
      // 它記下來了」在畫面上一模一樣——而這一格正是拿來當證據的。
      if (mine !== asking) return;
      noticeAboutSomethingElse(
        `這一次標記沒記進去：${err?.message ?? err ?? "不知道為什麼"}`,
      );
      paint();
    } finally {
      button.disabled = false;
    }
  });

  li.append(button);
  return li;
}

function answerTextForLocalSpeech() {
  const grounded = [...hitList.querySelectorAll(".grounded-text")]
    .map((node) => node.textContent.replace(/\s+/g, " ").trim())
    .filter((line) => line !== "")
    .join(" ");
  if (grounded !== "") return grounded;
  const copy = hitList.cloneNode(true);
  for (const node of copy.querySelectorAll(
    ".hit-source, .hits-mark, .hits-read, .hits-cloud, button, a",
  )) {
    node.remove();
  }
  return copy.textContent.replace(/\s+/g, " ").trim();
}

/**
 * Azure 只收 renderHits 明確標成答案正文的節點。不能從整張清單做「排除幾個
 * class」：那種 denylist 一新增提示、日期說明、follow-up 或診斷句就會悄悄出境。
 * OCR／模型文字只用 textContent 放進既有節點，不能自行鑄出這個 data attribute。
 */
function azureAnswerText() {
  return [...hitList.querySelectorAll("[data-azure-answer-body]")]
    .map((node) => node.textContent.replace(/\s+/g, " ").trim())
    .filter((line) => line !== "")
    .join("\n");
}

function answerReadLine() {
  const li = document.createElement("li");
  li.className = "hits-read";
  const button = document.createElement("button");
  button.type = "button";
  button.className = "answer-read";
  button.textContent = "🔊 用本機聲音朗讀";
  button.addEventListener("click", async (event) => {
    if (event?.isTrusted !== true) return;
    // 這是新的播放意圖：就算最後找不到 localService voice，也要先停掉上一句
    // bundled Ogg／pending read，不能一邊說「沒有本機聲音」一邊繼續播舊台詞。
    stopPersonaMedia();
    if (!personaVoiceEnabled) {
      personaLine.textContent = "先到設定打開「本機聲音」，我才會朗讀。";
      personaLine.hidden = false;
      return;
    }
    const text = answerTextForLocalSpeech();
    if (text === "") return;
    if (invoke === null) {
      if (speakWithLocalSystemVoice(text)) return;
    } else {
      const localIntent = localSpeechRevision;
      let presentation;
      try {
        presentation = await invoke("answer_local_speech_admit");
        const allowed = await beginNativePresentation(presentation);
        if (
          !allowed ||
          localIntent !== localSpeechRevision ||
          masterStopPhase !== "clear"
        ) {
          releaseNativePresentation(presentation);
          if (localIntent === localSpeechRevision) readMasterStopState();
          return;
        }
        if (speakWithLocalSystemVoice(text, presentation)) return;
        releaseNativePresentation(presentation);
      } catch (error) {
        releaseNativePresentation(presentation);
        if (localIntent !== localSpeechRevision || masterStopPhase !== "clear") return;
        personaLine.textContent = "本機朗讀未開始。解除全停後再重播。";
        personaLine.hidden = false;
        readMasterStopState();
        return;
      }
    }
    personaLine.textContent = "找不到本機中文語音。請先在 Windows 安裝中文語音。";
    personaLine.hidden = false;
  });
  li.append(button);
  return li;
}

async function speakAzureAnswer(button, intent) {
  if (intent !== AZURE_AUTO_ANSWER && intent !== AZURE_TRUSTED_REPLAY) return;
  if (azureCancelPending) {
    personaLine.textContent = "Azure 正在停止上一段朗讀。完成後再重播。";
    personaLine.hidden = false;
    return;
  }

  // 新答案由第四張同意 + 明確設定授權；手動重播則另外要 trusted
  // click。兩條路只共用這一個 outbound 出口：先停舊播放，再只抽當前正文。
  stopPersonaMedia();
  if (invoke === null) {
    personaLine.textContent = "請從 AI-Sister 桌面程式使用 Azure 朗讀。";
    personaLine.hidden = false;
    return;
  }
  const text = azureAnswerText();
  if (text === "") {
    personaLine.textContent = "這一題沒有可朗讀的正文。";
    personaLine.hidden = false;
    return;
  }
  const nativeExpected = azureNativeExpected;
  const nativeGeneration = nativeExpected?.generation;
  if (
    !Number.isSafeInteger(nativeGeneration) ||
    nativeGeneration < 0 ||
    nativeGeneration !== azureNativeGeneration
  ) {
    personaLine.textContent = "Azure 狀態更新中。更新後再重播。";
    personaLine.hidden = false;
    readAzureTts();
    return;
  }

  const revision = azureSpeechRevision;
  azureSpeechRequestPending = true;
  azurePendingGeneration = nativeGeneration;
  azureAnswerButton = button;
  button.textContent = "■ 停止／取消 Azure 朗讀";
  let audio;
  try {
    audio = await invoke("azure_tts_speak", {
      text,
      expected: nativeExpected,
    });
  } catch (err) {
    if (revision !== azureSpeechRevision) return;
    azureSpeechRequestPending = false;
    azurePendingGeneration = null;
    resetAzureAnswerButton();
    personaLine.textContent = "Azure 朗讀失敗。請檢查語音設定後重播。";
    personaLine.hidden = false;
    readAzureTts();
    return;
  }
  if (revision !== azureSpeechRevision) {
    releaseNativePresentation(audio);
    return;
  }
  azureSpeechRequestPending = false;
  azurePendingGeneration = null;
  if (
    !Number.isSafeInteger(audio?.generation) ||
    audio.generation !==
      (nativeGeneration >= Number.MAX_SAFE_INTEGER ? 0 : nativeGeneration + 1) ||
    audio?.content_type !== "audio/mpeg" ||
    !Number.isSafeInteger(audio?.audio_bytes) ||
    audio.audio_bytes < 1 ||
    audio.audio_bytes > 8 * 1024 * 1024 ||
    typeof audio?.data_url !== "string" ||
    !audio.data_url.startsWith("data:audio/mpeg;base64,") ||
    audio.data_url.length > 12 * 1024 * 1024 ||
    typeof audio?.presentation_id !== "string" ||
    audio.presentation_id === ""
  ) {
    releaseNativePresentation(audio);
    resetAzureAnswerButton();
    personaLine.textContent = "Azure 回傳的音訊無法播放。請再重播一次。";
    personaLine.hidden = false;
    readAzureTts();
    return;
  }
  // Native 每 admitted 一個 request 就消耗一代。只有精確 +1 的回應能成為下一次
  // 朗讀意圖的 snapshot；其餘欄位仍是這次 native 已逐格比對過的原始狀態。
  azureNativeGeneration = audio.generation;
  azureNativeExpected = Object.freeze({
    ...nativeExpected,
    generation: audio.generation,
  });
  if (!personaAudio || typeof personaAudio.play !== "function") {
    releaseNativePresentation(audio);
    resetAzureAnswerButton();
    personaLine.textContent = "這個視窗無法播放音訊。請重開 AI-Sister。";
    personaLine.hidden = false;
    return;
  }
  let presentationAllowed;
  try {
    presentationAllowed = await beginNativePresentation(audio);
  } catch (err) {
    releaseNativePresentation(audio);
    resetAzureAnswerButton();
    personaLine.textContent = "Azure 音訊未開始播放。請重開 AI-Sister 後重播。";
    personaLine.hidden = false;
    readMasterStopState();
    return;
  }
  if (revision !== azureSpeechRevision) {
    releaseNativePresentation(audio);
    return;
  }
  if (!presentationAllowed) {
    releaseNativePresentation(audio);
    resetAzureAnswerButton();
    personaLine.textContent = "Azure 音訊未開始播放。解除全停後再重播。";
    personaLine.hidden = false;
    readMasterStopState();
    return;
  }
  // begin 成功後 guard 不能像答案文字那樣在同步 render 後立刻 end：播放本身
  // 還在 WebView 的本機 media pipeline 裡。ended／error／Stop 才是這份 activity
  // 真正排乾；window/process teardown 則由 native 一次清掉。
  azurePlaybackPresentation = audio;
  let playbackFinished = false;
  const playbackFailed = () => {
    if (revision !== azureSpeechRevision || playbackFinished) return;
    playbackFinished = true;
    setPersonaSpeaking(PERSONA_SPEAKING_AZURE, false);
    resetAzureAnswerButton();
    personaAudio.onerror = null;
    personaAudio.onended = null;
    personaAudio.removeAttribute?.("src");
    releaseAzurePlaybackPresentation(audio);
    personaLine.textContent = "Azure 音訊播放失敗。請再重播一次。";
    personaLine.hidden = false;
  };
  personaAudio.onerror = playbackFailed;
  // 先裝 error handler 再交出 data URL；即使 decoder 立刻拒絕，也有清除記憶中
  // MP3 與還原按鈕的出口。
  personaAudio.currentTime = 0;
  personaAudio.src = audio.data_url;
  personaAudio.onended = () => {
    if (revision !== azureSpeechRevision || playbackFinished) return;
    playbackFinished = true;
    setPersonaSpeaking(PERSONA_SPEAKING_AZURE, false);
    resetAzureAnswerButton();
    personaAudio.onerror = null;
    personaAudio.onended = null;
    personaAudio.removeAttribute?.("src");
    releaseAzurePlaybackPresentation(audio);
  };
  try {
    await personaAudio.play();
    if (revision === azureSpeechRevision && !playbackFinished) {
      setPersonaSpeaking(PERSONA_SPEAKING_AZURE, true);
    }
  } catch {
    playbackFailed();
  }
}

/** 只有 `ask()` 驗過最新那一題後會建立這個一次性意圖。 */
function autoSpeakLatestAzureAnswer(mine) {
  if (mine !== asking) return;
  // 答案完成當下還沒有 authoritative status，就沒有證據證明當時已經 opt in。
  // 不把題目留下來等未來的設定／重簽；下一份新答案才可使用後來讀到的授權。
  if (!azureStatusKnown) {
    pendingAzureAutoAsk = null;
    return;
  }
  // 這一種等待與冷啟動 unknown 不同：它只承接已知 ready 時已開始、正由
  // native cancel 排序的上一段，cancel settle 後仍用權威新 generation。
  if (azureCancelPending) {
    pendingAzureAutoAsk = mine;
    return;
  }
  pendingAzureAutoAsk = null;
  if (!azureSpeechReady) return;
  const button = azureAnswerLine?.querySelector?.(".answer-cloud") ?? null;
  if (button === null) return;
  void speakAzureAnswer(button, AZURE_AUTO_ANSWER);
}

/**
 * 只替「舊題正在 cancel」時完成的當前題補一次。冷啟動 status 未知不排隊，
 * settings／重簽／status read 不能把未來授權借給先前完成的答案。
 */
function settlePendingAzureAutoAnswer() {
  const mine = pendingAzureAutoAsk;
  if (mine === null || azureCancelPending || !azureStatusKnown) return;
  pendingAzureAutoAsk = null;
  autoSpeakLatestAzureAnswer(mine);
}

function answerAzureLine() {
  const li = document.createElement("li");
  li.className = "hits-read hits-cloud";
  const button = document.createElement("button");
  button.type = "button";
  button.className = "answer-read answer-cloud";
  button.textContent = "☁ 用 Azure 朗讀／重播（送出這段文字）";
  button.addEventListener("click", (event) => {
    if (event?.isTrusted !== true) return;
    if (azureCancelPending) {
      personaLine.textContent = "Azure 正在停止上一段朗讀。完成後再重播。";
      personaLine.hidden = false;
      return;
    }
    if (azureAnswerButton === button) {
      stopPersonaMedia();
      personaLine.textContent = "Azure 朗讀已停止。";
      personaLine.hidden = false;
      return;
    }
    void speakAzureAnswer(button, AZURE_TRUSTED_REPLAY);
  });
  li.append(button);
  return li;
}

/** L2 卡片的作者與信心來源不是答案正文；這一行只留在本機畫面上。 */
function overviewProvenance(card) {
  const modelConfidence = () => {
    const confidence = card.model_confidence;
    if (
      typeof confidence !== "number" ||
      !Number.isFinite(confidence) ||
      confidence < 0 ||
      confidence > 1
    ) {
      throw new Error("記憶總覽的模型信心不是 0 到 1 的數字");
    }
    return confidence.toFixed(2);
  };
  switch (card.author) {
    case "interpreter":
      return `模型整理的假設 · 模型自報信心 ${modelConfidence()}（不是量出來的）`;
    case "reviewer":
      return `審閱層修訂 · 原模型自報信心 ${modelConfidence()}（不是量出來的）`;
    case "user":
      return "你修正過 · 不是她量出來的，也不是模型說的";
    default:
      throw new Error(`不認得的記憶總覽作者：${card.author ?? "缺少 author"}`);
  }
}

/**
 * 「我知道了什麼」不能再退回 FTS，把設定頁或問題本身的 OCR 片段當答案。
 * 這裡只畫後端已經封閉分類的 L2 總覽；未知 kind 是前後端 contract 壞掉，不能
 * 默默落進一般空結果，否則一個程式錯誤會被說成「沒有記憶」。
 *
 * @returns 這次是否真的畫出了有證據的答案卡片。
 */
function renderOverview(overview) {
  const line = (text, className = "hits-note", speak = true) => {
    const li = document.createElement("li");
    li.className = className;
    if (speak) li.dataset.azureAnswerBody = "";
    li.textContent = text;
    hitList.append(li);
  };

  switch (overview?.kind) {
    case "ready": {
      if (
        !Array.isArray(overview.cards) ||
        overview.cards.length === 0 ||
        overview.cards.length > 3
      ) {
        throw new Error("記憶總覽的 ready 卡片數不在 1 到 3 張之間");
      }
      if (typeof overview.truncated !== "boolean") {
        throw new Error("記憶總覽的 truncated 不是布林值");
      }
      if (
        !Number.isSafeInteger(overview.evidence_unavailable) ||
        overview.evidence_unavailable < 0
      ) {
        throw new Error("記憶總覽的 evidence_unavailable 不是非負整數");
      }
      line(
        "我目前對最近幾段有這些理解。每張下面都有畫面出處按鈕；內容可能由模型整理、審閱層修訂，或由你修正，不是我量到的確定事實：",
      );
      for (const card of overview.cards) {
        if (typeof card?.activity !== "string" || card.activity.trim() === "") {
          throw new Error("記憶總覽的 ready 卡片沒有 activity");
        }
        if (!Number.isSafeInteger(card.segment_started_at)) {
          throw new Error("記憶總覽的 segment_started_at 不是安全整數");
        }
        if (!Array.isArray(card.evidence) || card.evidence.length === 0) {
          throw new Error("記憶總覽的 ready 卡片沒有畫面出處");
        }
        for (const item of card.evidence) {
          if (!Number.isSafeInteger(item?.frame_id) || item.frame_id <= 0) {
            throw new Error("記憶總覽的 frame_id 不是正整數");
          }
          if (typeof item.label !== "string" || item.label.trim() === "") {
            throw new Error("記憶總覽的畫面出處沒有 label");
          }
        }
        const li = document.createElement("li");
        li.className = "hit overview-card";

        const activity = document.createElement("p");
        activity.className = "hit-text";
        activity.dataset.azureAnswerBody = "";
        activity.textContent = card.activity;
        li.append(activity);

        const meta = document.createElement("p");
        meta.className = "hit-source overview-meta";
        meta.textContent = `${when(card.segment_started_at)} · ${overviewProvenance(card)}`;
        li.append(meta);

        const evidence = document.createElement("p");
        evidence.className = "hit-source overview-sources";
        const label = document.createElement("span");
        label.textContent = "畫面出處";
        evidence.append(label);
        for (const item of card.evidence) {
          const button = document.createElement("button");
          button.type = "button";
          button.className = "overview-evidence";
          button.textContent = item.label;
          button.addEventListener("click", (event) => {
            if (event?.isTrusted !== true) return;
            void invoke?.("open_frame", { frameId: item.frame_id });
          });
          evidence.append(button);
        }
        li.append(evidence);
        hitList.append(li);
      }
      if (overview.truncated) {
        line("這裡只列最近一部分有證據的理解。", "hits-note hits-more", false);
      }
      if (overview.evidence_unavailable > 0) {
        line(
          `另外有 ${overview.evidence_unavailable} 張理解卡目前沒有可點開的畫面出處，這裡沒有列。`,
          "hits-note hits-more",
          false,
        );
      }
      return true;
    }
    case "raw_only":
      line(
        "我有原始紀錄，但還沒有整理成能直接回答的理解記憶；這次不會拿 OCR 片段冒充答案。",
        "hits-empty",
      );
      return false;
    case "empty":
      line("我目前還沒有留下能回答這題的記憶。", "hits-empty");
      return false;
    case "evidence_missing": {
      if (!Number.isInteger(overview.cards) || overview.cards <= 0) {
        throw new Error("記憶總覽回了 evidence_missing，卻沒有遺失證據的卡片數");
      }
      line(
        `我有整理過的理解記憶，但最近這 ${overview.cards} 張卡片目前沒有可點開的畫面出處；這裡不把它們當成答案。`,
        "hits-empty",
      );
      return false;
    }
    default:
      throw new Error(`不認得的記憶總覽狀態：${overview?.kind ?? "缺少 kind"}`);
  }
}

/**
 * CLI 只能替本機候選成句；每一句的 source ref 都已由 native 對本次候選做過
 * exact 驗證。按有畫面的來源直接開圖；只有文字的來源則移到下方原文。
 */
function renderGrounded(synthesis, facts, hits, queryId) {
  if (synthesis === null || synthesis === undefined) return false;
  if (
    !Array.isArray(synthesis.sentences) ||
    synthesis.sentences.length < 1 ||
    synthesis.sentences.length > 3
  ) {
    throw new Error("成句答案必須是 1 到 3 句");
  }

  const sourceTarget = (reference) => {
    if (reference.startsWith("fact:")) {
      const id = Number(reference.slice("fact:".length));
      const index = facts.findIndex((fact) => fact.fact_id === id);
      return index < 0 ? null : { item: facts[index], rank: index };
    }
    if (reference.startsWith("chunk:")) {
      const id = Number(reference.slice("chunk:".length));
      const index = hits.findIndex((hit) => hit.chunk_id === id);
      return index < 0
        ? null
        : { item: hits[index], rank: facts.length + index };
    }
    return null;
  };

  for (const sentence of synthesis.sentences) {
    if (typeof sentence?.text !== "string" || sentence.text.trim() === "") {
      throw new Error("成句答案裡有空句");
    }
    if (!Array.isArray(sentence.sources) || sentence.sources.length === 0) {
      throw new Error("成句答案裡有一句沒有本機出處");
    }
    const li = document.createElement("li");
    li.className = "hit grounded-answer";
    const text = document.createElement("p");
    text.className = "grounded-text";
    text.dataset.azureAnswerBody = "";
    text.textContent = sentence.text;
    li.append(text);

    const sourceLine = document.createElement("p");
    sourceLine.className = "hit-source grounded-sources";
    const lead = document.createElement("span");
    lead.textContent = "本機出處";
    sourceLine.append(lead);
    for (const source of sentence.sources) {
      if (
        typeof source?.ref !== "string" ||
        typeof source.label !== "string" ||
        source.label.trim() === ""
      ) {
        throw new Error("成句答案的本機出處不完整");
      }
      const target = sourceTarget(source.ref);
      if (target === null) throw new Error(`成句答案找不到 ${source.ref}`);
      const targetFrame = target.item.frame_id ?? null;
      const sourceFrame = source.frame_id ?? null;
      if (sourceFrame !== targetFrame) {
        throw new Error(`成句答案的 ${source.ref} 畫面來源不一致`);
      }
      const button = document.createElement("button");
      button.type = "button";
      button.className = "grounded-source";
      button.textContent = source.label;
      button.addEventListener("click", (event) => {
        if (event?.isTrusted !== true) return;
        if (Number.isSafeInteger(targetFrame) && targetFrame > 0) {
          void invoke?.("open_frame", { frameId: targetFrame });
          if (
            queryId !== null &&
            queryId !== undefined &&
            target.item.chunk_id !== null &&
            target.item.chunk_id !== undefined
          ) {
            void invoke?.("log_click", {
              queryId,
              chunkId: target.item.chunk_id,
              rank: target.rank,
            })?.catch?.(() => {});
          }
          return;
        }
        hitList
          .querySelector?.(`[data-evidence-ref="${source.ref}"]`)
          ?.scrollIntoView?.({ block: "nearest", behavior: "smooth" });
      });
      sourceLine.append(button);
    }
    li.append(sourceLine);
    hitList.append(li);
  }
  return true;
}

/**
 * @param hits 一筆一筆的原文。
 * @param kind `"keywords"`（比對字找到的）、`"recent"`（剛剛）、`"range"`（昨天下午那種日曆範圍），
 *   或 `"memory_overview"`（只讀 L2 整理結果，不跑一般檢索）。
 *   這個字是後端給的，不是這裡判斷的。一般檢索在 `sister query` 和這一頁共用
 *   sister-core 的問題規則；memory overview 是桌面問答外層的窄 intent。
 * @param facts L1 直接答得出來的那幾筆（★）。排在原文前面，因為那才是他問
 *   的東西本身：問「電話」要的是號碼，不是一段剛好提到電話的字。
 * @param blind 兩手空空時，她查得到的那幾個理由（後端給事實，句子在這裡組）。
 * @param truncated 原文底下還有，只是沒送過來。捲到底那一句要靠它。
 * @param factsTruncated ★ 那一半也被切掉了。分開一個參數是因為兩邊的下一步
 *   不一樣：原文要 `--limit`，★ 十個不同的答案代表問法太寬。
 * @param timeRange 問句裡認得出來的日曆範圍。`null` = 沒有時間範圍，沒去算章節。
 * @param chapters 那段時間切成的活動級段落。`null` = 沒算過；`[]` = 算過但切不出來。
 * @param followup 使用者先開口後，回答尾端才可附上的低頻確認。
 * @param closureNotice 文字結案是否成功；認不出來時也要明講沒有動卡片。
 * @param overview 「我知道了什麼」專用的 L2 總覽；`null` 代表一般檢索題。
 * @param synthesis 本機候選經已登入 CLI 成句後的逐句出處答案；失敗或未啟用時是 `null`。
 * @param brain 已選 CLI 實際處理這題的結果；畫面不靠 `synthesis === null` 猜原因。
 */
function renderHits(
  hits,
  kind,
  queryId = null,
  facts = [],
  blind = null,
  truncated = false,
  factsTruncated = false,
  searched = null,
  timeRange = null,
  chapters = null,
  followup = null,
  closureNotice = null,
  overview = null,
  synthesis = null,
  brain = null,
) {
  azureAnswerLine = null;
  azureAnswerButton = null;
  hitList.replaceChildren();

  const hasOverview = overview !== null && overview !== undefined;
  if ((kind === "memory_overview") !== hasOverview) {
    throw new Error(
      kind === "memory_overview"
        ? "記憶總覽答案缺少 overview"
        : `一般 ${kind ?? "未知"} 答案不該帶 overview`,
    );
  }

  if (closureNotice) {
    const notice = document.createElement("li");
    notice.className = "hits-note";
    notice.textContent = closureNotice;
    hitList.append(notice);
  }

  if (brain && typeof brain.state === "string") {
    const provider =
      typeof brain.provider === "string" && brain.provider.trim() !== ""
        ? brain.provider.trim()
        : "CLI 大腦";
    const messages = {
      used: `${provider} · 已使用本機記憶`,
      no_sources: `${provider} 已查過本機記憶；目前沒有可引用的內容。`,
      answer_failed: `${provider} 沒有完成回答；下方保留它查到的本機記憶。到設定按「測試目前大腦」即可重測。`,
      search_failed: `${provider} 這次沒有完成查詢；下方是依原問題找到的本機記憶。到設定按「測試目前大腦」即可重測。`,
      consent_required: `這題只顯示本機結果；第二張「雲端解讀」目前沒有授權 ${provider} 接手。`,
      not_configured: "到設定選一個 CLI，大腦才會接手文字問題。",
    };
    const text = messages[brain.state];
    if (text) {
      const status = document.createElement("li");
      status.className = `brain-note brain-${brain.state}`;
      status.textContent = text;
      hitList.append(status);
    }
  }

  if (hasOverview) {
    const hasOverviewAnswer = renderOverview(overview);

    if (followup) {
      const aside = document.createElement("li");
      aside.className = "hits-note";
      aside.textContent = followup;
      hitList.append(aside);
    }

    hitList.append(answerReadLine());
    if (azureSpeechEnabled) {
      azureAnswerLine = answerAzureLine();
      hitList.append(azureAnswerLine);
    }
    hitList.hidden = false;
    document.body.classList.add("has-hits");
    paintConversation();
    showingAnswer = hasOverviewAnswer;
    return;
  }

  const hasGroundedAnswer = renderGrounded(synthesis, facts, hits, queryId);

  // **她找的字不一定是他打的字。**
  //
  // `terms` 會把「剛剛」「那個」剝掉，剝到不足兩個字還會往回退一格——而那一格
  // 常常退進虛字裡：「剛剛那個板」→「個板」、「剛剛看到的人」→「的人」。
  //
  // 兩種完全不同的處境於是印出同一句「我記得的東西裡沒有這件事」：他打的字真的
  // 沒出現過，跟她根本沒找他打的字。前者他無能為力，後者他只要把那個詞重打一次
  // 就好。有命中的那一半更難看出來——「的人」在一年份的螢幕文字裡什麼都比得到，
  // 於是他拿到一串毫不相干的東西，而唯一的解讀是「這東西壞了」。所以這一句擺在
  // 最上面，兩種結果都蓋得到，而不是只掛在空手的那一邊。
  //
  // 後端只在**黏過**的時候送這個欄位（剝掉「剛剛那個」留下「優惠方案」是剝對
  // 了，每次都報一句只會讓人學會忽略它），所以這裡有值就一定要講。
  if (searched) {
    const why = document.createElement("li");
    why.className = "hits-note";
    why.textContent = `我拿去比對的是「${searched}」——那是從你打的字黏出來的，不是一個詞。直接打你要的那個詞再問一次。`;
    hitList.append(why);
  }

  // 他打了「剛剛發生什麼事」，而底下這幾筆跟那七個字一個都對不上。不先講
  // 一句「我把它當成時間問題了」，看起來就只是她答非所問。
  if (kind === "recent" && hits.length > 0) {
    const note = document.createElement("li");
    note.className = "hits-note";
    note.textContent = "你問的是「剛剛」，所以我沒有去比對字——這是我最後看到的幾件事：";
    hitList.append(note);
  }
  if (kind === "range" && hits.length > 0) {
    const note = document.createElement("li");
    note.className = "hits-note";
    note.textContent = "你問的是一段日子，所以我沒有拿時間詞去比對螢幕——這是那段時間看到的事：";
    hitList.append(note);
  }

  // 日曆範圍是另算的一區。`chapters === null` 時不要說「沒有章節」——那是
  // 沒算過，和算過但切不出來是兩件事。
  if (timeRange) {
    const recap = document.createElement("li");
    recap.className = "hits-note";
    recap.textContent = `你問的是「${timeRange.said}」，那段時間是 ${when(timeRange.from)} 到 ${when(timeRange.to)}`;
    hitList.append(recap);
    if (Array.isArray(chapters)) {
      if (chapters.length === 0) {
        const emptyCh = document.createElement("li");
        emptyCh.className = "hits-note";
        emptyCh.textContent = "那段時間沒有切得出來的段落。";
        hitList.append(emptyCh);
      } else {
        const count = document.createElement("li");
        count.className = "hits-note";
        count.textContent = `那段時間分成 ${chapters.length} 段：`;
        hitList.append(count);
        for (const ch of chapters) {
          hitList.append(chapterHit(ch, !hasGroundedAnswer));
        }
      }
    }
  }

  if (followup) {
    const aside = document.createElement("li");
    aside.className = "hits-note";
    aside.textContent = followup;
    hitList.append(aside);
  }

  // SPEC §8.2 的語氣規範：「我最後看到的是…」，不准講成斷言。★ 那幾筆最需要
  // 這一句——一個孤零零的號碼看起來像一句「這就是答案」，而她知道的只有
  // 「我在某個時間點的螢幕上看過它」。問「昨天的金額」而她給的是今天那筆的
  // 時候，這一行就是那個差別。
  if (facts.length > 0) {
    const note = document.createElement("li");
    note.className = "hits-note";
    // 這一句是事實答案必要的認知界線，不是操作提示；逐一 allow，而不是把
    // `.hits-note` 整類送出去。
    if (!hasGroundedAnswer) note.dataset.azureAnswerBody = "";
    note.textContent = "我最後看到的是：";
    hitList.append(note);
  }

  for (const [rank, fact] of facts.entries()) {
    const li = document.createElement("li");
    li.className = "hit fact";

    const value = document.createElement("p");
    value.className = "fact-value";
    if (!hasGroundedAnswer) value.dataset.azureAnswerBody = "";
    value.textContent = fact.value;
    // 1 次和 12 次是強度不同的答案。她自己不下判斷，只把數字講出來。
    if (fact.sightings > 1) {
      const seen = document.createElement("span");
      seen.className = "fact-seen";
      // CSS 的 badge 間距不是文字；Azure extractor 讀 textContent，需要真的有
      // 語音停頓，否則會念成「0800...看過十二次」黏在一起。
      seen.textContent = `（看過 ${fact.sightings} 次）`;
      value.append(seen);
    }
    li.append(value);

    // 正規化後的值認得出來，原文才認得出**場景**——`+886800080123` 是機器
    // 要的，`客服專線 0800-080-123` 才是他記得的那一行。兩個都給。
    const raw = document.createElement("p");
    raw.className = "hit-text fact-raw";
    if (!hasGroundedAnswer) raw.dataset.azureAnswerBody = "";
    li.dataset.evidenceRef = `fact:${fact.fact_id}`;
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

  const hasChapters = Array.isArray(chapters) && chapters.length > 0;

  if (hits.length === 0 && facts.length === 0 && !hasChapters) {
    const empty = document.createElement("li");
    empty.className = "hits-empty";
    empty.dataset.azureAnswerBody = "";
    // 「我沒看過這件事」和「我什麼都還沒看過」是兩件不同的事。
    //
    // 但問時間卻空手而回，**不是**只有「她還沒錄過東西」這一種解釋——三十行
    // 上面的 `blindLines()` 自己就列得出另外四種。以前這裡寫死了那一句，於是
    // 他在時間軸按過「忘掉這一整天」之後回來問「剛剛發生什麼事」，讀到的是
    //
    //     我什麼都還沒看到——要先跑 sister record 我才記得住。
    //     （我錄過，但現在什麼都不剩了——被忘掉了，或是過了保留期。）
    //
    // 上下兩行互相打臉，而錯的是**標題**那一行。OCR 斷掉的版本更糟：標題說
    // 「我什麼都還沒看到」，底下那行說「我看過 12000 張畫面」，然後叫他再去
    // 錄一天同樣讀不出字的畫面——那正是這個專案已知的主要故障形狀。
    //
    // `sister query` 修過同一句（見 ops.rs 裡 `Shape::Recent` 那段註解：
    // 空手的時候標題不講話，讓 `blind_lines` 講）。這裡是它在視窗這一邊的
    // 另一半，晚了三個版本。標題只講一件她一定知道的事——手上沒有東西——
    // 原因交給底下那幾行，它們是照著資料庫算出來的。
    //
    // 這一句本來是「這件事我沒看到過。」——一句關於**世界**的斷言，而她
    // 唯一有資格講的是關於**她自己的紀錄**的話（SPEC §8.2，和 ★ 上面那句
    // 「我最後看到的是：」同一條紀律）。那個差別不是措辭：東西可能就在螢幕
    // 上，只是被排除規則擋掉、被暫停跳過，或者 OCR 沒讀出來——最後這一種
    // 她連數都數不出來，所以下面那幾行理由永遠不會是完整的。
    //
    // 「我記得的東西」這幾個字也要看她這次到底翻了多少：只翻了 30 天卻說
    // 「我記得的東西」，是把十二分之一講成全部（見 `scan_horizon_days`）。
    empty.textContent =
      kind === "recent" || kind === "range"
        ? "我手上一件事都沒有。"
        : blind?.scan_horizon_days
          ? "我翻過的那幾段裡沒有這件事。"
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
    if (!hasGroundedAnswer) text.dataset.azureAnswerBody = "";
    li.dataset.evidenceRef = `chunk:${hit.chunk_id}`;
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

  // 「這一題我本來已經忘了」。**只在她真的給了東西的時候才出現**——一份空手
  // 而回的答案沒有什麼好標的，而一個掛在「我沒看過這件事」底下的「我早就忘了」
  // 按鈕，記下來的會是一次失敗。
  if (hits.length > 0 || facts.length > 0 || hasChapters) {
    // 沒有題號就標不了，而**這件事要講出來**。以前 `query_id` 是 `null` 只代
    // 表「這次點擊不會記帳」——看不見也無所謂。現在它代表那顆按鈕整個不見，
    // 而那顆按鈕是 Phase 1 第一條退場條件唯一的量法：安靜地少一個禮拜的證據，
    // 沒有人會發現。
    //
    // **三種原因**，在這一頁分不出來——後端只送得出一個 `null`——所以照這個
    // repo 的規矩，把可能性攤開，不要替他選一個。
    //
    // 上一版只攤兩種（那個勾關著／寫不進資料庫），漏掉的第三種是**設定檔讀不
    // 回來**：後端那邊是 `Config::load(…).unwrap_or(false)`，壞掉的 TOML 和
    // 「勾拿掉了」在那一行之後長得完全一樣。而它的下一步和另外兩種都不同——設
    // 定頁上那個勾看起來是開著的，資料庫也好好的，他照前兩句去查只會兩邊都
    // 撲空。一句「兩種」的話，本身就是在對一個它沒數過的集合下斷言。
    if (queryId === null || queryId === undefined) {
      const why = document.createElement("li");
      why.className = "hits-note hits-more";
      why.textContent =
        "（這一題沒進題庫，所以「我本來已經忘了」標不了：可能是設定裡「你問過她什麼」關著，也可能是設定檔讀不回來，還可能是剛剛寫不進資料庫。）";
      hitList.append(why);
    } else {
      hitList.append(markLine(queryId));
    }
  }

  // 本機朗讀仍是另一個明確 click。Azure 開關打開且第四張有效時，
  // `ask()` 只會在最新新答案完成後啟動一次；`renderHits()` 自己不觸發出境。
  hitList.append(answerReadLine());
  if (azureSpeechEnabled) {
    azureAnswerLine = answerAzureLine();
    hitList.append(azureAnswerLine);
  }

  hitList.hidden = false;
  document.body.classList.add("has-hits");
  paintConversation();
  // **不是無條件 `true`。** [`showingAnswer`] 的唯一讀者是那句「底下原本那幾筆
  // 是上一題的」，而空手而回的那一次底下躺的是「我記得的東西裡沒有這件事。」
  // 加上幾行理由——一筆都沒有。寫死 `true` 的話，下一題失敗會請他去看幾筆
  // 不存在的東西，而**空手而回正是他最可能連問第二次的那一種結果**。
  showingAnswer = hasGroundedAnswer || hits.length > 0 || facts.length > 0 || hasChapters;
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

async function ask(event = null) {
  const question = askInput.value.trim();
  if (question === "") return;
  if (consentGuideSheet !== null) {
    await handleConsentReply();
    return;
  }

  // 上一題若還在等開場 status，現在也不再是「最新那題」。
  pendingAzureAutoAsk = null;
  stopPersonaMedia();

  const mine = ++asking;
  // 新的一題蓋掉上一次那句「為什麼沒成」——他已經在做下一件事了。
  notice = null;
  slowNote = null;
  setState("thinking");
  const slow = setTimeout(() => {
    // 這個 timer 只量到一件事：這一題已經等了 4 秒。它沒有問資料庫是不是
    // 第一次開、有沒有 migration，也沒有看索引進度。以前那句「第一次打開
    // 資料庫要先整理索引」在任何慢查詢都會出現，這次甚至把 WebView2 deadlock
    // 講成索引。給人看的話只能說程式真的量到的那半。
    //
    // **`state === "thinking"` 不夠。** 他多按了幾次 Enter 的話，第一題的計時器
    // 會在第二題送出之後才響，而那時候 `state` 還是 `thinking`（是**第二題**
    // 的）——於是一句「這一題已經超過 4 秒」蓋在一個一百毫秒前才送出去的
    // 問題上。底下那個 `finally` 為了同一件事已經多問了一次
    // `mine === asking`；這裡是它漏掉的兄弟。
    if (mine === asking && state === "thinking") {
      // 舊版說「還在翻…」，而那是假的：四秒那個位置她幾乎一定不在翻本機
      // 資料庫，而是在等**使用者自己選的那支 CLI**。一題要來回兩趟——先問
      // 它「這題要查什麼」（`prepare_search_plan`），本機查完再請它「把查到
      // 的寫成一句」（`prepare`）——兩趟各是一次冷啟動加一次模型呼叫，本機
      // 檢索在旁邊是毫秒級的。講成「翻」會讓人以為是她的資料庫慢，然後去
      // 刪東西。
      //
      // 「多半」不是客套：這個計時器只量到「等了四秒」，沒有問是哪一段在等。
      // 真的要指名是哪一趟，得讓 native 在中間也送狀態出來，那是另一條線。
      slowNote =
        "超過 4 秒了。多半是在等你選的 CLI——一題要來回兩趟：先問它要查什麼，再請它把查到的寫成一句。";
      paint();
    }
  }, SLOW_MS);

  try {
    if (invoke === null) throw new Error("這一頁不是在 AI-Sister 裡打開的");
    const answer = await invoke("ask", { question });
    // 這一份過期了。畫面歸還在跑的那一次管，這裡連 idle 都不要設。
    if (mine !== asking) {
      releaseNativePresentation(answer);
      return;
    }
    // 啟動後若條文剛好改版，開場那次 consent read 可能早於這份新狀態。
    // 先重讀真正的四張；只有「尚未回答」才接進對話，不把使用者明確回答過的
    // 不同意又問一次。這一份本機 fallback 尚未畫出，先歸還 presentation lease。
    if (answer?.brain?.state === "consent_required") {
      let view = null;
      try {
        view = usableConsentView(await invoke("consent_read"));
      } catch {
        // 本機答案已經拿到了；同意書這次讀不到不能把它一起丟掉，也不能猜成
        // 未回答。照原本的 fail-closed 狀態畫出本機結果即可。
      }
      if (view !== null && nextConsentSheet(view) !== null) {
        releaseNativePresentation(answer);
        pendingConsentQuestion = question;
        askInput.value = "";
        consentGuideBusy = false;
        showConsentGuide(view);
        setState("idle");
        return;
      }
    }
    const presented = await commitNativePresentation(answer, () => {
      // begin 成功後 native guard 仍活著；這一段同步畫完才 end。外部 CLI 即使
      // 已發佈 pending，也只能在畫完之後回報全停成功。
      renderHits(
        answer.hits,
        answer.kind,
        answer.query_id,
        answer.answers,
        answer.blind,
        answer.truncated,
        answer.answers_truncated,
        answer.searched,
        answer.time_range,
        answer.chapters,
        answer.followup,
        answer.closure_notice,
        answer.overview,
        answer.synthesis,
        answer.brain,
      );
      setState("idle");
      // 答完才清掉。失敗的時候留著，他才不用把整句話重打一次。
      askInput.value = "";
      // 這是唯一條自動 Azure 入口：native 已經 ready、而且這份仍是最新
      // ask 的答案，才會把正文送一次。開機 demo、status event 與舊答案重畫不走這裡。
      autoSpeakLatestAzureAnswer(mine);
      // 語音是情境用的，回答歸回答：她出的是「找到了」，答案本文照舊用讀的。
      playAnswerBeat();
    });
    if (!presented && mine === asking) {
      setState("idle");
    }
  } catch (err) {
    if (mine !== asking) return;
    // 失敗要說出是什麼失敗。「沒有結果」跟「還沒錄過任何東西」跟「資料庫
    // 打不開」是三件不同的事，混成一句「查不到」等於把問題藏起來。
    //
    // 進 `notice`，不是直接寫那一格：直接寫的話這句話 5 秒內會被輪詢換成
    // 「在聽」，而那三件事就又混成同一片沉默了。
    //
    // 她正在起來的那 25 秒（`booting` 更久）他最可能問問題，而這一題失敗的
    // 原因和她還沒起來的原因是同一顆資料庫——所以這一句一定要自己帶主詞。
    noticeAboutSomethingElse(err?.message ?? err);
    setState("idle");
    // 上一題的答案要先撤掉。這一句以前不在，於是問了第二題而它壞掉的時候，
    // 畫面上是：新的問題還在輸入框裡、**舊的那一題的答案原封不動躺在下面**、
    // 角落一行小小的錯誤訊息。他讀到的是一份對不上題目的答案，而她連自己
    // 答錯了都不知道——這比一片空白糟得多。
    const failed = document.createElement("li");
    failed.className = "hits-empty";
    // 後半句是在描述**它剛剛做掉的事**，而那件事不一定發生過。第一次問問題的
    // 人底下從來沒有東西；連著失敗第二次的人底下躺的是上一次的同一句錯誤。
    // 兩種都被請去看一個不存在的東西——而底下那段註解自己寫著這條路「專挑新
    // 使用者」，也就是說這一句在它最重要的那台機器上永遠是假的。
    failed.textContent = showingAnswer
      ? "這一題我沒答成——底下原本那幾筆是上一題的，先收起來了。"
      : "這一題我沒答成。";
    hitList.replaceChildren(failed);
    showingAnswer = false;
    // **那個 `<ul>` 開場是 hidden 的**（`index.html` 上寫死，`styles.css` 還
    // 補了一條 `.hits[hidden] { display: none }`），而唯一會拿掉它的是
    // `renderHits`。少了下面這兩行，第一題就失敗的人**什麼都看不到**：這一句
    // 塞進一個 `display: none` 的容器裡，狀態那一行也只是一句錯誤訊息。
    // 也就是說這條路對「資料庫打不開」的新機器完全沉默——而那正是它要講話的
    // 那一台。答成過一次之後才會自己好，所以它專挑新使用者。
    hitList.hidden = false;
    document.body.classList.add("has-hits");
    paintConversation();
  } finally {
    clearTimeout(slow);
    // **過期的那一份不准動畫面，包括這裡。** 他多按了幾次 Enter、先送的後回，
    // 那一次走到這裡的時候還在跑的是別題——清掉的會是**那一題**的那句慢話。
    // `asking` 那個編號存在的理由就是這個，上面兩條路都問過了，這一條也要問。
    if (mine === asking) {
      slowNote = null;
      paint();
    }
  }
}

askSend?.addEventListener("click", () => void ask());

/*
 * 上面那條深色膠囊的開關。
 *
 * 平常它不在，也不擋滑鼠——收起來的規則是 CSS 的 `visibility: hidden`，而
 * `paintedRects()` 明文跳過那一種，所以「看不見」和「點得穿」是同一件事，
 * 不必在這裡另外推一次實心區。class 一換，那個 MutationObserver 就會排一次
 * `pet_solid_set`。
 *
 * **開機一律是收起來的。** 這是刻意的：那條膠囊唯一的預設狀態就是「不在」，
 * 所以重開之後畫面長什麼樣不必去猜，也不必再存一份設定。要用的人按一下就有，
 * 而她真的停下來的時候（`body.she-is-stopped`）它自己會出現——那一格不歸這顆
 * 鍵管，按下去也關不掉。
 */
function setChromeOpen(open) {
  document.body.classList.toggle("chrome-open", open);
  chromeToggle?.setAttribute?.("aria-expanded", open ? "true" : "false");
}

// 這顆不擋 `isTrusted`。上面那五顆（暫停、置頂、收起來…）也都沒擋——會擋的
// 是「講話」和「開東西」那幾條，因為那些有產品規則要求必須是真人當下的動作。
// 翻一個自己的 class 不在那一類，而擋了它就等於這一頁唯一的版面驗證工具
// （`scripts/shot.mjs` 用的是 `element.click()`）看不到收起來以外的樣子。
chromeToggle?.addEventListener("click", () => {
  setChromeOpen(!document.body.classList.contains("chrome-open"));
});

// 收起來的時候裡面那五顆鍵不能還在 Tab 順序上。`visibility: hidden` 本來就
// 會把它們拿掉，這裡是給沒有跟著走的環境（以及讀螢幕的人）一個明講的答案。
new MutationObserver(() => {
  const shown =
    document.body.classList.contains("chrome-open") ||
    document.body.classList.contains("she-is-stopped");
  chromeBar?.setAttribute?.("aria-hidden", shown ? "false" : "true");
}).observe(document.body, { attributes: true, attributeFilter: ["class"] });
askInput?.addEventListener("keydown", (event) => {
  // 選字中的 Enter 是「就選這個字」，不是「問出去」。注音打「剛剛發生什麼事」
  // 一路上會按好幾次 Enter，少了這一行，第一次選字就把半句話送出去了。
  // `keyCode === 229` 是舊的那條路，有些 IME 只給得出這個。
  if (event.isComposing || event.keyCode === 229) return;
  if (event.key === "Enter") void ask();
});

// ---------- 開場 ----------

// 開發用的兩個開關：`?state=paused` 直接看某一個狀態，`?hits=demo` 看
// 有答案時的版面。只有「沒有 Tauri IPC、而且網址明確帶 state」才把這份假狀態
// 當成可採信的畫面輸入；產品冷啟動在 heartbeat / supervisor 回來前仍然只能說
// 正在確認。它們存在的理由是這台開發機開不起 Tauri 視窗（沒有 webkit2gtk、
// 沒有 sudo），而版面對不對不該等到上了 Windows 才第一次看到。
const params = new URLSearchParams(globalThis.location.search);
const browserDemoQuery = invoke === null;
const requestedBrowserState = params.get("state");
const browserStateFixtures = new Set([
  "idle",
  "thinking",
  "paused",
  "stopped",
  "asleep",
  "booting",
]);
const authoritativeBrowserStateDemo =
  browserDemoQuery && browserStateFixtures.has(requestedBrowserState);
// Query string 不是 native recorder 的輸入。Tauri 視窗即使意外帶著 `?state=`，
// 或純瀏覽器收到拼錯／外來的值，都只保留「正在確認」的冷啟動畫面，直到真的
// heartbeat + supervisor 回來；不能先畫一格假的暫停、思考或在聽。
const wanted = authoritativeBrowserStateDemo ? requestedBrowserState : "idle";

if (authoritativeBrowserStateDemo) {
  // 這不是 native supervisor 的回覆，只是讓純瀏覽器截圖走同一套 truth gate。用
  // `running` 能涵蓋 recording / booting；asleep 則明確配 stopped。沒有 `?state=`
  // 的普通瀏覽器頁也不進來，避免一個預設值冒充量到的 recorder 狀態。
  recorderSupervisorStateKnown = true;
  recorderSupervisor = normalizeRecorderSupervisor({
    phase: wanted === "asleep" ? "stopped" : "running",
  });
}

seedSwayPhase();
applyPersona({ id: "chatgpt", enabled: true, motion: true, tap_lines: true });
paintPin();
// 只讀本機 config；失敗就留在 HTML 已經畫好的 ChatGPT 內建角色圖。
readPersona();
// 只讀開關／region／credential 四態／第四張同意書，不會合成，也不會連 Azure。
readAzureTts();
// 還沒回答的同意書直接在這顆對話氣泡逐張問；完整卡片仍留在設定入口。
void readConsentGuide();

// `?state=paused` 走的是**和產品一樣的那條路**（設 `paused` 旗標），不是另外
// 搬一個長得像暫停的樣子出來。這一點是被截圖抓到的：第一版讓它去設 `state`，
// 於是截出來的圖裡桌面姊妹是灰的、但拖曳條上的暫停鍵還是「⏸」——而截圖是這台
// 機器上唯一看得到 UI 的方式，一個走假路的開發開關會讓它騙我。
setPaused(wanted === "paused");
// `?state=stopped` 只替純瀏覽器 fixture 注入 renderer 狀態；產品裡仍只信
// `master_stop_state` 與 native event。
if (authoritativeBrowserStateDemo) {
  setMasterStopPhase(wanted === "stopped" ? "stopped" : "clear");
}
// `?state=asleep`：沒有人在跑 `sister record`。`?state=booting`：有一個起來
// 了，但還在開資料庫——那一格畫面上不是「沒有人在記錄」（那句話配著一顆按下
// 去會失敗的按鈕），而他那顆一年份的資料庫每天早上都會停在這裡好幾分鐘。
// 和上面同一條紀律——走真正的那個旗標，不另外搬一個長得像的樣子出來。
setRecording(
  wanted === "asleep"
    ? "none"
    : wanted === "booting"
      ? "booting"
      : "recording",
  // 純瀏覽器明確要求的 state 是可重現截圖的 fixture；Tauri 冷啟動或沒有 state 的
  // 瀏覽器頁都只是開場 shape，在真正 heartbeat 回來前不能替 recorder 作證。
  authoritativeBrowserStateDemo,
);
setState(
  wanted === "paused" || wanted === "stopped" || wanted === "asleep" || wanted === "booting"
    ? "idle"
    : wanted,
);

// 開場先問一次磁碟。暫停**不會自己過期**，所以「上禮拜按了暫停」是一條真實
// 的路——開起來就該是灰的，而不是先亮一下再變灰。
//
// 之後由 `pollRecording` 每 5 秒接手（它同時問這兩件事）。這一行留著是因為
// 視窗如果一開始就縮在系統匣裡，那個輪詢是不跑的。
if (invoke !== null) {
  invoke("pause_state").then(setPaused, () => {});
  readMasterStopState();
  // 即使 Windows 登入 intent 把主視窗留在系統匣，也先取一份 supervisor view；
  // 顯示中的五秒 poll 會接著重讀。兩次很接近時只有較新的 request 能套用。
  readRecorderSupervisor();
  // 只問一次，不進輪詢：這個答案只有他自己改得動，而她每 5 秒重問一次
  // 等於每 5 秒重畫一個他已經看過的問題。
  readUrlPolicy();
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
if (browserDemoQuery && params.get("asleep") === "stopped") {
  lastRun = {
    started_at: Date.now() - 4 * 3600 * 1000,
    ended_at: Date.now() - 90 * 60 * 1000,
    why: "你按了停止",
  };
  paint();
} else if (browserDemoQuery && params.get("asleep") === "crashed") {
  lastRun = {
    started_at: Date.now() - 19 * 3600 * 1000,
    ended_at: null,
    why: null,
  };
  paint();
} else if (browserDemoQuery && params.get("asleep") === "nobeat") {
  // 叫了、逾時了、還是沒有心跳。這一句要活得比一次輪詢久（以前它被下一個
  // `paint()` 蓋掉），而它換行、比另外兩句長——版面撐不撐得住要用眼睛看。
  noticeAboutHer(
    "等了 25 秒還沒有心跳。record.log 最後說：\n" +
      "（這一輪還沒寫出東西，以下是上一輪的 record.log）\n" +
      "第一張同意書還沒簽——她不會開始記錄。",
    true,
  );
  paint();
}

if (browserDemoQuery && params.get("hits") === "demo") {
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
    // 假的題號。**不是 `null`**：`null` 的意思是「這一份答案沒有掛在題庫上」，
    // 而那會把底下那顆「這件事我本來已經忘了」整個藏起來——於是這一頁唯一驗得
    // 到版面的路徑，剛好驗不到最新加上去的那一格。配 `&marked=1` 看按下去之後。
    77,
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
    // `?glued=的人`：她拿去比對的字是黏出來的，**而且還真的比到東西了**。
    // 這一半比空手那一半更需要看一眼：一串看起來像正常答案的東西配上一句
    // 「我找的不是你打的字」，兩者要能同時讀得下去才算對。
    params.get("glued"),
    null,
    null,
    null,
    null,
    null,
    null,
    { state: "used", provider: "Grok CLI" },
  );
}

// `?hits=recent` 是「剛剛發生什麼事」那條路的版面。分開一個開關而不是共用
// 上面那組假資料，是因為要看的正是**兩者長得不一樣**：時間問題多了一句說明，
// 而且答案裡不會有任何一個被標起來的字。
if (browserDemoQuery && params.get("hits") === "recent") {
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
    // `?hits=demo` 那邊為了同一個理由給了一個假題號：`queryId` 是 `null` 的
    // 話，答案底下換成那句「這一題沒進題庫，所以標不了」——在一頁假資料上那
    // 句話本身就是假的（沒有人寫不進資料庫），而這一頁是這台機器上唯一驗得到
    // 版面的路徑。上一次只補了那一邊，這一邊留在原地。
    88,
  );
}

/*
 * `?hits=grounded`：**已選 CLI 成句**的那一版。
 *
 * 這一格以前沒有 demo，而它正是使用者抱怨的那一頁——Ted 那張截圖上「畫面上
 * 可見有人要求 Codex Agent 寫交接檔…本機出處[文字#5443]」就是從這裡畫出來的。
 * 沒有 demo 的意思是：這台開發機上沒有任何辦法看到它長什麼樣，只能改完等他
 * 裝一次。所以先把它補上。
 *
 * 底下那兩句**不是手寫的**：把 `grounded_answer::prepare` 對這兩筆來源組出來的
 * prompt 餵給一支真的 CLI（2026-09-12 用 Grok 跑的），回來的就是這兩句，而且
 * 原封不動走得過產品自己那支 `parse`。手寫的樣本只證明版面畫得出那幾個字；
 * 真的輸出才量得到真正的長度——「你早上 10:14 在 Codex 裡要求…」比「有人要求…」
 * 長得多，會不會換行、尾巴那幾顆出處鍵會不會被擠掉，用眼睛看比用想的準。
 */
if (browserDemoQuery && params.get("hits") === "grounded") {
  const t = (h, m) => {
    const d = new Date();
    d.setHours(h, m, 0, 0);
    return d.getTime();
  };
  renderHits(
    [
      {
        ts: t(10, 14),
        snippet: "請幫我寫一份交接文件，把這幾天的進度、踩過的坑、決定都寫清楚",
        text: "",
        app: "WindowsTerminal.exe",
        title: "codex — AI-Sister",
        url: null,
        chunk_id: 5443,
        frame_id: 5443,
      },
      {
        ts: t(10, 31),
        snippet: "HANDOFF.md 已更新：真正的方向與接下來要做的事",
        text: "",
        app: "code.exe",
        title: "HANDOFF.md",
        url: null,
        chunk_id: 5429,
        frame_id: 5429,
      },
    ],
    "recent",
    93,
    [],
    null,
    false,
    false,
    null,
    null,
    null,
    null,
    null,
    null,
    {
      sentences: [
        {
          text: "你今天早上 10:14 在 Codex 裡要求寫一份交接文件，要把這幾天的進度、踩過的坑、決定，還有真正的方向跟接下來要做的事都寫清楚。",
          sources: [
            { ref: "chunk:5443", label: "文字#5443", frame_id: 5443 },
          ],
        },
        {
          text: "你早上 10:31 在編輯器打開 HANDOFF.md，裡面寫著已更新真正的方向與接下來要做的事。",
          sources: [{ ref: "chunk:5429", label: "文字#5429", frame_id: 5429 }],
        },
      ],
    },
  );
}

// `?hits=chapters`／`?demo=1`：問「昨天下午在弄什麼」那一版。章節在 facts
// 前面，標題是 app／title，不是一句「你在專心寫程式」。
if (
  browserDemoQuery &&
  (params.get("hits") === "chapters" || params.get("demo") === "1")
) {
  const y = new Date();
  y.setDate(y.getDate() - 1);
  y.setHours(12, 0, 0, 0);
  const from = y.getTime();
  const at = (h, m) => {
    const d = new Date(from);
    d.setHours(h, m, 0, 0);
    return d.getTime();
  };
  renderHits(
    [
      {
        ts: at(15, 40),
        snippet: "把時間軸接起來了。空白處現在會自己說明原因。",
        text: "",
        app: "Notion.exe",
        title: "週報",
        url: null,
        frame_id: 4310,
      },
    ],
    "keywords",
    91,
    [
      {
        value: "+886800080123",
        raw: "客服專線 0800-080-123",
        sightings: 2,
        ts: at(14, 30),
        chunk_id: 12,
        frame_id: 41,
        app: "chrome.exe",
        title: "帳單查詢",
        url: "https://example.com/bill",
      },
    ],
    null,
    false,
    false,
    null,
    { from, to: at(18, 0), said: "昨天下午" },
    [
      {
        start_ts: at(14, 0),
        end_ts: at(14, 45),
        core_start_ts: at(14, 0),
        core_end_ts: at(14, 45),
        core_ms: 45 * 60_000,
        segment_count: 5,
        app: "code.exe",
        title: "db.rs — AI-Sister",
        host: null,
      },
      {
        start_ts: at(14, 45),
        end_ts: at(15, 10),
        core_start_ts: at(14, 45),
        core_end_ts: at(15, 10),
        core_ms: 25 * 60_000,
        segment_count: 3,
        app: "chrome.exe",
        title: "SQLite user_version 文件",
        host: "sqlite.org",
      },
      {
        start_ts: at(15, 10),
        end_ts: at(15, 55),
        core_start_ts: at(15, 10),
        core_end_ts: at(15, 55),
        core_ms: 45 * 60_000,
        segment_count: 5,
        app: "notion.exe",
        title: "週報",
        host: null,
      },
    ],
  );
}

// `?hits=none` 是兩手空空那一版。要看的是「我沒看到過」底下那幾句——它們是
// 這個畫面上唯一會讓他知道「東西可能在，只是我不准看」的地方。
//
// `&blind=` 切幾個講法不同的處境。每一個都是後端真的送得出來的組合，而它們
// 以前有好幾個長得一模一樣：
//   dangling  紀錄裡掛著一段沒收尾的暫停，但她現在沒有暫停
//   flag      反過來，旗標在、紀錄裡什麼都沒有
//   scan      一個字的問題，只翻得動 30 天
//   blocked   一段字都沒有，而原因是暫停／排除，不是「被忘掉了」
//   forgotten 錄過、存過、被忘掉了
//   nulldata  錄過、**沒存過**，而 forget 從來沒被執行過。和上面那個在
//             資料庫上長得一樣，講出來的話必須相反
//   juststarted 沒存過，而且她**此刻正開著**——`nulldata` 那句「先看設定
//             頁」在這台機器上是誤導，這裡要的是「再等一下」
//   erasedlive 存過、被忘掉了，而她此刻正開著。這一種的兩個可能性都成立，
//             所以那句話要**兩個都講**（上一種只准講一個）
const BLIND_DEMOS = {
  "": {
    chunks: 8421,
    excluded: [
      ["excluded url", 12],
      ["excluded app: keepassxc", 3],
    ],
    paused_episodes: 2,
    paused_ms: 4 * 3600 * 1000,
    paused_open: true,
    paused_now: true,
    paused_truncated: 0,
  },
  dangling: {
    chunks: 8421,
    excluded: [],
    paused_episodes: 2,
    paused_ms: 4 * 3600 * 1000,
    paused_open: true,
    paused_now: false,
    paused_truncated: 0,
  },
  flag: {
    chunks: 8421,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: true,
    paused_truncated: 0,
  },
  scan: {
    chunks: 8421,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
    scan_horizon_days: 30,
  },
  // 底下三個配 `&kind=recent` 用：問「剛剛發生什麼事」而空手的三種處境。
  // 標題那一行以前寫死成「我什麼都還沒看到——要先跑 sister record 我才記得
  // 住」，於是前兩種讀起來是上下兩行互相打臉。這三個開關存在的理由就是那
  // 三行要用眼睛比過。
  forgotten: {
    chunks: 0,
    frames: 0,
    ever_recorded: true,
    ever_stored: true,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
  },
  // **和上面那個在資料庫上長得一模一樣，而它們的下一步剛好相反。**
  //
  // `capture.enabled = false` 的那台機器：她開場、跑完、收工，一列內容都沒
  // 進來過，`sister forget` 從來沒被執行過。差別只有 `ever_stored`，而那個
  // 位元以前不存在——於是這一種讀到的是上面那一句「被忘掉了」，一句關於他
  // 的東西被刪掉的假話。這兩行要用眼睛比過。
  nulldata: {
    chunks: 0,
    frames: 0,
    ever_recorded: true,
    ever_stored: false,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
  },
  // 同樣是「她錄過」、chunks == 0，只差在她**正在**錄。以前這兩種印同一
  // 句「被忘掉了，或是過了保留期」，而第二種最常見的成因是他三秒前才按下
  // 「開始記錄」。
  juststarted: {
    chunks: 0,
    frames: 0,
    ever_recorded: true,
    ever_stored: false,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
    recording_now: true,
  },
  // 和 `juststarted` 只差一個 `ever_stored`，而那一個字換掉整句話：她存過
  // 東西、被忘掉了、而且此刻正開著——兩種可能性同時成立，所以那句話兩邊都
  // 要講。上一版這兩顆共用同一句，於是「可能是之前的被忘掉了」被講給一台
  // 從來沒存過任何東西的機器聽。
  erasedlive: {
    chunks: 0,
    frames: 0,
    ever_recorded: true,
    ever_stored: true,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
    recording_now: true,
  },
  blind: {
    chunks: 0,
    ocr_is_dead: true,
    frames: 12000,
    ever_recorded: true,
    ever_stored: true,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
  },
  // **真的壞掉的那台機器長這樣，而上面那個 `blind` 長不出來。**
  //
  // `chunks` 不是 0：`insert_focus` 每次換視窗就寫一列視窗標題進 text_chunks，
  // 一行 OCR 都沒有也照寫。所以 OCR 全死的機器上，舊版那個 `chunks === 0` 的
  // 條件永遠不成立，這一行永遠不會出現——而它是那台機器唯一的正確診斷。
  // 這個開關存在的理由就是那兩種要用眼睛比過。
  ocrdead: {
    chunks: 3000,
    ocr_is_dead: true,
    frames: 40000,
    ever_recorded: true,
    ever_stored: true,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
  },
  fresh: {
    chunks: 0,
    frames: 0,
    ever_recorded: false,
    ever_stored: false,
    excluded: [],
    paused_episodes: 0,
    paused_ms: 0,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
  },
  blocked: {
    chunks: 0,
    frames: 0,
    ever_recorded: true,
    ever_stored: true,
    excluded: [["excluded app: keepassxc", 3]],
    paused_episodes: 1,
    paused_ms: 3600 * 1000,
    paused_open: false,
    paused_now: false,
    paused_truncated: 0,
  },
};
if (browserDemoQuery && params.get("hits") === "none") {
  // `&kind=recent`：同一組空手資料，但問的是時間而不是字。這兩種的**標題**
  // 不一樣，而以前不一樣的方式是錯的——時間那一條寫死了「我什麼都還沒看到
  // ——要先跑 sister record 我才記得住」，於是配上 `&blind=forgotten` 讀起來是
  //
  //     我什麼都還沒看到——要先跑 sister record 我才記得住。
  //     （我錄過，但現在什麼都不剩了——被忘掉了，或是過了保留期。）
  //
  // 上下兩行互相打臉。要用眼睛比的就是這個。
  renderHits(
    [],
    params.get("kind") === "recent" ? "recent" : "keywords",
    null,
    [],
    BLIND_DEMOS[params.get("blind") ?? ""] ?? BLIND_DEMOS[""],
    false,
    false,
    // `?glued=個板`：她拿去比對的字是從「剛剛那個板」黏出來的。看得見這一行
    // 才知道下一步是重打一個詞，而不是去設定頁找一條擋掉它的規則。
    params.get("glued"),
  );
}
