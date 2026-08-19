// 設定頁。和字母人一樣沒有打包步驟——這個檔案就是瀏覽器讀到的那個檔案。

const invoke = globalThis.__TAURI__?.core?.invoke ?? null;

const el = {
  path: document.querySelector("[data-path]"),
  apps: document.querySelector("[data-apps]"),
  urls: document.querySelector("[data-urls]"),
  titles: document.querySelector("[data-titles]"),
  screenshare: document.querySelector("[data-screenshare]"),
  redact: document.querySelector("[data-redact]"),
  querylog: document.querySelector("[data-querylog]"),
  framesDays: document.querySelector("[data-frames-days]"),
  textDays: document.querySelector("[data-text-days]"),
  lint: document.querySelector("[data-lint]"),
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

function paintHotkey(view) {
  capturing = false;
  el.combo.classList.remove("listening");
  el.combo.textContent =
    view.wanted === "" ? "沒有設" : pretty(view.wanted);

  const bad = view.rejected != null || (view.wanted !== "" && !view.registered);
  el.hotkeySay.classList.toggle("bad", bad);
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
    capturing = false;
    el.combo.classList.remove("listening");
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
    capturing = false;
    el.combo.classList.remove("listening");
    el.hotkeySay.classList.add("bad");
    el.hotkeySay.textContent = String(err?.message ?? err);
  }
}

// ---------- 讀 / 寫 ----------

function apply(s) {
  el.path.textContent = s.path;
  el.apps.value = s.excluded_apps.join("\n");
  el.urls.value = s.excluded_urls.join("\n");
  el.titles.value = s.excluded_titles.join("\n");
  el.screenshare.checked = s.pause_on_screenshare;
  el.redact.checked = s.redact_clipboard_secrets;
  el.querylog.checked = s.query_log;
  el.framesDays.value = s.frames_days;
  el.textDays.value = s.text_days;
}

async function load() {
  if (invoke === null) {
    say("這一頁不是在 AI-Sister 裡打開的，改了不會存到任何地方。", true);
    return;
  }
  try {
    apply(await invoke("settings_read"));
    say("");
    await relint();
  } catch (err) {
    say(String(err?.message ?? err), true);
  }
  // 熱鍵分開讀：它問的不是設定檔裡寫什麼，是**現在真的搶到了沒**——那個答案
  // 只有已經跑起來的那支程式知道。設定讀失敗也不該讓這一格空著。
  await reloadHotkey();
}

/**
 * 開發用：`settings.html?demo=1`，以及 `?demo=taken`（熱鍵被別的程式佔走）。
 *
 * 和 app.js 的 `?state=` 同一個理由——這台開發機開不起 Tauri 視窗，而這一頁
 * 有一堆版面（七個區塊、警告清單、底下那條）需要真的看過。**它走的是和產品
 * 一樣的 `apply()`、`paintLint()` 與 `paintHotkey()`**，不是另外畫一份長得像
 * 的出來：上一次讓開發開關走自己的路，截出來的圖就騙了我一次。
 *
 * 熱鍵那一格有兩種畫面，而「搶不到」才是這一格存在的理由，所以它也要被看過。
 */
function demo(variant) {
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
  });
  // 第二條規則故意是壞的，這樣才看得到警告那一格長什麼樣。理由字串抄自
  // `suspicious_url_rules` 真正回傳的那一句。
  paintLint([
    [
      "https://mail.google.com/*",
      "規則比對的是網址本身，開頭帶 https:// 的話多數瀏覽器讀回來的字串對不上；去掉通訊協定寫成 mail.google.com/* 才會命中",
    ],
  ]);
  paintHotkey(
    variant === "taken"
      ? {
          wanted: "Ctrl+Alt+KeyP",
          registered: false,
          reason: "RegisterEventHotKey failed",
        }
      : { wanted: "Ctrl+Alt+KeyP", registered: true, reason: null },
  );
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
    await invoke("settings_write", {
      settings: {
        excluded_apps: toLines(el.apps.value),
        excluded_urls: toLines(el.urls.value),
        excluded_titles: toLines(el.titles.value),
        pause_on_screenshare: el.screenshare.checked,
        redact_clipboard_secrets: el.redact.checked,
        query_log: el.querylog.checked,
        frames_days: days(el.framesDays, "畫面"),
        text_days: days(el.textDays, "文字"),
        // `path` 不送。要寫到哪個檔案由 Rust 那邊算，不是這一頁說了算。
      },
    });
    // 「存好了」還不夠。使用者要知道的是「她現在照這份跑了沒」——
    // 而答案是「最多 5 秒」，因為錄製那邊是輪詢設定檔的。
    say("存好了。正在跑的 record 會在 5 秒內換上這一份。");
    // 存進去的是剪過空白、丟過空行的版本，畫面要跟著變成那個樣子，
    // 不然他看到的和檔案裡的是兩份東西。
    await load();
  } catch (err) {
    say(String(err?.message ?? err), true);
  } finally {
    el.save.disabled = false;
  }
}

el.save?.addEventListener("click", () => void save());
el.reload?.addEventListener("click", () => void load());

const variant = new URLSearchParams(globalThis.location.search).get("demo");
if (variant !== null) {
  demo(variant);
} else {
  void load();
}
