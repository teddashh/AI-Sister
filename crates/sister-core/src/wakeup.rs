//! 錄製時讓解釋層／審閱層自己醒。
//!
//! SPEC §5.1 寫著「不是秒針」：算力跟資訊價值走，不是每 N 秒去問資料庫。
//! SPEC §6 的審閱層才是節奏型——活躍時 15 分鐘一輪、換日就日終。
//!
//! 這一層活在**另一條執行緒**上，有自己的資料庫連線。錄製熱路徑只准做
//! [`Handle::ping`]：一次 `AtomicBool::store`，不開連線、不查詢、不等待。
//! 模型 CLI 卡住也只卡住這條慢路徑；擷取迴圈看都看不到它。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::brain::{self, InterpretInput, OutboundOutcome, SkipReason as BrainSkip};
use crate::config::{BrainConfig, Config};
use crate::db::Db;
use crate::heartbeat;
use crate::local_day;
use crate::model::Millis;
use crate::reviewer::{self, ReviewInput, ReviewKind, SkipReason as ReviewSkip};
use crate::segment::{LOOKAROUND_MS, TIME_CAP_MS};

pub use crate::local_day::previous_local_day_key;

/// 腦執行緒睡一小段再看旗標。這不是在輪詢資料庫，只是讓
/// [`Handle::ping`]／收工能在幾十毫秒內被看到。
const SLEEP_SLICE: Duration = Duration::from_millis(50);

/// 沒設定 CLI 時，執行緒根本不會起來。見 [`Handle::maybe_spawn`]。
pub fn armed(brain: &BrainConfig) -> bool {
    brain.cli().is_some()
}

/// 收工時還開著的最後一段怎麼了。
///
/// 三種空各一句，不准和 [`format_report`] 裡「這一場一次都沒醒」「醒了沒東
/// 西可想」印成同一句——那些講的是整場已關閉的段落，這一個講的是按下停止
/// 的那一刻還開著的尾巴。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LastSegment {
    /// 沒走到收工那一步（沒設定 CLI、執行緒沒起來、沒同意書、預算用盡）。
    #[default]
    Skipped,
    /// 看過了，沒有值得理解的訊號。痕跡在 `brain_skips`，下次開機
    /// [`Engine::catch_up`] 看得到已關閉的那一段。
    NothingWorth,
    /// 跑過 CLI，在時限內結束。
    Ran,
    /// 等到 [`shutdown_think_bound`] 的上限，沒想完。痕跡在外送紀錄的
    /// `timeout`，下次開機 catch_up 會再補一次。
    TimedOut,
}

/// 收工時解釋層把最後一段想完的牆上時間上限。
///
/// 腦執行緒可能正好卡在一輪 CLI（最多 [`brain::SPAWN_TIMEOUT`]），看到停止
/// 旗標之後還會再跑一輪把開著的最後一段想完（再一個 SPAWN_TIMEOUT）。槽是
/// **並行的**，所以不是 concurrency × 120 秒。
pub fn shutdown_think_bound() -> Duration {
    brain::SPAWN_TIMEOUT + brain::SPAWN_TIMEOUT
}

pub fn shutdown_think_bound_secs() -> u64 {
    shutdown_think_bound().as_secs()
}

pub fn shutdown_think_bound_ms() -> i64 {
    shutdown_think_bound().as_millis() as i64
}

/// 錄製已停、開始等解釋層之前印的那一句。上限是「最多」，不是「一定」。
pub fn shutdown_wait_notice() -> String {
    format!(
        "錄製已停；解釋層還要把最後一段想完（最多 {} 秒）。",
        shutdown_think_bound_secs()
    )
}

/// 一場錄製裡腦自己做了什麼。給收工摘要用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub armed: bool,
    /// 因為找到值得理解的已關閉段落，真的叫了 [`brain::run`]。
    pub interpreter_wakes: u32,
    pub interpreter_cards: u32,
    pub interpreter_nothing: u32,
    pub interpreter_jobs: u32,
    /// 這一場實際跑到、之前已有保留外送列且這次仍沒寫成卡片的段落次數。
    pub interpreter_retried_without_card: u32,
    pub last_interpreter_skip: Option<String>,
    pub reviewer_interval_runs: u32,
    pub reviewer_eod_runs: u32,
    pub reviewer_nothing: u32,
    pub last_reviewer_skip: Option<String>,
    /// 執行緒自己的資料庫開不起來。錄製不受影響。
    pub open_failed: Option<String>,
    pub last_segment: LastSegment,
}

impl Report {
    pub fn unarmed() -> Self {
        Self {
            armed: false,
            interpreter_wakes: 0,
            interpreter_cards: 0,
            interpreter_nothing: 0,
            interpreter_jobs: 0,
            interpreter_retried_without_card: 0,
            last_interpreter_skip: None,
            reviewer_interval_runs: 0,
            reviewer_eod_runs: 0,
            reviewer_nothing: 0,
            last_reviewer_skip: None,
            open_failed: None,
            last_segment: LastSegment::Skipped,
        }
    }
}

/// 收工摘要。三種空各一句，不准長得一樣。
pub fn format_report(r: &Report) -> String {
    if let Some(e) = &r.open_failed {
        return format!("解釋層執行緒開不起來（錄製照跑，這一場一次都沒醒）：{e}");
    }
    let mut out = String::new();
    if !r.armed {
        out.push_str(
            "還沒設定 [brain] command。解釋層與審閱層這一場一次都不會醒。\n\
             （不是今天沒有東西可想——她根本沒有一支 CLI 可以叫。）",
        );
        return out;
    }
    if r.interpreter_wakes == 0 {
        out.push_str(
            "解釋層這一場一次都沒醒：沒有已關閉的段落帶著值得理解的訊號\
             （error code、大段貼上、長停留後恢復、工作集變更、卡住）。\n\
             （不是醒了卻沒東西可想——她根本沒被叫醒。）",
        );
    } else if r.interpreter_cards == 0 && r.interpreter_jobs == 0 {
        out.push_str(&format!(
            "解釋層醒過 {} 次，但沒有「值得理解」的已關閉段落可想。",
            r.interpreter_wakes
        ));
        if let Some(skip) = &r.last_interpreter_skip {
            out.push('\n');
            out.push_str(skip);
        }
    } else {
        out.push_str(&format!(
            "解釋層自己醒了 {} 次，跑了 {} 次，寫進 {} 張假設。",
            r.interpreter_wakes, r.interpreter_jobs, r.interpreter_cards
        ));
        if r.interpreter_retried_without_card > 0 {
            out.push_str(&format!(
                " 其中 {} 次是之前問過、這一次又沒寫成卡片。",
                r.interpreter_retried_without_card
            ));
        }
        if let Some(skip) = &r.last_interpreter_skip {
            out.push('\n');
            out.push_str(skip);
        }
    }
    out.push('\n');
    if r.reviewer_interval_runs == 0 && r.reviewer_eod_runs == 0 {
        out.push_str(
            "審閱層這一場一次都沒醒：活躍時還沒滿 15 分鐘，也還沒換日。\n\
             （不是醒了卻沒東西可審——她根本還沒到節奏。）",
        );
    } else if r.reviewer_nothing > 0
        && r.reviewer_interval_runs + r.reviewer_eod_runs == r.reviewer_nothing
    {
        out.push_str(&format!(
            "審閱層醒過 {} 輪（其中日終 {} 輪），但沒有還沒審過的 L2 假設。",
            r.reviewer_interval_runs + r.reviewer_eod_runs,
            r.reviewer_eod_runs
        ));
    } else {
        out.push_str(&format!(
            "審閱層自己醒了：活躍批次 {} 輪，日終 {} 輪。",
            r.reviewer_interval_runs, r.reviewer_eod_runs
        ));
        if let Some(skip) = &r.last_reviewer_skip {
            out.push('\n');
            out.push_str(skip);
        }
    }
    match r.last_segment {
        LastSegment::Skipped | LastSegment::Ran => {}
        LastSegment::NothingWorth => {
            out.push('\n');
            out.push_str("收工時看過還開著的最後一段，沒有值得理解的訊號可想。");
        }
        LastSegment::TimedOut => {
            out.push('\n');
            out.push_str(&format!(
                "收工時等到上限（{} 秒），還開著的最後一段沒想完。",
                shutdown_think_bound_secs()
            ));
        }
    }
    out
}

