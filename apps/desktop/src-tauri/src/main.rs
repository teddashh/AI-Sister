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
use chrono::{Local, Timelike};
use sister_core::gatekeeper_candidates::CommitmentRef;
use sister_shell as bounds;
use sister_shell::{PetState, Rect};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, PhysicalPosition, WindowEvent};

mod hands;

/// 行動紀錄那一欄一次顯示幾列。
///
/// 有上限就一定要講出被蓋掉了幾列——安靜地截斷會讀成「總共就這幾件事」。
const ACTION_LOG_SHOWN: usize = 20;

const PET: &str = "pet";
const PET_W: i32 = 340;
const PET_H: i32 = 560;

static SETTINGS_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static ONBOARDING_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static TIMELINE_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static METRICS_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static FRAME_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);

/// 同一扇輔助視窗的非阻塞建立保留。
///
/// `get_webview_window(label)` 和 `WebviewWindowBuilder::build()` 不是同一個
/// 原子操作。tray thread 與 async command 同時連點時，兩邊可能都先看到
/// `None`，再各建一扇同 label 的窗。第二個入口只要知道第一個正在建就夠了；
/// 不等待、不再排一份工作。build 成功、失敗或提早 return 都由 Drop 放回來。
struct WindowOpening(&'static AtomicBool);

impl WindowOpening {
    fn claim(slot: &'static AtomicBool) -> Option<Self> {
        slot.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(slot))
    }
}

