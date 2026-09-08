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
const MAIN = read(resolve(UI, "../src-tauri/src/main.rs"));

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
  frames_days: 14,
  text_days: 90,
  brain_command: "",
  brain_args: [],
  persona_enabled: true,
  persona_id: "neutral",
  persona_motion: true,
  persona_tap_lines: true,
};

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
  asset = ASSET_AVAILABLE,
  voice = false,
  hotkey = HOTKEY,
  loginStartup = LOGIN_STARTUP,
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
  globalThis.addEventListener = (ev, fn) => {
    if (ev === "keydown") keys.push(fn);
  };
  globalThis.removeEventListener = () => {};

  let state = { ...config };
  let assetState = { ...asset, disclosure: asset.disclosure ? { ...asset.disclosure } : null };
  let voiceState = voice;
  let loginStartupState = { ...loginStartup };
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
            return { voice_enabled: voiceState };
          case "persona_voice_set":
            if (onVoiceSet) {
              return onVoiceSet(arg, (enabled) => (voiceState = enabled));
            }
            voiceState = arg.enabled;
            return { voice_enabled: voiceState };
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
  await boot();
  await tick();
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
    setAsset(s) {
      assetState = { ...s, disclosure: s.disclosure ? { ...s.disclosure } : null };
    },
    combo: () => node("[data-combo]").textContent,
    hotkeySay: () => node("[data-hotkey-say]").textContent,
    handsCombo: () => node("[data-hands-combo]").textContent,
    handsHotkeySay: () => node("[data-hands-hotkey-say]").textContent,
    brainSay: () => node("[data-brain-say]").textContent,
    brainHidden: () => node("[data-brain-say]").hidden,
    loginStartupSay: () => node("[data-login-startup-say]").textContent,
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
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

function calls(p, command) {
  return p.invokes.filter(({ cmd }) => cmd === command);
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
  noCommand:
    "還沒填命令：解釋層和審閱層一次都不會醒。空著就是關，不是跑得慢一點。",
  noConsent:
    "命令有了，但第二張同意書還沒勾：螢幕上的字只留在這台機器，一次都不會交給這支 CLI。去「三張同意書」那一頁勾上雲解讀。",
  readyIdle:
    "命令和同意書都齊了。現在沒有人在錄，等你按下「開始記錄」她才會自己醒。",
  readyBooting:
    "命令和同意書都齊了。有一個 sister record 正在起來（多半在開資料庫）——它一開始錄，她就會自己醒，不必再按「開始記錄」。",
  live: "命令和同意書都齊了，而且正在錄：她會自己醒。",
  readyThinking:
    "命令和同意書都齊了。上一場錄製剛停，解釋層還在把最後一段想完——想完才能再開一場，這時候按「開始記錄」會被擋下來。",
};

console.log("⑫ 大腦：沒填命令（同意書勾了、正在錄也不算）");
{
  const p = await open({ cloud: true, watching: "recording" });
  check("就是那一句", p.brainSay() === SENTENCE.noCommand, p.brainSay());
  check("句子裡沒有「同意書」（那是另一種修法）", !p.brainSay().includes("同意書"), p.brainSay());
}

console.log("⑬ 大腦：填了命令，第二張沒勾（正在錄也不算）");
{
  const p = await open({
    config: { ...BASE, brain_command: "claude", brain_args: ["-p"] },
    cloud: false,
    watching: "recording",
  });
  check("就是那一句", p.brainSay() === SENTENCE.noConsent, p.brainSay());
  check("指得出去哪裡勾", p.brainSay().includes("三張同意書"), p.brainSay());
  check(
    "不是「還沒填命令」那句",
    p.brainSay() !== SENTENCE.noCommand,
    p.brainSay(),
  );
  check("是紅的", p.node("[data-brain-say]").classList.contains("bad"), p.brainSay());
}

console.log("⑭ 大腦：命令和同意書都齊，沒有人在錄");
{
  const p = await open({
    config: { ...BASE, brain_command: "claude" },
    cloud: true,
    watching: "none",
  });
  check("就是那一句", p.brainSay() === SENTENCE.readyIdle, p.brainSay());
  check("不是正在錄那句", p.brainSay() !== SENTENCE.live, p.brainSay());
}