/// 換日了沒。純函式，午夜測試不必真的等到明天。
pub fn day_changed(previous: &str, now: Millis) -> bool {
    match brain::local_day_key(now) {
        Some(today) => today != previous,
        None => false,
    }
}

/// 日終這一輪該不該跑。
///
/// - `last_eod_day`：最近一次成功日終的 run stamp（哪一天跑的），
///   只用來擋同一天再跑一輪。
/// - `yesterday_summarized`：昨天有沒有被盤點過。不是昨天有沒有跑過一輪。
/// - `day_just_changed`：這一場錄製親眼看到 `local_day_key` 變了。
/// - 否則是補跑：昨天有 L0、而且昨天還沒被盤點過。
pub fn eod_due(
    today: &str,
    last_eod_day: Option<&str>,
    yesterday_summarized: bool,
    yesterday_has_l0: bool,
    day_just_changed: bool,
) -> bool {
    if last_eod_day == Some(today) {
        return false;
    }
    if day_just_changed {
        return true;
    }
    yesterday_has_l0 && !yesterday_summarized
}

fn ms_until_next_local_day(now: Millis) -> Option<Millis> {
    let today = brain::local_day_key(now)?;
    let (_, end) = local_day::local_day_bounds(&today)?;
    Some(end.saturating_sub(now).max(1))
}

/// 下一拍鐘該等多久。上限是 10 分鐘（§4.1 時間上限），下限 50ms。
/// 不是每秒去問資料庫。
pub fn next_wait_ms(
    now: Millis,
    last_review_at: Option<Millis>,
    last_look_at: Option<Millis>,
) -> u64 {
    let mut wait = TIME_CAP_MS;
    if let Some(until_midnight) = ms_until_next_local_day(now) {
        wait = wait.min(until_midnight);
    }
    let since_review = last_review_at.map(|t| now.saturating_sub(t));
    let review_left = match since_review {
        Some(ago) if ago < reviewer::MIN_INTERVAL_MS => reviewer::MIN_INTERVAL_MS - ago,
        Some(_) => 1,
        None => reviewer::MIN_INTERVAL_MS,
    };
    wait = wait.min(review_left);
    if let Some(look) = last_look_at {
        let since = now.saturating_sub(look);
        if since < TIME_CAP_MS {
            wait = wait.min(TIME_CAP_MS - since);
        }
    }
    wait.max(SLEEP_SLICE.as_millis() as i64).min(TIME_CAP_MS) as u64
}

struct Shared {
    activity: AtomicBool,
    stop: AtomicBool,
    brain: Mutex<BrainConfig>,
}

/// 錄製迴圈握著的那一頭。熱路徑只碰 [`Self::ping`]。
pub struct Handle {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<Report>>,
    data_dir: PathBuf,
}

impl Handle {
    /// 沒設定 `[brain] command` 就回 `None`：執行緒都不會起來。
    pub fn maybe_spawn(data_dir: &Path, brain: BrainConfig, now: Millis) -> Result<Option<Self>> {
        if !armed(&brain) {
            return Ok(None);
        }
        Ok(Some(Self::spawn(data_dir, brain, now)?))
    }

    fn spawn(data_dir: &Path, brain: BrainConfig, now: Millis) -> Result<Self> {
        let db_path = Config::db_path(data_dir);
        let data_dir = data_dir.to_path_buf();
        let shared = Arc::new(Shared {
            activity: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            brain: Mutex::new(brain),
        });
        let thread_shared = shared.clone();
        let worker_dir = data_dir.clone();
        let thread = std::thread::Builder::new()
            .name("sister-brain".into())
            .spawn(move || worker(db_path, worker_dir, thread_shared, now))
            .context("spawn sister-brain thread")?;
        Ok(Self {
            shared,
            thread: Some(thread),
            data_dir,
        })
    }

    /// 熱路徑唯一准做的事。一次 atomic store，永不阻塞、不查資料庫。
    pub fn ping(&self) {
        self.shared.activity.store(true, Ordering::Relaxed);
    }

    /// 設定檔熱重載。沒設定 CLI 之後就不再醒。
    pub fn set_config(&self, brain: BrainConfig) {
        if let Ok(mut g) = self.shared.brain.lock() {
            *g = brain;
        }
    }

    /// 錄製要停了。會把還開著的最後一段也想一遍，然後加入執行緒。
    ///
    /// 加入之前先把心跳改成「沒在錄、但還佔著」：迴圈已經跳出，再蓋
    /// Recording 是謊；蓋墓碑則是另一個謊（行程還握著資料庫）。見
    /// [`heartbeat::beat_thinking`]。
    pub fn shutdown(mut self) -> Report {
        self.mark_thinking();
        self.shared.stop.store(true, Ordering::SeqCst);
        match self.thread.take() {
            Some(t) => t.join().unwrap_or_else(|_| Report {
                open_failed: Some("解釋層執行緒炸了".into()),
                ..Report::unarmed()
            }),
            None => Report::unarmed(),
        }
    }

    fn mark_thinking(&self) {
        let now = crate::now_ms();
        let until = now.saturating_add(shutdown_think_bound_ms());
        let _ = heartbeat::beat_thinking(&self.data_dir, now, until);
    }
}

impl Drop for Handle {
    /// 正常收工會先 `wake.take()` 再走 [`Self::shutdown`]；`Handle` 出生到那一行
    /// 之間沒有會提早回傳的 `?`／`return Err`／`bail!`。因此還握著 thread 的
    /// `Drop` 是 panic unwind 的保險路徑。
    ///
    /// join 期間仍要說 Thinking，避免另一個 recorder 進來；join 回來後這個
    /// 行程已經沒有人在想，必須立刻蓋墓碑，不能把兩分鐘上限留在磁碟上。
    ///
    /// **但只有這條路蓋。** [`Self::shutdown`] 收 `self`，所以正路上這支
    /// `drop` 也會跑一次；在這裡蓋墓碑等於把 `record` 收工那一段的順序倒過來，
    /// 而那一段自己的註解寫著「順序不能倒——倒了的話墓碑會在 CLI 還跑著的
    /// 時候放行第二個 recorder」。
    fn drop(&mut self) {
        // `shutdown()` 已經把執行緒收走 ⇒ 正路 ⇒ 墓碑歸呼叫端，這裡什麼都不做。
        let Some(t) = self.thread.take() else {
            return;
        };
        self.mark_thinking();
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = t.join();
        // 走到這裡代表沒有人呼叫 `shutdown()`，也就是 panic unwind。後面不會
        // 有人蓋墓碑，而心跳上還寫著「想最後一段，最多兩分鐘」——留著的話，
        // 一個已經不存在的行程會讓 `is_occupied` 假 true 240 秒（這一版之前
        // 是過期心跳的 16 秒，那是變壞不是變好）。
        heartbeat::stop(&self.data_dir, crate::now_ms());
    }
}

fn worker(db_path: PathBuf, data_dir: PathBuf, shared: Arc<Shared>, started: Millis) -> Report {
    let db = match Db::open(&db_path) {
        Ok(d) => d,
        Err(e) => {
            return Report {
                open_failed: Some(format!("{e:#}")),
                armed: true,
                ..Report::unarmed()
            };
        }
    };
    let brain = shared
        .brain
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|e| e.into_inner().clone());
    let mut engine = match Engine::new(db, data_dir, brain, started) {
        Ok(e) => e,
        Err(e) => {
            return Report {
                open_failed: Some(format!("{e:#}")),
                armed: true,
                ..Report::unarmed()
            };
        }
    };
    if let Err(e) = engine.catch_up(started) {
        engine.report.last_interpreter_skip = Some(format!("{e:#}"));
    }
    let mut next_clock = Instant::now() + Duration::from_millis(next_wait_ms(started, None, None));
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            let now = crate::now_ms();
            engine.refresh_brain(&shared);
            let _ = engine.step(now, Step::Shutdown);
            break;
        }
        if shared.activity.swap(false, Ordering::Relaxed) {
            let now = crate::now_ms();
            engine.refresh_brain(&shared);
            let _ = engine.step(now, Step::Activity);
            next_clock = Instant::now() + Duration::from_millis(engine.next_wait(crate::now_ms()));
            continue;
        }
        if Instant::now() >= next_clock {
            let now = crate::now_ms();
            engine.refresh_brain(&shared);
            let _ = engine.step(now, Step::Clock);
            next_clock = Instant::now() + Duration::from_millis(engine.next_wait(now));
            continue;
        }
        std::thread::sleep(SLEEP_SLICE);
    }
    engine.report
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Activity,
    Clock,
    Shutdown,
}