impl Drop for WindowOpening {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

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

#[derive(Serialize)]
struct GateEvidence {
    frame_id: i64,
    label: String,
}

#[derive(Serialize)]
struct GateDisplay {
    utterance_id: i64,
    form: &'static str,
    text: String,
    evidence: Vec<GateEvidence>,
    suggestion: Option<GateSuggestion>,
}

#[derive(Serialize)]
struct GateSuggestion {
    label: String,
    target_provenance: String,
    /// 畫面按這個 id 回叫，不回叫要執行什麼——見 [`hands_execute`]。
    commitment_id: i64,
}

#[derive(Serialize)]
struct GatekeeperView {
    display: Option<GateDisplay>,
    developer: Option<GatekeeperDeveloper>,
    action_log: Vec<String>,
}

#[derive(Serialize)]
struct GatekeeperDeveloper {
    points_spent: u32,
    points_limit: u32,
    holds: Vec<String>,
}

/// 把 core 的守門員判決接到每天真的會用的字母人。
///
/// **這個 command 是被輪詢的**，所以它的每一步都要問「同一件事被問第二次的
/// 時候會怎樣」：
///
/// 1. 同一件事今天只記一次帳（[`Db::utterance_today_for`]）。少了這一條，
///    「每天 5 點」量到的不是她講了幾句話，是畫面重新整理了幾次——五秒鐘
///    就用完，而人一句都還沒看到。
/// 2. 今天已經開口、人還沒回應的那一句**繼續顯示，不重扣點數**。她已經講
///    過了，重扣等於同一句話收兩次錢。
/// 3. 判決分兩趟：先全部判完、排名，**然後才寫**。落選的那幾句記成
///    [`HoldReason::OutrankedThisRound`]，不是記成 `spoke`——她一次只講一句，
///    而落選的那幾句人從來沒看到過。
/// 4. 擋下的理由**變了**才記一列。同一個理由連續輪詢不重複寫。
#[tauri::command(async)]
fn gatekeeper_check(shell: tauri::State<'_, Shell>) -> Result<GatekeeperView, String> {
    let now = sister_core::now_ms();
    let day_key = sister_core::local_day::local_day_key(now)
        .ok_or_else(|| "現在時間無法換成本地日期".to_string())?;
    let config = sister_core::Config::load(&config_path()?).map_err(|e| format!("{e:#}"))?;
    let local = Local::now();
    let quiet_hours_end = config
        .gatekeeper
        .quiet_end_at((local.hour() * 60 + local.minute()) as u16)
        .map_err(|e| format!("{e:#}"))?;
    let presence = shell
        .data_dir
        .as_ref()
        .map(|d| sister_core::heartbeat::presence(d, now))
        .unwrap_or(sister_core::heartbeat::Presence::NeverStarted);
    with_db_mut(&shell, |db| {
        let candidates = sister_core::gatekeeper_candidates::collect(db, now)
            .map_err(|e| format!("{e:#}"))?;
        let first = db.first_recording_at().map_err(|e| format!("{e:#}"))?.unwrap_or(now);
        let days_since = u32::try_from(now.saturating_sub(first) / 86_400_000).unwrap_or(u32::MAX);
        use sister_core::db::UtteranceDecision;
        use sister_core::gatekeeper::{HoldReason, Verdict};

        // 一次快照。迴圈裡重讀的話，第一句開口會把第二句的預算算成已經花掉，
        // 而這一輪最後只會有一句真的講出去。
        let spent = db.points_spent_today(&day_key).map_err(|e| format!("{e:#}"))?;
        let ever = db.has_ever_spoken().map_err(|e| format!("{e:#}"))?;

        // ── 第一趟：判，但不寫。 ──────────────────────────────────
        // 今天已經開口、人還沒回應的那一句，繼續掛著（不重判、不重扣）。
        let mut pending: Option<sister_core::db::UtteranceRow> = None;
        let mut judged: Vec<(sister_core::gatekeeper::Candidate, Verdict, Option<String>)> =
            Vec::new();
        let mut holds = Vec::new();
        for candidate in candidates {
            let existing = db
                .utterance_today_for(&day_key, candidate.category, &candidate.evidence)
                .map_err(|e| format!("{e:#}"))?;
            if let Some(row) = &existing
                && let UtteranceDecision::Spoke { .. } = row.decision
            {
                // 已經講過了。人還沒回應就繼續顯示；回應過就今天不再提。
                if row.reaction.is_none() && pending.as_ref().is_none_or(|p| p.id < row.id) {
                    pending = existing;
                }
                continue;
            }
            let previous_hold = existing.and_then(|row| match row.decision {
                UtteranceDecision::Held { reason } => Some(reason),
                UtteranceDecision::Spoke { .. } => None,
            });
            let cooldown_remaining_minutes = db
                .last_spoke_at(candidate.category)
                .map_err(|e| format!("{e:#}"))?
                .and_then(|last| {
                    let elapsed = now.saturating_sub(last) / 60_000;
                    (elapsed < i64::from(config.gatekeeper.cooldown_minutes))
                        .then(|| config.gatekeeper.cooldown_minutes - elapsed as u32)
                });
            let verdict = sister_core::gatekeeper::decide(&sister_core::gatekeeper::GateInput {
                candidate: candidate.clone(),
                presence,
                quiet_hours_end: quiet_hours_end.clone(),
                // 桌面目前也沒有可靠的跨平台前景視窗幾何；沒量到不是 Windowed。
                focus_mode: sister_core::gatekeeper::FocusMode::Unmeasured,
                days_since_first_recording: days_since,
                cold_start_days: config.gatekeeper.cold_start_days,
                has_ever_spoken: ever,
                cooldown_remaining_minutes,
                points_spent_today: spent,
                daily_budget_points: config.gatekeeper.daily_budget_points,
                min_score: config.gatekeeper.min_score,
            });
            judged.push((candidate, verdict, previous_hold));
        }

        // ── 排名：她一次只講一句。 ────────────────────────────────
        // 形式高的優先（建議卡 > 一行字 > 微光），同形式比分數。
        let mut winner = None;
        for (i, (candidate, verdict, _)) in judged.iter().enumerate() {
            if let Verdict::Speak { form, .. } = verdict {
                let rank = (form.cost(), candidate.score());
                if winner.is_none_or(|(_, best): (usize, (u32, f64))| {
                    rank.0 > best.0 || (rank.0 == best.0 && rank.1 > best.1)
                }) {
                    winner = Some((i, rank));
                }
            }
        }
        let winner = winner.map(|(i, _)| i);
        // 落選那幾句要講得出「輸給了哪一句」，所以先把贏家的原文留下來。
        let winner_text = winner.map(|i| judged[i].0.text.clone()).unwrap_or_default();

        // ── 第二趟：寫。 ─────────────────────────────────────────
        let mut display = None;
        for (i, (candidate, verdict, previous_hold)) in judged.into_iter().enumerate() {
            // 過得了關但不是這一輪最該講的那一句——記成落選，不是記成講過了。
            let verdict = match (&verdict, winner) {
                (Verdict::Speak { .. }, Some(w)) if w != i => {
                    Verdict::Hold(HoldReason::OutrankedThisRound {
                        by_text: winner_text.clone(),
                    })
                }
                _ => verdict,
            };
            // 擋下的理由沒變就不重複記——輪詢一次寫一列的話，這張表數的是
            // 畫面重新整理的次數，不是她考慮過幾次。
            let same_as_before = match (&verdict, &previous_hold) {
                (Verdict::Hold(reason), Some(before)) => {
                    before.starts_with(&format!("{}: ", reason.code()))
                }
                _ => false,
            };
            if same_as_before {
                if let Verdict::Hold(reason) = &verdict {
                    holds.push(reason.message());
                }
                continue;
            }
            let id = db
                .record_utterance(&sister_core::db::UtteranceInsert {
                    ts: now,
                    day_key: &day_key,
                    candidate: &candidate,
                    verdict: &verdict,
                })
                .map_err(|e| format!("{e:#}"))?;
            match verdict {
                Verdict::Speak { form, cost: _ } => {
                    let reference = CommitmentRef::from_candidate(candidate.commitment_id);
                    let suggestion = gate_suggestion(db, &reference, &mut holds)?;
                    let chips = frame_chips(&candidate.evidence);
                    holds.extend(evidence_not_on_screen(&candidate.evidence, &chips));
                    display = Some(GateDisplay {
                        utterance_id: id,
                        form: form.as_str(),
                        text: candidate.text,
                        evidence: chips,
                        suggestion,
                    });
                }
                Verdict::Hold(reason) => holds.push(reason.message()),
            }
        }
        // 舊帳優先：她已經講過而人還沒回應的那一句還掛在畫面上。
        if let Some(row) = pending {
            let form = match &row.decision {
                UtteranceDecision::Spoke { form, .. } => form.as_str(),
                // 上面已經濾過只留 Spoke；走到這裡代表濾網壞了，要看得見。
                UtteranceDecision::Held { .. } => {
                    return Err("pending 裡混進了一列 held——濾網壞了".into());
                }
            };
            let reference = CommitmentRef::from_evidence(&row.evidence);
            let suggestion = gate_suggestion(db, &reference, &mut holds)?;
            let chips = frame_chips(&row.evidence);
            holds.extend(evidence_not_on_screen(&row.evidence, &chips));
            display = Some(GateDisplay {
                utterance_id: row.id,
                form,
                text: row.text,
                evidence: chips,
                suggestion,
            });
        }
        let developer = if config.shell.developer_mode {
            Some(GatekeeperDeveloper {
                points_spent: db
                    .points_spent_today(&day_key)
                    .map_err(|e| format!("{e:#}"))?,
                points_limit: config.gatekeeper.daily_budget_points,
                holds: holds.into_iter().rev().take(5).collect(),
            })
        } else {
            None
        };
        let action_log = match &shell.data_dir {
            Some(dir) => sister_hands::ActionLog::in_data_dir(dir)
                .replay()
                .map(|replay| hands::recent_replay_lines(&replay, ACTION_LOG_SHOWN))
                .map_err(|e| format!("讀 action log 失敗：{e:#}"))?,
            None => vec!["這台機器上找不到資料目錄，action log 讀不到。".into()],
        };
        Ok(GatekeeperView { display, developer, action_log })
    })
}

/// 這張卡上要不要放「要我幫你…嗎」那顆按鈕。
///
/// 每一種不放按鈕的理由都各自留一句話給開發者模式看。安靜地回 `None` 的只有
/// 一種：這張卡根本不是在講承諾，那本來就沒有什麼下一步可做。
fn gate_suggestion(
    db: &sister_core::db::Db,
    reference: &CommitmentRef,
    developer_lines: &mut Vec<String>,
) -> Result<Option<GateSuggestion>, String> {
    use sister_hands::commitment_action::{AllowedNextStep, parse_allowed_next_step};
    if let Some(why) = reference.why_no_button() {
        developer_lines.push(why);
    }
    let CommitmentRef::One(id) = reference else {
        return Ok(None);
    };
    let id = *id;
    let row = db
        .commitment_by_id(id)
        .map_err(|e| format!("讀承諾 #{id} 失敗：{e:#}"))?;
    // 承諾可以被 `forget` 的血緣 cascade 整列刪掉，而那句話還掛在畫面上。
    // 這不該讓整個 gatekeeper 面板變成一句錯誤訊息、把她要講的話一起吃掉。
    let Some(row) = row else {
        developer_lines.push(format!("這句話指向的承諾 #{id} 已經不在了，所以不放按鈕。"));
        return Ok(None);
    };
    match parse_allowed_next_step(row.allowed_next_step.as_deref()) {
        AllowedNextStep::Missing => Ok(None),
        AllowedNextStep::Unparseable { raw, reason } => {
            developer_lines.push(format!("承諾 #{id} 的下一步讀不懂：{reason}；原文：{raw}"));
            Ok(None)
        }
        AllowedNextStep::Suggestion(button) => {
            let target = sister_core::db::target_app_for_button(&button, row.allowed_next_step_fact, |fact_id, raw| {
                db.app_for_target_fact(fact_id, raw)
                    .map_err(|e| format!("讀下一步目標 fact #{fact_id} 失敗：{e:#}"))
            })?;
            Ok(Some(gate_suggestion_from_target(
                id,
                &button,
                target.as_ref(),
            )))
        }
    }
}

fn gate_suggestion_from_target(
    commitment_id: i64,
    button: &sister_hands::SuggestionButton,
    target: Option<&sister_core::db::TargetApp>,
) -> GateSuggestion {
    let text = sister_core::db::suggestion_text(button, target);
    GateSuggestion {
        label: text.label,
        target_provenance: text.target_provenance,
        commitment_id,
    }
}

/// 按下那顆按鈕。
///
/// **參數是承諾的 id，不是要執行的動作。** 畫面送回來的字不會被拿去執行——
/// 要做什麼由這裡重新去資料庫讀一次。差別在於：前者是「畫面說要開這個」，
/// 後者是「她自己提過要開這個」，而 SPEC §9.7 要的是後者。
#[tauri::command(async)]
fn hands_execute(commitment_id: i64, shell: tauri::State<'_, Shell>) -> Result<String, String> {
    let data_dir = shell
        .data_dir
        .as_deref()
        .ok_or_else(|| "這台機器上找不到資料目錄，沒有動手。".to_string())?;
    let raw = with_db(&shell, |db| {
        let row = db
            .commitment_by_id(commitment_id)
            .map_err(|e| format!("讀承諾 #{commitment_id} 失敗：{e:#}"))?
            .ok_or_else(|| format!("承諾 #{commitment_id} 已經不在了，沒有動手。"))?;
        row.allowed_next_step
            .ok_or_else(|| format!("承諾 #{commitment_id} 上沒有寫下一步，沒有動手。"))
    })?;
    hands::execute_logged(data_dir, &raw, sister_core::now_ms())
}

/// evidence ref 裡指得到畫面的那幾個，變成可以點開的 chip。
///
/// 認不出來的 ref（`commitment:`、`segment:`、`fact:`）**不進來**：那不是
/// 「這張沒有畫面」，是「這一種 ref 指的不是畫面」。時間軸那邊的
/// `guessRow` 是同一套做法（`button.see` → `open_frame`）。
fn frame_chips(evidence: &[String]) -> Vec<GateEvidence> {
    evidence
        .iter()
        .filter_map(|r| {
            let frame_id = r.strip_prefix("frame:")?.parse::<i64>().ok()?;
            Some(GateEvidence {
                frame_id,
                label: format!("畫面 #{frame_id}"),
            })
        })
        .collect()
}

/// 這句話的出處一顆都點不開的時候，開發者那一欄要講出出處是什麼。
///
/// 畫面上收起那條空的 chip 帶不會說謊，但也不會說話。日終那種卡的出處是
/// `reviewer_run:` 和 `daysummary:`——「這句話沒有出處」和「這句話的出處
/// 不是畫面」是兩件事，而收起來之後兩者長得一樣。
fn evidence_not_on_screen(evidence: &[String], chips: &[GateEvidence]) -> Option<String> {
    if !chips.is_empty() || evidence.is_empty() {
        return None;
    }
    Some(format!(
        "這句話的出處點不開：{}——這幾種 ref 指的不是畫面。",
        evidence.join("、")
    ))
}

#[tauri::command(async)]
fn gatekeeper_react(utterance_id: i64, close: bool, shell: tauri::State<'_, Shell>) -> Result<String, String> {
    with_db_mut(&shell, |db| {
        let reaction = if close { sister_core::gatekeeper::Reaction::Close } else { sister_core::gatekeeper::Reaction::Other };
        let effect = sister_core::gatekeeper::react(db, utterance_id, reaction, sister_core::now_ms())
            .map_err(|e| format!("{e:#}"))?;
        Ok(match effect {
            sister_core::gatekeeper::CommitmentReaction::MarkDead { .. } => "這張記憶不會再提了",
            sister_core::gatekeeper::CommitmentReaction::SnoozeAndLowerWeight => "先收起來，之後再說",
            sister_core::gatekeeper::CommitmentReaction::None => "收到你的回饋；這一則沒有可結案或延後的承諾",
        }.to_string())
    })
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
/// # 為什麼回四個字串
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
/// 是在講這件事），而一個布林只答得出一個。收工想最後一段是第四種：沒在錄，
/// 但行程還在——那兩分鐘裡回 `"none"` 會讓他按開始，然後兩句話對打。
#[tauri::command]
fn recording_state(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) -> String {
    let presence = shell
        .data_dir
        .as_ref()
        .map(|dir| sister_core::heartbeat::presence(dir, sister_core::now_ms()))
        .unwrap_or(sister_core::heartbeat::Presence::NeverStarted);
    let now = sister_core::heartbeat::watching_word(presence);
    // 順手把系統匣那兩顆的字改對——手上已經有答案了，不必再讀一次磁碟。這不是
    // 唯一的刷新時機（見 [`refresh_tray`]），是最即時的那一個：視窗開著的時候，
    // 選單和字母人講的是同一秒的事。
    //
    // 那兩行字問的是「按下去會發生什麼」，不是「她在錄嗎」——正在起來的那一個
    // 也停得掉，也會被「結束」帶走，所以兩種都算佔著。
    set_record_labels(&app, presence);
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
    let (presence, paused) = match &shell.data_dir {
        Some(dir) => (
            // `is_occupied` 不是 `is_recording`：這兩行字問的是「按下去會發生
            // 什麼」。理由在 [`recording_state`] 上面。
            sister_core::heartbeat::presence(dir, sister_core::now_ms()),
            sister_core::pause::is_paused(dir),
        ),
        // 問不出資料目錄的時候，和 `recording_state` / `pause_state` 倒向同一邊：
        // 不確定就往「她做得比較少」那邊講。
        None => (sister_core::heartbeat::Presence::NeverStarted, true),
    };
    set_record_labels(app, presence);
    if let Some(item) = app.try_state::<PauseItem>() {
        let _ = item.0.set_text(pause_label(paused));
    }
}

/// 「開始／停止記錄」和「結束」那兩行字。分出來是因為 [`recording_state`] 手上
/// 已經有答案了，不必為了改字再讀一次磁碟。
///
/// 收完整 Presence，因為「佔著」有兩種按鍵後果：錄製／開機中能停止，Thinking
/// 只能等收尾。標籤直接沿用 core 裡 exhaustive 的三向答案。
fn set_record_labels(app: &tauri::AppHandle, presence: sister_core::heartbeat::Presence) {
    if let Some(item) = app.try_state::<RecordItem>() {
        let _ = item
            .0
            .set_text(sister_core::heartbeat::tray_record_label(presence));
    }
    if let Some(item) = app.try_state::<QuitItem>() {
        let _ = item
            .0
            .set_text(sister_core::heartbeat::tray_quit_label(presence));
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
    if let Some(why) = sister_core::heartbeat::occupied_why(dir, sister_core::now_ms()) {
        // 不是錯誤，但也不能安靜地再開一個：兩個 recorder 會各自錄一份，
        // 而使用者只會看到磁碟用得比講好的快一倍。想最後一段的那一種佔著
        // 不印「已經有一個在跑了」——心跳這時候說沒在錄，兩句會對打。
        return Err(why);
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
    followup: Option<String>,
    closure_notice: Option<String>,
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
    /// 問句裡認得出來的日曆範圍。`None` = 這句話沒有時間範圍，沒去算章節。
    time_range: Option<AskedTimeRange>,
    /// 那段時間切成的活動級段落。`None` 配 `time_range: None` = 沒算過；
    /// `Some([])` 配有範圍 = 算過，切不出段落。兩種不可以合成一個空陣列。
    ///
    /// 時間軸和答案端都是活動級。分鐘級 `segment` 在時間軸展開才看得到。
    chapters: Option<Vec<Chapter>>,
}

/// [`Answer::time_range`]：回述用的是他原話裡的那一段。
#[derive(Serialize)]
struct AskedTimeRange {
    from: i64,
    to: i64,
    said: String,
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
            followup: None,
            closure_notice: None,
            searched: None,
            query_id: None,
            answers: Vec::new(),
            hits: Vec::new(),
            // 空字串不是「問了但沒找到」，是根本沒問。
            blind: None,
            truncated: false,
            answers_truncated: false,
            time_range: None,
            chapters: None,
        });
    }
    let started = std::time::Instant::now();
    // 章節那一支要寫 `segment`，所以整條改拿可變借用。沒認到時間範圍
    // 時 `chapters_for_question` 立刻回 `None`，不會重算。
    with_db_mut(&shell, |db| {
        let now = sister_core::now_ms();
        let close = sister_core::reviewer::close_from_message(db, &question, now)
            .map_err(|e| format!("{e:#}"))?;
        let closure_notice = match close {
            sister_core::followup::CloseIntent::NotAClosure => None,
            sister_core::followup::CloseIntent::Unrecognized => Some("我認不出你指哪一張記憶，所以沒有動任何一張。".to_string()),
            sister_core::followup::CloseIntent::Ambiguous { .. } => Some("這句話對得上不只一張記憶，所以沒有動任何一張。".to_string()),
            sister_core::followup::CloseIntent::Close { .. } => Some("這張記憶已結案，不會再提。".to_string()),
        };
        let previous = sister_core::reviewer::followup_state(db).map_err(|e| format!("{e:#}"))?;
        let followup = match sister_core::followup::decide(&db.live_commitments().map_err(|e| format!("{e:#}"))?, now, previous.as_ref()) {
            sister_core::followup::FollowupDecision::Ask { commitment_id, text } => {
                sister_core::reviewer::record_followup(db, commitment_id, now).map_err(|e| format!("{e:#}"))?;
                Some(text)
            }
            sister_core::followup::FollowupDecision::NoEligibleCommitment
            | sister_core::followup::FollowupDecision::CoolingDown { .. } => None,
        };
        // 和 CLI、replay harness 共用同一條產品接線。字母人原本就是 facts 10
        // 筆、原文 20 筆，兩個上限各自保留，不為了共用函式偷偷改畫面密度。
        const FACTS: usize = 10;
        const HITS: usize = 20;
        let retrieval = sister_core::retrieval::RetrievalProfile::TextAndFacts
            .retrieve_with_limits(
                db,
                &question,
                sister_core::retrieval::RetrievalLimits::new(FACTS, HITS),
            )
            .map_err(|e| format!("{e:#}"))?;
        let shape = retrieval.shape;
        let facts = retrieval.answers;
        let facts_truncated = retrieval.answers_truncated;
        let hits = retrieval.hits;
        let truncated = retrieval.hits_truncated;
        let asked_chapters = db
            .chapters_for_question(&question, sister_core::now_ms())
            .map_err(|e| format!("{e:#}"))?;
        // **他打的那句話不進記錄檔。** 只留形狀、幾筆、幾毫秒——這三個數字
        // 足以回答「她是不是又卡住了」，而問題本身是他的東西，不是我的。
        tracing::info!(
            "問了一次（{}）：{} 個答案、{} 筆原文，{} ms",
            if shape == Shape::Recent || shape == Shape::Range {
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
            followup,
            closure_notice,
            // 只在**不一樣**的時候送。一樣的時候送過去，畫面那邊還要再比一次，
            // 而「這兩串字算不算同一句」是這裡才知道的事（`terms` 回的是原句的
            // 一個切片）。`Shape::Recent` 根本沒走比對那條路，所以也不送。
            searched: match shape {
                Shape::Recent | Shape::Range => None,
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
            time_range: asked_chapters.as_ref().map(|(r, _)| AskedTimeRange {
                from: r.from,
                to: r.to,
                said: r.said.clone(),
            }),
            chapters: asked_chapters
                .map(|(_, ch)| ch.into_iter().map(chapter_from_activity).collect()),
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

/// 時間軸上的一段。時間是含 5 秒重疊 margin 的顯示範圍。
#[derive(Serialize)]
struct Chapter {
    start_ts: i64,
    end_ts: i64,
    core_start_ts: i64,
    core_end_ts: i64,
    app: Option<String>,
    title: Option<String>,
    host: Option<String>,
    /// 打開這一段的切刀。空 = 當天第一段，沒有打開它的切刀。
    cut_kinds: Vec<String>,
    /// 沒有邊界可算是 `None`，不是 0.0。
    confidence: Option<f32>,
    /// 使用者編輯留下的形狀。演算法自己切的是 `None`。
    edited: Option<String>,
    /// 套用過的那一筆 `segment_edit.id`。沒有就是 `None`。
    edit_id: Option<i64>,
    /// 由幾個分鐘級 segment 併成。時間軸上的一格就是一段，不填。
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_count: Option<usize>,
    /// 核心時長。答案用這個。時間軸不填——那裡的 start_ts／end_ts 含 5 秒 margin。
    #[serde(skip_serializing_if = "Option::is_none")]
    core_ms: Option<i64>,
    /// 底下的分鐘級段落。活動級才填；答案端不送。
    #[serde(skip_serializing_if = "Option::is_none")]
    segments: Option<Vec<Chapter>>,
    /// 這一段上最新的 L2 假設。沒有就是沒有，不是空物件。
    #[serde(skip_serializing_if = "Option::is_none")]
    l2: Option<Vec<sister_core::brain::L2View>>,
}

fn chapter_from_segment(s: sister_core::segment::Segment) -> Chapter {
    Chapter {
        start_ts: s.started_at,
        end_ts: s.ended_at,
        core_start_ts: s.core_started_at,
        core_end_ts: s.core_ended_at,
        app: s.app,
        title: s.title,
        host: s.host,
        cut_kinds: s
            .cut_kinds
            .iter()
            .map(|k| k.as_str().to_string())
            .collect(),
        confidence: s.confidence,
        edited: s.last_edit.map(|e| e.kind.as_str().to_string()),
        edit_id: s.last_edit.map(|e| e.id),
        segment_count: None,
        core_ms: None,
        segments: None,
        l2: None,
    }
}

/// 答案端的一格。鐘面和時長都用核心時間；`segment_count` 說它是由幾段併成的。
fn chapter_from_activity(a: sister_core::activity::Activity) -> Chapter {
    chapter_from_activity_with_nested(a, false)
}

/// 時間軸上的一格。顯示範圍含 5 秒 margin（好把 moments 裝進去），時長仍用核心。
fn chapter_from_activity_timeline(a: sister_core::activity::Activity) -> Chapter {
    chapter_from_activity_with_nested(a, true)
}

fn chapter_from_activity_with_nested(
    a: sister_core::activity::Activity,
    nested: bool,
) -> Chapter {
    let core_ms = a.core_ms();
    let last = a.last_edit();
    let opening = a
        .segments
        .first()
        .map(|s| {
            s.cut_kinds
                .iter()
                .map(|k| k.as_str().to_string())
                .collect()
        })
        .unwrap_or_default();
    let confidence = a.segments.first().and_then(|s| s.confidence);
    let (start_ts, end_ts) = if nested {
        (a.started_at, a.ended_at)
    } else {
        (a.core_started_at, a.core_ended_at)
    };
    let segments = nested.then(|| {
        a.segments
            .into_iter()
            .map(chapter_from_segment)
            .collect()
    });
    Chapter {
        start_ts,
        end_ts,
        core_start_ts: a.core_started_at,
        core_end_ts: a.core_ended_at,
        app: a.app,
        title: a.title,
        host: a.host,
        cut_kinds: opening,
        confidence,
        edited: last.map(|e| e.kind.as_str().to_string()),
        edit_id: last.map(|e| e.id),
        segment_count: Some(a.segment_count),
        core_ms: Some(core_ms),
        segments,
        l2: None,
    }
}

/// 某一天切成的段落。打開時間軸才算，不在錄製那條路上。
#[tauri::command(async)]
fn timeline_chapters(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<Chapter>, String> {
    with_db_mut(&shell, |db| {
        let cards = db.l2_in_range(from_ts, to_ts).map_err(|e| format!("{e:#}"))?;
        Ok(db
            .activities_for_range(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|a| {
                let mut ch = chapter_from_activity_timeline(a);
                attach_l2(&mut ch, &cards);
                ch
            })
            .collect())
    })
}

fn attach_l2(ch: &mut Chapter, cards: &[sister_core::db::L2CardRow]) {
    let mut starts = vec![ch.core_start_ts];
    if let Some(segs) = &ch.segments {
        starts.extend(segs.iter().map(|s| s.core_start_ts));
    }
    let mut views = Vec::new();
    for start in starts {
        let mut versions: Vec<&sister_core::db::L2CardRow> = cards
            .iter()
            .filter(|c| c.segment_core_start == start)
            .collect();
        versions.sort_by_key(|c| (c.version, c.id));
        if let Some((row, prev)) = sister_core::brain::latest_with_previous(&versions) {
            let row = *row;
            let prev = prev.copied();
            let view = sister_core::brain::view_from_row_with_previous(row, prev);
            if !views
                .iter()
                .any(|v: &sister_core::brain::L2View| v.segment_ref == view.segment_ref)
            {
                views.push(view);
            }
        }
    }
    ch.l2 = if views.is_empty() { None } else { Some(views) };
}

fn timeline_chapters_after_edit(
    segs: Vec<sister_core::segment::Segment>,
    cards: &[sister_core::db::L2CardRow],
) -> Vec<Chapter> {
    sister_core::activity::group(&segs)
        .into_iter()
        .map(|a| {
            let mut ch = chapter_from_activity_timeline(a);
            attach_l2(&mut ch, cards);
            ch
        })
        .collect()
}

/// 把相鄰兩段併成一段。立刻回新的章節清單，不用重開視窗。
///
/// `left_core_start`／`right_core_start` 仍是分鐘級 segment 的核心起點。
/// 活動級畫面上「與下一段合併」要傳左件最後一段、右件第一段。
#[tauri::command(async)]
fn timeline_merge_chapters(
    left_core_start: i64,
    right_core_start: i64,
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<Chapter>, String> {
    with_db_mut(&shell, |db| {
        let segs = db
            .merge_chapters(left_core_start, right_core_start, from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        let cards = db.l2_in_range(from_ts, to_ts).map_err(|e| format!("{e:#}"))?;
        Ok(timeline_chapters_after_edit(segs, &cards))
    })
}

/// 在 `at_ts` 把一段切成兩段。
#[tauri::command(async)]
fn timeline_split_chapter(
    at_ts: i64,
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<Chapter>, String> {
    with_db_mut(&shell, |db| {
        let segs = db
            .split_chapter(at_ts, from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        let cards = db.l2_in_range(from_ts, to_ts).map_err(|e| format!("{e:#}"))?;
        Ok(timeline_chapters_after_edit(segs, &cards))
    })
}

/// 撤銷某一筆合併或切開。多寫一列，不改舊的訓練訊號。
#[tauri::command(async)]
fn timeline_undo_segment_edit(
    edit_id: i64,
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<Chapter>, String> {
    with_db_mut(&shell, |db| {
        let segs = db
            .undo_segment_edit(edit_id, from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        let cards = db.l2_in_range(from_ts, to_ts).map_err(|e| format!("{e:#}"))?;
        Ok(timeline_chapters_after_edit(segs, &cards))
    })
}

#[tauri::command(async)]
fn memory_guesses(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<sister_core::brain::L2View>, String> {
    with_db(&shell, |db| {
        let cards = db.l2_in_range(from_ts, to_ts).map_err(|e| format!("{e:#}"))?;
        let mut by_seg: std::collections::BTreeMap<i64, Vec<&sister_core::db::L2CardRow>> =
            std::collections::BTreeMap::new();
        for c in &cards {
            by_seg.entry(c.segment_core_start).or_default().push(c);
        }
        let mut views = Vec::new();
        for versions in by_seg.values() {
            let mut ordered = versions.clone();
            ordered.sort_by_key(|c| (c.version, c.id));
            if let Some((row, prev)) = sister_core::brain::latest_with_previous(&ordered) {
                let row = *row;
                let prev = prev.copied();
                views.push(sister_core::brain::view_from_row_with_previous(row, prev));
            }
        }
        Ok(views)
    })
}

#[derive(Serialize)]
struct CurrentGuessView {
    status: sister_core::brain::CurrentGuess,
    message: String,
    card: Option<sister_core::brain::L2View>,
}

/// 「現在」只在這裡組資料；狀態分類與人看到的字都由 sister-core 決定。
#[tauri::command(async)]
fn memory_current_guess(shell: tauri::State<'_, Shell>) -> Result<CurrentGuessView, String> {
    let dir = shell
        .data_dir
        .as_deref()
        .ok_or_else(|| "找不到資料目錄，不能判斷現在的錄製狀態".to_string())?;
    let presence = sister_core::heartbeat::presence(dir, sister_core::now_ms());
    let paused = sister_core::pause::is_paused(dir);
    let mut card = None;
    let status = sister_core::brain::CurrentGuess::decide(presence, paused, || {
        let config =
            sister_core::config::Config::load(&config_path()?).map_err(|e| format!("{e:#}"))?;
        let consented = sister_core::consent::load(dir).cloud_permit().is_some();
        let now = sister_core::now_ms();
        with_db_mut(&shell, |db| {
            let from = db
                .current_session_started_at()
                .map_err(|e| format!("{e:#}"))?
                .unwrap_or(now);
            let mut segments = db
                .chapters_for_range(from, now)
                .map_err(|e| format!("{e:#}"))?;
            // 錄製中最後一段還開著，解釋層不送它：`wakeup.rs` 的 `include_open ==
            // false` 那條路把右界設成 `segs.last().core_started_at`，而
            // `chapters_for_range` 過濾 `core_started_at < to_ts`，所以最後一段剛好
            // 被排除在外。這裡照同一條線拿掉它，再看最新的已關閉段落——不然畫面會
            // 對一段永遠不會有卡的段落說「還在排隊」。
            segments.pop();
            let latest = segments.pop();
            let Some(seg) = latest else {
                return Ok(sister_core::brain::RecordingFacts {
                    latest_closed: None,
                    has_command: config.brain.cli().is_some(),
                    consented,
                    used_today: 0,
                    daily_budget: config.brain.daily_budget,
                    previous_attempts: None,
                });
            };
            let versions = db
                .l2_versions_for_segment(seg.core_started_at)
                .map_err(|e| format!("{e:#}"))?;
            card = sister_core::brain::latest_with_previous(&versions).map(|(row, previous)| {
                sister_core::brain::view_from_row_with_previous(row, previous)
            });
            let facts = db
                .facts_in_range(seg.core_started_at, seg.core_ended_at)
                .map_err(|e| format!("{e:#}"))?;
            let large_clip = db
                .clipboard_in_range(seg.core_started_at, seg.core_ended_at)
                .map_err(|e| format!("{e:#}"))?
                .iter()
                .any(|c| c.byte_len >= sister_core::segment::LARGE_CLIPBOARD_BYTES);
            let stuck = db
                .stuck_in_range(seg.core_started_at, seg.core_ended_at)
                .map_err(|e| format!("{e:#}"))?
                .iter()
                .any(|s| s.started_at < seg.core_ended_at && s.ended_at > seg.core_started_at);
            let worth = sister_core::brain::worth_interpreting(&seg, &facts, large_clip, stuck);
            let day = sister_core::brain::local_day_key(now)
                .ok_or_else(|| "算不出今天的日期，不能核對解釋預算".to_string())?;
            let used = db
                .brain_outbound_count_on(&day)
                .map_err(|e| format!("{e:#}"))?;
            // 這一格的查詢一律把錯誤往上帶，整塊算完才交出去；任何一個查詢失敗，
            // 包括上面已經算好的 card，都會讓整塊失敗。brain 外送已經發生後的輔助查詢
            // 則不能擋住那次外送，所以 brain.rs 那邊會用 `.ok().flatten()`。
            let previous_attempts = db
                .retained_interpreter_attempts_for_segment(
                    seg.core_started_at,
                    seg.core_ended_at,
                )
                .map_err(|e| format!("{e:#}"))?;
            let latest_closed = sister_core::brain::LatestClosedSegment {
                has_card: card.is_some(),
                worth_interpreting: worth,
            };
            Ok(sister_core::brain::RecordingFacts {
                latest_closed: Some(latest_closed),
                has_command: config.brain.cli().is_some(),
                consented,
                used_today: used,
                daily_budget: config.brain.daily_budget,
                previous_attempts,
            })
        })
    })?;
    Ok(CurrentGuessView {
        message: status.message(),
        status,
        card,
    })
}

#[derive(Serialize)]
struct PledgeView {
    id: i64,
    text: String,
    kind: String,
    status: String,
    due_hint: Option<String>,
    due_source: Option<String>,
    confidence: f64,
    tombstoned: bool,
    kill_note: Option<String>,
    evidence: Vec<sister_core::brain::L2EvidenceView>,
}

#[tauri::command(async)]
fn memory_commitments(shell: tauri::State<'_, Shell>) -> Result<Vec<PledgeView>, String> {
    with_db(&shell, |db| {
        let rows = db.all_commitments().map_err(|e| format!("{e:#}"))?;
        Ok(rows
            .into_iter()
            .map(|c| {
                let refs: Vec<String> =
                    serde_json::from_str(&c.evidence_json).unwrap_or_default();
                PledgeView {
                    id: c.id,
                    text: c.text,
                    kind: c.kind,
                    status: c.status,
                    due_hint: c.due_hint,
                    due_source: c.due_source,
                    confidence: c.confidence,
                    tombstoned: c.tombstoned_at.is_some(),
                    kill_note: c.kill_note,
                    evidence: refs
                        .iter()
                        .filter_map(|s| sister_core::brain::EvidenceRef::parse(s))
                        .map(|r| match r {
                            sister_core::brain::EvidenceRef::Frame(id) => {
                                sister_core::brain::L2EvidenceView {
                                    kind: "frame",
                                    id,
                                    label: format!("畫面 #{id}"),
                                }
                            }
                            sister_core::brain::EvidenceRef::Fact(id) => {
                                sister_core::brain::L2EvidenceView {
                                    kind: "fact",
                                    id,
                                    label: format!("本機事實 #{id}"),
                                }
                            }
                        })
                        .collect(),
                }
            })
            .collect())
    })
}

#[derive(Serialize)]
struct OutboundLine {
    ts: i64,
    command: String,
    args: Vec<String>,
    chars_sent: i64,
    truncated: bool,
    outcome: String,
    duration_ms: i64,
    error: Option<String>,
    role: String,
}

#[derive(Serialize)]
struct SkipLine {
    ts: i64,
    reason: String,
    detail: String,
}

#[derive(Serialize)]
struct OutboundLog {
    outbound: Vec<OutboundLine>,
    skips: Vec<SkipLine>,
    ever_sent: bool,
}

#[tauri::command(async)]
fn memory_outbound(limit: Option<u32>, shell: tauri::State<'_, Shell>) -> Result<OutboundLog, String> {
    let take = limit.unwrap_or(200).clamp(1, 500) as usize;
    with_db(&shell, |db| {
        let outbound = db
            .list_brain_outbound(take)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|row| OutboundLine {
                ts: row.ts,
                command: row.command,
                args: serde_json::from_str(&row.args_json).unwrap_or_default(),
                chars_sent: row.chars_sent,
                truncated: row.truncated,
                outcome: row.outcome,
                duration_ms: row.duration_ms,
                error: row.error,
                role: row.role,
            })
            .collect();
        let skips = db
            .list_brain_skip(take)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|row| SkipLine {
                ts: row.ts,
                reason: row.reason,
                detail: row.detail,
            })
            .collect();
        let ever_sent = db.ever_brain_outbound().map_err(|e| format!("{e:#}"))?;
        Ok(OutboundLog {
            outbound,
            skips,
            ever_sent,
        })
    })
}

/// 這一天的日摘要。三種「沒有」是三個 `kind`，不是同一個空物件。
#[tauri::command(async)]
fn memory_day_summary(
    from_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<sister_core::db::DaySummaryGlance, String> {
    let date = sister_core::brain::local_day_key(from_ts)
        .ok_or_else(|| "算不出這一天的日期".to_string())?;
    with_db(&shell, |db| {
        db.day_summary_glance(&date).map_err(|e| format!("{e:#}"))
    })
}

#[tauri::command(async)]
fn correct_l2(
    segment_core_start: i64,
    activity: String,
    shell: tauri::State<'_, Shell>,
) -> Result<(), String> {
    with_db_mut(&shell, |db| {
        sister_core::reviewer::correct_l2(db, segment_core_start, &activity)
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
    })
}

#[tauri::command(async)]
fn commitment_kill(
    id: i64,
    note: Option<String>,
    shell: tauri::State<'_, Shell>,
) -> Result<(), String> {
    with_db_mut(&shell, |db| {
        sister_core::reviewer::kill_commitment(
            db,
            id,
            note.as_deref().unwrap_or("使用者結案"),
            sister_core::now_ms(),
        )
        .map_err(|e| format!("{e:#}"))?;
        Ok(())
    })
}

#[tauri::command(async)]
fn commitment_other(id: i64, shell: tauri::State<'_, Shell>) -> Result<(), String> {
    with_db_mut(&shell, |db| {
        sister_core::reviewer::snooze_commitment(db, id, sister_core::now_ms())
            .map_err(|e| format!("{e:#}"))?;
        Ok(())
    })
}

/// 設定頁上看得到、改得動的那幾項。
///
/// **刻意只是設定檔的一個子集。** 截圖間隔、去重門檻那些沒有放進來，因為它們
/// 改了要重開 `record` 才生效（見 `Recorder::set_privacy`）——一個按了儲存卻
/// 要等重開才生效、而且沒說的欄位，比沒有那個欄位更糟。
///
/// `[brain]` 的每日預算、併發、審閱預算同樣沒畫：它們有 `check()` 在守
/// （concurrency 1..=8），而且改錯了的後果是悄悄降級，不是看得見的東西。
/// 這一頁只動 `command` / `args`；另外三個必須在「先讀再改再寫」之後原封不動。
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
    /// `[brain] command`。空字串＝沒設定＝一次都不 spawn。
    #[serde(default)]
    brain_command: String,
    /// `[brain] args`。一行一個；prompt 走 stdin，不在這裡。
    #[serde(default)]
    brain_args: Vec<String>,
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
        brain_command: c.brain.command,
        brain_args: c.brain.args,
        path: path.display().to_string(),
    })
}

/// 把 CLI 產生的完整 eval report 縮成開發者指標頁能看的那一層。
///
/// 瀏覽器端會讀使用者明確選的檔案，再把內容送進這支 command。這裡
/// 不收路徑，所以一扇開發頁不會變成可以任意讀本機檔案的介面。
/// 回傳型別也刻意不含 report 裡任何自由字串；解析失敗也不把原值抄進畫面。
#[tauri::command(async)]
fn eval_report_view(contents: String) -> Result<sister_core::eval::MetricsView, String> {
    sister_core::eval::metrics_view_from_json(&contents)
        .map_err(|_| "JSON 格式或 eval report 版本不符合這一版 sister".to_string())
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
    /// 心跳現在說什麼：`"recording"`／`"booting"`／`"thinking"`／`"none"`。決定那句話怎麼講。
    ///
    /// **四個值，不是一個布林。** 上一版是 `recording: bool`（`is_recording`），
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
    // 只動這兩格。daily_budget / concurrency / reviewer_daily_budget 這一頁
    // 沒畫，從頭組一份 BrainConfig 會把它們重設成預設值。守這一點的測試是
    // `a_settings_page_write_must_not_reset_unexposed_brain_fields`。
    c.set_brain_cli_from_page(settings.brain_command, settings.brain_args);
    c.save(&path).map_err(|e| format!("{e:#}"))?;
    // 存成功之後才問。反過來的話，一個存不進去的檔案會拿到一句「5 秒內換上」。
    Ok(WriteOutcome {
        watching: shell
            .data_dir
            .as_ref()
            .map(|dir| {
                sister_core::heartbeat::watching_word(sister_core::heartbeat::presence(
                    dir,
                    sister_core::now_ms(),
                ))
            })
            .unwrap_or("none"),
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

/// 從同步系統匣 event handler 開一扇 WebView。
///
/// Tauri / WebView2 在 Windows 有一條明列的 deadlock：在 event handler
/// 裡直接 `WebviewWindowBuilder::build` 會卡在 controller callback，只留下
/// 有標題、全白、`Not Responding` 的原生窗。系統匣 callback 只准走到
/// 這裡；真正建視窗在獨立 OS thread 裡做。
fn spawn_window(
    app: tauri::AppHandle,
    description: &'static str,
    open: fn(tauri::AppHandle) -> Result<(), String>,
) {
    let _window_thread = std::thread::spawn(move || {
        if let Err(e) = open(app) {
            tracing::error!("{description}開不起來：{e}");
        }
    });
}

/// 真正建立設定頁的 internal helper。
///
/// Windows WebView2 在 synchronous command 或 event handler 裡直接建
/// `WebviewWindow` 會 deadlock。這支所以不是 command：IPC 入口由底下
/// 的 async wrapper 叫，系統匣則一律經 [`spawn_window`]。
fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&SETTINGS_WINDOW_OPENING) else {
        return Ok(());
    };
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

/// 開設定頁。同一個 label 重複用，所以按兩次不會得到兩個視窗。
#[tauri::command]
async fn open_settings(app: tauri::AppHandle) -> Result<(), String> {
    open_settings_window(app)
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

/// 真正建立同意書頁的 internal helper。只准從 async command、
/// [`spawn_window`] 或 Tauri 明確允許同步建視窗的 setup hook 進來。
fn open_onboarding_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&ONBOARDING_WINDOW_OPENING) else {
        return Ok(());
    };
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

/// 開同意書那一頁。同一個 label 重複用。
#[tauri::command]
async fn open_onboarding(app: tauri::AppHandle) -> Result<(), String> {
    open_onboarding_window(app)
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
    /// 那段時間裡她按你的指示動過幾次手（`action-log.jsonl`）。
    ///
    /// **這一欄不在資料庫裡**，所以 `From<PruneReport>` 給不出它，兩個呼叫端
    /// 各自要補。上面那一段講題庫、畫面紀錄、錄製紀錄的註解，講的都是同一個
    /// 故事：一類東西被刪掉了、卻沒有出現在這張清單上。這是第四次，而這一次
    /// 那類東西是完整的網址和檔案路徑。
    actions: u64,
    /// 讀不懂、問不出時間，因此忘掉時也會被刪掉的 action-log 列數。
    actions_unreadable: u64,
    /// 存著的授權書不屬於時間區間；按下忘掉仍會整張刪除。
    grant: bool,
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
            // action log 不在資料庫裡，`PruneReport` 看不到它。兩個呼叫端各自
            // 問一次 `ActionLog`，所以這裡只能是 0——和下面 `sessions_left`
            // 同一個模式：這一支答不出來的，不要在這裡編一個。
            actions: 0,
            actions_unreadable: 0,
            grant: false,
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
    // 預覽也要把她那段時間動過的手算進去；可讀列和讀不懂但仍會被刪掉的列
    // 分開回，和真正刪除的 `ForgetReport` 是同一組數字。
    let actions = sister_hands::ActionLog::in_data_dir(dir)
        .count_in_range(from_ts, to_ts)
        .map_err(|e| format!("{e:#}"))?;
    let grant = sister_hands::semi_action::grant_path(dir).exists()
        || sister_hands::semi_action::grant_tmp_path(dir).exists();
    with_db(&shell, |db| {
        db.forget_preview(from_ts, to_ts, Some(&frames))
            .map(|report| Erasure {
                actions: actions.removed_in_range,
                actions_unreadable: actions.removed_unreadable,
                grant,
                ..Erasure::from(report)
            })
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
    // 要保留 Thinking；它和「當掉」的下一步不同。
    let beat = match sister_core::heartbeat::watching_word(sister_core::heartbeat::presence(
        dir,
        sister_core::now_ms(),
    )) {
        "recording" => "live",
        other => other,
    };
    // 資料庫和畫面之外，`action-log.jsonl` 裡也有那一段的完整網址與檔案路徑。
    // 少了這一刀，那句「已經忘掉了」只對一半的磁碟成立。CLI 那邊是同一句話
    // （`crates/sister-cli/src/ops.rs` 的 `forget`），兩邊都要做。
    //
    // 在借資料庫之前先做：這一刀失敗要整個停下來，不能發生「資料庫刪了、
    // 檔案沒刪」而畫面照樣報成功。
    let forgotten = sister_hands::ActionLog::in_data_dir(dir)
        .forget_range(from_ts, to_ts)
        .map_err(|err| format!("{err:#}"))?;
    // 授權書。**同一支函式，CLI 的 `sister forget` 也走它**——兩邊各寫一份
    // 的話，改天多一個檔案只會補到其中一邊，而兩邊都照樣說「已經忘掉了」。
    let grant = !sister_hands::semi_action::forget_saved_grant(dir)
        .map_err(|err| format!("刪除授權書失敗：{err}"))?
        .is_empty();
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
            actions: forgotten.removed_in_range,
            actions_unreadable: forgotten.removed_unreadable,
            grant,
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
fn open_timeline_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&TIMELINE_WINDOW_OPENING) else {
        return Ok(());
    };
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

#[tauri::command]
async fn open_timeline(app: tauri::AppHandle) -> Result<(), String> {
    open_timeline_window(app)
}

/// 開發者模式才會在系統匣出現的評測摘要頁。
///
/// 它不自己跑評測，也不去猜資料目錄；只讀使用者在頁面上明確選中的
/// `replay evaluate --to` 報告。同一個 label 重複使用，避免開出兩份互相
/// 不知道對方載了哪個檔案的數字。
fn open_metrics_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&METRICS_WINDOW_OPENING) else {
        return Ok(());
    };
    const METRICS: &str = "metrics";
    if let Some(win) = app.get_webview_window(METRICS) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(&app, METRICS, tauri::WebviewUrl::App("metrics.html".into()))
        .title("AI-Sister 開發者指標")
        .inner_size(1080.0, 720.0)
        .min_inner_size(720.0, 480.0)
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
fn open_frame_window(app: tauri::AppHandle, frame_id: i64) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&FRAME_WINDOW_OPENING) else {
        return Ok(());
    };
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

#[tauri::command]
async fn open_frame(app: tauri::AppHandle, frame_id: i64) -> Result<(), String> {
    open_frame_window(app, frame_id)
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
            eval_report_view,
            lint_url_rules,
            privacy_health,
            open_settings,
            open_timeline,
            timeline_days,
            timeline_moments,
            timeline_chapters,
            timeline_merge_chapters,
            timeline_split_chapter,
            timeline_undo_segment_edit,
            memory_guesses,
            memory_current_guess,
            memory_commitments,
            memory_outbound,
            memory_day_summary,
            correct_l2,
            commitment_kill,
            commitment_other,
            forget_preview,
            forget_range,
            consent_read,
            consent_set,
            open_onboarding,
            hotkey_state,
            hotkey_set
            ,gatekeeper_check
            ,gatekeeper_react
            ,hands_execute
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
            let hands_stop_item =
                MenuItem::with_id(app, "hands-stop", "拔掉她的手", true, None::<&str>)?;
            let hands_resume_item =
                MenuItem::with_id(app, "hands-resume", "把手接回去", true, None::<&str>)?;
            // 開始／停止和暫停是兩件事，所以是兩顆。暫停是「先別看，但留在
            // 這裡」，停止是「今天到此為止」——把停止做成「一直暫停」會留下
            // 一個永遠在跑卻永遠不做事的行程，而他在工作管理員裡看得到它。
            //
            // 這兩行字問的是「按下去會發生什麼」，所以看的是**有沒有人佔著**
            // ——理由在 [`set_record_labels`] 上面。
            let presence_now = app
                .state::<Shell>()
                .data_dir
                .as_ref()
                .map(|dir| sister_core::heartbeat::presence(dir, sister_core::now_ms()))
                .unwrap_or(sister_core::heartbeat::Presence::NeverStarted);
            let record_item = MenuItem::with_id(
                app,
                "record",
                sister_core::heartbeat::tray_record_label(presence_now),
                true,
                None::<&str>,
            )?;
            let timeline_item =
                MenuItem::with_id(app, "timeline", "她記得的每一天…", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "設定…", true, None::<&str>)?;
            let consent_item =
                MenuItem::with_id(app, "consent", "三張同意書…", true, None::<&str>)?;
            // 開發者入口預設不存在，不是放一顆灰掉的按鈕讓一般使用者猜。
            // 設定檔讀不懂時也維持隱藏；這個選項只加一扇工具頁，沒有理由在
            // 不確定時自行打開。改完設定要重開桌面殼，選單才會重建。
            let developer_mode = config_path()
                .and_then(|path| {
                    sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}"))
                })
                .map(|config| config.shell.developer_mode)
                .unwrap_or(false);
            let metrics_item = if developer_mode {
                Some(MenuItem::with_id(
                    app,
                    "metrics",
                    "評測指標…",
                    true,
                    None::<&str>,
                )?)
            } else {
                None
            };
            let quit_item = MenuItem::with_id(
                app,
                "quit",
                sister_core::heartbeat::tray_quit_label(presence_now),
                true,
                None::<&str>,
            )?;
            let menu = match &metrics_item {
                Some(metrics_item) => Menu::with_items(
                    app,
                    &[
                        &show_item,
                        &record_item,
                        &pause_item,
                        &hands_stop_item,
                        &hands_resume_item,
                        &timeline_item,
                        &settings_item,
                        &consent_item,
                        metrics_item,
                        &quit_item,
                    ],
                )?,
                None => Menu::with_items(
                    app,
                    &[
                        &show_item,
                        &record_item,
                        &pause_item,
                        &hands_stop_item,
                        &hands_resume_item,
                        &timeline_item,
                        &settings_item,
                        &consent_item,
                        &quit_item,
                    ],
                )?,
            };
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
                    "hands-stop" | "hands-resume" => {
                        let shell = app.state::<Shell>();
                        let resume = event.id.as_ref() == "hands-resume";
                        let changed = shell
                            .data_dir
                            .as_ref()
                            .ok_or_else(|| "資料目錄讀不到".to_string())
                            .and_then(|dir| {
                                if resume {
                                    sister_hands::kill_switch::release(dir)
                                        .map(|_| ())
                                        .map_err(|e| e.to_string())
                                } else {
                                    sister_hands::kill_switch::pull(dir, sister_core::now_ms())
                                        .map(|_| ())
                                        .map_err(|e| e.to_string())
                                }
                            });
                        match changed {
                            Ok(()) => refresh_tray(app),
                            Err(e) => {
                                tracing::error!("拔手開關切換失敗：{e}");
                                if let Some(win) = app.get_webview_window(PET) {
                                    let _ = win.show();
                                    let _ = win.set_focus();
                                }
                                use tauri::Emitter;
                                let _ = app.emit("recorder-failed", format!("拔手開關失敗：{e}"));
                                refresh_tray(app);
                            }
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
                        let now = sister_core::now_ms();
                        let presence = shell
                            .data_dir
                            .as_ref()
                            .map(|dir| sister_core::heartbeat::presence(dir, now))
                            .unwrap_or(sister_core::heartbeat::Presence::NeverStarted);
                        let action = sister_core::heartbeat::tray_record_action(presence);
                        let done = match action {
                            sister_core::heartbeat::TrayRecordAction::Start => {
                                start_recording(shell.clone())
                            }
                            sister_core::heartbeat::TrayRecordAction::Stop => {
                                stop_recording(shell.clone())
                            }
                            sister_core::heartbeat::TrayRecordAction::WaitForThinking => Err(
                                sister_core::heartbeat::occupied_why_of(presence, now)
                                    .expect("Thinking 一定有 occupied_why"),
                            ),
                        };
                        match done {
                            // 立刻改字，不等下一次輪詢——按了之後那一顆要當場
                            // 看起來不一樣，不然他會再按一次。
                            Ok(()) => {
                                refresh_tray(app);
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
                        spawn_window(app.clone(), "時間軸", open_timeline_window);
                    }
                    "settings" => {
                        spawn_window(app.clone(), "設定頁", open_settings_window);
                    }
                    "consent" => {
                        spawn_window(app.clone(), "同意書", open_onboarding_window);
                    }
                    "metrics" => {
                        spawn_window(app.clone(), "評測指標", open_metrics_window);
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
                && let Err(e) = open_onboarding_window(app.handle().clone())
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
