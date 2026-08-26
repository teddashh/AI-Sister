// 字母人的行為。純 ES module，**沒有打包步驟**——這個檔案就是瀏覽器讀到的
// 那個檔案。Phase 1 的退場條件裡有一條「clone → 跑起來 < 10 分鐘」，而在一個
// Rust repo 裡塞一套 Node 工具鏈是最容易讓那條過不了的東西。
//
// 需要型別的時候再加 tsc，那是改一個檔案的事；現在還不需要。

/**
 * 她**正在做什麼**。注意 `paused` 不在這裡：它是另一個維度（見下面的
 * `paused`），因為她可以「暫停中、同時正在想一個問題的答案」。
 */
const STATES = Object.freeze(["idle", "thinking"]);

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

const avatar = document.querySelector("[data-avatar]");
const stateLine = document.querySelector("[data-state-line]");
const askInput = document.querySelector("[data-ask-input]");
const askSend = document.querySelector("[data-ask-send]");
const pinButton = document.querySelector("#pin");
const hideButton = document.querySelector("#hide");
const pauseButton = document.querySelector("#pause");
const timelineButton = document.querySelector("#timeline");
const wakeButton = document.querySelector("[data-wake]");
const utterance = document.querySelector("[data-utterance]");
const utteranceText = document.querySelector("[data-utterance-text]");
const utteranceEvidence = document.querySelector("[data-utterance-evidence]");
const utteranceActions = document.querySelector("[data-utterance-actions]");
const utteranceClose = document.querySelector("[data-utterance-close]");
const utteranceOther = document.querySelector("[data-utterance-other]");
const utteranceResult = document.querySelector("[data-utterance-result]");
const gateDebug = document.querySelector("[data-gate-debug]");
const suggestionButton = document.querySelector("[data-utterance-suggestion]");
const handsLog = document.querySelector("[data-hands-log]");

/**
 * Tauri 的 IPC。**在瀏覽器裡打開時是 null**，而那是刻意支援的：字母人整個
 * 是 HTML/CSS，所以它可以在一般瀏覽器裡開發、截圖、比對，不必每次都去開一個
 * 桌面視窗。所有會用到 IPC 的地方都要能在 null 之下安靜地降級。
 */
const invoke = globalThis.__TAURI__?.core?.invoke ?? null;

// ---------- 狀態 ----------

/**
 * 兩個獨立的東西，不是三選一：
 *
 * - `state` 是她**正在做什麼**（在聽／想一下）。
 * - `paused` 是她**有沒有在看**，而且真相不在這個行程裡，是 data dir 裡的
 *   一個檔案（見 sister-core 的 `pause` 模組）。系統匣、上一次開機、甚至
 *   使用者自己去刪檔案，都能改變它。
 *
 * 混成一個變數的話會出現一個很難發現的 bug：暫停中問一句話 → 進 thinking →
 * 答完回 idle → **暫停的樣子不見了，但她其實還在暫停**。
 */
let state = "idle";
let paused = false;

/**
 * 她有沒有一件事想讓人看見，是第四個維度：不改「正在做什麼」、不假裝錄製
 * 狀態，也不改暫停真相。Glimmer 只把這個位元翻起來；所以仍可同時是
 * thinking + paused + has-something，而 `paint()` 的前三個合成規則完全不變。
 */
let hasSomething = false;
let activeUtteranceId = null;

function paintGatekeeper(view) {
  const item = view?.display ?? null;
  const debug = view?.developer ?? null;
  if (handsLog) {
    const lines = view?.action_log ?? [];
    handsLog.textContent = lines.length === 0 ? "還沒有提出過任何動作。" : `行動紀錄\n${lines.join("\n")}`;
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
    suggestionButton.hidden = true;
    suggestionButton.removeAttribute("data-commitment-id");
    return;
  }
  // 同一句話重畫一次不要把他讀到一半的回條擦掉。
  if (item.utterance_id === activeUtteranceId) return;
  activeUtteranceId = item.utterance_id;
  hasSomething = true;
  avatar.classList.add("has-something");
  utteranceResult.textContent = "";
  utteranceEvidence.replaceChildren();
  if (item.suggestion === null) {
    suggestionButton.hidden = true;
    suggestionButton.removeAttribute("data-commitment-id");
  } else {
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
      utteranceEvidence.hidden = false;
      utteranceActions.hidden = false;
      return;
  }
  throw new Error(`不知道的 gatekeeper form：${item.form}`);
}

