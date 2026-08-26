// 設定頁。和字母人一樣沒有打包步驟——這個檔案就是瀏覽器讀到的那個檔案。

const invoke = globalThis.__TAURI__?.core?.invoke ?? null;

const el = {
  path: document.querySelector("[data-path]"),
  brainCommand: document.querySelector("[data-brain-command]"),
  brainArgs: document.querySelector("[data-brain-args]"),
  brainSay: document.querySelector("[data-brain-say]"),
  apps: document.querySelector("[data-apps]"),
  urls: document.querySelector("[data-urls]"),
  titles: document.querySelector("[data-titles]"),
  screenshare: document.querySelector("[data-screenshare]"),
  redact: document.querySelector("[data-redact]"),
  querylog: document.querySelector("[data-querylog]"),
  framesDays: document.querySelector("[data-frames-days]"),
  textDays: document.querySelector("[data-text-days]"),
  lint: document.querySelector("[data-lint]"),
  health: document.querySelector("[data-health]"),
  captureOff: document.querySelector("[data-capture-off]"),
  machine: document.querySelector("[data-machine]"),
  unreadable: document.querySelector("[data-unreadable]"),
  combo: document.querySelector("[data-combo]"),
  clearCombo: document.querySelector("[data-clear]"),
  hotkeySay: document.querySelector("[data-hotkey-say]"),
  say: document.querySelector("[data-say]"),
  save: document.querySelector("[data-save]"),
  reload: document.querySelector("[data-reload]"),
};

function say(message, bad = false) {
  el.say.textContent = message;
  el.say.classList.toggle("bad", bad);
}

/**
 * 一行一條，空行丟掉，前後空白剪掉。
 *
 * 剪空白不是為了整齊：`" keepassxc"` 這條規則永遠不會命中任何 app 名稱，
 * 而使用者看著清單上那一行，會以為它擋住了。這是這一頁想擋掉的整類錯誤裡
 * 最不起眼、也最容易犯的一種。
 */
function toLines(text) {
  return text
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => s !== "");
}

// ---------- 規則檢查 ----------

/**
 * 網址規則的即時檢查。判斷本身在 Rust 那邊（`suspicious_url_rules`），
 * 和 `sister doctor`、`record` 用的是**同一份**——三個地方各寫一次判斷，
 * 遲早會變成三個不一樣的答案。
 */
async function relint() {
  const rules = toLines(el.urls.value);
  await refreshHealth(rules);
  if (invoke === null || rules.length === 0) {
    el.lint.hidden = true;
    return;
  }
  try {
    paintLint(await invoke("lint_url_rules", { rules }));
  } catch {
    // 檢查器掛了不該擋住編輯。它是加分項，不是關卡。
    el.lint.hidden = true;
  }
}

function paintLint(bad) {
  el.lint.replaceChildren();
  for (const [rule, why] of bad) {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.className = "rule";
    name.textContent = rule;
    const reason = document.createElement("span");
    reason.className = "why";
    reason.textContent = why;
    li.append(name, reason);
    el.lint.append(li);
  }
  el.lint.hidden = bad.length === 0;
}

let lintTimer = null;
el.urls?.addEventListener("input", () => {
  // 每個按鍵都打一次 IPC 太吵；等他停手再說。
  clearTimeout(lintTimer);
  lintTimer = setTimeout(() => void relint(), 250);
});

/**
 * 整組規則到底生不生效。
 *
 * `relint()` 檢查的是**某一條**寫錯了；這一條檢查的是這台機器讀不到瀏覽器
 * 網址，於是**一條都不生效**——而那在每一條規則都寫得完美的時候照樣成立。
 * 以前唯一知道這件事的人是 recorder，它把那句話印進 `record.log`；使用者是
 * 在這一頁上打那些規則的，那句話該出現在這裡。
 *
 * 判斷在 Rust 那邊（`capabilities::Report::broken_privacy_rules`），和
 * `sister doctor`、`record` 用的是同一份。
 */
async function refreshHealth(urls) {
  if (invoke === null || !el.health) return;
  try {
    paintHealth(await invoke("privacy_health", { urls }), urls.length > 0);
  } catch {
    paintHealthUnaskable(urls.length > 0);
  }
}

/**
 * 「問不到」不可以印成空白。
 *
 * `paintHealth` 每一條路都會講一句話，所以空白在這一格只剩一個意思：**你一條
 * 規則都沒寫**，沒有那個問題要回答。他明明寫了、我們只是問不到答案的時候印
 * 空白，等於替一個沒問出口的問題填一個「沒事」。以前這裡是 `hidden = true`。
 */
function paintHealthUnaskable(hasRules) {
  if (!el.health) return;
  el.health.classList.add("unknown");
  el.health.textContent =
    "問不出這幾條規則會不會生效（設定或能力報告讀不出來）。" +
    "底下那幾條可能一條都沒在擋——sister doctor 問得到同一份答案。";
  el.health.hidden = !hasRules;
  // 問不到就什麼都不知道，包括總開關和輸入 hook。留著上一次的答案會讓那兩格
  // 變成「現在的狀況」，而它們可能是十分鐘前的。
  if (el.captureOff) el.captureOff.hidden = true;
  if (el.machine) el.machine.hidden = true;
}

/**
 * 總開關關著：這一整頁在描述一台不存在的機器。
 *
 * `capture.enabled = false` 的時候底下每一格照樣說「這幾條規則會生效」——而
 * 那句話沒有意義，因為沒有任何畫面進來。它和一台一切正常的機器在畫面上長得
 * 一模一樣。`sister doctor` 早就把這件事當成頭條（「看起來正常，但其實記不住
 * 東西」），而這一頁才是他真正在改設定的地方。
 */
