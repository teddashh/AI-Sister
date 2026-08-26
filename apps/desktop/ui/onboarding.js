// 三張同意書。和其他幾頁一樣沒有打包步驟——這個檔案就是瀏覽器讀到的那個檔案。
//
// 這一頁上**沒有任何一句條文**。三段文字全部是從 Rust 那邊拿的
// （`sister_core::consent::Sheet`），因為 `sister consent`、`sister doctor` 和
// 這一頁講的必須是同一句話——三個地方各抄一份，遲早會變成三份不一樣的承諾，
// 而「他到底同意了哪一句」就沒有答案了。

let invoke = globalThis.__TAURI__?.core?.invoke ?? null;

const el = {
  cards: document.querySelector("[data-cards]"),
  path: document.querySelector("[data-path]"),
  say: document.querySelector("[data-say]"),
  done: document.querySelector("[data-done]"),
};

function say(message, bad = false) {
  el.say.textContent = message;
  el.say.classList.toggle("bad", bad);
}

const when = new Intl.DateTimeFormat("zh-TW", {
  year: "numeric",
  month: "long",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

function card(sheet) {
  const li = document.createElement("li");
  li.className = sheet.effective ? "card on" : "card";

  const label = document.createElement("label");
  const box = document.createElement("input");
  box.type = "checkbox";
  box.checked = sheet.effective;
  // 把 `box` 也交出去：寫失敗的時候要有人把它翻回來，理由見 `set` 的 catch。
  box.addEventListener("change", () => void set(sheet.key, box.checked, box));

  const body = document.createElement("span");

  const wording = document.createElement("span");
  wording.className = "wording";
  wording.textContent = sheet.wording;

  const without = document.createElement("span");
  without.className = "without";
  // 第二張勾下去之後，沒簽那句「一次都不會呼叫」就不該再印在勾好的框裡。
  without.textContent =
    sheet.key === "cloud-reading" && sheet.effective
      ? "勾了之後，螢幕上的字會原封不動交給你在設定裡指定的那支 CLI。沒設定命令就一次都不叫。"
      : sheet.without;

  body.append(wording, without);

  // 「我什麼時候同意的」是他有權利問的問題，而我們答得出來。
  if (sheet.granted_at !== null) {
    const since = document.createElement("span");
    since.className = sheet.effective ? "since" : "since stale";
    since.textContent = sheet.effective
      ? `${when.format(sheet.granted_at)} 同意`
      : // 簽過、但條文改版了。不能只把勾勾拿掉就算了——那看起來像他從來
        // 沒按過，而他真的按過。
        `${when.format(sheet.granted_at)} 同意過，但條文後來改版了，要重新確認`;
    body.append(since);
  }

  label.append(box, body);
  li.append(label);
  return li;
}

/**
 * 「所以到底會不會留截圖」——四種狀態，不是兩種。
 *
 * 這一句以前只看第三張同意書。但同意是**上限**，不是開關：設定檔的
 * `store_images` 關著的時候，簽了也一張都不會留，而這一頁會照樣說「而且會留
 * 截圖」——一句他要去翻 frames/ 才戳得破的假話。
 *
 * `store_images` 是 `null` 表示後端讀不出設定檔。那不是「關著」，也不是「開
 * 著」，所以它自己一句：猜錯的方向會是「答應了一件不會發生的事」。
 */
function frames(view) {
  if (!view.allows_frames) return "只記螢幕上的字、不留截圖。";
  if (view.store_images === true) return "而且會留截圖。";
  if (view.store_images === false)
    return "但設定檔的 store_images 關著，所以不會留截圖——這一張你已經簽了。";
  return "會不會留截圖要看設定檔的 store_images，而現在讀不到那個檔案。";
}

function paint(view) {
  el.path.textContent = view.path;
  el.cards.replaceChildren(...view.sheets.map(card));
  if (view.allows_recording && view.capture_enabled === false) {
    // 總開關關著。這一句要**排在留不留圖前面**：那個判斷回答的是「會不會
    // 多出截圖」，而這裡的答案是連字都不會有。以前這兩件事只有一句話，
    // 而那句話說的是「她才會開始，而且會留截圖」——兩個子句都錯。
    say(
      "簽好了，但設定檔的 capture.enabled 是 false：sister record 跑起來" +
        "也會每一拍直接跳過，一個字都不會記。改成 true 才會真的開始。",
    );
  } else if (view.allows_recording && view.capture_enabled === null) {
    // `null` 不是 `false`：它表示後端連設定檔都讀不出來（TOML 語法錯，或
    // `retention` 填了 `check()` 過不了的值）。而 `sister record` 走的是同一支
    // `Config::load`，它會在門口就退出——她不會開始。
    //
    // 這個狀態被建模在型別裡、寫了九行文件、穿過 IPC、`frames()` 裡還有專屬
    // 的一句，然後在**寫標題的這一支**掉進了下面那條「簽好了。接下來…她才會
    // 開始」。畫面上唯一的保留意見只涵蓋「會不會留截圖」，而真正的後果是
    // 一個字都不會有。他從這一頁走掉，二十五秒後才有機會在 record.log 裡讀到
    // 原因——前提是他想得到去看。
    say(
      "簽好了，但設定檔讀不出來（多半是 TOML 打錯字，或 retention 填了不合法的值）：" +
        "在修好之前她不會開始錄。設定頁上會說是哪裡壞了。",
      true,
    );
  } else if (view.allows_recording) {
    // 「她可以開始記錄了」讀起來像**已經**開始了，而這一頁只負責同意——
    // 真正在錄的是另一個執行檔。第一次打開的人如果以為勾完就在錄了，他會
    // 等上一整天，然後發現什麼都沒有。所以這一句要指出下一步是什麼。
    //
    // 下一步是**按鈕**，不是指令。她那個小視窗右上角有一顆「開始記錄」，
    // 系統匣裡也有一個。叫一個只裝了視窗版的人去開終端機打 `sister record`，
    // 是在描述一個更早的版本——而那個版本的下一步他做不到。
    say(`簽好了。按下面那顆「好」回到她那邊，按「開始記錄」她就開始，${frames(view)}`);
  } else if (view.sheets[0]?.granted_at != null) {
    // **第三種狀態。** 他簽過了，只是條文後來改版。以前這裡和「從來沒勾過」
    // 走同一條分支，於是卡片上寫著「2026年8月13日 同意過，但條文後來改版
    // 了」，三吋以下的這一行寫著「第一張沒勾」——同一個畫面上互相打臉。
    // CLI 早就有這第三句（`ops.rs` 的 consent 那一段）。
    say(
      "第一張你簽過，但條文後來改版了，要再確認一次才算數——" +
        "在那之前 sister record 不會開始錄。",
    );
  } else {
    // 這一句要講「現在的後果」，不是催他去按。
    say("第一張沒勾，sister record 不會開始錄；正在錄的也會停下來。");
  }
}

async function set(key, granted, box = null) {
  if (invoke === null) return;
  try {
    const view = await invoke("consent_set", { key, granted });
    paint(view);
    // 底下兩句會蓋掉 `paint` 剛寫的那一行，因為它們講的是**剛剛那一下做了
    // 什麼**，而那比「現在的狀態是什麼」更急：狀態他看得到（勾勾就在那裡），
    // 後果他看不到。
    if (view.reset_by_version && !granted) {
      // 他**取消**勾選的那一路：後端先把整份清成預設值（改版的簽名不能跟著
      // 新的一起存成「這一版簽的」），然後 `revoke` 落在一張本來就空的紙上。
      // 結果是三張全空，而下面那句話說「只留下你剛剛按的這一張」——那一張
      // 不存在。
      say("條文改版了，你之前簽的那幾張本來就已經不算數。現在三張都是空的，要重新確認。");
    } else if (view.reset_by_version) {
      // CLI 對這件事印一行 ⚠，這一頁以前完全安靜——他勾了一張，另外兩張的
      // 「2026 年 7 月 2 日同意過」就從畫面上消失了，像是紀錄被弄丟了。
      say("條文改版了，之前簽的那幾張不再算數，現在只留下你剛剛按的這一張。另外兩張要重新確認。");
    } else if (key === "frame-storage" && !granted) {
      // 撤掉這一張只擋**新的**截圖。不講的話，「不留截圖」讀起來像「截圖沒
      // 了」——而他要去翻 frames/ 才會發現不是。錄製迴圈撤回時也講同一句話。
      say(
        "從現在起她不會再寫新的截圖。先前已經寫下的不會因為這個動作消失" +
          "——要清掉請用時間軸的「忘掉這一段」，或跑 sister prune。",
      );
    }
  } catch (err) {
    // 存不進去就**不要**改畫面。顯示成已同意、實際上沒寫進檔案，是這一頁
    // 唯一一種真正嚴重的錯——`sister record` 讀的是那個檔案，不是這個畫面。
    //
    // **勾勾要在這裡翻回來，不能只靠底下那個 `load()`。** 那是一顆原生的
    // `<input type="checkbox">`：`change` 事件走到這裡的時候，瀏覽器早就把
    // 它翻過去了。而 `load()` 讀的是同一個檔案、同一顆磁碟、同一個鎖——寫
    // 不進去的時候它多半也讀不出來，那條路上 `paint()` 根本不會跑，勾勾就
    // 停在他剛剛按的樣子。畫面說已同意，檔案裡什麼都沒有。
    //
    // 反方向更糟：他把第三張**取消**勾選來停掉截圖，寫失敗、讀也失敗，勾勾
    // 留在取消的樣子——而她繼續寫圖。他關掉這一頁就不會再回來看了。
    //
    // 翻回去要用的是「他按之前的值」，那個我們自己知道（`!granted`），不必
    // 去問磁碟。讀得回來的話 `load()` 會用檔案裡那一份蓋掉整批卡片，這一行
    // 就只是中間狀態。
    if (box) box.checked = !granted;
    say(String(err?.message ?? err), true);
    // `load()` 會呼叫 `paint()`，而 `paint()` 一定會 `say(...)`——所以上面那
    // 行紅字會被立刻蓋成一句平靜的「第一張沒勾」，而且連 `bad` 都被清掉。
    // 使用者看到的是：勾勾彈回去，配一句告訴他「你還沒同意」。他可以按到
    // 天亮。狀態修好了、訊息毀了，等於只做了一半。
    await load({ keepSay: true });
  }
}

/**
 * 重讀同意書並重畫。
 *
 * `keepSay` 是給「寫失敗之後要把畫面轉回真實狀態」那條路用的：卡片要修正，
 * 但底下那行錯誤訊息**不能**被 `paint()` 的例行敘述蓋掉。
 */
async function load({ keepSay = false } = {}) {
  if (invoke === null) {
    say("這一頁不是在 AI-Sister 裡打開的，勾了不會存到任何地方。", true);
    return;
  }
  const before = keepSay
    ? { text: el.say.textContent, bad: el.say.classList.contains("bad") }
    : null;
  try {
    paint(await invoke("consent_read"));
    if (before) say(before.text, before.bad);
  } catch (err) {
    // 讀也失敗的時候，畫面上該留哪一句？**留寫的那一句。**
    //
    // 兩個失敗多半是同一個原因（同一個檔案），而他能動的只有寫的那一件：
    // 「你剛剛那一下沒有存進去」。換成「讀不出同意書」的話，聽起來像顯示
    // 問題——他會以為勾勾是對的、只是畫面沒更新，而那正好是這一頁最不能
    // 讓他相信的一件事。
    //
    // 第二行講的是這一刻真正的處境：這一頁上的勾勾現在誰也不代表。
    if (before) {
      say(
        `${before.text}\n而且現在連同意書本身都讀不出來，所以這一頁上的勾勾不保證是檔案裡的樣子。`,
        true,
      );
      return;
    }
    say(String(err?.message ?? err), true);
  }
}

// 「好」只是關掉這一頁。**沒有「全部同意」那顆按鈕**——一顆一次勾完三張的
// 按鈕，會讓第二張（上雲解讀）在他沒有分別想過的情況下被打開。
el.done?.addEventListener("click", () => {
  globalThis.__TAURI__?.window?.getCurrentWindow?.()?.close?.();
});

/**
 * 開發用：`onboarding.html?demo=1`（一般狀態）與 `?demo=stale`（條文改版）。
 *
 * 理由和其他幾頁一樣——這台開發機開不起 Tauri。假的是後端，`paint()` /
 * `card()` 走的是產品那一條路。
 *
 * **為什麼是兩種而不是一張圖裡三種狀態全有**：`current: false` 的時候，簽過的
 * 每一張都同時失效——它是整份的屬性，不是單張的。第一版的假資料讓「現行有效」
 * 和「條文過期」出現在同一張圖上，那是一個真實世界不可能發生的畫面，而我差點
 * 就拿它當作驗過了。
 */
const DEMO = {
  path: "C:\\Users\\ted\\AppData\\Roaming\\ted-h\\AI-Sister\\data\\consent.toml",
  wording: [
    "我同意在我的硬碟上記錄我的螢幕。",
    "我同意把螢幕上的文字原文（OCR 抽出來的字，永不含畫面）交給我在設定裡指定的本機 CLI，由那支程式去做解讀。裡面有什麼就送什麼，不會先遮掉。",
    "我同意保留變化幀的截圖，而不是只留上面的字。",
  ],
  without: [
    "沒有這一張，sister record 不會開始錄；錄到一半撤回，正在跑的 record 每 5 秒重讀同意書，最多再錄 5 秒加一拍；capture.min_interval_ms 超過 5 秒時，主要會等那一拍。",
    "沒有這一張，她一次都不會呼叫那支 CLI；解釋層保持關閉，只累積本機的畫面與文字。",
    "沒有這一張，她只記螢幕上的字，不留截圖。",
  ],
  keys: ["local-recording", "cloud-reading", "frame-storage"],
};

function demoView(current, storeImages = true, mode = "1") {
  const at = [
    Date.UTC(2026, 7, 14, 2, 31),
    // 第二張平常是沒勾的。`?demo=cloud` 把它勾起來——那是唯一看得到
    // 「勾了之後字會交給 CLI」那句話的辦法。
    mode === "cloud" ? Date.UTC(2026, 7, 14, 2, 32) : null,
    Date.UTC(2026, 6, 2, 9, 5),
  ];
  return {
    path: DEMO.path,
    current,
    allows_recording: current,
    allows_frames: current,
    store_images: storeImages,
    // `?demo=broken`：設定檔讀不出來。`null` 不是 `false`——她連開始都不會。
    capture_enabled: mode === "broken" ? null : true,
    reset_by_version: false,
    sheets: DEMO.keys.map((key, i) => ({
      key,
      wording: DEMO.wording[i],
      without: DEMO.without[i],
      granted_at: at[i],
      // 簽過**而且**條文沒改版才算數。`current` 是整份的屬性——改版時三張
      // 一起失效，不會有一張有效、另一張同時顯示「條文改版了」。
      effective: current && at[i] !== null,
    })),
  };
}

const demo = new URLSearchParams(globalThis.location.search).get("demo");
if (demo !== null) {
  // `?demo=off` / `?demo=unknown`：第三張簽了，但設定檔說不留圖 / 讀不到設定
  // 檔。這兩種在這台開發機上是看得到那句話的唯一辦法。
  //
  // `hasOwn` 而不是 `?? true`：`unknown` 對到的就是 `null`，而 `null ?? true`
  // 會把它變成 true——那個 demo 會照著「會留截圖」畫，於是這一頁最新的那個
  // 狀態永遠沒有被看過。（第一版就是這樣寫的，截圖當場抓到。）
  const STORE = { off: false, unknown: null };
  const store = Object.hasOwn(STORE, demo) ? STORE[demo] : true;
  invoke = async (cmd, args) => {
    if (cmd === "consent_read") return demoView(demo !== "stale", store, demo);
    // 按下去之後才長得出來的那兩句話，只有從這裡才看得到。`shot.mjs` 收
    // selector，所以它們是截得到的——一個只驗初始畫面的工具，會讓人以為
    // 沒截到的那一半是好的。
    // `?demo=readonly`：同意書寫不進去（唯讀磁碟、磁碟滿、防毒鎖住
    // consent.toml）。這個 demo 存在的理由是它以前**看起來完全正常**——
    // 紅字被 `load()` 的重畫立刻蓋成一句平靜的「第一張沒勾」。
    if (cmd === "consent_set" && demo === "readonly") {
      throw new Error(
        "寫不進 consent.toml：拒絕存取（os error 5）。她的同意狀態沒有改變。",
      );
    }
    if (cmd === "consent_set") {
      const view = demoView(true, store, demo);
      const reset = demo === "stale";
      // 假資料也要照著他按的那一下改。不改的話畫面上會出現一個真實世界不
      // 可能的組合（勾勾還在、底下卻說她不會再留截圖），而這一頁上一次差
      // 點被當成「驗過了」，就是因為一張這樣的假畫面。
      for (const s of view.sheets) {
        const mine = s.key === args.key;
        const on = mine ? args.granted : !reset && s.effective;
        s.effective = on;
        s.granted_at = on ? (mine ? Date.UTC(2026, 7, 19, 9, 0) : s.granted_at) : null;
      }
      view.allows_recording = view.sheets[0].effective;
      view.allows_frames = view.sheets[2].effective;
      return { ...view, reset_by_version: reset };
    }
    throw new Error(`demo 沒有實作 ${cmd}`);
  };
}
void load();
