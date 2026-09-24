#!/usr/bin/env node
/*
 * 設定頁按下「儲存」之後，那一格到底有沒有說話。
 *
 * 這一支存在的理由：`save()` 成功的時候會講一句話，然後呼叫 `load()` 把剛
 * 寫進去的那一份讀回來畫上去——而 `load()` 成功的路徑上有一行 `say("")`。
 * 兩行各自都對（重讀一份新的不該留著上一件事的結果；存完要把剪過空白的版本
 * 畫回去），湊起來這一頁從出生到 alpha.37 為止，**每一句「存好了」都在幾毫秒
 * 後被自己抹成空白**。失敗那條路反而活得好好的（`catch` 走不到 `load()`），
 * 所以症狀是：存壞了會說話，存好了什麼都不說。對按下按鈕的人來說，沉默就是
 * 「按了沒反應」。
 *
 * 為什麼是這種寫法：它載入的是 `apps/desktop/ui/settings.js` **原檔**，不是
 * 一份抄過來的邏輯。抄過來的那種測試只證明抄本會動。假 DOM 只有幾十行，夠
 * 這一頁跑完 load / save 一輪。
 *
 * 為什麼要 CI 顧：這一頁沒有任何自動測試，開發機也開不起 Tauri 視窗，所以
 * 這一整類「畫面說了什麼」的錯，一直是靠 Ted 在 Windows 上用眼睛抓。這一條
 * 抓得到的那一種，不必等到他那邊。
 *
 * 後來長出來的另外兩件（⑤⑧⑨⑩），形狀不同但同一族——**錯誤處理自己造成的
 * 傷害**：
 *
 * - 讀不回設定檔的時候整張表會被清空並灰掉（那是對的，一張空白的排除清單
 *   讀起來是「你什麼都沒擋」，而那是假的）。但 `save()` 的 `finally` 緊接著
 *   把儲存鍵點回來——他再按一次，那三份排除規則就被空白覆寫掉了。
 * - 換熱鍵失敗的時候那一格停在「按下你要的那一組…」，而已經沒有人在聽。
 *   那一格是唯一寫著暫停鍵是哪一組的地方，卡在那句話上等於他連現在按哪一顆
 *   會暫停都問不到。
 */

import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { domOf, fakeDocument, loader, read, watchNonsense } from "./fake-dom.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/desktop/ui");
const SRC = process.argv[2] ?? join(UI, "settings.js");
const HTML = read(join(UI, "settings.html"));
const boot = loader(read(SRC));
const catalogBoot = loader(read(join(UI, "personas/catalog.js")));
const MAIN = read(resolve(UI, "../src-tauri/src/main.rs"));
const BRAIN = read(resolve(UI, "../../../crates/sister-core/src/brain.rs"));

function settingsWriteWatching(state) {
  const body = MAIN.match(/fn settings_write\([\s\S]*?struct PrivacyHealth/)?.[0] ?? "";
  if (body.includes("heartbeat::watching_word") && body.includes("heartbeat::presence")) {
    return state;
  }
  const wanted = {
    recording: /Presence::Live\([\s\S]*?Phase::Recording[\s\S]*?\)\s*=>\s*"recording"/.test(body),
    booting: /Presence::Live\([\s\S]*?Phase::Booting[\s\S]*?\)\s*=>\s*"booting"/.test(body),
    thinking: /Presence::Thinking[\s\S]*?=>\s*"thinking"/.test(body),
    none: /Presence::(?:Missing|Stalled)|watching_word/.test(body),
  };
  return wanted[state] ? state : "none";
}

const BASE = {
  path: "C:\\Users\\ted\\AppData\\Roaming\\sister\\config.toml",
  excluded_apps: ["keepassxc"],
  excluded_urls: ["*.bank.example"],
  excluded_titles: [],
  pause_on_screenshare: true,
  redact_clipboard_secrets: true,
  query_log: true,
  remember_told: true,
  frames_days: 14,
  text_days: 90,
  persona_enabled: true,
  persona_id: "chatgpt",
  persona_motion: true,
  persona_tap_lines: true,
};

function brainView(selected = null, { busy = false, custom = false } = {}) {
  const versions = {
    claude: "2.1.266",
    codex: "codex-cli 0.153.2",
    gemini: null,
    grok: "grok 1.0.13",
  };
  const labels = {
    claude: "Claude Code",
    codex: "Codex CLI",
    gemini: "Gemini CLI",
    grok: "Grok CLI",
  };
  return {
    selected,
    custom_configured: custom,
    busy,
    providers: ["claude", "codex", "gemini", "grok"].map((id) => ({
      id,
      label: labels[id],
      installed: id !== "gemini",
      version: versions[id],
      selected: id === selected,
    })),
  };
}

const ASSET_DISCLOSURE = {
  release_id: "persona-pack-v1",
  host: "cdn.example.invalid",
  path: "/ai-sister/persona-pack-v1.zip",
  bytes: 123456789,
  boundary: "只送固定素材包請求；不送任何記錄內容。",
};

const ASSET_AVAILABLE = {
  phase: "available",
  disclosure: ASSET_DISCLOSURE,
  asset_file_bytes: 0,
  portrait_count: 0,
  voice_count: 0,
};

const LOCAL_TTS_OFF = {
  generation: 3,
  config_readable: true,
  enabled: false,
  voice_enabled: false,
  endpoint: "http://127.0.0.1:8231/tts",
  health_endpoint: "http://127.0.0.1:8231/health",
  service: "missing",
  persona: "chatgpt",
  ready: false,
  config_error: null,
};

const USAGE_OFF = {
  generation: 0,
  config_readable: true,
  enabled: false,
  reaction_enabled: false,
  local_sessions_enabled: false,
  local_sessions_dir: "",
  stopped: false,
  served_from: "disabled",
  fetch_error: null,
  local_error: null,
  local_files_read: 0,
  local_files_found: 0,
  local_files_capped: 0,
  local_skipped_auth: 0,
  local_scan_complete: true,
  local_products: [],
  board_live: false,
  board_updated_at: null,
  products: [],
  attribution: "LimitReset（limitreset.net），CC BY 4.0",
  source_name: "LimitReset",
  source_url: "https://limitreset.net/",
  license: "CC BY 4.0",
  endpoint: "https://limitreset.net/api/v1/status",
  host: "limitreset.net",
  last_success_unix_ms: null,
  config_error: null,
};

const AZURE_OFF = {
  generation: 7,
  config_readable: true,
  enabled: false,
  region: null,
  voice: "zh-TW-HsiaoChenNeural",
  endpoint: null,
  credential: "missing",
  consented: false,
  consent_at: null,
  ready: false,
  config_error: null,
};

const AZURE_READY = {
  ...AZURE_OFF,
  enabled: true,
  region: "eastasia",
  endpoint: "https://eastasia.tts.speech.microsoft.com/cognitiveservices/v1",
  credential: "present",
  consented: true,
  consent_at: 1_757_299_200_000,
  ready: true,
};

/*
 * `HotkeyView` 的形狀，照 main.rs 那個 struct 抄的。
 *
 * 上一版這裡寫的是 `{ kind: "held", combo: "Ctrl + Alt + P" }`——一個這支程式
 * 裡不存在的形狀。`paintHotkey` 拿到它會在 `pretty(undefined)` 上炸掉，而那個
 * 例外正好被 `reloadHotkey` 的 catch 吃掉，於是每一個 case 其實都在走**失敗**
 * 那條路，而測試照樣全綠。假資料的形狀不對，等於那一整段沒被測過。
 */
const HOTKEY = {
  wanted: "Ctrl+Alt+KeyP",
  registered: true,
  reason: null,
  rejected: null,
  config_unreadable: null,
  hands_wanted: "Ctrl+Alt+KeyH",
  hands_registered: true,
  hands_reason: null,
  hands_collided: false,
};

const LOGIN_STARTUP_EXPECTED =
  '"C:\\Users\\ted\\AppData\\Local\\AI-Sister\\sister-desktop.exe" --ai-sister-login';
const LOGIN_STARTUP = {
  state: "disabled",
  expected: LOGIN_STARTUP_EXPECTED,
  actual: null,
  reason: null,
};

const PLATFORM_WINDOWS = {
  platform: "windows",
  screen_recording: null,
  accessibility: null,
};

const tick = () => new Promise((r) => setTimeout(r, 20));

async function open({
  config = BASE,
  onRead,
  onWrite,
  onHealth,
  onHotkeySet,
  onAssetStatus,
  onAssetInstall,
  onAssetCancel,
  onAssetRemove,
  onVoiceSet,
  onPersonaRead,
  onLoginStartupRead,
  onLoginStartupSet,
  onPlatformAccessRead,
  onPlatformAccessOpen,
  onDiagnoseExport,
  onAzureRead,
  onAzureConfigSet,
  onAzureKeySet,
  onAzureKeyDelete,
  onLocalTtsRead,
  onLocalTtsConfigSet,
  onBrainRead,
  onBrainConnect,
  onBrainTest,
  onBrainCancel,
  asset = ASSET_AVAILABLE,
  azure = AZURE_OFF,
  localTts = LOCAL_TTS_OFF,
  voice = false,
  hotkey = HOTKEY,
  loginStartup = LOGIN_STARTUP,
  platformAccess = PLATFORM_WINDOWS,
  brain = brainView(),
  cloud = false,
  watching = "recording",
} = {}) {
  // **`domOf` 而不是「要什麼就生什麼」。** 上一版任何選擇器都回一個新的
  // `fakeEl()`，於是⑧那條守著「儲存被灰掉的時候『重新讀取』還按得動」的斷言，
  // 在那顆按鈕被從 settings.html 刪掉、或者被加上 disabled 的時候照樣是綠的
  // ——而它是那一頁唯一的出口。假 DOM 比真的寬鬆一格，它守的那條線就是假的。
  const node = domOf(HTML);
  globalThis.document = fakeDocument(node);
  globalThis.location = { search: "" };
  // 那顆按鍵監聽掛在 window 上，不是掛在那一格上（理由見 settings.js：捕捉
  // 模式底下要吃掉 Tab / Enter，不然瀏覽器會先拿去換焦點）。所以要在這裡接。
  const keys = [];
  const windowEvents = new Map();
  globalThis.addEventListener = (ev, fn) => {
    if (ev === "keydown") keys.push(fn);
    else (windowEvents.get(ev) ?? windowEvents.set(ev, []).get(ev)).push(fn);
  };
  globalThis.removeEventListener = () => {};

  let state = { ...config };
  let assetState = { ...asset, disclosure: asset.disclosure ? { ...asset.disclosure } : null };
  let voiceState = voice;
  let loginStartupState = { ...loginStartup };
  let platformAccessState = { ...platformAccess };
  let azureState = { ...azure };
  let localTtsState = { ...localTts };
  let usageState = { ...USAGE_OFF };
  let brainState = structuredClone(brain);
  const writes = [];
  const invokes = [];
  const events = new Map();
  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, arg) => {
        invokes.push({ cmd, arg });
        switch (cmd) {
          case "settings_read":
            if (onRead) return onRead(state);
            return { ...state };
          case "settings_write":
            writes.push(arg.settings);
            if (onWrite) return onWrite(arg.settings, (s) => (state = { ...state, ...s }));
            state = { ...state, ...arg.settings };
            return { watching: settingsWriteWatching(watching) };
          case "brain_cli_read":
            if (onBrainRead) return onBrainRead(structuredClone(brainState));
            return structuredClone(brainState);
          case "brain_cli_connect": {
            if (onBrainConnect) {
              return onBrainConnect(
                arg.provider,
                structuredClone(brainState),
                (next) => (brainState = structuredClone(next)),
              );
            }
            brainState = brainView(arg.provider);
            return {
              provider: arg.provider,
              label: brainState.providers.find((item) => item.id === arg.provider).label,
              brain: structuredClone(brainState),
            };
          }
          case "brain_cli_test": {
            if (onBrainTest) return onBrainTest(structuredClone(brainState));
            const current = brainState.providers.find((item) => item.selected);
            if (!current) throw new Error("還沒選大腦");
            return {
              provider: current.id,
              label: current.label,
              brain: structuredClone(brainState),
            };
          }
          case "brain_cli_cancel":
            if (onBrainCancel) return onBrainCancel();
            return true;
          case "lint_url_rules":
            return [];
          case "privacy_health":
            if (onHealth) return onHealth(arg.urls);
            return {
              broken: [],
              capture_off: false,
              input_hook: "available",
              url_rules: { kind: "working", reads: 12 },
              at: 1_755_000_000_000,
            };
          case "hotkey_state":
            return hotkey;
          case "hotkey_set":
            if (onHotkeySet) return onHotkeySet(arg.combo);
            return { ...hotkey, wanted: arg.combo, registered: true };
          case "consent_read":
            return {
              path: "C:\\consent.toml",
              current: true,
              allows_recording: true,
              allows_frames: true,
              store_images: true,
              capture_enabled: true,
              reset_by_version: false,
              sheets: [
                {
                  key: "local-recording",
                  wording: "x",
                  without: "x",
                  granted_at: 1,
                  effective: true,
                },
                {
                  key: "cloud-reading",
                  wording: "x",
                  without: "x",
                  granted_at: cloud ? 1 : null,
                  effective: cloud,
                },
                {
                  key: "frame-storage",
                  wording: "x",
                  without: "x",
                  granted_at: 1,
                  effective: true,
                },
                {
                  key: "azure-tts",
                  wording: "x",
                  without: "x",
                  granted_at: null,
                  effective: false,
                },
              ],
            };
          case "recording_state":
            return watching;
          case "login_startup_read":
            if (onLoginStartupRead) return onLoginStartupRead({ ...loginStartupState });
            return { ...loginStartupState };
          case "login_startup_set":
            if (onLoginStartupSet) {
              return onLoginStartupSet(
                arg,
                { ...loginStartupState },
                (s) => (loginStartupState = { ...s }),
              );
            }
            loginStartupState = arg.enabled
              ? {
                  state: "enabled",
                  expected: LOGIN_STARTUP_EXPECTED,
                  actual: LOGIN_STARTUP_EXPECTED,
                  reason: null,
                }
              : {
                  state: "disabled",
                  expected: LOGIN_STARTUP_EXPECTED,
                  actual: null,
                  reason: null,
                };
            return { ...loginStartupState };
          case "platform_access_read":
            if (onPlatformAccessRead) {
              return onPlatformAccessRead({ ...platformAccessState });
            }
            return { ...platformAccessState };
          case "platform_access_open":
            if (onPlatformAccessOpen) {
              return onPlatformAccessOpen(
                arg.kind,
                { ...platformAccessState },
                (next) => (platformAccessState = { ...next }),
              );
            }
            return { ...platformAccessState };
          case "diagnose_export":
            if (onDiagnoseExport) return onDiagnoseExport();
            return "C:\\Users\\ted\\AppData\\Roaming\\ted-h\\AI-Sister\\diagnose-2026-09-22.txt";
          case "persona_asset_status":
            if (onAssetStatus) return onAssetStatus(assetState);
            return {
              ...assetState,
              disclosure: assetState.disclosure ? { ...assetState.disclosure } : null,
            };
          case "persona_asset_install":
            if (onAssetInstall) {
              return onAssetInstall(assetState, (s) => (assetState = { ...s }));
            }
            assetState = {
              ...assetState,
              phase: "installed",
              asset_file_bytes: 100000000,
              portrait_count: 4,
              voice_count: 8,
            };
            return null;
          case "persona_asset_cancel":
            if (onAssetCancel) {
              return onAssetCancel(assetState, (s) => (assetState = { ...s }));
            }
            assetState = { ...assetState, phase: "available", asset_file_bytes: 0 };
            return null;
          case "persona_asset_remove":
            if (onAssetRemove) {
              return onAssetRemove(assetState, (s) => (assetState = { ...s }));
            }
            assetState = {
              ...ASSET_AVAILABLE,
              disclosure: { ...ASSET_DISCLOSURE },
            };
            voiceState = false;
            return null;
          case "persona_read":
            if (onPersonaRead) return onPersonaRead(voiceState);
            return { id: state.persona_id, voice_enabled: voiceState };
          case "persona_voice_set":
            if (onVoiceSet) {
              return onVoiceSet(arg, (enabled) => (voiceState = enabled));
            }
            voiceState = arg.enabled;
            return { voice_enabled: voiceState };
          case "local_tts_read":
            if (onLocalTtsRead) return onLocalTtsRead({ ...localTtsState });
            return { ...localTtsState };
          case "local_tts_config_set":
            if (onLocalTtsConfigSet) {
              return onLocalTtsConfigSet(arg, { ...localTtsState }, (next) => {
                localTtsState = { ...next };
              });
            }
            localTtsState = {
              ...localTtsState,
              enabled: arg.enabled,
              ready: arg.enabled === true && localTtsState.service === "ready",
            };
            return { ...localTtsState };
          case "azure_tts_read":
            if (onAzureRead) return onAzureRead({ ...azureState });
            return { ...azureState };
          case "azure_tts_config_set":
            if (onAzureConfigSet) {
              return onAzureConfigSet(
                arg,
                { ...azureState },
                (next) => (azureState = { ...next }),
              );
            }
            azureState = {
              ...azureState,
              enabled: arg.enabled,
              region: arg.region,
              voice: arg.voice,
              endpoint:
                arg.region === null
                  ? null
                  : `https://${arg.region}.tts.speech.microsoft.com/cognitiveservices/v1`,
            };
            azureState.ready =
              azureState.enabled &&
              azureState.region !== null &&
              azureState.credential === "present" &&
              azureState.consented === true;
            return { ...azureState };
          case "azure_tts_key_set":
            if (onAzureKeySet) {
              return onAzureKeySet(
                arg,
                { ...azureState },
                (next) => (azureState = { ...next }),
              );
            }
            azureState = { ...azureState, credential: "present" };
            azureState.ready =
              azureState.enabled && azureState.region !== null && azureState.consented === true;
            return { ...azureState };
          case "usage_status_read":
            return { ...usageState };
          case "usage_public_status_set":
            usageState = {
              ...usageState,
              enabled: arg.enabled,
              reaction_enabled: arg.reactionEnabled,
              local_sessions_enabled: arg.localSessionsEnabled,
              local_sessions_dir: arg.localSessionsDir,
              served_from: arg.enabled ? "network" : "disabled",
              board_live: false,
            };
            return { ...usageState };
          case "usage_public_status_refresh":
            if (usageState.stopped) {
              return { ...usageState, served_from: "stopped", fetch_error: null };
            }
            if (!usageState.enabled) {
              return { ...usageState, served_from: "disabled" };
            }
            usageState = {
              ...usageState,
              served_from: "network",
              board_live: true,
              fetch_error: null,
              products: [
                {
                  id: "codex",
                  name: "Codex",
                  reset: "confirmed",
                  event_id: "codex:2026-09-12",
                  announced_at: "2026-09-12T03:20:36.000Z",
                  public_event_count: 32,
                  forecast_p24: 0,
                  forecast_p48: 0.27,
                  forecast_basis: "empirical",
                },
              ],
              local_products: [
                {
                  id: "codex",
                  name: "Codex",
                  observed_tokens: 125,
                  remaining_tokens: null,
                  observed_at_unix_ms: 1_757_644_836_000,
                  quota_used_percent: 12,
                  quota_window_minutes: 300,
                  quota_resets_at_unix: 1783800000,
                  provenance: "codex-session-jsonl",
                },
              ],
            };
            return { ...usageState };
          case "azure_tts_key_delete":
            if (onAzureKeyDelete) {
              return onAzureKeyDelete(
                { ...azureState },
                (next) => (azureState = { ...next }),
              );
            }
            azureState = { ...azureState, credential: "missing", ready: false };
            return { ...azureState };
          case "open_onboarding":
            return null;
          default:
            return null;
        }
      },
    },
    event: {
      listen: async (name, handler) => {
        (events.get(name) ?? events.set(name, []).get(name)).push(handler);
        return () => {};
      },
    },
  };

  const nonsense = watchNonsense();
  // 真 settings.html 會先跑 classic local catalog，再跑 settings.js；fixture 也要
  // 維持同一個順序，不能讓產品因測試漏載依賴而走一條真人不會走的 fallback。
  await catalogBoot();
  await boot();
  await tick();

  const descendants = (root) => {
    const found = [];
    const walk = (current) => {
      for (const child of current?.children ?? []) {
        found.push(child);
        walk(child);
      }
    };
    walk(root);
    return found;
  };
  const personaRadios = () =>
    descendants(node("[data-persona-choice-groups]")).filter(
      (candidate) => candidate.tag === "input" && candidate.type === "radio",
    );
  return {
    node,
    writes,
    invokes,
    nonsense,
    say: () => node("[data-say]").textContent,
    bad: () => node("[data-say]").classList.contains("bad"),
    /**
     * 按一下儲存。回傳「這一下真的按到了沒」。
     *
     * 真的瀏覽器不會把 click 送給一顆 disabled 的按鈕，所以這裡也不可以送——
     * 不然「那顆按鈕該不該是灰的」這件事在這支測試裡永遠驗不出來，而它守的
     * 正是那個：讀不回設定檔的時候整張表會被清空，那時候按鈕還亮著的話，
     * 下一下就把空白寫回檔案裡。
     */
    async save() {
      if (node("[data-save]").disabled) return false;
      for (const fn of node("[data-save]").handlers.click ?? []) fn();
      await tick();
      return true;
    },
    async reload() {
      if (node("[data-reload]").disabled) return false;
      for (const fn of node("[data-reload]").handlers.click ?? []) fn();
      await tick();
      return true;
    },
    async act(selector, { trusted = true, event = "click" } = {}) {
      const target = node(selector);
      if (target.disabled || target.hidden) return false;
      for (const fn of target.handlers[event] ?? []) fn({ isTrusted: trusted });
      await tick();
      return true;
    },
    async emit(name, payload = null) {
      for (const fn of events.get(name) ?? []) fn({ payload });
      await tick();
    },
    personaRadios,
    personaImages: () =>
      descendants(node("[data-persona-choice-groups]")).filter(
        (candidate) => candidate.tag === "img",
      ),
    async choosePersona(id) {
      const target = personaRadios().find((radio) => radio.value === id);
      if (!target || target.disabled) return false;
      target.checked = true;
      for (const fn of target.handlers.change ?? []) fn({ isTrusted: true });
      await tick();
      return true;
    },
    previewChildren: () => descendants(node("[data-persona-preview-media]")),
    setAsset(s) {
      assetState = { ...s, disclosure: s.disclosure ? { ...s.disclosure } : null };
    },
    setUsage(s) {
      usageState = { ...USAGE_OFF, ...s };
    },
    setAzure(s) {
      azureState = { ...s };
    },
    setLocalTts(s) {
      localTtsState = { ...s };
    },
    combo: () => node("[data-combo]").textContent,
    hotkeySay: () => node("[data-hotkey-say]").textContent,
    handsCombo: () => node("[data-hands-combo]").textContent,
    handsHotkeySay: () => node("[data-hands-hotkey-say]").textContent,
    brainSay: () => node("[data-brain-say]").textContent,
    brainHidden: () => node("[data-brain-say]").hidden,
    loginStartupSay: () => node("[data-login-startup-say]").textContent,
    platformAccessSay: () => node("[data-platform-access-say]").textContent,
    health: () => node("[data-health]").textContent,
    healthHidden: () => node("[data-health]").hidden,
    healthUnknown: () => node("[data-health]").classList.contains("unknown"),
    healthOk: () => node("[data-health]").classList.contains("ok"),
    machine: () => node("[data-machine]").textContent,
    machineHidden: () => node("[data-machine]").hidden,
    machineUnknown: () => node("[data-machine]").classList.contains("unknown"),
    async changeLoginStartup(checked, { trusted = true } = {}) {
      const target = node("[data-login-startup]");
      if (target.disabled) return false;
      // 原生 checkbox 的 click 會先落到新 checked 值、拿掉 indeterminate，再送
      // change；假 DOM 要走同一個順序，否則測到的是另一個控制。
      target.checked = checked;
      target.indeterminate = false;
      for (const fn of target.handlers.change ?? []) fn({ isTrusted: trusted });
      await tick();
      return true;
    },
    setPlatformAccess(next) {
      platformAccessState = { ...next };
    },
    async emitWindow(name) {
      for (const fn of windowEvents.get(name) ?? []) fn();
      await tick();
    },
    /** 按那一格 → 進捕捉模式 → 按一組鍵下去。和真人的順序一樣。 */
    async pressCombo(e) {
      for (const fn of node("[data-combo]").handlers.click ?? []) fn();
      await tick();
      for (const fn of keys) {
        fn({ preventDefault() {}, stopPropagation() {}, ctrlKey: false, altKey: false, shiftKey: false, ...e });
      }
      await tick();
    },
  };
}