function paintCaptureOff(off) {
  if (!el.captureOff) return;
  el.captureOff.classList.toggle("bad", off);
  el.captureOff.textContent = off
    ? "設定檔裡 [capture] enabled = false：她連開始都不會開始。" +
      "底下這幾條規則現在一條都不會被用到——不是因為壞掉，是因為根本沒有畫面進來。" +
      "把那一行改成 true（或刪掉）她才會真的錄。"
    : "";
  el.captureOff.hidden = !off;
}

/**
 * 不屬於底下任何一格的能力缺口。目前只有輸入 hook。
 *
 * 它以前和網址規則那幾句話擠在同一格裡，而那一格就在網址輸入框正下方——
 * 一個 hook 裝不上的人，讀到的是「我有一條網址規則寫壞了」。
 */
function paintMachine(lines) {
  if (!el.machine) return;
  el.machine.classList.toggle("bad", lines.length > 0);
  el.machine.textContent = lines.join("\n");
  el.machine.hidden = lines.length === 0;
}

function paintHealth(health, hasRules) {
  if (!el.health) return;
  // 先把不屬於這一格的兩件事送去它們該去的地方。**在 `at === null` 那條早退
  // 路徑之前**：一台從沒錄過的機器，總開關可能就是關著的，而那正是他最該
  // 知道的時候——他還在設定，還沒按下開始。
  paintCaptureOff(health.capture_off === true);
  paintMachine(
    (health.broken ?? [])
      .filter((b) => b.about !== "url_rules")
      .map((b) => b.message),
  );
  const urlRules = (health.broken ?? []).filter((b) => b.about === "url_rules");
  // **三種狀態，不是兩種。** 「沒有報告」和「都生效」以前會長得一樣，而那
  // 正是這一格要修的那種安靜——一個從沒錄過的人看到一片乾淨，會以為門關好了。
  //
  // 一條規則都還沒寫的時候不講「還不知道」：那句話問的是「你寫的這幾條會不會
  // 生效」，而他還沒寫。空輸入框底下掛一句警告只會變成背景雜訊。
  if (health.at === null || health.at === undefined) {
    el.health.classList.add("unknown");
    // 不寫死「還沒有人跑過記錄」。`capabilities::read` 對三件事都回 `None`
    // ——沒有這個檔、讀不出來、或內容不是我們寫的那個形狀——而它的註解說得
    // 很清楚：對讀的人來說那只是一句「還不知道」。還有第四種：字母人和
    // `sister record --data-dir X` 指到不同的資料夾，那台機器**天天在錄**，
    // 而「第一次開始記錄之後回來看這裡」那句話永遠不會成真。
    //
    // `.health` 是 `white-space: pre-line`，所以下面那個 `\n` 是真的斷行。
    // 需要它：不斷的話 `--data-dir` 會落在行尾被拆成「--」和「data-dir」，
    // 而這一頁的破折號全寫成「——」，於是行尾那個「--」讀起來像標點，整句
    // 裡唯一可行動的那一半就消失了。
    el.health.textContent =
      "還不知道這幾條會不會生效——這個資料夾裡沒有能力報告。\n" +
      "跑過一次 sister record 之後就會有。已經跑過卻還是這樣的話，" +
      "多半是它寫到別的 --data-dir 去了。";
    el.health.hidden = !hasRules;
    return;
  }
  el.health.classList.remove("unknown", "ok");
  // 「開始記錄的時候測到的」以前寫在這裡，而那句話本身就是那個 bug：這個檔案
  // 開機寫一次就凍住，於是 UIA 半路投降之後的那幾小時，這一頁拿著一份開機時的
  // 「一切正常」什麼都不說。現在 recorder 錄製途中每分鐘蓋一次，所以這個時戳
  // 講的是「這份報告描述的是哪一刻」——而它離現在多遠，讀的人自己判斷。
  //
  // **時戳掛在每一句上，包括好消息。** 以前只有失敗那一句帶時間，而底下 `when`
  // 那段註解自己論證過為什麼不行：「去年 8/19 測的和今天早上測的長得一模一
  // 樣，而中間隔著一整年的瀏覽器和 Windows 更新」。
  if (urlRules.length > 0) {
    el.health.textContent = `${urlRules.map((b) => b.message).join("\n")}（${when(health.at)} 的狀況）`;
    el.health.hidden = false;
    return;
  }

  // 到這裡 `broken` 裡沒有網址那一格的話——而那**不等於「都生效」**，儘管這一
  // 頁到 alpha.37 為止就是這樣印的。`broken_privacy_rules` 最後
  // 那一格要求 `browser_ticks >= 20` 才敢講話，門檻沒到就什麼都不 push；於是
  // 「UIA 真的讀得到網址」和「UIA 起得來但一次都沒讀到過」印同一片空白，而後
  // 者是 `capabilities.rs` 叫做「這一整條線最常見的壞法」的那一台——那個人
  // 現在就在這一頁上打 `*.bank.com.tw*`，然後去網銀。
  const verdict = health.url_rules ?? { kind: "none" };
  if (verdict.kind === "unproven") {
    el.health.classList.add("unknown");
    el.health.textContent =
      `還不知道這幾條會不會生效：上一場在瀏覽器視窗上只停了 ${verdict.ticks} 拍` +
      `（要 ${verdict.need} 拍才問得出來），一個網址都還沒讀到過。\n` +
      `多用一下瀏覽器再回來看這裡。（${when(health.at)} 的狀況）`;
    el.health.hidden = !hasRules;
    return;
  }
  if (verdict.kind === "working") {
    el.health.classList.add("ok");
    el.health.textContent =
      `這幾條生效中：上一場讀到 ${verdict.reads} 個網址，` +
      `所以規則有東西可以比對。（${when(health.at)} 的狀況）`;
    el.health.hidden = false;
    return;
  }
  // `none`（一條規則都沒寫）。這一格問的是「你寫的這幾條會不會生效」，沒寫
  // 就沒有那個問題——空輸入框底下掛一句話只會變成背景雜訊。
  el.health.textContent = "";
  el.health.hidden = true;
}

