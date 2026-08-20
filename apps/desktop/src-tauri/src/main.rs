// 沒有主控台視窗。少了這行，release build 在 Windows 上會多開一個黑框，
// 而她的賣點是「安靜地待在角落」。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 字母人的外殼。
//!
//! 配方照 PHASES.md Phase 1 寫的：透明、置頂、拖曳條、關閉即收進系統匣。
//! 那份配方來自 TokenMonster（Ted 自己的 repo，MIT），但那邊是 Electron——
//! 這裡是 Tauri，於是有一個結構上的差別值得記下來：
//!
//! Electron 的 renderer 是另一個行程、裡面沒有 Rust，所以 TokenMonster 得在
//! 本機開一個 loopback HTTP gateway、發一次性 bootstrap token、換成 HttpOnly
//! cookie，才能讓畫面跟後端說話。**Tauri 不需要那一整套**，因為後端就是這個
//! 行程裡的 Rust。少一個 port、少一個 token、少一個「有人搶走那個 port 之後
//! 會怎樣」的問題。
//!
//! 這是唯一一個「因為換了殼所以整段不抄」的地方，其餘行為都照舊。

use serde::{Deserialize, Serialize};
use sister_shell as bounds;
use sister_shell::{PetState, Rect};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, PhysicalPosition, WindowEvent};

const PET: &str = "pet";
const PET_W: i32 = 340;
const PET_H: i32 = 560;

struct Shell {
    state: Mutex<PetState>,
    state_path: PathBuf,
    /// 資料目錄。`None` = 這台機器上問不出使用者的 data dir，那時候暫停鍵
    /// 只能誠實地失敗——一顆按了沒反應、卻假裝有反應的暫停鍵最糟。
    data_dir: Option<PathBuf>,
    /// 資料庫連線。**開得很懶**：她可以在完全沒有資料的機器上開起來，
    /// 使用者按下第一個問題之前不必碰硬碟。
    db: Mutex<Option<sister_core::db::Db>>,
    /// 這個視窗自己開起來的那個 recorder（`None` = 沒開過／已經走了）。
    ///
    /// 「已經有一個在跑了嗎」以前只問心跳，而心跳是 recorder **開機做完之後**
    /// 才蓋的——於是那段開機時間裡它對這道閘門是隱形的，第二下就穿過去了。
    /// 心跳那一頭已經補好（見 `ops::BootBeat`），但那是靠時間差贏的；握著這個
    /// 把手就不必賭：行程還活著就是還活著，跟它寫沒寫檔案無關。
    spawned: Mutex<Option<Spawned>>,
}

/// 我們自己開起來的那個 recorder，**加上它是什麼時候被開起來的**。
struct Spawned {
    child: std::process::Child,
    /// spawn 之前那一刻的牆上時間。
    ///
    /// 「結束」那道落刀的閘門靠它分辨心跳檔上那一行是**上一場留下的**還是
    /// **我這個 child 剛寫的**——見 [`sister_core::heartbeat::safe_to_kill_spawn`]。
    /// 一台錄過東西的機器上那個檔案永遠都在，所以分得出來的只有時戳。
    at: sister_core::Millis,
}

impl Shell {
    fn persist(&self) {
        let snapshot = *self.state.lock().expect("pet state");
        bounds::save(&self.state_path, &snapshot);
    }
}

/// 一筆答案。這是她說話的全部形狀——**每一筆都帶出處**。
///
/// `frame_id` 是「點開看當時那張畫面」的鑰匙，而**這裡的它已經不只是來源**：
/// 資料庫那一欄只講「這段字抄自哪一幀」，送到畫面上之前會過一次
/// [`sister_core::db::Db::frames_with_image`]，沒有照片的一律變回 `None`。
/// 所以 `Some` 就是點得開，`None` 就是點不開——只記字、截圖節流、額度用完、
/// 過了保留期，通通收在同一個 `None` 底下。
#[derive(Serialize)]
struct Hit {
    /// 題庫要靠它記下「他點開的是哪一筆」（見 `log_click`）。畫面上不顯示。
    chunk_id: i64,
    ts: i64,
    text: String,
    snippet: String,
    app: Option<String>,
    title: Option<String>,
    url: Option<String>,
    frame_id: Option<i64>,
}

#[tauri::command]
fn toggle_pin(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) -> bool {
    let pinned = {
        let mut state = shell.state.lock().expect("pet state");
        state.pinned = !state.pinned;
        state.pinned
    };
    if let Some(win) = app.get_webview_window(PET) {
        let _ = win.set_always_on_top(pinned);
    }
    shell.persist();
    pinned
}

/// 她現在有沒有在看。
///
/// 每次都去讀磁碟，**不快取在這個行程裡**：按下暫停的可能是系統匣、可能是
/// 上一次開機、也可能是使用者自己去刪了那個檔案。這個視窗只是一面鏡子，
/// 真相在 data dir 裡。
#[tauri::command]
fn pause_state(shell: tauri::State<'_, Shell>) -> bool {
    match &shell.data_dir {
        Some(dir) => sister_core::pause::is_paused(dir),
        // 問不出資料目錄 = 不知道她在做什麼。按照 `pause` 模組的規則，
        // 不確定就報暫停——寧可顯示得比實際保守。
        None => true,
    }
}

/// 現在到底有沒有人在錄。
///
/// 這和暫停是**兩個不同的問題**，而字母人以前只問得出後者：暫停鍵沒被按下
/// 的時候它就顯示「在聽」——即使根本沒有人把 `sister record` 跑起來。那是這個
/// 產品唯一不能說的那種謊：使用者照著那三個字相信她記得住今天，然後某天問
/// 「剛剛發生什麼事」，得到一片空白。
///
/// 判斷靠 recorder 每 5 秒蓋一次的時戳（見 [`sister_core::heartbeat`]），
/// 不靠 `sessions.ended_at`——那一列在 recorder 當掉的時候永遠停在 NULL。
///
/// # 為什麼回三個字串
///
/// 上一版回 `is_recording` 那個布林，而**它把「正在起來」歸進「沒有人在
/// 錄」**——於是 `app.js` 那個等她起來的迴圈（`startRecording`）在一顆一年份
/// 的資料庫上一定逾時：`Db::open` 要跑好幾分鐘，那 25 秒裡 `is_recording` 從
/// 頭到尾是 false，畫面說「等了 25 秒還沒有心跳」。**而心跳從第一秒就在**
/// （`ops::BootBeat` 在開資料庫之前先蓋一次），那段註解自己也是這樣寫的。
/// 同一顆按鈕在系統匣上更難看：那邊看到 false 就去 `start_recording`，而那一
/// 支的第一道閘門是 `is_occupied`——按下去只會回一句「已經有一個 sister
/// record 在跑了」。
///
/// 「她在錄嗎」和「有人佔著這個目錄嗎」是兩個問題（`heartbeat.rs` 開頭那段就
/// 是在講這件事），而一個布林只答得出一個。
#[tauri::command]
fn recording_state(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) -> String {
    let now = match shell
        .data_dir
        .as_ref()
        .and_then(|dir| sister_core::heartbeat::phase(dir, sister_core::now_ms()))
    {
        Some(sister_core::heartbeat::Phase::Recording) => "recording",
        Some(sister_core::heartbeat::Phase::Booting) => "booting",
        // 問不出資料目錄的時候，`pause_state` 回報「暫停」是為了少錄；
        // 這裡回報「沒在錄」是為了少吹牛。同一個方向：不確定就往
        // 「她做得比較少」那邊倒。
        None => "none",
    };
    // 順手把系統匣那兩顆的字改對——手上已經有答案了，不必再讀一次磁碟。這不是
    // 唯一的刷新時機（見 [`refresh_tray`]），是最即時的那一個：視窗開著的時候，
    // 選單和字母人講的是同一秒的事。
    //
    // 那兩行字問的是「按下去會發生什麼」，不是「她在錄嗎」——正在起來的那一個
    // 也停得掉，也會被「結束」帶走，所以兩種都算佔著。
    set_record_labels(&app, now != "none");
    now.to_string()
}

/// 系統匣那三行字：全部重新去問一次磁碟。
///
/// 三個項目講的都是「她現在在幹嘛」，而三個都會過期：
///
/// * 「開始記錄／停止記錄」和「結束（記錄也會停）」以前只在 [`recording_state`]
///   被呼叫時才刷，而那是**畫面**每 5 秒問一次的——`visibilitychange` 一關就
///   停（見 `app.js` 的 `updatePollGate`）。也就是說**視窗一收進系統匣，系統匣
///   的字就凍住了**，而那正是它變成唯一介面的那一刻。收起來之後 recorder 被
///   Ctrl+C 掉，選單還寫著「結束（記錄也會停）」——他讀到的是「她在錄」。
/// * 「暫停記錄／繼續記錄」過期得更早：它只在 [`announce_pause`] 裡改，而那
///   只有**這個行程自己**按下去才會走到。終端機裡一句 `sister pause` 之後，
///   選單永遠寫著「暫停記錄」，按下去等於把她放出來。
///
/// 所以刷新得由這一端自己排（見 `main` 裡那條 [`TRAY_REFRESH`]），不是搭畫面
/// 的便車——搭便車的東西會在那個人走掉的時候一起停。
fn refresh_tray(app: &tauri::AppHandle) {
    let Some(shell) = app.try_state::<Shell>() else {
        return;
    };
    let (occupied, paused) = match &shell.data_dir {
        Some(dir) => (
            // `is_occupied` 不是 `is_recording`：這兩行字問的是「按下去會發生
            // 什麼」。理由在 [`recording_state`] 上面。
            sister_core::heartbeat::is_occupied(dir, sister_core::now_ms()),
            sister_core::pause::is_paused(dir),
        ),
        // 問不出資料目錄的時候，和 `recording_state` / `pause_state` 倒向同一邊：
        // 不確定就往「她做得比較少」那邊講。
        None => (false, true),
    };
    set_record_labels(app, occupied);
    if let Some(item) = app.try_state::<PauseItem>() {
        let _ = item.0.set_text(pause_label(paused));
    }
}

/// 「開始／停止記錄」和「結束」那兩行字。分出來是因為 [`recording_state`] 手上
/// 已經有答案了，不必為了改字再讀一次磁碟。
///
/// 收的是**佔不佔著這個目錄**，不是「在不在錄」：正在起來的那一個停得掉，也會
/// 被「結束」帶走，而寫著「開始記錄」的那一顆在那幾分鐘按下去只會回一句「已經
/// 有一個 sister record 在跑了」。
fn set_record_labels(app: &tauri::AppHandle, occupied: bool) {
    if let Some(item) = app.try_state::<RecordItem>() {
        let _ = item.0.set_text(record_label(occupied));
    }
    if let Some(item) = app.try_state::<QuitItem>() {
        let _ = item.0.set_text(quit_label(occupied));
    }
}

/// 系統匣刷字的節奏。
///
/// 跟著心跳走（`heartbeat::BEAT_EVERY_MS` 是 5 秒）。更慢的話，「她剛剛當掉了」
/// 和「選單還在說她活著」之間會有一段看得到的空窗。成本是每 5 秒兩個小檔案的
/// 讀取——和畫面開著時本來就在做的事一樣，只是現在收起來也做。
const TRAY_REFRESH: std::time::Duration = std::time::Duration::from_secs(5);

/// 上一場錄製是什麼時候、為什麼結束的。
///
/// 「沒有人在記錄」永遠帶著下一個問題：那她是什麼時候停的、為什麼停的。
/// 答不出來的話，那句灰字讀起來就像故障——而「你自己按了停止」和「同意書
/// 被撤回」的下一步完全不同。
///
/// 只在她沒在錄的時候問（見畫面那一邊）：正在錄的時候，最後一場就是這一場，
/// 而它還沒有結束。
///
/// 時戳直接給出去，不在這裡排版：時區是畫面那一邊的東西，而它已經有一個
/// `when()` 在用同一種格式印出處了（時間軸那條 `tz_offset_ms` 是同一條紀律）。
#[tauri::command(async)]
fn last_recording_end(shell: tauri::State<'_, Shell>) -> Option<LastRun> {
    // 資料庫還沒有／打不開的時候回 `None`。那句灰字自己站得住，這裡不該為了
    // 補一行字而把「她還沒錄過任何東西」講成一個錯誤。
    //
    // `None` 現在還有第三種來源：他把整段時間忘掉了，那幾場的紀錄跟著走了
    // （`retention::delete_empty_sessions`）。畫面那邊照樣只是少講一句
    // 「上一次幾點停的」——少講不等於講錯，而「她錄過」那件事有
    // [`has_ever_recorded`] 專門在答，時間軸就是拿它畫那一頁的。
    with_db(&shell, |db| {
        Ok(db
            .last_session()
            .map_err(|e| format!("{e:#}"))?
            .map(|s| LastRun {
                started_at: s.started_at,
                ended_at: s.ended_at,
                why: s
                    .reason
                    .as_deref()
                    .map(|r| sister_core::model::EndReason::describe(r).to_string()),
                // 沒有理由**而且**那一場的事件全被清掉了：理由本來很可能寫過
                // ，是後來被保留期或 `sister forget` 帶走的。畫面上要講成
                // 「查不出來了」，不是「那時候還沒在記」——見
                // `LastSession::events_left`。
                why_gone: s.reason.is_none() && s.events_left == 0,
            }))
    })
    .ok()
    .flatten()
}

/// 她**曾經**開始記過東西嗎。
///
/// 和 [`last_recording_end`] 分開，因為那一支答的是「上一場長什麼樣」，而
/// 一場錄製的紀錄現在會跟著它記下來的東西一起消失（見
/// `retention::delete_empty_sessions`）。時間軸以前拿它當「她錄過嗎」用，
/// 於是一個把整顆資料庫忘光的人會拿到「她還沒記得任何東西。跑 sister record
/// 之後再回來看。」——叫他重做一件他剛剛才故意做掉的事。
///
/// 反過來也不能拿它當「她錄過嗎」：最後一場**當掉**的話，那一列撐得過
/// `forget`（那道守衛不准碰還沒收尾的最新一列，因為那可能是此刻正在錄的那一
/// 場），於是同一顆被清空的資料庫會因為「上一場有沒有當掉」給出兩種答案。
///
/// 這一支問的是 `meta` 裡那個位元：沒有時間、沒有長度，忘不掉也重建不出東西。
#[tauri::command(async)]
fn has_ever_recorded(shell: tauri::State<'_, Shell>) -> bool {
    with_db(&shell, |db| db.ever_recorded().map_err(|e| format!("{e:#}")))
        .unwrap_or(false)
}