struct Engine {
    db: Db,
    data_dir: PathBuf,
    brain: BrainConfig,
    session_started_at: Millis,
    last_day: String,
    last_review_at: Option<Millis>,
    last_look_at: Option<Millis>,
    recorded_no_consent: bool,
    budget_exhausted: bool,
    report: Report,
}

impl Engine {
    fn new(db: Db, data_dir: PathBuf, brain: BrainConfig, now: Millis) -> Result<Self> {
        let last_day = brain::local_day_key(now).unwrap_or_else(|| "unknown".into());
        Ok(Self {
            db,
            data_dir,
            report: Report {
                armed: armed(&brain),
                ..Report::unarmed()
            },
            brain,
            session_started_at: now,
            last_day,
            last_review_at: None,
            last_look_at: None,
            recorded_no_consent: false,
            budget_exhausted: false,
        })
    }

    fn refresh_brain(&mut self, shared: &Shared) {
        if let Ok(g) = shared.brain.lock() {
            self.brain = g.clone();
        }
        self.report.armed = armed(&self.brain);
    }

    fn next_wait(&self, now: Millis) -> u64 {
        if !armed(&self.brain) {
            return TIME_CAP_MS as u64;
        }
        next_wait_ms(now, self.last_review_at, self.last_look_at)
    }

    fn catch_up(&mut self, now: Millis) -> Result<()> {
        if !armed(&self.brain) {
            return Ok(());
        }
        self.maybe_eod(now, false)?;
        // 上一場收工時沒想完的最後一段：session 已結束，那些章節已關閉。
        // LOOKAROUND_MS 之內再開始錄，這裡會補上；更久以前的那一筆 Timeout
        // 還留在外送紀錄裡，只是這一次看不到。
        self.maybe_interpret(now, false)
    }

    fn step(&mut self, now: Millis, kind: Step) -> Result<()> {
        if !armed(&self.brain) {
            self.report.armed = false;
            return Ok(());
        }
        self.report.armed = true;
        let today = brain::local_day_key(now).unwrap_or_else(|| self.last_day.clone());
        let changed = today != self.last_day;
        if changed {
            self.maybe_eod(now, true)?;
            self.last_day = today;
            self.budget_exhausted = false;
        }
        match kind {
            Step::Activity | Step::Clock => {
                self.maybe_interpret(now, false)?;
                self.maybe_interval(now)?;
            }
            Step::Shutdown => {
                self.maybe_interpret(now, true)?;
            }
        }
        Ok(())
    }

    fn maybe_eod(&mut self, now: Millis, day_just_changed: bool) -> Result<()> {
        let Some(today) = brain::local_day_key(now) else {
            return Ok(());
        };
        let yesterday = previous_local_day_key(now);
        let last_eod = self.db.last_reviewer_eod_day().ok().flatten();
        let yesterday_has_l0 = yesterday
            .as_deref()
            .and_then(local_day::local_day_bounds)
            .map(|(from, to)| self.db.has_l0_in_range(from, to).unwrap_or(false))
            .unwrap_or(false);
        let yesterday_summarized = yesterday
            .as_deref()
            .map(|d| self.db.has_reviewer_eod_for_day(d).unwrap_or(false))
            .unwrap_or(false);
        if !eod_due(
            &today,
            last_eod.as_deref(),
            yesterday_summarized,
            yesterday_has_l0,
            day_just_changed,
        ) {
            return Ok(());
        }
        self.run_review(now, ReviewKind::Eod)
    }

    fn maybe_interval(&mut self, now: Millis) -> Result<()> {
        if let Some(last) = self.last_review_at
            && now.saturating_sub(last) < reviewer::MIN_INTERVAL_MS
        {
            return Ok(());
        }
        if self.last_review_at.is_none()
            && now.saturating_sub(self.session_started_at) < reviewer::MIN_INTERVAL_MS
        {
            return Ok(());
        }
        self.run_review(now, ReviewKind::Interval)
    }

    fn maybe_interpret(&mut self, now: Millis, include_open: bool) -> Result<()> {
        self.last_look_at = Some(now);
        let consent = crate::consent::load(&self.data_dir);
        if consent.cloud_permit().is_none() {
            if !self.recorded_no_consent {
                self.db.insert_brain_skip(
                    now,
                    BrainSkip::NoConsent.as_str(),
                    None,
                    &BrainSkip::NoConsent.message(),
                )?;
                self.recorded_no_consent = true;
                self.report.last_interpreter_skip = Some(BrainSkip::NoConsent.message());
            }
            return Ok(());
        }
        if self.budget_exhausted {
            return Ok(());
        }

        let from = self.session_started_at.saturating_sub(LOOKAROUND_MS);
        let segs = self.db.chapters_for_range(from, now)?;
        let to = if include_open {
            now
        } else {
            match segs.last() {
                Some(last) => last.core_started_at,
                None => return Ok(()),
            }
        };
        if to <= from {
            if include_open {
                self.report.last_segment = LastSegment::NothingWorth;
            }
            return Ok(());
        }

        let mut input = InterpretInput {
            db: &mut self.db,
            consent: &consent,
            brain: &self.brain,
            from_ts: from,
            to_ts: to,
            limit: self.brain.concurrency_slots() as usize,
            only_core_start: None,
        };
        let dry = brain::prepare(&mut input)?;
        match &dry.skip {
            Some(BrainSkip::NoCommand) => {
                self.report.armed = false;
                Ok(())
            }
            Some(BrainSkip::NoConsent) => Ok(()),
            Some(BrainSkip::BudgetExhausted { .. }) => {
                let msg = dry.skip.as_ref().map(|s| s.message());
                let result = brain::run(&mut input)?;
                self.budget_exhausted = true;
                self.report.last_interpreter_skip = result.skip.map(|s| s.message()).or(msg);
                Ok(())
            }
            Some(BrainSkip::NothingWorthInterpreting { remaining }) => {
                if include_open {
                    let skip = BrainSkip::NothingWorthInterpreting {
                        remaining: *remaining,
                    };
                    self.db
                        .insert_brain_skip(now, skip.as_str(), None, &skip.message())?;
                    self.report.last_segment = LastSegment::NothingWorth;
                }
                Ok(())
            }
            None => {
                if dry.jobs.is_empty() {
                    if include_open {
                        self.report.last_segment = LastSegment::NothingWorth;
                    }
                    return Ok(());
                }
                self.report.interpreter_wakes += 1;
                let result = brain::run(&mut input)?;
                self.report.interpreter_jobs += result.ran.len() as u32;
                self.report.interpreter_cards +=
                    result.ran.iter().filter(|j| j.card.is_some()).count() as u32;
                self.report.interpreter_retried_without_card += result
                    .ran
                    .iter()
                    .filter(|j| j.previous.is_some() && !j.outcome.wrote_card())
                    .count() as u32;
                if include_open {
                    self.report.last_segment = if result
                        .ran
                        .iter()
                        .any(|j| j.outcome == OutboundOutcome::Timeout)
                    {
                        LastSegment::TimedOut
                    } else {
                        LastSegment::Ran
                    };
                }
                match result.skip {
                    Some(BrainSkip::NothingWorthInterpreting { .. }) => {
                        self.report.interpreter_nothing += 1;
                        self.report.last_interpreter_skip = Some(
                            BrainSkip::NothingWorthInterpreting {
                                remaining: dry.budget_limit.saturating_sub(dry.budget_used),
                            }
                            .message(),
                        );
                    }
                    Some(BrainSkip::BudgetExhausted { .. }) => {
                        self.budget_exhausted = true;
                        self.report.last_interpreter_skip = result.skip.map(|s| s.message());
                    }
                    Some(s) => self.report.last_interpreter_skip = Some(s.message()),
                    None => {}
                }
                Ok(())
            }
        }
    }