/**
 * `MM-DD HH:MM`，跨年的話補上年份。
 *
 * 以前不印年份，理由是「年份對『上一次測是什麼時候』這句話沒有用」——但
 * `capabilities.rs` 在同一份 repo 裡寫的正好相反：一份三個禮拜前的報告和
 * 今天早上那份可信度不同，**不要替他決定「夠新了」**。去年 8/19 測的和
 * 今天早上測的長得一模一樣，而中間隔著一整年的瀏覽器和 Windows 更新。
 */
function when(ts) {
  const d = new Date(ts);
  const two = (n) => String(n).padStart(2, "0");
  const stamp = `${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}`;
  return d.getFullYear() === new Date().getFullYear()
    ? stamp
    : `${d.getFullYear()}-${stamp}`;
}

// ---------- 暫停熱鍵 ----------

/**
 * 瀏覽器的按鍵事件 → Tauri 那邊看得懂的字串。
 *
 * 巧的是不用查表：`KeyboardEvent.code`（`KeyP`、`Digit1`、`Space`、`ArrowUp`、
 * `F5`）和 `global-hotkey` 的解析器收的字**一模一樣**。自己寫一張對照表的話，
 * 它遲早會跟那個 crate 走散——而症狀是某一顆鍵存進去、下次開機註冊不起來。
 *
 * 兩條規則：只按修飾鍵不算（他還沒按完），**一個修飾鍵都沒有也不算**——一組
 * 綁在裸 `P` 上的全域熱鍵，會把整台機器上所有程式裡的 P 都吃掉。
 */
const MODIFIER_KEYS = new Set(["Control", "Alt", "Shift", "Meta"]);

function comboOf(e) {
  if (MODIFIER_KEYS.has(e.key)) return null;
  const parts = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");
  if (parts.length === 0) return null;
  parts.push(e.code);
  return parts.join("+");
}

/** `Ctrl+Alt+KeyP` → `Ctrl + Alt + P`。存的是前者，給人看的是後者。 */
function pretty(combo) {
  return combo
    .split("+")
    .map((t) => t.replace(/^Key(?=.$)/, "").replace(/^Digit(?=.$)/, ""))
    .join(" + ");
}

let capturing = false;

/**
 * 那一格上一次**確定是真的**寫的是什麼。
 *
 * 存在的理由是捕捉模式：按下那一格之後它變成「按下你要的那一組…」，而那句話
 * 是一句**承諾**——我在聽。只有 `paintHotkey` 有資格把它換掉。問題是通往
 * `paintHotkey` 的路會失敗，而失敗的那條路以前只寫底下那行說明、不碰這一格：
 * 於是它永遠停在「按下你要的那一組…」，而 `capturing` 已經是 false。他再怎麼
 * 按鍵盤都不會有事發生，畫面卻一直說在等他按。
 *
 * 更糟的是那一格是**唯一**寫著暫停鍵是哪一組的地方。卡在那句話上等於他連
 * 「現在按哪一顆會暫停」都問不到——而後端在那條路上已經把舊的那組裝回去了，
 * 答案是知道的，只是沒有人把它畫回去。
 */
let comboShown = null;

/** 把那一格退回上一次確定為真的內容。不知道就說不知道，不要留著那句承諾。 */
function restoreCombo() {
  capturing = false;
  el.combo.classList.remove("listening");
  el.combo.textContent = comboShown ?? "讀不出來";
}

function paintHotkey(view) {
  capturing = false;
  el.combo.classList.remove("listening");
  comboShown = view.wanted === "" ? "沒有設" : pretty(view.wanted);
  el.combo.textContent = comboShown;

  const bad =
    view.rejected != null ||
    view.config_unreadable != null ||
    (view.wanted !== "" && !view.registered);
  el.hotkeySay.classList.toggle("bad", bad);
  // **這一條排在最前面。** 開機讀不出設定檔的時候，底下每一句話講的都是
  // 內建預設值那一組——而它們全都是肯定句（「搶到了。現在按 Ctrl+Alt+P…」）。
  // 他設的是 Ctrl+Alt+S，按下去沒有反應，而這一頁指著另一組說沒問題。
  //
  // 對一顆暫停鍵來說那是最壞的一種壞法：他以為她停了，她還在錄。
  if (view.config_unreadable != null) {
    el.hotkeySay.textContent =
      `開機時讀不出設定檔，所以現在用的是內建預設的那一組${
        view.wanted === "" ? "" : `（${pretty(view.wanted)}）`
      }，不是你設的。` +
      `你設的那一組現在按下去不會有任何反應。修好設定檔再重開一次她：\n${view.config_unreadable}`;
    return;
  }
  // 剛剛試的那組被別人佔走了。後端已經把舊的那組裝回去了——講清楚「你試的
  // 那組沒成功、現在還在用哪一組」，不然他會以為暫停鍵從此不見了（以前**真的**
  // 會不見：`apply_hotkey` 先 `unregister_all()`，失敗就什麼都沒裝回去）。
  if (view.rejected != null) {
    const now = view.registered
      ? `還在用 ${pretty(view.wanted)}，那一組按得動。`
      : view.wanted === ""
        ? "熱鍵本來就是關掉的，維持原狀。"
        : `而舊的那組 ${pretty(view.wanted)} 現在也搶不到（${view.reason ?? "原因不明"}）——改用系統匣裡的暫停。`;
    el.hotkeySay.textContent = `${pretty(view.rejected)} 搶不到，多半是別的程式先拿走了。${now}`;
  } else if (view.wanted === "") {
    el.hotkeySay.textContent =
      "熱鍵是關掉的。暫停還在系統匣選單和字母人身上，只是要先找到她。";
  } else if (view.registered) {
    el.hotkeySay.textContent = `搶到了。現在在任何程式裡按 ${pretty(view.wanted)} 都會暫停或繼續。`;
  } else {
    // 這一句是這一格存在的理由：搶不到的時候要指名道姓，而不是讓他按了沒反應。
    el.hotkeySay.textContent = `這一組搶不到（${view.reason ?? "原因不明"}）。換一組，或改用系統匣裡的暫停。`;
  }
}