/// 她有沒有**真的存下來過一列內容**。
///
/// 上面那一支答不出這一題，而時間軸那一頁需要的正是這一題：它拿
/// `has_ever_recorded` 當「這些紀錄是被忘掉的」的根據，可是那個旗標在
/// `start_session` 就翻成 true，**第一張畫面之前**。於是一台
/// `capture.enabled = false` 的機器——她跑完、一個字都沒記到、`sister forget`
/// 從來沒有被執行過——會在那一頁上讀到「這些紀錄是被忘掉的，或是過了保留
/// 期」。那是指控一件沒發生的事。
///
/// 兩支分開而不是合成一個結構回去：`has_ever_recorded` 已經有呼叫端和假後端
/// 在用，而這兩個位元各自都有單獨成立的意思。見
/// [`sister_core::db::Db::ever_stored`]。
#[tauri::command(async)]
fn has_ever_stored(shell: tauri::State<'_, Shell>) -> bool {
    with_db(&shell, |db| db.ever_stored().map_err(|e| format!("{e:#}"))).unwrap_or(false)
}

/// 上一場錄製（見 [`last_recording_end`]）。
#[derive(Serialize)]
struct LastRun {
    started_at: i64,
    /// `None` = 沒有好好結束。問這支命令的前提是「現在沒有心跳」，所以
    /// `Db::last_session` 上那個「還是它正在跑」的歧義在這裡已經被排除了。
    ended_at: Option<i64>,
    /// 已經翻成人話的理由。`None` 有兩種意思，靠 [`why_gone`](Self::why_gone) 分。
    why: Option<String>,
    /// `why` 是 `None` 的原因是**紀錄被清掉了**，不是那一版沒在記。
    why_gone: bool,
}

fn record_label(recording: bool) -> &'static str {
    if recording {
        "停止記錄"
    } else {
        "開始記錄"
    }
}

/// 「結束」在正在錄的時候要把後果講出來。
///
/// 結束會**連 recorder 一起停**（見系統匣的 `"quit"`）。不停的話，他關掉的是
/// 唯一看得見的那個視窗，而螢幕還在被記錄——那是上一版剛修掉的那個謊反過來
/// 講一次，而反過來的這一版更糟：前者是少記了，後者是在他以為已經關掉之後
/// 繼續記。
fn quit_label(recording: bool) -> &'static str {
    if recording {
        "結束（記錄也會停）"
    } else {
        "結束"
    }
}

/// 系統匣裡的那一顆開始／停止。理由和 [`PauseItem`] 一樣：一個永遠寫著同一句
/// 話的切換項目，會讓人按出他沒想要的那個方向。
struct RecordItem(MenuItem<tauri::Wry>);

/// 系統匣裡的「結束」。存起來的理由見 [`quit_label`]。
struct QuitItem(MenuItem<tauri::Wry>);

/// `sister.exe` 在哪裡。
///
/// 和 `sister-desktop.exe` 同一個資料夾——release 的 zip 裡兩個檔案就是一起
/// 解出來的。**不去 `PATH` 裡找**：使用者多半沒把它加進去，而在 `PATH` 上撿到
/// 另一個版本的 sister（舊的 alpha、別的資料目錄）比找不到更糟——那會是一場
/// 沒有人知道自己在跑哪個版本的錄製。
fn recorder_path() -> Result<PathBuf, String> {
    let me = std::env::current_exe().map_err(|e| format!("問不出自己在哪裡：{e}"))?;
    let dir = me
        .parent()
        .ok_or_else(|| "問不出自己在哪個資料夾".to_string())?;
    let name = if cfg!(windows) {
        "sister.exe"
    } else {
        "sister"
    };
    let path = dir.join(name);
    match path.try_exists() {
        Ok(true) => Ok(path),
        _ => Err(format!(
            "找不到 {name}——它應該和 sister-desktop 放在同一個資料夾（{}）",
            dir.display()
        )),
    }
}

/// 把 recorder 跑起來。
///
/// 字母人在上一版學會了說「沒有人在記錄」，但說完之後使用者唯一的下一步是
/// 開一個終端機、找到 `sister.exe`、打一行指令。而 Phase 1 的退場條件是
/// 「自用 7 天」——一個每天早上都要開終端機的東西撐不到第七天，那條退場
/// 條件就永遠量不到。
///
/// 用**另一個行程**而不是把錄製迴圈搬進來，是刻意的：擷取那條路會長時間佔著
/// CPU、會碰 UIA、會 OCR，而它當掉的時候不該把使用者的問答視窗一起帶走。
/// 「一個記、一個問」本來就是這兩個執行檔的分工。
#[tauri::command(async)]
fn start_recording(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，開不起來".to_string())?;
    if sister_core::heartbeat::is_occupied(dir, sister_core::now_ms()) {
        // 不是錯誤，但也不能安靜地再開一個：兩個 recorder 會各自錄一份，
        // 而使用者只會看到磁碟用得比講好的快一倍。
        return Err("已經有一個 sister record 在跑了".into());
    }
    // 心跳還沒出現，不代表沒有人在起來。上一下按出去的那個行程可能正卡在
    // `Db::open` 的 migration 上——它還沒蓋出第一個心跳，所以上面那道閘門
    // 看不見它。**問行程，不要問它寫的檔案**：這是唯一一條不用賭時間差的路。
    {
        let mut spawned = shell.spawned.lock().expect("spawned recorder");
        match spawned.as_mut().map(|s| s.child.try_wait()) {
            // 還在跑（`Ok(None)` = 沒退出）。
            Some(Ok(None)) => {
                return Err("上一次按的那個還在起來——第一次開資料庫要重建索引，\
                            大的資料庫可能要幾分鐘。再等一下"
                    .into());
            }
            // 已經走了，或者連問都問不到（handle 壞了）。清掉再開新的。
            _ => *spawned = None,
        }
    }
    // 同意書那道閘門在 `sister record` 裡面，而我們等一下就要把它的視窗藏起來
    // ——它印出來的拒絕理由**沒有人看得到**。在這裡先問一次同一個問題，那句
    // 話才有地方顯示；不然按下去的結果是「閃一下，然後什麼都沒發生」。
    if !sister_core::consent::load(dir).allows_recording() {
        // 指路要指得到。這個視窗上沒有 ⚙（只有 ⏸ ▤ ● −），而同意書也不在
        // 設定頁上——設定頁管的是排除規則、保留天數那些。三張同意書是系統匣
        // 選單裡自己的一頁。指去一個不存在的按鈕，比不指路更糟。
        return Err(
            "第一張同意書還沒簽——她不會開始記錄。\
             在系統匣圖示上按右鍵，選「三張同意書…」簽好再回來"
                .into(),
        );
    }
    let exe = recorder_path()?;
    // 上一次在沒有 recorder 的時候按下的「停止」會留在磁碟上，而那會讓這一場
    // 在第一個 tick 就自己結束。recorder 自己也清一次（在 `BootBeat::start`
    // 裡，開機窗打開的那一刻——**不是**在 `Db::open` 之後；那一版會把他在開機
    // 那幾分鐘按的停止刪掉），這裡再清是因為下一行就是 spawn——清的成本是一次
    // unlink，漏掉的代價是「按了沒反應」。
    //
    // 兩次清理中間夾著一次 spawn，那是幾毫秒的窗；在那之內按停止仍然會被吃
    // 掉。和以前那個「一顆一年份的資料庫要開好幾分鐘」的窗差了五個數量級，
    // 而且那幾毫秒裡畫面上還寫著「正在叫她起來」，沒有停止鍵。
    sister_core::control::clear_stop(dir);

    // 它的 stdout 沒有終端機可以去。丟掉的話，「為什麼她開了三秒就不見了」
    // 永遠問不出答案——和 desktop.log 同一個理由、同一個作法。
    let out = start_log_at(dir, "record.log").ok_or_else(|| "寫不出 record.log".to_string())?;
    let err = out
        .try_clone()
        .map_err(|e| format!("寫不出 record.log：{e}"))?;

    let mut cmd = std::process::Command::new(&exe);
    // 明講 `--data-dir` 而不是讓它自己算：兩邊各算一次的話，有一天它們會算出
    // 不一樣的答案，而症狀是「她說她在錄，但問什麼都查不到」。
    cmd.arg("--data-dir")
        .arg(dir)
        .arg("record")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(out))
        .stderr(std::process::Stdio::from(err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW。少了它，每次按「開始記錄」都會彈出一個黑色主控台
        // ——而那個視窗被關掉就等於 recorder 被殺掉，使用者不會知道那兩件事
        // 是同一件事。
        cmd.creation_flags(0x0800_0000);
    }
    // **在 spawn 之前讀鐘。** 讀在後面的話，child 有機會在這兩行之間就蓋出第
    // 一拍，於是那一拍的時戳比 `at` 還小，而「結束」那道閘門會把它讀成「上一
    // 場留下的」然後放行落刀——她已經開著資料庫了。往前讀最壞只是多不敢砍幾
    // 毫秒；往後讀最壞是砍掉一場正在錄的。
    let at = sister_core::now_ms();
    let child = cmd
        .spawn()
        .map_err(|e| format!("{} 起不來：{e}", exe.display()))?;
    // 握著它。不握的話，下一下按進來的時候我們只剩心跳可以問，而開機那幾分鐘
    // 心跳還不在。順便：`Child` 被 drop 不會殺掉行程，所以她活得比這個視窗久
    // ——那是刻意的，`stop_recording` 用的是檔案而不是 kill。
    *shell.spawned.lock().expect("spawned recorder") = Some(Spawned { child, at });
    tracing::info!("把 recorder 開起來了：{}", exe.display());
    Ok(())
}

/// 請 recorder 收工。
///
/// 寫一個檔案，不去 kill 那個行程——理由寫在 [`sister_core::control`]：
/// `TerminateProcess` 會讓她死在半路，留下一筆永遠不會結束的 session
/// 和一個還在說「我在錄」的心跳檔。
#[tauri::command]
fn stop_recording(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，停不了".to_string())?;
    sister_core::control::request_stop(dir).map_err(|e| format!("{e:#}"))?;
    tracing::info!("請 recorder 收工");
    Ok(())
}

/// recorder 最後說的那幾句話。
///
/// 按了「開始記錄」卻沒有起來的時候，理由已經寫在 `record.log` 裡了——但那個
/// 檔案在 `%APPDATA%` 深處，而正在看著一個沒反應的按鈕的人不會去翻它。把最後
/// 幾行直接端到畫面上，「按了沒反應」才會變成一句看得懂的話。
/// `record.log` 是**按下去的那一刻**才建的，而建之前 [`start_log_at`] 會把上
/// 一輪改名成 `.1`。所以「按了沒起來、再按一次」的第二下，唯一寫著原因的那一份
/// 已經變成 `.1`，新開的那一份是空的——以前這裡只讀新的那一份，於是畫面說
/// 「沒有留下任何理由」，而理由就躺在它旁邊。
#[tauri::command(async)]
fn recorder_log_tail(shell: tauri::State<'_, Shell>) -> String {
    const LINES: usize = 6;
    let Some(dir) = shell.data_dir.as_ref() else {
        return String::new();
    };
    let tail = |name: &str| -> String {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
            return String::new();
        };
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .rev()
            .take(LINES)
            .collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    let now = tail("record.log");
    if !now.is_empty() {
        return now;
    }
    match tail("record.log.1").as_str() {
        "" => String::new(),
        // 講明白這是哪一輪的。不講的話，兩輪前的一句錯誤會被讀成「她剛剛就是
        // 這樣死的」——而那兩件事要做的處置不一樣。
        before => format!("（這一輪還沒寫出東西，以下是上一輪的 record.log）\n{before}"),
    }
}

#[tauri::command]
fn toggle_pause(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) -> Result<bool, String> {
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，暫停鍵沒有作用".to_string())?;
    let next = !sister_core::pause::is_paused(dir);
    sister_core::pause::set_paused(dir, next, sister_core::now_ms())
        .map_err(|e| format!("{e:#}"))?;
    announce_pause(&app, next);
    Ok(next)
}

/// 把新的暫停狀態同時送到視窗和系統匣。
///
/// 兩個地方都要更新，因為兩個地方都能觸發它——只更新自己那一邊的話，
/// 從系統匣暫停之後，視窗裡的字母人會繼續一臉「我在聽」。
fn announce_pause(app: &tauri::AppHandle, paused: bool) {
    use tauri::Emitter;
    let _ = app.emit("pause-changed", paused);
    if let Some(item) = app.try_state::<PauseItem>() {
        let _ = item.0.set_text(pause_label(paused));
    }
}

fn pause_label(paused: bool) -> &'static str {
    if paused {
        "繼續記錄"
    } else {
        "暫停記錄"
    }
}

/// 系統匣裡的那一顆暫停。存起來是為了改它的字——選單上一個永遠寫著
/// 「暫停記錄」的項目，在已經暫停的時候會讓人再按一次，然後把它打開。
struct PauseItem(MenuItem<tauri::Wry>);

#[tauri::command]
fn hide_to_tray(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) {
    if let Some(win) = app.get_webview_window(PET) {
        remember_position(&win, &shell);
        let _ = win.hide();
    }
    shell.persist();
}

