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

use serde::Serialize;
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

#[tauri::command]
fn ask(question: String, shell: tauri::State<'_, Shell>) -> Result<Vec<Hit>, String> {
    let question = question.trim().to_string();
    if question.is_empty() {
        return Ok(Vec::new());
    }

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
    let db = slot.as_ref().expect("just opened");

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
    let slot = shell.db.lock().map_err(|_| "資料庫鎖壞了".to_string())?;
    let db = slot.as_ref().ok_or_else(|| "資料庫還沒開".to_string())?;

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
            toggle_pause
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
            let quit_item = MenuItem::with_id(app, "quit", "結束", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &pause_item, &quit_item])?;
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