async function setCombo(combo) {
  if (invoke === null) return;
  try {
    paintHotkey(await invoke("hotkey_set", { combo }));
  } catch (err) {
    // **不要在這裡呼叫 `reloadHotkey()`。** 它成功的話會走 `paintHotkey`，
    // 而那件事會把底下這句錯誤蓋掉——換成一句肯定句（「搶到了。現在按…」）。
    // 那正是這一頁上剛修掉的那一族：每一行都是真的，湊起來在說謊。
    //
    // 要的只是把那一格退回去。後端在這條路上已經把舊的那組裝回去了
    // （見 `hotkey_set` 的 `persist()` 失敗分支），所以 `comboShown` 就是
    // 現在真的在生效的那一組。
    restoreCombo();
    el.hotkeySay.classList.add("bad");
    el.hotkeySay.textContent = String(err?.message ?? err);
  }
}

el.combo?.addEventListener("click", () => {
  capturing = true;
  el.combo.classList.add("listening");
  el.combo.textContent = "按下你要的那一組…";
  el.hotkeySay.classList.remove("bad");
  el.hotkeySay.textContent = "至少要有一個 Ctrl / Alt / Shift。Esc 取消。";
});

el.clearCombo?.addEventListener("click", () => void setCombo(""));

// 掛在 window 上而不是那顆按鈕上：捕捉模式底下要吃掉**所有**按鍵，不然
// Tab、Enter 這幾顆會先被瀏覽器拿去換焦點、按下另一顆按鈕。
globalThis.addEventListener(
  "keydown",
  (e) => {
    if (!capturing) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      capturing = false;
      void reloadHotkey();
      return;
    }
    const combo = comboOf(e);
    if (combo === null) return;
    // 先關掉再送。押著不放的話 keydown 會一直重複進來，而每一次都是一趟
    // 「拆掉舊的、重新註冊」——那是一個真的會在他手指底下發生的競爭。
    capturing = false;
    void setCombo(combo);
  },
  true,
);

async function reloadHotkey() {
  if (invoke === null) return;
  try {
    paintHotkey(await invoke("hotkey_state"));
  } catch (err) {
    // Esc 取消也走這裡（見上面那個 keydown）。那時候那一格上寫的是
    // 「按下你要的那一組…」，而這條路問不到現在是哪一組——退回上一次
    // 確定為真的那個，問不到就說「讀不出來」。留著那句承諾是最壞的：
    // 沒有人在聽，而畫面說在聽。
    restoreCombo();
    el.hotkeySay.classList.add("bad");
    el.hotkeySay.textContent = String(err?.message ?? err);
  }
}

// ---------- 讀 / 寫 ----------

/**
 * 這一頁攤開的時候，題庫那顆勾是勾著的嗎。
 *
 * 存的時候要拿它比一次：「剛剛關掉」和「本來就關著」對使用者是兩件事，而只有
 * 前者需要一句「先前記下的那些不會跟著消失」。存完 `load()` 會再蓋一次，所以
 * 下一次比的是新的基準。
 */
let queryLogWas = null;

/**
 * 第二張同意書現在算不算數。`true` / `false` / `null`（問不到）。
 *
 * `null` 不可以印成 `false`：那會把「她有 CLI 但你沒許可」講成一件我們沒問
 * 過的事。問的路是既有的 `consent_read`（onboarding / 時間軸同一條），看
 * `cloud-reading` 那張的 `effective`——簽過但條文改版了的，那一格就是 false。
 */
let cloudOk = null;

/**
 * 心跳現在說什麼：`"recording"`／`"booting"`／`"none"`。
 *
 * 和 `WriteOutcome.watching` 同一個判斷，不是另一個。存完用後端剛回的那一份；
 * 開場問 `recording_state`（同一顆 `heartbeat::phase`）。認不得的值走
 * `"none"`：三句裡只有它不會替一件沒發生的事背書。
 */
let watchingNow = "none";

function apply(s) {
  queryLogWas = s.query_log;
  el.path.textContent = s.path;
  if (el.brainCommand) el.brainCommand.value = s.brain_command ?? "";
  if (el.brainArgs) el.brainArgs.value = (s.brain_args ?? []).join("\n");
  el.apps.value = s.excluded_apps.join("\n");
  el.urls.value = s.excluded_urls.join("\n");
  el.titles.value = s.excluded_titles.join("\n");
  el.screenshare.checked = s.pause_on_screenshare;
  el.redact.checked = s.redact_clipboard_secrets;
  el.querylog.checked = s.query_log;
  el.framesDays.value = s.frames_days;
  el.textDays.value = s.text_days;
}

/**
 * 大腦這一格現在到底是開是關。四種，不是兩種。
 *
 * 「她沒有 CLI 可以叫」和「她有 CLI 但你沒許可」修法完全不同（一個去填框，
 * 一個去勾同意書）。印成同一句就是這個 repo 犯過四十次的那個錯。
 */