function reactToGatekeeper(close) {
  if (invoke === null || activeUtteranceId === null) return;
  invoke("gatekeeper_react", { utteranceId: activeUtteranceId, close }).then(
    (message) => {
      // 三個後端結果分別是「這張記憶不會再提了」、「先收起來，之後再說」、
      // 「收到你的回饋；這一則沒有可結案或延後的承諾」。不在前端猜類別。
      utteranceResult.textContent = message;
      utteranceActions.hidden = true;
      hasSomething = false;
      avatar.classList.remove("has-something");
    },
    (error) => { utteranceResult.textContent = String(error); },
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
function noticeAboutHer(text) {
  notice = { text: String(text ?? ""), aboutHer: true };
}

/**
 * 這句話在講他**同時**做的別的事：問一題、按暫停、開時間軸。
 *
 * 這三個是唯一會在她起來的那 25 秒（`booting` 更久）裡寫字進來、而且真的和她
 * 起不起得來無關的來源。而那正是他最可能去問問題的那 25 秒——畫面剛剛叫他
 * 等一下。
 */
function noticeAboutSomethingElse(text) {
  notice = { text: String(text ?? ""), aboutHer: false };
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
 * 這一題翻很久了（`null` = 沒有／已經回來了）。見 [`SLOW_MS`]。
 *
 * 一樣是被 `paint()` 蓋掉的那一種：它以前直接寫 `stateLine.textContent`，於是
 * 「還在翻…」在畫面上閃一下就被輪詢換回「想一下…」——而「想一下…」不動地停
 * 在那裡，正是這一句話當初要修的那個畫面。修法變成看起來像 glitch，比不修
 * 更糟。
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
  // 而「被你叫停」要壓過「根本沒人開她」——前者是他做的決定，後者只是狀態。
  const shown = paused ? "paused" : recording ? state : "asleep";
  avatar.dataset.state = shown;

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
  const line = booting
    ? "她起來了，正在開資料庫…（大的記憶要等一下，這期間還沒開始記）"
    : thinkingLast
      ? "錄製已停，解釋層還在想最後一段"
      : starting
        ? "正在把她叫起來…"
        : state === "thinking" && slowNote !== null
          ? slowNote
          : state === "thinking" && shown !== "thinking"
            ? `想一下…（${shown === "paused" ? "仍在暫停" : "但沒有人在記錄"}）`
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
  if (notice !== null) {
    detail =
      (starting || booting || thinkingLast) && !notice.aboutHer
        ? `這是另一件事：${notice.text}`
        : notice.text;
  } else if (!starting && !booting && !thinkingLast && shown === "asleep") {
    detail = asleepDetail();
  }
  stateLine.textContent = detail === "" ? line : `${line}\n${detail}`;
  // 讀螢幕的人也要知道她在忙，不然「想一下…」只是給看得見的人看的。
  avatar.setAttribute("aria-label", `AI-Sister：${line}`);

  if (wakeButton) {
    // 只在真的沒人在錄的時候出現。暫停中不出現——那時候的下一步是按 ▶，
    // 而不是再開一個 recorder（那會變成兩個行程各錄一份）。
    //
    // `booting` 也不出現，而且理由是同一個：那幾分鐘目錄已經有人佔著，按下去
    // 撞的是 `start_recording` 那道 `is_occupied` 閘門。
    wakeButton.hidden = shown !== "asleep" || starting || booting || thinkingLast;
  }
}

function setState(next) {
  if (!STATES.includes(next)) return;
  state = next;
  paint();
}

function setPaused(next) {
  const was = paused;
  paused = next === true;
  // 從系統匣（或熱鍵）切過來的那一下，也算「下一件事發生了」。見
  // [`overtakenByEvents`]——這一格漏掉的時候，「暫停鍵沒有作用」會掛在
  // 一個已經暫停了的字母人底下。
  if (was !== paused) overtakenByEvents();
  if (pauseButton) {
    pauseButton.textContent = paused ? "▶" : "⏸";
    pauseButton.title = paused ? "繼續記錄" : "暫停記錄";
    // 「按下去了」= 暫停中。CSS 會把沒按下的那顆調淡，所以暫停時它最亮——
    // 這正是我們要的：不正常的狀態要吵。
    pauseButton.setAttribute("aria-pressed", String(paused));
  }
  paint();
}

/**
 * 心跳說什麼：`"recording"`／`"booting"`／`"thinking"`／`"none"`（見後端的
 * `recording_state`）。
 *
 * **收四個字串，不是一個布林。** 認不得的值一律當成「沒有人在錄」——四種裡
 * 只有它不會替一件沒發生的事背書。`"thinking"` 是錄製已停、腦還在想最後
 * 一段：說「在聽」是謊，說「沒有人在記錄」配一顆開始鍵也是謊。
 */
function setRecording(next) {
  const was = recording;
  const wasBooting = booting;
  const wasThinking = thinkingLast;
  recording = next === "recording";
  booting = next === "booting";
  thinkingLast = next === "thinking";
  // 她從別的地方被開起來、或是自己停掉了：一樣是「下一件事發生了」。見
  // [`overtakenByEvents`]。
  if (was !== recording || wasBooting !== booting || wasThinking !== thinkingLast) {
    overtakenByEvents();
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
  const fullyStopped = !recording && !booting && !thinkingLast;
  const wasFullyStopped = !was && !wasBooting && !wasThinking;
  if (!wasFullyStopped && fullyStopped) refreshLastRun();
  paint();
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
    noticeAboutHer(err?.message ?? err);
    starting = false;
    paint();
    return;
  }
  const deadline = Date.now() + WAKE_TIMEOUT_MS;
  while (starting && Date.now() < deadline) {
    await new Promise((done) => setTimeout(done, WAKE_POLL_MS));
    // `setRecording(true)` 會把 `starting` 關掉，迴圈自己就結束了。
    try {
      setRecording(await invoke("recording_state"));
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
  noticeAboutHer(
    why
      ? `等了 ${waited} 秒還沒有心跳。record.log 最後說：\n${why}`
      : `等了 ${waited} 秒還沒有心跳，record.log 也還是空的。` +
          "她可能還在起來——再等一下，或去看那個檔案",
  );
  starting = false;
  paint();
}

wakeButton?.addEventListener("click", startRecording);

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
 * 所以在終端機裡 `sister resume` 之後，她其實已經在錄了，而字母人會一直灰著
 * 說「已暫停，沒有在看」。更糟的是那顆 ▶：`toggle_pause` 讀的是磁碟，所以按
 * 下「繼續記錄」實際上是把她**暫停**——而畫面本來就畫成暫停的樣子，按完什麼
 * 都不會變。旁邊那句註解說得很清楚：顯示成已暫停、實際上還在錄，是這個產品
 * 能犯的最嚴重的一種謊。反過來這一種同樣是它。
 *
 * 磁碟上的旗標是真相，這個視窗只是鏡子——和 `ask` 每次重讀設定檔同一條紀律。
 */
function pollRecording() {
  if (invoke === null) return;
  invoke("recording_state").then(setRecording, () => {});
  invoke("pause_state").then(setPaused, () => {});
  // 守門員也要一直問下去。**只在開場問一次的話，五點才到期的那張承諾
  // 永遠不會被看到**——而 a 類（顯式時間承諾）正是整個 Phase 5 冷啟動期
  // 唯一放行的兩類之一，它不動就等於守門員沒上線。
  //
  // 每 5 秒問一次不會把預算燒掉：後端那一側同一件事今天只記一次帳，
  // 已經開口而人還沒回應的那一句是繼續顯示、不重扣。理由寫在
  // `main.rs` 的 `gatekeeper_check` 上面。
  invoke("gatekeeper_check").then(paintGatekeeper, () => {});
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
    document.visibilityState === "visible" && reducedMotion?.matches !== true;
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
  pinButton.textContent = pinned ? "●" : "○";
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
    noticeAboutSomethingElse(err?.message ?? err);
    paint();
  }
});

/**
 * 系統匣上也有同一顆暫停鍵，所以狀態可能從**這個視窗以外**改變。
 * 沒有這一段的話，從系統匣暫停之後，字母人會繼續一臉「我在聽」。
 */
globalThis.__TAURI__?.event
  ?.listen?.("pause-changed", (event) => setPaused(event.payload))
  ?.catch?.(() => {});

/**
 * 從系統匣按「開始記錄」失敗的時候，那句原因沒有地方可以寫。
 *
 * 同意書沒簽、找不到 `sister.exe`、已經有一個在跑——三句都是後端寫好的完整
 * 中文，而系統匣選單上沒有一格能放字。以前它們只進 `desktop.log`：按下去的
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
    // **這一句必然是在講她。** 系統匣那一顆送出去的不是 start 就是 stop
    // （`main.rs` 的 handler 讀 `recording_state` 決定方向，不讀選單上那行
    // 字），所以每一句都在回答「她會不會開始／她停下來了沒」。
    noticeAboutHer(event.payload);
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

function chapterHit(ch) {
  const li = document.createElement("li");
  li.className = "hit chapter";
  const whenEl = document.createElement("p");
  whenEl.className = "chapter-when";
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
  const label = [ch.app, ch.title || ch.host].filter(Boolean).join(" · ");
  what.textContent = label || "一段紀錄";
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
    const blocked = blind.paused_episodes > 0 || blind.excluded?.length > 0;
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
      out.push("（我錄過，但一列內容都沒存進來過——先看設定頁的「開始記錄」那一段，`sister doctor` 會直接說。）");
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
  let marked = new URLSearchParams(location.search).get("marked") === "1";

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

/**
 * @param hits 一筆一筆的原文。
 * @param kind `"keywords"`（比對字找到的）、`"recent"`（剛剛）、或 `"range"`（昨天下午那種日曆範圍）。
 *   這個字是後端給的，不是這裡判斷的——同一句話在 `sister query` 和這一頁
 *   必須得到同一種答案，所以規則只有一份，在 sister-core 的 `question`。
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
) {
  hitList.replaceChildren();

  if (closureNotice) {
    const notice = document.createElement("li");
    notice.className = "hits-note";
    notice.textContent = closureNotice;
    hitList.append(notice);
  }

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
          hitList.append(chapterHit(ch));
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
    note.textContent = "我最後看到的是：";
    hitList.append(note);
  }

  for (const [rank, fact] of facts.entries()) {
    const li = document.createElement("li");
    li.className = "hit fact";

    const value = document.createElement("p");
    value.className = "fact-value";
    value.textContent = fact.value;
    // 1 次和 12 次是強度不同的答案。她自己不下判斷，只把數字講出來。
    if (fact.sightings > 1) {
      const seen = document.createElement("span");
      seen.className = "fact-seen";
      seen.textContent = `看過 ${fact.sightings} 次`;
      value.append(seen);
    }
    li.append(value);

    // 正規化後的值認得出來，原文才認得出**場景**——`+886800080123` 是機器
    // 要的，`客服專線 0800-080-123` 才是他記得的那一行。兩個都給。
    const raw = document.createElement("p");
    raw.className = "hit-text fact-raw";
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

  hitList.hidden = false;
  document.body.classList.add("has-hits");
  // **不是無條件 `true`。** [`showingAnswer`] 的唯一讀者是那句「底下原本那幾筆
  // 是上一題的」，而空手而回的那一次底下躺的是「我記得的東西裡沒有這件事。」
  // 加上幾行理由——一筆都沒有。寫死 `true` 的話，下一題失敗會請他去看幾筆
  // 不存在的東西，而**空手而回正是他最可能連問第二次的那一種結果**。
  showingAnswer = hits.length > 0 || facts.length > 0 || hasChapters;
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

async function ask() {
  const question = askInput.value.trim();
  if (question === "") return;

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
      slowNote = "還在翻…（這一題已經超過 4 秒）";
      paint();
    }
  }, SLOW_MS);

  try {
    if (invoke === null) throw new Error("這一頁不是在 AI-Sister 裡打開的");
    const answer = await invoke("ask", { question });
    // 這一份過期了。畫面歸還在跑的那一次管，這裡連 idle 都不要設。
    if (mine !== asking) return;
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
    );
    setState("idle");
    // 答完才清掉。失敗的時候留著，他才不用把整句話重打一次。
    askInput.value = "";
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
  } finally {
    clearTimeout(slow);
    // **過期的那一份不准動畫面，包括這裡。** 他多按了幾次 Enter、先送的後回，
    // 那一次走到這裡的時候還在跑的是別題——清掉的會是**那一題**的「還在翻…」。
    // `asking` 那個編號存在的理由就是這個，上面兩條路都問過了，這一條也要問。
    if (mine === asking) {
      slowNote = null;
      paint();
    }
  }
}

askSend?.addEventListener("click", () => void ask());
askInput?.addEventListener("keydown", (event) => {
  // 選字中的 Enter 是「就選這個字」，不是「問出去」。注音打「剛剛發生什麼事」
  // 一路上會按好幾次 Enter，少了這一行，第一次選字就把半句話送出去了。
  // `keyCode === 229` 是舊的那條路，有些 IME 只給得出這個。
  if (event.isComposing || event.keyCode === 229) return;
  if (event.key === "Enter") void ask();
});

// ---------- 開場 ----------

// 開發用的兩個開關：`?state=paused` 直接看某一個狀態，`?hits=demo` 看
// 有答案時的版面。**Tauri 載入時沒有 query string**，所以這兩條路在產品裡
// 走不到——它們存在的理由是這台開發機開不起 Tauri 視窗（沒有 webkit2gtk、
// 沒有 sudo），而版面對不對不該等到上了 Windows 才第一次看到。
const params = new URLSearchParams(globalThis.location.search);

seedSwayPhase();
updateMotionGate();
paintPin();

// `?state=paused` 走的是**和產品一樣的那條路**（設 `paused` 旗標），不是另外
// 搬一個長得像暫停的樣子出來。這一點是被截圖抓到的：第一版讓它去設 `state`，
// 於是截出來的圖裡字母人是灰的、但拖曳條上的暫停鍵還是「⏸」——而截圖是這台
// 機器上唯一看得到 UI 的方式，一個走假路的開發開關會讓它騙我。
const wanted = params.get("state") ?? "idle";
setPaused(wanted === "paused");
// `?state=asleep`：沒有人在跑 `sister record`。`?state=booting`：有一個起來
// 了，但還在開資料庫——那一格畫面上不是「沒有人在記錄」（那句話配著一顆按下
// 去會失敗的按鈕），而他那顆一年份的資料庫每天早上都會停在這裡好幾分鐘。
// 和上面同一條紀律——走真正的那個旗標，不另外搬一個長得像的樣子出來。
setRecording(
  wanted === "asleep"
    ? "none"
    : wanted === "booting"
      ? "booting"
      : wanted === "thinking"
        ? "thinking"
        : "recording",
);
setState(
  wanted === "paused" || wanted === "asleep" || wanted === "booting" || wanted === "thinking"
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
  invoke("gatekeeper_check").then(paintGatekeeper, () => {});
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
if (params.get("asleep") === "stopped") {
  lastRun = {
    started_at: Date.now() - 4 * 3600 * 1000,
    ended_at: Date.now() - 90 * 60 * 1000,
    why: "你按了停止",
  };
  paint();
} else if (params.get("asleep") === "crashed") {
  lastRun = {
    started_at: Date.now() - 19 * 3600 * 1000,
    ended_at: null,
    why: null,
  };
  paint();
} else if (params.get("asleep") === "nobeat") {
  // 叫了、逾時了、還是沒有心跳。這一句要活得比一次輪詢久（以前它被下一個
  // `paint()` 蓋掉），而它換行、比另外兩句長——版面撐不撐得住要用眼睛看。
  noticeAboutHer(
    "等了 25 秒還沒有心跳。record.log 最後說：\n" +
      "（這一輪還沒寫出東西，以下是上一輪的 record.log）\n" +
      "第一張同意書還沒簽——她不會開始記錄。",
  );
  paint();
}

if (params.get("hits") === "demo") {
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
  );
}

// `?hits=recent` 是「剛剛發生什麼事」那條路的版面。分開一個開關而不是共用
// 上面那組假資料，是因為要看的正是**兩者長得不一樣**：時間問題多了一句說明，
// 而且答案裡不會有任何一個被標起來的字。
if (params.get("hits") === "recent") {
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

// `?hits=chapters`／`?demo=1`：問「昨天下午在弄什麼」那一版。章節在 facts
// 前面，標題是 app／title，不是一句「你在專心寫程式」。
if (params.get("hits") === "chapters" || params.get("demo") === "1") {
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
if (params.get("hits") === "none") {
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