// 「先前記下的那些不會消失」那一句的指紋。
const KEPT = "從現在起她不會再記你問過的問題";

let failed = 0;
let passed = 0;
function check(name, ok, detail) {
  if (ok) passed++;
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

function calls(p, command) {
  return p.invokes.filter(({ cmd }) => cmd === command);
}

console.log("A149. 設定頁同意書朗讀讀寫");
for (const enabled of [false, true]) {
  const p = await open({ config: { ...BASE, persona_consent_read_aloud: enabled } });
  check(`A149 設定頁讀回朗讀 ${enabled}`, p.node("[data-persona-consent-read-aloud]").checked === enabled);
  p.node("[data-persona-consent-read-aloud]").checked = !enabled;
  await p.save();
  check(`A149 設定頁保存朗讀 ${!enabled} 並重讀`, p.writes[0]?.persona_consent_read_aloud === !enabled && p.node("[data-persona-consent-read-aloud]").checked === !enabled);
}
if (process.env.A149_ONLY === "1") process.exit(failed ? 1 : 0);

console.log("⓪ᴍ macOS 權限是原生真值、trusted click 與回到視窗後重讀");
{
  const denied = {
    platform: "macos",
    screen_recording: false,
    accessibility: false,
  };
  const p = await open({
    platformAccess: denied,
    onPlatformAccessOpen: (kind, current, store) => {
      const next = {
        ...current,
        screen_recording: kind === "screen-recording" ? true : current.screen_recording,
        accessibility: kind === "accessibility" ? true : current.accessibility,
      };
      store(next);
      return next;
    },
  });
  check("macOS 只顯示自己的權限區", p.node("[data-platform-access-section]").hidden === false && p.node("[data-login-startup-section]").hidden === true);
  check("兩項 native false 都畫成未開啟", p.node("[data-platform-screen-state]").textContent === "未開啟" && p.node("[data-platform-ax-state]").textContent === "未開啟");
  await p.act("[data-platform-screen-open]", { trusted: false });
  check("script 假 click 不會要求 TCC 或開設定", calls(p, "platform_access_open").length === 0, p.invokes);
  await p.act("[data-platform-screen-open]");
  await p.act("[data-platform-ax-open]");
  check(
    "兩個真人按鈕只送 exact typed 權限項目",
    JSON.stringify(calls(p, "platform_access_open").map(({ arg }) => arg)) ===
      '[{"kind":"screen-recording"},{"kind":"accessibility"}]',
    p.invokes,
  );
  check("後端回條把兩項都畫成已開啟", p.node("[data-platform-screen-state]").textContent === "已開啟" && p.node("[data-platform-ax-state]").textContent === "已開啟");
  p.setPlatformAccess({ ...denied, screen_recording: true, accessibility: true });
  await p.emitWindow("focus");
  check("切回設定頁會重讀 native truth", calls(p, "platform_access_read").length === 2, p.invokes);
  check("兩項都成立才顯示就緒", p.platformAccessSay() === "畫面、讀字與隱私排除已就緒。", p.platformAccessSay());
}

{
  const p = await open({
    platformAccess: { platform: "linux", screen_recording: null, accessibility: null },
  });
  check("Linux 不顯示 Windows 或 macOS 專屬控制", p.node("[data-login-startup-section]").hidden === true && p.node("[data-platform-access-section]").hidden === true);
}

console.log("⓪ Windows 登入項五態不會把 Unknown 畫成 Off");
{
  const cases = [
    {
      name: "enabled",
      view: {
        state: "enabled",
        expected: LOGIN_STARTUP_EXPECTED,
        actual: LOGIN_STARTUP_EXPECTED,
        reason: null,
      },
      visual: "true/false/false",
      says: ["已登錄", "工作管理員仍可另外停用"],
    },
    {
      name: "disabled",
      view: { ...LOGIN_STARTUP },
      visual: "false/false/false",
      says: ["未登錄", "下次登入不會由這一項啟動"],
    },
    {
      name: "mismatch",
      view: {
        state: "mismatch",
        expected: LOGIN_STARTUP_EXPECTED,
        actual: '"D:\\old\\AI-Sister.exe"',
        reason: "HKCU Run 不是這一版命令",
      },
      visual: "false/true/false",
      says: [
        "命令不相符",
        "不能算已開啟",
        "預期：",
        "目前：",
        "HKCU Run 不是這一版命令",
      ],
    },
    {
      name: "unreadable",
      view: {
        state: "unreadable",
        expected: LOGIN_STARTUP_EXPECTED,
        actual: null,
        reason: "拒絕存取 registry",
      },
      visual: "false/true/true",
      says: ["狀態未知", "不能把它當成關閉", "拒絕存取 registry"],
    },
    {
      name: "unsupported",
      view: {
        state: "unsupported",
        expected: null,
        actual: null,
        reason: null,
      },
      visual: "false/false/true",
      says: ["免安裝/診斷版需先用 Setup", "不管理 Windows 登入項"],
    },
  ];
  const sentences = [];
  const visuals = [];
  for (const c of cases) {
    const p = await open({ loginStartup: c.view });
    const box = p.node("[data-login-startup]");
    const visual = `${box.checked}/${box.indeterminate}/${box.disabled}`;
    visuals.push(visual);
    sentences.push(p.loginStartupSay());
    check(`${c.name} 的 checkbox 形狀`, visual === c.visual, visual);
    check(
      `${c.name} 的文案只描述這一態`,
      c.says.every((part) => p.loginStartupSay().includes(part)),
      p.loginStartupSay(),
    );
  }
  check("五態的 checkbox 形狀沒有兩態相同", new Set(visuals).size === 5, visuals);
  check("五態的完整文字沒有兩態相同", new Set(sentences).size === 5, sentences);
}

console.log("⓪ᵇ 登入啟動的後果、同意書與暫停都在控制旁邊說清楚");
{
  const compact = HTML.replace(/\s+/g, "");
  check(
    "只在背景常駐且不彈窗",
    compact.includes("登入Windows後在背景啟動AI-Sister") &&
      compact.includes("不彈出角色或同意書視窗"),
    "settings.html login startup copy",
  );
  check(
    "有效第一張同意書才開始 recorder",
    compact.includes("只有第一張「本機記錄」同意書仍有效時才會開始recorder"),
    "settings.html login startup copy",
  );
  check(
    "不解除 pause，關掉也不停本輪",
    compact.includes("不會解除你已經按下的暫停") &&
      compact.includes("關掉只影響下次登入，不會停止這一輪正在跑的desktop或recorder"),
    "settings.html login startup copy",
  );
}

console.log("⓪ᶜ config.toml 讀壞仍獨立讀得到、也改得到 Windows 登入項");
{
  const p = await open({
    onRead: () => {
      throw new Error("config.toml 壞了");
    },
  });
  check("一般儲存已 fail closed", p.node("[data-save]").disabled === true);
  check("registry 仍讀了一次", calls(p, "login_startup_read").length === 1, p.invokes);
  check("已知 disabled 仍可操作，不被 config 一起灰掉", p.node("[data-login-startup]").disabled === false);
  await p.changeLoginStartup(true);
  const sets = calls(p, "login_startup_set");
  check(
    "可信勾選只送 enabled true",
    sets.length === 1 && JSON.stringify(sets[0].arg) === '{"enabled":true}',
    sets,
  );
  check("寫完依後端回條畫成已登錄", p.node("[data-login-startup]").checked && p.loginStartupSay().includes("已登錄"), p.loginStartupSay());
}

console.log("⓪ᵈ 一般儲存不會順手改 Windows 登入項");
{
  const p = await open();
  await p.save();
  check("settings_write 照常發生", p.writes.length === 1, p.writes);
  check("但 login_startup_set 一次都沒有", calls(p, "login_startup_set").length === 0, p.invokes);
  check(
    "settings payload 也沒有夾帶 registry 狀態",
    !("login_startup" in p.writes[0]) && !("login_startup_enabled" in p.writes[0]),
    p.writes[0],
  );
}

console.log("⓪ᵉ 只有 trusted change 能寫，mismatch 一次勾選會修成 enabled");
{
  const mismatch = {
    state: "mismatch",
    expected: LOGIN_STARTUP_EXPECTED,
    actual: '"D:\\old\\AI-Sister.exe"',
    reason: null,
  };
  const p = await open({ loginStartup: mismatch });
  await p.changeLoginStartup(true, { trusted: false });
  check("script 假事件沒有寫 registry", calls(p, "login_startup_set").length === 0, p.invokes);
  check(
    "假事件也沒有把 Unknown 留成假的 On",
    p.node("[data-login-startup]").indeterminate && !p.node("[data-login-startup]").checked,
  );
  await p.changeLoginStartup(true);
  check("真人勾選只寫一次", calls(p, "login_startup_set").length === 1, p.invokes);
  check("mismatch 修成後端確認的 enabled", p.node("[data-login-startup]").checked && !p.node("[data-login-startup]").indeterminate, p.loginStartupSay());
}

console.log("⓪ᶠ 寫入忙碌時不能重入");
{
  let finish = null;
  const p = await open({
    onLoginStartupSet: (_arg, _old, store) =>
      new Promise((resolveSet) => {
        finish = () => {
          const enabled = {
            state: "enabled",
            expected: LOGIN_STARTUP_EXPECTED,
            actual: LOGIN_STARTUP_EXPECTED,
            reason: null,
          };
          store(enabled);
          resolveSet(enabled);
        };
      }),
  });
  await p.changeLoginStartup(true);
  check("第一趟還沒完成時開關是灰的", p.node("[data-login-startup]").disabled === true);
  check("第二下按不到", (await p.changeLoginStartup(false)) === false);
  check("所以 set 仍只有一趟", calls(p, "login_startup_set").length === 1, p.invokes);
  finish();
  await tick();
  check("完成後依回條恢復可操作的 On", p.node("[data-login-startup]").checked && !p.node("[data-login-startup]").disabled, p.loginStartupSay());
}

console.log("⓪ᵍ set 失敗後一定重讀，不拿點擊後的勾勾冒充結果");
{
  let reads = 0;
  const p = await open({
    onLoginStartupRead: () => {
      reads += 1;
      return reads === 1
        ? { ...LOGIN_STARTUP }
        : {
            state: "enabled",
            expected: LOGIN_STARTUP_EXPECTED,
            actual: LOGIN_STARTUP_EXPECTED,
            reason: null,
          };
    },
    onLoginStartupSet: () => {
      throw new Error("寫 registry 時拒絕存取");
    },
  });
  await p.changeLoginStartup(true);
  check("開場一次、set 失敗後再 read 一次", reads === 2, reads);
  check("仍保留真正的寫入錯誤", p.loginStartupSay().includes("寫 registry 時拒絕存取"), p.loginStartupSay());
  check("同時明講重讀後其實已登錄", p.loginStartupSay().includes("重新讀取後：已登錄"), p.loginStartupSay());
  check("勾勾依 readback，不依 set 的 throw", p.node("[data-login-startup]").checked === true);
}

{
  let reads = 0;
  const p = await open({
    onLoginStartupRead: () => {
      if (++reads === 1) return { ...LOGIN_STARTUP };
      throw new Error("連 readback 也失敗");
    },
    onLoginStartupSet: () => {
      throw new Error("set 失敗");
    },
  });
  await p.changeLoginStartup(true);
  check(
    "set 和 readback 都失敗就成為正式 Unknown",
    p.node("[data-login-startup]").indeterminate &&
      p.node("[data-login-startup]").disabled &&
      p.loginStartupSay().includes("set 失敗") &&
      p.loginStartupSay().includes("連 readback 也失敗"),
    p.loginStartupSay(),
  );
}

{
  const p = await open();
  check("親口告訴她的話預設勾選", p.node("[data-remember-told]").checked === true);
  p.node("[data-remember-told]").checked = false;
  await p.save();
  check("關閉親口記憶說清楚舊資料保留", p.say().includes("不會再記你告訴她的話") && p.say().includes("先前記下的那些不會因為這個動作消失"), p.say());
  check("重新讀取仍為關閉", p.node("[data-remember-told]").checked === false);
  await p.save();
  check("沒有重複關閉通知", !p.say().includes("不會再記你告訴她的話"));
}

{
  const readBody = MAIN.match(/fn settings_read\([\s\S]*?fn /)?.[0] ?? "";
  const writeBody = MAIN.match(/fn settings_write\([\s\S]*?struct PrivacyHealth/)?.[0] ?? "";
  check("native 設定讀取接上親口記憶", readBody.includes("remember_told: c.privacy.remember_told,"));
  check("native 設定寫入接上親口記憶", writeBody.includes("c.privacy.remember_told = settings.remember_told;"));
}

console.log("① 題庫本來開著，關掉按儲存");
{
  const p = await open();
  p.node("[data-querylog]").checked = false;
  await p.save();
  check("那一格不是空的", p.say() !== "", p.say());
  check("說了「存好了」", p.say().includes("存好了"), p.say());
  check("說了「先前記下的那些不會消失」", p.say().includes(KEPT), p.say());
  check("是兩行，不是黏成一長條", p.say().split("\n").length === 2, p.say());
  // 假設定檔少抄一欄的時候，那幾個天數欄位會印出 undefined，而上面每一條
  // 斷言都不會發現——它們問的都是別的句子。見 fake-dom.mjs 的 `watchNonsense`。
  check("畫面上沒有出現過 NaN / undefined", p.nonsense().length === 0, p.nonsense());

  // 存完 `load()` 會把 `queryLogWas` 換成剛存進去的那一份，所以第二次按下去
  // 沒有人「剛剛關掉」任何東西。少了這一條，那句話會變成每次儲存都出現的
  // 背景雜訊——而它是一句只在那一下成立的話。
  console.log("② 同一頁上，關著的狀態下再按一次儲存");
  await p.save();
  check("還是說了「存好了」", p.say().includes("存好了"), p.say());
  check("沒有再講一次「先前記下的那些」", !p.say().includes(KEPT), p.say());
}

console.log("③ 題庫一直開著，按儲存");
{
  const p = await open();
  await p.save();
  check("說了「存好了」", p.say().includes("存好了"), p.say());
  check("沒有那句多的", !p.say().includes(KEPT), p.say());
}

console.log("④ 題庫本來關著，打開按儲存");
{
  const p = await open({ config: { ...BASE, query_log: false } });
  p.node("[data-querylog]").checked = true;
  await p.save();
  check("說了「存好了」", p.say().includes("存好了"), p.say());
  check("沒有那句多的", !p.say().includes(KEPT), p.say());
}

// 寫進去了、再讀出來卻讀不出來，多半是我們剛剛把那個檔寫壞了。那件事比
// 「存好了」急，而且「存好了」在那個當下已經不是一句完整的真話。
console.log("⑤ 存成功，但存完那次重讀炸了");
{
  let reads = 0;
  const p = await open({
    onRead: (s) => {
      if (++reads > 1) throw new Error("retention.frames_days 不能是 0");
      return { ...s };
    },
  });
  await p.save();
  check("留著那則解析錯誤", p.say().includes("frames_days"), p.say());
  check("沒有被「存好了」蓋掉", !p.say().includes("存好了"), p.say());
  check("而且是紅的", p.bad(), p.bad());

  // 讀不回來的時候 `setUnreadable(true)` 會把三個排除框**清空**並灰掉整張表
  // ——包括儲存鍵。那是對的：一張空白的排除清單讀起來是「你什麼都沒擋」，
  // 而它是假的。但 `save()` 的 `finally` 緊接著又把儲存鍵點亮了。
  //
  // 於是畫面是：三格排除規則空白、儲存鍵亮著。他再按一次，`[]` 就寫進
  // excluded_apps / urls / titles——九條 app、十六條網址靜靜消失，而這一頁
  // 從頭到尾沒有一句話說發生了這件事。`days()` 也擋不住：那兩個天數欄位
  // 沒被清掉。
  check("儲存鍵留在灰的", p.node("[data-save]").disabled === true, p.node("[data-save]").disabled);
  check("排除清單是空的（這是 setUnreadable 做的，故意的）", p.node("[data-apps]").value === "");
  check(
    "大腦那一句不可以變成「還沒填命令」（框被清空了，那是假的）",
    p.brainHidden() === true || !p.brainSay().includes("還沒填命令"),
    p.brainSay(),
  );
  const before = p.writes.length;
  check("再按一次按不動", (await p.save()) === false);
  check(
    "所以沒有第二次寫入把排除規則洗成空的",
    p.writes.length === before,
    p.writes[before]?.excluded_apps,
  );
}

console.log("⑥ 存不進去");
{
  const p = await open({
    onWrite: () => {
      throw new Error("寫不進去：拒絕存取");
    },
  });
  await p.save();
  check("留著那則寫入錯誤", p.say().includes("拒絕存取"), p.say());
  check("而且是紅的", p.bad(), p.bad());
}

// 三向那句話（#50）也是被同一行抹掉的，所以它也要有人顧。
console.log("⑦ 沒有人在錄的時候按儲存");
{
  const p = await open({
    watching: "none",
    onWrite: (s, commit) => {
      commit(s);
      return { watching: "none" };
    },
  });
  await p.save();
  check("留著「等你按下『開始記錄』才會生效」", p.say().includes("開始記錄"), p.say());
}

console.log("⑦ᵇ heartbeat 讀不懂時，存成功也不能冒充沒有人在錄");
{
  const p = await open({ watching: "unreadable" });
  await p.save();
  check("保留存成功回條", p.say().includes("存好了"), p.say());
  check("明講 heartbeat 讀不懂", p.say().includes("讀不懂 recording.beat"), p.say());
  check("不叫人按一顆可能失敗的開始鍵", !p.say().includes("按下「開始記錄」"), p.say());
}

// 灰掉一顆按鈕很容易變成「他從此出不去」。⑤ 那條線只有在這一條也成立的時候
// 才是對的：讀得回來，人就要能回到能存的狀態。
console.log("⑧ 讀不回來之後，「重新讀取」要把人救回來");
{
  let reads = 0;
  const p = await open({
    onRead: (s) => {
      // 第一次是開場那次（好），第二次是存完那次（炸），第三次是他按重新讀取。
      if (++reads === 2) throw new Error("retention.frames_days 不能是 0");
      return { ...s };
    },
  });
  await p.save();
  check("先卡在灰的", p.node("[data-save]").disabled === true);
  check("「重新讀取」沒有跟著被灰掉", (await p.reload()) === true);
  check("排除規則回來了", p.node("[data-apps]").value.includes("keepassxc"), p.node("[data-apps]").value);
  check("那則錯誤不再掛著", !p.say().includes("frames_days"), p.say());
  check("儲存鍵按得下去了", (await p.save()) === true);
  check("而且它說了話", p.say().includes("存好了"), p.say());
}

// 那一格是**唯一**寫著暫停鍵是哪一組的地方。按下去它會變成一句承諾——
// 「按下你要的那一組…」，我在聽。承諾的壽命不可以比 `capturing` 長。
console.log("⑨ 換熱鍵，而後端換不成");
{
  const p = await open({
    onHotkeySet: () => {
      // `hotkey_set` 唯一會 Err 的路：搶到了但寫不進設定檔，於是退回舊的那組。
      throw new Error("搶到了，但存不進設定檔，所以退回原來那一組。還在用 Ctrl + Alt + P。\n拒絕存取");
    },
  });
  check("開場那一格印的是人看得懂的那一種", p.combo() === "Ctrl + Alt + P", p.combo());
  await p.pressCombo({ key: "s", code: "KeyS", ctrlKey: true, altKey: true });
  check("說得出為什麼沒換成", p.hotkeySay().includes("拒絕存取"), p.hotkeySay());
  check(
    "那一格不可以停在「按下你要的那一組…」",
    !p.combo().includes("按下你要的那一組"),
    p.combo(),
  );
  check("它退回還在生效的那一組", p.combo() === "Ctrl + Alt + P", p.combo());

  // 而且退回去之後要還能再按一次——不然他被鎖在一個什麼都改不了的畫面上。
  await p.pressCombo({ key: "d", code: "KeyD", ctrlKey: true, altKey: true });
  check("再試一次還是接得到鍵盤", p.hotkeySay().includes("拒絕存取"), p.hotkeySay());
}

console.log("⑩ 換熱鍵，這次成功");
{
  const p = await open();
  await p.pressCombo({ key: "s", code: "KeyS", ctrlKey: true, altKey: true });
  check("那一格換成新的那一組", p.combo() === "Ctrl + Alt + S", p.combo());
  check("說了搶到了", p.hotkeySay().includes("搶到了"), p.hotkeySay());
}

console.log("⑩ᵇ 拔手熱鍵：搶到、搶不到、關掉、撞號都畫得出來");
{
  const cases = [
    {
      name: "搶到",
      hotkey: { ...HOTKEY },
      premise: (h) => h.hands_registered === true && h.hands_wanted !== "" && !h.hands_collided,
      says: "搶到了",
      combo: "Ctrl + Alt + H",
    },
    {
      name: "搶不到",
      hotkey: { ...HOTKEY, hands_registered: false, hands_reason: "已被佔用" },
      premise: (h) => h.hands_registered === false && h.hands_wanted !== "" && !h.hands_collided,
      says: "這一組搶不到（已被佔用）",
      combo: "Ctrl + Alt + H",
    },
    {
      name: "關掉",
      hotkey: { ...HOTKEY, hands_wanted: "", hands_registered: false },
      premise: (h) => h.hands_registered === false && h.hands_wanted === "" && !h.hands_collided,
      says: "拔手熱鍵是關掉的",
      combo: "沒有設",
    },
    {
      name: "撞號",
      hotkey: { ...HOTKEY, hands_collided: true },
      premise: (h) => h.hands_collided === true && h.hands_wanted !== "",
      says: "和暫停熱鍵撞號了",
      combo: "Ctrl + Alt + H",
    },
  ];
  for (const c of cases) {
    check(`${c.name}的前提是真的`, c.premise(c.hotkey), c.hotkey);
    const p = await open({ hotkey: c.hotkey });
    check(`${c.name}時印出人看得懂的組合`, p.handsCombo() === c.combo, p.handsCombo());
    check(`${c.name}時說得出狀態`, p.handsHotkeySay().includes(c.says), p.handsHotkeySay());
  }
}

console.log("⑪ 存不進去，而且退回去的那一組現在也搶不到了");
{
  // `hotkey_set` 那條路上第三種分支：`!restored.registered && !wanted.is_empty()`。
  // 這一格照樣印那一組，而那一組現在**沒有人在聽**——看起來像在說謊。
  //
  // 不是。這一格答的是「暫停鍵設成哪一組」（設定檔沒被改動，答案就是舊的那一
  // 組），而「它現在搶不搶得到」是底下那一句的工作，`paintHotkey` 對同一種
  // 情況也是這樣分工的（`view.registered ? … : …`，那一格照印）。
  //
  // 這一條把那個分工釘住：兩件事都要說得出口，而且要是紅的。少了任何一半，
  // 這一格就變成一句「按這個會暫停」的斷言。
  const p = await open({
    onHotkeySet: () => {
      throw new Error(
        "搶到了，但存不進設定檔，所以退回原來那一組。而舊的那組現在也搶不到了——改用系統匣裡的暫停。\n拒絕存取",
      );
    },
  });
  await p.pressCombo({ key: "s", code: "KeyS", ctrlKey: true, altKey: true });
  check("那一格還是答得出「設成哪一組」", p.combo() === "Ctrl + Alt + P", p.combo());
  check("而底下那句要說它現在搶不到", p.hotkeySay().includes("也搶不到了"), p.hotkeySay());
  check("而且是紅的", p.node("[data-hotkey-say]").classList.contains("bad"), p.hotkeySay());
}

const SENTENCE = {
  noCli: "選一個已安裝的 CLI。",
  noConsent: "Claude Code 已接好。完成第二張「雲端解讀」後，文字問題才會交給它。",
  ready: "Claude Code 已接好，會接手每個文字問題並查本機記憶。",
};

console.log("⑫ 大腦：四支 CLI 都由 native 偵測，未選時只有一個下一步");
{
  const p = await open({ cloud: true, watching: "recording" });
  check("就是那一句", p.brainSay() === SENTENCE.noCli, p.brainSay());
  check("沒安裝的 Gemini 不能按", p.node("[data-brain-gemini-action]").disabled, "gemini");
  check("已安裝的 Claude 可以按", !p.node("[data-brain-claude-action]").disabled, "claude");
  check("開頁只讀狀態，沒有啟動登入", calls(p, "brain_cli_connect").length === 0, p.invokes);
}

console.log("⑬ 大腦：CLI 已接好、第二張同意書沒勾");
{
  const p = await open({
    brain: brainView("claude"),
    cloud: false,
    watching: "recording",
  });
  check("就是那一句", p.brainSay() === SENTENCE.noConsent, p.brainSay());
  check("指得出要完成第二張", p.brainSay().includes("第二張「雲端解讀」"), p.brainSay());
  check("Claude 卡片顯示已選用", p.node("[data-brain-claude-status]").textContent.includes("已選用"), p.node("[data-brain-claude-status]").textContent);
}

console.log("⑭–⑯ 大腦：選用後接手每個文字問題，不綁 recorder 狀態");
for (const watching of ["none", "recording", "booting", "thinking"]) {
  const p = await open({ brain: brainView("claude"), cloud: true, watching });
  check(`${watching} 都是同一個已選大腦`, p.brainSay() === SENTENCE.ready, p.brainSay());
  check(`${watching} 都是綠的`, p.node("[data-brain-say]").classList.contains("ok"), p.brainSay());
}

console.log("⑯ᵇ Windows 背景 CLI 不彈 console；只有互動式登入保留可見終端機");
{
  const managed = BRAIN.match(
    /#\[cfg\(windows\)\][\s\S]*?pub fn configure_managed_process[\s\S]*?command\.creation_flags\(flags\);/,
  )?.[0] ?? "";
  check(
    "visible branch 用 CREATE_NEW_CONSOLE，background branch 用 CREATE_NO_WINDOW",
    /if visible_console\s*\{[\s\S]*CREATE_NEW_CONSOLE[\s\S]*\}\s*else\s*\{[\s\S]*CREATE_NO_WINDOW/.test(
      managed,
    ),
    managed,
  );
}

console.log("⑯ᶜ 存檔回條：上一場剛停時不可以叫他按一顆會被擋的開始鍵");
{
  const p = await open({ watching: "thinking" });
  await p.save();
  check("存檔回條說得出還在收尾", p.say().includes("還在把最後一段想完"), p.say());
  check("沒有叫他按開始記錄", !p.say().includes("等你按下「開始記錄」"), p.say());
}

console.log("⑯ᵈ 登入成功才獨立保存，不經頁尾 settings_write");
{
  const p = await open({
    brain: brainView("claude"),
    onBrainConnect: (provider, _current, set) => {
      const next = brainView(provider);
      set(next);
      return {
        provider,
        label: "Codex CLI",
        brain: next,
      };
    },
  });
  await p.act("[data-brain-codex-action]");
  check("只送一趟 provider connect", calls(p, "brain_cli_connect").length === 1, p.invokes);
  check("沒有借頁尾 settings_write", p.writes.length === 0, p.writes);
  check("成功後直接顯示 Codex 已選用", p.node("[data-brain-codex-status]").textContent.includes("已選用"), p.node("[data-brain-codex-status]").textContent);
  check("Claude 同時不再是已選用", !p.node("[data-brain-claude-status]").textContent.includes("已選用"), p.node("[data-brain-claude-status]").textContent);
  check("成功句沒有被重讀抹掉", p.brainSay() === "Codex CLI 已登入、測通並設為大腦。", p.brainSay());
}

{
  const four = [
    SENTENCE.noCli,
    SENTENCE.noConsent,
    SENTENCE.ready,
  ];
  const unique = new Set(four);
  check("未選、未同意、已可用三種狀態沒有混在一起", unique.size === 3, four);
}

console.log("⑰ 已選的大腦可以單獨測試");
{
  const p = await open({ brain: brainView("grok") });
  check("測試鍵看得到", !p.node("[data-brain-test]").hidden, "test");
  await p.act("[data-brain-test]");
  check("只送一趟固定測試", calls(p, "brain_cli_test").length === 1, p.invokes);
  check("通過後說出 provider", p.brainSay() === "Grok CLI 測試通過。", p.brainSay());
}

console.log("⑰ᵇ 登入失敗不改畫面上的選擇");
{
  const p = await open({
    brain: brainView("claude"),
    onBrainConnect: () => {
      throw new Error("Codex CLI 登入沒有完成；原本的大腦沒有改");
    },
  });
  await p.act("[data-brain-codex-action]");
  check("Claude 仍是已選用", p.node("[data-brain-claude-status]").textContent.includes("已選用"), p.node("[data-brain-claude-status]").textContent);
  check("失敗句是紅的", p.node("[data-brain-say]").classList.contains("bad"), p.brainSay());
  check("說原本選擇沒改", p.brainSay().includes("原本的大腦沒有改"), p.brainSay());
}

console.log("⑰ᶜ CLI 登入、測試、取消都只接受 trusted click");
{
  const p = await open({ brain: brainView("claude") });
  await p.act("[data-brain-codex-action]", { trusted: false });
  await p.act("[data-brain-test]", { trusted: false });
  check(
    "假事件沒有啟動登入或測試",
    calls(p, "brain_cli_connect").length === 0 && calls(p, "brain_cli_test").length === 0,
    p.invokes,
  );

  let finishConnect;
  const held = await open({
    brain: brainView("claude"),
    onBrainConnect: () =>
      new Promise((_resolve, reject) => {
        finishConnect = () => reject(new Error("登入已取消；原本的大腦沒有改"));
      }),
    onBrainCancel: () => {
      setTimeout(finishConnect, 0);
      return true;
    },
  });
  void held.act("[data-brain-codex-action]");
  await tick();
  await held.act("[data-brain-cancel]", { trusted: false });
  check("假取消沒有送 IPC", calls(held, "brain_cli_cancel").length === 0, held.invokes);
  await held.act("[data-brain-cancel]");
  await tick();
  check("真人取消只送一次", calls(held, "brain_cli_cancel").length === 1, held.invokes);
  check(
    "取消完成後仍是 Claude 已選用",
    held.node("[data-brain-claude-status]").textContent.includes("已選用"),
    held.node("[data-brain-claude-status]").textContent,
  );
  check("取消結果沒有被正在取消覆蓋", held.brainSay().includes("登入已取消"), held.brainSay());
}

console.log("⑱ 頁尾儲存不會用較早讀到的值蓋掉 CLI 選擇");
{
  const p = await open({ brain: brainView("codex") });
  await p.save();
  const sent = p.writes[0];
  check("settings payload 沒有 brain_command", !Object.hasOwn(sent, "brain_command"), sent);
  check("settings payload 沒有 brain_args", !Object.hasOwn(sent, "brain_args"), sent);
}

console.log("⑲ 桌面後端真的把 Thinking 接到設定頁和系統匣");
{
  const write = MAIN.match(/fn settings_write\([\s\S]*?struct PrivacyHealth/)?.[0] ?? "";
  const presentation = MAIN.match(/fn record_menu_presentation\([\s\S]*?fn quit_menu_label/)?.[0] ?? "";
  const trayClick = MAIN.match(/"record" => \{[\s\S]*?"settings" =>/)?.[0] ?? "";
  check("settings_write 從完整 Presence 推出 watching", write.includes("heartbeat::watching_word") && write.includes("heartbeat::presence"), write);
  check("tray 標籤用 core 的 exhaustive 純函式", MAIN.includes("heartbeat::tray_record_label(presence)") && MAIN.includes("heartbeat::tray_quit_label(presence)"), "tray labels");
  check("tray 按鍵按 core 的三向 action 分流", MAIN.includes("heartbeat::tray_record_action(presence)"), "tray action");
  check(
    "Thinking 那一向會顯示原因",
    presentation.includes("Presence::Thinking { .. }") &&
      presentation.includes("action: RecordMenuAction::Wait") &&
      presentation.includes("heartbeat::tray_record_label(presence)") &&
      trayClick.includes("RecordMenuAction::Wait => {") &&
      trayClick.includes("heartbeat::occupied_why_of(presence, now)"),
    "thinking feedback",
  );
}

console.log("⑲ᵖ Persona 四格會一起讀寫，關掉角色不會清掉選擇");
{
  const p = await open({
    config: {
      ...BASE,
      persona_enabled: true,
      persona_id: "chatgpt",
      persona_motion: true,
      persona_tap_lines: true,
    },
  });
  check("讀回 ChatGPT 的穩定 ID", p.node("[data-persona-id]").value === "chatgpt", p.node("[data-persona-id]").value);
  check("讀回角色開關", p.node("[data-persona-enabled]").checked === true);
  p.node("[data-persona-id]").value = "grok";
  p.node("[data-persona-motion]").checked = false;
  p.node("[data-persona-tap-lines]").checked = false;
  p.node("[data-persona-enabled]").checked = false;
  for (const fn of p.node("[data-persona-enabled]").handlers.change ?? []) fn();
  check("關掉只灰掉選擇、值仍是 Grok", p.node("[data-persona-id]").disabled && p.node("[data-persona-id]").value === "grok");
  await p.save();
  const sent = p.writes[0];
  check("送出關閉狀態", sent.persona_enabled === false, sent);
  check("關掉仍保留 Grok", sent.persona_id === "grok", sent);
  check("動畫和 tap-lines 各自可關", sent.persona_motion === false && sent.persona_tap_lines === false, sent);

  const body = MAIN.match(/fn settings_write\([\s\S]*?struct PrivacyHealth/)?.[0] ?? "";
  check("Rust 只透過 typed persona setter 寫四格", body.includes("c.set_persona_from_page("), body);
  check("存成功才通知另一扇 WebView", body.includes('"persona-changed"') && body.indexOf("c.save(&path)") < body.indexOf('"persona-changed"'), body);
}

console.log("⑲ᑫ Persona event 沒送到時，存檔成功與畫面未更新要分開說");
{
  const p = await open({
    onWrite(settings, store) {
      store(settings);
      return { watching: "recording", persona_event_emitted: false };
    },
  });
  await p.save();
  check("仍明講設定已存", p.say().includes("角色設定已存進檔案"), p.say());
  check("不冒充桌面已即時換角", p.say().includes("即時更新事件沒能送出"), p.say());
  check("給得出真的恢復路徑", p.say().includes("重新啟動 AI-Sister desktop"), p.say());
  check("部分套用用警告色", p.bad(), p.say());
}

console.log("⑲ʳ 未知 Persona ID 讓整份設定 unreadable，不靜默改成 ChatGPT");
{
  const p = await open({ config: { ...BASE, persona_id: "not-in-this-version" } });
  check("未知 ID 的錯誤有說出來", p.say().includes("不認得的角色 ID"), p.say());
  check("整份 config 表單 fail closed", !p.node("[data-unreadable]").hidden && p.node("[data-save]").disabled);
  check("renderer 沒把未知值改寫成 ChatGPT", p.node("[data-persona-id]").value !== "chatgpt", p.node("[data-persona-id]").value);
  check(
    "未知的非空 ID 原值仍被保留",
    p.node("[data-persona-id]").dataset.unrecognizedPersonaId === "not-in-this-version",
    p.node("[data-persona-id]").dataset.unrecognizedPersonaId,
  );
  check(
    "未知 ID 不會被誤報成 catalog 壞掉",
    p.node("[data-persona-preview-state]").textContent.includes("沒有改成 ChatGPT") &&
      !p.node("[data-persona-preview-state]").textContent.includes("catalog 不完整"),
    p.node("[data-persona-preview-state]").textContent,
  );
}

console.log("⑲ˢ 17 人圖像 radio catalog 只改未存預覽；Save 成功才走既有寫入");
{
  const p = await open();
  const radios = p.personaRadios();
  const images = p.personaImages();
  const ids = radios.map((radio) => radio.value);
  check("恰好 17 個 native radio", radios.length === 17 && radios.every((radio) => radio.type === "radio"), ids);
  check("同名 native radio 保留 Tab／方向鍵／Space 語意", radios.every((radio) => radio.name === "persona-id"), radios.map((radio) => radio.name));
  check(
    "每位只有自己的 bundled WebP 縮圖，不載 Reel layers",
    images.length === 17 &&
      images.every((image) => image.src === `./personas/${image.parentNode?.parentNode?.parentNode?.children?.[0]?.value}.webp`) &&
      !SRC.includes("persona-reels") &&
      !/\bfetch\s*\(/u.test(SRC),
    images.map((image) => image.src),
  );
  check(
    "卡片上看得到 alias 與四姊妹／13 位閨密分組",
    p.node("[data-persona-choice-groups]").textContent.includes("ChatGPT") &&
      p.node("[data-persona-choice-groups]").textContent.includes("MiMo") &&
      p.node("[data-persona-choice-groups]").textContent.includes("四姊妹") &&
      p.node("[data-persona-choice-groups]").textContent.includes("13 位閨密"),
    p.node("[data-persona-choice-groups]").textContent,
  );
  const selectedBefore = radios.filter((radio) => radio.checked);
  check(
    "已存角色同時有 checked 與 aria-checked，不只換顏色",
    selectedBefore.length === 1 &&
      selectedBefore[0].value === "chatgpt" &&
      selectedBefore[0].dataset["aria-checked"] === "true" &&
      p.node("[data-persona-choice-groups]").textContent.includes("已選"),
    selectedBefore.map((radio) => ({ value: radio.value, aria: radio.dataset["aria-checked"] })),
  );
  check(
    "focus ring 畫在整張卡且使用固定高對比前景色",
    read(join(UI, "settings.css")).includes(
      '.persona-choice > input[type="radio"]:focus-visible + .persona-choice-card',
    ) && read(join(UI, "settings.css")).includes("outline: 3px solid var(--fg)"),
  );

  const commandsBefore = p.invokes.map(({ cmd }) => cmd);
  await p.choosePersona("grok");
  const commandsAfterChoice = p.invokes.map(({ cmd }) => cmd);
  check("點卡只改 hidden form value", p.node("[data-persona-id]").value === "grok");
  check("本機預覽立即換成 Grok", p.node("[data-persona-preview-name]").textContent === "Grok");
  check(
    "未存狀態逐字分開預覽與設定檔",
    p.node("[data-persona-preview-state]").textContent.includes("尚未儲存") &&
      p.node("[data-persona-preview-state]").textContent.includes("仍是 ChatGPT"),
    p.node("[data-persona-preview-state]").textContent,
  );
  check(
    "選角本身 0 IPC／GET／TTS，也沒有 rig decode",
    JSON.stringify(commandsAfterChoice) === JSON.stringify(commandsBefore) &&
      p.writes.length === 0 &&
      !SRC.includes(".decode("),
    { before: commandsBefore, after: commandsAfterChoice },
  );
  await p.save();
  check("Save 才把 Grok 放進既有 settings_write", p.writes.length === 1 && p.writes[0].persona_id === "grok", p.writes);
  check(
    "成功讀回後才把 Grok 稱作設定檔現值",
    p.node("[data-persona-preview-state]").textContent === "設定檔目前存的是 Grok。",
    p.node("[data-persona-preview-state]").textContent,
  );
}

console.log("⑲ᵗ Save 失敗保留未存預覽；縮圖壞掉仍有完整名稱可選");
{
  const p = await open({
    onWrite() {
      throw new Error("config.toml 是唯讀的");
    },
  });
  const grokImage = p.personaImages().find((image) => image.src === "./personas/grok.webp");
  for (const fn of grokImage?.handlers.error ?? []) fn();
  check(
    "broken thumbnail 換成文字回退，不生字母 glyph",
    grokImage?.hidden === true &&
      grokImage?.parentNode?.children?.some(
        (child) => child.hidden === false && child.textContent.includes("仍可用名稱選擇"),
      ) &&
      !HTML.includes("data-persona-glyph") &&
      !SRC.includes("data-persona-glyph"),
  );
  check("broken thumbnail 的 Grok radio 仍可選", await p.choosePersona("grok"));
  await p.save();
  check("寫失敗說出真正原因", p.say().includes("config.toml 是唯讀的"), p.say());
  check(
    "失敗後不冒充已套用，仍說尚未儲存且設定檔是 ChatGPT",
    p.node("[data-persona-preview-state]").textContent.includes("尚未儲存") &&
      p.node("[data-persona-preview-state]").textContent.includes("仍是 ChatGPT"),
    p.node("[data-persona-preview-state]").textContent,
  );
}

console.log("⑳ 拔手撞號的純決策真的接回桌面回傳值");
{
  // 這三條只讀 main.rs 原始碼，守的是接線形狀，不是執行覆蓋。Rust 的八格
  // 測試會咬住純決策；這裡能咬住 R7/R8 這種欄位漏接，卻不能證明 Tauri 在
  // Windows 上真的註冊、還原或寫檔成功。
  const collision = MAIN.match(
    /HotkeySetAction::RestoreCollision\s*=>\s*\{[\s\S]*?\n\s*\}/,
  )?.[0] ?? "";
  check(
    "桌面用 sister-hands 的三向決策",
    MAIN.includes("kill_switch::hotkey_set_action(") &&
      MAIN.includes("HotkeySetAction::Persist =>") &&
      MAIN.includes("HotkeySetAction::RestoreCollision =>") &&
      MAIN.includes("HotkeySetAction::RestoreRejected =>"),
    "hotkey_set match",
  );
  check(
    "撞號臂保留被拒絕的那一組",
    collision.includes("restored.rejected = Some(combo);"),
    collision,
  );
  check(
    "撞號臂把撞號事實送到畫面",
    collision.includes("restored.hands_collided = true;"),
    collision,
  );
}

console.log("㉑ 17 人日常語音隨程式完整安裝，不再出現舊素材下載流程");
{
  const p = await open();
  const compactHtml = HTML.replace(/\s+/g, "");
  check(
    "主畫面列出 17 人、544 段與基本／擴充包",
    compactHtml.includes("17位角色、544段固定語音已隨程式安裝") &&
      compactHtml.includes("<dt>基本包</dt><dd>每人8句</dd>") &&
      compactHtml.includes("<dt>擴充包</dt><dd>每人24句</dd>"),
    "settings.html persona voice pack",
  );
  check("舊素材操作不再出現在可見設定介面", !compactHtml.includes("下載完整素材包"));
  check(
    "開場不會下載舊素材包",
    calls(p, "persona_asset_status").length === 1 &&
      calls(p, "persona_asset_install").length === 0,
    p.invokes,
  );
  check("語音出廠畫成關閉", p.node("[data-persona-voice]").checked === false);
}
console.log("㉚ Bundled 聲音是獨立 trusted opt-in，失敗會退回，設定頁永遠不播放");
{
  const installed = {
    phase: "installed",
    disclosure: { ...ASSET_DISCLOSURE },
    asset_file_bytes: 100000000,
    portrait_count: 4,
    voice_count: 8,
  };
  const p = await open({ asset: installed });
  check("bundled 語音預設不會自行打開", p.node("[data-persona-voice]").checked === false);
  check("本機聲音 opt-in 可按", p.node("[data-persona-voice]").disabled === false);
  p.node("[data-persona-voice]").checked = true;
  await p.act("[data-persona-voice]", { trusted: false, event: "change" });
  check("假 change 被退回關閉", p.node("[data-persona-voice]").checked === false);
  check("假 change 沒有寫設定", calls(p, "persona_voice_set").length === 0);
  p.node("[data-persona-voice]").checked = true;
  await p.act("[data-persona-voice]", { event: "change" });
  const voiceSets = calls(p, "persona_voice_set");
  check("真人 opt-in 立刻寫一次，不等頁尾 Save", voiceSets.length === 1 && p.writes.length === 0, { voiceSets, writes: p.writes });
  check("voice IPC 只送 enabled bool", JSON.stringify(voiceSets[0]?.arg) === JSON.stringify({ enabled: true }), voiceSets[0]);
  check("成功後直接回報本機角色聲音已開啟", p.node("[data-persona-voice-state]").textContent === "本機角色聲音已開啟。", p.node("[data-persona-voice-state]").textContent);
  check("設定頁沒有任何 audio play 路徑", !read(SRC).includes(".play("), "settings.js");
  check("opt-in 沒有讀任何語音 bytes", calls(p, "persona_voice_read").length === 0, p.invokes);
}

{
  const p = await open({
    asset: {
      phase: "installed",
      disclosure: { ...ASSET_DISCLOSURE },
      asset_file_bytes: 100000000,
      portrait_count: 4,
      voice_count: 8,
    },
    onVoiceSet: () => {
      throw new Error("設定檔拒絕寫入");
    },
  });
  p.node("[data-persona-voice]").checked = true;
  await p.act("[data-persona-voice]", { event: "change" });
  check("voice 寫失敗會退回原值", p.node("[data-persona-voice]").checked === false);
  check("而且把失敗原因說出來", p.node("[data-persona-voice-state]").textContent.includes("設定檔拒絕寫入"), p.node("[data-persona-voice-state]").textContent);
}

{
  const p = await open({
    asset: {
      phase: "installed",
      disclosure: { ...ASSET_DISCLOSURE },
      asset_file_bytes: 100000000,
      portrait_count: 4,
      voice_count: 8,
    },
    onVoiceSet: () => ({}),
  });
  p.node("[data-persona-voice]").checked = true;
  await p.act("[data-persona-voice]", { event: "change" });
  const voiceState = p.node("[data-persona-voice-state]").textContent;
  check("voice success payload 缺欄位不拿 wanted 冒充結果", !p.node("[data-persona-voice]").checked, voiceState);
  check("malformed voice result 轉成 unknown 並鎖住開關", p.node("[data-persona-voice]").disabled === true, voiceState);
  check("malformed voice result 明講無法確認", voiceState.includes("無法確認是否已改"), voiceState);
  check("malformed voice result 不宣稱已開啟", !voiceState.includes("語音已開啟"), voiceState);
}

{
  let finishVoice = null;
  const installed = {
    phase: "installed",
    disclosure: { ...ASSET_DISCLOSURE },
    asset_file_bytes: 100000000,
    portrait_count: 4,
    voice_count: 8,
  };
  const p = await open({
    asset: installed,
    onVoiceSet: (arg, store) =>
      new Promise((resolveVoice) => {
        finishVoice = () => {
          store(arg.enabled);
          resolveVoice({ voice_enabled: arg.enabled });
        };
      }),
  });
  p.node("[data-persona-voice]").checked = true;
  await p.act("[data-persona-voice]", { event: "change" });
  await p.emit("persona-assets-changed");
  check("voice write 飛行中遇到 asset event 仍保持 disabled", p.node("[data-persona-voice]").disabled === true);
  finishVoice();
  await tick();
  check("write 回來後不會永久卡在 busy", p.node("[data-persona-voice]").checked === true && p.node("[data-persona-voice]").disabled === false, {
    checked: p.node("[data-persona-voice]").checked,
    disabled: p.node("[data-persona-voice]").disabled,
  });
}

console.log("㉚⁰ 本機台灣語音：服務未就緒時不露出可用開關");
{
  const p = await open();
  check("missing 不勾", p.node("[data-local-tts-enabled]").checked === false);
  check(
    "missing 不能當可用開關",
    p.node("[data-local-tts-enabled]").disabled === true,
  );
  check(
    "說明未就緒",
    p.node("[data-local-tts-state]").textContent.includes("沒有偵測到"),
    p.node("[data-local-tts-state]").textContent,
  );

  const q = await open({
    localTts: { ...LOCAL_TTS_OFF, service: "ready", enabled: false, ready: false },
  });
  check(
    "服務就緒才讓人打開",
    q.node("[data-local-tts-enabled]").disabled === false,
  );
  await q.act("[data-local-tts-enabled]", { trusted: false, event: "change" });
  check("假 change 不寫本機台灣語音", calls(q, "local_tts_config_set").length === 0);
  q.node("[data-local-tts-enabled]").checked = true;
  await q.act("[data-local-tts-enabled]", { event: "change" });
  const writes = calls(q, "local_tts_config_set");
  check(
    "trusted change 才保存 enabled",
    writes.length === 1 && writes[0].arg.enabled === true,
    writes,
  );
}

function localTtsCase(service, enabled, voiceEnabled = true) {
  return {
    ...LOCAL_TTS_OFF,
    service,
    enabled,
    voice_enabled: voiceEnabled,
    ready: enabled === true && service === "ready",
    persona: "chatgpt",
  };
}

console.log("㉚⁰ᵇ 本機台灣語音：保存失敗的那句話要留到下一次新狀態");
{
  const readyOff = localTtsCase("ready", false, true);
  const rejected = await open({
    localTts: readyOff,
    onLocalTtsConfigSet: () => {
      throw new Error("save failed");
    },
  });
  rejected.node("[data-local-tts-enabled]").checked = true;
  await rejected.act("[data-local-tts-enabled]", { event: "change" });
  const savedText = rejected.node("[data-local-tts-state]").textContent;
  check("local_tts_config_set reject 最後仍說沒有保存", savedText.includes("沒有保存"), savedText);
  check(
    "保存失敗後勾勾回到還沒寫進去的值",
    rejected.node("[data-local-tts-enabled]").checked === false,
    rejected.node("[data-local-tts-enabled]").checked,
  );

  let reads = 0;
  const bothRejected = await open({
    localTts: readyOff,
    onLocalTtsRead: (state) => {
      reads += 1;
      if (reads > 1) throw new Error("read failed");
      return state;
    },
    onLocalTtsConfigSet: () => {
      throw new Error("save failed");
    },
  });
  bothRejected.node("[data-local-tts-enabled]").checked = true;
  await bothRejected.act("[data-local-tts-enabled]", { event: "change" });
  const still = bothRejected.node("[data-local-tts-state]").textContent;
  check("重讀也 reject 時最後仍是沒有保存", still.includes("沒有保存"), still);
  check("重讀失敗不改口成認不得的狀態", !still.includes("後端沒有回傳可辨識"), still);
}

console.log("㉚⁰ᶜ 本機台灣語音：查過的服務狀態各說各話");
{
  const matrix = [
    [
      "missing",
      false,
      ["沒有偵測到", "啟動本機 BreezyVoice"],
      ["尚未載入完成", "健康檢查", "將用"],
    ],
    [
      "missing",
      true,
      ["沒有回應", "沒有改用系統語音或 Azure"],
      ["沒有偵測到", "尚未載入完成", "健康檢查"],
    ],
    [
      "not_ready",
      false,
      ["有回應", "尚未載入完成", "載入完成後才可選用"],
      ["沒有偵測到", "啟動本機 BreezyVoice", "沒有回應"],
    ],
    [
      "not_ready",
      true,
      ["服務有回應", "尚未載入完成", "沒有改用系統語音或 Azure"],
      ["沒有回應", "沒有偵測到"],
    ],
    [
      "protocol",
      false,
      ["不是本機台灣語音認得的健康檢查", "不能打開"],
      ["沒有偵測到", "尚未載入完成", "已停止", "沒有完成"],
    ],
    [
      "protocol",
      true,
      ["不是認得的健康檢查", "沒有改用系統語音或 Azure"],
      ["沒有回應", "尚未載入完成", "沒有偵測到"],
    ],
    [
      "cancelled",
      false,
      ["健康檢查已停止", "不能打開"],
      ["沒有偵測到", "認得的健康檢查", "沒有完成"],
    ],
    [
      "cancelled",
      true,
      ["健康檢查已停止", "沒有改用系統語音或 Azure"],
      ["沒有回應", "認得的健康檢查", "沒有完成"],
    ],
    [
      "failed",
      false,
      ["健康檢查沒有完成", "不能打開"],
      ["已停止", "沒有偵測到", "認得的健康檢查"],
    ],
    [
      "failed",
      true,
      ["健康檢查沒有完成", "沒有改用系統語音或 Azure"],
      ["已停止", "沒有回應", "認得的健康檢查"],
    ],
    [
      "ready",
      false,
      ["已就緒", "目前關閉", "答案朗讀仍用系統語音"],
      ["將用", "本機聲音", "下一步"],
    ],
    [
      "ready",
      true,
      ["將用目前角色（chatgpt）的本機台灣語音朗讀答案"],
      ["尚未載入完成", "沒有回應", "沒有偵測到"],
    ],
  ];
  const sentences = [];
  for (const [service, enabled, words, absent] of matrix) {
    const page = await open({ localTts: localTtsCase(service, enabled, true) });
    const sentence = page.node("[data-local-tts-state]").textContent;
    sentences.push(sentence);
    const label = `${service}/${enabled ? "開" : "關"}`;
    for (const word of words) {
      check(`${label}說到「${word}」`, sentence.includes(word), sentence);
    }
    for (const word of absent) {
      check(`${label}不說「${word}」`, !sentence.includes(word), sentence);
    }
    const canToggle = enabled === true || service === "ready";
    check(
      `${label}的開關${canToggle ? "可關或可開" : "不能當可用開關"}`,
      page.node("[data-local-tts-enabled]").disabled === !canToggle,
      page.node("[data-local-tts-enabled]").disabled,
    );
    if (service === "ready" && enabled === true) {
      check(`${label}才畫成可朗讀`, page.node("[data-local-tts-state]").classList.contains("ok"), sentence);
    } else {
      check(`${label}不畫成可朗讀`, !page.node("[data-local-tts-state]").classList.contains("ok"), sentence);
    }
  }
  check(
    "服務狀態兩兩不是同一句",
    new Set(sentences).size === matrix.length,
    sentences,
  );
  const settingsServices = [
    ...read(SRC)
      .match(/const LOCAL_TTS_SERVICES = Object\.freeze\(\[([\s\S]*?)\]\)/)[1]
      .matchAll(/"([a-z_]+)"/g),
  ].map((match) => match[1]);
  const appServices = [
    ...read(join(UI, "app.js"))
      .match(/\[((?:"[a-z_]+",?\s*)+)\]\.includes\(raw\.service\)/)[1]
      .matchAll(/"([a-z_]+)"/g),
  ].map((match) => match[1]);
  const localRs = read(resolve(UI, "../../../crates/sister-tts/src/local.rs"));
  const statusImpl = localRs.slice(
    localRs.indexOf("impl LocalServiceStatus {"),
    localRs.indexOf("struct ObservedLocalHealth"),
  );
  const rustServices = [...statusImpl.matchAll(/=> "([a-z_]+)"/g)].map((match) => match[1]);
  check(
    "設定頁、主視窗、Rust 認得同一組 service",
    JSON.stringify([...settingsServices].sort()) === JSON.stringify([...appServices].sort()) &&
      JSON.stringify([...settingsServices].sort()) === JSON.stringify([...rustServices].sort()),
    { settingsServices, appServices, rustServices },
  );
}

console.log("㉚⁰ᵈ 本機聲音關著時，不承諾答案鍵會朗讀");
{
  const opened = await open({ localTts: localTtsCase("ready", true, false) });
  const openedText = opened.node("[data-local-tts-state]").textContent;
  check("已開且就緒但本機聲音關著，指出下一步", openedText.includes("下一步") && openedText.includes("本機聲音"), openedText);
  check("這時不承諾將用本機台灣語音朗讀答案", !openedText.includes("將用") && !openedText.includes("朗讀答案"), openedText);
  check("這時不畫成可朗讀", !opened.node("[data-local-tts-state]").classList.contains("ok"), openedText);

  const closed = await open({ localTts: localTtsCase("ready", false, false) });
  const closedText = closed.node("[data-local-tts-state]").textContent;
  check("服務就緒但兩格都關時，指出本機聲音", closedText.includes("本機聲音") && closedText.includes("下一步"), closedText);
  check("這時不說答案朗讀仍用系統語音", !closedText.includes("答案朗讀仍用系統語音"), closedText);
  check("這時也不承諾將用朗讀答案", !closedText.includes("將用") && !closedText.includes("朗讀答案"), closedText);

  const promised = await open({ localTts: localTtsCase("ready", true, true) });
  const promisedText = promised.node("[data-local-tts-state]").textContent;
  check(
    "本機聲音開著且服務就緒，才出現承諾句",
    promisedText.includes("將用目前角色（chatgpt）的本機台灣語音朗讀答案"),
    promisedText,
  );
}

console.log("㉚⁰ᵉ 本機台灣語音寫入飛行中的重讀要排隊，完成後補讀");
{
  let reads = 0;
  let finishWrite = null;
  const saved = localTtsCase("ready", false, true);
  const nativeTruth = { ...saved, enabled: true, ready: true, persona: "claude" };
  const page = await open({
    localTts: saved,
    onLocalTtsRead: (state) => {
      reads += 1;
      return state;
    },
    onLocalTtsConfigSet: () =>
      new Promise((resolveWrite) => {
        finishWrite = () => resolveWrite({ ...saved, enabled: true, ready: true });
      }),
  });
  page.node("[data-local-tts-enabled]").checked = true;
  await page.act("[data-local-tts-enabled]", { event: "change" });
  check("前提：本機台灣語音寫入還在飛", typeof finishWrite === "function");
  const readsBeforeEvent = reads;
  page.setLocalTts(nativeTruth);
  await page.emit("local-tts-changed");
  check("busy 時重讀先排隊，不平行讀", reads === readsBeforeEvent, reads);
  finishWrite();
  await tick();
  await tick();
  check("寫入完成後補讀恰好一次", reads === readsBeforeEvent + 1, reads);
  const after = page.node("[data-local-tts-state]").textContent;
  check(
    "補讀的角色蓋過寫入回條",
    after.includes("將用目前角色（claude）的本機台灣語音朗讀答案"),
    after,
  );
}

console.log("㉚⁰ᵍ 本機台灣語音重畫丟例外後，忙旗標要放下，下一輪才讀得到");
{
  const readyOff = localTtsCase("ready", false, true);
  const page = await open({ localTts: readyOff });
  const stateEl = page.node("[data-local-tts-state]");
  const original = Object.getOwnPropertyDescriptor(stateEl, "textContent");
  let thrown = false;
  Object.defineProperty(stateEl, "textContent", {
    configurable: true,
    get() {
      return original.get.call(stateEl);
    },
    set(value) {
      if (!thrown) {
        thrown = true;
        throw new Error("paint once");
      }
      return original.set.call(stateEl, value);
    },
  });
  page.node("[data-local-tts-enabled]").checked = true;
  await page.act("[data-local-tts-enabled]", { event: "change" });
  const afterSave = stateEl.textContent;
  check("前提：重畫那一步真的丟了一次", thrown === true);
  check(
    "保存成功後重畫失敗，不改口成沒有保存",
    !afterSave.includes("沒有保存"),
    afterSave,
  );
  const before = calls(page, "local_tts_read").length;
  await page.emit("local-tts-changed");
  const after = calls(page, "local_tts_read").length;
  check(
    "重畫丟過一次後，下一次 local-tts-changed 真的讀了",
    after === before + 1,
    { before, after },
  );
}

console.log("㉚⁰ᶠ 讀不出設定時 voice_enabled 只能是 null");
{
  const unreadable = {
    generation: 4,
    config_readable: false,
    enabled: null,
    voice_enabled: null,
    endpoint: null,
    health_endpoint: null,
    service: "missing",
    persona: null,
    ready: false,
    config_error: "config.toml 讀不出來。",
  };
  const page = await open({ localTts: unreadable });
  check(
    "讀不出設定不畫成可用",
    page.node("[data-local-tts-state]").textContent.includes("設定讀不出來"),
    page.node("[data-local-tts-state]").textContent,
  );
  const lied = await open({ localTts: { ...unreadable, voice_enabled: false } });
  check(
    "config 讀不出來卻帶 voice_enabled 不算可辨識",
    lied.node("[data-local-tts-state]").textContent.includes("後端沒有回傳可辨識"),
    lied.node("[data-local-tts-state]").textContent,
  );
}

console.log("㉚ᵃ Azure 五態分開畫；只有四道 native gate 齊全才說 ready");
{
  const cases = [
    ["關閉", AZURE_OFF, "目前關閉"],
    [
      "缺區域",
      { ...AZURE_READY, region: null, endpoint: null, ready: false },
      "還沒選金鑰所屬區域",
    ],
    ["缺金鑰", { ...AZURE_READY, credential: "missing", ready: false }, "沒有金鑰"],
    [
      "缺第四張",
      { ...AZURE_READY, consented: false, consent_at: null, ready: false },
      "還沒勾第四張",
    ],
    ["全齊", AZURE_READY, "四個條件都齊了"],
  ];
  const sentences = [];
  for (const [name, azure, words] of cases) {
    const p = await open({ azure });
    const sentence = p.node("[data-azure-state]").textContent;
    sentences.push(sentence);
    check(`${name}照實說`, sentence.includes(words), sentence);
    check(
      `${name}的 ready 沒有自創第五種判斷`,
      p.node("[data-azure-state]").classList.contains("ok") === azure.ready,
      sentence,
    );
  }
  check("五態不是同一句", new Set(sentences).size === cases.length, sentences);

  const compact = HTML.replace(/\s+/g, "");
  check(
    "主控制保持簡短，完整 Azure 邊界集中在頁尾條款",
    compact.includes("<detailsclass=\"product-details\">") &&
      compact.indexOf("<detailsclass=\"product-details\">") >
        compact.indexOf("<sectionclass=\"azure-tts\"") &&
      compact.includes("Azure每份新答案只送出該題答案正文原文一次") &&
      compact.includes("手動重播會再送一次") &&
      compact.includes("不送截圖、來源連結、memoryid、整份資料庫、其他文字或角色選擇") &&
      compact.includes("WindowsCredentialManager") &&
      compact.includes("不寫入config.toml、log、資料庫或export") &&
      compact.includes("不會回傳顯示") &&
      compact.includes("停止朗讀會丟掉晚到的音訊") &&
      compact.includes("已開始的請求最長可執行45秒"),
    "settings.html Azure disclosure",
  );
}

console.log("㉚ᵇ Azure 設定只接受 trusted controls，IPC 只帶 typed 三格");
{
  const p = await open();
  const setWanted = () => {
    p.node("[data-azure-enabled]").checked = true;
    p.node("[data-azure-region]").value = "southeastasia";
    p.node("[data-azure-voice]").value = "zh-TW-YunJheNeural";
  };
  setWanted();
  await p.act("[data-azure-enabled]", { trusted: false, event: "change" });
  check("script 假 change 沒有寫 Azure 設定", calls(p, "azure_tts_config_set").length === 0);
  setWanted();
  await p.act("[data-azure-enabled]", { event: "change" });
  const writes = calls(p, "azure_tts_config_set");
  check(
    "真人 change 恰好送 enabled/region/voice",
    writes.length === 1 &&
      JSON.stringify(writes[0].arg) ===
        JSON.stringify({
          enabled: true,
          region: "southeastasia",
          voice: "zh-TW-YunJheNeural",
        }),
    writes,
  );
  check("Azure 即時設定不混進頁尾 settings payload", p.writes.length === 0, p.writes);
  check(
    "endpoint 只能由 typed region 投影",
    p.node("[data-azure-endpoint]").textContent ===
      "https://southeastasia.tts.speech.microsoft.com/cognitiveservices/v1",
    p.node("[data-azure-endpoint]").textContent,
  );
}

console.log("㉚ᶜ Azure key 只往 credential command 送一次，立刻離開 DOM且不混進 config");
{
  const secret = "0123456789abcdef0123456789abcdef";
  let received = null;
  const p = await open({
    azure: { ...AZURE_READY, credential: "missing", ready: false },
    onAzureKeySet: (arg, state, store) => {
      received = arg;
      const next = { ...state, credential: "present", ready: true };
      store(next);
      return next;
    },
  });
  p.node("[data-azure-key]").value = secret;
  await p.act("[data-azure-key]", { event: "input" });
  await p.act("[data-azure-key-save]", { trusted: false });
  check("假 click 沒有送 key", calls(p, "azure_tts_key_set").length === 0);
  check("假 click 也不冒充保存成功", p.node("[data-azure-key]").value === secret);
  await p.act("[data-azure-key-save]");
  check(
    "真人 click 只送 key_set 一次",
    calls(p, "azure_tts_key_set").length === 1 && received?.key === secret,
  );
  check("送出當下就把 password input 清空", p.node("[data-azure-key]").value === "");
  check(
    "一般 config payload 從來沒有 key",
    p.writes.every((write) => !Object.keys(write).some((key) => /azure|credential|key/i.test(key))),
    p.writes,
  );
  check(
    "後端回條只投影 credential state，不回 key",
    !Object.hasOwn(AZURE_READY, "key") && !p.node("[data-azure-state]").textContent.includes(secret),
    p.node("[data-azure-state]").textContent,
  );
}

console.log("㉚ᵈ key 刪除與 consent 入口也只接受 trusted click；changed event 只重讀");
{
  const p = await open({ azure: AZURE_READY });
  await p.act("[data-azure-key-delete]", { trusted: false });
  await p.act("[data-azure-consent]", { trusted: false });
  check("兩個假 click 都沒有 IPC", calls(p, "azure_tts_key_delete").length === 0 && calls(p, "open_onboarding").length === 0);
  await p.act("[data-azure-key-delete]");
  check(
    "刪除是零參數且只做一次",
    calls(p, "azure_tts_key_delete").length === 1 && calls(p, "azure_tts_key_delete")[0].arg === undefined,
    calls(p, "azure_tts_key_delete"),
  );

  const q = await open();
  await q.act("[data-azure-consent]");
  check("真人才能開四張同意書", calls(q, "open_onboarding").length === 1);
  const reads = calls(q, "azure_tts_read").length;
  const mutationsBeforeEvent = [
    "azure_tts_config_set",
    "azure_tts_key_set",
    "azure_tts_key_delete",
    "open_onboarding",
  ].map((command) => calls(q, command).length);
  q.setAzure(AZURE_READY);
  await q.emit("azure-tts-changed");
  check("狀態事件只多讀一次 native truth", calls(q, "azure_tts_read").length === reads + 1, q.invokes);
  check(
    "事件不寫設定、不存刪 key、不開同意書",
    [
      "azure_tts_config_set",
      "azure_tts_key_set",
      "azure_tts_key_delete",
      "open_onboarding",
    ].every((command, index) => calls(q, command).length === mutationsBeforeEvent[index]),
    q.invokes,
  );
}

console.log("㉚ᵉ config 讀壞與 malformed status 都 fail closed，但 credential 刪除出口仍在");
{
  const unreadable = {
    generation: 8,
    config_readable: false,
    enabled: null,
    region: null,
    voice: null,
    endpoint: null,
    credential: "present",
    consented: false,
    consent_at: null,
    ready: false,
    config_error: "config.toml parse error",
  };
  const p = await open({ azure: unreadable });
  check("壞 config 不冒充 Azure 關閉", p.node("[data-azure-state]").textContent.includes("設定讀不出來"));
  check("開關與 typed region 都鎖住", p.node("[data-azure-enabled]").disabled && p.node("[data-azure-region]").disabled);
  check("credential present 仍保留刪除出口", p.node("[data-azure-key-delete]").disabled === false);

  const malformed = await open({ azure: { ...AZURE_READY, ready: false } });
  check(
    "後端把 ready 算錯時不畫成可連線",
    malformed.node("[data-azure-state]").textContent.includes("沒有回傳可辨識") &&
      !malformed.node("[data-azure-state]").classList.contains("ok"),
    malformed.node("[data-azure-state]").textContent,
  );
}

console.log("㉚ᶠ Azure write 飛行中收到 changed event，完成後必須補讀 native truth");
{
  let finishWrite = null;
  let reads = 0;
  const staleWriteReply = {
    ...AZURE_READY,
    region: "southeastasia",
    endpoint: "https://southeastasia.tts.speech.microsoft.com/cognitiveservices/v1",
  };
  const p = await open({
    onAzureRead: (state) => {
      reads += 1;
      return state;
    },
    onAzureConfigSet: () =>
      new Promise((resolveWrite) => {
        finishWrite = () => resolveWrite(staleWriteReply);
      }),
  });
  p.node("[data-azure-enabled]").checked = true;
  p.node("[data-azure-region]").value = "southeastasia";
  p.node("[data-azure-voice]").value = "zh-TW-HsiaoChenNeural";
  await p.act("[data-azure-enabled]", { event: "change" });
  check("前提：write 還在飛且控制已鎖", typeof finishWrite === "function" && p.node("[data-azure-enabled]").disabled);
  const readsBeforeEvent = reads;
  p.setAzure(AZURE_READY);
  await p.emit("azure-tts-changed");
  check("busy 時 event 先排隊，不平行讀舊狀態", reads === readsBeforeEvent, reads);
  finishWrite();
  await tick();
  await tick();
  check("write 完成後補讀恰好一次", reads === readsBeforeEvent + 1, reads);
  check(
    "補讀的 eastasia native truth 蓋過 stale southeastasia write reply",
    p.node("[data-azure-endpoint]").textContent === AZURE_READY.endpoint,
    p.node("[data-azure-endpoint]").textContent,
  );
  check("補讀完成後控制恢復", p.node("[data-azure-enabled]").disabled === false);
}

function failAzurePaintThatClaimsNoRequest(stateEl) {
  const original = Object.getOwnPropertyDescriptor(stateEl, "textContent");
  Object.defineProperty(stateEl, "textContent", {
    configurable: true,
    get() {
      return original.get.call(stateEl);
    },
    set(value) {
      original.set.call(stateEl, value);
      if (String(value).includes("沒有送出 request")) {
        throw new Error("paintAzureTts threw inside refresh catch");
      }
    },
  });
}

console.log("㉚ᵍ 金鑰保存或刪除失敗、而且重讀自己也丟例外時，不能改口成沒有送出 request");
{
  // Node 15+ 會把沒人接的 rejection 直接殺掉行程。重讀例外穿出 save/delete
  // 時，要先讓這支測試讀到畫面上的那一句，再由斷言判紅。
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    const secret = "0123456789abcdef0123456789abcdef";
    let failRead = false;
    const saved = await open({
      azure: { ...AZURE_READY, credential: "missing", ready: false },
      onAzureRead: (state) => {
        if (failRead) throw new Error("status read blew up");
        return state;
      },
      onAzureKeySet: () => {
        throw new Error("credential store refused");
      },
    });
    failRead = true;
    failAzurePaintThatClaimsNoRequest(saved.node("[data-azure-state]"));
    saved.node("[data-azure-key]").value = secret;
    await saved.act("[data-azure-key]", { event: "input" });
    const clickedSave = await saved.act("[data-azure-key-save]");
    await tick();
    const saveText = saved.node("[data-azure-state]").textContent;
    check(
      "前提：金鑰保存鍵按得到且 request 有送出",
      clickedSave === true && calls(saved, "azure_tts_key_set").length === 1,
    );
    check(
      "存檔失敗而且重讀自己也丟例外時，最後一句是金鑰沒有保存",
      saveText === "金鑰沒有保存：credential store refused" &&
        !saveText.includes("沒有送出 request"),
      saveText,
    );

    failRead = false;
    const deleted = await open({
      azure: AZURE_READY,
      onAzureRead: (state) => {
        if (failRead) throw new Error("status read blew up");
        return state;
      },
      onAzureKeyDelete: () => {
        throw new Error("credential delete refused");
      },
    });
    failRead = true;
    failAzurePaintThatClaimsNoRequest(deleted.node("[data-azure-state]"));
    const clickedDelete = await deleted.act("[data-azure-key-delete]");
    await tick();
    const deleteText = deleted.node("[data-azure-state]").textContent;
    check(
      "前提：金鑰刪除鍵按得到且 request 有送出",
      clickedDelete === true && calls(deleted, "azure_tts_key_delete").length === 1,
    );
    check(
      "刪除失敗而且重讀自己也丟例外時，最後一句是金鑰沒有刪掉",
      deleteText === "金鑰沒有刪掉：credential delete refused" &&
        !deleteText.includes("沒有送出 request"),
      deleteText,
    );
    check(
      "這兩次重讀例外都留在函式裡",
      rejections.length === 0,
      rejections.map((reason) => String(reason?.message ?? reason)),
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }
}

function failNextAzurePaintIncluding(stateEl, needle) {
  const original = Object.getOwnPropertyDescriptor(stateEl, "textContent");
  let thrown = false;
  Object.defineProperty(stateEl, "textContent", {
    configurable: true,
    get() {
      return original.get.call(stateEl);
    },
    set(value) {
      // 命中的那一次先丟、不寫進畫面。成功句要是已經寫上去，
      // 只斷言「不含失敗句」會在失敗句沒蓋掉時假綠。
      if (!thrown && String(value).includes(needle)) {
        thrown = true;
        throw new Error("paintAzureTts threw on the success repaint");
      }
      original.set.call(stateEl, value);
    },
  });
  return () => thrown;
}

console.log("㉚ʰ Azure 失敗路徑的重讀、與成功路徑的重畫，丟例外都不能改口");
{
  // Node 15+ 會把沒人接的 rejection 直接殺掉行程。重畫例外穿出這三支時，
  // 要先讓這支測試讀到畫面，再由斷言判紅。
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    let failRead = false;
    const configFailed = await open({
      azure: AZURE_OFF,
      onAzureRead: (state) => {
        if (failRead) throw new Error("status read blew up");
        return state;
      },
      onAzureConfigSet: () => {
        throw new Error("config write refused");
      },
    });
    failRead = true;
    failAzurePaintThatClaimsNoRequest(configFailed.node("[data-azure-state]"));
    const rejectionsBeforeConfigFail = rejections.length;
    configFailed.node("[data-azure-enabled]").checked = true;
    await configFailed.act("[data-azure-enabled]", { event: "change" });
    await tick();
    const configFailText = configFailed.node("[data-azure-state]").textContent;
    check(
      "開關保存失敗而且重讀自己也丟例外時，最後一句是沒有完整存好",
      calls(configFailed, "azure_tts_config_set").length === 1 &&
        configFailText === "Azure 開關、區域與聲音沒有完整存好：config write refused",
      configFailText,
    );
    check(
      "這次開關失敗路徑的重畫例外留在函式裡",
      rejections.length === rejectionsBeforeConfigFail,
      rejections.slice(rejectionsBeforeConfigFail).map((reason) => String(reason?.message ?? reason)),
    );

    const secret = "0123456789abcdef0123456789abcdef";
    const savedSentence =
      "四個條件都齊了。每份最新答案完成後會自動送出正文並播放一次；答案下方的 Azure 按鈕可停止或手動重播，重播會再送一次。關閉操作成功回覆後就不再開始新的自動朗讀。";
    const saved = await open({
      azure: { ...AZURE_READY, credential: "missing", ready: false },
    });
    saved.node("[data-azure-key]").value = secret;
    await saved.act("[data-azure-key]", { event: "input" });
    const seenSaveThrow = failNextAzurePaintIncluding(
      saved.node("[data-azure-state]"),
      "四個條件都齊了",
    );
    const rejectionsBeforeSave = rejections.length;
    await saved.act("[data-azure-key-save]");
    await tick();
    const saveText = saved.node("[data-azure-state]").textContent;
    check(
      "存檔成功而成功路徑重畫丟例外時，不說金鑰沒有保存，並畫出已保存狀態",
      seenSaveThrow() === true &&
        calls(saved, "azure_tts_key_set").length === 1 &&
        saveText === savedSentence &&
        !saveText.includes("金鑰沒有保存"),
      saveText,
    );
    check(
      "這次存檔成功路徑的重畫例外留在函式裡",
      rejections.length === rejectionsBeforeSave,
      rejections.slice(rejectionsBeforeSave).map((reason) => String(reason?.message ?? reason)),
    );

    const deletedSentence =
      "Azure 開關與區域已有設定，但 Windows Credential Manager 裡沒有金鑰；不會送出 request。";
    const deleted = await open({ azure: AZURE_READY });
    const seenDeleteThrow = failNextAzurePaintIncluding(
      deleted.node("[data-azure-state]"),
      "Windows Credential Manager 裡沒有金鑰",
    );
    const rejectionsBeforeDelete = rejections.length;
    await deleted.act("[data-azure-key-delete]");
    await tick();
    const deleteText = deleted.node("[data-azure-state]").textContent;
    check(
      "刪除成功而成功路徑重畫丟例外時，不說金鑰沒有刪掉，並畫出已刪除狀態",
      seenDeleteThrow() === true &&
        calls(deleted, "azure_tts_key_delete").length === 1 &&
        deleteText === deletedSentence &&
        !deleteText.includes("金鑰沒有刪掉"),
      deleteText,
    );
    check(
      "這次刪除成功路徑的重畫例外留在函式裡",
      rejections.length === rejectionsBeforeDelete,
      rejections.slice(rejectionsBeforeDelete).map((reason) => String(reason?.message ?? reason)),
    );

    const openedSentence =
      "Azure 開關已打開，但還沒選金鑰所屬區域；沒有可用 endpoint，也不會送出 request。";
    const opened = await open({ azure: AZURE_OFF });
    const seenOpenThrow = failNextAzurePaintIncluding(
      opened.node("[data-azure-state]"),
      "還沒選金鑰所屬區域",
    );
    const rejectionsBeforeOpen = rejections.length;
    opened.node("[data-azure-enabled]").checked = true;
    await opened.act("[data-azure-enabled]", { event: "change" });
    await tick();
    const openText = opened.node("[data-azure-state]").textContent;
    check(
      "開關保存成功而成功路徑重畫丟例外時，不說沒有完整存好，並畫出已打開但未選區域",
      seenOpenThrow() === true &&
        calls(opened, "azure_tts_config_set").length === 1 &&
        openText === openedSentence &&
        !openText.includes("Azure 開關、區域與聲音沒有完整存好"),
      openText,
    );
    check(
      "這次開關保存成功路徑的重畫例外留在函式裡",
      rejections.length === rejectionsBeforeOpen,
      rejections.slice(rejectionsBeforeOpen).map((reason) => String(reason?.message ?? reason)),
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }
}

const ASSET_INSTALLED = {
  phase: "installed",
  disclosure: { ...ASSET_DISCLOSURE },
  asset_file_bytes: 100000000,
  portrait_count: 4,
  voice_count: 8,
};
const ASSET_REPAIR = { ...ASSET_AVAILABLE, phase: "repair-needed" };
const ASSET_INSTALLING = { ...ASSET_AVAILABLE, phase: "installing" };
const INSTALLED_SUMMARY =
  "已安裝並驗證 persona-pack-v1：4 張舊版立繪（桌面不採用）、8 句固定台詞錄音，素材檔合計 100,000,000 bytes（95.37 MiB）。";
const AVAILABLE_SUMMARY =
  "17 張內建角色圖已可使用；四姊妹的額外固定錄音尚未下載。記錄、搜尋、證據、刪除與匯出不受影響。";

// 每次命中都丟。這三支 painter 是同步的（`await` 0 處、`invoke` 0 處），第一次丟
// 到第二次之間沒有交錯點，真實的重畫失敗會每次丟在同一行。
//
// **這裡沒有「只丟一次」的選項，是故意的。** a147 R7 有過一版：產品在 catch 裡
// 再呼叫一次同一支 painter「補畫」，而那段補畫只在「剛好只丟一次」的夾具底下會
// 成功。拿掉補畫、和把夾具改成每次都丟，紅的是**同樣那五條**正面斷言——
// 兩者是同一根槓桿。留一個沒有人用的 once 開關，就是把那扇門留著。
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

function rejectionMessages(rejections, from) {
  return rejections.slice(from).map((reason) => String(reason?.message ?? reason));
}

function failAfterFirstStatus(first) {
  let reads = 0;
  return () => {
    reads += 1;
    if (reads === 1) {
      return {
        ...first,
        disclosure: first.disclosure ? { ...first.disclosure } : null,
      };
    }
    throw new Error("status read blew up");
  };
}

console.log("㉚ⁱ 素材操作失敗、重讀再丟時，失敗句仍要畫完");
{
  // Node 15+ 會把沒人接的 rejection 直接殺掉行程。重讀例外穿出這三支時，
  // 要先讓這支測試讀到畫面，再由斷言判紅。
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    async function failAssetRefresh(page) {
      const seen = throwOnText(
        page.node("[data-persona-asset-error]"),
        "問不到本機素材狀態",
      );
      return seen;
    }

    const available = await open({
      asset: ASSET_AVAILABLE,
      onAssetInstall: () => {
        throw new Error("cdn refused");
      },
      onAssetStatus: failAfterFirstStatus(ASSET_AVAILABLE),
    });
    const seenAvailable = await failAssetRefresh(available);
    const beforeAvailable = rejections.length;
    const clickedDownload = await available.act("[data-persona-download]");
    await tick();
    const availableError = available.node("[data-persona-asset-error]").textContent;
    check(
      "前提：可下載時下載鍵按得到",
      clickedDownload === true && calls(available, "persona_asset_install").length === 1,
      { clickedDownload, invokes: available.invokes },
    );
    check(
      "下載失敗而且重讀自己也丟例外時，最後一句是下載沒有完成",
      seenAvailable.seen() === true &&
        availableError === "下載沒有完成：cdn refused。17 張內建角色圖仍可使用。",
      availableError,
    );
    check(
      "這次下載失敗路徑的重讀例外留在函式裡",
      rejections.length === beforeAvailable,
      rejectionMessages(rejections, beforeAvailable),
    );

    const repair = await open({
      asset: ASSET_REPAIR,
      onAssetInstall: () => {
        throw new Error("cdn refused");
      },
      onAssetStatus: failAfterFirstStatus(ASSET_REPAIR),
    });
    const seenRepair = await failAssetRefresh(repair);
    const beforeRepair = rejections.length;
    const clickedRepair = await repair.act("[data-persona-repair]");
    await tick();
    const repairError = repair.node("[data-persona-asset-error]").textContent;
    check(
      "前提：待修復時修復鍵按得到",
      clickedRepair === true && calls(repair, "persona_asset_install").length === 1,
      { clickedRepair, invokes: repair.invokes },
    );
    check(
      "修復失敗而且重讀自己也丟例外時，最後一句是修復沒有完成",
      seenRepair.seen() === true &&
        repairError === "修復沒有完成：cdn refused。17 張內建角色圖仍可使用。",
      repairError,
    );
    check(
      "這次修復失敗路徑的重讀例外留在函式裡",
      rejections.length === beforeRepair,
      rejectionMessages(rejections, beforeRepair),
    );

    const cancelling = await open({
      asset: ASSET_INSTALLING,
      onAssetCancel: () => {
        throw new Error("cancel refused");
      },
      onAssetStatus: failAfterFirstStatus(ASSET_INSTALLING),
    });
    const seenCancel = await failAssetRefresh(cancelling);
    const beforeCancel = rejections.length;
    const clickedCancel = await cancelling.act("[data-persona-cancel]");
    await tick();
    const cancelError = cancelling.node("[data-persona-asset-error]").textContent;
    check(
      "前提：下載中取消鍵按得到",
      clickedCancel === true && calls(cancelling, "persona_asset_cancel").length === 1,
      { clickedCancel, invokes: cancelling.invokes },
    );
    check(
      "取消失敗而且重讀自己也丟例外時，最後一句是取消失敗",
      seenCancel.seen() === true &&
        cancelError === "取消失敗：cancel refused。下載狀態沒有被假裝成已停止。",
      cancelError,
    );
    check(
      "這次取消失敗路徑的重讀例外留在函式裡",
      rejections.length === beforeCancel,
      rejectionMessages(rejections, beforeCancel),
    );

    const removing = await open({
      asset: ASSET_INSTALLED,
      onAssetRemove: () => {
        throw new Error("remove refused");
      },
      onAssetStatus: failAfterFirstStatus(ASSET_INSTALLED),
    });
    const seenRemove = await failAssetRefresh(removing);
    const beforeRemove = rejections.length;
    const clickedRemove = await removing.act("[data-persona-remove]");
    await tick();
    const removeError = removing.node("[data-persona-asset-error]").textContent;
    check(
      "前提：已安裝時刪除鍵按得到",
      clickedRemove === true && calls(removing, "persona_asset_remove").length === 1,
      { clickedRemove, invokes: removing.invokes },
    );
    check(
      "刪除失敗而且重讀自己也丟例外時，最後一句是刪除結果無法確認",
      seenRemove.seen() === true &&
        removeError === "刪除結果無法確認：remove refused。重新開啟設定再查一次本機狀態。",
      removeError,
    );
    check(
      "這次刪除失敗路徑的重讀例外留在函式裡",
      rejections.length === beforeRemove,
      rejectionMessages(rejections, beforeRemove),
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }
}

console.log("㉚ʲ 素材操作已經成功時，重畫丟例外不改口成沒有完成");
{
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    function escapeAfterStatusPaint(page, summaryNeedle) {
      const seenSummary = throwOnText(page.node("[data-persona-asset-summary]"), summaryNeedle, {
        afterWrite: true,
      });
      const seenRefresh = throwOnText(
        page.node("[data-persona-asset-error]"),
        "問不到本機素材狀態",
      );
      return () => seenSummary.seen() && seenRefresh.seen();
    }

    const installed = await open({ asset: ASSET_AVAILABLE });
    const seenInstall = escapeAfterStatusPaint(installed, "已安裝並驗證");
    const beforeInstall = rejections.length;
    const clickedInstall = await installed.act("[data-persona-download]");
    await tick();
    const installSummary = installed.node("[data-persona-asset-summary]").textContent;
    const installError = installed.node("[data-persona-asset-error]").textContent;
    check("前提：下載鍵按得到且安裝 request 有送出", clickedInstall === true && seenInstall() === true);
    check(
      "下載成功而重畫丟例外時，畫面上是已安裝那一句",
      installSummary === INSTALLED_SUMMARY,
      installSummary,
    );
    check(
      "下載成功而重畫丟例外時，不說下載沒有完成",
      !installError.includes("下載沒有完成"),
      installError,
    );
    check(
      "這次下載成功路徑的重畫例外留在函式裡",
      rejections.length === beforeInstall,
      rejectionMessages(rejections, beforeInstall),
    );

    // 成功路徑的第一次重讀畫出 available，接著在 refresh 的 catch 裡再丟。
    // 沒有乙的時候外層會再讀一次；第三次故意回 unavailable，三臂才落到「刪除結果無法確認」。
    // 回 available 的話會落到另一臂，這兩句否定都打不中。
    let removeStatusReads = 0;
    const removedToAvailable = await open({
      asset: ASSET_INSTALLED,
      onAssetRemove: (_state, store) => {
        store({ ...ASSET_AVAILABLE, disclosure: { ...ASSET_DISCLOSURE } });
      },
      onAssetStatus: () => {
        removeStatusReads += 1;
        if (removeStatusReads === 1) {
          return { ...ASSET_INSTALLED, disclosure: { ...ASSET_DISCLOSURE } };
        }
        if (removeStatusReads === 2) {
          return { ...ASSET_AVAILABLE, disclosure: { ...ASSET_DISCLOSURE } };
        }
        return {
          phase: "unavailable",
          disclosure: null,
          asset_file_bytes: null,
          portrait_count: null,
          voice_count: null,
        };
      },
    });
    const seenRemoveUnknown = escapeAfterStatusPaint(removedToAvailable, AVAILABLE_SUMMARY);
    const beforeRemoveUnknown = rejections.length;
    const clickedRemoveUnknown = await removedToAvailable.act("[data-persona-remove]");
    await tick();
    const removeUnknownSummary = removedToAvailable.node("[data-persona-asset-summary]").textContent;
    const removeUnknownError = removedToAvailable.node("[data-persona-asset-error]").textContent;
    check(
      "前提：刪除後重讀會畫出可用狀態，而且重畫例外有穿出",
      clickedRemoveUnknown === true && seenRemoveUnknown() === true,
    );
    check(
      "刪除成功而重畫丟例外時，畫面上是後端回報的可用狀態",
      removeUnknownSummary === AVAILABLE_SUMMARY,
      removeUnknownSummary,
    );
    check(
      "刪除成功而重畫丟例外時，不說刪除結果無法確認",
      !removeUnknownError.includes("刪除結果無法確認"),
      removeUnknownError,
    );
    check(
      "刪除成功而重畫丟例外時，不說刪除沒有完成",
      !removeUnknownError.includes("刪除沒有完成"),
      removeUnknownError,
    );
    check(
      "這次刪除成功、狀態被重讀蓋掉的重畫例外留在函式裡",
      rejections.length === beforeRemoveUnknown,
      rejectionMessages(rejections, beforeRemoveUnknown),
    );

    // 上面那條重讀的 catch 會先把狀態改成 null，失敗句只會落到「無法確認」。
    // 這一條讓狀態停在 installed，重畫例外從語音那一側穿出，失敗句才會是「刪除沒有完成」。
    let armVoiceFailure = false;
    let voiceReadFails = false;
    const removedStaysInstalled = await open({
      asset: ASSET_INSTALLED,
      onAssetRemove: (_state, store) => {
        store({ ...ASSET_INSTALLED, disclosure: { ...ASSET_DISCLOSURE } });
      },
      onAssetStatus: (state) => {
        if (armVoiceFailure) voiceReadFails = true;
        return {
          ...state,
          disclosure: state.disclosure ? { ...state.disclosure } : null,
        };
      },
      onPersonaRead: (enabled) => {
        if (voiceReadFails) throw new Error("persona read blew up");
        return { id: "chatgpt", voice_enabled: enabled };
      },
    });
    const seenVoice = throwOnText(
      removedStaysInstalled.node("[data-persona-voice-state]"),
      "正在讀本機聲音設定",
    );
    armVoiceFailure = true;
    const beforeRemoveInstalled = rejections.length;
    const clickedRemoveInstalled = await removedStaysInstalled.act("[data-persona-remove]");
    await tick();
    const removeInstalledSummary = removedStaysInstalled.node("[data-persona-asset-summary]").textContent;
    const removeInstalledError = removedStaysInstalled.node("[data-persona-asset-error]").textContent;
    check(
      "前提：刪除後狀態仍是已安裝，而且語音重畫例外有穿出",
      clickedRemoveInstalled === true &&
        seenVoice.seen() === true &&
        calls(removedStaysInstalled, "persona_asset_remove").length === 1,
    );
    check(
      "刪除成功、狀態仍是已安裝而重畫丟例外時，畫面上是已安裝那一句",
      removeInstalledSummary === INSTALLED_SUMMARY,
      removeInstalledSummary,
    );
    check(
      "刪除成功、狀態仍是已安裝而重畫丟例外時，不說刪除沒有完成",
      !removeInstalledError.includes("刪除沒有完成"),
      removeInstalledError,
    );
    check(
      "刪除成功、狀態仍是已安裝而重畫丟例外時，不說刪除結果無法確認",
      !removeInstalledError.includes("刪除結果無法確認"),
      removeInstalledError,
    );
    check(
      "這次刪除成功、狀態仍是已安裝的重畫例外留在函式裡",
      rejections.length === beforeRemoveInstalled,
      rejectionMessages(rejections, beforeRemoveInstalled),
    );

    const cancelled = await open({ asset: ASSET_INSTALLING });
    const seenCancel = escapeAfterStatusPaint(cancelled, AVAILABLE_SUMMARY);
    const beforeCancel = rejections.length;
    const clickedCancel = await cancelled.act("[data-persona-cancel]");
    await tick();
    const cancelSummary = cancelled.node("[data-persona-asset-summary]").textContent;
    const cancelError = cancelled.node("[data-persona-asset-error]").textContent;
    check(
      "前提：取消鍵按得到且重畫例外有穿出",
      clickedCancel === true && seenCancel() === true && calls(cancelled, "persona_asset_cancel").length === 1,
    );
    check(
      "取消成功而重畫丟例外時，畫面上是後端回報的可用狀態",
      cancelSummary === AVAILABLE_SUMMARY,
      cancelSummary,
    );
    check(
      "取消成功而重畫丟例外時，不說取消失敗",
      !cancelError.includes("取消失敗"),
      cancelError,
    );
    check(
      "這次取消成功路徑的重畫例外留在函式裡",
      rejections.length === beforeCancel,
      rejectionMessages(rejections, beforeCancel),
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }
}

console.log("㉚ᵏ 登入項讀回成功後，重畫丟例外不能說成讀不回");
{
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    let reads = 0;
    const page = await open({
      onLoginStartupRead: () => {
        reads += 1;
        if (reads === 1) return { ...LOGIN_STARTUP };
        return {
          state: "enabled",
          expected: LOGIN_STARTUP_EXPECTED,
          actual: LOGIN_STARTUP_EXPECTED,
          reason: null,
        };
      },
      onLoginStartupSet: () => {
        throw new Error("set refused");
      },
    });
    const seen = throwOnText(page.node("[data-login-startup-say]"), "已登錄");
    const before = rejections.length;
    const clicked = await page.changeLoginStartup(true);
    await tick();
    const say = page.loginStartupSay();
    const box = page.node("[data-login-startup]");
    check("前提：登入項寫入有送出且重畫丟了一次", clicked === true && seen.seen() === true && reads === 2, {
      clicked,
      reads,
      say,
    });
    check("這時不說變更後也讀不回", !say.includes("變更後也讀不回"), say);
    check(
      "勾勾是已登錄那一態，不是未知態",
      box.checked === true && box.disabled === false && box.indeterminate === false,
      { checked: box.checked, disabled: box.disabled, indeterminate: box.indeterminate, say },
    );
    check(
      "這次登入項重畫例外留在函式裡",
      rejections.length === before,
      rejectionMessages(rejections, before),
    );
    const disabledNow = box.disabled;
    const checkedNow = box.checked;
    seen.disarm();
    const readsBeforeReload = calls(page, "login_startup_read").length;
    const reloaded = await page.reload();
    const again = page.loginStartupSay();
    const boxAfter = page.node("[data-login-startup]");
    check(
      "讀回 enabled 之後重畫丟例外，勾勾沒有卡死，重讀後是已登錄",
      disabledNow === false &&
        checkedNow === true &&
        reloaded === true &&
        calls(page, "login_startup_read").length === readsBeforeReload + 1 &&
        again.includes("已登錄：Windows 登入項精確符合這一版預期的命令。") &&
        !again.includes("變更後也讀不回") &&
        boxAfter.checked === true &&
        boxAfter.disabled === false &&
        boxAfter.indeterminate === false,
      {
        say,
        again,
        disabledNow,
        checkedNow,
        after: {
          checked: boxAfter.checked,
          disabled: boxAfter.disabled,
          indeterminate: boxAfter.indeterminate,
        },
      },
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }
}

console.log("㉚ᵏ² 登入項寫入成功後，重畫丟例外不能說成變更失敗");
{
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    const page = await open();
    const readsBefore = calls(page, "login_startup_read").length;
    const seen = throwOnText(page.node("[data-login-startup-say]"), "已登錄");
    const before = rejections.length;
    const clicked = await page.changeLoginStartup(true);
    await tick();
    const say = page.loginStartupSay();
    const sayEl = page.node("[data-login-startup-say]");
    const box = page.node("[data-login-startup]");
    check(
      "前提：登入項寫入成功且重畫有丟",
      clicked === true && seen.seen() === true && calls(page, "login_startup_set").length === 1,
      { clicked, say, reads: calls(page, "login_startup_read").length },
    );
    check(
      "登入項寫入成功而重畫丟例外時，不再讀一次，說明也不標成失敗",
      !say.includes("變更 Windows 登入項失敗") &&
        sayEl.classList.contains("bad") === false &&
        calls(page, "login_startup_read").length === readsBefore,
      {
        say,
        bad: sayEl.classList.contains("bad"),
        reads: calls(page, "login_startup_read").length,
        readsBefore,
      },
    );
    check(
      "這次登入項成功路徑的重畫例外留在函式裡",
      rejections.length === before,
      rejectionMessages(rejections, before),
    );
    const disabledNow = box.disabled;
    seen.disarm();
    const readsAtReload = calls(page, "login_startup_read").length;
    const reloaded = await page.reload();
    const again = page.loginStartupSay();
    const boxAfter = page.node("[data-login-startup]");
    check(
      "登入項成功路徑重畫丟例外後沒有卡死，重讀回到已登錄",
      disabledNow === false &&
        reloaded === true &&
        calls(page, "login_startup_read").length === readsAtReload + 1 &&
        again.includes("已登錄：Windows 登入項精確符合這一版預期的命令。") &&
        !again.includes("變更 Windows 登入項失敗") &&
        boxAfter.checked === true &&
        boxAfter.disabled === false &&
        boxAfter.indeterminate === false,
      {
        disabledNow,
        again,
        checked: boxAfter.checked,
        disabled: boxAfter.disabled,
        indeterminate: boxAfter.indeterminate,
      },
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }
}

console.log("㉚ˡ 用量與角色聲音存好後，重畫丟例外不改口");
{
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    const usage = await open();
    const seenUsage = throwOnText(usage.node("[data-usage-state]"), "公開看板已開啟，還沒查過。");
    const beforeUsage = rejections.length;
    usage.node("[data-usage-public-enabled]").checked = true;
    const clickedUsage = await usage.act("[data-usage-public-enabled]", { event: "change" });
    await tick();
    const usageText = usage.node("[data-usage-state]").textContent;
    check(
      "前提：用量設定有送出且重畫丟了一次",
      clickedUsage === true &&
        seenUsage.seen() === true &&
        calls(usage, "usage_public_status_set").length === 1,
    );
    check("用量重畫之後畫面上有字", usageText.length > 0, usageText);
    const usageRefreshNow = usage.node("[data-usage-refresh]").disabled;
    const usagePublicNow = usage.node("[data-usage-public-enabled]").checked;
    seenUsage.disarm();
    const usageReadsBefore = calls(usage, "usage_status_read").length;
    const usageReloaded = await usage.reload();
    const usageAgain = usage.node("[data-usage-state]").textContent;
    const usageRefreshAfter = usage.node("[data-usage-refresh]").disabled;
    const usagePublicAfter = usage.node("[data-usage-public-enabled]").checked;
    check(
      "用量重畫丟例外後沒有卡死，重讀後是已開啟、還沒查過",
      usageRefreshNow === false &&
        usagePublicNow === true &&
        usageReloaded === true &&
        calls(usage, "usage_status_read").length === usageReadsBefore + 1 &&
        usageAgain.includes("公開看板已開啟，還沒查過。") &&
        !usageAgain.includes("用量設定沒有存好") &&
        usageRefreshAfter === false &&
        usagePublicAfter === true,
      { usageText, usageAgain, usageRefreshNow, usageRefreshAfter, usagePublicNow, usagePublicAfter },
    );
    check("用量重畫之後不說沒有存好", !usageText.includes("用量設定沒有存好"), usageText);
    check(
      "這次用量重畫例外留在函式裡",
      rejections.length === beforeUsage,
      rejectionMessages(rejections, beforeUsage),
    );

    const voice = await open({ asset: ASSET_INSTALLED });
    const seenVoice = throwOnText(voice.node("[data-persona-voice-state]"), "本機角色聲音已開啟。");
    const beforeVoice = rejections.length;
    voice.node("[data-persona-voice]").checked = true;
    const clickedVoice = await voice.act("[data-persona-voice]", { event: "change" });
    await tick();
    const voiceBox = voice.node("[data-persona-voice]");
    const voiceText = voice.node("[data-persona-voice-state]").textContent;
    check(
      "前提：角色聲音有送出且重畫丟了一次",
      clickedVoice === true && seenVoice.seen() === true && calls(voice, "persona_voice_set").length === 1,
    );
    check(
      "角色聲音重畫丟例外後，勾勾沒有留在停用",
      voiceBox.disabled === false && voiceBox.checked === true,
      { checked: voiceBox.checked, disabled: voiceBox.disabled, voiceText },
    );
    check("角色聲音重畫之後不說沒有改", !voiceText.includes("語音設定沒有改"), voiceText);
    const voiceCheckedNow = voiceBox.checked;
    seenVoice.disarm();
    const voiceReadsBefore = calls(voice, "persona_read").length;
    const voiceReloaded = await voice.reload();
    const voiceAgain = voice.node("[data-persona-voice-state]").textContent;
    const voiceAfter = voice.node("[data-persona-voice]");
    check(
      "勾勾是開的，畫面沒有說沒有改，重讀後句子和勾勾一致",
      voiceCheckedNow === true &&
        !voiceText.includes("語音設定沒有改") &&
        voiceReloaded === true &&
        calls(voice, "persona_read").length === voiceReadsBefore + 1 &&
        voiceAgain === "本機角色聲音已開啟。" &&
        voiceAfter.checked === true &&
        voiceAfter.disabled === false &&
        !voiceAgain.includes("語音設定沒有改"),
      {
        voiceCheckedNow,
        voiceText,
        voiceAgain,
        afterChecked: voiceAfter.checked,
        afterDisabled: voiceAfter.disabled,
      },
    );
    check(
      "這次角色聲音重畫例外留在函式裡",
      rejections.length === beforeVoice,
      rejectionMessages(rejections, beforeVoice),
    );

    const unknown = await open({
      asset: ASSET_INSTALLED,
      onVoiceSet: () => ({}),
    });
    const seenUnknown = throwOnText(
      unknown.node("[data-persona-voice-state]"),
      "無法確認是否已改",
    );
    const beforeUnknown = rejections.length;
    unknown.node("[data-persona-voice]").checked = true;
    const clickedUnknown = await unknown.act("[data-persona-voice]", { event: "change" });
    await tick();
    const unknownText = unknown.node("[data-persona-voice-state]").textContent;
    check(
      "前提：缺少開關的那一臂有走到且重畫丟了一次",
      clickedUnknown === true && seenUnknown.seen() === true && calls(unknown, "persona_voice_set").length === 1,
    );
    seenUnknown.disarm();
    const unknownReadsBefore = calls(unknown, "persona_read").length;
    const unknownReloaded = await unknown.reload();
    const unknownAgain = unknown.node("[data-persona-voice-state]").textContent;
    const unknownAfter = unknown.node("[data-persona-voice]");
    check(
      "無法確認那一臂重畫丟例外後沒有卡死，重讀後句子和勾勾一致",
      !unknownText.includes("語音設定沒有改") &&
        unknownReloaded === true &&
        calls(unknown, "persona_read").length === unknownReadsBefore + 1 &&
        unknownAfter.disabled === false &&
        unknownAfter.checked === false &&
        unknownAgain === "本機聲音目前關閉。" &&
        !unknownAgain.includes("語音設定沒有改"),
      {
        unknownText,
        unknownAgain,
        afterChecked: unknownAfter.checked,
        afterDisabled: unknownAfter.disabled,
      },
    );
    check(
      "無法確認那一臂重畫丟例外後，不改口成語音設定沒有改",
      !unknownText.includes("語音設定沒有改"),
      unknownText,
    );
    check(
      "這次無法確認重畫例外留在函式裡",
      rejections.length === beforeUnknown,
      rejectionMessages(rejections, beforeUnknown),
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }
}

console.log("㉚ᵐ 熱鍵設定成功後，重畫丟例外不能把選單寫回舊組合");
{
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    const page = await open();
    const seen = throwOnText(page.node("[data-hotkey-say]"), "Ctrl + Alt + S");
    const stateReadsBefore = calls(page, "hotkey_state").length;
    const before = rejections.length;
    await page.pressCombo({ key: "s", code: "KeyS", ctrlKey: true, altKey: true });
    await tick();
    check(
      "前提：hotkey_set 有送出且重畫丟了一次",
      seen.seen() === true && calls(page, "hotkey_set").length === 1,
      { combo: page.combo(), say: page.hotkeySay() },
    );
    check(
      "hotkey_set 成功而重畫丟例外時，選單是新組合",
      page.combo() === "Ctrl + Alt + S",
      page.combo(),
    );
    check(
      "成功路徑沒有再讀 hotkey_state，也就沒有走到 restoreCombo",
      calls(page, "hotkey_state").length === stateReadsBefore,
      calls(page, "hotkey_state").length,
    );
    check(
      "這次熱鍵重畫例外留在函式裡",
      rejections.length === before,
      rejectionMessages(rejections, before),
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }
}

console.log("㉛ 能力報告的 Unknown 不會被畫成可用或不可用");
{
  const available = await open();
  check(
    "URL Working 只說這場真的讀到過可供比對的網址",
    available.health().includes("至少有 12 個瀏覽器拍讀到網址") &&
      available.health().includes("可供這幾條規則比對") &&
      !available.health().includes("生效中"),
    available.health(),
  );
  check("輸入 hook 已驗證可用時不畫警告", available.machineHidden(), available.machine());

  const unknown = await open({
    onHealth: () => ({
      broken: [],
      capture_off: false,
      input_hook: "unknown",
      url_rules: { kind: "unknown" },
      at: 1_755_000_000_000,
    }),
  });
  check("URL Unknown 是灰色問號狀態", !unknown.healthHidden() && unknown.healthUnknown(), unknown.health());
  check("URL Unknown 明說沒量到", unknown.health().includes("沒有量到"), unknown.health());
  check("URL Unknown 不宣稱生效或失效", !unknown.health().includes("生效中") && !unknown.health().includes("規則失效"), unknown.health());
  check("hook Unknown 是灰色問號狀態", !unknown.machineHidden() && unknown.machineUnknown(), unknown.machine());
  check("hook Unknown 不宣稱已探測或節奏必然是空的", !unknown.machine().includes("已探測") && !unknown.machine().includes("會是空的"), unknown.machine());
}

{
  let reports = 0;
  const changed = await open({
    onHealth: () => {
      reports += 1;
      return reports === 1
        ? {
            broken: [],
            capture_off: false,
            input_hook: "available",
            url_rules: { kind: "working", reads: 4 },
            at: 1_755_000_000_000,
          }
        : {
            broken: [],
            capture_off: false,
            input_hook: "unknown",
            url_rules: { kind: "unknown" },
            at: 1_755_000_000_000,
          };
    },
  });
  check("Working 開始時是綠燈", changed.healthOk(), changed.health());
  await changed.reload();
  check(
    "同一頁轉成 Unknown 會拿掉上一拍的綠燈",
    changed.healthUnknown() && !changed.healthOk(),
    changed.health(),
  );
}

{
  const privacyReason =
    "開機探測拿不到 UIA；若錄製後端同樣無法確認，會停在內容來源前。";
  const combined = await open({
    onHealth: () => ({
      broken: [{ about: "privacy_capture", message: privacyReason }],
      capture_off: false,
      input_hook: "unknown",
      url_rules: { kind: "broken" },
      at: 1_755_000_000_000,
    }),
  });
  check(
    "PrivacyCapture 警告與 hook Unknown 同時呈現",
    combined.machine().includes(privacyReason) &&
      combined.machine().includes("還不知道輸入 hook"),
    combined.machine(),
  );
  check(
    "UrlRules Broken 不會在旁邊說假的沒有原因",
    combined.healthHidden() && !combined.health().includes("找不到對應原因"),
    { health: combined.health(), machine: combined.machine() },
  );
}

{
  const bootProbe = await open({
    config: { ...BASE, excluded_urls: [] },
    onHealth: () => ({
      broken: [
        {
          about: "privacy_capture",
          message: "開機探測拿不到 UIA；若錄製後端同樣無法確認，會停在內容來源前。",
        },
      ],
      capture_off: false,
      input_hook: "available",
      url_rules: { kind: "none" },
      at: 1_755_000_000_000,
    }),
  });
  check(
    "沒有 URL 規則也會顯示 UIA 開機探測警告",
    !bootProbe.machineHidden() &&
      bootProbe.machine().includes("若錄製後端同樣無法確認，會停在內容來源前"),
    bootProbe.machine(),
  );
}

{
  // 舊 desktop 或損壞的 payload 漏了新欄位，也必須 fail-unknown，不能走
  // 舊的 `?? { kind: "none" }` 藏掉整格。
  const legacy = await open({
    onHealth: () => ({ broken: [], capture_off: false, at: 1_755_000_000_000 }),
  });
  check("漏 url_rules 的舊回應不會假裝無規則", !legacy.healthHidden() && legacy.healthUnknown(), legacy.health());
  check("漏 input_hook 的舊回應不會假裝 hook 可用", !legacy.machineHidden() && legacy.machineUnknown(), legacy.machine());
}

{
  const unaskable = await open({
    onHealth: (() => {
      let reports = 0;
      return () => {
        reports += 1;
        if (reports === 1) {
          return {
            broken: [],
            capture_off: false,
            input_hook: "available",
            url_rules: { kind: "working", reads: 3 },
            at: 1_755_000_000_000,
          };
        }
        throw new Error("capabilities.json 讀不到");
      };
    })(),
  });
  check("privacy_health 問得到時先是綠燈", unaskable.healthOk(), unaskable.health());
  await unaskable.reload();
  check(
    "privacy_health 後來問不到會拿掉上一拍綠燈",
    unaskable.healthUnknown() && !unaskable.healthOk(),
    unaskable.health(),
  );
  check("privacy_health 整個問不到時 hook 仍畫 Unknown", !unaskable.machineHidden() && unaskable.machineUnknown(), unaskable.machine());
  check("問不到不會留住或藏成 hook 可用", unaskable.machine().includes("還不知道") && !unaskable.machine().includes("已探測"), unaskable.machine());
}

console.log("㉛ Usage 開關、停止、錯誤與公開重置不把剩餘畫成 0");
{
  const p = await open();
  check(
    "出廠關閉，重新查詢是灰的",
    p.node("[data-usage-refresh]").disabled === true,
  );
  check(
    "關閉時不宣稱已連線",
    p.node("[data-usage-state]").textContent.includes("關閉"),
    p.node("[data-usage-state]").textContent,
  );
  await p.act("[data-usage-public-enabled]", { trusted: false, event: "change" });
  check("假 change 不寫公開看板", calls(p, "usage_public_status_set").length === 0);
  p.node("[data-usage-public-enabled]").checked = true;
  await p.act("[data-usage-public-enabled]", { event: "change" });
  check("真人開啟只送一次 set", calls(p, "usage_public_status_set").length === 1);
  p.node("[data-usage-refresh]").disabled = false;
  await p.act("[data-usage-refresh]");
  const board = p.node("[data-usage-board]").textContent;
  check("公開重置有寫出產品", board.includes("Codex") && board.includes("已驗證重置"), board);
  check("剩餘 token 保持未知", board.includes("剩餘 token：未知"), board);
  check("已觀察 token 不是 0 冒充", board.includes("已觀察 token：125"), board);
  check("額度快照不是剩餘", board.includes("額度快照已用 12%") && !board.includes("剩餘 token：0"), board);
  check(
    "觀察時間是時鐘不是 unix 毫秒",
    board.includes("2025-09-12 02:40 UTC") && !board.includes("1757644836000"),
    board,
  );
}

{
  const p = await open();
  p.setUsage({
    enabled: true,
    stopped: true,
    served_from: "stopped",
    config_readable: true,
  });
  await p.emit("usage-status-changed");
  check(
    "全停時說明沒有查詢",
    p.node("[data-usage-state]").textContent.includes("全停"),
    p.node("[data-usage-state]").textContent,
  );
  check("全停時查詢鍵不能按", p.node("[data-usage-refresh]").disabled === true);
}

{
  const p = await open();
  p.setUsage({
    enabled: true,
    config_readable: true,
    fetch_error: "連線逾時",
    served_from: "network-error",
    local_scan_complete: false,
    local_sessions_enabled: true,
    local_files_capped: 3,
  });
  await p.emit("usage-status-changed");
  const text = p.node("[data-usage-state]").textContent;
  check("網路錯誤說得出沒查到", text.includes("沒查到") && text.includes("連線逾時"), text);
  check("部分掃描會講出來", text.includes("部分掃描"), text);
  check("錯誤是紅的", p.node("[data-usage-state]").classList.contains("bad"));
}

function usageText(view) {
  return `${view.state}\n${view.board}`;
}

console.log("㉜ Usage 沒查過、上一份、即時、全停是四句");
{
  const p = await open();
  async function paint(patch) {
    p.setUsage(patch);
    await p.emit("usage-status-changed");
    return {
      state: p.node("[data-usage-state]").textContent,
      board: p.node("[data-usage-board]").textContent,
    };
  }
  const never = await paint({
    enabled: true,
    stopped: false,
    config_readable: true,
    config_error: null,
    fetch_error: null,
    board_live: false,
    last_success_unix_ms: null,
    board_updated_at: null,
    products: [],
    local_products: [],
  });
  const failed = await paint({
    enabled: true,
    stopped: false,
    config_readable: true,
    config_error: null,
    fetch_error: "連線逾時",
    board_live: false,
    last_success_unix_ms: 1_757_644_836_000,
    board_updated_at: "2026-09-21T01:00:22.000Z",
    products: [
      {
        id: "codex",
        name: "Codex",
        reset: "confirmed",
        event_id: "codex:2026-09-12",
        announced_at: "2026-09-12T03:20:36.000Z",
        public_event_count: 1,
        forecast_p24: null,
        forecast_p48: null,
        forecast_basis: null,
      },
    ],
  });
  const live = await paint({
    enabled: true,
    stopped: false,
    config_readable: true,
    config_error: null,
    fetch_error: null,
    served_from: "network",
    board_live: true,
    last_success_unix_ms: 1_757_644_836_000,
    products: [
      {
        id: "codex",
        name: "Codex",
        reset: "confirmed",
        event_id: "codex:2026-09-12",
        announced_at: "2026-09-12T03:20:36.000Z",
        public_event_count: 1,
        forecast_p24: null,
        forecast_p48: null,
        forecast_basis: null,
      },
    ],
  });
  const stopped = await paint({
    enabled: true,
    stopped: true,
    config_readable: true,
    config_error: null,
    fetch_error: null,
    board_live: false,
    products: [],
  });
  check("沒查過", never.state.includes("還沒查過"), never.state);
  check(
    "這次失敗且底下列的是上一份",
    failed.state.includes("沒查到") &&
      failed.state.includes("底下列的是上一份") &&
      failed.state.includes("2025-09-12 02:40 UTC"),
    failed.state,
  );
  check("這次成功是即時結果", live.state.includes("即時結果"), live.state);
  check("全停", stopped.state.includes("全停"), stopped.state);
  const four = [
    ["沒查過", never.state],
    ["上一份", failed.state],
    ["即時", live.state],
    ["全停", stopped.state],
  ];
  for (let i = 0; i < four.length; i++) {
    for (let j = i + 1; j < four.length; j++) {
      check(
        `${four[i][0]} 和 ${four[j][0]} 不是同一句`,
        four[i][1] !== four[j][1],
        { a: four[i][1], b: four[j][1] },
      );
    }
  }
  const closed = await paint({
    enabled: false,
    stopped: false,
    config_readable: true,
    config_error: null,
    fetch_error: null,
    board_live: false,
    products: [],
  });
  check(
    "關閉也不是上面四句",
    closed.state.includes("關閉") && four.every((entry) => entry[1] !== closed.state),
    closed.state,
  );

  const emptyLive = await paint({
    enabled: true,
    stopped: false,
    config_readable: true,
    config_error: null,
    fetch_error: null,
    served_from: "network",
    board_live: true,
    products: [
      {
        id: "codex",
        name: "Codex",
        reset: "none",
        event_id: null,
        announced_at: null,
        public_event_count: 0,
        forecast_p24: null,
        forecast_p48: null,
        forecast_basis: null,
      },
    ],
  });
  const missed = await paint({
    enabled: true,
    stopped: false,
    config_readable: true,
    config_error: null,
    fetch_error: "連線逾時",
    board_live: false,
    last_success_unix_ms: null,
    board_updated_at: null,
    products: [],
  });
  const emptyText = usageText(emptyLive);
  const missedText = usageText(missed);
  check(
    "查過但看板上沒有事件",
    emptyText.includes("沒有已驗證重置事件") && emptyText.includes("即時結果") && !emptyText.includes("沒查到"),
    emptyText,
  );
  check(
    "沒查到看板",
    missedText.includes("沒查到") && !missedText.includes("沒有已驗證重置事件"),
    missedText,
  );
  check("兩種零不是同一句", emptyText !== missedText, { emptyText, missedText });
}

console.log("㊲ Usage 即時只在這次問過網路；磁碟回想要帶查到時間");
{
  const p = await open();
  async function paint(patch) {
    p.setUsage(patch);
    await p.emit("usage-status-changed");
    return p.node("[data-usage-state]").textContent;
  }
  const base = {
    enabled: true,
    stopped: false,
    config_readable: true,
    config_error: null,
    fetch_error: null,
    board_live: true,
    products: [],
    local_products: [],
  };
  const network = await paint({
    ...base,
    served_from: "network",
    last_success_unix_ms: 1_757_644_836_000,
  });
  const cached = await paint({
    ...base,
    served_from: "cache",
    last_success_unix_ms: 1_757_644_836_000,
    board_updated_at: "1999-01-01T00:00:00.000Z",
  });
  const cachedNoTime = await paint({
    ...base,
    served_from: "cache",
    last_success_unix_ms: null,
    board_updated_at: "1999-01-01T00:00:00.000Z",
  });
  const never = await paint({
    ...base,
    served_from: "cache",
    board_live: false,
    last_success_unix_ms: null,
    board_updated_at: null,
  });
  check("network 才說即時", network.includes("即時"), network);
  check(
    "cache 有時間不說即時，而且帶 UTC 時鐘",
    !cached.includes("即時") && cached.includes("2025-09-12 02:40 UTC"),
    cached,
  );
  check(
    "cache 沒時間不說即時，也不是還沒查過",
    !cachedNoTime.includes("即時") &&
      !cachedNoTime.includes("還沒查過") &&
      cachedNoTime.includes("查到過") &&
      cachedNoTime.includes("沒有記下時間") &&
      cachedNoTime !== never &&
      !cachedNoTime.includes("1999-01-01"),
    cachedNoTime,
  );
  const three = [
    ["network", network],
    ["cache", cached],
    ["cache 沒時間", cachedNoTime],
  ];
  for (let i = 0; i < three.length; i++) {
    for (let j = i + 1; j < three.length; j++) {
      check(
        `${three[i][0]} 和 ${three[j][0]} 不是同一句`,
        three[i][1] !== three[j][1],
        { a: three[i][1], b: three[j][1] },
      );
    }
  }
}

console.log("㉝ Usage 設定讀不出來和看板狀態檔讀不出來不是同一句");
{
  const p = await open();
  const sentences = [
    {
      name: "找不到設定路徑",
      patch: {
        config_readable: false,
        config_error: "找不到設定檔路徑。公開看板與本機用量都還沒讀。",
        enabled: null,
      },
      includes: ["找不到設定檔路徑", "都還沒讀"],
    },
    {
      name: "設定解析失敗",
      patch: {
        config_readable: false,
        config_error: "用量設定讀不出來：config.toml 解析失敗。公開看板與本機用量都還沒讀。",
        enabled: null,
      },
      includes: ["用量設定讀不出來", "都還沒讀"],
    },
    {
      name: "找不到資料目錄",
      patch: {
        config_readable: true,
        enabled: true,
        config_error: "找不到資料目錄。公開看板狀態還沒讀。本機用量仍照它自己的設定讀。",
        fetch_error: null,
        board_live: false,
      },
      includes: ["找不到資料目錄", "本機用量仍照它自己的設定讀"],
    },
    {
      name: "狀態檔讀不出來",
      patch: {
        config_readable: true,
        enabled: true,
        config_error:
          "公開看板狀態檔 usage-public-status-v1.json 讀不出來。刪掉該檔後再查。本機用量不受這個檔影響。",
        fetch_error: null,
        board_live: false,
      },
      includes: ["usage-public-status-v1.json", "刪掉該檔後再查"],
    },
  ];
  const painted = [];
  for (const sentence of sentences) {
    p.setUsage(sentence.patch);
    await p.emit("usage-status-changed");
    const text = p.node("[data-usage-state]").textContent;
    painted.push(text);
    check(
      sentence.name,
      sentence.includes.every((part) => text.includes(part)),
      text,
    );
  }
  for (let i = 0; i < painted.length; i++) {
    for (let j = i + 1; j < painted.length; j++) {
      check(`${sentences[i].name} 和 ${sentences[j].name} 不是同一句`, painted[i] !== painted[j], {
        a: painted[i],
        b: painted[j],
      });
    }
  }
  check(
    "狀態檔那句有下一步且不是設定檔那句",
    painted[3].includes("刪掉該檔後再查") && !painted[1].includes("刪掉該檔後再查"),
    { config: painted[1], store: painted[3] },
  );
}

console.log("㉞ Usage 四種重置值各一句");
{
  const p = await open();
  p.setUsage({
    enabled: true,
    board_live: true,
    config_error: null,
    fetch_error: null,
    products: [
      {
        id: "codex",
        name: "Codex",
        reset: "confirmed",
        announced_at: "2026-09-12T03:20:36.000Z",
        event_id: "codex:1",
        public_event_count: 1,
        forecast_p24: null,
        forecast_p48: null,
        forecast_basis: null,
      },
      {
        id: "claude",
        name: "Claude",
        reset: "unverified",
        announced_at: "2026-09-13T00:00:00.000Z",
        event_id: "claude:1",
        public_event_count: 1,
        forecast_p24: null,
        forecast_p48: null,
        forecast_basis: null,
      },
      {
        id: "chatgpt",
        name: "ChatGPT",
        reset: "other",
        announced_at: "2026-09-14T01:02:03.000Z",
        event_id: "chatgpt:1",
        public_event_count: 1,
        forecast_p24: null,
        forecast_p48: null,
        forecast_basis: null,
      },
      {
        id: "cursor",
        name: "Cursor",
        reset: "none",
        announced_at: null,
        event_id: null,
        public_event_count: 0,
        forecast_p24: null,
        forecast_p48: null,
        forecast_basis: null,
      },
    ],
  });
  await p.emit("usage-status-changed");
  const lines = p.node("[data-usage-board]").textContent.split("\n");
  const clause = (prefix) => {
    const line = lines.find((item) => item.startsWith(prefix));
    return line.replace(new RegExp(`^${prefix}`), "").replace(/。無預測$/, "");
  };
  const confirmed = clause("Codex ");
  const unverified = clause("Claude ");
  const other = clause("ChatGPT ");
  const none = clause("Cursor ");
  check("confirmed 有時間", confirmed.includes("已驗證重置") && confirmed.includes("2026-09-12T03:20:36.000Z"), confirmed);
  check("unverified 單獨一句", unverified === "有事件但未驗證，不當成重置", unverified);
  check(
    "other 說明不是重置並帶時間",
    other.includes("不是重置的事件") && other.includes("2026-09-14T01:02:03.000Z"),
    other,
  );
  check("none 是沒有已驗證事件", none === "公開看板沒有已驗證重置事件", none);
  const resets = [
    ["confirmed", confirmed],
    ["unverified", unverified],
    ["other", other],
    ["none", none],
  ];
  for (let i = 0; i < resets.length; i++) {
    for (let j = i + 1; j < resets.length; j++) {
      check(`${resets[i][0]} 和 ${resets[j][0]} 不是同一句`, resets[i][1] !== resets[j][1], {
        a: resets[i][1],
        b: resets[j][1],
      });
    }
  }
}

console.log("㉟ Usage 跳過的檔要出現在狀態句");
{
  const p = await open();
  p.setUsage({
    enabled: false,
    local_sessions_enabled: true,
    local_scan_complete: false,
    local_skipped_deep: 2,
    local_skipped_symlink: 1,
    local_skipped_hidden: 3,
    local_products: [
      {
        id: "codex",
        name: "Codex",
        observed_tokens: 10,
        remaining_tokens: null,
        observed_at_unix_ms: null,
        quota_used_percent: null,
      },
    ],
  });
  await p.emit("usage-status-changed");
  const partial = p.node("[data-usage-state]").textContent;
  check(
    "部分掃描寫出深度、連結、點開頭",
    partial.includes("部分掃描") &&
      partial.includes("深度超過上限的目錄 2 個") &&
      partial.includes("符號連結 1 個") &&
      partial.includes("點開頭的項目 3 個"),
    partial,
  );
  p.setUsage({
    enabled: false,
    local_sessions_enabled: true,
    local_scan_complete: true,
    local_skipped_auth: 2,
    local_products: [
      {
        id: "codex",
        name: "Codex",
        observed_tokens: 10,
        remaining_tokens: null,
        observed_at_unix_ms: null,
        quota_used_percent: null,
      },
    ],
  });
  await p.emit("usage-status-changed");
  const auth = p.node("[data-usage-state]").textContent;
  check("驗證檔有單獨講而且有數量", auth.includes("另有 2 個驗證檔沒有讀"), auth);
  check("刻意不讀驗證檔不算部分掃描", !auth.includes("部分掃描"), auth);
}

console.log("㊱ Usage 剩餘 token 讀後端欄位");
{
  const p = await open();
  p.setUsage({
    enabled: true,
    board_live: true,
    config_error: null,
    fetch_error: null,
    local_sessions_enabled: true,
    local_products: [
      {
        id: "codex",
        name: "Codex",
        observed_tokens: 125,
        remaining_tokens: null,
        observed_at_unix_ms: null,
        quota_used_percent: 12,
      },
    ],
    products: [],
  });
  await p.emit("usage-status-changed");
  const unknown = p.node("[data-usage-board]").textContent;
  check("remaining_tokens null 是未知", unknown.includes("剩餘 token：未知"), unknown);
  p.setUsage({
    enabled: true,
    board_live: true,
    config_error: null,
    fetch_error: null,
    local_sessions_enabled: true,
    local_products: [
      {
        id: "codex",
        name: "Codex",
        observed_tokens: 125,
        remaining_tokens: 1234,
        observed_at_unix_ms: 1_757_644_836_000,
        quota_used_percent: 12,
      },
    ],
    products: [],
  });
  await p.emit("usage-status-changed");
  const counted = p.node("[data-usage-board]").textContent;
  check("remaining_tokens 1234 印在畫面上", counted.includes("剩餘 token：1234"), counted);
  check("有剩餘時不再寫未知", !counted.includes("剩餘 token：未知"), counted);
  check(
    "有值那列的時間不是 unix 毫秒",
    counted.includes("2025-09-12 02:40 UTC") && !counted.includes("1757644836000"),
    counted,
  );
}

console.log("㉚ⁿ 診斷檔寫出去之後，重畫丟例外不改口成寫不出來");
{
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    const path = "C:\\Users\\ted\\AppData\\Roaming\\ted-h\\AI-Sister\\diagnose-r8.txt";
    const page = await open({
      onDiagnoseExport: () => path,
    });
    const seen = throwOnText(page.node("[data-diagnose-say]"), "寫好了");
    const before = rejections.length;
    const clicked = await page.act("[data-diagnose-export]");
    await tick();
    const say = page.node("[data-diagnose-say]").textContent;
    const button = page.node("[data-diagnose-export]");
    check(
      "前提：診斷有送出且成功句有丟",
      clicked === true && seen.seen() === true && calls(page, "diagnose_export").length === 1,
      { clicked, seen: seen.seen(), say, invokes: calls(page, "diagnose_export") },
    );
    check(
      "診斷寫出去而重畫丟例外時，不說寫不出來",
      !say.includes("寫不出來") && !say.includes("repaint failed"),
      say,
    );
    check(
      "這次診斷重畫例外留在函式裡",
      rejections.length === before,
      rejectionMessages(rejections, before),
    );
    check("診斷鈕沒有留在停用", button.disabled === false, button.disabled);
    seen.disarm();
    const againClicked = await page.act("[data-diagnose-export]");
    const again = page.node("[data-diagnose-say]").textContent;
    check(
      "再按一次仍寫得出路徑，而且不說寫不出來",
      againClicked === true &&
        calls(page, "diagnose_export").length === 2 &&
        again.includes(`寫好了：${path}`) &&
        !again.includes("寫不出來") &&
        page.node("[data-diagnose-export]").disabled === false,
      { againClicked, again, disabled: page.node("[data-diagnose-export]").disabled },
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }

  const failed = await open({
    onDiagnoseExport: () => {
      throw new Error("磁碟滿了");
    },
  });
  const failClicked = await failed.act("[data-diagnose-export]");
  const failSay = failed.node("[data-diagnose-say]").textContent;
  check(
    "診斷真的寫不出去時，仍說寫不出來和原因",
    failClicked === true && failSay.includes("寫不出來") && failSay.includes("磁碟滿了"),
    failSay,
  );
  check(
    "診斷失敗之後按鈕可以再按",
    failed.node("[data-diagnose-export]").disabled === false,
    failed.node("[data-diagnose-export]").disabled,
  );
}

console.log("㉚ᵒ macOS 設定已經打開時，重畫丟例外不改口成失敗");
{
  const rejections = [];
  const noteRejection = (reason) => {
    rejections.push(reason);
  };
  process.on("unhandledRejection", noteRejection);
  try {
    const denied = {
      platform: "macos",
      screen_recording: false,
      accessibility: false,
    };
    const page = await open({
      platformAccess: denied,
      onPlatformAccessOpen: (kind, current, store) => {
        const next = {
          ...current,
          screen_recording: kind === "screen-recording" ? true : current.screen_recording,
          accessibility: kind === "accessibility" ? true : current.accessibility,
        };
        store(next);
        return next;
      },
    });
    const seen = throwOnText(page.node("[data-platform-access-say]"), "已開啟 macOS 設定");
    const before = rejections.length;
    const readsBefore = calls(page, "platform_access_read").length;
    const clicked = await page.act("[data-platform-screen-open]");
    await tick();
    const say = page.platformAccessSay();
    const sayEl = page.node("[data-platform-access-say]");
    const button = page.node("[data-platform-screen-open]");
    check(
      "前提：螢幕錄製設定有送出且成功句有丟",
      clicked === true &&
        seen.seen() === true &&
        calls(page, "platform_access_open").length === 1 &&
        calls(page, "platform_access_open")[0].arg?.kind === "screen-recording",
      { clicked, seen: seen.seen(), say, invokes: calls(page, "platform_access_open") },
    );
    check(
      "設定已經打開而重畫丟例外時，不說重畫例外，說明也不標成失敗",
      !say.includes("repaint failed") &&
        sayEl.classList.contains("bad") === false &&
        calls(page, "platform_access_read").length === readsBefore,
      {
        say,
        bad: sayEl.classList.contains("bad"),
        reads: calls(page, "platform_access_read").length,
        readsBefore,
      },
    );
    check(
      "這次系統設定重畫例外留在函式裡",
      rejections.length === before,
      rejectionMessages(rejections, before),
    );
    check("開啟設定的按鈕沒有留在停用", button.disabled === false, {
      disabled: button.disabled,
      hidden: button.hidden,
    });
    seen.disarm();
    const readsAt = calls(page, "platform_access_read").length;
    await page.emitWindow("focus");
    const again = page.platformAccessSay();
    check(
      "切回這一頁重讀後是已開啟的螢幕錄製，不留失敗句",
      calls(page, "platform_access_read").length === readsAt + 1 &&
        page.node("[data-platform-screen-state]").textContent === "已開啟" &&
        page.node("[data-platform-ax-state]").textContent === "未開啟" &&
        !again.includes("repaint failed") &&
        sayEl.classList.contains("bad") === false &&
        button.disabled === false,
      {
        again,
        bad: sayEl.classList.contains("bad"),
        screen: page.node("[data-platform-screen-state]").textContent,
        ax: page.node("[data-platform-ax-state]").textContent,
        reads: calls(page, "platform_access_read").length,
        disabled: button.disabled,
      },
    );
  } finally {
    process.off("unhandledRejection", noteRejection);
  }

  const failed = await open({
    platformAccess: {
      platform: "macos",
      screen_recording: false,
      accessibility: false,
    },
    onPlatformAccessOpen: () => {
      throw new Error("設定打不開");
    },
  });
  const failClicked = await failed.act("[data-platform-screen-open]");
  const failSay = failed.platformAccessSay();
  check(
    "系統設定真的打不開時，仍把原因標成失敗",
    failClicked === true &&
      failSay.includes("設定打不開") &&
      failed.node("[data-platform-access-say]").classList.contains("bad") === true &&
      failed.node("[data-platform-screen-open]").disabled === false,
    {
      failSay,
      bad: failed.node("[data-platform-access-say]").classList.contains("bad"),
      disabled: failed.node("[data-platform-screen-open]").disabled,
    },
  );
}

console.log("㉚ᵠ 打開系統設定時 invoke 丟出空值，仍走失敗那一臂");
{
  // 拒絕的理由是 null，不是 Error。null 的真假是假的；走哪一臂要看旗標。
  const falsy = await open({
    platformAccess: {
      platform: "macos",
      screen_recording: false,
      accessibility: false,
    },
    onPlatformAccessOpen: () => {
      throw null;
    },
  });
  const clicked = await falsy.act("[data-platform-screen-open]");
  const sayEl = falsy.node("[data-platform-access-say]");
  check(
    "invoke 丟出空值時仍走失敗：說明標成失敗",
    clicked === true && sayEl.classList.contains("bad") === true,
    { clicked, bad: sayEl.classList.contains("bad") },
  );
}

console.log("");
console.log(`${passed} passed; ${failed} failed`);
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——設定頁在某一種情況下說了謊，或什麼都沒說。`);
  process.exit(1);
}
console.log("✓ 設定頁：成功和失敗都說得出話，而失敗不會偷偷刪掉他的規則");