function paintBrain() {
  if (!el.brainSay) return;
  if (unreadable) {
    el.brainSay.textContent = "";
    el.brainSay.classList.remove("bad", "ok");
    el.brainSay.hidden = true;
    return;
  }
  el.brainSay.hidden = false;
  el.brainSay.classList.remove("bad", "ok");
  const cmd = (el.brainCommand?.value ?? "").trim();
  if (!cmd) {
    // 1. 沒填命令（不管同意書勾了沒）。
    el.brainSay.textContent =
      "還沒填命令：解釋層和審閱層一次都不會醒。空著就是關，不是跑得慢一點。";
    return;
  }
  if (cloudOk === null) {
    el.brainSay.textContent =
      "命令有了，但問不到第二張同意書勾了沒。在問得到之前，不能當成會送。同意書那一頁看得到同一份答案。";
    return;
  }
  if (cloudOk === false) {
    // 2. 填了命令，第二張沒勾。
    el.brainSay.classList.add("bad");
    el.brainSay.textContent =
      "命令有了，但第二張同意書還沒勾：螢幕上的字只留在這台機器，一次都不會交給這支 CLI。去「三張同意書」那一頁勾上雲解讀。";
    return;
  }
  // 兩個條件都成立。watching 那三個值決定她現在會不會醒——不要自己再發明一個判斷。
  if (watchingNow === "recording") {
    // 4. 兩個都成立而且正在錄。
    el.brainSay.classList.add("ok");
    el.brainSay.textContent = "命令和同意書都齊了，而且正在錄：她會自己醒。";
    return;
  }
  if (watchingNow === "booting") {
    el.brainSay.textContent =
      "命令和同意書都齊了。有一個 sister record 正在起來（多半在開資料庫）——它一開始錄，她就會自己醒，不必再按「開始記錄」。";
    return;
  }
  if (watchingNow === "thinking") {
    // 收工那兩分鐘：錄製迴圈已經停了，解釋層還在把最後一段想完
    // （`heartbeat::Presence::Thinking`）。她確實沒在錄——但底下那句
    // 「等你按下『開始記錄』」會指著一顆這時候按下去只會回一句
    // 「還在想最後一段，最多還要 N 秒」的按鈕。和 `WriteOutcome.watching`
    // 當初為了 booting 拆出第三個值是同一個理由。
    el.brainSay.textContent =
      "命令和同意書都齊了。上一場錄製剛停，解釋層還在把最後一段想完——想完才能再開一場，這時候按「開始記錄」會被擋下來。";
    return;
  }
  // 3. 填了命令、同意書也勾了，但現在沒有 record 在跑。
  el.brainSay.textContent =
    "命令和同意書都齊了。現在沒有人在錄，等你按下「開始記錄」她才會自己醒。";
}

async function refreshBrainFacts() {
  if (invoke === null) {
    cloudOk = null;
    watchingNow = "none";
    return;
  }
  try {
    // **不可以叫 `view`。** `check-combo-is-readable.py` 的白名單是正面表列：
    // `settings.js` 裡的 `view` 後面永遠要接一個 `.`。那條規則成立的前提是
    // 「這顆物件只活在 `paintHotkey` 裡」——熱鍵那一份，生的 accelerator
    // 不准走上畫面。這裡多一個同名的區域變數，會讓那道閘門在一段跟熱鍵
    // 完全無關的程式碼上翻紅（實測過，就是這兩行）。
    const consentView = await invoke("consent_read");
    const sheet = (consentView?.sheets ?? []).find(
      (s) => s.key === "cloud-reading",
    );
    cloudOk = sheet ? sheet.effective === true : null;
  } catch {
    cloudOk = null;
  }
  try {
    const w = await invoke("recording_state");
    // `recording_state` 回四個字串（見 main.rs 上面那段 doc）。少收一個的
    // 話它會掉進 `"none"`，而 `"none"` 那句寫著「等你按下『開始記錄』」——
    // 想最後一段的那兩分鐘按下去是會被擋的。
    watchingNow =
      w === "recording" || w === "booting" || w === "thinking" || w === "none"
        ? w
        : "none";
  } catch {
    watchingNow = "none";
  }
}

/**
 * 讀不出設定檔的時候，把整張表關掉。
 *
 * 不是為了防呆——`days()` 本來就會在空欄位上擋下儲存。是為了不說謊：一張
 * 空白的排除清單和兩顆沒打勾的防線，讀起來是一個明確的斷言（「你什麼都沒
 * 擋」），而它是假的。灰掉 + 換掉 placeholder 之後，那張表不再宣稱任何事。
 */
/**
 * 現在這張表是不是「不算數」的狀態。
 *
 * 存在的理由是 `save()` 的 `finally`：它無條件把儲存鍵點亮，而這正好會拆掉
 * `setUnreadable(true)` 剛做的事。那個 `finally` 本來是對的（按鈕在存的期間
 * 灰掉，存完不管成敗都要能再按），它只是不知道自己中間可能經過一個**清空
 * 整張表**的分支。
 */
let unreadable = false;

