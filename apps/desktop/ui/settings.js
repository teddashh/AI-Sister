// 設定頁。和字母人一樣沒有打包步驟——這個檔案就是瀏覽器讀到的那個檔案。

const invoke = globalThis.__TAURI__?.core?.invoke ?? null;

const el = {
  path: document.querySelector("[data-path]"),
  apps: document.querySelector("[data-apps]"),
  urls: document.querySelector("[data-urls]"),
  titles: document.querySelector("[data-titles]"),
  screenshare: document.querySelector("[data-screenshare]"),
  redact: document.querySelector("[data-redact]"),
  framesDays: document.querySelector("[data-frames-days]"),
  textDays: document.querySelector("[data-text-days]"),
  lint: document.querySelector("[data-lint]"),
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

// ---------- 讀 / 寫 ----------

function apply(s) {
  el.path.textContent = s.path;
  el.apps.value = s.excluded_apps.join("\n");
  el.urls.value = s.excluded_urls.join("\n");
  el.titles.value = s.excluded_titles.join("\n");
  el.screenshare.checked = s.pause_on_screenshare;
  el.redact.checked = s.redact_clipboard_secrets;
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
}

/**
 * 開發用：`settings.html?demo=1`。
 *
 * 和 app.js 的 `?state=` 同一個理由——這台開發機開不起 Tauri 視窗，而這一頁
 * 有一堆版面（六個區塊、警告清單、底下那條）需要真的看過。**它走的是和產品
 * 一樣的 `apply()` 與 `paintLint()`**，不是另外畫一份長得像的出來：上一次
 * 讓開發開關走自己的路，截出來的圖就騙了我一次。
 */
function demo() {
  apply({
    path: "C:\\Users\\ted\\AppData\\Roaming\\ted-h\\AI-Sister\\config\\config.toml",
    excluded_apps: ["keepassxc", "1password", "bitwarden", "authy"],
    excluded_urls: ["*.bank.com.tw*", "https://mail.google.com/*", "*/admin/*"],
    excluded_titles: ["*password*", "*信用卡*"],
    pause_on_screenshare: true,
    redact_clipboard_secrets: true,
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
  say("這是 demo 版面，沒有讀任何真的設定。");
}

/**
 * 天數欄位。空的、負的、寫成「三十」的，都不可以變成 0——
 * 因為 0 的意思是**立刻刪**，而一個手滑清空的欄位不該把使用者一年的
 * 文字紀錄按下去。看不懂就退回目前的值，並且說出來。
 */
function days(input, label) {
  const n = Number.parseInt(input.value, 10);
  if (!Number.isInteger(n) || n < 0) {
    throw new Error(`${label}的天數看不懂：「${input.value}」`);
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

if (new URLSearchParams(globalThis.location.search).get("demo") === "1") {
  demo();
} else {
  void load();
}