console.log("⑮ 大腦：兩個都成立而且正在錄");
{
  const p = await open({
    config: { ...BASE, brain_command: "claude" },
    cloud: true,
    watching: "recording",
  });
  check("就是那一句", p.brainSay() === SENTENCE.live, p.brainSay());
  check("是綠的", p.node("[data-brain-say]").classList.contains("ok"), p.brainSay());
}

console.log("⑯ 大腦：兩個都齊，record 正在起來");
{
  const p = await open({
    config: { ...BASE, brain_command: "claude" },
    cloud: true,
    watching: "booting",
  });
  check("就是那一句", p.brainSay() === SENTENCE.readyBooting, p.brainSay());
  check(
    "不是「按開始記錄」那句（那顆按鈕這時候按下去會說已經有人在跑）",
    !p.brainSay().includes("等你按下「開始記錄」"),
    p.brainSay(),
  );
}

/*
 * `recording_state` 回**四**個字串，而這一頁的白名單只收前三個的話，
 * `"thinking"` 會掉進 `"none"` ——於是收工那兩分鐘裡這一格說「等你按下
 * 『開始記錄』」，而那顆按鈕這時候按下去只會回一句「還在想最後一段」。
 * 和 ⑯ 守 booting 的理由一模一樣，只是換一個狀態。
 */
console.log("⑯ᵇ 大腦：兩個都齊，上一場剛停、腦還在想最後一段");
{
  const p = await open({
    config: { ...BASE, brain_command: "claude" },
    cloud: true,
    watching: "thinking",
  });
  check("就是那一句", p.brainSay() === SENTENCE.readyThinking, p.brainSay());
  check(
    "沒有掉回「沒有人在錄」那句",
    p.brainSay() !== SENTENCE.readyIdle,
    p.brainSay(),
  );
  check(
    "不是「按開始記錄」那句（這時候按下去會被擋）",
    !p.brainSay().includes("等你按下「開始記錄」"),
    p.brainSay(),
  );
}

console.log("⑯ᶜ 存檔回條：上一場剛停時不可以叫他按一顆會被擋的開始鍵");
{
  const p = await open({ watching: "thinking" });
  await p.save();
  check("存檔回條說得出還在收尾", p.say().includes("還在把最後一段想完"), p.say());
  check("沒有叫他按開始記錄", !p.say().includes("等你按下「開始記錄」"), p.say());
}

console.log("⑯ᵈ 沒存的命令不可以改寫現在的出境狀態");
{
  const p = await open({
    config: { ...BASE, brain_command: "claude" },
    cloud: true,
    watching: "recording",
  });
  check("開場照已存檔值說正在錄", p.brainSay() === SENTENCE.live, p.brainSay());
  p.node("[data-brain-command]").value = "";
  for (const fn of p.node("[data-brain-command]").handlers.input ?? []) fn();
  check("清空但沒存時只說這是未存改動", p.brainSay().includes("這是還沒存的改動，按下儲存才算數"), p.brainSay());
  check("不可以宣布解釋層一次都不會醒", !p.brainSay().includes("一次都不會醒"), p.brainSay());
  check("確實沒有送 settings_write", p.writes.length === 0, p.writes);
}

{
  const four = [
    SENTENCE.noCommand,
    SENTENCE.noConsent,
    SENTENCE.readyIdle,
    SENTENCE.live,
  ];
  const unique = new Set(four);
  check("C 的四句話沒有兩句一樣", unique.size === 4, four);
  // booting 和 thinking 都是「兩個條件都齊、但現在不會醒」的變體，最容易
  // 被寫成同一句——而它們的下一步不一樣（一個等一下就好，一個要等她想完）。
  const all = [...four, SENTENCE.readyBooting, SENTENCE.readyThinking];
  check("連 booting、thinking 六句都沒有兩句一樣", new Set(all).size === 6, all);
}

console.log("⑰ 打字當下要標成未存改動——而且不可以把同意書那句警告一起吞掉");
{
  const p = await open({ cloud: false, watching: "none" });
  check("開場是沒填命令", p.brainSay() === SENTENCE.noCommand, p.brainSay());
  p.node("[data-brain-command]").value = "claude";
  for (const fn of p.node("[data-brain-command]").handlers.input ?? []) fn();
  check("立刻說這是未存改動", p.brainSay().includes("還沒存的改動"), p.brainSay());
  // 同意書勾了沒是**磁碟上的事實**，跟這個框無關。一句「還沒存」替代掉一句
  // 警告不叫少講一句——他存下去之後，擋住他的就是它。
  check(
    "同意書那句警告還在",
    p.brainSay().includes("第二張同意書還沒勾"),
    p.brainSay(),
  );
  check(
    "而且是紅的",
    p.node("[data-brain-say]").classList.contains("bad"),
    p.brainSay(),
  );
}