function setUnreadable(on) {
  unreadable = on;
  for (const node of [
    el.brainCommand,
    el.brainArgs,
    el.apps,
    el.urls,
    el.titles,
    el.screenshare,
    el.redact,
    el.querylog,
    el.framesDays,
    el.textDays,
    el.save,
  ]) {
    if (node) node.disabled = on;
  }
  for (const box of [el.brainCommand, el.brainArgs, el.apps, el.urls, el.titles]) {
    if (!box) continue;
    if (on) {
      box.value = "";
      box.placeholder =
        "讀不出設定檔——正在跑的記錄用的還是舊的那一份，這裡顯示不了它。";
    } else {
      box.placeholder = "";
    }
  }
  if (on) el.path.textContent = "讀不出來";
  if (el.unreadable) {
    // 這裡是 textContent，不是 markdown——寫 `**…**` 只會印出星號。
    el.unreadable.textContent =
      "讀不出設定檔，所以這一頁上的每一格都不算數：底下的空白和沒打勾，" +
      "不代表那些規則沒生效。正在跑的記錄用的是它上次讀成功的那一份，" +
      "排除規則和兩道防線都還在擋。修好底下那行錯誤再回來。";
    el.unreadable.hidden = !on;
  }
  // 命令框被清空之後，這一格會看起來像「她沒有 CLI 可以叫」。那是假的——
  // 正在跑的那一份我們讀不到。藏起來，讓上面那句「都不算數」說話。
  paintBrain();
}

/**
 * 讀一份新的畫上去。回傳「讀出來了沒」。
 *
 * 那個回傳值是給 `save()` 用的，而它是必要的：這個函式成功的時候會 `say("")`
 * ——那件事本身是對的（重新讀一份，就不該留著上一件事的結果掛在那裡），但
 * `save()` 存完會呼叫它，於是「存好了」講完幾毫秒就被抹成空白。兩行各自都對，
 * 湊起來這一頁在**成功**的時候永遠沉默，只有失敗才說話（`catch` 走不到這裡）。
 * 對按下按鈕的人來說，沉默就是「按了沒反應」。
 */
async function load() {
  if (invoke === null) {
    say("這一頁不是在 AI-Sister 裡打開的，改了不會存到任何地方。", true);
    return false;
  }
  let ok = false;
  try {
    apply(await invoke("settings_read"));
    setUnreadable(false);
    say("");
    await refreshBrainFacts();
    paintBrain();
    await relint();
    ok = true;
  } catch (err) {
    // `apply` 在 `await` 之後才跑，所以讀失敗的時候**一個欄位都沒被寫過**，
    // 畫面上留著 settings.html 的預設值：三個空的排除框、兩顆沒打勾的防線。
    // 而那正好是「什麼都沒擋、兩道防線都關了」長的樣子。
    //
    // 真正在跑的 recorder 這時候用的是舊的那一份（見 `Config::reload`），
    // 九條 app、十六條網址、兩道防線全部照常生效。畫面和事實完全相反，
    // 而唯一的線索是底下那條 bar 上的 TOML 錯誤。
    setUnreadable(true);
    say(String(err?.message ?? err), true);
  }
  // 熱鍵分開讀：它問的不是設定檔裡寫什麼，是**現在真的搶到了沒**——那個答案
  // 只有已經跑起來的那支程式知道。設定讀失敗也不該讓這一格空著。
  await reloadHotkey();
  return ok;
}

/**
 * 開發用：`settings.html?demo=1`，以及 `?demo=taken`（熱鍵被別的程式佔走）、
 * `?demo=off`（`capture.enabled = false`）、`?demo=nohotkey`（開機讀不出設定檔，
 * 於是熱鍵是內建預設值那一組，不是他設的）。
 *
 * 健康那一格自己有四張臉，**要並排看過**：`1`（橘色，整組規則不生效）、
 * `unknown`（灰，這個資料夾沒有能力報告）、`unproven`（灰，UIA 起得來但瀏覽器
 * 用得還不夠多，問不出來）、`working`（綠，上一場真的讀到過網址）。後面兩張
 * 在這之前一張都畫不出來——`broken` 是空的，這一格就整個藏起來，於是三台不同
 * 的機器共用同一片空白。分不開就等於沒修，所以這四張要一次看完。
 *
 * 和 app.js 的 `?state=` 同一個理由——這台開發機開不起 Tauri 視窗，而這一頁
 * 有一堆版面（八個區塊、警告清單、底下那條）需要真的看過。**它走的是和產品
 * 一樣的 `apply()`、`paintLint()`、`paintBrain()` 與 `paintHotkey()`**，不是另外
 * 畫一份長得像的出來：上一次讓開發開關走自己的路，截出來的圖就騙了我一次。
 *
 * 大腦那一格有四張臉，**要並排看過**：沒填命令、填了但第二張沒勾、兩個都齊
 * 但沒人在錄、兩個都齊而且正在錄。前兩張印成同一句就是這個 repo 犯過四十次
 * 的那個錯。`?demo=brain`（沒勾）、`?demo=brainready`（沒人在錄）、
 * `?demo=brainon`（正在錄）；沒帶這些的 demo 是沒填命令那一張。
 *
 * 熱鍵那一格有兩種畫面，而「搶不到」才是這一格存在的理由，所以它也要被看過。
 */