    fn run_review(&mut self, now: Millis, kind: ReviewKind) -> Result<()> {
        let consent = crate::consent::load(&self.data_dir);
        let from = match kind {
            ReviewKind::Eod => now.saturating_sub(36 * 3_600_000),
            ReviewKind::Interval => self
                .last_review_at
                .unwrap_or(self.session_started_at)
                .saturating_sub(LOOKAROUND_MS),
        };
        let result = {
            let mut input = ReviewInput {
                db: &mut self.db,
                consent: &consent,
                brain: &self.brain,
                from_ts: from,
                to_ts: now,
                kind,
                force: false,
                now,
            };
            reviewer::run(&mut input)?
        };
        if let Some(skip) = &result.skip {
            self.report.last_reviewer_skip = Some(skip.message());
            if matches!(skip, ReviewSkip::Cadence { .. }) {
                self.last_review_at = self.db.last_reviewer_run_at().ok().flatten().or(Some(now));
                return Ok(());
            }
            self.last_review_at = Some(now);
            if matches!(skip, ReviewSkip::NoCommand) {
                self.report.armed = false;
                return Ok(());
            }
            if matches!(skip, ReviewSkip::NothingToReview { .. }) && result.ran {
                self.count_review(kind, true);
            }
            return Ok(());
        }
        self.last_review_at = Some(now);
        self.count_review(kind, false);
        Ok(())
    }

