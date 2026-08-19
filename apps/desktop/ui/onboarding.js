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
  box.addEventListener("change", () => void set(sheet.key, box.checked));

  const body = document.createElement("span");

  const wording = document.createElement("span");
  wording.className = "wording";
  wording.textContent = sheet.wording;

  const without = document.createElement("span");
  without.className = "without";
  without.textContent = sheet.without;

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
  if (view.allows_recording) {
    // 「她可以開始記錄了」讀起來像**已經**開始了，而這一頁只負責同意——
    // 真正在錄的是另一個執行檔。第一次打開的人如果以為勾完就在錄了，他會
    // 等上一整天，然後發現什麼都沒有。所以這一句要指出下一步是什麼。
    say(`簽好了。接下來跑 sister record 她才會開始，${frames(view)}`);
  } else {
    // 這一句要講「現在的後果」，不是催他去按。
    say("第一張沒勾，sister record 不會開始錄；正在錄的也會停下來。");
  }
}

async function set(key, granted) {
  if (invoke === null) return;
  try {
    const view = await invoke("consent_set", { key, granted });
    paint(view);
    // 底下兩句會蓋掉 `paint` 剛寫的那一行，因為它們講的是**剛剛那一下做了
    // 什麼**，而那比「現在的狀態是什麼」更急：狀態他看得到（勾勾就在那裡），
    // 後果他看不到。
    if (view.reset_by_version) {
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
    say(String(err?.message ?? err), true);
    await load();
  }
}

async function load() {
  if (invoke === null) {
    say("這一頁不是在 AI-Sister 裡打開的，勾了不會存到任何地方。", true);
    return;
  }
  try {
    paint(await invoke("consent_read"));
  } catch (err) {
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
    "我同意把去識別化後的文字（永不含畫面）送到我指定的模型商做解讀。",
    "我同意保留變化幀的截圖，而不是只留上面的字。",
  ],
  without: [
    "沒有這一張，sister record 不會開始錄；正在錄的也會在 5 秒內停下來。",
    "沒有這一張，她完全在本機跑。（目前這份程式裡沒有任何連外路徑，所以這一張還沒有東西可以開。）",
    "沒有這一張，她只記螢幕上的字，不留截圖。",
  ],
  keys: ["local-recording", "cloud-reading", "frame-storage"],
};

function demoView(current, storeImages = true) {
  const at = [Date.UTC(2026, 7, 14, 2, 31), null, Date.UTC(2026, 6, 2, 9, 5)];
  return {
    path: DEMO.path,
    current,
    allows_recording: current,
    allows_frames: current,
    store_images: storeImages,
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
    if (cmd === "consent_read") return demoView(demo !== "stale", store);
    // 按下去之後才長得出來的那兩句話，只有從這裡才看得到。`shot.mjs` 收
    // selector，所以它們是截得到的——一個只驗初始畫面的工具，會讓人以為
    // 沒截到的那一半是好的。
    if (cmd === "consent_set") {
      const view = demoView(true, store);
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