function demo(variant) {
  // 設定檔讀不出來的那一頁（`?demo=broken`）。這是最需要被眼睛看過的一種：
  // 它以前長得跟「你什麼都沒擋、兩道防線都關了」一模一樣。
  if (variant === "broken") {
    setUnreadable(true);
    say(
      "讀不出設定檔：retention.frames_days 不能是 0。0 在有些工具裡是「不限制」，但在這裡它的意思是「下一次整理就把畫面檔全部刪掉」。",
      true,
    );
    paintHealthUnaskable(true);
    return;
  }
  apply({
    path: "C:\\Users\\ted\\AppData\\Roaming\\ted-h\\AI-Sister\\config\\config.toml",
    excluded_apps: ["keepassxc", "1password", "bitwarden", "authy"],
    excluded_urls: ["*.bank.com.tw*", "https://mail.google.com/*", "*/admin/*"],
    excluded_titles: ["*password*", "*信用卡*"],
    pause_on_screenshare: true,
    redact_clipboard_secrets: true,
    query_log: true,
    frames_days: 30,
    text_days: 365,
    brain_command: "",
    brain_args: [],
  });
  // 第二條規則故意是壞的，這樣才看得到警告那一格長什麼樣。理由字串抄自
  // `suspicious_url_rules` 真正回傳的那一句。
  paintLint([
    [
      "https://mail.google.com/*",
      "規則比對的是網址本身，開頭帶 https:// 的話多數瀏覽器讀回來的字串對不上；去掉通訊協定寫成 mail.google.com/* 才會命中",
    ],
  ]);
  // 這一格是這一頁上最嚴重的一句話（整組排除規則不生效），所以 demo 版面
  // 一定要看得到它——版面沒被眼睛看過的警告，等於還沒寫。`?demo=unknown` 是
  // 另外那一半：一台還沒錄過的機器。那句話要**看起來明顯比較不吵**，不然
  // 「還不知道」會被當成「有問題」，而那一頁上真正的警告就貶值了。
  // `?demo=unproven` 和 `?demo=working` 是這一格的另外兩張臉，而它們在這之前
  // **同一張都畫不出來**：`broken` 是空的，這一格就整個藏起來。三台不同的機器
  // 一片相同的空白，其中一台把使用者的網銀錄了一整天。
  //
  // 這兩張要並排看過一次才算數：`unproven` 必須明顯比 `working` 不確定，而
  // 兩張都必須和上面那則真警告（橘色）分得開。三個都用灰的話，等於沒修。
  const verdicts = {
    unproven: { kind: "unproven", ticks: 6, need: 20 },
    working: { kind: "working", reads: 412 },
  };
  paintHealth(
    variant === "unknown"
      ? { broken: [], at: null, capture_off: false }
      : verdicts[variant]
        ? {
            broken: [],
            at: Date.now() - 40 * 60 * 1000,
            capture_off: false,
            url_rules: verdicts[variant],
          }
        : {
            broken: [
              {
                about: "url_rules",
                message:
                  "沒有 UIA 網址擷取：3 條 excluded_urls 規則（網銀、登入頁）目前不會生效，瀏覽器畫面只靠視窗標題規則過濾",
              },
              // 這一則故意混在同一份回答裡：它**不可以**出現在網址輸入框底下。
              // 那個位置以前讓一句和網址無關的話，看起來像在指著他剛打的三行。
              {
                about: "input_hook",
                message: "輸入 hook 裝不上：節奏訊號這個 session 會是空的",
              },
            ],
            at: Date.now() - 3 * 3600 * 1000,
            // 總開關那一句要被眼睛看過：它是這一頁上唯一一句「這一整頁都不算數」。
            capture_off: variant === "off",
          },
    true,
  );
  paintHotkey(
    variant === "taken"
      ? {
          wanted: "Ctrl+Alt+KeyP",
          registered: false,
          reason: "RegisterEventHotKey failed",
          config_unreadable: null,
        }
      : variant === "nohotkey"
        ? {
            // 開機讀不出設定檔 → 用的是內建預設值那一組，而他設的是別的。
            wanted: "Ctrl+Alt+KeyP",
            registered: true,
            reason: null,
            config_unreadable:
              "retention.frames_days 不能是 0：0 的意思是「下一次整理就把畫面檔全部刪掉」",
          }
        : {
            wanted: "Ctrl+Alt+KeyP",
            registered: true,
            reason: null,
            config_unreadable: null,
          },
  );
  // 大腦那一格的四張臉。預設是「沒填命令」——那是產品出廠的樣子（A/B 沒贏
  // 保持預設關）。另外三張要指名才畫得出來。
  if (variant === "brain") {
    if (el.brainCommand) el.brainCommand.value = "claude";
    if (el.brainArgs) el.brainArgs.value = "-p";
    cloudOk = false;
    watchingNow = "recording";
  } else if (variant === "brainready") {
    if (el.brainCommand) el.brainCommand.value = "claude";
    if (el.brainArgs) el.brainArgs.value = "-p";
    cloudOk = true;
    watchingNow = "none";
  } else if (variant === "brainon") {
    if (el.brainCommand) el.brainCommand.value = "claude";
    if (el.brainArgs) el.brainArgs.value = "-p";
    cloudOk = true;
    watchingNow = "recording";
  } else {
    cloudOk = false;
    watchingNow = "none";
  }
  paintBrain();
  say("這是 demo 版面，沒有讀任何真的設定。");
}

/**
 * 天數欄位。空的、負的、寫成「三十」的，都不可以變成 0——
 * 因為 0 的意思是**立刻刪**，而一個手滑清空的欄位不該把使用者一年的
 * 文字紀錄按下去。看不懂就退回目前的值，並且說出來。
 *
 * 這段註解本來就是這樣寫的，但守衛寫成 `n < 0`——也就是說 0 一路通過，
 * 存進設定檔，然後在下一次開始錄的時候把全部東西清掉。真正擋住的那一道
 * 在 `RetentionConfig::check`（存和讀都會過），這裡只是把話講在他還看得到
 * 那個輸入框的時候。
 */
function days(input, label) {
  const n = Number.parseInt(input.value, 10);
  if (!Number.isInteger(n) || n < 1) {
    throw new Error(
      `${label}的天數要是 1 以上的數字：「${input.value}」。` +
        `0 在這裡不是「不限制」，是「下一次整理就全部刪掉」——想留久一點請寫大一點（36500 大約是 100 年）。`,
    );
  }
  return n;
}

