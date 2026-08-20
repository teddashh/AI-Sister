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
};

const tick = () => new Promise((r) => setTimeout(r, 20));

async function open({ config = BASE, onRead, onWrite, onHotkeySet, hotkey = HOTKEY } = {}) {
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
  const writes = [];
  globalThis.__TAURI__ = {
    core: {
      invoke: async (cmd, arg) => {
        switch (cmd) {
          case "settings_read":
            if (onRead) return onRead(state);
            return { ...state };
          case "settings_write":
            writes.push(arg.settings);
            if (onWrite) return onWrite(arg.settings, (s) => (state = { ...state, ...s }));
            state = { ...state, ...arg.settings };
            return { watching: "recording" };
          case "lint_url_rules":
            return [];
          case "privacy_health":
            return { url_rules: { kind: "working", reads: 12 }, at: 1_755_000_000_000 };
          case "hotkey_state":
            return hotkey;
          case "hotkey_set":
            if (onHotkeySet) return onHotkeySet(arg.combo);
            return { ...hotkey, wanted: arg.combo, registered: true };
          default:
            return null;
        }
      },
    },
  };

  const nonsense = watchNonsense();
  await boot();
  await tick();
  return {
    node,
    writes,
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
    combo: () => node("[data-combo]").textContent,
    hotkeySay: () => node("[data-hotkey-say]").textContent,
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
    onWrite: (s, commit) => {
      commit(s);
      return { watching: "idle" };
    },
  });
  await p.save();
  check("留著「等你按下『開始記錄』才會生效」", p.say().includes("開始記錄"), p.say());
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

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——設定頁在某一種情況下說了謊，或什麼都沒說。`);
  process.exit(1);
}
console.log("✓ 設定頁：成功和失敗都說得出話，而失敗不會偷偷刪掉他的規則");