/// 借用那顆資料庫，需要的話當場開。
///
/// 開得很懶（她可以在完全沒有資料的機器上啟動），但**每一支要讀資料的命令都
/// 得走這裡**。在這之前只有 `ask` 會開：於是先點開時間軸、還沒問過任何問題的
/// 那條路上，看圖那支會回「資料庫還沒開」——一句只有寫程式的人看得懂、而且
/// 根本不是真正原因的錯誤訊息。多一個入口就多一條這種路。
///
/// **叫得動這裡的命令一律要標 `#[tauri::command(async)]`。** Tauri 的同步命令
/// 跑在主執行緒上，而這一支的第一次呼叫可能要幾分鐘：`Db::open` 會把還沒跑過
/// 的 migration 跑完，其中 003 要把整張 `text_chunks` 讀出來重算 bigram。主
/// 執行緒卡住的時候整個殼都跟著卡——暫停鍵、系統匣、連拖曳都沒反應，而畫面
/// 停在「想一下…」上，看起來就只是她想很久。這是一個真的發生過的當機畫面，
/// 不是理論。
fn with_db<T>(
    shell: &tauri::State<'_, Shell>,
    f: impl FnOnce(&sister_core::db::Db) -> Result<T, String>,
) -> Result<T, String> {
    let mut slot = shell.db.lock().map_err(|_| "資料庫鎖壞了".to_string())?;
    if slot.is_none() {
        let dir = sister_core::config::Config::default_data_dir()
            .ok_or_else(|| "找不到資料目錄".to_string())?;
        let path = sister_core::config::Config::db_path(&dir);
        // 她還沒錄過任何東西的時候，這裡**不要**建一個空資料庫然後假裝正常。
        // 「我還沒有任何記憶」跟「我查不到」是兩件不同的事，使用者該分得出來。
        if !path.exists() {
            return Err("還沒有任何記憶——先跑 `sister record`".to_string());
        }
        // `{e:#}` 而不是 `to_string()`：anyhow 的 `to_string()` 只給最外面那
        // 一層 context，這裡就是「open C:\…\sister.db」——一句廢話。真正寫給
        // 他看的那幾行（例如「這份資料庫比這個執行檔新」）躺在底下。這一頁
        // 上 26 個 `map_err` 全部同一個理由。
        *slot = Some(sister_core::db::Db::open(&path).map_err(|e| format!("{e:#}"))?);
    }
    f(slot.as_ref().expect("just opened"))
}

/// 同上，但拿得到可變借用。刪東西的那兩支要用這個。
fn with_db_mut<T>(
    shell: &tauri::State<'_, Shell>,
    f: impl FnOnce(&mut sister_core::db::Db) -> Result<T, String>,
) -> Result<T, String> {
    with_db(shell, |_| Ok(()))?;
    let mut slot = shell.db.lock().map_err(|_| "資料庫鎖壞了".to_string())?;
    f(slot.as_mut().expect("with_db opened it"))
}

/// 一次回答。
///
/// `kind` 不是給程式判斷用的，是**要講給他聽的**：他打了「剛剛發生什麼事」，
/// 而她回的東西跟那七個字一個都不像——不說清楚「我把它當成時間問題了」，看起來
/// 就只是她答非所問。
///
/// 判斷放在 core（[`sister_core::question`]），不在這裡也不在畫面上。`sister
/// query` 和這一頁必須對同一句話給同一種答案，各抄一份遲早會變成兩種行為。
#[derive(Serialize)]
struct Answer {
    kind: &'static str,
    /// 她拿去比對的那串字，**但只在它是黏出來的時候**。`None` = 沒什麼好講。
    ///
    /// `question::terms` 會把「剛剛」「那個」剝掉，剝到不足兩個字還會往回退
    /// 一格——而那一格常常退進虛字裡：「剛剛那個板」→「個板」、「剛剛看到的
    /// 人」→「的人」。於是兩種完全不同的處境印出同一句「我記得的東西裡沒有
    /// 這件事」：他打的字真的沒出現過，跟她根本沒找他打的字。有命中的那一半
    /// 更難看出來——「的人」在一年份的螢幕文字裡什麼都比得到，於是他拿到一串
    /// 毫不相干的東西，而唯一讀得出來的意思是「這東西壞了」。
    ///
    /// 前者他無能為力；後者他只要把那個詞重打一次就好。唯一能讓他分辨的，是
    /// 看到她到底拿什麼去比對。
    ///
    /// 和 `sister query --json` 的 `terms` 是同一件事的兩種送法，**不要合成
    /// 一個**：那一份給機器讀（也是 Phase 2 評測語料的來源），所以每一題都要
    /// 有；這一份給人看，每次都報一句只會讓人學會忽略它，所以剝對了就閉嘴，
    /// 只有黏出不是詞的東西才出聲。判斷交給 `terms_with_retreat`——它答的是
    /// 「有沒有退過邊界」，而退邊界是唯一一種黏得出非詞的來源。
    searched: Option<String>,
    /// 這一題在題庫裡的編號。點開出處的時候要掛回來（見 `log_click`）。
    ///
    /// `None` = 沒記成功。畫面那一邊要能在沒有編號的情況下照常運作——記不成
    /// 題庫不該讓一個能回答的問題變成錯誤。
    query_id: Option<i64>,
    /// L1 直接答得出來的那幾筆，排在原文前面。
    ///
    /// 這一層以前只長在 `sister query` 裡：於是同一句「電話」，終端機回得出
    /// 號碼、她只會說找不到——螢幕上寫的是「客服**專線**」，全文比對永遠接不
    /// 起那兩個詞。而她才是他每天真的會用的那一個，Phase 1 的退場條件
    /// （「答對我自己都忘掉的東西」）也是拿她量的。
    answers: Vec<Fact>,
    hits: Vec<Hit>,
    /// 底下還有，只是沒送過來。
    ///
    /// `20 筆` 和「一共就這 20 筆」在畫面上長得一模一樣——終端機那邊為了這件
    /// 事在數字後面印一個 `+`，時間軸為了同一件事多撈一筆來判斷
    /// （見 [`DayView::truncated`]）。她這裡以前什麼都沒說：捲到底就是底了，
    /// 而「她只記得這些」正是他會下的結論。
    ///
    /// 只講原文那一半。★ 那一半在 [`answers_truncated`](Self::answers_truncated)。
    truncated: bool,
    /// ★ 答案也被切掉了。
    ///
    /// 這裡以前的說法是「上限 10 筆是**去重後的不同值**——同一個問題有十個
    /// 不同答案的時候，問題出在問法，不是在少給了第十一個」。那句話讀起來
    /// 很有道理，但它把「她只知道這十個」和「她知道更多、只是沒送過來」壓成
    /// 同一個畫面——而這正是隔壁那一欄存在的全部理由。
    answers_truncated: bool,
    /// 一筆都沒找到的時候，她**查得到**的那幾個理由。兩邊都有東西時是 `None`
    /// ——沒答不出來就沒有什麼好解釋的，而且那幾個查詢不必白跑。
    ///
    /// 她原本說的是「這件事我沒看到過」，一句斷言；而正確答案可能是「你自己
    /// 叫我不要看那個網站」。SPEC §8.2 的語氣規範講的就是這個。
    blind: Option<Blind>,
}

/// [`Answer::blind`] 的內容。核心那份（`sister_core::answer::BlindSpots`）只回
/// 事實，句子由這一頁自己組——終端機和字母人的講法不一樣，根據是同一份。
#[derive(Serialize)]
struct Blind {
    /// 她一共記過幾段文字。`0` **不等於**「還沒開始記」，也**不等於**
    /// 「OCR 沒讀到東西」——見 [`sister_core::answer::BlindSpots::chunks`]。
    chunks: i64,
    /// 留了畫面卻一行字都沒讀出來。
    ///
    /// 送一個**布林**而不是 `ocr_blocks` 的數字：門檻（幾張畫面才算數）是
    /// 核心那邊的判斷，兩邊各寫一次的話遲早有一邊改了另一邊沒改，而這一句
    /// 正好是這個專案已知的主要故障形狀唯一會被講出口的地方。見
    /// [`sister_core::answer::BlindSpots::ocr_is_dead`]。
    ocr_is_dead: bool,
    /// 她一共留下幾張畫面。`chunks == 0 && frames > 0` = 她看了，
    /// 但一個字都沒讀出來（讀字那一段斷了）。
    frames: i64,
    /// 她**曾經**開始記過東西嗎。兩個 0 配上 `true` = 錄過、但被忘掉了，
    /// 不是還沒開始——見 [`sister_core::answer::BlindSpots::ever_recorded`]。
    ever_recorded: bool,
    /// 她有沒有**真的存下來過一列內容**。上面那個位元在 `start_session` 就
    /// 翻成 true，第一張畫面之前——所以一台 `capture.enabled = false` 的機器
    /// 兩個 0 也配得到 `ever_recorded: true`，然後被告知東西被忘掉了。見
    /// [`sister_core::answer::BlindSpots::ever_stored`]。
    ever_stored: bool,
    /// 排除規則生效過的（理由, 段數）。段不是張。
    excluded: Vec<(String, i64)>,
    paused_episodes: i64,
    /// **只含已結束的那幾段。**`0` 配上 `paused_open` 的意思是「三天前按下去
    /// 到現在都沒解除」，不是「暫停了一瞬間」——見
    /// [`sister_core::answer::BlindSpots::paused_open`]。
    paused_ms: i64,
    paused_open: bool,
    /// 她**此刻**閉著眼睛沒有。和 `paused_open` 是兩件事：那一個講的是
    /// 資料庫裡最後一筆暫停有沒有配到解除，而暫停中關掉 recorder、事後才
    /// 解除的人，會永遠掛著一筆配不到的——見
    /// [`sister_core::answer::BlindSpots::paused_now`]。
    paused_now: bool,
    paused_truncated: i64,
    /// 這一題只翻了最近幾天。`null` = 整顆資料庫都翻過了。見
    /// [`sister_core::answer::BlindSpots::scan_horizon_days`]。
    scan_horizon_days: Option<i64>,
    /// 現在有沒有人在錄。只在「一段字都沒有」那組句子裡用得到，而它在那裡
    /// 分開的是「被忘掉了／過期了」和「她三秒前才開起來」——見
    /// [`sister_core::answer::BlindSpots::recording_now`]。
    recording_now: bool,
    /// 有一個 recorder **正在起來**，還沒開始錄。和上面那個是同一次心跳讀出
    /// 來的兩半，永遠不會同時為真——見
    /// [`sister_core::answer::BlindSpots::booting_now`]。
    ///
    /// 少了這一格，開機那幾分鐘字母人會說「先看設定頁的『開始記錄』那一段」，
    /// 對一個什麼都還沒開始的 recorder。
    booting_now: bool,
}

/// 一筆 ★ 答案。
#[derive(Serialize)]
struct Fact {
    /// 正規化後的值——`+886800080123`，不是螢幕上那串 `0800-080-123`。
    value: String,
    /// 螢幕上真正長的樣子。兩個都給：正規化後的值認得出來，原文才認得出**場景**。
    ///
    /// 也是 `value` 難讀時的救生索：金額正規化成 `TWD:13450`，而他記得的是
    /// 這一行裡的「帳單 NT$13,450」。
    raw: String,
    /// 看過幾次。1 次和 12 次是不同強度的答案，而她自己不做判斷，只把數字講出來。
    ///
    /// 數的是**遇到過幾次**，不是資料庫裡有幾列——一列是一張留下來的畫面，
    /// 而盯著同一個視窗二十分鐘會留下三百張。見 `Db::SAME_SITTING_MS`。
    sightings: usize,
    ts: i64,
    /// 這個值是從哪一段字抽出來的。點開出處時要掛回題庫（見 `log_click`）。
    /// `None` 的那些照樣點得開，只是那一下不會被記成正解。
    chunk_id: Option<i64>,
    frame_id: Option<i64>,
    app: Option<String>,
    title: Option<String>,
    url: Option<String>,
}

