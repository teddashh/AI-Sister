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
}

impl Shell {
    fn persist(&self) {
        let snapshot = *self.state.lock().expect("pet state");
        bounds::save(&self.state_path, &snapshot);
    }
}

/// 一筆答案。這是她說話的全部形狀——**每一筆都帶出處**。
///
/// `frame_id` 是「點開看當時那張畫面」的鑰匙。它可以是 `None`（文字保留期比
/// 畫面長，舊的字還在但圖已經清掉了），而那時候 UI 要顯示「圖已過期」，
/// 不是假裝沒有這件事。
#[derive(Serialize)]
struct Hit {
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

#[tauri::command]
fn toggle_pause(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) -> Result<bool, String> {
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，暫停鍵沒有作用".to_string())?;
    let next = !sister_core::pause::is_paused(dir);
    sister_core::pause::set_paused(dir, next, sister_core::now_ms()).map_err(|e| e.to_string())?;
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
    if paused { "繼續記錄" } else { "暫停記錄" }
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
        *slot = Some(sister_core::db::Db::open(&path).map_err(|e| e.to_string())?);
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

#[tauri::command]
fn ask(question: String, shell: tauri::State<'_, Shell>) -> Result<Vec<Hit>, String> {
    let question = question.trim().to_string();
    if question.is_empty() {
        return Ok(Vec::new());
    }
    with_db(&shell, |db| {
        let hits = db.search(&question, 20).map_err(|e| e.to_string())?;
        Ok(hits
            .into_iter()
            .map(|h| Hit {
                ts: h.ts,
                text: h.text,
                snippet: h.snippet,
                app: h.app_id,
                title: h.window_title,
                url: h.url,
                frame_id: h.frame_id,
            })
            .collect())
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
#[tauri::command]
fn timeline_days(tz_offset_ms: i64, shell: tauri::State<'_, Shell>) -> Result<Vec<Day>, String> {
    const H: i64 = 3_600_000;
    let tz = tz_offset_ms.clamp(-12 * H, 14 * H);
    with_db(&shell, |db| {
        Ok(db
            .days_with_data(tz)
            .map_err(|e| e.to_string())?
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
    /// `None` = 字還在，但那張圖已經過了保留期。
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
#[tauri::command]
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
            .map_err(|e| e.to_string())?;
        let truncated = rows.len() > limit;
        rows.truncate(limit);

        let pauses = db
            .pause_spans(from_ts, to_ts)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|s| Gap {
                from: s.from,
                to: s.to,
            })
            .collect();

        Ok(DayView {
            moments: rows
                .into_iter()
                .map(|m| Moment {
                    ts: m.ts,
                    app: m.app,
                    title: m.title,
                    url: m.url,
                    text: m.text,
                    frame_id: m.frame_id,
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
    let c = sister_core::config::Config::load(&path).map_err(|e| e.to_string())?;
    Ok(Settings {
        excluded_apps: c.privacy.excluded_apps,
        excluded_urls: c.privacy.excluded_urls,
        excluded_titles: c.privacy.excluded_titles,
        pause_on_screenshare: c.privacy.pause_on_screenshare,
        redact_clipboard_secrets: c.privacy.redact_clipboard_secrets,
        frames_days: c.retention.frames_days,
        text_days: c.retention.text_days,
        path: path.display().to_string(),
    })
}

#[tauri::command]
fn settings_write(settings: Settings) -> Result<(), String> {
    let path = config_path()?;
    // **先讀再改再寫**，不是從空白組一份出來。設定檔裡有這一頁沒有畫出來的
    // 欄位（截圖間隔、每日畫面額度……），從頭組一份會把它們全部重設成預設值
    // ——使用者只是改了個保留天數，磁碟預算卻被悄悄換掉了。
    let mut c = sister_core::config::Config::load(&path).map_err(|e| e.to_string())?;
    c.privacy.excluded_apps = settings.excluded_apps;
    c.privacy.excluded_urls = settings.excluded_urls;
    c.privacy.excluded_titles = settings.excluded_titles;
    c.privacy.pause_on_screenshare = settings.pause_on_screenshare;
    c.privacy.redact_clipboard_secrets = settings.redact_clipboard_secrets;
    c.retention.frames_days = settings.frames_days;
    c.retention.text_days = settings.text_days;
    c.save(&path).map_err(|e| e.to_string())
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
#[tauri::command]
fn frame_image(frame_id: i64, shell: tauri::State<'_, Shell>) -> Result<FrameView, String> {
    with_db(&shell, |db| {
        let ctx = db
            .frame_context(frame_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "找不到這張畫面".to_string())?;

        // 文字保留 365 天、畫面 30 天，所以「有這一筆但圖沒了」是**正常**的，
        // 不是錯誤。差別要講清楚，不然使用者會以為程式壞了。
        let path = ctx
            .image_path
            .ok_or_else(|| "這張畫面已經過了保留期，只有文字留下來".to_string())?;
        let bytes = std::fs::read(&path).map_err(|_| format!("圖不見了：{path}"))?;

        let ext = std::path::Path::new(&path)
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
    .map_err(|e| e.to_string())?;
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
    allows_frames: bool,
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
    use sister_core::consent::Sheet;
    let c = sister_core::consent::load(dir);
    ConsentView {
        path: sister_core::consent::path(dir).display().to_string(),
        current: c.current(),
        allows_recording: c.allows_recording(),
        allows_frames: c.allows_frames(),
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
    if !c.current() {
        c = sister_core::consent::Consent::default();
    }
    if granted {
        c.grant(sheet, sister_core::now_ms());
    } else {
        c.revoke(sheet);
    }
    sister_core::consent::save(dir, &c).map_err(|e| e.to_string())?;
    Ok(consent_view(dir))
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
    .map_err(|e| e.to_string())?;
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
    /// 刪不掉的檔案。**不吞掉**：那幾張截圖還躺在磁碟上，而使用者以為
    /// 它們已經不在了。
    failed: Vec<String>,
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
            failed: r.failed,
        }
    }
}

/// 忘掉這一段會刪掉什麼。一句 DELETE 都沒有。
#[tauri::command]
fn forget_preview(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Erasure, String> {
    with_db(&shell, |db| {
        db.forget_preview(from_ts, to_ts)
            .map(Erasure::from)
            .map_err(|e| e.to_string())
    })
}

/// 真的刪。**沒有回收桶，沒有復原。**
///
/// 前端會先叫一次 `forget_preview` 把數字擺在使用者眼前，但那個順序是前端的
/// 禮貌，不是這裡的前提——這一支不管有沒有人預覽過都會照做，因為「一定要先
/// 預覽」的規則放在畫面上就等於沒有規則。真正的防線在 core：區間反過來的話
/// 一列都不動。
#[tauri::command]
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
    with_db_mut(&shell, |db| {
        db.forget(from_ts, to_ts, Some(&frames))
            .map(Erasure::from)
            .map_err(|e| e.to_string())
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
    .map_err(|e| e.to_string())?;
    Ok(())
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
        .map_err(|e| e.to_string())?;
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

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sister_desktop=info".into()),
        )
        .init();

    let data_dir = sister_core::config::Config::default_data_dir();
    let state_path = data_dir
        .clone()
        .unwrap_or_else(std::env::temp_dir)
        .join("pet-window.json");

    tauri::Builder::default()
        .manage(Shell {
            state: Mutex::new(bounds::load(&state_path).unwrap_or_default()),
            state_path,
            data_dir,
            db: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            toggle_pin,
            hide_to_tray,
            ask,
            open_frame,
            frame_image,
            pause_state,
            toggle_pause,
            settings_read,
            settings_write,
            lint_url_rules,
            open_settings,
            open_timeline,
            timeline_days,
            timeline_moments,
            forget_preview,
            forget_range,
            consent_read,
            consent_set,
            open_onboarding
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

            // ---- 系統匣 ----
            // 暫停放在系統匣，是因為視窗收起來的時候它是唯一按得到的地方——
            // 而「我現在不想被看」最常發生的時機，正是她不在畫面上的時候。
            //
            // 還缺一個全域熱鍵（PHASES.md Phase 1 那條「pause 快捷鍵」）。
            // 那要多一個 plugin，等設定頁一起做。
            let paused_now = pause_state(app.state::<Shell>());
            let show_item = MenuItem::with_id(app, "show", "顯示 AI-Sister", true, None::<&str>)?;
            let pause_item = MenuItem::with_id(
                app,
                "pause",
                pause_label(paused_now),
                true,
                None::<&str>,
            )?;
            let timeline_item =
                MenuItem::with_id(app, "timeline", "她記得的每一天…", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "設定…", true, None::<&str>)?;
            let consent_item =
                MenuItem::with_id(app, "consent", "三張同意書…", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "結束", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show_item,
                    &pause_item,
                    &timeline_item,
                    &settings_item,
                    &consent_item,
                    &quit_item,
                ],
            )?;
            app.manage(PauseItem(pause_item));

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
                        app.state::<Shell>().persist();
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