async function save() {
  if (invoke === null) return;
  el.save.disabled = true;
  try {
    const outcome = await invoke("settings_write", {
      settings: {
        excluded_apps: toLines(el.apps.value),
        excluded_urls: toLines(el.urls.value),
        excluded_titles: toLines(el.titles.value),
        pause_on_screenshare: el.screenshare.checked,
        redact_clipboard_secrets: el.redact.checked,
        query_log: el.querylog.checked,
        frames_days: days(el.framesDays, "畫面"),
        text_days: days(el.textDays, "文字"),
        brain_command: (el.brainCommand?.value ?? "").trim(),
        brain_args: toLines(el.brainArgs?.value ?? ""),
        // `path` 不送。要寫到哪個檔案由 Rust 那邊算，不是這一頁說了算。
      },
    });
    // 「存好了」還不夠。使用者要知道的是「她現在照這份跑了沒」——而那句話
    // 以前寫死是「正在跑的 record 會在 5 秒內換上這一份」，帶著一個沒被檢查
    // 的前提：**真的有一個 record 在跑**。一個剛裝好、還沒按過「開始記錄」的
    // 人，看到的是一句承諾一件不會發生的事的話；而一個以為她在錄、實際上那個
    // 行程幾分鐘前就掛了的人，看到的是一句替那件事背書的話。
    //
    // 三句都是真的，差別只在心跳說什麼——所以去問心跳，不要用猜的。
    //
    // **「正在起來」那一句是第三種，不是「沒有人在錄」的一種。** 上一版收的是
    // 一個布林（`is_recording`），於是開機那幾分鐘（`Db::open` 在一顆大資料庫
    // 上要跑好幾分鐘）這一頁把他送去按「開始記錄」——而那顆按鈕在那幾分鐘按
    // 下去只會回一句「已經有一個 sister record 在跑了」。他剛改完排除規則，
    // 而這一頁給了他一條走不通的路。
    //
    // 認不得的值走「沒有人在錄」那一句：三句裡只有它不會替一件沒發生的事背書。
    if (
      outcome?.watching === "recording" ||
      outcome?.watching === "booting" ||
      outcome?.watching === "none"
    ) {
      watchingNow = outcome.watching;
    } else {
      watchingNow = "none";
    }
    const watching =
      watchingNow === "recording"
        ? "存好了。正在跑的 record 會在 5 秒內換上這一份。"
        : watchingNow === "booting"
          ? "存好了。有一個 sister record 正在起來（多半在開資料庫）——它一開始錄就會換上這一份，不必再按「開始記錄」。"
          : "存好了——不過現在沒有人在錄，所以這一份要等你按下「開始記錄」才會生效。";
    // 把題庫關掉只擋**新的**問題。不講的話，「不要記下我問過的問題」讀起來像
    // 「那些問題沒了」——而 `queries` 是這整顆資料庫裡唯一一張存著**他自己打
    // 進去的字**的表（`settings.html` 那一格自己就這樣寫），所以它剛好是最容
    // 易被誤以為「我剛剛清掉了」的一張。
    //
    // 隔壁那一頁早就有一模一樣的句子：撤掉截圖同意書的時候，`onboarding.js`
    // 說「先前已經寫下的不會因為這個動作消失——要清掉請用時間軸的『忘掉這一
    // 段』，或跑 sister prune」。同一種動作、同一種誤解，這一頁少了那一句。
    //
    // **在 `load()` 之前算完**：它會把 `el.querylog.checked` 和 `queryLogWas`
    // 一起換成剛存進去的那一份，那之後這個比較永遠是 false。
    const justTurnedOff = queryLogWas === true && el.querylog.checked === false;
    const message = justTurnedOff
      ? `${watching}\n從現在起她不會再記你問過的問題。先前記下的那些不會因為這個動作消失——要清掉請用時間軸的「忘掉這一段」，或等文字保留期到。`
      : watching;
    // 存進去的是剪過空白、丟過空行的版本，畫面要跟著變成那個樣子，
    // 不然他看到的和檔案裡的是兩份東西。
    //
    // **先讀回來，再講話。** 反過來的話這句話活不過幾毫秒——`load()` 成功的
    // 路徑上有一行 `say("")`，而那一行是對的（見它的註解）。這一頁從出生到
    // alpha.37 為止，每一句「存好了」都是這樣被自己抹掉的：存壞了會說話，
    // 存好了什麼都不說，而對按下按鈕的人來說沉默就是「按了沒反應」。
    //
    // 讀不回來就**不要**蓋掉 `load()` 剛印上去的那則錯誤。寫進去了、再讀出來
    // 卻讀不出來，多半是我們剛剛寫壞了那個檔——那件事比「存好了」急，而且
    // 「存好了」在那個當下已經不是一句完整的真話。
    if (await load()) {
      say(message);
    }
  } catch (err) {
    say(String(err?.message ?? err), true);
  } finally {
    // **只有在這張表還算數的時候才把按鈕點回來。** 中間那個 `load()` 可能走
    // 進 `setUnreadable(true)`，而那件事會清空三個排除框並灰掉整張表。這一行
    // 無條件寫 `false` 的時候，畫面會變成「規則全空、儲存鍵亮著」——他再按
    // 一次，`[]` 就寫進 excluded_apps / urls / titles，九條 app、十六條網址
    // 靜靜消失。`days()` 攔不住：那兩個天數欄位沒被清掉，照樣是合法的數字。
    //
    // 兩行各自都對：按鈕存完要能再按，讀不回來要把表關掉。湊起來，這一頁的
    // 錯誤處理會刪掉他的隱私規則。
    //
    // 出路是那顆「重新讀取」——它不在停用名單裡，讀成功就會 `setUnreadable(false)`。
    if (!unreadable) el.save.disabled = false;
  }
}

el.save?.addEventListener("click", () => void save());
el.reload?.addEventListener("click", () => void load());
el.brainCommand?.addEventListener("input", () => paintBrain());

const variant = new URLSearchParams(globalThis.location.search).get("demo");
if (variant !== null) {
  demo(variant);
} else {
  void load();
}