#[tauri::command(async)]
fn ask(question: String, shell: tauri::State<'_, Shell>) -> Result<Answer, String> {
    use sister_core::question::Shape;
    let question = question.trim().to_string();
    if question.is_empty() {
        return Ok(Answer {
            kind: "keywords",
            searched: None,
            query_id: None,
            answers: Vec::new(),
            hits: Vec::new(),
            // 空字串不是「問了但沒找到」，是根本沒問。
            blind: None,
            truncated: false,
            answers_truncated: false,
        });
    }
    let shape = sister_core::question::shape(&question);
    let started = std::time::Instant::now();
    with_db(&shell, |db| {
        // L1 的事實是「這個值是什麼」，回答不了「剛剛」。時間問題硬跑一次
        // 只會拿電話號碼去回答一個沒有人問號碼的問題——和 `sister query`
        // 同一條分法。
        let facts = match shape {
            Shape::Recent => Default::default(),
            Shape::Keywords => {
                sister_core::answer::answers(db, &question, 10).map_err(|e| format!("{e:#}"))?
            }
        };
        let (facts, facts_truncated) = (facts.items, facts.truncated);
        // 多要一筆，用來判斷「還有沒有」。少了這一步就只能猜——而猜錯的方向
        // 是「剛好滿 20 筆」被當成剛好結束。時間軸那邊同一個寫法。
        const HITS: usize = 20;
        let mut hits = match shape {
            Shape::Recent => db.recent(HITS + 1),
            // 比對用 `terms`（剝掉頭尾的「剛剛」「那個」），★ 答案用原句——
            // 理由和 `sister query` 那邊同一條，寫在那裡。
            Shape::Keywords => db.search(sister_core::question::terms(&question), HITS + 1),
        }
        .map_err(|e| format!("{e:#}"))?;
        let truncated = hits.len() > HITS;
        hits.truncate(HITS);
        // **他打的那句話不進記錄檔。** 只留形狀、幾筆、幾毫秒——這三個數字
        // 足以回答「她是不是又卡住了」，而問題本身是他的東西，不是我的。
        tracing::info!(
            "問了一次（{}）：{} 個答案、{} 筆原文，{} ms",
            if shape == Shape::Recent {
                "時間"
            } else {
                "關鍵字"
            },
            facts.len(),
            hits.len(),
            started.elapsed().as_millis()
        );
        // 進題庫。他打的原話在**資料庫**裡，不在記錄檔裡——記錄檔是我會看的
        // 東西，資料庫是他的。刪得掉（時間軸上那條「忘掉這一段」會一起帶走）、
        // 過得了期（跟著文字的保留期）。理由與代價寫在 DATA_INVENTORY。
        //
        // 記不進去不算失敗：他要的是答案。
        //
        // 每次都重讀設定檔，不快取：他剛在設定頁上把那個勾拿掉，下一個問題就
        // 不該再被記。和暫停旗標同一條紀律——真相在磁碟上，這個行程只是鏡子。
        // 讀不到設定檔就當成不要記（`unwrap_or(false)`）：不確定的時候少存
        // 一點，方向和其他每一個 fail-closed 一致。
        let wanted = config_path()
            .and_then(|p| sister_core::config::Config::load(&p).map_err(|e| format!("{e:#}")))
            .map(|c| c.privacy.query_log)
            .unwrap_or(false);
        let query_id = wanted
            .then(|| {
                db.log_query(&sister_core::db::QueryLogEntry {
                    ts: sister_core::now_ms(),
                    question: &question,
                    shape: shape.name(),
                    // ★ 答案也算——她給了他東西就不是「答不出來」。
                    // 見 `QueryLogEntry::hits`。
                    hits: facts.len() + hits.len(),
                    latency_ms: started.elapsed().as_millis() as i64,
                    source: sister_core::db::SOURCE_DESKTOP,
                })
                .map_err(|e| tracing::warn!("這一題沒記進題庫：{e}"))
                .ok()
            })
            .flatten();
        // 只有兩手空空的時候才去問。有答案的話這幾個 COUNT 是白跑的，而這條
        // 路上使用者正等著看畫面。
        let blind = if facts.is_empty() && hits.is_empty() {
            // 比對用的是 `terms`，掃描界線也照 `terms` 判——理由和
            // `sister query` 那邊同一條。
            let asked = sister_core::question::terms(&question);
            // 不給空路徑當退路：`pause::is_paused` 的規矩是「問不出來就當成
            // 暫停」，而 `Path::new("")` 會讓它去工作目錄找一個不存在的旗標、
            // 然後回一個很有把握的「沒有暫停」。寧可這一段沒有理由可講。
            let dir = shell
                .data_dir
                .as_deref()
                .ok_or_else(|| "找不到資料目錄".to_string())?;
            let b =
                sister_core::answer::blind_spots(db, dir, asked).map_err(|e| format!("{e:#}"))?;
            Some(Blind {
                chunks: b.chunks,
                ocr_is_dead: b.ocr_is_dead(),
                frames: b.frames,
                ever_recorded: b.ever_recorded,
                ever_stored: b.ever_stored,
                excluded: b.excluded,
                paused_episodes: b.paused_episodes,
                paused_ms: b.paused_ms,
                paused_open: b.paused_open,
                paused_now: b.paused_now,
                paused_truncated: b.paused_truncated,
                scan_horizon_days: b.scan_horizon_days,
                recording_now: b.recording_now,
                booting_now: b.booting_now,
            })
        } else {
            None
        };
        // 出處的 `frame_id` 一路上都只回答「這段字抄自哪一幀」——那是來源，
        // 一直都在。畫面上那個「點開看當時的畫面」問的卻是另一件事：那一幀
        // 有沒有留下照片。整份答案畫完問一次，沒有照片的就把鑰匙收回去。
        let openable = {
            let ids: Vec<i64> = hits
                .iter()
                .filter_map(|h| h.frame_id)
                .chain(facts.iter().filter_map(|a| a.latest.frame_id))
                .collect();
            db.frames_with_image(&ids).map_err(|e| format!("{e:#}"))?
        };
        Ok(Answer {
            kind: shape.name(),
            // 只在**不一樣**的時候送。一樣的時候送過去，畫面那邊還要再比一次，
            // 而「這兩串字算不算同一句」是這裡才知道的事（`terms` 回的是原句的
            // 一個切片）。`Shape::Recent` 根本沒走比對那條路，所以也不送。
            searched: match shape {
                Shape::Recent => None,
                Shape::Keywords => {
                    // 只在**黏過**的時候送。剝掉「剛剛那個」留下「優惠方案」是
                    // 剝對了，每次都報一句只會讓人學會忽略它；黏出「個板」才是
                    // 她找了一個不是詞的東西。
                    let (t, glued) = sister_core::question::terms_with_retreat(&question);
                    glued.then(|| t.to_string())
                }
            },
            query_id,
            blind,
            truncated,
            answers_truncated: facts_truncated,
            answers: facts
                .into_iter()
                .map(|a| Fact {
                    value: a.latest.normalized,
                    raw: a.latest.raw,
                    sightings: a.sightings,
                    ts: a.latest.ts,
                    chunk_id: a.latest.chunk_id,
                    frame_id: a.latest.frame_id.filter(|id| openable.contains(id)),
                    app: a.latest.app_id,
                    title: a.latest.window_title,
                    url: a.latest.url,
                })
                .collect(),
            hits: hits
                .into_iter()
                .map(|h| Hit {
                    chunk_id: h.chunk_id,
                    ts: h.ts,
                    text: h.text,
                    snippet: h.snippet,
                    app: h.app_id,
                    title: h.window_title,
                    url: h.url,
                    frame_id: h.frame_id.filter(|id| openable.contains(id)),
                })
                .collect(),
        })
    })
}

/// 時間軸上的一天。
#[derive(Serialize)]
struct Day {
    start_ts: i64,
    chunks: i64,
    first_ts: i64,
    last_ts: i64,
}

/// 哪幾天她其實有在看。
///
/// `tz_offset_ms` 由視窗傳進來（`-new Date().getTimezoneOffset() * 60000`），
/// 不是在 Rust 這邊算的：core 刻意不認識時區，而畫面本來就要用同一個偏移量把
/// 日期印出來——兩邊各算一次遲早會在日光節約時間那天對不起來。
///
/// 收到之後仍然夾一次。合法範圍是 UTC−12 到 UTC+14，超出去的值只可能來自
/// 前端的 bug，而一個離譜的偏移量會把「一天」切在莫名其妙的地方，然後看起來
/// 只是資料很怪。
#[tauri::command(async)]
fn timeline_days(tz_offset_ms: i64, shell: tauri::State<'_, Shell>) -> Result<Vec<Day>, String> {
    const H: i64 = 3_600_000;
    let tz = tz_offset_ms.clamp(-12 * H, 14 * H);
    with_db(&shell, |db| {
        Ok(db
            .days_with_data(tz)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|d| Day {
                start_ts: d.start_ts,
                chunks: d.chunks,
                first_ts: d.first_ts,
                last_ts: d.last_ts,
            })
            .collect())
    })
}

/// 時間軸上的一格。
#[derive(Serialize)]
struct Moment {
    ts: i64,
    app: Option<String>,
    title: Option<String>,
    url: Option<String>,
    text: String,
    /// `None` = 字還在，但沒有畫面可以給他看。同 [`Hit::frame_id`]。
    frame_id: Option<i64>,
}

/// 她閉眼的一段。兩端都可以是 `None`，見 [`sister_core::db::PauseSpan`]。
#[derive(Serialize)]
struct Gap {
    from: Option<i64>,
    to: Option<i64>,
}

/// 一天的內容。
#[derive(Serialize)]
struct DayView {
    moments: Vec<Moment>,
    /// 這一天她被關掉的那幾段。**沒有這個欄位的時間軸會說謊**：一片空白到底
    /// 是他去開會了，還是她被按了暫停，在畫面上長得一模一樣。
    pauses: Vec<Gap>,
    /// 這一天還有更多沒送過來。安靜地截斷會讓一整天看起來比實際短。
    truncated: bool,
}

/// 一天裡她看到的東西。
#[tauri::command(async)]
fn timeline_moments(
    from_ts: i64,
    to_ts: i64,
    limit: usize,
    shell: tauri::State<'_, Shell>,
) -> Result<DayView, String> {
    // 上限再夾一次：前端要多少就給多少的話，一個算錯的日期範圍會把一整年
    // 的文字塞進 webview，然後那個視窗就沒了。
    let limit = limit.clamp(1, 2_000);
    with_db(&shell, |db| {
        // 多要一筆，用來判斷「還有沒有」。少了這一步就只能猜——而猜錯的方向
        // 是「剛好滿 limit 筆」被當成剛好結束。
        let mut rows = db
            .timeline(from_ts, to_ts, limit + 1)
            .map_err(|e| format!("{e:#}"))?;
        let truncated = rows.len() > limit;
        rows.truncate(limit);

        let pauses = db
            .pause_spans(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|s| Gap {
                from: s.from,
                to: s.to,
            })
            .collect();

        // 同 `ask`：來源一直都在，照片不一定。點得開的才給鑰匙。
        let openable = {
            let ids: Vec<i64> = rows.iter().filter_map(|m| m.frame_id).collect();
            db.frames_with_image(&ids).map_err(|e| format!("{e:#}"))?
        };
        Ok(DayView {
            moments: rows
                .into_iter()
                .map(|m| Moment {
                    ts: m.ts,
                    app: m.app,
                    title: m.title,
                    url: m.url,
                    text: m.text,
                    frame_id: m.frame_id.filter(|id| openable.contains(id)),
                })
                .collect(),
            pauses,
            truncated,
        })
    })
}

/// 設定頁上看得到、改得動的那幾項。
///
/// **刻意只是設定檔的一個子集。** 截圖間隔、去重門檻那些沒有放進來，因為它們
/// 改了要重開 `record` 才生效（見 `Recorder::set_privacy`）——一個按了儲存卻
/// 要等重開才生效、而且沒說的欄位，比沒有那個欄位更糟。
#[derive(Serialize, Deserialize)]
struct Settings {
    excluded_apps: Vec<String>,
    excluded_urls: Vec<String>,
    excluded_titles: Vec<String>,
    pause_on_screenshare: bool,
    redact_clipboard_secrets: bool,
    query_log: bool,
    frames_days: u32,
    text_days: u32,
    /// 設定檔實際的位置。給人看的——她說她存到哪，就要指得出來是哪一個檔案。
    ///
    /// **只出不進**：存檔時路徑一律由 `config_path()` 重算，不是相信視窗傳回來
    /// 的那個字串。一個「要寫到哪個檔案」由前端決定的介面，等於讓那一頁指到
    /// 任何一個地方去。`default` 讓存檔的 payload 不必回傳它。
    #[serde(default)]
    path: String,
}

fn config_path() -> Result<PathBuf, String> {
    sister_core::config::Config::default_path().ok_or_else(|| "找不到設定檔路徑".to_string())
}

#[tauri::command]
fn settings_read() -> Result<Settings, String> {
    let path = config_path()?;
    let c = sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}"))?;
    Ok(Settings {
        excluded_apps: c.privacy.excluded_apps,
        excluded_urls: c.privacy.excluded_urls,
        excluded_titles: c.privacy.excluded_titles,
        pause_on_screenshare: c.privacy.pause_on_screenshare,
        redact_clipboard_secrets: c.privacy.redact_clipboard_secrets,
        query_log: c.privacy.query_log,
        frames_days: c.retention.frames_days,
        text_days: c.retention.text_days,
        path: path.display().to_string(),
    })
}

/// 存完之後，「她什麼時候會照這份跑」的答案。
///
/// 存好之後那句話以前寫死是「正在跑的 record 會在 5 秒內換上這一份」。那句
/// 話有一個沒被檢查的前提：**真的有一個 record 在跑**。一個剛裝好、還沒按過
/// 「開始記錄」的人，看到的是一句承諾一件不會發生的事的話——他改完排除規則，
/// 以為門從此關著，然後才去按開始（那時候倒是真的生效了），或者根本不去按。
///
/// 而更難看的是反過來：他**以為**她在錄（工作管理員裡有那個行程、系統匣有
/// 圖示），實際上那個行程幾分鐘前就掛了。這句話會替那件事背書。
#[derive(Serialize)]
struct WriteOutcome {
    /// 心跳現在說什麼：`"recording"`／`"booting"`／`"none"`。決定那句話怎麼講。
    ///
    /// **三個值，不是一個布林。** 上一版是 `recording: bool`（`is_recording`），
    /// 於是開機那幾分鐘這一頁說「現在沒有人在錄，所以這一份要等你按下**開始
    /// 記錄**才會生效」——而那顆按鈕在那幾分鐘按下去只會回一句「已經有一個
    /// sister record 在跑了」（見 [`start_recording`] 那道 `is_occupied` 閘
    /// 門）。一句在他剛改完排除規則的那一刻、指著一條走不通的路的話。
    watching: &'static str,
}

#[tauri::command]
fn settings_write(
    settings: Settings,
    shell: tauri::State<'_, Shell>,
) -> Result<WriteOutcome, String> {
    let path = config_path()?;
    // **先讀再改再寫**，不是從空白組一份出來。設定檔裡有這一頁沒有畫出來的
    // 欄位（截圖間隔、每日畫面額度……），從頭組一份會把它們全部重設成預設值
    // ——使用者只是改了個保留天數，磁碟預算卻被悄悄換掉了。
    let mut c = sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}"))?;
    c.privacy.excluded_apps = settings.excluded_apps;
    c.privacy.excluded_urls = settings.excluded_urls;
    c.privacy.excluded_titles = settings.excluded_titles;
    c.privacy.pause_on_screenshare = settings.pause_on_screenshare;
    c.privacy.redact_clipboard_secrets = settings.redact_clipboard_secrets;
    c.privacy.query_log = settings.query_log;
    c.retention.frames_days = settings.frames_days;
    c.retention.text_days = settings.text_days;
    c.save(&path).map_err(|e| format!("{e:#}"))?;
    // 存成功之後才問。反過來的話，一個存不進去的檔案會拿到一句「5 秒內換上」。
    Ok(WriteOutcome {
        watching: match shell
            .data_dir
            .as_ref()
            .and_then(|dir| sister_core::heartbeat::phase(dir, sister_core::now_ms()))
        {
            Some(sister_core::heartbeat::Phase::Recording) => "recording",
            Some(sister_core::heartbeat::Phase::Booting) => "booting",
            None => "none",
        },
    })
}