    fn count_review(&mut self, kind: ReviewKind, nothing: bool) {
        match kind {
            ReviewKind::Interval => self.report.reviewer_interval_runs += 1,
            ReviewKind::Eod => self.report.reviewer_eod_runs += 1,
        }
        if nothing {
            self.report.reviewer_nothing += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::{Consent, Sheet};
    use crate::db::{DaySummaryGlance, Db, L2Author, L2Insert};
    use crate::heartbeat;
    use crate::model::{FocusEvent, FocusKind, FocusSnapshot, FrameCapture, OcrBlock};
    use chrono::{Local, LocalResult, TimeZone};

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-wakeup-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("tmpdir");
            Self(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn seed_worth(db: &mut Db, ts: Millis) -> i64 {
        let sid = db.start_session("test", "0").expect("session");
        db.insert_focus(
            sid,
            &FocusEvent {
                ts,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("code.exe".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus a");
        db.insert_focus(
            sid,
            &FocusEvent {
                ts: ts + 180_000,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus b");
        // 切刀要嚴格早於 stream_end，否則最後一次換 app 不會關上前一段。
        db.insert_focus(
            sid,
            &FocusEvent {
                ts: ts + 200_000,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    window_title: Some("still chrome".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus c");
        let frame = FrameCapture {
            ts: ts + 30_000,
            monitor: 0,
            width: 100,
            height: 100,
            dhash: 1,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: "error[E0308]: mismatched types".into(),
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                confidence: 1.0,
            }],
            focus: FocusSnapshot {
                app_id: Some("code.exe".into()),
                ..Default::default()
            },
        };
        let (fid, _, _) = db.insert_frame(sid, &frame, None, 0).expect("frame");
        let second = FrameCapture {
            ts: ts + 190_000,
            dhash: 2,
            ocr: vec![OcrBlock {
                text: "error[E0425]: cannot find value".into(),
                ..frame.ocr[0].clone()
            }],
            focus: FocusSnapshot {
                app_id: Some("chrome.exe".into()),
                ..Default::default()
            },
            ..frame
        };
        db.insert_frame(sid, &second, None, 0)
            .expect("second worthy frame");
        fid
    }

    fn fake_cli(dir: &Path, json: &str, sentinel: &Path, sleep_secs: u64) -> (String, Vec<String>) {
        let script = dir.join("fake-brain.py");
        std::fs::write(
            &script,
            format!(
                "import sys, time, pathlib\n\
                 sys.stdin.buffer.read()\n\
                 pathlib.Path(sys.argv[1]).write_text('started')\n\
                 time.sleep({sleep_secs})\n\
                 sys.stdout.buffer.write({json:?}.encode('utf-8'))\n"
            ),
        )
        .expect("script");
        (
            "python3".into(),
            vec![
                script.to_string_lossy().into_owned(),
                sentinel.to_string_lossy().into_owned(),
            ],
        )
    }

    fn grant_cloud(dir: &Path) {
        let mut c = Consent::default();
        c.grant(Sheet::LocalRecording, 1);
        c.grant(Sheet::CloudReading, 1);
        crate::consent::save(dir, &c).expect("consent");
    }

    fn local_ms(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> Option<Millis> {
        match Local.with_ymd_and_hms(year, month, day, hour, min, sec) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => Some(dt.timestamp_millis()),
            LocalResult::None => None,
        }
    }

    /// 那一天有 L0（focus）和一張 L2，日終才寫得出 Live 摘要。
    fn seed_day_with_card(db: &mut Db, ts: Millis, activity: &str) {
        let sid = db.start_session("test", "0").expect("session");
        db.insert_focus(
            sid,
            &FocusEvent {
                ts,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("code.exe".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus");
        db.insert_l2_card(&L2Insert {
            segment_core_start: ts,
            segment_ref: &format!("segment:{ts}"),
            activity,
            entities_json: "[]".into(),
            continues_json: None,
            commitments_json: "[]".into(),
            model_confidence: 0.6,
            evidence_json: "[]".into(),
            open_questions_json: "[]".into(),
            author: L2Author::Interpreter,
        })
        .expect("l2");
    }

    fn dummy_reviewer_brain() -> BrainConfig {
        BrainConfig {
            command: "python3".into(),
            args: vec!["-c".into(), "import sys; sys.stdin.buffer.read()".into()],
            ..Default::default()
        }
    }

    fn assert_live_summary(db: &Db, date: &str, activity: &str) {
        match db.day_summary_glance(date).unwrap() {
            DaySummaryGlance::Live {
                date: d, clauses, ..
            } => {
                assert_eq!(d, date);
                let texts: Vec<&str> = clauses.iter().map(|c| c.text.as_str()).collect();
                assert_eq!(texts, [activity], "{date} 的摘要對不上");
            }
            other => panic!("{date} 該是 Live，實際是 {other:?}"),
        }
    }

    #[test]
    fn silence_sentences_are_not_the_same() {
        let unarmed = format_report(&Report::unarmed());
        let never = format_report(&Report {
            armed: true,
            ..Report::unarmed()
        });
        let woke_nothing = format_report(&Report {
            armed: true,
            interpreter_wakes: 2,
            interpreter_nothing: 2,
            ..Report::unarmed()
        });
        let last_nothing = format_report(&Report {
            armed: true,
            last_segment: LastSegment::NothingWorth,
            ..Report::unarmed()
        });
        let last_timeout = format_report(&Report {
            armed: true,
            last_segment: LastSegment::TimedOut,
            ..Report::unarmed()
        });
        let reviewer_never = never.clone();
        assert_ne!(unarmed, never, "沒 CLI 和沒醒過印成同一句");
        assert_ne!(never, woke_nothing, "沒醒過和醒了沒東西印成同一句");
        assert_ne!(
            never, last_nothing,
            "這一場一次都沒醒，和收工時最後一段沒東西可想，印成同一句"
        );
        assert_ne!(
            last_nothing, last_timeout,
            "最後一段沒東西可想，和等到上限沒想完，印成同一句"
        );
        assert_ne!(
            never, last_timeout,
            "一次都沒醒，和等到上限沒想完，印成同一句"
        );
        assert!(unarmed.contains("[brain] command"), "{unarmed}");
        assert!(never.contains("一次都沒醒"), "{never}");
        assert!(never.contains("根本沒被叫醒"), "{never}");
        assert!(woke_nothing.contains("醒過"), "{woke_nothing}");
        assert!(woke_nothing.contains("沒有「值得理解」"), "{woke_nothing}");
        assert!(last_nothing.contains("還開著的最後一段"), "{last_nothing}");
        assert!(
            last_nothing.contains("沒有值得理解的訊號可想"),
            "{last_nothing}"
        );
        assert!(!last_nothing.contains("沒想完"), "{last_nothing}");
        assert!(last_timeout.contains("等到上限"), "{last_timeout}");
        assert!(last_timeout.contains("沒想完"), "{last_timeout}");
        assert!(
            !last_timeout.contains("沒有值得理解的訊號可想"),
            "{last_timeout}"
        );
        assert!(reviewer_never.contains("審閱層這一場一次都沒醒"), "{never}");
        assert!(
            !unarmed.contains("沒有已關閉的段落"),
            "沒 CLI 不該講成沒段落：{unarmed}"
        );
        let wait = shutdown_wait_notice();
        assert!(wait.contains("錄製已停"), "{wait}");
        assert!(wait.contains("最後一段"), "{wait}");
        assert!(
            wait.contains(&shutdown_think_bound_secs().to_string()),
            "{wait}"
        );
        assert_ne!(wait, last_timeout, "開始等之前那句和等到上限那句印成同一句");
        assert!(!wait.contains("一次都沒醒"), "{wait}");
    }

    #[test]
    fn report_adds_retry_count_only_when_nonzero() {
        let without = format_report(&Report {
            armed: true,
            interpreter_wakes: 3,
            interpreter_jobs: 12,
            ..Report::unarmed()
        });
        assert!(
            without.contains("解釋層自己醒了 3 次，跑了 12 次，寫進 0 張假設。"),
            "{without}"
        );
        assert!(!without.contains("之前問過"), "{without}");
        assert_eq!(
            without.lines().next(),
            Some("解釋層自己醒了 3 次，跑了 12 次，寫進 0 張假設。"),
            "零次時既有句子必須逐字不變"
        );

        let with = format_report(&Report {
            armed: true,
            interpreter_wakes: 3,
            interpreter_jobs: 12,
            interpreter_retried_without_card: 8,
            ..Report::unarmed()
        });
        assert!(
            with.contains("其中 8 次是之前問過、這一次又沒寫成卡片。"),
            "{with}"
        );
        println!("沒有重問：\n{without}\n---\n有重問：\n{with}");
    }

    #[test]
    fn unarmed_does_not_spawn_a_thread() {
        let tmp = Tmp::new("unarmed");
        let h = Handle::maybe_spawn(&tmp.0, BrainConfig::default(), 1_700_000_000_000)
            .expect("unarmed is Ok(None)");
        assert!(h.is_none(), "沒設定 CLI 卻開了執行緒");
        let text = format_report(&Report::unarmed());
        assert!(text.contains("一次都不會醒"), "{text}");
    }

    #[test]
    fn eod_due_only_when_the_day_ended() {
        // `yesterday_summarized` 刻意餵 false：不變式成立的時候這一組不會出現
        // （今天 stamp 的那一輪盤點的就是昨天），所以餵 true 的話最後一行自己
        //  就是 false，這句話會在 guard 完全沒出力的情況下通過——實測拿掉
        // `last_eod_day == Some(today)` 那道 guard，十六條 wakeup 測試全綠。
        // 這道 guard 守的是「不變式破了也不准同一天燒兩次大腦預算」。
        assert!(
            !eod_due("2026-08-26", Some("2026-08-26"), false, true, false),
            "今天已經日終過了還要跑"
        );
        assert!(
            eod_due("2026-08-26", None, false, true, false),
            "昨天有資料、從來沒日終，要補"
        );
        assert!(
            !eod_due("2026-08-26", None, false, false, false),
            "昨天沒有 L0 卻補跑"
        );
        assert!(
            eod_due("2026-08-26", None, false, false, true),
            "親眼看到換日卻不跑"
        );
        assert!(
            !eod_due("2026-08-26", Some("2026-08-25"), true, true, false),
            "昨天已經日終過了還補"
        );
        assert!(
            eod_due("2026-08-26", Some("2026-08-25"), false, true, false),
            "昨天有 L0、上一輪 stamp 在昨天（盤點的是前天），昨天自己還沒被盤點"
        );
    }

    #[test]
    fn local_day_key_changes_across_midnight() {
        let after = match Local.with_ymd_and_hms(2026, 8, 26, 0, 0, 1) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.timestamp_millis(),
            LocalResult::None => return,
        };
        let before = after - 2_000;
        let a = brain::local_day_key(before).expect("before");
        let b = brain::local_day_key(after).expect("after");
        assert_ne!(a, b, "跨過本地午夜，日期不該一樣：{a} / {b}");
        assert!(day_changed(&a, after));
        assert!(!day_changed(&b, after));
    }

    #[test]
    fn next_wait_is_not_a_one_second_poll() {
        let now = 1_700_000_000_000;
        let wait = next_wait_ms(now, None, None);
        assert!(wait >= 60_000, "沒有任何到期事件時不該每秒醒：{wait} ms");
        assert!(wait <= TIME_CAP_MS as u64, "不該比時間上限還長：{wait}");
        let soon = next_wait_ms(now, Some(now - 14 * 60_000), Some(now));
        assert!(
            soon <= 60_000 + 1_000,
            "審閱還差一分鐘就該在那附近醒：{soon}"
        );
    }

    #[test]
    fn activity_without_closed_worth_is_never_woke_not_woke_nothing() {
        let tmp = Tmp::new("look-no-wake");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_300_000;
        let sid = db.start_session("test", "0").expect("session");
        db.insert_focus(
            sid,
            &FocusEvent {
                ts,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("code.exe".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus");
        let brain = BrainConfig {
            command: "python3".into(),
            args: vec!["-c".into(), "raise SystemExit(1)".into()],
            ..Default::default()
        };
        let mut engine = Engine::new(db, tmp.0.clone(), brain, ts).expect("engine");
        engine.step(ts + 5_000, Step::Activity).expect("step");
        assert_eq!(engine.report.interpreter_wakes, 0, "{:?}", engine.report);
        let text = format_report(&engine.report);
        assert!(text.contains("一次都沒醒"), "{text}");
        assert!(!text.contains("醒過"), "{text}");
    }

    #[test]
    fn closed_worth_segment_wakes_interpreter_through_fake_cli() {
        let tmp = Tmp::new("wake-ok");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_400_000;
        let fid = seed_worth(&mut db, ts);
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        assert!(segs.len() >= 2, "要有已關閉的前一段才測得到喚醒：{segs:?}");
        let core = segs[0].core_started_at;
        let json = format!(
            r#"{{"segment_ref":"segment:{core}","activity":"在修 compiler error","entities":[],"confidence":0.55,"evidence_refs":["frame:{fid}"],"open_questions":[]}}"#
        );
        let sentinel = tmp.0.join("started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 0);
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let now = ts + 200_000;
        let mut engine = Engine::new(db, tmp.0.clone(), brain, ts).expect("engine");
        engine.step(now, Step::Activity).expect("step");
        assert!(
            engine.report.interpreter_wakes >= 1,
            "值得理解的已關閉段落卻沒醒：{:?}",
            engine.report
        );
        assert!(
            engine.report.interpreter_cards >= 1,
            "醒了卻沒寫卡片：{:?}",
            engine.report
        );
        assert!(sentinel.exists(), "該 spawn 假 CLI");
        let text = format_report(&engine.report);
        assert!(text.contains("自己醒了"), "{text}");
        assert!(!text.contains("解釋層這一場一次都沒醒"), "{text}");
    }

    #[test]
    fn wakeup_does_not_count_a_first_attempt_that_fails() {
        let tmp = Tmp::new("wake-retry-fail");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_450_000;
        let fid = seed_worth(&mut db, ts);
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        assert!(segs.len() >= 2, "要有已關閉的前一段：{segs:?}");
        let first_attempt_core = segs[1].core_started_at;
        let day = brain::local_day_key(ts).expect("day");
        db.insert_brain_outbound(&crate::db::OutboundInsert {
            ts: ts - 1,
            day_key: &day,
            command: "old-agent",
            args: &[],
            segment_core_start: Some(segs[0].core_started_at),
            chars_sent: 1,
            truncated: false,
            outcome: "timeout",
            duration_ms: 1,
            error: None,
            role: "interpreter",
        })
        .expect("prior attempt only for the job that succeeds this time");
        let json = format!(
            r#"{{"segment_ref":"segment:{}","activity":"修好第一段","entities":[],"confidence":0.7,"evidence_refs":["frame:{}"],"open_questions":[]}}"#,
            segs[0].core_started_at, fid
        );
        let sentinel = tmp.0.join("mixed-started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 0);
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut engine = Engine::new(db, tmp.0.clone(), brain, ts).expect("engine");
        engine
            .step(ts + 201_000, Step::Shutdown)
            .expect("mixed retry step");
        assert_eq!(engine.report.interpreter_jobs, 2, "測試必須真的跑兩種 job");
        assert_eq!(engine.report.interpreter_cards, 1, "其中一個重問這次要成功");
        assert_eq!(
            engine.report.interpreter_retried_without_card, 0,
            "segment:{first_attempt_core} 這次失敗，但它沒有前置外送列，不可算重問：{:?}",
            engine.report
        );
    }

    #[test]
    fn wakeup_counts_only_the_retried_job_that_fails_again() {
        let tmp = Tmp::new("wake-retry-fail");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_450_000;
        let fid = seed_worth(&mut db, ts);
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        assert!(segs.len() >= 2, "要有已關閉的前一段：{segs:?}");
        let failed_core = segs[1].core_started_at;
        let day = brain::local_day_key(ts).expect("day");
        for core in [segs[0].core_started_at, failed_core] {
            db.insert_brain_outbound(&crate::db::OutboundInsert {
                ts: ts - 1,
                day_key: &day,
                command: "old-agent",
                args: &[],
                segment_core_start: Some(core),
                chars_sent: 1,
                truncated: false,
                outcome: "timeout",
                duration_ms: 1,
                error: None,
                role: "interpreter",
            })
            .expect("prior attempt");
        }
        let json = format!(
            r#"{{"segment_ref":"segment:{}","activity":"修好第一段","entities":[],"confidence":0.7,"evidence_refs":["frame:{}"],"open_questions":[]}}"#,
            segs[0].core_started_at, fid
        );
        let sentinel = tmp.0.join("mixed-started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 0);
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut engine = Engine::new(db, tmp.0.clone(), brain, ts).expect("engine");
        engine
            .step(ts + 201_000, Step::Shutdown)
            .expect("mixed retry step");
        assert_eq!(engine.report.interpreter_jobs, 2, "測試必須真的跑兩種 job");
        assert_eq!(engine.report.interpreter_cards, 1, "其中一個重問這次要成功");
        assert_eq!(
            engine.report.interpreter_retried_without_card, 1,
            "只數這一場跑到、先前問過、這次仍沒卡片的那一段：{:?}",
            engine.report
        );
    }

    #[test]
    fn a_stuck_cli_does_not_stall_the_record_loop() {
        let tmp = Tmp::new("stuck-cli");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_500_000;
        let fid = seed_worth(&mut db, ts);
        drop(db);
        let segs = {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("reopen");
            db.chapters_for_range(ts, ts + 400_000).expect("segs")
        };
        let core = segs[0].core_started_at;
        let json = format!(
            r#"{{"segment_ref":"segment:{core}","activity":"x","entities":[],"confidence":0.5,"evidence_refs":["frame:{fid}"],"open_questions":[]}}"#
        );
        let sentinel = tmp.0.join("started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 2);
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let handle = Handle::maybe_spawn(&tmp.0, brain, ts)
            .expect("spawn")
            .expect("armed");
        handle.ping();
        let gave_up = Instant::now() + Duration::from_secs(5);
        while !sentinel.exists() {
            assert!(
                Instant::now() < gave_up,
                "假 CLI 五秒內沒進 sleep，測不到卡住"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let start = Instant::now();
        let mut ticks = 0u32;
        while start.elapsed() < Duration::from_millis(400) {
            ticks += 1;
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            ticks >= 20,
            "CLI 睡 2 秒的期間錄製迴圈只走了 {ticks} 拍——被堵住了"
        );
        assert!(
            start.elapsed() < Duration::from_millis(800),
            "400ms 的拍子花了 {:?}，CLI 把熱路徑拖住了",
            start.elapsed()
        );
        let report = handle.shutdown();
        assert!(
            report.interpreter_wakes >= 1 || report.open_failed.is_some(),
            "慢路徑該把那一段想完：{report:?}"
        );
    }

    /// 慢路徑在想事情的時候，錄製那一邊**寫得進資料庫**。
    ///
    /// 上面那條測的是執行緒各走各的，而它的「錄製迴圈」只是 `sleep(10ms)`
    /// ——不碰資料庫，所以它證明的東西在 Rust 裡本來就成立。真正會出事的
    /// 是**第二個寫入者**：這顆資料庫跑 WAL，同時只准一個 writer，而
    /// `busy_timeout` 是 5 秒。慢路徑要是把寫鎖抓著不放，錄製那一邊每一拍
    /// 都會卡在 `insert_frame` 上——畫面就掉了，而且畫面上不會有人承認。
    ///
    /// 所以這裡讓錄製那一邊真的一直插畫面，一邊讓假 CLI 睡著。
    #[test]
    fn the_recorder_can_still_write_while_the_slow_path_thinks() {
        let tmp = Tmp::new("write-contention");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_500_000;
        let fid = seed_worth(&mut db, ts);
        drop(db);
        let segs = {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("reopen");
            db.chapters_for_range(ts, ts + 400_000).expect("segs")
        };
        let core = segs[0].core_started_at;
        let json = format!(
            r#"{{"segment_ref":"segment:{core}","activity":"x","entities":[],"confidence":0.5,"evidence_refs":["frame:{fid}"],"open_questions":[]}}"#
        );
        let sentinel = tmp.0.join("started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 2);
        let handle = Handle::maybe_spawn(
            &tmp.0,
            BrainConfig {
                command,
                args,
                ..Default::default()
            },
            ts,
        )
        .expect("spawn")
        .expect("armed");
        handle.ping();
        let gave_up = Instant::now() + Duration::from_secs(5);
        while !sentinel.exists() {
            assert!(Instant::now() < gave_up, "假 CLI 沒進 sleep，測不到卡住");
            std::thread::sleep(Duration::from_millis(20));
        }

        // 錄製那一邊：自己的連線，一直寫。
        let mut rec = Db::open(&Config::db_path(&tmp.0)).expect("recorder conn");
        let sid = rec.start_session("recorder", "0").expect("session");
        let mut worst = Duration::ZERO;
        for i in 0..40 {
            let at = Instant::now();
            rec.insert_focus(
                sid,
                &FocusEvent {
                    ts: ts + 500_000 + i * 1_000,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot {
                        app_id: Some("code.exe".into()),
                        ..Default::default()
                    },
                },
            )
            .expect("錄製那一邊寫不進去了");
            worst = worst.max(at.elapsed());
        }
        assert!(
            worst < Duration::from_millis(500),
            "最慢的一次寫入花了 {worst:?}——慢路徑把寫鎖抓著，錄製會掉幀"
        );
        handle.shutdown();
    }

    #[test]
    fn observing_midnight_runs_eod() {
        let tmp = Tmp::new("eod");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let after = match Local.with_ymd_and_hms(2026, 8, 26, 0, 0, 2) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.timestamp_millis(),
            LocalResult::None => return,
        };
        let before = after - 5_000;
        let sid = db.start_session("test", "0").expect("session");
        db.insert_focus(
            sid,
            &FocusEvent {
                ts: before - 60_000,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("code.exe".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus");
        let brain = BrainConfig {
            command: "python3".into(),
            args: vec!["-c".into(), "import sys; sys.stdin.buffer.read()".into()],
            ..Default::default()
        };
        let mut engine = Engine::new(db, tmp.0.clone(), brain, before).expect("engine");
        engine.step(after, Step::Clock).expect("clock");
        assert!(
            engine.report.reviewer_eod_runs >= 1,
            "親眼看到換日，日終該跑：{:?}",
            engine.report
        );
        let text = format_report(&engine.report);
        assert!(
            text.contains("日終") || text.contains("審閱層自己醒了") || text.contains("醒過"),
            "{text}"
        );
    }

    #[test]
    fn interpreter_budget_exhaustion_clears_after_midnight() {
        let tmp = Tmp::new("budget-midnight");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let after = match Local.with_ymd_and_hms(2026, 8, 26, 0, 0, 2) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.timestamp_millis(),
            LocalResult::None => return,
        };
        let before = after - 240_000;
        let fid = seed_worth(&mut db, before);
        let core = db.chapters_for_range(before, after).expect("segments")[0].core_started_at;
        let json = format!(
            r#"{{"segment_ref":"segment:{core}","activity":"跨日後再問","entities":[],"confidence":0.7,"evidence_refs":["frame:{fid}"],"open_questions":[]}}"#
        );
        let sentinel = tmp.0.join("after-midnight-started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 0);
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut engine = Engine::new(db, tmp.0.clone(), brain, before).expect("engine");
        engine.budget_exhausted = true;
        engine
            .step(before + 1_000, Step::Shutdown)
            .expect("same-day step");
        assert_eq!(
            engine.report.interpreter_jobs, 0,
            "額度用完後同一天不該再問"
        );
        assert!(!sentinel.exists(), "額度用完後同一天不該叫 CLI");

        engine.step(after, Step::Shutdown).expect("next-day step");
        assert!(
            engine.report.interpreter_jobs > 0,
            "跨過午夜後應清掉這一場的額度快取並再問：{:?}",
            engine.report
        );
        assert!(sentinel.exists(), "跨過午夜後應再次叫 CLI");
    }

    /// 按下停止之後，畫面說的和行程真實狀態一致：不在錄，但還佔著。
    ///
    /// 假 CLI 睡 2 秒，所以收工那一窗看得到、不必等 120 秒。
    #[test]
    fn after_stop_the_beat_says_not_recording_while_the_process_still_thinks() {
        let tmp = Tmp::new("stop-window");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_800_000;
        let fid = seed_worth(&mut db, ts);
        drop(db);
        let segs = {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("reopen");
            db.chapters_for_range(ts, ts + 400_000).expect("segs")
        };
        let core = segs[0].core_started_at;
        let json = format!(
            r#"{{"segment_ref":"segment:{core}","activity":"x","entities":[],"confidence":0.5,"evidence_refs":["frame:{fid}"],"open_questions":[]}}"#
        );
        let sentinel = tmp.0.join("started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 2);
        heartbeat::beat(&tmp.0, crate::now_ms()).expect("recording beat");
        assert!(
            heartbeat::is_recording(&tmp.0, crate::now_ms()),
            "前提：停止之前她在錄"
        );

        let handle = Handle::maybe_spawn(
            &tmp.0,
            BrainConfig {
                command,
                args,
                ..Default::default()
            },
            ts,
        )
        .expect("spawn")
        .expect("armed");

        let dir = tmp.0.clone();
        let joined = std::thread::spawn(move || handle.shutdown());

        let gave_up = Instant::now() + Duration::from_secs(2);
        loop {
            let now = crate::now_ms();
            if !heartbeat::is_recording(&dir, now) {
                assert!(
                    heartbeat::is_occupied(&dir, now),
                    "行程還在想最後一段，目錄卻說沒人佔著"
                );
                match heartbeat::presence(&dir, now) {
                    heartbeat::Presence::Thinking { .. } => {}
                    other => panic!("停止之後心跳該是 Thinking，實際是 {other:?}"),
                }
                let why = heartbeat::occupied_why(&dir, now).expect("佔著就要說為什麼");
                assert!(why.contains("想最後一段"), "{why}");
                assert!(why.contains("秒"), "{why}");
                break;
            }
            assert!(
                Instant::now() < gave_up,
                "shutdown 開始兩秒後心跳還在說她在錄"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let report = joined.join().expect("join");
        assert!(
            report.last_segment == LastSegment::Ran
                || report.last_segment == LastSegment::TimedOut
                || report.open_failed.is_some(),
            "最後一段該被想過：{report:?}"
        );
        heartbeat::stop(&tmp.0, crate::now_ms());
        assert!(
            !heartbeat::is_occupied(&tmp.0, crate::now_ms()),
            "墓碑之後才可以再開一個"
        );
        assert!(!heartbeat::is_recording(&tmp.0, crate::now_ms()));
    }

    /// `Drop` 會等解釋執行緒退出；退出之後磁碟也必須說沒有人在想。
    #[test]
    fn dropping_the_handle_leaves_a_tombstone_after_joining() {
        let tmp = Tmp::new("drop-thinking");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_950_000;
        let fid = seed_worth(&mut db, ts);
        drop(db);
        let core = {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("reopen");
            db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at
        };
        let json = format!(
            r#"{{"segment_ref":"segment:{core}","activity":"x","entities":[],"confidence":0.5,"evidence_refs":["frame:{fid}"],"open_questions":[]}}"#
        );
        let sentinel = tmp.0.join("started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 0);

        let beat_at = crate::now_ms();
        heartbeat::beat(&tmp.0, beat_at).expect("recording beat");
        let handle = Handle::maybe_spawn(
            &tmp.0,
            BrainConfig {
                command,
                args,
                ..Default::default()
            },
            ts,
        )
        .expect("spawn")
        .expect("armed");

        // **不呼叫 shutdown()。** 模擬 panic unwind 直接丟掉 owner。
        drop(handle);

        let after_drop = crate::now_ms();
        assert!(
            !heartbeat::is_occupied(&tmp.0, after_drop),
            "Drop 已經 join 完、行程不再想了，磁碟不可以還宣稱有人在想"
        );
        assert!(matches!(
            heartbeat::presence(&tmp.0, after_drop),
            heartbeat::Presence::Stopped { .. }
        ));
    }

    /// 正路上墓碑**歸呼叫端**，`Drop` 不准搶著蓋。
    ///
    /// `shutdown(mut self)` 收 `self`，所以它跑完 `Drop` 也會跑一次。要是
    /// `Drop` 在那裡蓋墓碑，`record` 收工那一段的順序就倒了——那一段自己
    /// 寫著「倒了的話墓碑會在 CLI 還跑著的時候放行第二個 recorder」，而
    /// `rec.finish()`（寫資料庫、可能失敗）還排在後面。
    #[test]
    fn shutdown_leaves_the_tombstone_to_the_caller() {
        let tmp = Tmp::new("shutdown-tombstone");
        grant_cloud(&tmp.0);
        let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_950_000;
        let fid = seed_worth(&mut db, ts);
        drop(db);
        let core = {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("reopen");
            db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at
        };
        let json = format!(
            r#"{{"segment_ref":"segment:{core}","activity":"x","entities":[],"confidence":0.5,"evidence_refs":["frame:{fid}"],"open_questions":[]}}"#
        );
        let sentinel = tmp.0.join("started");
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel, 0);

        heartbeat::beat(&tmp.0, crate::now_ms()).expect("recording beat");
        let handle = Handle::maybe_spawn(
            &tmp.0,
            BrainConfig {
                command,
                args,
                ..Default::default()
            },
            ts,
        )
        .expect("spawn")
        .expect("armed");

        handle.shutdown();

        // 腦已經加入，但 CLI 還要寫 session end / finish()。這段路上
        // `is_occupied` 必須還是 true，直到呼叫端自己蓋墓碑。
        let after = crate::now_ms();
        assert!(
            heartbeat::is_occupied(&tmp.0, after),
            "shutdown() 之後墓碑就蓋上去的話，第二個 recorder 會在 CLI 還跑著的時候進來"
        );
        assert!(
            !heartbeat::is_recording(&tmp.0, after),
            "迴圈已經跳出，不可以還說在錄"
        );

        // 呼叫端蓋上去，這時候才放開。
        heartbeat::stop(&tmp.0, crate::now_ms());
        assert!(!heartbeat::is_occupied(&tmp.0, crate::now_ms()));
    }

    #[test]
    fn shutdown_with_nothing_worth_does_not_print_timed_out() {
        let tmp = Tmp::new("last-nothing");
        grant_cloud(&tmp.0);
        let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        let ts = 1_700_000_900_000;
        let brain = BrainConfig {
            command: "python3".into(),
            args: vec!["-c".into(), "raise SystemExit(1)".into()],
            ..Default::default()
        };
        let mut engine = Engine::new(db, tmp.0.clone(), brain, ts).expect("engine");
        engine.step(ts + 5_000, Step::Shutdown).expect("shutdown");
        assert_eq!(engine.report.last_segment, LastSegment::NothingWorth);
        let text = format_report(&engine.report);
        assert!(text.contains("還開著的最後一段"), "{text}");
        assert!(text.contains("沒有值得理解的訊號可想"), "{text}");
        assert!(!text.contains("沒想完"), "{text}");
        assert!(text.contains("一次都沒醒"), "{text}");
    }

    /// 連續三天早上開機、都不跨午夜：每一天都該被盤點到，一天一次。
    /// 時間是餵進去的，不是真的等到明天。
    #[test]
    fn three_weekday_mornings_each_summarize_yesterday() {
        let tmp = Tmp::new("three-mornings");
        grant_cloud(&tmp.0);
        let brain = dummy_reviewer_brain();

        let Some(sunday) = local_ms(2026, 8, 23, 14, 0, 0) else {
            return;
        };
        let Some(monday) = local_ms(2026, 8, 24, 14, 0, 0) else {
            return;
        };
        let Some(tuesday) = local_ms(2026, 8, 25, 14, 0, 0) else {
            return;
        };
        let Some(monday_am) = local_ms(2026, 8, 24, 9, 0, 0) else {
            return;
        };
        let Some(tuesday_am) = local_ms(2026, 8, 25, 9, 0, 0) else {
            return;
        };
        let Some(wednesday_am) = local_ms(2026, 8, 26, 9, 0, 0) else {
            return;
        };

        let sun = brain::local_day_key(sunday).expect("sun");
        let mon = brain::local_day_key(monday).expect("mon");
        let tue = brain::local_day_key(tuesday).expect("tue");
        assert_eq!(sun, "2026-08-23");
        assert_eq!(mon, "2026-08-24");
        assert_eq!(tue, "2026-08-25");

        {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            seed_day_with_card(&mut db, sunday, "星期天改測試");
        }
        {
            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let mut engine =
                Engine::new(db, tmp.0.clone(), brain.clone(), monday_am).expect("engine");
            engine.catch_up(monday_am).expect("monday morning");
            assert!(
                engine.report.reviewer_eod_runs >= 1,
                "星期一早上該補星期天：{:?}",
                engine.report
            );
            assert_live_summary(&engine.db, &sun, "星期天改測試");
        }

        {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            seed_day_with_card(&mut db, monday, "星期一讀 SPEC");
        }
        {
            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let mut engine =
                Engine::new(db, tmp.0.clone(), brain.clone(), tuesday_am).expect("engine");
            engine.catch_up(tuesday_am).expect("tuesday morning");
            assert!(
                engine.report.reviewer_eod_runs >= 1,
                "星期二早上該補星期一：{:?}",
                engine.report
            );
            assert_live_summary(&engine.db, &mon, "星期一讀 SPEC");
        }

        {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            seed_day_with_card(&mut db, tuesday, "星期二寫 RESULT");
        }
        {
            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let mut engine =
                Engine::new(db, tmp.0.clone(), brain.clone(), wednesday_am).expect("engine");
            engine.catch_up(wednesday_am).expect("wednesday morning");
            assert!(
                engine.report.reviewer_eod_runs >= 1,
                "星期三早上該補星期二：{:?}",
                engine.report
            );
            assert_live_summary(&engine.db, &tue, "星期二寫 RESULT");
        }

        let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
        assert_live_summary(&db, &sun, "星期天改測試");
        assert_live_summary(&db, &mon, "星期一讀 SPEC");
        assert_live_summary(&db, &tue, "星期二寫 RESULT");
    }

    /// 同一天重開錄製兩次，日終不會跑第二次。
    #[test]
    fn the_same_morning_does_not_run_eod_twice() {
        let tmp = Tmp::new("same-morning");
        grant_cloud(&tmp.0);
        let brain = dummy_reviewer_brain();

        let Some(sunday) = local_ms(2026, 8, 23, 14, 0, 0) else {
            return;
        };
        let Some(monday_am) = local_ms(2026, 8, 24, 9, 0, 0) else {
            return;
        };
        let sun = brain::local_day_key(sunday).expect("sun");
        let mon = brain::local_day_key(monday_am).expect("mon");
        assert_eq!(sun, "2026-08-23");
        assert_eq!(mon, "2026-08-24");

        {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            seed_day_with_card(&mut db, sunday, "星期天改測試");
        }
        {
            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let mut engine =
                Engine::new(db, tmp.0.clone(), brain.clone(), monday_am).expect("engine");
            engine.catch_up(monday_am).expect("first boot");
            assert!(
                engine.report.reviewer_eod_runs >= 1,
                "第一次開機該補星期天：{:?}",
                engine.report
            );
            assert_live_summary(&engine.db, &sun, "星期天改測試");
            assert_eq!(
                engine.db.last_reviewer_eod_day().unwrap().as_deref(),
                Some(mon.as_str())
            );
        }
        {
            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let mut engine = Engine::new(db, tmp.0.clone(), brain, monday_am).expect("engine");
            engine.catch_up(monday_am).expect("second boot");
            assert_eq!(
                engine.report.reviewer_eod_runs, 0,
                "同一天再開不該再跑日終：{:?}",
                engine.report
            );
            assert_live_summary(&engine.db, &sun, "星期天改測試");
            assert_eq!(
                engine.db.last_reviewer_eod_day().unwrap().as_deref(),
                Some(mon.as_str())
            );
        }
    }

    /// 關機三天後開機只補昨天。更早的日子仍是 NeverRan。
    #[test]
    fn catch_up_after_three_days_off_only_summarizes_yesterday() {
        let tmp = Tmp::new("three-days-off");
        grant_cloud(&tmp.0);
        let brain = dummy_reviewer_brain();

        let Some(friday) = local_ms(2026, 8, 21, 14, 0, 0) else {
            return;
        };
        let Some(saturday) = local_ms(2026, 8, 22, 14, 0, 0) else {
            return;
        };
        let Some(sunday) = local_ms(2026, 8, 23, 14, 0, 0) else {
            return;
        };
        let Some(monday_am) = local_ms(2026, 8, 24, 9, 0, 0) else {
            return;
        };
        let fri = brain::local_day_key(friday).expect("fri");
        let sat = brain::local_day_key(saturday).expect("sat");
        let sun = brain::local_day_key(sunday).expect("sun");
        assert_eq!(fri, "2026-08-21");
        assert_eq!(sat, "2026-08-22");
        assert_eq!(sun, "2026-08-23");

        {
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            seed_day_with_card(&mut db, friday, "星期五改測試");
            seed_day_with_card(&mut db, saturday, "星期六讀 SPEC");
            seed_day_with_card(&mut db, sunday, "星期天寫 RESULT");
        }
        {
            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let mut engine = Engine::new(db, tmp.0.clone(), brain, monday_am).expect("engine");
            engine.catch_up(monday_am).expect("monday morning");
            assert!(
                engine.report.reviewer_eod_runs >= 1,
                "星期一早上該補星期天：{:?}",
                engine.report
            );
            assert_live_summary(&engine.db, &sun, "星期天寫 RESULT");
            for (date, why) in [(fri.as_str(), "星期五"), (sat.as_str(), "星期六")] {
                match engine.db.day_summary_glance(date).unwrap() {
                    DaySummaryGlance::NeverRan { date: d } => assert_eq!(d, date),
                    other => panic!("{why} 沒被盤點，該是 NeverRan，實際是 {other:?}"),
                }
            }
        }
    }
}