console.log("⑰ᵇ 勾了同意書的時候，未存改動不用扛那句警告");
{
  const p = await open({ cloud: true, watching: "none" });
  p.node("[data-brain-command]").value = "claude";
  for (const fn of p.node("[data-brain-command]").handlers.input ?? []) fn();
  check(
    "只說未存，不無中生有一句同意書警告",
    p.brainSay() === "這是還沒存的改動，按下儲存才算數。",
    p.brainSay(),
  );
}

console.log("⑱ 儲存會送出命令和參數，空白會剪掉");
{
  const p = await open();
  p.node("[data-brain-command]").value = "  gemini  ";
  p.node("[data-brain-args]").value = "-m\n\nflash\n";
  await p.save();
  const sent = p.writes[0];
  check("送了剪過空白的命令", sent.brain_command === "gemini", sent.brain_command);
  check(
    "參數一行一個、空行丟掉",
    JSON.stringify(sent.brain_args) === JSON.stringify(["-m", "flash"]),
    sent.brain_args,
  );
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
  check("讀回 Aster 的穩定 ID", p.node("[data-persona-id]").value === "chatgpt", p.node("[data-persona-id]").value);
  check("讀回角色開關", p.node("[data-persona-enabled]").checked === true);
  p.node("[data-persona-id]").value = "grok";
  p.node("[data-persona-motion]").checked = false;
  p.node("[data-persona-tap-lines]").checked = false;
  p.node("[data-persona-enabled]").checked = false;
  for (const fn of p.node("[data-persona-enabled]").handlers.change ?? []) fn();
  check("關掉只灰掉選擇、值仍是 Rook", p.node("[data-persona-id]").disabled && p.node("[data-persona-id]").value === "grok");
  await p.save();
  const sent = p.writes[0];
  check("送出關閉狀態", sent.persona_enabled === false, sent);
  check("關掉仍保留 Rook", sent.persona_id === "grok", sent);
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

console.log("㉑ 素材下載前先把 exact host／path／bytes／資料邊界攤開");
{
  const p = await open();
  const compactHtml = HTML.replace(/\s+/g, "");
  check("available 會顯示下載鍵", p.node("[data-persona-download]").hidden === false);
  check("揭露區不是藏著", p.node("[data-persona-disclosure]").hidden === false);
  check("host 一字不改", p.node("[data-persona-host]").textContent === ASSET_DISCLOSURE.host);
  check("固定 path 一字不改", p.node("[data-persona-pack-path]").textContent === ASSET_DISCLOSURE.path);
  check(
    "完整 byte 數看得到，不只是一個四捨五入 MB",
    p.node("[data-persona-size]").textContent.includes("123,456,789 bytes"),
    p.node("[data-persona-size]").textContent,
  );
  check(
    "後端回的資料邊界一字不改",
    p.node("[data-persona-boundary]").textContent === ASSET_DISCLOSURE.boundary,
    p.node("[data-persona-boundary]").textContent,
  );
  check(
    "固定文案明講 CDN metadata 與不能帶出的私人資料",
    compactHtml.includes("來源IP、時間、TLS") &&
      compactHtml.includes("角色選擇") &&
      compactHtml.includes("OCR") &&
      compactHtml.includes("問題、答案、記憶ID或資料庫內容") &&
      compactHtml.includes("四位角色的method、URL、headers與body完全相同") &&
      compactHtml.includes("至多一個HTTPSGET") &&
      compactHtml.includes("不retry、不先HEAD"),
    "settings.html disclosure",
  );
  check("開場只讀 status，沒有自己下載", calls(p, "persona_asset_install").length === 0, p.invokes);
  check("status 是零參數 IPC", calls(p, "persona_asset_status").every((c) => c.arg === undefined), calls(p, "persona_asset_status"));
  check("語音出廠畫成關閉", p.node("[data-persona-voice]").checked === false);
}

console.log("㉒ 下載只有 trusted click 能開始，而且 IPC 不帶 persona 或私人資料");
{
  const p = await open();
  await p.act("[data-persona-download]", { trusted: false });
  check("script 送的假 click 不下載", calls(p, "persona_asset_install").length === 0, p.invokes);
  await p.act("[data-persona-download]");
  const installs = calls(p, "persona_asset_install");
  check("真人 click 恰好下載一次", installs.length === 1, installs);
  check("install 是零參數，角色選擇不可能混進 request", installs[0]?.arg === undefined, installs[0]);
  check("完成後重讀成 installed", p.node("[data-persona-asset-summary]").textContent.includes("已安裝並驗證"), p.node("[data-persona-asset-summary]").textContent);
}

console.log("㉑ᵇ cache 路徑問不到不能假裝是尚未下載");
{
  const p = await open({
    asset: {
      ...ASSET_AVAILABLE,
      phase: "unavailable",
    },
  });
  const summary = p.node("[data-persona-asset-summary]").textContent;
  check("明說是 cache 路徑問不出來", summary.includes("問不出預設素材 cache 路徑"), summary);
  check("不顯示下載鍵", p.node("[data-persona-download]").hidden === true);
  check("不顯示修復或刪除鍵", p.node("[data-persona-repair]").hidden && p.node("[data-persona-remove]").hidden);
  check("開場沒有產生 GET 入口", calls(p, "persona_asset_install").length === 0, p.invokes);
}

console.log("㉑ᶜ 素材 status 失敗或陌生時保持 unknown，不宣稱正在用 fallback 或一定不會播放");
for (const [label, onAssetStatus] of [
  ["讀取失敗", () => { throw new Error("cache 暫時讀不到"); }],
  ["陌生 phase", (status) => ({ ...status, phase: "future-pack-state" })],
]) {
  const p = await open({ voice: true, onAssetStatus });
  const summary = p.node("[data-persona-asset-summary]").textContent;
  const voice = p.node("[data-persona-voice-state]").textContent;
  check(`${label}不冒充字母 fallback 已套用`, !summary.includes("角色仍使用內建字母人"), summary);
  check(`${label}明講這一頁不會下載`, summary.includes("不會啟動下載"), summary);
  check(`${label}不把未知素材折成一定不播放`, voice.includes("尚未確認") && !voice.includes("所以不會播放"), voice);
  check(`${label}時語音開關 fail closed`, p.node("[data-persona-voice]").disabled === true);
  check(`${label}沒有 GET 入口`, calls(p, "persona_asset_install").length === 0, p.invokes);
}

console.log("㉓ persona 選擇只改本機表單，不會觸發素材下載");
{
  const p = await open();
  const statusReads = calls(p, "persona_asset_status").length;
  p.node("[data-persona-id]").value = "grok";
  for (const fn of p.node("[data-persona-id]").handlers.change ?? []) fn({ isTrusted: true });
  await tick();
  check("Rook tagline 會換", p.node("[data-persona-tagline]").textContent.includes("深棕與淡黃"), p.node("[data-persona-tagline]").textContent);
  check("沒有下載", calls(p, "persona_asset_install").length === 0, p.invokes);
  check("連下載 status 都沒有因 persona 另打一份", calls(p, "persona_asset_status").length === statusReads, p.invokes);
}

console.log("㉔ installing 是不確定進度，且取消也是 trusted、零參數");
{
  let settleInstall = null;
  const p = await open({
    onAssetInstall: () =>
      new Promise((resolveInstall) => {
        settleInstall = resolveInstall;
      }),
    onAssetCancel: (_status, store) => {
      store({ ...ASSET_AVAILABLE, disclosure: { ...ASSET_DISCLOSURE } });
      settleInstall?.();
      return null;
    },
  });
  await p.act("[data-persona-download]");
  check("等待整包完成時畫 installing", p.node("[data-persona-asset-summary]").textContent.includes("正在下載並驗證"), p.node("[data-persona-asset-summary]").textContent);
  check("不確定 progress 看得到", p.node("[data-persona-progress]").hidden === false);
  const progressTag = HTML.match(/<progress[\s\S]*?<\/progress>/)?.[0] ?? "";
  check("沒有虛構 chunk 百分比", !/\bvalue\s*=|\bmax\s*=/.test(progressTag), progressTag);
  check("下載中只有取消鍵是這條路的出口", p.node("[data-persona-cancel]").hidden === false);
  await p.act("[data-persona-cancel]", { trusted: false });
  check("假 click 不取消", calls(p, "persona_asset_cancel").length === 0);
  await p.act("[data-persona-cancel]");
  const cancels = calls(p, "persona_asset_cancel");
  check("真人 click 取消一次", cancels.length === 1, cancels);
  check("cancel 是零參數", cancels[0]?.arg === undefined, cancels[0]);
  check("取消後回到字母 fallback", p.node("[data-persona-asset-summary]").textContent.includes("目前使用內建字母人"), p.node("[data-persona-asset-summary]").textContent);
}

console.log("㉕ repair-needed 用同一個受揭露保護的 install contract");
{
  const p = await open({
    asset: {
      ...ASSET_AVAILABLE,
      phase: "repair-needed",
      asset_file_bytes: 42,
    },
  });
  check("修復鍵看得到", p.node("[data-persona-repair]").hidden === false);
  check("壞素材不冒充立繪可用", p.node("[data-persona-asset-summary]").textContent.includes("立繪與語音已停用"), p.node("[data-persona-asset-summary]").textContent);
  await p.act("[data-persona-repair]");
  const installs = calls(p, "persona_asset_install");
  check("修復沿用 install 一次", installs.length === 1, installs);
  check("修復同樣不送參數", installs[0]?.arg === undefined, installs[0]);
}

console.log("㉖ 揭露缺一格就 fail closed，不會讓下載按得下去");
{
  const p = await open({
    asset: {
      ...ASSET_AVAILABLE,
      disclosure: { ...ASSET_DISCLOSURE, bytes: null },
    },
  });
  check("仍把缺的那格畫成沒有回報", p.node("[data-persona-size]").textContent === "沒有回報", p.node("[data-persona-size]").textContent);
  check("下載鍵是灰的", p.node("[data-persona-download]").disabled === true);
  check("明講是大小缺失，不是假裝網路壞", p.node("[data-persona-asset-error]").textContent.includes("缺少大小"), p.node("[data-persona-asset-error]").textContent);
  check("按不到也沒有 IPC", (await p.act("[data-persona-download]")) === false && calls(p, "persona_asset_install").length === 0, p.invokes);
}

console.log("㉗ removing 是正式 fail-closed 態；完成後回字母人");
{
  let finishRemove = null;
  const installed = {
    phase: "installed",
    disclosure: { ...ASSET_DISCLOSURE },
    asset_file_bytes: 100000000,
    portrait_count: 4,
    voice_count: 8,
  };
  const p = await open({
    asset: installed,
    voice: true,
    onAssetRemove: (_status, store) =>
      new Promise((resolveRemove) => {
        finishRemove = () => {
          store({ ...ASSET_AVAILABLE, disclosure: { ...ASSET_DISCLOSURE } });
          resolveRemove();
        };
      }),
  });
  check("已安裝時可刪除", p.node("[data-persona-remove]").hidden === false);
  await p.act("[data-persona-remove]");
  check("刪除一開始就畫 removing", p.node("[data-persona-asset-summary]").textContent.includes("正在刪除本機素材"), p.node("[data-persona-asset-summary]").textContent);
  check("刪除中明講立繪與語音已停用", p.node("[data-persona-asset-summary]").textContent.includes("立繪與語音已停用"), p.node("[data-persona-asset-summary]").textContent);
  check("刪除中也只有不確定進度", p.node("[data-persona-progress]").hidden === false);
  check("刪除中語音不可再操作", p.node("[data-persona-voice]").disabled === true);
  const removes = calls(p, "persona_asset_remove");
  check("remove 是零參數", removes.length === 1 && removes[0].arg === undefined, removes);
  finishRemove();
  await tick();
  check("完成後回 available 字母人", p.node("[data-persona-asset-summary]").textContent.includes("目前使用內建字母人"), p.node("[data-persona-asset-summary]").textContent);
}

console.log("㉘ config 讀壞不會連素材 status／remove 的出口一起關掉");
{
  const p = await open({
    onRead: () => {
      throw new Error("config.toml 壞了");
    },
    asset: {
      phase: "installed",
      disclosure: { ...ASSET_DISCLOSURE },
      asset_file_bytes: 100000000,
      portrait_count: 4,
      voice_count: 8,
    },
  });
  check("一般儲存確實因 config 壞掉而關閉", p.node("[data-save]").disabled === true);
  check("asset status 仍獨立讀到了", calls(p, "persona_asset_status").length === 1, p.invokes);
  check("刪除素材沒有跟著灰掉", p.node("[data-persona-remove]").disabled === false);
  await p.act("[data-persona-remove]");
  check("config 壞掉仍送得出 remove", calls(p, "persona_asset_remove").length === 1, p.invokes);
  check("語音設定不冒充可讀可寫", p.node("[data-persona-voice]").disabled === true);
}

{
  const p = await open({
    onRead: () => {
      throw new Error("config.toml 壞了");
    },
    asset: {
      phase: "installed",
      disclosure: { ...ASSET_DISCLOSURE },
      asset_file_bytes: 100000000,
      portrait_count: 4,
      voice_count: 8,
    },
    onAssetRemove: (_status, store) => {
      store({ ...ASSET_AVAILABLE, disclosure: { ...ASSET_DISCLOSURE } });
      throw new Error("本機素材已刪除，但聲音偏好存不回設定檔");
    },
  });
  await p.act("[data-persona-remove]");
  const line = p.node("[data-persona-asset-error]").textContent;
  check("cache 刪完但 config 寫壞時不會反過來宣稱刪除失敗", line.includes("本機素材現在已移除") && !line.startsWith("刪除失敗"), line);
  check("部分成功的真正錯誤仍完整可見", line.includes("聲音偏好存不回設定檔"), line);
}

console.log("㉙ persona-assets-changed 只重讀真相，不會自行下載");
{
  const p = await open();
  const before = calls(p, "persona_asset_status").length;
  p.setAsset({
    phase: "installed",
    disclosure: { ...ASSET_DISCLOSURE },
    asset_file_bytes: 100000000,
    portrait_count: 4,
    voice_count: 8,
  });
  await p.emit("persona-assets-changed");
  check("事件後多讀一次 status", calls(p, "persona_asset_status").length === before + 1, p.invokes);
  check("畫面換成 installed", p.node("[data-persona-asset-summary]").textContent.includes("4 張立繪、8 句"), p.node("[data-persona-asset-summary]").textContent);
  check("事件本身不下載", calls(p, "persona_asset_install").length === 0, p.invokes);
}

console.log("㉚ 聲音是另外一次 trusted opt-in，失敗會退回，設定頁永遠不播放");
{
  const installed = {
    phase: "installed",
    disclosure: { ...ASSET_DISCLOSURE },
    asset_file_bytes: 100000000,
    portrait_count: 4,
    voice_count: 8,
  };
  const p = await open({ asset: installed });
  check("pack 安裝不會順便打開聲音", p.node("[data-persona-voice]").checked === false);
  check("已安裝才讓 voice opt-in 可按", p.node("[data-persona-voice]").disabled === false);
  p.node("[data-persona-voice]").checked = true;
  await p.act("[data-persona-voice]", { trusted: false, event: "change" });
  check("假 change 被退回關閉", p.node("[data-persona-voice]").checked === false);
  check("假 change 沒有寫設定", calls(p, "persona_voice_set").length === 0);
  p.node("[data-persona-voice]").checked = true;
  await p.act("[data-persona-voice]", { event: "change" });
  const voiceSets = calls(p, "persona_voice_set");
  check("真人 opt-in 立刻寫一次，不等頁尾 Save", voiceSets.length === 1 && p.writes.length === 0, { voiceSets, writes: p.writes });
  check("voice IPC 只送 enabled bool", JSON.stringify(voiceSets[0]?.arg) === JSON.stringify({ enabled: true }), voiceSets[0]);
  check("成功後明講只在按角色時播固定台詞", p.node("[data-persona-voice-state]").textContent.includes("只會在你明確按角色時播放預錄固定台詞"), p.node("[data-persona-voice-state]").textContent);
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

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——設定頁在某一種情況下說了謊，或什麼都沒說。`);
  process.exit(1);
}
console.log("✓ 設定頁：成功和失敗都說得出話，而失敗不會偷偷刪掉他的規則");