/// 這台機器上，這幾條規則到底生不生效。
///
/// [`lint_url_rules`] 檢查的是**一條規則自己**寫得對不對。這一支檢查的是另一
/// 件事：整組規則會不會因為機器讀不到網址而**一條都不生效**。兩者都通過的
/// 使用者，看到的是一份綠色的清單和一個以為關上了的門。
///
/// 探測是 recorder 做的（只有它有 UIA），所以這裡讀的是它留下來的報告，而且
/// 拿現在這一刻的設定重算——見 [`sister_core::capabilities`]。
#[derive(Serialize)]
struct PrivacyHealth {
    /// 現在這份設定裡，有哪幾件事其實沒在做。空的 = 都生效。
    ///
    /// 每一則都帶著 `about`，因為它們該掛在這一頁上**不同的區塊**底下——
    /// 見 [`sister_core::capabilities::About`]。
    broken: Vec<sister_core::capabilities::Broken>,
    /// 報告是什麼時候探的。`None` = **還沒有報告**（沒錄過、或那個檔案被刪
    /// 了）——那和「都生效」是兩件事，畫面上不准長得一樣。
    at: Option<i64>,
    /// `capture.enabled = false`：她連開始都不會開始。
    ///
    /// 這一格問的是「這幾條規則生不生效」，而總開關關著的時候那個問題沒有
    /// 意義——**一條都不會被用到，因為根本沒有畫面進來**。而它在畫面上和
    /// 「一切正常」長得一模一樣：規則清單是綠的、這一格是空的。使用者以為
    /// 他設好了一台會記錄、而且會避開網銀的機器；他有的是一台什麼都不記的。
    ///
    /// 這個欄位不能塞進 `broken`：那個清單講的是「你以為關上的門其實開著」，
    /// 而這件事的方向剛好相反（整棟房子是空的）。混在一起會讓那幾句話變成
    /// 一堆語氣一樣、輕重不分的字。
    capture_off: bool,
    /// 那幾條規則**驗過了沒有**。`None` = 根本沒有報告（由 `at` 那一格回答）。
    ///
    /// 同樣不能塞進 `broken`，同樣是因為方向不同：那個清單講「門開著」，這裡
    /// 講「我還不知道門關了沒」。而**「不知道」在這一頁上一直長得像「沒問
    /// 題」**——`broken` 是空的、這一格就是空白，而這一頁自己寫著「空白在這
    /// 一格就是『都生效』」。見 [`sister_core::capabilities::UrlRules`]。
    url_rules: Option<sister_core::capabilities::UrlRules>,
}

#[tauri::command]
fn privacy_health(urls: Vec<String>, shell: tauri::State<'_, Shell>) -> Result<PrivacyHealth, String> {
    let path = config_path()?;
    let mut config = sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}"))?;
    // 拿**輸入框裡現在這一刻**的規則去問，不是設定檔裡存好的那一份——和
    // `lint_url_rules` 同一條紀律。他正在打第一條的時候就該知道它不會生效；
    // 等他按了儲存才講晚了一步，而那一步裡他已經相信門關上了。
    config.privacy.excluded_urls = urls;
    let capture_off = !config.capture.enabled;
    let report = shell
        .data_dir
        .as_ref()
        .and_then(|dir| sister_core::capabilities::read(dir));
    Ok(match report {
        Some(r) => PrivacyHealth {
            broken: r.broken_privacy_rules(&config.privacy),
            at: Some(r.at),
            capture_off,
            url_rules: Some(r.url_rules_verdict(&config.privacy)),
        },
        None => PrivacyHealth {
            broken: Vec::new(),
            at: None,
            capture_off,
            url_rules: None,
        },
    })
}

/// 哪幾條網址規則寫了也不會命中。
///
/// **這是這一頁最有價值的一格。** 排除規則最糟的失效方式不是漏寫，是寫了一條
/// 自以為有效的——使用者看著清單上那一行，以為網銀已經擋掉了。同一份判斷
/// `sister doctor` 和 `record` 都在用，這裡只是把它搬到打字的當下。
#[tauri::command]
fn lint_url_rules(rules: Vec<String>) -> Vec<(String, String)> {
    sister_core::config::suspicious_url_rules(&rules)
}

/// 那張畫面本身。
///
/// **這是這個產品的重點，不是附加功能。** 她說「你三天前看過這個」的時候，
/// 使用者要能當場翻回去看——不然那句話跟任何一個會唬爛的東西沒有差別。
///
/// 圖用 data URL 送過去而不是開一個檔案協定：少一個要設 scope 的表面，
/// 而且「哪些檔案讀得到」的答案就變成「只有這一行指到的那一張」。
#[tauri::command(async)]
fn frame_image(frame_id: i64, shell: tauri::State<'_, Shell>) -> Result<FrameView, String> {
    // `frames.image_path` 存的是**相對**路徑（`2026/08/19/0-….png`），因為整個
    // `frames/` 目錄要能整包搬走、整包備份。所以讀它之前一定要接上根目錄。
    //
    // 少了這一段的時候，`fs::read` 拿行程的工作目錄當根——那是他按下捷徑的
    // 那個資料夾，永遠不會是資料目錄。於是**每一次**點出處都得到「圖不見了」，
    // 而那張圖好端端地躺在磁碟上。這一支的文件第一行寫著「這是這個產品的重點，
    // 不是附加功能」，而它從來沒有成功過一次。
    let root = shell
        .data_dir
        .as_deref()
        .map(sister_core::config::Config::frames_dir)
        .ok_or_else(|| "找不到資料目錄，讀不到那張畫面".to_string())?;
    with_db(&shell, |db| {
        let ctx = db
            .frame_context(frame_id)
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| "找不到這張畫面".to_string())?;

        // 「有這一筆但沒有圖」是**正常**的，不是錯誤。差別要講清楚，不然
        // 使用者會以為程式壞了。
        //
        // 不說是哪一種原因：這裡看到的只有一個 NULL，而 NULL 底下躺著四件
        // 事（只記字、截圖節流、每日額度、保留期到了）。挑一個講出來有四分
        // 之三的機會是錯的。
        let rel = ctx
            .image_path
            .ok_or_else(|| "這一筆沒有留下畫面，只有文字".to_string())?;
        let path = root.join(&rel);
        // 路徑要印出來，而且是**接好根目錄之後**的那一條。使用者拿它去檔案總管
        // 貼上就知道到底有沒有那個檔——這是他唯一能自己驗證這句話的辦法。
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("圖不見了：{}（{e}）", path.display()))?;

        let ext = std::path::Path::new(&rel)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("webp");
        Ok(FrameView {
            data_url: format!("data:image/{ext};base64,{}", sister_shell::base64(&bytes)),
            ts: ctx.ts,
            app: ctx.app_id,
            title: ctx.window_title,
            url: ctx.url,
        })
    })
}

// ---------- 全域暫停熱鍵 ----------

/// 熱鍵現在到底有沒有搶到手。
///
/// **這個結構存在的唯一理由是「搶不到」這件事會發生。** 全域熱鍵是先搶先贏的：
/// 同一組 `Ctrl+Alt+P` 可能早就被螢幕錄影軟體、輸入法或另一個常駐程式拿走了，
/// 而作業系統不會告訴使用者是誰拿走的——它只會讓他按下去、什麼都沒發生。
///
/// 對一顆**暫停**鍵來說那是最壞的一種壞法：他以為她停了，她還在錄。所以註冊
/// 的結果要一路送到設定頁上，寫成一句話，而不是塞進一行只有 `--verbose` 才
/// 看得到的 log。
#[derive(Clone, Serialize, Default)]
struct HotkeyView {
    /// 設定檔裡寫的那一組。空字串 = 使用者關掉了它。
    wanted: String,
    /// 現在真的按得動嗎。
    registered: bool,
    /// 沒搶到的話，作業系統或 Tauri 給的原因。
    reason: Option<String>,
    /// 剛剛試了、但沒搶到的那一組。[`wanted`](Self::wanted) 已經退回上一組。
    ///
    /// 存在的理由是 [`apply_hotkey`] 會先 `unregister_all()`。搶不到的時候
    /// 如果就這樣結束，**連本來好好的那一組也一起沒了**——而畫面上還顯示著
    /// 一組既沒註冊也沒寫進設定檔的組合，下次開機又默默變回舊的那個。
    /// 使用者看到的只有「剛剛試的那組沒成功」，完全不知道暫停鍵從此失效。
    rejected: Option<String>,
    /// 開機時讀不出設定檔，所以 [`wanted`](Self::wanted) 是**內建預設值**，
    /// 不是他設的那一組。帶著讀失敗的原因。
    ///
    /// 開機那段以前是 `Config::load(&p).ok().unwrap_or_default()`——一個
    /// 手寫壞掉的 `config.toml`（或被 OneDrive 鎖住、磁碟滿）會讓他設的
    /// `Ctrl+Alt+S` 安靜地變成內建的那一組。症狀是：他按 S 沒有反應，而
    /// 設定頁指著另一組說「搶到了」，兩邊都不提設定檔。
    ///
    /// 對一顆**暫停**鍵來說這是最壞的一種壞法，和 [`hotkey_set`] 那段註解
    /// 講的是同一件事：他以為她停了，她還在錄。
    config_unreadable: Option<String>,
}

struct Hotkey(Mutex<HotkeyView>);

/// 把設定檔裡那一組換上去，回報結果。
///
/// 先 `unregister_all` 再註冊：這一支同時被開機和「設定頁上改了一組」呼叫，
/// 不先拆掉舊的話，改過三次之後會有三組熱鍵同時活著——其中兩組是他以為自己
/// 已經取消掉的。
fn apply_hotkey(app: &tauri::AppHandle, wanted: &str) -> HotkeyView {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let wanted = wanted.trim().to_string();
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();

    // 空字串是一個正當的選擇，不是壞掉的設定：全域熱鍵會從所有程式手上把那個
    // 組合搶走，所以要留一條關掉它的路。這裡不回報 reason，因為沒有失敗。
    if wanted.is_empty() {
        return HotkeyView {
            wanted,
            registered: false,
            reason: None,
            rejected: None,
            // 這一支只負責「去搶那一組」。設定檔讀不讀得出來是呼叫端的事，
            // 而唯一問得到的呼叫端是開機那一段——`hotkey_set` 走到這裡的時候
            // 設定檔剛剛才讀成功過。
            config_unreadable: None,
        };
    }

    let reason = shortcuts
        .on_shortcut(wanted.as_str(), |app, _shortcut, event| {
            // 只認**按下**。少了這一行，按一次會進來兩次（按下 + 放開），
            // 於是暫停立刻被自己取消掉——一顆看起來完全沒反應的熱鍵。
            if event.state() != tauri_plugin_global_shortcut::ShortcutState::Pressed {
                return;
            }
            match toggle_pause(app.clone(), app.state::<Shell>()) {
                Ok(paused) => announce_hotkey(app, paused),
                Err(e) => tracing::error!("熱鍵暫停失敗：{e}"),
            }
        })
        .err()
        .map(|e| e.to_string());

    HotkeyView {
        wanted,
        registered: reason.is_none(),
        reason,
        rejected: None,
        config_unreadable: None,
    }
}

/// 熱鍵按下去之後，讓他看得到結果。
///
/// 系統匣的選單字會變、視窗裡的字母人會變灰，但**兩個他都可能看不到**——熱鍵
/// 存在的理由正是「她不在畫面上的時候我也想按」。所以停下來的那一下要把她叫
/// 出來：一個看得到的灰色字母人，就是那句「好，我停了」。
///
/// 只在**停**的方向叫，不在恢復的方向叫。停下來按錯了是隱私問題，恢復按錯了
/// 只是白按一次；而每次切換都彈一個視窗出來，會讓收進系統匣這個動作失去意義。
fn announce_hotkey(app: &tauri::AppHandle, paused: bool) {
    if !paused {
        return;
    }
    if let Some(win) = app.get_webview_window(PET) {
        // 不 `set_focus`：他按下暫停的那一刻，螢幕上多半正有一件他在做的事，
        // 把游標從那件事上搶走不是幫忙。
        let _ = win.show();
    }
}

#[tauri::command]
fn hotkey_state(hotkey: tauri::State<'_, Hotkey>) -> HotkeyView {
    hotkey.0.lock().expect("hotkey").clone()
}


/// 換一組熱鍵：先真的去搶，搶到了才寫進設定檔。
///
/// 順序是刻意的。反過來寫（先存再註冊）的話，一組搶不到的熱鍵會留在設定檔裡，
/// 下次開機再失敗一次——而使用者早就把那一頁關掉了。
///
/// **搶不到的時候要把舊的那組裝回去。** [`apply_hotkey`] 開頭就
/// `unregister_all()` 了，所以「試了一組被別人佔走的組合」以前的後果是連本來
/// 好好的那一組也一起消失：`Ctrl+Alt+P` 本來按得動，他試了一次 `Ctrl+Alt+S`
/// ——從那一刻起暫停熱鍵完全失效，設定頁顯示一組設定檔裡不存在的組合，下次
/// 開機又默默變回 `Ctrl+Alt+P`。而畫面上那句話讓他以為只有剛剛試的那組沒成功。
///
/// 對一顆暫停鍵來說那是最壞的一種壞法：他以為她停了，她還在錄。
#[tauri::command]
fn hotkey_set(
    app: tauri::AppHandle,
    combo: String,
    hotkey: tauri::State<'_, Hotkey>,
) -> Result<HotkeyView, String> {
    let previous = hotkey.0.lock().expect("hotkey").wanted.clone();
    let view = apply_hotkey(&app, &combo);
    let view = if view.registered || view.wanted.is_empty() {
        let persist = || -> Result<(), String> {
            let path = config_path()?;
            let mut c = sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}"))?;
            c.shell.pause_shortcut = view.wanted.clone();
            c.save(&path).map_err(|e| format!("{e:#}"))
        };
        // 存不進去的時候**不可以直接 `?` 出去**。那三行以前是裸的 `?`，於是
        // 新的那組已經真的搶下來了（`apply_hotkey` 開頭就 `unregister_all()`），
        // 而底下那行「把結果寫回 state」永遠跑不到——`hotkey_state` 從此回報
        // 舊的那組 `registered: true`，設定頁照著印「搶到了。現在按 Ctrl+Alt+P
        // 都會暫停或繼續」。真正會暫停的是他剛剛試的那一組，P 是死的。下次
        // 開機又從設定檔讀回 P，所以這個分歧不留下任何痕跡。
        //
        // 而且這是那顆**暫停**鍵。上面那段註解說這一格最壞的壞法是「他以為
        // 她停了，她還在錄」——這條路正好走到那裡。
        //
        // 什麼時候會走到這裡：設定檔壞掉（手寫的 retention = 0）、防毒或
        // OneDrive 鎖著 config.toml、磁碟滿。
        if let Err(e) = persist() {
            let restored = apply_hotkey(&app, &previous);
            // `pretty_combo` 而不是原樣印：這一整串是塞進 `Err(String)` 直接
            // 上畫面的，設定頁不會替它排版。原樣印出來是「還在用 Ctrl+Alt+KeyP」
            // ——而鍵盤上沒有一顆鍵叫 KeyP。他要照著這句話去按的。
            let still = if restored.registered {
                format!("還在用 {}。", sister_shell::pretty_combo(&restored.wanted))
            } else if restored.wanted.is_empty() {
                "熱鍵本來就是關掉的，維持原狀。".to_string()
            } else {
                "而舊的那組現在也搶不到了——改用系統匣裡的暫停。".to_string()
            };
            *hotkey.0.lock().expect("hotkey") = restored;
            return Err(format!(
                "搶到了，但存不進設定檔，所以退回原來那一組。{still}\n{e}"
            ));
        }
        view
    } else {
        // 設定檔沒動過，所以退回去的一定是設定檔裡那一組。`rejected` 帶著他
        // 剛剛打的那個組合，讓那句話講得出「你試的那組沒搶到，還在用舊的」。
        HotkeyView {
            rejected: Some(view.wanted),
            ..apply_hotkey(&app, &previous)
        }
    };
    *hotkey.0.lock().expect("hotkey") = view.clone();
    Ok(view)
}

/// 開設定頁。同一個 label 重複用，所以按兩次不會得到兩個視窗。
#[tauri::command]
fn open_settings(app: tauri::AppHandle) -> Result<(), String> {
    const SETTINGS: &str = "settings";
    if let Some(win) = app.get_webview_window(SETTINGS) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        SETTINGS,
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title("AI-Sister 設定")
    .inner_size(640.0, 720.0)
    .min_inner_size(460.0, 420.0)
    .build()
    .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

// ---------- 三張同意書 ----------

#[derive(Serialize)]
struct SheetView {
    key: String,
    /// 條文本身。**從 core 拿，不在這裡重打一份**——同一句話在 CLI 和視窗上
    /// 長得不一樣的話，「他到底同意了哪一句」就沒有答案了。
    wording: String,
    without: String,
    granted_at: Option<i64>,
    /// 現在算不算數。簽過但條文改版了的話，`granted_at` 有值而這裡是 false。
    effective: bool,
}

#[derive(Serialize)]
struct ConsentView {
    path: String,
    current: bool,
    allows_recording: bool,
    /// 第三張**同意書**的狀態：可不可以留圖。
    allows_frames: bool,
    /// 設定檔的 `capture.store_images`。`None` = 那個檔案讀不出來。
    ///
    /// 同意是**上限**，不是開關：這一張簽了、設定檔卻關著的時候，硬碟上一張
    /// 截圖都不會多。只看 `allows_frames` 的那一頁會說「而且會留截圖」，而那
    /// 是一句他要去翻 frames/ 才戳得破的假話。
    ///
    /// 讀不出來就是 `None`，不是猜一個預設值：預設是 true，猜錯的方向正好是
    /// 「答應了一件不會發生的事」。
    store_images: Option<bool>,
    /// 設定檔的 `capture.enabled`——那個總開關。`None` = 檔案讀不出來。
    ///
    /// 它關著的時候每個 tick 直接回 `Tick::Disabled`，連螢幕都不會碰。而這一
    /// 頁簽完名的最後一句話是「接下來跑 sister record 她才會開始，而且會留
    /// 截圖」——**兩個子句都錯**，而他要等上一整天才會發現。
    ///
    /// 和 `store_images` 分開送，因為那兩件事要講的話不一樣：一個是「她根本
    /// 不會開始」，一個是「她會開始，但只記字」。
    capture_enabled: Option<bool>,
    /// 剛剛那一下**順手把另外兩張的簽署時間清掉了**（條文改版）。
    ///
    /// `consent_read` 永遠是 false——只有真的動手的那一下才會是 true。CLI 對
    /// 這件事會印一行 ⚠，這一頁以前完全安靜：他勾了一張，另外兩張的「2026 年
    /// 7 月 2 日同意過」就這樣從畫面上消失，沒有人告訴他為什麼。
    reset_by_version: bool,
    sheets: Vec<SheetView>,
}

fn consent_dir<'r>(shell: &tauri::State<'r, Shell>) -> Result<&'r std::path::Path, String> {
    // `inner()` 而不是直接 deref：借的是**受管理的那份 state**（活得和 app 一樣
    // 久），不是這個 `State` 包裝的區域變數。
    shell
        .inner()
        .data_dir
        .as_deref()
        // 問不出資料目錄的時候**不要**猜一個。同意書寫錯地方，等於他按了同意
        // 而 `sister record` 永遠讀不到——一顆按了沒用、卻顯示成功的按鈕。
        .ok_or_else(|| "找不到資料目錄，同意書沒有地方可以存".to_string())
}

fn consent_view(dir: &std::path::Path) -> ConsentView {
    consent_view_after(dir, false)
}

fn consent_view_after(dir: &std::path::Path, reset_by_version: bool) -> ConsentView {
    use sister_core::consent::Sheet;
    let c = sister_core::consent::load(dir);
    // 只讀一次設定檔。分兩次讀的話，兩個欄位有機會來自不同的兩份內容
    // ——他正好在這中間存檔的話，畫面上會出現一個檔案裡沒有的組合。
    let config = config_path()
        .ok()
        .and_then(|p| sister_core::config::Config::load(&p).ok());
    ConsentView {
        reset_by_version,
        path: sister_core::consent::path(dir).display().to_string(),
        current: c.current(),
        allows_recording: c.allows_recording(),
        allows_frames: c.allows_frames(),
        store_images: config.as_ref().map(|c| c.capture.store_images),
        capture_enabled: config.as_ref().map(|c| c.capture.enabled),
        sheets: Sheet::ALL
            .into_iter()
            .map(|s| SheetView {
                key: s.key().to_string(),
                wording: s.wording().to_string(),
                without: s.without().to_string(),
                granted_at: c.get(s),
                effective: c.current() && c.get(s).is_some(),
            })
            .collect(),
    }
}

#[tauri::command]
fn consent_read(shell: tauri::State<'_, Shell>) -> Result<ConsentView, String> {
    Ok(consent_view(consent_dir(&shell)?))
}

/// 勾或不勾其中一張。
///
/// 一次只動一張，而且每一下都馬上落地——「按了三個勾再按確定」的做法，會在
/// 他關掉視窗的那一刻讓前兩個勾消失，而他以為都存好了。
#[tauri::command]
fn consent_set(
    key: String,
    granted: bool,
    shell: tauri::State<'_, Shell>,
) -> Result<ConsentView, String> {
    use std::str::FromStr;
    let dir = consent_dir(&shell)?;
    let sheet = sister_core::consent::Sheet::from_str(&key)?;
    let mut c = sister_core::consent::load(dir);
    // 條文改版之後，舊的那幾張不能跟著新的一起被存成「現在這一版簽的」。
    // 和 CLI 那邊同一個決定：整份清掉，只留他這次真的按下去的。
    //
    // **而且要講出來。** CLI 對這件事印一行 ⚠，這一頁以前完全安靜——他勾了
    // 一張，另外兩張的「2026 年 7 月 2 日同意過」就從畫面上消失了，看起來像
    // 這個程式把他的紀錄弄丟了。
    let reset_by_version = !c.current() && c != sister_core::consent::Consent::default();
    if !c.current() {
        c = sister_core::consent::Consent::default();
    }
    if granted {
        c.grant(sheet, sister_core::now_ms());
    } else {
        c.revoke(sheet);
    }
    sister_core::consent::save(dir, &c).map_err(|e| format!("{e:#}"))?;
    Ok(consent_view_after(dir, reset_by_version))
}

/// 開同意書那一頁。同一個 label 重複用。
#[tauri::command]
fn open_onboarding(app: tauri::AppHandle) -> Result<(), String> {
    const ONBOARDING: &str = "onboarding";
    if let Some(win) = app.get_webview_window(ONBOARDING) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        ONBOARDING,
        tauri::WebviewUrl::App("onboarding.html".into()),
    )
    .title("三張同意書")
    .inner_size(620.0, 720.0)
    .min_inner_size(460.0, 480.0)
    .build()
    .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

/// 一次刪除的規模。給人看的，所以欄位名是中文語意上的那幾個東西。
#[derive(Serialize)]
struct Erasure {
    chunks: u64,
    facts: u64,
    frames: u64,
    images: u64,
    image_bytes: u64,
    events: u64,
    /// 那段時間裡他自己問過的話（題庫）。單獨一項，理由見
    /// `PruneReport::queries_deleted`。
    queries: u64,
    /// 那段時間結束之後**一列都不剩**的那幾場錄製。
    ///
    /// 刪的不是內容，是「那天 13:02 到 17:44 她在錄」——一份沒有任何內容、
    /// 卻證明他那段時間坐在電腦前的紀錄。那張表以前誰都不刪，而這一頁那顆
    /// 按鈕上寫的是「忘掉」。見 `retention::delete_empty_sessions`。
    sessions: u64,
    /// 刪不掉的檔案。**不吞掉**：那幾張截圖還躺在磁碟上，而使用者以為
    /// 它們已經不在了。
    failed: Vec<String>,
    /// 資料庫說有圖、磁碟上找不到那個檔。
    ///
    /// 不是失敗（東西確實不在了），但**也不是刪掉了**。少了這一欄，預覽說
    /// 「12 張畫面（1.8 MB）」而結果一張都沒提，中間那個落差沒有人解釋——
    /// 而那正是他拿來對帳的兩個數字。CLI 早就有這一行。
    missing: u64,
    /// 刪完之後**沒被帶走**的那幾列 `sessions`——見
    /// [`DbStats::only_session_shells_left`](sister_core::db::DbStats::only_session_shells_left)。
    ///
    /// `None` 不是 0：預覽算不出這個（它一列都不動，沒有「刪完之後」可言）。
    /// 兩種意思寫成同一個 0 的話，這裡就變成它自己要修的那個 bug——`Some(0)`
    /// 是「沒有東西留下來」，`None` 是「這一趟沒有問這個問題」。
    sessions_left: Option<u64>,
    /// 留下來的那一列是誰的：`"live"`（她此刻正在錄）、`"booting"`（有一個
    /// recorder 正在起來，那一列不是它的）、`"gone"`（沒有人在，她當掉了）。
    ///
    /// 只有 `sessions_left > 0` 的時候有意義，所以它和上面那一欄要在同一個
    /// `if` 裡讀完——分開讀就會有人拿一個沒問過的值去講一句斷言。
    ///
    /// # 為什麼是三個字串，不是一個布林
    ///
    /// 上一版是 `shell_is_live: bool`，算的是 `heartbeat::is_occupied`，而註解
    /// 把那個選擇寫成有理由的（「正在開機的 recorder 也佔著這個目錄」）。於是
    /// 畫面印的是「此刻有人佔著這個資料目錄（**她正在錄，或正在開機**）」——那
    /// 個「或」正是這個 repo 一路在刪的東西，而且它在開機那幾分鐘是**假的**：
    /// `BootBeat::start` 先寫心跳，`start_session` 最後才 INSERT，所以那幾分鐘
    /// 裡手上這一列一定是**上一次當機留下來的殼**，不是佔著目錄的那一個。三種
    /// 心跳三句話，而一個布林湊不出三種答案。CLI 那邊是同一個判斷
    /// （`session_shell_why` 收 `Option<Phase>`）。
    ///
    /// 一個欄位三個值，不是兩個布林：兩個布林拼得出「又在錄又在開機」這種不存
    /// 在的組合，而拼錯的那一次不會有人紅。
    shell_beat: &'static str,
}

impl From<sister_core::retention::PruneReport> for Erasure {
    fn from(r: sister_core::retention::PruneReport) -> Self {
        Self {
            chunks: r.chunks_deleted,
            facts: r.facts_deleted,
            frames: r.frames_deleted,
            images: r.images_deleted,
            image_bytes: r.image_bytes_freed,
            events: r.events_deleted,
            queries: r.queries_deleted,
            sessions: r.sessions_deleted,
            failed: r.failed,
            missing: r.missing,
            // 這一支只看得到「刪掉了什麼」。留下什麼要再問一次資料庫，所以
            // 預設是「沒問」，由 `forget_range` 補上。
            sessions_left: None,
            shell_beat: "gone",
        }
    }
}

/// 忘掉這一段會刪掉什麼。一句 DELETE 都沒有。
///
/// 畫面檔的根目錄拿不到就整支拒絕，理由和 `forget_range` **正好相反但一樣硬**：
/// 那邊是不能假裝刪掉了，這邊是不能假裝放得出空間。退成 `None` 的話這一支會
/// 回報「0 個畫面檔」，而真的按下去會刪掉幾百張——一份把代價說小的預覽，比
/// 沒有預覽更糟。
#[tauri::command(async)]
fn forget_preview(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Erasure, String> {
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，算不出這一段會刪掉多少東西".to_string())?;
    let frames = sister_core::config::Config::frames_dir(dir);
    with_db(&shell, |db| {
        db.forget_preview(from_ts, to_ts, Some(&frames))
            .map(Erasure::from)
            .map_err(|e| format!("{e:#}"))
    })
}

/// 真的刪。**沒有回收桶，沒有復原。**
///
/// 前端會先叫一次 `forget_preview` 把數字擺在使用者眼前，但那個順序是前端的
/// 禮貌，不是這裡的前提——這一支不管有沒有人預覽過都會照做，因為「一定要先
/// 預覽」的規則放在畫面上就等於沒有規則。真正的防線在 core：區間反過來的話
/// 一列都不動。
#[tauri::command(async)]
fn forget_range(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Erasure, String> {
    // 畫面檔的根目錄拿不到就整支拒絕，**不要**退成 `None` 硬幹。
    // `None` 的意思是「只刪資料庫、不碰檔案」，那會回報一份漂亮的成功，
    // 而那段時間的截圖一張不少地留在磁碟上。
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，不能保證截圖真的會被刪掉".to_string())?;
    let frames = sister_core::config::Config::frames_dir(dir);
    // 在借出資料庫之前問，因為它讀的是磁碟上的心跳檔，不是資料庫。**問
    // `phase` 不問 `is_occupied`**：理由在 `Erasure::shell_beat` 上面。
    let beat = match sister_core::heartbeat::phase(dir, sister_core::now_ms()) {
        Some(sister_core::heartbeat::Phase::Recording) => "live",
        Some(sister_core::heartbeat::Phase::Booting) => "booting",
        None => "gone",
    };
    with_db_mut(&shell, |db| {
        let report = db
            .forget(from_ts, to_ts, Some(&frames))
            .map_err(|err| format!("{err:#}"))?;
        // **沒被帶走的那一列也要講。** 上一場當掉的話，那一列 `sessions` 撐得
        // 過這一刀（守衛不准碰還沒收尾的最新一列，因為那可能是此刻正在錄的那
        // 一場）。少了這一欄，這一頁只列得出刪掉的東西，他要到別的地方才會撞
        // 見一個「1 場錄製」站在一整排 0 旁邊。CLI 那邊是同一句話。
        let stats = db.stats().map_err(|err| format!("{err:#}"))?;
        let left = if stats.only_session_shells_left() {
            stats.sessions as u64
        } else {
            0
        };
        Ok(Erasure {
            sessions_left: Some(left),
            // 分得出來就不要印「或」。CLI 那邊同一個判斷（`session_shell_why`）。
            // 沒有留下來的列就沒有這個問題——那時候這一欄不准講一個沒問過的
            // 答案，所以跟著 `From` 的預設走。
            shell_beat: if left > 0 { beat } else { "gone" },
            ..Erasure::from(report)
        })
    })
}

/// 開時間軸。
///
/// 為什麼要一個視窗而不是塞進字母人：搜尋回答的是「我記得的那件事在哪」，
/// 時間軸回答的是**「她到底記了什麼」**——後者是使用者決定要不要信任她的
/// 依據，而 340 像素寬的欄位撐不起「翻過一整天」這件事。
///
/// 同一個 label 重複用，所以按兩次不會得到兩個視窗。
#[tauri::command]
fn open_timeline(app: tauri::AppHandle) -> Result<(), String> {
    const TIMELINE: &str = "timeline";
    if let Some(win) = app.get_webview_window(TIMELINE) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        TIMELINE,
        tauri::WebviewUrl::App("timeline.html".into()),
    )
    .title("她記得的每一天")
    .inner_size(980.0, 720.0)
    .min_inner_size(560.0, 420.0)
    .build()
    .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

/// 他點開了第 `rank` 筆的出處。
///
/// 這是檢索品質唯一不需要人工標註就拿得到的訊號：他點下去的那一刻，等於幫那
/// 一題標了正解，而 `rank` 直接說出排序把它放在第幾個。Phase 2 的題庫要「≥ 30
/// 題來自真實 query log」，靠的就是這一筆。
///
/// 和 [`open_frame`] 分開兩個命令，因為它們的失敗方向不一樣：畫面開不起來要
/// 讓他知道，記不進題庫不該打斷他正在做的事。畫面那一邊是 fire-and-forget。
#[tauri::command(async)]
fn log_click(
    query_id: i64,
    chunk_id: i64,
    rank: usize,
    shell: tauri::State<'_, Shell>,
) -> Result<(), String> {
    with_db(&shell, |db| {
        db.log_click(query_id, chunk_id, rank).map_err(|e| {
            tracing::warn!("這一次點擊沒記進題庫：{e}");
            e.to_string()
        })
    })
}

/// 他說「這一題我本來已經忘了」（或者收回那句話）。
///
/// PHASES.md Phase 1 的第一條退場條件是「自用 7 天內 ≥ 3 次答對我自己都忘掉的
/// 東西」，而那件事只有他知道——題庫裡沒有任何一欄答得出來。
///
/// **和 [`log_click`] 的失敗處理相反。** 點擊是 fire-and-forget：他要的是那張
/// 畫面，記不記得到帳是次要的。這裡他要的**就是**記這一筆——記不進去而畫面裝
/// 作記進去了，等於在退場條件的證據上說謊。所以錯誤要回到畫面上。
#[tauri::command(async)]
fn mark_query(query_id: i64, marked: bool, shell: tauri::State<'_, Shell>) -> Result<bool, String> {
    with_db(&shell, |db| {
        db.mark_query(query_id, marked)
            // 這裡只要 `marked`——那顆按鈕問的是「現在該畫成什麼樣子」。
            // `changed`（這一次有沒有真的動到）是終端機才需要分辨的事：那邊
            // 打得出一個他自己想錯的題號，這邊的 id 是剛剛那一次回答帶下來的。
            .map(|o| o.marked)
            .map_err(|e| {
                tracing::warn!("這一次標記沒記進題庫：{e}");
                format!("{e:#}")
            })
    })
}

/// 開一個看圖的視窗。
///
/// 為什麼是另一個視窗：字母人只有 340 像素寬，一張 2560×1440 的畫面縮進去
/// 是一片糊——「點開看當時畫面」看不清楚就等於沒做。
///
/// 同一個 label 重複用，所以連點五筆結果不會得到五個視窗；已經開著就換一張
/// 圖並拉到前面。
#[tauri::command]
fn open_frame(app: tauri::AppHandle, frame_id: i64) -> Result<(), String> {
    const VIEWER: &str = "frame";
    let target = format!("frame.html?id={frame_id}");

    if let Some(win) = app.get_webview_window(VIEWER) {
        let _ = win.eval(format!("window.location.replace('{target}')"));
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }

    tauri::WebviewWindowBuilder::new(&app, VIEWER, tauri::WebviewUrl::App(target.into()))
        .title("當時的畫面")
        .inner_size(1100.0, 720.0)
        .min_inner_size(480.0, 320.0)
        .build()
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

#[derive(Serialize)]
struct FrameView {
    data_url: String,
    ts: i64,
    app: Option<String>,
    title: Option<String>,
    url: Option<String>,
}

/// 把視窗現在的位置記進記憶體（不寫檔）。
///
/// 拖曳的時候 `Moved` 每幾毫秒就來一次，每次都寫硬碟是拿一個常駐程式去
/// 磨 SSD。寫檔的時機是收進系統匣、切換置頂、關閉——也就是**位置真的
/// 定下來**的那幾個時刻。代價寫在這裡：被工作管理員直接砍掉的話，
/// 那一次的移動記不住。
fn remember_position(win: &tauri::WebviewWindow, shell: &tauri::State<'_, Shell>) {
    if let Ok(pos) = win.outer_position() {
        let mut state = shell.state.lock().expect("pet state");
        state.x = pos.x;
        state.y = pos.y;
    }
}

fn monitors_of(win: &tauri::WebviewWindow) -> Vec<Rect> {
    win.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| Rect {
            x: m.position().x,
            y: m.position().y,
            w: m.size().width as i32,
            h: m.size().height as i32,
        })
        .collect()
}

/// 這一份記錄裡**沒有任何一個字來自螢幕**。
///
/// 只有這個殼自己的事：熱鍵搶到了沒、視窗開不開得起來、資料庫花了幾毫秒打開。
/// 之所以要寫成檔案，是因為 release build 沒有主控台（見檔案最上面的
/// `windows_subsystem`）——`tracing::error!("同意書開不起來：{e}")` 這種唯一
/// 講得出原因的話，在唯一會出貨的平台上是講給空氣聽的。
///
/// 這一條是從一張截圖上學到的：她卡住了，而我沒有任何辦法知道她卡在哪裡。
/// 一個讀不到的診斷，和沒有診斷是同一件事。
fn start_log(data_dir: Option<&PathBuf>) -> Option<std::fs::File> {
    start_log_at(data_dir?, "desktop.log")
}

/// 開一份新的記錄檔，並把上一輪的留成 `.1`。
///
/// 留一份是因為：當掉之後要看的正是**當掉那一輪**寫了什麼，而他為了找記錄檔
/// 一定得先把她重開——直接覆蓋的話，唯一有用的那一份就沒了。
fn start_log_at(dir: &std::path::Path, name: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(name);
    let _ = std::fs::rename(&path, dir.join(format!("{name}.1")));
    std::fs::File::create(&path).ok()
}

fn main() {
    let data_dir = sister_core::config::Config::default_data_dir();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "sister_desktop=info".into());
    match start_log(data_dir.as_ref()) {
        // 檔案裡不要 ANSI 跳脫碼，記事本打開會是一片亂碼。
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init(),
        // 連資料目錄都問不出來就退回 stdout。開發時（有主控台）仍然看得到，
        // 出貨時看不到——但那個情況下她本來也幾乎做不了任何事。
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
    tracing::info!("AI-Sister {} 起來了", env!("CARGO_PKG_VERSION"));
    let state_path = data_dir
        .clone()
        .unwrap_or_else(std::env::temp_dir)
        .join("pet-window.json");

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(Shell {
            state: Mutex::new(bounds::load(&state_path).unwrap_or_default()),
            state_path,
            data_dir,
            db: Mutex::new(None),
            spawned: Mutex::new(None),
        })
        .manage(Hotkey(Mutex::new(HotkeyView::default())))
        .invoke_handler(tauri::generate_handler![
            toggle_pin,
            hide_to_tray,
            ask,
            open_frame,
            log_click,
            mark_query,
            frame_image,
            pause_state,
            recording_state,
            start_recording,
            stop_recording,
            recorder_log_tail,
            last_recording_end,
            has_ever_recorded,
            has_ever_stored,
            toggle_pause,
            settings_read,
            settings_write,
            lint_url_rules,
            privacy_health,
            open_settings,
            open_timeline,
            timeline_days,
            timeline_moments,
            forget_preview,
            forget_range,
            consent_read,
            consent_set,
            open_onboarding,
            hotkey_state,
            hotkey_set
        ])
        .setup(|app| {
            let win = app
                .get_webview_window(PET)
                .expect("pet window is declared in tauri.conf.json");
            let shell = app.state::<Shell>();

            // ---- 位置 ----
            let screens = monitors_of(&win);
            let saved = *shell.state.lock().expect("pet state");
            let first_run = saved.x == 0 && saved.y == 0;

            let place = if first_run {
                let primary = win
                    .primary_monitor()
                    .ok()
                    .flatten()
                    .map(|m| Rect {
                        x: m.position().x,
                        y: m.position().y,
                        w: m.size().width as i32,
                        h: m.size().height as i32,
                    })
                    .unwrap_or(Rect {
                        x: 0,
                        y: 0,
                        w: 1920,
                        h: 1080,
                    });
                let (x, y) = bounds::first_run_corner(PET_W, PET_H, primary);
                Rect {
                    x,
                    y,
                    w: PET_W,
                    h: PET_H,
                }
            } else {
                // 這一行就是 TokenMonster 少掉的那一步：還原之前先問「現在
                // 還看得到嗎」。見 bounds.rs 的說明。
                bounds::nudge_onto(
                    Rect {
                        x: saved.x,
                        y: saved.y,
                        w: PET_W,
                        h: PET_H,
                    },
                    &screens,
                )
            };

            let _ = win.set_position(PhysicalPosition::new(place.x, place.y));
            let _ = win.set_always_on_top(saved.pinned);
            {
                let mut state = shell.state.lock().expect("pet state");
                state.x = place.x;
                state.y = place.y;
            }
            let _ = win.show();

            // ---- 先去把資料庫打開 ----
            //
            // 不等人問問題才開。第一次開可能要跑 migration，而 003 要把整張
            // `text_chunks` 讀出來重算 bigram——升級上來的資料庫愈大愈久。那段
            // 時間如果是掛在「他剛剛按下 Enter」上，畫面就會停在「想一下…」，
            // 而他無從分辨那是她在想、還是她死了。
            //
            // 開不起來**不在這裡報錯**：她本來就該在一台什麼都還沒錄過的機器上
            // 站得住。真正要講的那句話（「還沒有任何記憶——先跑 sister record」）
            // 留給他真的問問題的時候講，那時候他才需要知道。
            let warm = app.handle().clone();
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                match with_db(&warm.state::<Shell>(), |_| Ok(())) {
                    Ok(()) => {
                        tracing::info!("資料庫開好了（{} ms）", started.elapsed().as_millis());
                    }
                    Err(e) => tracing::info!("資料庫先不開：{e}"),
                }
            });

            // ---- 全域暫停熱鍵 ----
            //
            // 系統匣那一顆要先看得到圖示、再點開選單；熱鍵是**不用先找到她**的
            // 那條路，而「我現在不想被看」最常發生的時機，正是她不在畫面上、
            // 而且你手上正忙著別的事的時候。
            //
            // 搶不到不是致命的（系統匣那顆還在），所以這裡不 `?`——但也不能就
            // 這樣算了：狀態存進 `Hotkey`，設定頁上寫得出「這一組被別人拿走了」。
            //
            // **讀不出設定檔的時候不可以安靜地換一組。** 這裡以前是
            // `Config::load(&p).ok().unwrap_or_default()`——一個手寫壞掉的
            // `config.toml`（或被防毒／OneDrive 鎖著、磁碟滿）會讓他設的
            // `Ctrl+Alt+S` 變成內建的那一組。症狀是他按 S 沒有反應，而設定頁
            // 指著另一組說「搶到了」。這一頁上別的地方早就在對付這種事
            // （`setUnreadable`、`Config::reload` 的「繼續用舊的那一份」），
            // 只有這條路上的那個 `.ok()` 把原因整個吞掉了。
            {
                let loaded = config_path().and_then(|p| {
                    sister_core::config::Config::load(&p).map_err(|e| format!("{e:#}"))
                });
                let config_unreadable = loaded.as_ref().err().cloned();
                let wanted = loaded
                    .unwrap_or_default()
                    .shell
                    .pause_shortcut;
                let view = HotkeyView {
                    config_unreadable,
                    ..apply_hotkey(app.handle(), &wanted)
                };
                // 成功也要留一行。「搶到了」和「這段程式根本沒跑到」在一份
                // 只記失敗的記錄檔裡長得一模一樣，而那正是他按了熱鍵沒反應時
                // 唯一想分辨的兩件事。
                match &view.reason {
                    Some(reason) => tracing::warn!("暫停熱鍵 {} 註冊不起來：{reason}", view.wanted),
                    None if view.wanted.is_empty() => tracing::info!("暫停熱鍵是關掉的"),
                    None => tracing::info!("暫停熱鍵 {} 搶到了", view.wanted),
                }
                *app.state::<Hotkey>().0.lock().expect("hotkey") = view;
            }

            // ---- 系統匣 ----
            // 暫停也放在系統匣，因為熱鍵可能被別的程式搶走，而她收起來的時候
            // 系統匣是最後一個一定按得到的地方。
            let paused_now = pause_state(app.state::<Shell>());
            let show_item = MenuItem::with_id(app, "show", "顯示 AI-Sister", true, None::<&str>)?;
            let pause_item =
                MenuItem::with_id(app, "pause", pause_label(paused_now), true, None::<&str>)?;
            // 開始／停止和暫停是兩件事，所以是兩顆。暫停是「先別看，但留在
            // 這裡」，停止是「今天到此為止」——把停止做成「一直暫停」會留下
            // 一個永遠在跑卻永遠不做事的行程，而他在工作管理員裡看得到它。
            //
            // 這兩行字問的是「按下去會發生什麼」，所以看的是**有沒有人佔著**
            // ——理由在 [`set_record_labels`] 上面。
            let occupied_now =
                recording_state(app.handle().clone(), app.state::<Shell>()) != "none";
            let record_item = MenuItem::with_id(
                app,
                "record",
                record_label(occupied_now),
                true,
                None::<&str>,
            )?;
            let timeline_item =
                MenuItem::with_id(app, "timeline", "她記得的每一天…", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "設定…", true, None::<&str>)?;
            let consent_item =
                MenuItem::with_id(app, "consent", "三張同意書…", true, None::<&str>)?;
            let quit_item =
                MenuItem::with_id(app, "quit", quit_label(occupied_now), true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show_item,
                    &record_item,
                    &pause_item,
                    &timeline_item,
                    &settings_item,
                    &consent_item,
                    &quit_item,
                ],
            )?;
            app.manage(PauseItem(pause_item));
            app.manage(RecordItem(record_item));
            app.manage(QuitItem(quit_item));

            TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("icon").clone())
                .tooltip("AI-Sister")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window(PET) {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "pause" => {
                        // 失敗要看得見。一顆按了什麼都沒發生的暫停鍵，
                        // 使用者只會以為自己按到了。
                        if let Err(e) = toggle_pause(app.clone(), app.state::<Shell>()) {
                            tracing::error!("暫停切換失敗：{e}");
                        }
                    }
                    "record" => {
                        // 讀當下的真相再做相反的事，不看選單上那行字——那行字
                        // 最多可能舊了 5 秒（見 `recording_state`），而在那 5 秒
                        // 裡按下去的人，想要的是他**看到的狀態**的相反。
                        let shell = app.state::<Shell>();
                        // 「有人佔著」才是這一顆的問題，不是「她在錄嗎」：正在
                        // 起來的那幾分鐘走 `start_recording` 只會撞上它自己那道
                        // `is_occupied` 閘門，回一句「已經有一個在跑了」——而他
                        // 按的是一顆寫著「開始記錄」的按鈕。
                        let on = recording_state(app.clone(), shell.clone()) != "none";
                        let done = if on {
                            stop_recording(shell.clone())
                        } else {
                            start_recording(shell.clone())
                        };
                        match done {
                            // 立刻改字，不等下一次輪詢——按了之後那一顆要當場
                            // 看起來不一樣，不然他會再按一次。
                            Ok(()) => {
                                if let Some(item) = app.try_state::<RecordItem>() {
                                    let _ = item.0.set_text(record_label(!on));
                                }
                            }
                            Err(e) => {
                                tracing::error!("開始／停止記錄失敗：{e}");
                                // 系統匣選單上沒有一格能放字。只寫進記錄檔的話，
                                // 按下去的後果是**什麼都沒發生**——而 `e` 是一句
                                // 寫好的完整中文（同意書沒簽、找不到 sister.exe、
                                // 已經有一個在跑），只是躺在他不會開的檔案裡。
                                //
                                // 先把視窗叫出來再送：收在系統匣的時候，那邊沒有
                                // 人在聽，這句話會掉在地上。
                                if let Some(win) = app.get_webview_window(PET) {
                                    let _ = win.show();
                                    let _ = win.set_focus();
                                }
                                use tauri::Emitter;
                                let _ = app.emit("recorder-failed", e);
                            }
                        }
                    }
                    "timeline" => {
                        if let Err(e) = open_timeline(app.clone()) {
                            tracing::error!("時間軸開不起來：{e}");
                        }
                    }
                    "settings" => {
                        if let Err(e) = open_settings(app.clone()) {
                            tracing::error!("設定頁開不起來：{e}");
                        }
                    }
                    "consent" => {
                        if let Err(e) = open_onboarding(app.clone()) {
                            tracing::error!("同意書開不起來：{e}");
                        }
                    }
                    "quit" => {
                        let shell = app.state::<Shell>();
                        // 走人之前把 recorder 也叫停。留著的話，他關掉的是唯一
                        // 看得見的那個視窗，而螢幕還在被記錄——他會以為自己
                        // 已經關掉了。這比「她其實沒在錄卻說在聽」更糟：那個是
                        // 少記了，這個是在他以為關掉之後繼續記。
                        //
                        // 不管那場 recorder 是不是這裡開起來的，都停。要在兩種
                        // 錯之間選一個的話，「停掉一個終端機裡的 record，而那個
                        // 終端機會印出是誰叫它停的」，比「安靜地繼續錄」好。
                        //
                        // **正在起來的那一個也要停。** 上一版問的是「她在錄
                        // 嗎」，於是他在那幾分鐘按「結束」，視窗關了，而那個還
                        // 在開資料庫的行程留在工作管理員裡，幾分鐘後開始錄——
                        // 這一段註解上面兩行講的正是這件事。
                        //
                        // **而「正在起來」還有更早的一段，那一段連心跳都還沒
                        // 有。** `recording_state` 讀的是 `recording.beat`；從
                        // `cmd.spawn()` 回來到 child 在 `BootBeat::start` 寫下
                        // 第一拍之間，那個檔案根本不存在，於是這裡讀到 "none"。
                        // 平常是幾毫秒，但第一次安裝之後 Defender 要掃一顆
                        // 6.7 MB 的新 exe，可以是好幾秒——正好是新使用者最會亂
                        // 按的那一刻。`start_recording` 早就解過同一個缺口了
                        // （「問行程，不要問它寫的檔案」），只有這裡沒跟上。
                        //
                        // 所以先寫檔、再問行程，兩件事都做。
                        //
                        // 檔案無條件寫。以前只在 `!= "none"` 的時候寫，但要停的
                        // 正是那個讀不到心跳的——那個判斷式和要救的情況是相反
                        // 的。沒人在跑的時候多留一個 `stop.request` 不會傷到下一
                        // 場：`BootBeat::start` 開機第一行就清掉它，而全 repo 只
                        // 有錄製迴圈的 `take_stop` 讀這個檔，沒有任何畫面會因為
                        // 它躺在那裡而說錯話。
                        if let Err(e) = stop_recording(shell.clone()) {
                            tracing::error!("結束時停不掉 recorder：{e}");
                        }
                        // 但光是檔案不夠。child 自己的 `clear_stop` 就排在它開機
                        // 的第一行，我們剛寫的這一個很可能被它擦掉——這正是 #61
                        // 那個修法帶來的副作用，而那個修法是對的。所以還要問一次
                        // 行程。
                        //
                        // 光靠那個檔案還漏一格：`clear_stop` 是 `BootBeat::start`
                        // 的第一行，所以只要她已經蓋過一拍，這個請求就不會被她自
                        // 己擦掉；漏掉的是 spawn 回來到她跑到那第一行之間那幾毫秒
                        // ——child 會把我們剛寫的請求擦掉，然後留一個還在錄的孤
                        // 兒。那是 #63 的洞。
                        //
                        // 補它要落刀，而落刀從來不是問「她在錄嗎」，是問「她還沒
                        // 走到 `Db::open`，所以磁碟上沒有東西會被我砍壞嗎」。這兩
                        // 句之間靠一條不變式接起來：**心跳蓋在 `Db::open` 之前**。
                        //
                        // 上一版把那個問題寫成 `beat == "none"`，而那句話同時是三
                        // 件事——她還沒起來（可以砍）、她正在乾淨收工（`stop` 寫
                        // 在 `rec.finish()` 之前）、她好好的只是這一拍慢了 16 秒。
                        // 後兩種落刀的代價是 `end_session` 永遠是 NULL，於是
                        // `doctor` 說「她當掉了」——她沒當掉，是我砍的。而第二種
                        // 只要按「停止記錄」再按「結束」就會發生，那兩顆按鈕在同
                        // 一張選單上，中間隔 0.4 秒。所以那一版被整個拿掉了。
                        //
                        // 現在 `heartbeat::stop` 留墓碑而不是刪檔，這三件事在
                        // `Presence` 上是三個不同的值，閘門搬進
                        // `heartbeat::safe_to_kill_spawn`（那裡有測試，這裡沒有）。
                        // 它只放行一種：**這個資料目錄上沒有任何東西是在我 spawn
                        // 之後寫下的**。
                        //
                        // 只砍我們自己 spawn 的那個 handle，不照 PID 去找：別人在
                        // 終端機裡開的那一場不歸這個視窗管。
                        if let Some(dir) = shell.data_dir.as_deref() {
                            let mut spawned = shell.spawned.lock().expect("spawned recorder");
                            if let Some(s) = spawned.as_mut()
                                && matches!(s.child.try_wait(), Ok(None))
                                && sister_core::heartbeat::safe_to_kill_spawn(
                                    dir,
                                    s.at,
                                    sister_core::now_ms(),
                                )
                            {
                                match s.child.kill() {
                                    Ok(()) => {
                                        tracing::info!(
                                            "剛 spawn 出來還沒開資料庫，直接收掉：pid {}",
                                            s.child.id()
                                        );
                                        // 收屍，不然她變 zombie 掛在我們身上。
                                        let _ = s.child.wait();
                                    }
                                    Err(e) => tracing::error!("收不掉剛 spawn 的 recorder：{e}"),
                                }
                            }
                        }
                        shell.persist();
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左鍵點圖示 = 開關。這是這類常駐程式唯一大家都會試的手勢。
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                        && let Some(win) = tray.app_handle().get_webview_window(PET)
                    {
                        if win.is_visible().unwrap_or(false) {
                            let _ = win.hide();
                        } else {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                })
                .build(app)?;

            // ---- 系統匣的字自己刷 ----
            //
            // 理由寫在 `refresh_tray`：那三行字以前是搭畫面輪詢的便車更新的，
            // 而畫面收進系統匣就不問了——正好是系統匣變成唯一介面的那一刻。
            let ticker = app.handle().clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(TRAY_REFRESH);
                    // 選單是 UI，回主執行緒去動。這一步失敗只有一個原因：事件
                    // 迴圈已經走了。那就跟著收工，不要留一條對著死掉的 app
                    // handle 每 5 秒喊一次的執行緒。
                    let on_main = ticker.clone();
                    if ticker
                        .run_on_main_thread(move || refresh_tray(&on_main))
                        .is_err()
                    {
                        break;
                    }
                }
            });

            // ---- 關閉 = 收起來，不是結束 ----
            let handle = app.handle().clone();
            win.on_window_event(move |event| match event {
                WindowEvent::CloseRequested { api, .. } => {
                    // Alt+F4 在這裡不該是「再見」。真的要結束走系統匣選單。
                    api.prevent_close();
                    if let Some(win) = handle.get_webview_window(PET) {
                        let shell = handle.state::<Shell>();
                        remember_position(&win, &shell);
                        shell.persist();
                        let _ = win.hide();
                    }
                }
                WindowEvent::Moved(pos) => {
                    let shell = handle.state::<Shell>();
                    let mut state = shell.state.lock().expect("pet state");
                    state.x = pos.x;
                    state.y = pos.y;
                }
                _ => {}
            });

            // ---- 還沒簽同意書就先問 ----
            //
            // 這一支不會擋住字母人，因為她本來就只是**讀**資料庫——沒有同意書
            // 也讀得到已經存在的東西，而擋掉只會讓一個想來撤回同意的人進不來。
            // 真正的閘門在 `sister record`（見 `ops::record::gate`）。
            //
            // 這裡做的是另一件事：`sister record` 拒絕啟動的時候，那句話印在
            // 一個他可能根本沒開的終端機裡。字母人是他看得到的那一面。
            if !consent_read(app.state::<Shell>())
                .map(|v| v.allows_recording)
                .unwrap_or(false)
                && let Err(e) = open_onboarding(app.handle().clone())
            {
                tracing::error!("同意書開不起來：{e}");
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("build AI-Sister")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                app.state::<Shell>().persist();
            }
        });
}
