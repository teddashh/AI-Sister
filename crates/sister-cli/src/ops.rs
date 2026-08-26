//! 各個子命令的實作。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use sister_core::db::Db;

pub mod speak {
    use super::*;
    use chrono::{Local, Timelike};
    use sister_core::gatekeeper::{FocusMode, GateInput, Verdict, decide};
    use sister_core::moments::SpeakCategory;

    fn signal_source_notice(category: SpeakCategory) -> Option<String> {
        match category {
            SpeakCategory::CommitmentDue | SpeakCategory::Stuck | SpeakCategory::SessionEnd => None,
            SpeakCategory::UnattendedNotification | SpeakCategory::Leaving => {
                Some(format!("{}：這一類目前沒有訊號源。", category.as_str()))
            }
        }
    }

    pub fn run(
        data_dir: &Path,
        config: &sister_core::Config,
        dry_run: bool,
        day: Option<&str>,
    ) -> Result<()> {
        let db = Db::open(&sister_core::Config::db_path(data_dir))?;
        let now = sister_core::now_ms();
        let day_key = match day {
            Some(day) => {
                sister_core::local_day::local_day_bounds(day)
                    .ok_or_else(|| anyhow::anyhow!("看不懂日期 {day:?}，要用 YYYY-MM-DD"))?;
                day.to_string()
            }
            None => sister_core::local_day::local_day_key(now)
                .ok_or_else(|| anyhow::anyhow!("現在時間無法換成本地日期"))?,
        };
        if !dry_run {
            let spent = db.points_spent_today(&day_key)?;
            println!(
                "{day_key}：用了 {spent} 點 / 上限 {} 點",
                config.gatekeeper.daily_budget_points
            );
            for row in db.utterances_on_day(&day_key)? {
                let decision = match row.decision {
                    sister_core::db::UtteranceDecision::Spoke { form, cost } => {
                        format!("開口 {}，{cost} 點", form.as_str())
                    }
                    sister_core::db::UtteranceDecision::Held { reason } => {
                        format!("擋下：{reason}")
                    }
                };
                println!(
                    "{} {} score={:.3} {} — {}",
                    row.ts,
                    row.category.as_str(),
                    row.score,
                    decision,
                    row.text
                );
            }
            return Ok(());
        }

        let candidates = sister_core::gatekeeper_candidates::collect(&db, now)?;
        // 走 `ALL`，不是走一份手寫的清單。手寫的那一份和 `signal_source_notice`
        // 的 `match` 是同一件事的兩個副本，而下一次有人接上訊號源的時候只會
        // 改到其中一份——留在畫面上的那一句會宣布一件已經不成立的事。
        for category in SpeakCategory::ALL {
            if let Some(line) = signal_source_notice(category) {
                println!("{line}");
            }
        }
        // d 類沉默的理由**不寫死**，跟 `collect()` 問同一支函式。d 有四種
        // 結果、其中三種都是「沒有候選」，而三個理由完全不同：一句寫死的
        // 「沒有最近的日終盤點」會在他剛把那天的筆記忘掉之後印出來，
        // 而盤點一分鐘前才跑完。
        if let Some(line) = sister_core::gatekeeper_candidates::session_end(&db, now)?.why_silent()
        {
            println!("{line}");
        }
        if candidates.is_empty() {
            println!(
                "現在一句候選都沒有：a 類沒有 40 分鐘內到期的顯式時間承諾，c 類沒有最近 40 分鐘的卡住訊號。d/b/e 三類的狀況見上面幾行。"
            );
            return Ok(());
        }
        let presence = sister_core::heartbeat::presence(data_dir, now);
        // 這一版沒有跨平台的「前景視窗是不是全螢幕」訊號，所以講出來。
        // 傳 `Windowed` 會是一句沒有人查證過的斷言。
        println!("專注模式：這一版量不到（沒有前景視窗幾何訊號），所以沒有靜音。");
        let local = Local::now();
        let quiet_hours_end = config
            .gatekeeper
            .quiet_end_at((local.hour() * 60 + local.minute()) as u16)?;
        let first = db.first_recording_at()?.unwrap_or(now);
        let days_since = u32::try_from(now.saturating_sub(first) / 86_400_000).unwrap_or(u32::MAX);
        let spent = db.points_spent_today(&day_key)?;
        let ever = db.has_ever_spoken()?;
        for candidate in candidates {
            let cooldown_remaining_minutes =
                db.last_spoke_at(candidate.category)?.and_then(|last| {
                    let elapsed = now.saturating_sub(last) / 60_000;
                    (elapsed < i64::from(config.gatekeeper.cooldown_minutes))
                        .then(|| config.gatekeeper.cooldown_minutes - elapsed as u32)
                });
            let input = GateInput {
                candidate: candidate.clone(),
                presence,
                quiet_hours_end: quiet_hours_end.clone(),
                focus_mode: FocusMode::Unmeasured,
                days_since_first_recording: days_since,
                cold_start_days: config.gatekeeper.cold_start_days,
                has_ever_spoken: ever,
                cooldown_remaining_minutes,
                points_spent_today: spent,
                daily_budget_points: config.gatekeeper.daily_budget_points,
                min_score: config.gatekeeper.min_score,
            };
            let verdict = decide(&input);
            let line = match &verdict {
                Verdict::Speak { form, cost } => format!("開口：{}，{cost} 點", form.as_str()),
                Verdict::Hold(reason) => format!("擋下：{}（{}）", reason.code(), reason.message()),
            };
            println!(
                "{} score={:.3} [impact={:.3} confidence={:.3} timeliness={:.3} evidence_strength={:.3}] {line} — {}",
                candidate.category.as_str(),
                candidate.score(),
                candidate.impact,
                candidate.confidence,
                candidate.timeliness,
                candidate.evidence_strength,
                candidate.text
            );
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn source_notices_keep_b_and_e_missing_but_not_d() {
            let b = signal_source_notice(SpeakCategory::UnattendedNotification)
                .expect("b 仍然沒有訊號源");
            assert!(b.contains("沒有訊號源"));
            assert!(b.contains("unattended_notification"));
            assert!(!b.contains("已接上"));

            assert!(signal_source_notice(SpeakCategory::SessionEnd).is_none());

            let e = signal_source_notice(SpeakCategory::Leaving).expect("e 仍然沒有訊號源");
            assert!(e.contains("沒有訊號源"));
            assert!(e.contains("leaving"));
            assert!(!e.contains("已接上"));
        }
    }
}

/// 兩次看螢幕之間的下限。設定檔可以寫得更慢，寫得更快則無效。
///
/// 定在這裡而不是各自 `.max(200)`，是因為 doctor 得印出**實際會用的值**：
/// 一個調了不生效的旋鈕，比一個沒有這個旋鈕更糟。
pub(crate) const MIN_TICK_MS: u64 = 200;

/// 測試用的暫存目錄。自己刻，不引 `tempfile`——理由見
/// `scripts/check-no-network.sh`：出貨的相依樹上每多一個 crate，那份稽核
/// 就多一份要人讀的東西，而這個結構只有九行。
#[cfg(test)]
pub(crate) mod tmp {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub struct Tmp(pub PathBuf);

    impl Tmp {
        pub fn new(name: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        /// 寫一份設定檔進去，回傳它的路徑。
        pub fn file(&self, body: &str) -> PathBuf {
            let p = self.0.join("config.toml");
            std::fs::write(&p, body).expect("write");
            p
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// 開啟既有資料庫。查詢類命令不該憑空造一顆空的資料庫出來——
/// 那會讓「我明明錄了東西」變成「查無資料」的無聲錯誤。
fn open_existing(data_dir: &Path) -> Result<Db> {
    let path = crate::db_path(data_dir);
    anyhow::ensure!(
        path.exists(),
        "找不到資料庫：{}\n先跑 `sister replay <腳本>` 或 `sister record` 產生資料。",
        path.display()
    );
    Db::open(&path).with_context(|| format!("open {}", path.display()))
}

/// `30m`／`2h`／`7d` → 毫秒。
///
/// 匯出與刪除共用同一套寫法，避免同一個 `--last` 在兩個指令裡代表不同時間。
/// 單位不可以省：`30` 猜成分鐘或天，兩邊都會是一個看似合理的錯誤。
pub(crate) fn parse_span(s: &str) -> Result<sister_core::Millis> {
    const SECOND: sister_core::Millis = 1_000;
    const MIN: sister_core::Millis = 60 * SECOND;
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .with_context(|| format!("看不懂「{s}」——要像 `30m`、`2h`、`7d` 這樣寫"))?;
    let mult = match unit {
        "s" | "sec" => SECOND,
        "m" | "min" => MIN,
        "h" | "hr" => 60 * MIN,
        "d" | "day" => 24 * 60 * MIN,
        "" => anyhow::bail!("「{s}」少了單位。`{s}m` 是 {s} 分鐘、`{s}d` 是 {s} 天，差很多"),
        other => anyhow::bail!("看不懂單位「{other}」——只認得 s（秒）、m（分）、h（時）、d（天）"),
    };
    n.checked_mul(mult)
        .with_context(|| format!("「{s}」太長了"))
}

/// 把「排除 80」拆成「是誰擋的」。
///
/// 摘要上那個數字本身沒有錯，但它回答不了使用者唯一會問的問題。而排除
/// 恰恰是這個專案最容易安靜地過度生效的地方——規則寫寬了、UIA 一直答不出
/// 密碼欄狀態、某個 app 名稱剛好是別人的子字串，症狀全都長得一樣：
/// 她什麼都記不住，摘要上只有一個沒有解釋的數字。
///
/// 印出來的理由字串和寫進 `system_events` 的是同一串，所以看到什麼就能
/// 拿什麼去資料庫裡查。
/// 整拍炸掉了幾次（`None` = 一次都沒有）。
///
/// 錄製迴圈把單次 tick 的錯誤吞成一行 `tracing::warn!` 就繼續跑，而那個決定
/// 是對的：抓不到畫面多半是暫時的（切使用者、螢幕休眠），下一秒就好了。錯
/// 的是那行 warn **是唯一的紀錄**，而它躺在 `%APPDATA%` 深處的 `record.log`
/// 裡——會去翻那個檔案的人已經知道出事了。
///
/// 沒有這一行的話，一場每一拍都炸掉的錄製和一場「你今天只是沒開電腦」印出
/// 同樣五個零。`ocr_failures`、`image_failures` 早就各自有計數也各自有一句
/// 話，理由逐字相同——這一條只是把同一條規則補在最大的那個洞上。
///
/// 「全部」和「偶爾幾次」要分得開：前者是她這段時間什麼都沒記住，後者是
/// 正常的雜訊。分界用 `working_ticks`（過了 `capture.enabled` 和暫停那兩道
/// 門的拍數），因為空轉的拍本來就不會失敗，算進去只會把比例稀釋掉。
fn tick_failures_line(stats: &sister_capture::RecorderStats) -> Option<String> {
    let n = stats.tick_failures;
    if n == 0 {
        return None;
    }
    // 一句原文遠比一個數字有用。`last_*_error` 那兩個欄位同一個理由。
    let why = match &stats.last_tick_error {
        Some(e) => format!("最後一次是：{e}"),
        None => "但沒有留下原因（這是 bug）".to_string(),
    };
    Some(if n >= stats.working_ticks && stats.working_ticks > 0 {
        format!("  ⚠  這 {n} 拍**每一拍都失敗了**——她這段時間什麼都沒記住。{why}")
    } else {
        format!(
            "  ⚠  {n} 拍失敗（共 {} 拍做過事）。{why}",
            stats.working_ticks
        )
    })
}

/// 把「她有多少時間是閉著眼睛的」講出來。
///
/// 這個數字不講就是 bug。省電和停止工作在帳面上長得一模一樣：tick 照跑、
/// 沒有錯誤、CPU 很漂亮，而畫面上完全看不出來她其實有 87% 的時間沒看螢幕。
/// 那正是 alpha.4 那種「✓ 但什麼都沒產出」的失效形狀，只是這次是我們自己
/// 刻意造出來的——所以更該說。
///
/// 而 `skipped_idle == 0` 有三種意思，以前它們印出來一模一樣（一片空白，
/// 因為這裡直接 `return`）：他真的整天都在打字、這台機器答不出閒置訊號、
/// 標題一直在跳所以閘門根本沒被問到。後面兩個是**閘門從第一秒起就沒生效**
/// ——CPU 直接超支十幾倍，而摘要一個字都不說。
fn idle_line(stats: &sister_capture::RecorderStats) -> Option<String> {
    if stats.skipped_idle > 0 {
        // 分母是 `working_ticks` 不是 `ticks`：暫停和關閉的空轉拍從來沒問過
        // 「有人動過嗎」，把它們算進去只會把省下來的比例稀釋掉。
        let pct = stats.skipped_idle as f64 / stats.working_ticks.max(1) as f64 * 100.0;
        Some(format!(
            "  省下：{} 次沒碰螢幕（{pct:.0}%——那段時間你沒動鍵盤滑鼠；最多每 5 秒仍會看一次）",
            stats.skipped_idle
        ))
    } else if stats.idle_unknown > 0 {
        // 不說「這台機器」：`replay` 走的是腳本後端，它本來就沒有閒置訊號，
        // 而這一行在兩個地方都會印。說「擷取後端」兩邊都是真的。
        Some(format!(
            "  ⚠  省電閘門一次都沒生效：問了 {} 次「有人動過嗎」，這個擷取後端一次都答不出來。\
             \n\x20    每一拍都會真的去讀一次螢幕——真的錄起來的話，CPU 大約是設計值的十幾倍。",
            stats.idle_unknown
        ))
    } else if stats.idle_asked > 0 {
        // 條件是 `idle_asked` 不是 `working_ticks`。這一行是一句**關於使用者
        // 的斷言**（「你一直在動」），所以它得有依據，而依據只有一個：那一拍
        // 真的走到了閘門。
        //
        // `working_ticks` 在 tick 的第 0 步就加了，閘門在第 5 步。被排除規則
        // 擋掉的、以及在第 3～4 步 `?` 出去的，統統沒走到那裡。於是一場「資
        // 料庫寫不進去、每一拍都炸掉」的錄製，摘要印的是
        //
        //     完成：7200 tick → 保留 0、重複 0、排除 0、無畫面 0
        //     省下：0 次沒碰螢幕（這段時間你一直在動，或每一拍脈絡都變了）
        //
        // 五個零加一句她編出來的解釋，而那是整份摘要裡唯一一句解釋。
        // 真正的原因在 `tick_failures_line`。
        Some("  省下：0 次沒碰螢幕（這段時間你一直在動，或每一拍脈絡都變了）".to_string())
    } else {
        None
    }
}

fn report_idle(stats: &sister_capture::RecorderStats) {
    if let Some(line) = idle_line(stats) {
        println!("{line}");
    }
    if stats.title_clock_ticks > 0 {
        println!(
            "  忽略：{} 次標題跳動（那個視窗的標題是時鐘——進度、未讀數、跑動的 log。\
             跟著它睜眼的話，省電閘門會整天關著）",
            stats.title_clock_ticks
        );
    }
}

fn report_exclusions(stats: &sister_capture::RecorderStats) {
    if stats.excluded_reasons.is_empty() {
        return;
    }
    // 擋最多的排前面：真正在吃掉一天的那條規則要第一個被看見
    let mut by_count: Vec<_> = stats.excluded_reasons.iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (reason, n) in by_count {
        println!("        排除 {n} 次：{reason}");
    }
    if stats.kept == 0 && stats.excluded > 0 {
        println!(
            "  ⚠  這一整段沒有留下任何畫面，全部被上面的規則擋掉了。\
             如果那不是你要的，改 config 的 privacy 那一段。"
        );
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    /// 一場「每一拍都炸掉」的錄製，摘要必須說得出這件事，而且**不准**
    /// 順口替使用者作證。
    ///
    /// 這兩件事是同一個 bug 的兩面。舊的摘要印的是
    ///
    ///     完成：7200 tick → 保留 0、重複 0、排除 0、無畫面 0
    ///     省下：0 次沒碰螢幕（這段時間你一直在動，或每一拍脈絡都變了）
    ///
    /// 五個零，和「今天沒開電腦」逐字相同；唯一多出來的那句話還是編的
    /// ——那一拍在第 4 步就 `?` 出去了，省電閘門在第 5 步，根本沒被問到。
    /// 所以：⚠ 要出現，「你一直在動」要消失。
    #[test]
    fn a_recording_that_failed_every_tick_says_so_instead_of_vouching_for_him() {
        let mut s = sister_capture::RecorderStats {
            working_ticks: 7_200,
            tick_failures: 7_200,
            last_tick_error: Some("insert frame: database is locked".into()),
            ..Default::default()
        };
        // 閘門在第 5 步，這一場一次都沒走到——`idle_asked` 維持 0。

        let warn = tick_failures_line(&s).expect("整拍都炸掉了，一定要有一行");
        assert!(warn.contains("每一拍都失敗了"), "{warn}");
        assert!(warn.contains("database is locked"), "要帶原文：{warn}");
        assert_eq!(idle_line(&s), None, "閘門沒被問到就不准解釋為什麼省下 0 次");

        // 對照組：閘門真的問到了、而他真的一直在動——那句話這時才有依據。
        s.tick_failures = 0;
        s.last_tick_error = None;
        s.idle_asked = 7_200;
        assert_eq!(tick_failures_line(&s), None);
        assert!(
            idle_line(&s).is_some_and(|l| l.contains("你一直在動")),
            "問到了閘門、一次都沒省下，就是他真的一直在動"
        );
    }

    /// 偶爾炸幾拍是雜訊，不是「她什麼都沒記住」——兩句話不能混用。
    #[test]
    fn a_few_failed_ticks_do_not_get_the_end_of_the_world_sentence() {
        let s = sister_capture::RecorderStats {
            working_ticks: 7_200,
            tick_failures: 3,
            last_tick_error: Some("grab: 螢幕鎖定".into()),
            ..Default::default()
        };
        let warn = tick_failures_line(&s).expect("三拍也要說");
        assert!(warn.contains("3 拍失敗（共 7200 拍做過事）"), "{warn}");
        assert!(!warn.contains("每一拍都失敗了"), "{warn}");
    }
}

/// 「這顆資料庫裡沒有東西」的**三種**來源。
///
/// 這個型別存在的理由，是同一組 0 曾經被三個地方各自講成同一件事，而三種的
/// 下一步完全不同：`Fresh` 要他去按開始錄，`Blocked` 要他去看那幾條規則，
/// `Erased` 是他五分鐘前才親手做的、不需要任何下一步。
///
/// **它只回答「為什麼是空的」，不回答「空不空」。** 呼叫端要自己先確定手上
/// 真的是 0 再問它——問錯的下場長這樣：`兩個字的中文` 那一列拿 `with_cjk == 0`
/// 去問，於是一台跑英文、順手擋了 keepassxc 的機器被告知「沒有中文可以驗，
/// 那段時間被規則擋掉了」，而它的 6 段英文好好地躺在兩行之上的 ✓ 裡。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emptiness {
    /// 從來沒錄過，或錄了但還沒輪到第一列。
    Fresh,
    /// 這顆資料庫裡有擋掉／暫停的紀錄可以解釋這個 0——證據還在，指過去。
    ///
    /// 它**不宣稱那是唯一的原因**：`forget --last 24h` 刪的是一段時間，前天
    /// 那幾列排除稽核會活下來，於是「昨天被擋光、今天被忘掉」兩件事同時成立。
    /// 那時候該先看的還是那幾條規則（它們確實擋過東西），只是這句話不可以
    /// 反過來說成「沒有人刪過任何東西」。
    Blocked,
    /// 錄過、**存過東西**，而現在一列都不剩：`forget` 或保留期。
    Erased,
    /// 她跑過，可是這顆資料庫**從來沒有一列內容落地**。
    ///
    /// 最常見的一種：`capture.enabled = false`。她開場、跑完、收工，
    /// `ever_recorded` 一路是 true，而 `sister forget` 從來沒被執行過。
    ///
    /// 這一種以前不存在，它是被 `Erased` 吃掉的——三個畫面同時告訴他「被
    /// `sister forget` 忘掉了，或是過了保留期」，而他一次都沒刪過東西。那句
    /// 話指控了一件沒發生的事，而且把下一步指到相反的方向：該看的是
    /// `capture.enabled`，不是保留期。
    ///
    /// **alpha.33 以前就被清空的資料庫不在這裡**，它落在 `Erased`：升級那一刻
    /// 那顆檔案答不出「它到底存過沒有」，而 migration 005 給它一張
    /// `assumed-at-upgrade` 的標籤，讓它繼續說 alpha.32 說過的那句話。升級不
    /// 可以改寫一句關於他的資料的舊話——他昨天刪掉一整天，今天升級，然後被送
    /// 去看一個他機器上根本沒問題的設定。理由寫在 `db::migration_005`。
    Barren,
    /// [`Barren`](Self::Barren) 的另一半：她**此刻正佔著這個資料目錄**，而一
    /// 列內容都還沒落地。
    ///
    /// 兩者的下一步是相反的：一個要他去看設定，一個要他**再等一下**。而它們
    /// 之間有一整個灰帶——第一拍還沒跑完的 recorder、每一拍都在錯的
    /// recorder、開起來就被砍掉的 recorder——三種都長成這個樣子，所以這句話
    /// 只講看得見的那一件事（她正開著、還沒有東西），把「是哪一種」留給
    /// `sister doctor`。
    ///
    /// 這一種也是被 `Barren` 吃掉的，而 `Barren` 是我上一版**修出來**的：修
    /// 「兩種 0 長得一樣」的手段是多問一個位元，而多問的那個位元自己又蓋住了
    /// 一組。字母人那邊早就分對了（`blind_lines` 的 `recording_now` 問在
    /// `!ever_stored` 前面），CLI 這三頁沒跟上——`Emptiness::of` 收不到
    /// `data_dir`，於是它結構上就問不出這一題。
    Live,
    /// [`Live`](Self::Live) 的另一半：有一個 recorder **正在起來**，而它還沒
    /// 開始錄。
    ///
    /// `BootBeat` 一寫下心跳，`Db::open` 才開始跑——一顆存了一年的資料庫要好
    /// 幾分鐘。那幾分鐘裡「有人佔著這個目錄」是真的、「她正在錄」是假的，而
    /// 上一版把 `heartbeat::phase` 壓成 `beat.is_some()` 餵進來，於是這四頁一
    /// 起說「她此刻正在錄，還沒有東西落地」，而同一份 doctor 底下兩行寫著
    /// 「有一個 sister record **正在起來**……還沒開始記東西」。
    ///
    /// **這一種是上一版修「開機不算在錄」自己造出來的。** 那一版把 `Phase` 帶
    /// 進了 `crash_audit` 和 `signal_audit`，卻讓隔壁的 `Emptiness` 留在那個被
    /// 證明不夠用的布林上——修好一對雙胞胎的同時，讓它和鄰居變成新的一對。
    /// 一個位元進來之後，要去看這一整頁上還有誰在描述同一件事。
    ///
    /// 下一步和 [`Live`](Self::Live) 一樣是「再等一下」，但講的不是同一件事：
    /// `Live` 的 recorder 已經在跑、每一拍都可能是壞的（所以那句話把「是哪一
    /// 種」推給 doctor），這一種**連第一拍都還沒開始**，沒有任何東西可以是壞的。
    Booting,
}

impl Emptiness {
    /// 六種全部。**這一節的測試靠它站著**：「N 種 0 的下一步都不一樣，不可以
    /// 共用一句話」那幾個迴圈遍歷的就是這個陣列。
    ///
    /// 以前它們寫的是 `[Fresh, Blocked, Erased]` 這種字面量，而**陣列字面量不
    /// 會因為 enum 多一種變體而編不過**——`Barren` 加進來的那一版，那兩個迴圈
    /// 還停在三種。守著「不可以兩種處境共用一句話」的那條測試，剛好漏掉的就是
    /// 那次新加的那一種。同 [`crate::ops::Emptiness`] 底下每一個 `match` 都不
    /// 寫 `_`，是同一根釘子。
    ///
    /// `#[cfg(test)]`：產品那邊沒有人要遍歷五種（每一頁都是一個 `match`），所
    /// 以留著只會是一個 `dead_code`。代價是「多一種變體就編不過」只在
    /// `cargo test` 那一趟成立——而 CI 兩趟都跑。
    #[cfg(test)]
    pub const ALL: [Self; 6] = {
        // 這支函式只有一個用途：多一種變體的時候讓底下那一列在這裡編不過。
        const fn _every_one_of_them(e: Emptiness) -> u8 {
            match e {
                Emptiness::Fresh => 0,
                Emptiness::Blocked => 1,
                Emptiness::Erased => 2,
                Emptiness::Barren => 3,
                Emptiness::Live => 4,
                Emptiness::Booting => 5,
            }
        }
        [
            Self::Fresh,
            Self::Blocked,
            Self::Erased,
            Self::Barren,
            Self::Live,
            Self::Booting,
        ]
    };

    /// 資料庫自己回答這是哪一種。
    ///
    /// `Erased` 問在最前面，因為它的條件最強（`nothing_recorded_left()` 要求
    /// 每一個計數器都是 0，包含稽核紀錄自己那幾列）。條件最強的先問，才不會被
    /// 一個條件比較寬的答案蓋掉。
    ///
    /// 但「條件最強」在這裡也是一個陷阱，而我踩過：往那個述詞多塞一張表，就是
    /// 把 `Erased` 收窄一次，而它讓開之後接的是**最寬**的那個答案 `Fresh`——
    /// 「她還沒錄過」。`queries` 曾經被算進去，於是 `forget` 完問一句「真的沒
    /// 了嗎」，整個刪除就從三個畫面上消失了。改那個述詞以前先問：新加的這張表
    /// 會不會在清空**之後**長出來？
    ///
    /// **`ever_recorded` 和 `ever_stored` 是兩題，這裡兩題都要問。** 只問前面
    /// 那一題的時候，一台 `capture.enabled = false` 的機器會走進 `Erased`——見
    /// [`Emptiness::Barren`]。而 `ever_recorded` 自己的註解早就寫著它答不出這
    /// 一題（「旗標在 `start_session` 就翻成 1，第一張畫面之前」），我照樣拿
    /// 它去答了一次。
    ///
    /// **心跳要從呼叫端進來，因為資料庫答不出「她現在在不在」。** 三個呼叫端
    /// 手上本來就有 `data_dir`（而且同一頁上別的地方早就在問這個問題了），所
    /// 以這不是多要一份資料，是把一份已經在手上的資料接進來——上一版沒接，於
    /// 是這支函式結構上就答不出 [`Live`](Self::Live)。
    ///
    /// **收的是 [`Presence`] 本人，不是壓扁的 `Option<Phase>`。** 上一版收
    /// `is_occupied`，理由寫成「正在開機的 recorder 也佔著這個目錄，而那時候
    /// 『再等一下』正是對的話」——下一步猜對了，句子卻錯了：那四頁印的是「她
    /// **此刻正在錄**」，而同一份 doctor 底下兩行寫著「有一個 sister record
    /// **正在起來**……還沒開始記東西」。同一頁，兩句互相打臉，第十八次。
    ///
    /// 再上一版收 `Option<Phase>`，於是 `Thinking` 掉進 `None` → [`Barren`]：
    /// 按下停止之後那兩分鐘，空資料庫上這四頁說「多半是 `capture.enabled =
    /// false`」，而同一份 doctor 六行之隔說行程還在。想最後一段不是一種新的
    /// 空（東西沒進來的理由還是「她跑過、沒存到」），所以走 [`Barren`]——但
    /// 要列出來，不能靠 `_`。
    ///
    /// [`Presence`]: sister_core::heartbeat::Presence
    pub fn of(
        db: &Db,
        s: &sister_core::db::DbStats,
        beat: sister_core::heartbeat::Presence,
    ) -> Result<Self> {
        use sister_core::heartbeat::{Phase, Presence};
        if s.nothing_recorded_left() && db.ever_recorded()? {
            if db.ever_stored()? {
                // 存過又清光了：她此刻在不在都不改變「東西被拿走了」這件事，
                // 所以這一種不分。
                return Ok(Self::Erased);
            }
            return Ok(match beat {
                Presence::Live(Phase::Recording) => Self::Live,
                Presence::Live(Phase::Booting) => Self::Booting,
                Presence::Thinking { .. }
                | Presence::NeverStarted
                | Presence::Unreadable
                | Presence::Stopped { .. }
                | Presence::Stalled { .. } => Self::Barren,
            });
        }
        Ok(
            if !db.exclusion_audit()?.is_empty() || db.pause_audit()?.episodes > 0 {
                Self::Blocked
            } else {
                Self::Fresh
            },
        )
    }
}

/// 那一列**為什麼**還在，以及它**什麼時候**會走。兩句一起回，因為第二句完全
/// 取決於第一句——拆開的話遲早有人把「當掉」那一句配上「正在錄」那個下場。
///
/// 上一版這裡回的是一句「當掉了，或是她此刻正在錄」，理由寫成「他分得出自己
/// 是哪一種，這幾行分不出來」。**那句話是假的**：`forget` 在四行外就算過
/// `heartbeat::is_occupied`，`stats` 手上也有 `data_dir`。一個分得出來的地方
/// 印一個「或」，是把自己的懶惰講成使用者的功課。
///
/// **收 [`Phase`] 本人，不是一個布林。** 上一版收 `occupied = beat.is_some()`，
/// 於是開機那幾分鐘印的是「此刻有人佔著這個資料目錄（她正在錄，或正在開機）」
/// ——那個「或」正是上一段罵過的東西，而且它在 `stats` 上和同一秒的 `doctor`
/// 打架：doctor 說「她當掉了。現在有一個 sister record 正在起來」，stats 說那
/// 一列還開著是因為有人佔著。**開機那幾分鐘她那一列還沒 INSERT**，所以手上這
/// 一列是上一次當機留下來的殼，不是佔著目錄的那一個。三種心跳三句話。
///
/// 方向要往「不敢說死」倒——把一場活的錄製講成當機是這幾句話裡唯一會嚇到人的
/// 錯，所以只有 `Recording` 那一格敢說「她正在錄」。
///
/// [`Phase`]: sister_core::heartbeat::Phase
///
/// **第二句是量出來的，不是推出來的。** 兩支各有一條照抄產品呼叫順序的測試在
/// `retention.rs` 裡釘著，而兩句都曾經因為「照著守衛的條件用推的」而是假話：
///
/// * 「當掉」那一支 →
///   `the_shell_a_crash_left_behind_survives_the_next_recordings_startup_prune`。
///   `record` 的開機清理跑在 `start_session` **之前**，所以那一刀砍不到這一列
///   ——它那時候還是最新的一列。
/// * 「正在錄」那一支 →
///   `a_session_erased_mid_recording_goes_away_when_the_recorder_finishes`。
///   `Recorder::finish` **先**寫一列 `SessionEnd` **再**呼叫 `end_session`，於是
///   它自己剛剛寫的那一列讓那一場「不空」——那道清掃在產品裡從來沒有刪掉過任何
///   一列。現在 `delete_empty_sessions` 不把那兩列標籤當成內容，這句話才是真的。
fn session_shell_why(beat: sister_core::heartbeat::Presence) -> (&'static str, &'static str) {
    use sister_core::heartbeat::{Phase, Presence};
    match beat {
        Presence::Live(Phase::Recording) => (
            "那一場還沒收尾——她此刻正在錄",
            "等她收工的時候，那一場如果還是一列都不剩，那一列就會跟著走。",
        ),
        // 她的列還沒進來，所以手上這一列不可能是她的。「什麼時候會走」那一句
        // 因此跟著當機那一格走，只是等待變短了：正在起來的那一個一開始錄，這
        // 一列就不再是最新的一列。
        // 三句話都要接得上呼叫端那個「，裡面一列都不剩。」（見
        // [`forget::session_shell_note`]），所以這一句不能在中間收一個句號
        // ——收了的話「裡面」的先行詞會變成正在起來的那一個。
        Presence::Live(Phase::Booting) => (
            "那一場沒有正常收尾——她當掉了；現在有一個 sister record 正在起來，但這一列不是它的",
            "等它開始錄，這一列就不再是最新的一列，接下來任何一次清理都會把它帶走。",
        ),
        // 按下停止之後那兩分鐘：這一列**是她的**（`finish()` 還沒跑），行程
        // 還在。說「當掉了」或「沒有任何 recorder 佔著」都是假的。
        Presence::Thinking { .. } => (
            "那一場還沒收尾——錄製已停，解釋層還在想最後一段",
            "想完收工的時候，那一場如果還是一列都不剩，那一列就會跟著走。",
        ),
        Presence::NeverStarted
        | Presence::Unreadable
        | Presence::Stopped { .. }
        | Presence::Stalled { .. } => (
            "那一場沒有正常收尾——她當掉了（現在沒有任何 recorder 佔著這個資料目錄）",
            "她**再開始錄之後**，那一列就不再是最新的一列，接下來任何一次清理都會把它\
             帶走——想馬上清掉的話，開始錄之後跑一次 `sister prune`。",
        ),
    }
}

pub mod consent {
    use super::*;
    use sister_core::config::Config;
    use sister_core::consent::{Consent, Sheet};
    use std::str::FromStr;

    /// `sister consent [--grant …] [--revoke …]`。
    ///
    /// 在無頭機器上把整條同意流程走完的入口。字母人上那三張卡片按下去之後
    /// 改的是**同一個檔案**，所以這裡驗得過的東西，那邊也就驗過了。
    pub fn run(
        data_dir: &Path,
        config: &Config,
        grant: &[String],
        revoke: &[String],
        json: bool,
    ) -> Result<()> {
        let mut c = sister_core::consent::load(data_dir);

        // 先把名字全部解析完再動任何東西。一半成功一半失敗的狀態，會讓
        // 「我剛剛到底同意了什麼」變成一個沒有答案的問題。
        let grant: Vec<Sheet> = parse(grant)?;
        let revoke: Vec<Sheet> = parse(revoke)?;
        let changing = !grant.is_empty() || !revoke.is_empty();

        if changing {
            // 條文換版之後，舊簽名不算數（`current()` 會是 false）。這時候
            // 只要他重簽任何一張，其餘沒重簽的就該當作沒簽——所以整份先清掉，
            // 而不是讓一份半新半舊的檔案留在磁碟上假裝自己是完整的。
            if !c.current() && c != Consent::default() {
                println!("⚠  同意書條文已經改版，之前簽的那幾張不再算數，只保留這次指定的。");
                c = Consent::default();
            }
            let now = sister_core::now_ms();
            for s in &revoke {
                c.revoke(*s);
            }
            for s in &grant {
                c.grant(*s, now);
            }
            // 撤回全部之後 `version` 還停在舊值沒關係——`allows_*` 看的是
            // 「有沒有那個時戳」，而三個都 None 的時候答案本來就是不准。
            sister_core::consent::save(data_dir, &c)?;
        }

        if json {
            print_json(data_dir, &c, config)
        } else {
            print_human(data_dir, &c, config, changing);
            Ok(())
        }
    }

    fn parse(names: &[String]) -> Result<Vec<Sheet>> {
        names
            .iter()
            .map(|n| Sheet::from_str(n).map_err(anyhow::Error::msg))
            .collect()
    }

    /// 第二張同意書**簽下去之後**，這一頁上唯一告訴他「簽了會發生什麼」的幾行。
    ///
    /// 抽成函式不是為了漂亮，是因為 `print_human` 用 `println!` 直接寫 stdout，
    /// 於是這幾行在測試裡一個字都看不到——而它們正是這一頁上最容易過期的字。
    ///
    /// 底下那份指令清單就是 `Consent::cloud_permit()` 的呼叫端。加第五個持票人
    /// 的時候要回來改這裡——**沒有任何閘門會替你發現這份清單變短了**，短了也
    /// 只是少一行，讀的人看不出來少了什麼。
    fn cloud_reading_signed_lines(has_cli: bool) -> &'static [&'static str] {
        if has_cli {
            &[
                "螢幕上的原文會交給設定裡的那支 CLI，沒有去識別化。",
                "會用這張同意書的是 `sister interpret`、`sister review`、`sister watch`；`sister watch` 會照著 --every 連續問很多次。",
                // 這一行是整段裡他最不可能自己想到的那一句，所以最不能省。
                // 前三支要他自己打，`record` 不用——`sister record` 開著的時候
                // 會自己起一條 wakeup 執行緒去叫解釋層和審閱層（`wakeup.rs`），
                // 而 `record` 正好是唯一一支他會整天開著的。他以為那只是在錄。
                "還有 `sister record`：只要設定檔有 [brain] command，它一邊錄就會一邊自己叫解釋層和審閱層，不用你再下任何指令。",
            ]
        } else {
            &["已同意，但還沒設定 [brain] command，一次都不會呼叫。"]
        }
    }

    fn print_human(data_dir: &Path, c: &Consent, config: &Config, changed: bool) {
        println!(
            "三張同意書（{}）",
            sister_core::consent::path(data_dir).display()
        );
        for sheet in Sheet::ALL {
            let mark = match c.get(sheet) {
                // 簽過、但條文換版了 = 還是不算數，而且要看得出來是哪一種。
                Some(_) if !c.current() => "⟳",
                Some(_) => "✓",
                None => "✗",
            };
            println!("\n  {mark} {}", sheet.wording());
            match c.get(sheet) {
                Some(ts) => {
                    println!("      {} 同意", crate::fmt::timestamp(ts));
                    // 簽下去之後，第二張在這一頁上和另外兩張長得一模一樣——
                    // 而另外兩張簽下去是真的會改變行為。「這一張還沒有東西
                    // 可以開」那句話只寫在 `without()` 裡，也就是只在**沒簽**
                    // 的分支印得出來，於是唯一讀不到它的人正好是簽了的那個。
                    if sheet == Sheet::CloudReading {
                        for line in cloud_reading_signed_lines(config.brain.cli().is_some()) {
                            println!("      {line}");
                        }
                    }
                }
                None => println!("      {}", sheet.without()),
            }
        }

        println!();
        if c.allows_recording() && !config.capture.enabled {
            // 同意書全過、總開關關著。這一行以前會說「她可以錄，而且不留
            // 截圖——這一張簽了，是設定檔的 store_images 關著」：前半句錯
            // （她根本不會錄），後半句指錯旗標（`store_images` 開著，關著的
            // 是 `capture.enabled`）。他照著那句話去改一個沒問題的設定，
            // 改完還是什麼都沒有。
            println!(
                "→ 同意書這一關過了，但**設定檔的 capture.enabled 關著**：`sister record` 會啟動、\n  \
                 會印出「開始記錄」，然後每一個 tick 直接跳過，什麼都不會記。"
            );
        } else if c.allows_recording() {
            // 三個狀態，不是兩個。這一行以前只問同意書，於是簽了第三張、
            // 設定檔卻把 `store_images` 關著的機器上，它會說「會留截圖」
            // ——而硬碟上一張都不會多。同意是**上限**，不是開關。
            let frames = if c.keeps_images(config) {
                "會留截圖"
            } else if c.allows_frames() {
                "不留截圖——這一張簽了，是設定檔的 store_images 關著"
            } else {
                "只記螢幕上的字、不留截圖"
            };
            println!("→ 她可以錄，而且{frames}。");
        } else if c.get(Sheet::LocalRecording).is_some() {
            println!(
                "→ **她不會開始錄**：條文改版了（現在是第 {} 版），要重簽一次。\n  \
                 `sister consent --grant local-recording`",
                sister_core::consent::VERSION
            );
        } else {
            println!(
                "→ **她不會開始錄。** 要她開始請跑：\n  \
                 `sister consent --grant local-recording`\n  \
                 想連截圖一起留就再加 `--grant frame-storage`。"
            );
        }
        if !changed {
            println!("\n（這次沒有改動任何東西。）");
        }
    }

    fn print_json(data_dir: &Path, c: &Consent, config: &Config) -> Result<()> {
        let out = serde_json::json!({
            "path": sister_core::consent::path(data_dir).display().to_string(),
            "version": c.version,
            "current": c.current(),
            "allows_recording": c.allows_recording(),
            // `allows_frames` 是同意書說「可以」，`keeps_images` 是硬碟上**真的**
            // 會多出檔案。設定檔關著 `store_images` 的時候這兩個不一樣，而拿
            // 前者去畫 UI 的人會畫錯——所以兩個都給，名字也講清楚是哪一個。
            "allows_frames": c.allows_frames(),
            "allows_cloud": c.allows_cloud(),
            "keeps_images": c.keeps_images(config),
            "config_store_images": config.capture.store_images,
            "sheets": Sheet::ALL.iter().map(|s| serde_json::json!({
                "key": s.key(),
                "granted_at": c.get(*s),
                // 「簽過」和「現在算數」是兩件事：條文改版之後前者還是 true。
                "effective": c.current() && c.get(*s).is_some(),
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_signed_second_sheet_does_not_promise_a_deidentification_that_was_removed() {
            let text = cloud_reading_signed_lines(true).join("\n");
            assert!(
                !text.contains("去識別化後"),
                "仍承諾已移除的去識別化：{text}"
            );
            assert!(text.contains("原文"), "沒有說送出去的是原文：{text}");
            assert!(
                text.contains("沒有去識別化"),
                "沒有明說原文未去識別化：{text}"
            );
        }

        /// 一支一支斷言，不要寫成迴圈掃一個陣列——那樣少一個名字的時候，
        /// 陣列和斷言會一起變小，測試照樣全綠。
        #[test]
        fn the_signed_second_sheet_names_every_command_that_spends_the_permit() {
            let text = cloud_reading_signed_lines(true).join("\n");
            assert!(text.contains("sister interpret"), "漏掉 interpret：{text}");
            assert!(text.contains("sister review"), "漏掉 review：{text}");
            assert!(text.contains("sister watch"), "漏掉 watch：{text}");
            assert!(text.contains("sister record"), "漏掉 record：{text}");
        }

        /// `record` 進清單還不夠：他看到 `sister record` 四個字，想到的是「錄影」，
        /// 不是「送字出去」。這一條釘的是那句**為什麼**——只要設了 `[brain]`，
        /// 光是錄著就會送。少了它，清單多一個名字也還是解釋不了什麼。
        #[test]
        fn the_signed_second_sheet_warns_that_recording_alone_wakes_the_brain() {
            let text = cloud_reading_signed_lines(true).join("\n");
            let line = text
                .lines()
                .find(|l| l.contains("sister record"))
                .expect("上一條測試已經保證 record 在裡面");
            assert!(
                line.contains("[brain]"),
                "沒說是哪個設定讓 record 自己送字：{line}"
            );
            assert!(
                line.contains("解釋層") && line.contains("審閱層"),
                "沒說 record 自己叫的是哪兩層：{line}"
            );
        }

        #[test]
        fn the_unsigned_arm_still_says_the_cli_is_missing() {
            assert_eq!(
                cloud_reading_signed_lines(false),
                &["已同意，但還沒設定 [brain] command，一次都不會呼叫。"]
            );
        }
    }
}

pub mod interpret {
    use super::*;
    use sister_core::brain::{self, InterpretInput, OutboundOutcome};
    use sister_core::config::Config;

    pub fn run(
        data_dir: &Path,
        config: &Config,
        dry_run: bool,
        last: &str,
        limit: usize,
        only_core_start: Option<i64>,
    ) -> Result<()> {
        let span = super::parse_span(last)?;
        let to = sister_core::now_ms();
        let from = to.saturating_sub(span);
        let mut db = open_existing(data_dir)?;

        let consent = sister_core::consent::load(data_dir);
        let mut input = InterpretInput {
            db: &mut db,
            consent: &consent,
            brain: &config.brain,
            from_ts: from,
            to_ts: to,
            limit,
            only_core_start,
        };

        if dry_run {
            let report = brain::prepare(&mut input)?;
            print!("{}", brain::format_dry_run(&report));
            return Ok(());
        }

        let result = brain::run(&mut input)?;
        if let Some(skip) = &result.skip {
            println!("{}", skip.message());
            return Ok(());
        }
        if result.ran.is_empty() {
            println!("沒有跑任何一段。");
            return Ok(());
        }
        for job in &result.ran {
            println!("── {} ──", job.segment_ref);
            println!(
                "  結局：{}（{} ms）",
                match job.outcome {
                    OutboundOutcome::Success => "成功，寫進 L2",
                    OutboundOutcome::SpawnFailed => "CLI 叫不起來／失敗",
                    OutboundOutcome::Timeout => "逾時",
                    OutboundOutcome::BadJson => "拿回的 JSON 不能用，沒寫卡片",
                },
                job.duration_ms
            );
            if let Some(err) = &job.error {
                println!("  {err}");
            }
            if let Some(card) = &job.card {
                println!(
                    "  假設（模型說的，confidence {:.2}）：{}",
                    card.model_confidence, card.activity
                );
                if !card.evidence_refs.is_empty() {
                    let refs: Vec<_> = card.evidence_refs.iter().map(|r| r.as_str()).collect();
                    println!("  根據：{}", refs.join("、"));
                }
            }
        }
        Ok(())
    }
}

pub mod brain {
    use super::*;

    fn outbound_role_label(role: &str) -> String {
        match role {
            "interpreter" => "解釋層".into(),
            "reviewer" => "審閱層".into(),
            "watcher" => "盯梢層".into(),
            other => format!("不認得的層別（{other}）"),
        }
    }

    fn outbound_segment_cell(role: &str, segment: Option<sister_core::Millis>) -> String {
        match (role, segment) {
            ("watcher", None) => "（盯梢問的是時間區間，本來就沒有段落）".into(),
            (_, Some(ts)) => ts.to_string(),
            (_, None) => "（沒有對上段落）".into(),
        }
    }

    pub fn log(data_dir: &Path, limit: usize) -> Result<()> {
        let db = open_existing(data_dir)?;
        println!(
            "{}",
            sister_core::reviewer::format_recheck_rate(&db.reviewer_recheck_stats()?)
        );
        println!();
        let outbound = db.list_brain_outbound(limit)?;
        let skips = db.list_brain_skip(limit)?;
        println!("送出去的是螢幕上的原文，沒有去識別化。這裡只記結構和計數，不留那份原文。\n");
        if outbound.is_empty() && skips.is_empty() {
            if db.ever_brain_outbound()? {
                println!("送過，但那些列已經被保留期或「忘掉」清掉了。不是從來沒送。");
            } else {
                println!("還沒送過任何東西，也還沒有降級紀錄。");
            }
            return Ok(());
        }
        if outbound.is_empty() && db.ever_brain_outbound()? {
            println!("外送紀錄本身已經被清掉了（送過，不是從來沒送）。\n");
        }
        if !outbound.is_empty() {
            println!("外送紀錄（記結構和計數，不留送出去的原文）\n");
            for row in &outbound {
                let args: Vec<String> = serde_json::from_str(&row.args_json).unwrap_or_default();
                println!(
                    "{}  {}  {} {}",
                    crate::fmt::timestamp(row.ts),
                    outbound_role_label(&row.role),
                    row.command,
                    args.join(" ")
                );
                println!(
                    "    segment={}  {} 字{}  結局 {}  {} ms",
                    outbound_segment_cell(&row.role, row.segment_core_start),
                    row.chars_sent,
                    if row.truncated { "（截斷）" } else { "" },
                    row.outcome,
                    row.duration_ms
                );
                if let Some(err) = &row.error {
                    println!("    {err}");
                }
                println!();
            }
        }
        if !skips.is_empty() {
            println!("沒送出去的原因\n");
            for row in &skips {
                println!(
                    "{}  [{}]\n    {}",
                    crate::fmt::timestamp(row.ts),
                    row.reason,
                    row.detail.replace('\n', "\n    ")
                );
                println!();
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_role_the_product_writes_has_a_chinese_label() {
            assert_eq!(outbound_role_label("interpreter"), "解釋層");
            assert_eq!(outbound_role_label("reviewer"), "審閱層");
            assert_eq!(outbound_role_label("watcher"), "盯梢層");
        }

        #[test]
        fn an_unknown_role_says_it_is_unknown_instead_of_guessing() {
            let label = outbound_role_label("future-role");
            assert!(!label.contains("解釋層"), "不認得卻猜成解釋層：{label}");
            assert!(label.contains("future-role"), "沒帶出不認得的原值：{label}");
        }

        #[test]
        fn a_watcher_row_says_it_has_no_segment_by_design() {
            let cell = outbound_segment_cell("watcher", None);
            assert!(
                cell.contains("時間區間"),
                "沒說明盯梢問的是時間區間：{cell}"
            );
            assert!(
                cell.contains("本來就沒有段落"),
                "把正常的無段落寫成異常：{cell}"
            );
        }

        #[test]
        fn an_interpreter_row_with_no_segment_still_reads_as_a_miss() {
            let cell = outbound_segment_cell("interpreter", None);
            assert!(
                cell.contains("沒有對上段落"),
                "異常的無段落沒有寫成 miss：{cell}"
            );
            assert!(
                !cell.contains("本來就沒有"),
                "把 interpreter 的 miss 寫成正常：{cell}"
            );
        }
    }
}

pub mod review {
    use super::*;
    use sister_core::config::Config;
    use sister_core::reviewer::{self, ReviewInput, ReviewKind};

    pub fn run(
        data_dir: &Path,
        config: &Config,
        dry_run: bool,
        last: &str,
        eod: bool,
        force: bool,
    ) -> Result<()> {
        let span = super::parse_span(last)?;
        let to = sister_core::now_ms();
        let from = to.saturating_sub(span);
        let mut db = open_existing(data_dir)?;
        let consent = sister_core::consent::load(data_dir);
        let mut input = ReviewInput {
            db: &mut db,
            consent: &consent,
            brain: &config.brain,
            from_ts: from,
            to_ts: to,
            kind: if eod {
                ReviewKind::Eod
            } else {
                ReviewKind::Interval
            },
            force,
            now: to,
        };
        if dry_run {
            let stats = input.db.reviewer_recheck_stats()?;
            println!("── 不會送出去（--dry-run）──\n");
            println!(
                "命令：{}",
                config
                    .brain
                    .cli()
                    .map(|(c, a)| format!("{c} {}", a.join(" ")))
                    .unwrap_or_else(|| "（還沒設定 [brain] command）".into())
            );
            println!(
                "同意書 2：{}",
                if consent.cloud_permit().is_some() {
                    "已簽"
                } else {
                    "沒簽——真的跑的話一次都不會呼叫"
                }
            );
            let used = {
                let day = sister_core::brain::local_day_key(to).unwrap_or_default();
                input.db.brain_outbound_count_on_role(&day, "reviewer")?
            };
            println!(
                "今日審閱預算：{}/{}，還剩 {} 次",
                used,
                config.brain.reviewer_daily_budget,
                config.brain.reviewer_daily_budget.saturating_sub(used)
            );
            println!(
                "節奏：{}",
                if eod {
                    "日終盤點"
                } else {
                    "活躍批次（最短 15 分鐘）"
                }
            );
            println!();
            println!("{}", reviewer::format_recheck_rate(&stats));
            // 分歧也要印在**這條路**上。SPEC §6「分歧 = 警報」，而一個只有真的
            // 跑一輪（＝花掉雲端預算、開兩個 CLI）才看得到的警報，等於要他付錢
            // 才讀得到上一次的警報。`--dry-run` 是這支命令唯一唯讀的入口，
            // 而它要看的剛好就是**上一輪**留下來的東西。
            print!(
                "{}",
                reviewer::format_reviewer_visibility(
                    &input.db.latest_dual_pass_divergences()?,
                    &input.db.latest_reviewer_refusals()?,
                    &input.db.entity_memory()?,
                )
            );
            return Ok(());
        }
        let result = reviewer::run(&mut input)?;
        let stats = input.db.reviewer_recheck_stats()?;
        print!("{}", reviewer::format_review_result(&result, &stats));
        let divergences = input.db.latest_dual_pass_divergences()?;
        let refusals = input.db.latest_reviewer_refusals()?;
        let entities = input.db.entity_memory()?;
        print!(
            "{}",
            reviewer::format_reviewer_visibility(&divergences, &refusals, &entities)
        );
        Ok(())
    }
}

pub mod act {
    use super::*;
    use sister_hands::commitment_action::AllowedNextStep;
    use sister_hands::semi_action::{
        AbortActor, ActionKind, AllowedActions, AllowedApps, App, Expiry, Grant, GrantRejection,
        PresentedStep, RunConclusion, RunConclusionRecord, SemiActionRun, StepLimit, StepRequest,
        Task, execute_approved_step,
    };
    use sister_hands::{ActionEvent, ActionLog, ExecutionResult, Outcome, RefusalReason};
    use std::collections::BTreeSet;
    use std::io::{BufRead, Write};

    pub struct Options {
        pub task: String,
        pub apps: Vec<String>,
        pub allow: Vec<String>,
        pub minutes: u64,
        pub steps: u32,
        pub dry_run: bool,
    }

    /// 這一步宣告它會碰哪一個 app。**三種，不是兩種。**
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum StepApp {
        /// 證據鏈上剛好一個 app。
        Known(String),
        /// 證據鏈上有兩個以上不同的 app：兩個答案就是沒有答案。
        Ambiguous,
        /// 證據都問不出 app（沒有 app_id、或證據不見了）。
        Unknown,
    }

    impl StepApp {
        fn request_app(&self) -> App {
            App::new(match self {
                Self::Known(app) => app.as_str(),
                Self::Ambiguous => "<兩個以上的 app>",
                Self::Unknown => "<問不出是哪個 app>",
            })
        }
        fn label(&self) -> &str {
            match self {
                Self::Known(app) => app,
                Self::Ambiguous => "兩個以上的 app",
                Self::Unknown => "問不出是哪個 app",
            }
        }
        fn explanation(&self) -> Option<&'static str> {
            match self {
                Self::Known(_) => None,
                Self::Ambiguous => Some("證據鏈記得兩個以上不同的 app，不能替這一步選一個。"),
                Self::Unknown => Some("證據鏈沒有一個問得出的 app，不能證明這一步在授權範圍內。"),
            }
        }
    }

    #[derive(Default)]
    struct Tally {
        asked: u32,
        done: u32,
        declined: u32,
        blocked: u32,
        failed: u32,
    }

    /// `sister do` 只跟資料庫要兩件事。
    ///
    /// **抽成 trait 是為了讓下面那一整段逐步核准跑得起來測試，不是為了抽象。**
    /// `Db::insert_commitment` 要一張 `sister_core::reviewer::L3Write`，而那張票
    /// **故意**只有 sister-core 自己鑄得出來（見 `reviewer.rs` 開頭那段：
    /// 「不是 `bool`，也不是 `struct L3Write { allowed: bool }`」）。
    /// 所以 CLI 這一側**種不出承諾卡**——沒有這道縫的話，這支指令會跟
    /// `apps/desktop` 那半邊一樣變成零執行覆蓋的接線層。
    ///
    /// 假的 source 不會走到 `app_for_evidence` 真正那段 SQL；那一段由
    /// `sister-core` 自己的測試蓋（`db.rs` 裡的 `app_for_evidence` 三態測試）。
    pub(crate) trait StepSource {
        fn live_commitments(&self) -> Result<Vec<sister_core::db::CommitmentRow>>;
        fn app_for_evidence(&self, r: &sister_core::brain::EvidenceRef) -> Result<Option<String>>;
    }

    impl StepSource for Db {
        fn live_commitments(&self) -> Result<Vec<sister_core::db::CommitmentRow>> {
            Db::live_commitments(self)
        }
        fn app_for_evidence(&self, r: &sister_core::brain::EvidenceRef) -> Result<Option<String>> {
            Db::app_for_evidence(self, r)
        }
    }

    pub fn run(data_dir: &Path, opts: &Options) -> Result<()> {
        let stdin = std::io::stdin();
        run_with(
            data_dir,
            opts,
            &mut stdin.lock(),
            &mut sister_hands::platform::PlatformExecutor,
            &mut sister_core::now_ms,
        )
    }

    pub fn run_with(
        data_dir: &Path,
        opts: &Options,
        input: &mut impl BufRead,
        executor: &mut impl sister_hands::Executor,
        clock: &mut impl FnMut() -> i64,
    ) -> Result<()> {
        let db = open_existing(data_dir)?;
        run_with_output(
            data_dir,
            opts,
            &db,
            input,
            executor,
            clock,
            &mut std::io::stdout(),
        )
    }

    fn parse_kinds(raw: &[String]) -> Result<Vec<ActionKind>> {
        let values: &[String] = if raw.is_empty() {
            static DEFAULT: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
            DEFAULT.get_or_init(|| vec!["open-url".into()])
        } else {
            raw
        };
        values
            .iter()
            .map(|value| match value.as_str() {
                "open-url" => Ok(ActionKind::OpenUrl),
                "open-file" => Ok(ActionKind::OpenFile),
                "focus-window" => Ok(ActionKind::FocusWindow),
                _ => anyhow::bail!(
                    "不認得 --allow {value:?}；只接受 open-url / open-file / focus-window"
                ),
            })
            .collect()
    }

    fn step_app(source: &impl StepSource, evidence_json: &str) -> Result<StepApp> {
        let refs: Vec<String> = serde_json::from_str(evidence_json)
            .with_context(|| format!("證據清單不是 JSON 字串陣列：{evidence_json}"))?;
        let mut apps = BTreeSet::new();
        for raw in refs {
            let Some(reference) = sister_core::brain::EvidenceRef::parse(&raw) else {
                continue;
            };
            if let Some(app) = source.app_for_evidence(&reference)? {
                apps.insert(app);
            }
        }
        Ok(match apps.len() {
            0 => StepApp::Unknown,
            1 => StepApp::Known(apps.into_iter().next().expect("len checked")),
            _ => StepApp::Ambiguous,
        })
    }

    fn coverage(grant: &Grant, step: &StepRequest, app: &StepApp, now: i64) -> String {
        match grant.covers(step, now) {
            Ok(()) => "授權涵蓋這一步".into(),
            Err(rejection) => {
                let mut text = format!("授權不涵蓋：{}", rejection.message());
                if rejection == GrantRejection::Apps
                    && let Some(extra) = app.explanation()
                {
                    text.push_str(&format!(" {extra}"));
                }
                text
            }
        }
    }

    enum Answer {
        Approve,
        Decline,
        Abort(AbortActor),
    }

    fn ask(input: &mut impl BufRead, out: &mut impl Write) -> Result<Answer> {
        loop {
            write!(out, "要做嗎？好／不要／停：")?;
            out.flush()?;
            let mut answer = String::new();
            if input.read_line(&mut answer)? == 0 {
                writeln!(out, "沒有收到回答；系統中止這一輪。")?;
                return Ok(Answer::Abort(AbortActor::System));
            }
            match answer.trim().to_lowercase().as_str() {
                "好" | "y" | "yes" | "是" => return Ok(Answer::Approve),
                "不要" | "n" | "no" | "跳過" => return Ok(Answer::Decline),
                "停" | "中止" | "q" | "quit" => return Ok(Answer::Abort(AbortActor::User)),
                _ => writeln!(out, "聽不懂；好／不要／停")?,
            }
        }
    }

    pub(crate) fn run_with_output(
        data_dir: &Path,
        opts: &Options,
        source: &impl StepSource,
        input: &mut impl BufRead,
        executor: &mut impl sister_hands::Executor,
        clock: &mut impl FnMut() -> i64,
        out: &mut impl Write,
    ) -> Result<()> {
        let step_limit =
            StepLimit::new(opts.steps).context("步數上限不能是 0；請把 --steps 設為至少 1")?;
        let kinds = parse_kinds(&opts.allow)?;
        let valid_for_ms = opts
            .minutes
            .checked_mul(60_000)
            .context("--minutes 太大，無法換算授權期限")?;
        let issued_at = clock();
        let grant = Grant::new(
            Task::new(&opts.task),
            AllowedApps::new(opts.apps.iter().cloned().map(App::new)),
            AllowedActions::new(kinds),
            Expiry::after_issued(issued_at, valid_for_ms),
            step_limit,
        );
        let mut run = SemiActionRun::new(grant.clone());
        let commitments = source.live_commitments()?;
        let log = ActionLog::in_data_dir(data_dir);
        let mut tally = Tally::default();
        let mut terminal: Option<RunConclusion> = None;

        // **這一輪的第一列是那張授權書。** 少了它，紀錄裡只有「她做了什麼」，
        // 而「他准了什麼」是唯一沒被記下來的那一半；順帶它也是一輪的界線——
        // 沒有它的話兩次 `sister do` 的步驟在檔案裡直接接在一起。
        //
        // 預演不寫：`--dry-run` 什麼都不會發生，寫一列「已授權」進去就等於在
        // 紀錄上留下一輪從來沒有跑過的授權。
        if !opts.dry_run {
            log.append(&ActionEvent::Granted {
                at_ms: clock(),
                grant,
            })?;
        }

        // 一張都沒端出來的時候，**為什麼**有三種答案，而它們的下一步各自不同。
        // 只數 `presented == 0` 會讓三種都印成同一句「這一輪的步驟都問完了」。
        let live = commitments.len();
        let mut without_next_step = 0_usize;
        let mut unparseable = 0_usize;
        let mut presented = 0_usize;

        for commitment in commitments {
            let button = match sister_hands::commitment_action::parse_allowed_next_step(
                commitment.allowed_next_step.as_deref(),
            ) {
                AllowedNextStep::Missing => {
                    without_next_step += 1;
                    continue;
                }
                AllowedNextStep::Unparseable { reason, .. } => {
                    unparseable += 1;
                    writeln!(out, "#{} 的下一步讀不懂，跳過：{reason}", commitment.id)?;
                    continue;
                }
                AllowedNextStep::Suggestion(button) => button,
            };
            presented += 1;
            if let Err(conclusion) = run.may_start_step() {
                terminal = Some(conclusion);
                break;
            }
            let declared_app = step_app(source, &commitment.evidence_json)?;
            let action = button.snapshot();
            let step = StepRequest::new(
                // 這支 CLI 的 request 和 grant 都直接取自同一個 `--task`，所以 Task
                // 在這個呼叫端不可能拒絕。那一維要等授權書能跨行程重用才守得到。
                Task::new(&opts.task),
                declared_app.request_app(),
                action.clone(),
            );
            // 印出來的那句判斷講的是「她提出這一步的那一刻」——她只能拿她問你的
            // 時候手上有的東西來判斷。**執行的那一刻要另外問一次時間**，見下面
            // `execute_approved_step` 那裡。
            let step_now = clock();
            let covered = coverage(run.grant(), &step, &declared_app, step_now);
            writeln!(out, "承諾 #{}：{}", commitment.id, commitment.text)?;
            writeln!(out, "步驟：{}", action.describe())?;
            writeln!(out, "宣告 app：{}", declared_app.label())?;
            writeln!(out, "{covered}")?;
            if opts.dry_run {
                writeln!(out)?;
                continue;
            }

            tally.asked += 1;
            log.append(&ActionEvent::Proposed {
                at_ms: clock(),
                action: action.clone(),
            })?;
            let presented = PresentedStep::new(step.clone());
            match ask(input, out)? {
                Answer::Decline => {
                    tally.declined += 1;
                    log.append(&ActionEvent::Refused {
                        at_ms: clock(),
                        action,
                        reason: RefusalReason::UserDeclinedThisStep,
                    })?;
                }
                Answer::Abort(by) => {
                    let event = run.abort(clock(), by);
                    log.append(&event)?;
                    terminal = Some(match event {
                        ActionEvent::Aborted {
                            after_completed_steps,
                            by,
                            ..
                        } => RunConclusion::Aborted {
                            after_completed_steps,
                            by,
                        },
                        _ => unreachable!("abort always returns Aborted"),
                    });
                    break;
                }
                Answer::Approve => {
                    let approval = presented.approve();
                    let suggestion = button.press();
                    log.append(&ActionEvent::Approved {
                        at_ms: clock(),
                        action: action.clone(),
                    })?;
                    // **不是 `step_now`。** `--minutes` 那一維說的是「這張授權書
                    // 多久後失效」，而失效要管得住的是**她真的動手的那一刻**，
                    // 不是她開口問的那一刻。拿問話時的時戳來執行的話，一個開著
                    // 沒人答的提問可以放到三小時後，他順手打一個「好」，動作照樣
                    // 發生——而畫面上和 `--minutes 5` 都跟他說只有五分鐘。
                    //
                    // 兩個時戳因此**故意**不一樣，而它們不一樣的時候，畫面上那句
                    // 「授權涵蓋這一步」不會變成假話：它講的是問你的那一刻，是真的。
                    // 改變的事實由這裡吐出一列 `refused` 說出來（`expiry 維度拒絕`），
                    // 統計那一行也會把它算進「授權擋掉」。
                    let outcome = execute_approved_step(
                        run.grant(),
                        clock(),
                        approval,
                        &step,
                        executor,
                        &suggestion,
                    );
                    // **他說了「好」之後，螢幕上一定要有一句話。** 三種結果都寫進
                    // log 了，但 log 在磁碟上；他看著的是這個終端機。少了這一段，
                    // 「擋掉了」和「做好了」在他眼前長得一模一樣——一片空白，而
                    // 空白讀起來是「成功了」。這正是這個 repo 一路在修的那件事。
                    //
                    // 三句話要分得開的是**同一件事的三個不同答案**：
                    // 沒交出去（Refused）／交出去了但那一端失敗（Failed）／成了。
                    let event = match &outcome {
                        Outcome::Refused { reason } => {
                            tally.blocked += 1;
                            writeln!(out, "沒有做，也沒有交給作業系統：{}", reason.message())?;
                            ActionEvent::Refused {
                                at_ms: clock(),
                                action: action.clone(),
                                reason: reason.clone(),
                            }
                        }
                        Outcome::Failed { error } => {
                            tally.failed += 1;
                            writeln!(out, "交出去了，那一端失敗了：{error}")?;
                            ActionEvent::Executed {
                                at_ms: clock(),
                                action: action.clone(),
                                result: ExecutionResult::Failed {
                                    error: error.clone(),
                                },
                            }
                        }
                        Outcome::Done { detail } => {
                            tally.done += 1;
                            writeln!(out, "做了：{detail}")?;
                            ActionEvent::Executed {
                                at_ms: clock(),
                                action: action.clone(),
                                result: ExecutionResult::Succeeded {
                                    detail: detail.clone(),
                                },
                            }
                        }
                    };
                    log.append(&event)?;
                    if matches!(outcome, Outcome::Done { .. }) {
                        // `StepLimitReached` 的欄位叫 `completed_steps`：一次交出去但失敗
                        // 的嘗試不是完成的步驟，因此 Failed 不消耗這版的步數預算。
                        let finished = run
                            .finish_step(clock(), action, None)
                            .map_err(|conclusion| anyhow::anyhow!(conclusion.message()))?;
                        log.append(&finished)?;
                    }
                }
            }
            writeln!(out)?;
        }

        if presented == 0 {
            writeln!(
                out,
                "{}",
                nothing_to_offer(live, without_next_step, unparseable)
            )?;
        }

        if opts.dry_run {
            // 用 `presented`，不要再養一個只在預演路徑上加一的第二個計數器。
            // 兩個變數答同一個問題，遲早有一天只有一個被改到。
            if presented > 0 {
                writeln!(
                    out,
                    "步數上限 {}：真的跑起來的時候，做成 {} 步之後就停，後面的步驟不會被問到。",
                    opts.steps, opts.steps
                )?;
            }
            return Ok(());
        }
        let conclusion = terminal.unwrap_or(RunConclusion::Completed);
        writeln!(out, "{}", conclusion.message())?;
        match conclusion {
            RunConclusion::Aborted { .. } => {}
            RunConclusion::StepLimitReached {
                completed_steps,
                limit,
            } => {
                log.append(&ActionEvent::Concluded {
                    at_ms: clock(),
                    conclusion: RunConclusionRecord::StepLimitReached {
                        completed_steps,
                        limit: limit.get(),
                    },
                })?;
            }
            RunConclusion::Completed => log.append(&ActionEvent::Concluded {
                at_ms: clock(),
                // 這個數字就是底下那一行印給他看的 `tally.asked`，同一個變數。
                // 抄一份「大概是幾」進紀錄的話，兩邊會在某一版分家。
                conclusion: RunConclusionRecord::Completed {
                    asked: Some(tally.asked),
                },
            })?,
        }
        writeln!(
            out,
            "這一輪：問了 {} 步，做成 {} 步，你說不要 {} 步，授權擋掉 {} 步，執行失敗 {} 步。",
            tally.asked, tally.done, tally.declined, tally.blocked, tally.failed
        )?;
        Ok(())
    }

    /// 一張都端不出來的時候，**為什麼**。
    ///
    /// 這四種處境在 alpha.70 剛做出來的時候印的是同一句話——
    /// 「這一輪的步驟都問完了。問了 0 步。」而那句話的意思是「沒有東西再問了」，
    /// 讀起來像「都處理完了」。四種的下一步完全不同：一種要去錄東西，一種要
    /// 等 L3 挑得出可以打開的東西，一種是資料壞了，一種是他自己該去看承諾表。
    fn nothing_to_offer(live: usize, without_next_step: usize, unparseable: usize) -> String {
        if live == 0 {
            return "承諾表上一張活著的卡都沒有，所以沒有東西可以問你。\
                    先讓她整理一次：`sister review --force`（要簽過第二張同意書）。"
                .to_string();
        }
        if unparseable > 0 && without_next_step + unparseable == live {
            return format!(
                "{live} 張活著的承諾裡，{unparseable} 張的下一步讀不懂（理由在上面），\
                 其餘 {without_next_step} 張沒有下一步。這一輪沒有東西可以問你。"
            );
        }
        if without_next_step == live {
            return format!(
                "有 {live} 張活著的承諾，但**沒有一張帶著下一步**——L3 在那天的事實裡\
                 挑不到可以打開的網址或檔案，就會把那一欄留空。這不是她做完了。"
            );
        }
        // 走到這裡代表有卡帶著下一步、卻一步都沒被端出來：今天唯一的路是步數
        // 上限在第一步之前就用完了，而那不可能（`StepLimit` 保證 ≥ 1）。留一句
        // 說得出自己不知道的話，比留一句聽起來很篤定的假話好。
        format!("{live} 張活著的承諾，一步都沒有端出來，而我說不出為什麼。這是個 bug，請回報。")
    }

    /// `sister hands log`：把 `action-log.jsonl` 讀成人話。
    pub fn log(data_dir: &Path, limit: usize) -> Result<()> {
        log_to(data_dir, limit, &mut std::io::stdout())
    }

    pub(crate) fn log_to(data_dir: &Path, limit: usize, out: &mut impl Write) -> Result<()> {
        // **這一句擋的是一句假話。** 資料目錄打錯字的時候，下面那個 replay 會
        // 回一份空的（`ActionLog::replay` 把「檔案不存在」當成空的，那對它自己
        // 那一層是對的），於是螢幕上會寫「她還沒有動過手」——而真相是我們根本
        // 沒看那個目錄。「查不到」不可以講成「沒有」。
        anyhow::ensure!(
            data_dir.exists(),
            "找不到這個資料目錄：{}\n這不是「她沒有動過手」，是我們沒有看到那個目錄。",
            data_dir.display()
        );
        let log = ActionLog::in_data_dir(data_dir);
        let replay = log.replay()?;
        let lines = sister_hands::replay_copy::recent_replay_lines(&replay, limit);
        if lines.is_empty() {
            // 上面那句 `ensure!` 擋掉了「目錄查不到」被講成「沒有」，然後**同一個
            // 錯又在低一層原封不動地再犯一次**：零列有兩種，而它們印同一句話。
            //
            // 檔案不存在＝她真的從來沒有把一個動作端到你面前。
            // 檔案在、但是空的＝她端過，那些列被 `sister forget` 刪掉了
            // （`ActionLog::forget_range` 保留的列全部落在範圍內的時候，寫出去的
            // 就是一個 0 位元組的檔案再 rename 蓋上去）。對後者說「她從來沒有」，
            // 是拿一句沒查過的歷史宣告去蓋掉一次真的刪除。
            if log.path().exists() {
                writeln!(
                    out,
                    "這份紀錄是空的——**不是**「她從來沒動過手」，是裡面的列被刪光了。"
                )?;
                writeln!(
                    out,
                    "會把列刪掉的只有 `sister forget`（和字母人上的「忘掉這一整天」）。"
                )?;
            } else {
                writeln!(
                    out,
                    "還沒有任何動作紀錄。她從來沒有把一個動作端到你面前過。"
                )?;
            }
            writeln!(out, "（紀錄會寫在 {}）", log.path().display())?;
            return Ok(());
        }
        writeln!(out, "動作紀錄（{}）\n", log.path().display())?;
        for line in &lines {
            writeln!(out, "{line}")?;
        }
        // **這一句不是裝飾。** `unreadable` 的那幾列在上面是有印出來的，但它們
        // 混在一整串裡很容易被讀成「她做過的事情之一」。壞掉的列代表**那一段
        // 發生過但我們解不開**，和「她沒做」是兩件事。
        if !replay.unreadable.is_empty() {
            writeln!(
                out,
                "\n有 {} 列讀不懂。那幾列是發生過但解不開，不是沒有發生。",
                replay.unreadable.len()
            )?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        use sister_core::db::CommitmentRow;
        use sister_hands::{ActionSnapshot, Replay};

        /// 一張假的承諾卡來源。理由見 [`StepSource`] 的註解。
        #[derive(Default)]
        struct Source {
            rows: Vec<CommitmentRow>,
            /// `frame:<id>` → 那個 frame 上問得出來的 app。缺席＝問不出來。
            apps: std::collections::BTreeMap<i64, String>,
        }

        impl StepSource for Source {
            fn live_commitments(&self) -> Result<Vec<CommitmentRow>> {
                Ok(self.rows.clone())
            }
            fn app_for_evidence(
                &self,
                r: &sister_core::brain::EvidenceRef,
            ) -> Result<Option<String>> {
                Ok(match r {
                    sister_core::brain::EvidenceRef::Frame(id) => self.apps.get(id).cloned(),
                    sister_core::brain::EvidenceRef::Fact(_) => None,
                })
            }
        }

        /// 一張活著的承諾卡，帶著它的下一步和它的證據。
        fn card(id: i64, next: Option<&str>, frames: &[i64]) -> CommitmentRow {
            let refs: Vec<String> = frames.iter().map(|f| format!("frame:{f}")).collect();
            CommitmentRow {
                id,
                text: format!("承諾 {id}"),
                kind: "promise".into(),
                born_from: 1,
                evidence_json: serde_json::to_string(&refs).expect("evidence json"),
                people_json: "[]".into(),
                due_hint: None,
                due_source: None,
                due_at: None,
                status: "open".into(),
                confidence: 0.8,
                allowed_next_step: next.map(str::to_owned),
                last_evidence_seen_at: None,
                kill_note: None,
                created_at: 1,
                updated_at: 1,
                tombstoned_at: None,
            }
        }

        fn open_url(url: &str) -> String {
            serde_json::json!({"action": "open_url", "url": url}).to_string()
        }

        #[derive(Default)]
        struct Fake {
            calls: Vec<ActionSnapshot>,
            fail: Option<String>,
        }

        impl sister_hands::Executor for Fake {
            fn execute(&mut self, suggestion: &sister_hands::Suggestion) -> Result<String, String> {
                self.calls.push(suggestion.snapshot());
                match &self.fail {
                    Some(error) => Err(error.clone()),
                    None => Ok("假的執行器接受了".into()),
                }
            }
        }

        fn opts(task: &str, apps: &[&str], steps: u32, minutes: u64, dry_run: bool) -> Options {
            Options {
                task: task.into(),
                apps: apps.iter().map(|a| (*a).to_owned()).collect(),
                allow: vec!["open-url".into()],
                minutes,
                steps,
                dry_run,
            }
        }

        /// 每叫一次就往前 1 秒。**時間必須會走**，否則 grant 的 expiry 那一維
        /// 恆為 0 毫秒、永遠不可能拒絕，而 log 上一整輪會擠在同一毫秒。
        fn ticking(start: i64) -> impl FnMut() -> i64 {
            let mut t = start;
            move || {
                t += 1_000;
                t
            }
        }

        struct Run {
            out: String,
            executor: Fake,
            dir: crate::ops::tmp::Tmp,
        }

        impl Run {
            fn events(&self) -> Vec<ActionEvent> {
                self.replay().events
            }
            fn replay(&self) -> Replay {
                ActionLog::in_data_dir(&self.dir.0)
                    .replay()
                    .expect("replay the action log")
            }
            fn log_exists(&self) -> bool {
                ActionLog::in_data_dir(&self.dir.0).path().exists()
            }
        }

        fn go(name: &str, source: &Source, opts: &Options, typed: &str, fail: Option<&str>) -> Run {
            go_from(name, source, opts, typed, fail, ticking(1_700_000_000_000))
        }

        fn go_from(
            name: &str,
            source: &Source,
            opts: &Options,
            typed: &str,
            fail: Option<&str>,
            mut clock: impl FnMut() -> i64,
        ) -> Run {
            let dir = crate::ops::tmp::Tmp::new(name);
            let mut executor = Fake {
                fail: fail.map(str::to_owned),
                ..Fake::default()
            };
            let mut input = std::io::Cursor::new(typed.as_bytes().to_vec());
            let mut out = Vec::new();
            run_with_output(
                &dir.0,
                opts,
                source,
                &mut input,
                &mut executor,
                &mut clock,
                &mut out,
            )
            .expect("run");
            Run {
                out: String::from_utf8(out).expect("utf-8"),
                executor,
                dir,
            }
        }

        fn one_card() -> Source {
            Source {
                rows: vec![card(7, Some(&open_url("https://example.com/a")), &[1])],
                apps: [(1, "chrome.exe".to_string())].into_iter().collect(),
            }
        }

        #[test]
        fn saying_yes_hands_it_to_the_executor_and_leaves_the_whole_story_in_the_log() {
            let run = go(
                "act-yes",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n",
                None,
            );
            assert_eq!(
                run.executor.calls,
                vec![ActionSnapshot::OpenUrl {
                    url: "https://example.com/a".into()
                }]
            );
            // 成了這件事也要當著他的面說一次，而且要帶著執行那一端回報的話——
            // 三種結果各有各的句子，才分得出「做了」「擋了」「試了但失敗」。
            assert!(run.out.contains("做了：假的執行器接受了"), "{}", run.out);
            let kinds: Vec<&str> = run
                .events()
                .iter()
                .map(|e| match e {
                    ActionEvent::Granted { .. } => "granted",
                    ActionEvent::Proposed { .. } => "proposed",
                    ActionEvent::Approved { .. } => "approved",
                    ActionEvent::Executed { .. } => "executed",
                    ActionEvent::Refused { .. } => "refused",
                    ActionEvent::StepFinished { .. } => "finished",
                    ActionEvent::Aborted { .. } => "aborted",
                    ActionEvent::Concluded { .. } => "concluded",
                })
                .collect();
            assert_eq!(
                kinds,
                vec![
                    "granted",
                    "proposed",
                    "approved",
                    "executed",
                    "finished",
                    "concluded"
                ],
                "{}",
                run.out
            );
            // 授權那一列要**排在第一個**。排在後面的話，讀的人會看到一步在
            // 一張還沒有發出來的票底下發生。
            let ActionEvent::Granted { grant, .. } = &run.events()[0] else {
                panic!("第一列要是授權：{:?}", run.events());
            };
            let described = grant.describe();
            assert!(described.contains("開今天的連結"), "{described}");
            assert!(described.contains("chrome.exe"), "{described}");
            assert!(described.contains("open-url"), "{described}");
            assert!(described.contains("最多 3 步"), "{described}");
        }

        /// **這是 A1 的驗收。** 一輪互動花掉的時間如果全部蓋同一個 `at_ms`，
        /// 回放的人會看到一次零秒的思考——而那個零是我們沒有量，不是他沒有猶豫。
        #[test]
        fn two_rows_in_one_run_do_not_claim_the_same_millisecond() {
            let run = go(
                "act-clock",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n",
                None,
            );
            let stamps: Vec<i64> = run.events().iter().map(|e| e.at_ms()).collect();
            assert!(stamps.len() >= 2, "{stamps:?}");
            assert!(
                stamps.windows(2).all(|w| w[0] < w[1]),
                "每一列都該有自己的時刻：{stamps:?}"
            );
        }

        /// 「他說不要」和「這一步根本沒被提出」的差別就是那一列。
        #[test]
        fn saying_no_leaves_a_row_that_says_you_said_no_instead_of_a_silence() {
            let run = go(
                "act-no",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "不要\n",
                None,
            );
            assert!(run.executor.calls.is_empty(), "不該交給執行器");
            let declined = run.events().iter().any(|e| {
                matches!(
                    e,
                    ActionEvent::Refused {
                        reason: RefusalReason::UserDeclinedThisStep,
                        ..
                    }
                )
            });
            assert!(
                declined,
                "action log 要留下「你說不要」那一列：{:?}",
                run.events()
            );
        }

        /// `AbortActor` 這個型別存在的全部理由，就是把這兩件事分開。
        #[test]
        fn the_log_says_who_stopped_it_and_nobody_answering_is_not_him_saying_stop() {
            let by = |typed: &str, name: &str| {
                let run = go(
                    name,
                    &one_card(),
                    &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                    typed,
                    None,
                );
                run.events()
                    .iter()
                    .find_map(|e| match e {
                        ActionEvent::Aborted { by, .. } => Some(*by),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("{name}：沒有中止那一列"))
            };
            let typed_stop = by("停\n", "act-stop");
            let nobody_home = by("", "act-eof");
            assert_eq!(typed_stop, AbortActor::User);
            assert_eq!(nobody_home, AbortActor::System);
            assert_ne!(typed_stop, nobody_home);
        }

        /// 停在上限不代表任務完成——`RunConclusion` 自己的文案就是這樣寫的。
        #[test]
        fn hitting_the_step_limit_is_not_the_same_as_finishing() {
            let source = Source {
                rows: vec![
                    card(7, Some(&open_url("https://example.com/a")), &[1]),
                    card(8, Some(&open_url("https://example.com/b")), &[1]),
                ],
                apps: [(1, "chrome.exe".to_string())].into_iter().collect(),
            };
            let run = go(
                "act-limit",
                &source,
                &opts("開今天的連結", &["chrome.exe"], 1, 5, false),
                "好\n好\n",
                None,
            );
            assert!(run.out.contains("不代表任務完成"), "{}", run.out);
            assert!(!run.out.contains("都問完了"), "{}", run.out);
            assert_eq!(run.executor.calls.len(), 1, "上限 1 步，只該交出去一次");
            let last = run.events().last().cloned().expect("有事件");
            assert_eq!(
                last,
                ActionEvent::Concluded {
                    at_ms: match last {
                        ActionEvent::Concluded { at_ms, .. } => at_ms,
                        ref other => panic!("最後一列該是 Concluded：{other:?}"),
                    },
                    conclusion: RunConclusionRecord::StepLimitReached {
                        completed_steps: 1,
                        limit: 1,
                    },
                }
            );
        }

        #[test]
        fn an_app_you_did_not_authorise_never_reaches_the_executor() {
            let run = go(
                "act-app",
                &one_card(),
                &opts("開今天的連結", &["notepad.exe"], 3, 5, false),
                "好\n",
                None,
            );
            assert!(run.executor.calls.is_empty(), "沒授權的 app 不該交出去");
            let blocked = run.events().iter().any(|e| {
                matches!(
                    e,
                    ActionEvent::Refused {
                        reason: RefusalReason::NotCoveredByGrant {
                            rejection: GrantRejection::Apps
                        },
                        ..
                    }
                )
            });
            assert!(blocked, "{:?}", run.events());
            assert!(run.out.contains("授權擋掉 1 步"), "{}", run.out);
            // **畫面上那句話也要是真的。** 只斷言「預覽和真的跑一致」證不出
            // 這件事——一個永遠回「授權涵蓋這一步」的 `coverage()` 兩邊一樣假，
            // 那條一致性測試照樣綠。所以這裡把印出來的判斷釘在真的發生的事上。
            assert!(
                run.out.contains("授權不涵蓋：apps 維度拒絕"),
                "擋下來了就要說擋下來：{}",
                run.out
            );
            assert!(!run.out.contains("授權涵蓋這一步"), "{}", run.out);
        }

        /// 「她記得是兩個 app」和「她根本不記得」是他要看到的兩件不同的事。
        #[test]
        fn not_knowing_which_app_and_knowing_two_are_not_the_same_sentence() {
            let source = Source {
                rows: vec![
                    card(7, Some(&open_url("https://example.com/a")), &[1, 2]),
                    card(8, Some(&open_url("https://example.com/b")), &[3]),
                ],
                apps: [(1, "chrome.exe".to_string()), (2, "slack.exe".to_string())]
                    .into_iter()
                    .collect(),
            };
            let run = go(
                "act-app-three",
                &source,
                &opts("開今天的連結", &["chrome.exe"], 3, 5, true),
                "",
                None,
            );
            assert!(run.out.contains("兩個以上的 app"), "{}", run.out);
            assert!(run.out.contains("問不出是哪個 app"), "{}", run.out);
        }

        /// **這是 A1 的第二個驗收。** 一整輪共用同一個 `now` 的話，`elapsed`
        /// 恆為 0，`--minutes` 就是一個印在 `--help` 上但不做事的旗標。
        #[test]
        fn an_expired_grant_refuses_even_after_he_says_yes() {
            let source = Source {
                rows: vec![
                    card(7, Some(&open_url("https://example.com/a")), &[1]),
                    card(8, Some(&open_url("https://example.com/b")), &[1]),
                ],
                apps: [(1, "chrome.exe".to_string())].into_iter().collect(),
            };
            // 第一步在授權還新的時候；**做完第一步之後**，時鐘跳過兩分鐘，
            // 於是第二步連問都不該被問成「涵蓋」。
            // 呼叫順序：1 發證、2 寫授權那一列、3 判斷、4 proposed、5 approved、
            // 6 執行、7 結果、8 step_finished。跳在 8。
            let mut calls = 0;
            let mut t = 1_700_000_000_000;
            let clock = move || {
                calls += 1;
                t += if calls == 8 { 120_000 } else { 1_000 };
                t
            };
            let run = go_from(
                "act-expiry",
                &source,
                &opts("開今天的連結", &["chrome.exe"], 3, 1, false),
                "好\n好\n",
                None,
                clock,
            );
            assert_eq!(
                run.executor.calls.len(),
                1,
                "第二步應該在過期那一維就被擋下來：{}",
                run.out
            );
            let expired = run.events().iter().any(|e| {
                matches!(
                    e,
                    ActionEvent::Refused {
                        reason: RefusalReason::NotCoveredByGrant {
                            rejection: GrantRejection::ExpiryElapsed
                        },
                        ..
                    }
                )
            });
            assert!(expired, "{:?}", run.events());
        }

        /// 他去泡了杯茶，回來才打「好」——而那杯茶泡得比 `--minutes` 還久。
        ///
        /// **這一條和上面那條不是同一件事。** 上面那條的授權是在**她開口之前**
        /// 就過期了，所以畫面上會直接寫「授權不涵蓋」。這一條是她問的時候還沒
        /// 過期（畫面上寫「授權涵蓋這一步」，那句話是真的），過期發生在他答話
        /// 的那段空白裡。拿問話那一刻的時戳去執行的話，`--minutes 1` 攔不住一個
        /// 開著兩小時沒人答的提問——而那正是 `--minutes` 唯一存在的理由。
        #[test]
        fn a_yes_typed_long_after_the_ticket_expired_does_not_reach_the_executor() {
            let source = one_card();
            // 呼叫順序：1 發證、2 寫授權那一列、3 印判斷用的、4 proposed、
            // 5 approved、6 執行。茶泡在第 5 和第 6 中間——也就是他盯著那個
            // 問句的那段時間。
            let mut calls = 0;
            let mut t = 1_700_000_000_000;
            let clock = move || {
                calls += 1;
                t += if calls == 6 { 7_200_000 } else { 1_000 };
                t
            };
            let run = go_from(
                "act-slow-yes",
                &source,
                &opts("開今天的連結", &["chrome.exe"], 3, 1, false),
                "好\n",
                None,
                clock,
            );
            assert!(
                run.out.contains("授權涵蓋這一步"),
                "她問的那一刻票還是好的，畫面上不可以先說謊：{}",
                run.out
            );
            assert!(
                run.executor.calls.is_empty(),
                "票過期之後那個「好」不可以把動作交出去：{}",
                run.out
            );
            let expired = run.events().iter().any(|e| {
                matches!(
                    e,
                    ActionEvent::Refused {
                        reason: RefusalReason::NotCoveredByGrant {
                            rejection: GrantRejection::ExpiryElapsed
                        },
                        ..
                    }
                )
            });
            assert!(
                expired,
                "擋下來這件事要在 log 上有一列說得出理由：{:?}",
                run.events()
            );
            assert!(
                run.out.contains("授權擋掉 1 步"),
                "統計那一行要把它算進「授權擋掉」，不是「執行失敗」：{}",
                run.out
            );
            // **他打完「好」之後不可以是一片空白。** 空白讀起來是「成功了」，
            // 而這一步連交都沒有交出去。
            assert!(
                run.out
                    .contains("沒有做，也沒有交給作業系統：expiry 維度拒絕：授權已過期。"),
                "擋掉這件事要當著他的面說，不能只寫進 log：{}",
                run.out
            );
            assert!(!run.out.contains("做了："), "{}", run.out);
        }

        /// 預覽答應一件事、按下去做另一件事，是這個 repo 一路在修的那件事。
        #[test]
        fn a_dry_run_writes_nothing_and_says_what_a_real_run_would_say() {
            let source = one_card();
            let preview = go(
                "act-dry",
                &source,
                &opts("開今天的連結", &["notepad.exe"], 3, 5, true),
                "",
                None,
            );
            assert!(!preview.log_exists(), "預覽不可以寫 action log");
            let real = go(
                "act-dry-real",
                &source,
                &opts("開今天的連結", &["notepad.exe"], 3, 5, false),
                "好\n",
                None,
            );
            let verdict = |text: &str| {
                text.lines()
                    .find(|l| l.starts_with("授權涵蓋") || l.starts_with("授權不涵蓋"))
                    .unwrap_or_else(|| panic!("找不到涵蓋判斷那一行：{text}"))
                    .to_owned()
            };
            assert_eq!(verdict(&preview.out), verdict(&real.out));
            // 一致還不夠——兩邊一起說假話也是一致的。這一步的 app 是
            // `chrome.exe`、授權只給 `notepad.exe`，所以那句話只能是「不涵蓋」。
            assert!(
                verdict(&preview.out).starts_with("授權不涵蓋"),
                "{}",
                preview.out
            );
            assert!(real.executor.calls.is_empty(), "說不涵蓋就不可以交出去");
        }

        #[test]
        fn the_dry_run_says_the_step_limit_will_cut_the_list_short() {
            let source = Source {
                rows: vec![
                    card(7, Some(&open_url("https://example.com/a")), &[1]),
                    card(8, Some(&open_url("https://example.com/b")), &[1]),
                ],
                apps: [(1, "chrome.exe".to_string())].into_iter().collect(),
            };
            let run = go(
                "act-dry-limit",
                &source,
                &opts("開今天的連結", &["chrome.exe"], 1, 5, true),
                "",
                None,
            );
            assert!(run.out.contains("https://example.com/a"), "{}", run.out);
            assert!(run.out.contains("https://example.com/b"), "{}", run.out);
            assert!(run.out.contains("步數上限 1"), "{}", run.out);
            // 一張可做的卡都沒有的時候，不要對著空清單講上限。
            let empty = go(
                "act-dry-empty",
                &Source::default(),
                &opts("開今天的連結", &["chrome.exe"], 1, 5, true),
                "",
                None,
            );
            assert!(!empty.out.contains("步數上限"), "{}", empty.out);
        }

        /// 五個數字各自獨立累加。任何一個從別的數字減出來的實作都會在這裡紅：
        /// 問了 3 步、做成 1 步，而「說不要」是 1 不是 2。
        #[test]
        fn the_tally_counts_what_happened_instead_of_deriving_it() {
            let source = Source {
                rows: vec![
                    card(7, Some(&open_url("https://example.com/a")), &[1]),
                    card(8, Some(&open_url("https://example.com/b")), &[1]),
                    card(9, Some(&open_url("https://example.com/c")), &[2]),
                ],
                apps: [(1, "chrome.exe".to_string()), (2, "slack.exe".to_string())]
                    .into_iter()
                    .collect(),
            };
            let run = go(
                "act-tally",
                &source,
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n不要\n好\n",
                None,
            );
            assert!(
                run.out.contains(
                    "這一輪：問了 3 步，做成 1 步，你說不要 1 步，授權擋掉 1 步，執行失敗 0 步。"
                ),
                "{}",
                run.out
            );
        }

        /// 中止的那一步已經被問了，所以 `asked` 會比其他四個數字的和多 1。
        /// 這是對的：中止那句話就印在這一行上面，讀的人看得到那一步沒有答案。
        #[test]
        fn a_run_that_was_cut_short_does_not_pretend_every_step_got_an_answer() {
            let run = go(
                "act-cut",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "停\n",
                None,
            );
            assert!(
                run.out.contains(
                    "這一輪：問了 1 步，做成 0 步，你說不要 0 步，授權擋掉 0 步，執行失敗 0 步。"
                ),
                "{}",
                run.out
            );
            assert!(run.out.contains("已中止"), "{}", run.out);
        }

        #[test]
        fn a_failed_attempt_is_not_a_refusal_and_says_so() {
            let run = go(
                "act-fail",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n",
                Some("作業系統拒絕開啟"),
            );
            assert_eq!(run.executor.calls.len(), 1, "失敗代表它真的被交出去過");
            assert!(run.out.contains("執行失敗 1 步"), "{}", run.out);
            assert!(run.out.contains("做成 0 步"), "{}", run.out);
            // 統計那一行在最後面；他在那一刻看到的是這一句。**它要說「交出去
            // 了」**——那是這一種和「擋掉了」唯一的差別，而那個差別決定他要不要
            // 去別的地方看看有沒有半途發生的事。
            assert!(
                run.out.contains("交出去了，那一端失敗了：作業系統拒絕開啟"),
                "他說了好，螢幕上要說發生了什麼：{}",
                run.out
            );
            assert!(
                !run.out.contains("做了："),
                "失敗不可以印成做好了：{}",
                run.out
            );
            assert!(
                !run.out.contains("沒有交給作業系統"),
                "它真的被交出去了，不可以說沒有：{}",
                run.out
            );
            let finished = run
                .events()
                .iter()
                .any(|e| matches!(e, ActionEvent::StepFinished { .. }));
            assert!(
                !finished,
                "失敗的嘗試不是一個完成的步驟：{:?}",
                run.events()
            );
        }

        #[test]
        fn zero_steps_and_a_nonsense_allow_each_say_what_is_wrong() {
            let dir = crate::ops::tmp::Tmp::new("act-bad-args");
            let mut bad_steps = opts("開今天的連結", &["chrome.exe"], 0, 5, true);
            bad_steps.steps = 0;
            let err = run_with_output(
                &dir.0,
                &bad_steps,
                &Source::default(),
                &mut std::io::Cursor::new(Vec::new()),
                &mut Fake::default(),
                &mut ticking(1),
                &mut Vec::new(),
            )
            .expect_err("步數上限 0 該被拒絕");
            assert!(format!("{err:#}").contains("至少 1"), "{err:#}");

            let mut bad_allow = opts("開今天的連結", &["chrome.exe"], 3, 5, true);
            bad_allow.allow = vec!["開個檔案".into()];
            let err = run_with_output(
                &dir.0,
                &bad_allow,
                &Source::default(),
                &mut std::io::Cursor::new(Vec::new()),
                &mut Fake::default(),
                &mut ticking(1),
                &mut Vec::new(),
            )
            .expect_err("不認得的 --allow 該被拒絕");
            let text = format!("{err:#}");
            assert!(text.contains("開個檔案"), "{text}");
            assert!(text.contains("open-url"), "{text}");
        }

        /// 「這張卡沒有下一步」和「有寫但讀不懂」共用一句話的那一天，
        /// 一張明明寫了東西的卡片會安靜地不見。
        #[test]
        fn a_next_step_that_cannot_be_read_is_not_a_card_without_one() {
            let source = Source {
                rows: vec![card(7, Some("幫我處理這件事"), &[1]), card(8, None, &[1])],
                apps: [(1, "chrome.exe".to_string())].into_iter().collect(),
            };
            let run = go(
                "act-unreadable",
                &source,
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "",
                None,
            );
            // 逐張指名的那一行帶著「跳過：」和原因；底下那句總結不帶。
            assert_eq!(
                run.out.matches("，跳過：").count(),
                1,
                "壞掉的那一張要被指名一次，只有一次：{}",
                run.out
            );
            assert!(run.out.contains("#7"), "{}", run.out);
            assert!(
                !run.out.contains("#8"),
                "沒有下一步的卡片要安靜：{}",
                run.out
            );
            // 一步都端不出來的時候要說出**為什麼**，而且兩種原因分開算。
            assert!(
                run.out.contains("2 張活著的承諾裡，1 張的下一步讀不懂"),
                "{}",
                run.out
            );
            assert!(run.out.contains("其餘 1 張沒有下一步"), "{}", run.out);
            assert!(run.out.contains("問了 0 步"), "{}", run.out);
        }

        /// **零步不是一種狀況，是四種。** 而「這一輪的步驟都問完了」讀起來像
        /// 「都處理完了」——四種裡沒有一種是那個意思。
        #[test]
        fn nothing_to_do_says_which_kind_of_nothing_it_was() {
            let empty = go(
                "act-none-empty",
                &Source::default(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "",
                None,
            );
            assert!(empty.out.contains("一張活著的卡都沒有"), "{}", empty.out);
            assert!(empty.out.contains("sister review --force"), "{}", empty.out);

            let no_steps = go(
                "act-none-nostep",
                &Source {
                    rows: vec![card(7, None, &[1]), card(8, None, &[1])],
                    apps: [(1, "chrome.exe".to_string())].into_iter().collect(),
                },
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "",
                None,
            );
            assert!(
                no_steps.out.contains("有 2 張活著的承諾"),
                "{}",
                no_steps.out
            );
            assert!(
                no_steps.out.contains("沒有一張帶著下一步"),
                "{}",
                no_steps.out
            );
            // **這一句是重點。** 沒有它的話，畫面等於在說「都做完了」。
            assert!(no_steps.out.contains("這不是她做完了"), "{}", no_steps.out);

            // 四種話要真的是四種——湊在一起說同一件事就白做了。
            assert_ne!(
                empty.out.lines().next(),
                no_steps.out.lines().next(),
                "兩種零步不可以是同一句話"
            );
        }

        /// 有東西可以問的時候，就不要插那一句「一步都沒端出來」。
        #[test]
        fn a_run_that_had_something_to_offer_does_not_apologise_for_nothing() {
            let run = go(
                "act-none-not",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n",
                None,
            );
            for phrase in ["一張活著的卡都沒有", "沒有一張帶著下一步", "說不出為什麼"]
            {
                assert!(!run.out.contains(phrase), "{phrase}：{}", run.out);
            }
        }

        /// 同一個資料目錄跑兩輪，讀回去要分得出那是兩輪、而且兩張票不一樣。
        ///
        /// 沒有那一列的時候，兩輪的步驟在檔案裡直接接在一起：既沒有界線，
        /// 也沒有任何東西說得出「這一步是在什麼權限底下發生的」。
        #[test]
        fn two_runs_in_one_data_dir_each_carry_their_own_grant() {
            let dir = crate::ops::tmp::Tmp::new("act-two-grants");
            let source = one_card();
            for (task, app, steps) in [
                ("開今天的連結", "chrome.exe", 3),
                ("整理下載資料夾", "explorer.exe", 1),
            ] {
                let mut executor = Fake::default();
                run_with_output(
                    &dir.0,
                    &opts(task, &[app], steps, 5, false),
                    &source,
                    &mut std::io::Cursor::new(b"\xe5\xa5\xbd\n".to_vec()),
                    &mut executor,
                    &mut ticking(1_700_000_000_000),
                    &mut Vec::new(),
                )
                .expect("跑一輪");
            }
            let events = ActionLog::in_data_dir(&dir.0)
                .replay()
                .expect("replay")
                .events;
            let grants: Vec<String> = events
                .iter()
                .filter_map(|e| match e {
                    ActionEvent::Granted { grant, .. } => Some(grant.describe()),
                    _ => None,
                })
                .collect();
            assert_eq!(grants.len(), 2, "兩輪要留下兩張票：{events:?}");
            assert_ne!(grants[0], grants[1], "兩張不一樣的票不可以讀成同一張");
            assert!(grants[0].contains("chrome.exe") && grants[0].contains("最多 3 步"));
            assert!(grants[1].contains("explorer.exe") && grants[1].contains("最多 1 步"));
            // 界線要在最前面：第一列是票，不是步驟。
            assert!(
                matches!(events[0], ActionEvent::Granted { .. }),
                "{events:?}"
            );
            // 讀成人話的那一份也要看得到它。
            let text = logged(&dir.0, 99);
            assert_eq!(text.matches(" 授權：").count(), 2, "{text}");
            assert!(text.contains("整理下載資料夾"), "{text}");
        }

        /// 一個 app 都沒授權的時候，那一列不可以讀起來像「沒有限制」。
        #[test]
        fn an_empty_app_list_reads_as_blocking_everything_not_as_unrestricted() {
            let grant = Grant::new(
                Task::new("開今天的連結"),
                AllowedApps::new(std::iter::empty()),
                AllowedActions::new([ActionKind::OpenUrl]),
                Expiry::after_issued(0, 60_000),
                StepLimit::new(3).expect("3 > 0"),
            );
            let described = grant.describe();
            assert!(described.contains("每一步都會被擋"), "{described}");
            // 五個維度一個都不能少——省掉的那一維會被讀成「這裡沒有限制」。
            for wanted in ["任務「", "app：", "動作：", "毫秒內有效", "最多 3 步"] {
                assert!(described.contains(wanted), "少了 {wanted}：{described}");
            }
        }

        fn logged(dir: &std::path::Path, limit: usize) -> String {
            let mut out = Vec::new();
            log_to(dir, limit, &mut out).expect("讀動作紀錄");
            String::from_utf8(out).expect("utf8")
        }

        /// 「這個目錄我沒看過」和「她沒有動過手」是兩句話。
        ///
        /// `ActionLog::replay` 把「檔案不存在」當成一份空的回放——對它那一層是
        /// 對的（還沒寫過就是還沒有）。但打錯資料目錄的時候，同一份空回放會讓
        /// 畫面替你宣布一件它根本沒查過的事。
        #[test]
        fn a_data_dir_we_never_looked_at_is_not_a_hand_that_never_moved() {
            let missing = crate::ops::tmp::Tmp::new("act-log-missing").0.join("nope");
            let mut out = Vec::new();
            let err = log_to(&missing, 20, &mut out).expect_err("目錄不存在要報錯");
            let text = format!("{err:#}");
            assert!(text.contains("找不到這個資料目錄"), "{text}");
            assert!(
                out.is_empty(),
                "不可以在報錯之前就先印一句「還沒有任何動作紀錄」：{out:?}"
            );
        }

        #[test]
        fn an_empty_log_says_nothing_was_ever_offered_and_where_it_would_live() {
            let dir = crate::ops::tmp::Tmp::new("act-log-empty");
            let text = logged(&dir.0, 20);
            assert!(text.contains("還沒有任何動作紀錄"), "{text}");
            assert!(
                text.contains("action-log.jsonl"),
                "空的時候更要講清楚它會寫在哪：{text}"
            );
        }

        /// 跑完一輪之後，`sister hands log` 講的要是**同一輪**發生的事。
        #[test]
        fn after_a_real_run_the_log_reads_back_the_same_story() {
            let run = go(
                "act-log-story",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n",
                None,
            );
            let text = logged(&run.dir.0, 20);
            assert!(text.contains("提出："), "{text}");
            assert!(text.contains("核准："), "{text}");
            assert!(text.contains("已執行："), "{text}");
            assert!(text.contains("https://example.com/a"), "{text}");
            assert!(
                !text.contains("還沒有任何動作紀錄"),
                "有紀錄就不可以說沒有：{text}"
            );
        }

        /// 只給最近幾列的時候，被蓋掉的那幾列要有人講；而讀不懂的那幾列
        /// 不可以混進「她做過的事」裡不說話。
        #[test]
        fn truncation_and_unreadable_lines_each_get_their_own_sentence() {
            let run = go(
                "act-log-truncate",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n",
                None,
            );
            let path = ActionLog::in_data_dir(&run.dir.0).path().to_owned();
            let mut body = std::fs::read_to_string(&path).expect("讀回 jsonl");
            body.push_str("{這不是 json\n");
            std::fs::write(&path, body).expect("寫回 jsonl");

            let full = logged(&run.dir.0, 99);
            assert!(
                full.contains("列讀不懂。那幾列是發生過但解不開，不是沒有發生。"),
                "{full}"
            );
            assert!(
                !full.contains("沒有顯示"),
                "沒有截斷就不要憑空講截斷：{full}"
            );

            let short = logged(&run.dir.0, 2);
            assert!(short.contains("沒有顯示"), "截斷了要說出來：{short}");
            assert!(
                short.contains("列讀不懂"),
                "截斷不可以把那句解釋也吃掉：{short}"
            );
        }

        /// 空的紀錄有兩種，而它們原本印同一句話。
        ///
        /// 檔案不存在＝她真的一次都沒問過你。
        /// 檔案在、裡面沒東西＝她問過，那些列被 `sister forget` 刪掉了。
        /// 對後者說「她從來沒有把一個動作端到你面前過」，是拿一句沒查過的歷史
        /// 去蓋掉一次真的刪除——而讀的人會以為自己記錯了。
        ///
        /// 上面那條 `a_data_dir_we_never_looked_at_…` 擋的是同一個錯的高一層版本
        /// （目錄查不到），低一層這個當時漏掉了。
        #[test]
        fn an_emptied_log_is_not_a_hand_that_never_moved() {
            let run = go(
                "act-log-emptied",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n",
                None,
            );
            let log = ActionLog::in_data_dir(&run.dir.0);
            let before = logged(&run.dir.0, 20);
            assert!(before.contains("已執行："), "先確定真的寫進去了：{before}");

            // 走真正的刪除路徑，不是自己 truncate 一個檔案出來——這條測試要證明的
            // 就是「那條路留下的東西長什麼樣」。
            let report = log.forget_range(0, i64::MAX).expect("忘掉全部");
            assert!(report.kept == 0, "整段都在範圍裡，一列都不該留：{report:?}");
            assert!(log.path().exists(), "刪的是列，不是檔案本身");

            let after = logged(&run.dir.0, 20);
            assert!(
                !after.contains("從來沒有"),
                "她端過，只是紀錄被刪了。這句話是假的：{after}"
            );
            assert!(after.contains("被刪光了"), "{after}");
            assert!(
                after.contains("sister forget"),
                "要講出唯一會把列刪掉的那條路：{after}"
            );
        }

        /// 「問了三步、三步都做完」和「一步都沒問到你」在紀錄裡不可以是同一列。
        ///
        /// alpha.70 在**螢幕上**把這兩件事分開了（`nothing_to_offer`），
        /// 磁碟上那一半漏掉：兩種都寫 `{"conclusion":"completed"}`，
        /// 而 `sister hands log` 隔一週再看回去，它們又合成同一句
        /// 「這一輪的步驟都問完了。」——那句話讀起來是「都處理完了」。
        #[test]
        fn a_round_that_asked_nothing_does_not_read_as_all_done_in_the_log() {
            let no_next_step = Source {
                rows: vec![card(7, None, &[1])],
                apps: [(1, "chrome.exe".to_string())].into_iter().collect(),
            };
            let asked_none = go(
                "act-log-zero",
                &no_next_step,
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "",
                None,
            );
            let zero = logged(&asked_none.dir.0, 20);
            assert!(
                zero.contains("一步都沒有問到你"),
                "零步要自己講出來：{zero}"
            );

            let asked_one = go(
                "act-log-one",
                &one_card(),
                &opts("開今天的連結", &["chrome.exe"], 3, 5, false),
                "好\n",
                None,
            );
            let one = logged(&asked_one.dir.0, 20);
            assert!(one.contains("問到你面前 1 步"), "{one}");
            assert_ne!(
                zero.contains("一步都沒有問到你"),
                one.contains("一步都沒有問到你"),
                "兩輪的收尾列不可以讀起來一樣\n零步：{zero}\n一步：{one}"
            );
        }
    }
}

pub mod watch {
    use super::*;
    use anyhow::ensure;
    use sister_core::brain;
    use sister_core::db::OutboundInsert;
    use sister_core::heartbeat::{Phase, Presence};
    use sister_core::watch::{Blind, GRACE, Look, Tally, Verdict, WatchEnd, WatchSkip};
    use sister_core::{Config, Millis};
    use std::io::Write;

    const HIT_LIMIT: usize = 200;

    pub struct WatchOpts {
        pub question: String,
        pub every: Millis,
        pub stop_after: Millis,
        pub quiet_for: Option<Millis>,
        pub dry_run: bool,
        pub notify: bool,
    }

    /// **閃到他回頭看為止，不是閃三下就算了。**
    ///
    /// 這整支旗標存在的前提是「他不在這個終端機前面」。`uCount: 3` 配
    /// `FLASHW_TRAY` 是閃三下（約一秒半）然後自己停下來——他去泡個咖啡就完全
    /// 錯過，回來看到的是一個安靜的工作列和一句他不知道什麼時候印出來的話。
    /// 那和沒有通知是同一件事，而且更糟：他以為自己收得到。
    ///
    /// `FLASHW_TIMERNOFG` 是「一直閃到這個視窗被叫到前景為止」，也就是**閃到
    /// 他真的看到**。這是 Windows 為這件事準備的那一個旗標。搭配它的時候
    /// `uCount` 不再是次數上限，填 0。
    ///
    /// 這一段在這台 Linux 上編得到（`check-windows.sh`）但**執行不到**，
    /// 一條斷言都蓋不住它——所以它上面這幾行是它唯一的規格。
    #[cfg(windows)]
    fn platform_notify() {
        use windows::Win32::System::Console::GetConsoleWindow;
        use windows::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows::Win32::UI::WindowsAndMessaging::{
            FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, MB_OK,
        };

        // SAFETY: both calls only address this process's console window. They neither retain
        // pointers nor transfer focus; failure merely leaves BEL as the notification.
        unsafe {
            let hwnd = GetConsoleWindow();
            if !hwnd.is_invalid() {
                let info = FLASHWINFO {
                    cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                    hwnd,
                    dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
                    uCount: 0,
                    dwTimeout: 0,
                };
                let _ = FlashWindowEx(&info);
            }
            let _ = MessageBeep(MB_OK);
        }
    }

    #[cfg(not(windows))]
    fn platform_notify() {}

    fn notify(out: &mut impl Write) -> Result<()> {
        write!(out, "\x07")?;
        out.flush()?;
        platform_notify();
        Ok(())
    }

    pub fn run(data_dir: &Path, config: &Config, opts: &WatchOpts) -> Result<()> {
        run_with(
            data_dir,
            config,
            opts,
            &mut sister_core::now_ms,
            &mut |span| std::thread::sleep(std::time::Duration::from_millis(span as u64)),
            &mut std::io::stdout(),
        )
    }

    /// **她死掉也算「停下來」，所以那一聲也要響。**
    ///
    /// 開跑那一行答應的是「停下來時我會響一聲」，而收尾那五種各自都響了。
    /// 少的是第六條出路：迴圈中間任何一個 `?`（資料庫被鎖住、磁碟滿了）會
    /// 直接把 `Err` 丟回去，**一聲都不響**。而 `--notify` 這支旗標的整個前提
    /// 就是他不在螢幕前面——她死了又不出聲，他會一直等下去，正好是這支旗標
    /// 要防的那件事。沉默被讀成「還在跑」。
    ///
    /// 開跑前就失敗的那幾種（目錄不存在、資料庫開不起來）也會走到這裡。那時候
    /// 還沒答應過任何事，多響一聲是噪音——但他人就坐在終端機前面，因為那幾種
    /// 在第一秒就炸了。多一聲噪音，比她死得無聲無息便宜太多。
    pub(crate) fn run_with(
        data_dir: &Path,
        config: &Config,
        opts: &WatchOpts,
        clock: &mut dyn FnMut() -> i64,
        sleep: &mut dyn FnMut(Millis),
        out: &mut impl Write,
    ) -> Result<()> {
        let result = watch_body(data_dir, config, opts, clock, sleep, out);
        if result.is_err() && opts.notify && !opts.dry_run {
            // 已經在回報一個錯了，這一聲響不出來就算了——不要拿它蓋掉真正的原因。
            let _ = notify(out);
        }
        result
    }

    fn watch_body(
        data_dir: &Path,
        config: &Config,
        opts: &WatchOpts,
        clock: &mut dyn FnMut() -> i64,
        sleep: &mut dyn FnMut(Millis),
        out: &mut impl Write,
    ) -> Result<()> {
        ensure!(
            data_dir.is_dir(),
            "不是她沒看到，是我們沒找到那個目錄：{}",
            data_dir.display()
        );
        let mut db = Db::open(&Config::db_path(data_dir))?;
        let consent = sister_core::consent::load(data_dir);
        // 這三句用 `WatchSkip`，**不是** `brain::SkipReason`。它們在開跑當下
        // 就結束，使用者仍坐在終端機前，所以即使有 --notify 也不發訊號。那三句是替
        // `sister interpret` 寫的，其中一句說「超過即靜默降級，只累積 L0/L1」
        // ——對 interpret 是真的，對這裡是假的：這個行程當場就結束，她一眼
        // 都不會看。指路那一行也一樣，要指到 `sister watch --dry-run`。
        // **開跑那一刻的閘門。** 迴圈裡每一輪都會再讀一次（見那裡）——這一張
        // 票只證明「開跑的時候他簽著」，不證明十分鐘後他還簽著。
        if consent.cloud_permit().is_none() {
            writeln!(out, "{}", WatchSkip::NoConsent.message())?;
            return Ok(());
        }
        let Some((command, args)) = config.brain.cli() else {
            writeln!(out, "{}", WatchSkip::NoCommand.message())?;
            return Ok(());
        };
        let command = command.to_string();
        let args = args.to_vec();
        let started = clock();
        // daily_budget 是所有外送共用的天花板（brain::run 也讀這支不分 role 的
        // 計數）；reviewer_daily_budget 只是其中 reviewer 的子上限。watcher 若改
        // 用 role 計數，會偷偷把使用者設定的每日總數往上加。
        //
        // 這裡只是**開跑那一刻**的數字，拿來印那句「還剩幾次」。迴圈裡每一輪
        // 都會重新算一次，連日期也重新算——`--stop-after 8h` 跨過午夜的時候，
        // 一個在啟動時算好就不再動的 `day` 會把四十列外送記在昨天的帳上，
        // 而昨天的額度已經滿了：使用者設的每日上限就這樣被悄悄加倍。
        // `brain::run` 也是每次進去才算（`local_day_key(now_ms())`）。
        let used = db.brain_outbound_count_on(&day_of(started)?)?;
        if used >= config.brain.daily_budget {
            writeln!(
                out,
                "{}",
                WatchSkip::NoBudgetToday {
                    used,
                    limit: config.brain.daily_budget
                }
                .message()
            )?;
            return Ok(());
        }

        let every = opts.every.max(sister_core::watch::MIN_EVERY);
        writeln!(out, "盯著：{}", opts.question)?;
        if every != opts.every {
            writeln!(
                out,
                "你設定的間隔太快，已從 {} 秒抬到 30 秒。",
                opts.every / 1_000
            )?;
        }
        // **她量不出比自己看一次還短的安靜。**
        //
        // 那個判斷掛在「這一輪的視窗裡一段字都沒有」上，而視窗就是一個
        // `every`。所以 `--quiet-for 30s --every 2m` 之下，那個旗標**一次都不會
        // 觸發**——她永遠會在視窗裡看到那段字。一個安靜地什麼都不做的旗標，
        // 比沒有這個旗標更糟：使用者設了、以為她在盯，然後等了一小時。
        // 抬到看得出來的最短值，然後講出來。
        let quiet_for = opts.quiet_for.map(|asked| asked.max(every));
        if let (Some(asked), Some(used_span)) = (opts.quiet_for, quiet_for)
            && used_span != asked
        {
            writeln!(
                out,
                "你要求畫面安靜 {}就講，但我每 {}才看一次——比這更短的安靜我量不出來，已抬到 {}。",
                crate::fmt::duration_ms(asked),
                crate::fmt::duration_ms(every),
                crate::fmt::duration_ms(used_span)
            )?;
        }
        writeln!(
            out,
            "{}",
            sister_core::watch::plan_line(every, opts.stop_after, used, config.brain.daily_budget)
        )?;
        if let Some(span) = quiet_for {
            writeln!(
                out,
                "畫面連續 {}沒有新的字，我就停下來講一聲（那是本機的判斷，不問大腦、不吃預算）。",
                crate::fmt::duration_ms(span)
            )?;
        }
        // **預演不會盯，所以它沒有收尾可以通知。**
        //
        // `notification_notice()` 那一句原本印在 `--dry-run` 那個 early return
        // 前面，於是 `sister watch … --dry-run --notify` 會承諾「停下來時我會讓
        // 工作列那顆按鈕閃一下、響一聲」——一句關於一場永遠不會發生的盯梢的
        // 承諾。往下搬之後換成另一種危險：一個在預演裡安靜地什麼都不做的旗標。
        // 所以兩條路各講各的話，兩句都不是沉默。
        if opts.notify {
            if opts.dry_run {
                writeln!(
                    out,
                    "（--notify 這一趟用不到：預演只印出要送的字就結束，沒有收尾可以通知你。）"
                )?;
            } else {
                writeln!(out, "{}", WatchEnd::notification_notice(cfg!(windows)))?;
            }
        }

        let mut last_seen = started.saturating_sub(every);
        if opts.dry_run {
            let (from, to) = window(last_seen, started).expect("開跑那一刻的時鐘不會比自己早");
            let (hits, more) = newest_since(&db, from, to, HIT_LIMIT)?;
            if more {
                writeln!(
                    out,
                    "（這段時間的字超過 {HIT_LIMIT} 段，底下只有最新的 {HIT_LIMIT} 段。）"
                )?;
            }
            let prompt = sister_core::watch::build_watch_prompt(&opts.question, &hits)?;
            writeln!(out, "{}", prompt.payload)?;
            if prompt.truncated {
                writeln!(
                    out,
                    "注意：畫面證據超過上限，這次只印出最新的 {} 段，也只會送出這些，不是全部。",
                    prompt.included_chunks
                )?;
            }
            writeln!(out, "預演到此為止：一次都沒送，也沒有寫外送紀錄。")?;
            return Ok(());
        }

        let deadline = started.saturating_add(opts.stop_after);
        let mut tally = Tally::default();
        let end: WatchEnd = loop {
            let now = clock();
            // **到期那一刻要再看最後一眼，才輪到收尾那句話。**
            //
            // 原本這裡是「到期就直接印 Deadline 然後 return」，於是最後那一段
            // `[上一輪, deadline)` 從來沒有被查過——`--every 2m --stop-after 1h`
            // 之下，第 59 分 30 秒才跑完的那個編譯，會得到一句「沒有等到」，
            // 而那段字就躺在她自己的資料表裡。
            let expired = now >= deadline;

            // **每一輪重讀一次同意書。**
            //
            // 這是這個 repo 明訂的規矩：`docs/PRIVACY.md` 寫著「『隨時』是真的
            // 隨時，不是『下次重開』」，而 `sister record` 的迴圈每 5 秒重讀一次
            // 就是為了那句話。`sister interpret` 是一次性的，所以在 alpha.71
            // 之前，第二張同意書**沒有任何長命的持票人**。
            //
            // `watch` 是第一個。開跑時鑄一張票然後抱著它跑八小時的話，他在另一
            // 個視窗打 `sister consent --revoke cloud-reading`、螢幕上回他
            // 「沒有這一張，她一次都不會呼叫那支 CLI」（現在式），而這個迴圈
            // 會在接下來的七小時五十七分裡把螢幕原文送出去兩百多次。
            //
            // 讀在**這一輪的最前面**，不是只讀在有字要送的那一臂裡：畫面安靜
            // 的時候她不會送，但她也不該還醒著——她已經問不了了。
            let Some(permit) = sister_core::consent::load(data_dir).cloud_permit() else {
                break WatchEnd::ConsentRevoked { tally };
            };

            let look = match window(last_seen, now) {
                // 時鐘往回跳了（NTP 校時、睡眠喚醒），這一輪的區間是反的。
                // 不查，也不要把一次沒查過的空手講成「她確實正在錄」。
                None => Look::NothingNew(Blind::ClockWentBackwards { last_seen }),
                Some((from, to)) => {
                    let (hits, more) = newest_since(&db, from, to, HIT_LIMIT)?;
                    if more {
                        writeln!(
                            out,
                            "（這段時間的字超過 {HIT_LIMIT} 段，只拿最新的 {HIT_LIMIT} 段去問。）"
                        )?;
                    }
                    if hits.is_empty() {
                        let blind = blind_reason(data_dir, now);
                        if blind == Blind::RecordingButQuiet
                            && let (Some(threshold), Some(last)) =
                                (quiet_for, db.recent(1)?.into_iter().next())
                        {
                            let elapsed = now.saturating_sub(last.ts);
                            if now >= last.ts && elapsed >= threshold {
                                // 這一輪確實沒有新畫面可看，所以仍算進 blind；
                                // WentQuiet 是停止原因，不是第四種計數。
                                tally.count(&Look::NothingNew(Blind::RecordingButQuiet));
                                break WatchEnd::WentQuiet {
                                    tally,
                                    quiet_for: elapsed,
                                    last_at: last.ts,
                                    last_app: last.app_id,
                                };
                            }
                        }
                        Look::NothingNew(blind)
                    } else {
                        // 日期和用量都在這裡才算。跑八小時跨過午夜的話，開跑
                        // 那一刻算好的 `day` 會把今天的外送記在昨天的帳上。
                        let day = day_of(now)?;
                        let used = db.brain_outbound_count_on(&day)?;
                        if used >= config.brain.daily_budget {
                            // **「預算*先*用完了」是一個排序斷言。**
                            //
                            // 到期那一刻同時撞到預算牆的話，那句話沒查過就
                            // 宣布了誰先誰後——而 `expired` 就在三行以上的
                            // 作用域裡。它還接著說「我沒有再看下去」，讓人以為
                            // 多給預算就會有答案；時間已經到了，不會有。
                            // 他要的是 `--stop-after`，那一句才是他的答案。
                            break if expired {
                                WatchEnd::Deadline {
                                    tally,
                                    hopeless: false,
                                }
                            } else {
                                WatchEnd::BudgetRanOut {
                                    tally,
                                    used,
                                    limit: config.brain.daily_budget,
                                }
                            };
                        }
                        let prompt = sister_core::watch::build_watch_prompt(&opts.question, &hits)?;
                        if prompt.truncated {
                            writeln!(
                                out,
                                "注意：畫面證據超過上限，這輪只問最新的 {} 段，不是全部。",
                                prompt.included_chunks
                            )?;
                        }
                        let sent_hits = &hits[prompt.included_from..];
                        let spawn = brain::spawn_cli(permit, &prompt.payload, &command, &args);
                        let (outcome, verdict) = sister_core::watch::verdict_from_spawn(&spawn);
                        db.insert_brain_outbound(&OutboundInsert {
                            ts: now,
                            day_key: &day,
                            command: &command,
                            args: &args,
                            segment_core_start: None,
                            chars_sent: prompt.payload.chars().count() as i64,
                            truncated: more || prompt.truncated,
                            outcome: outcome.as_str(),
                            duration_ms: spawn.duration_ms as i64,
                            error: spawn.spawn_error.as_deref(),
                            role: "watcher",
                        })?;
                        Look::Asked {
                            available_chunks: hits.len(),
                            available_capped: more,
                            chunks: prompt.included_chunks,
                            newest_app: sent_hits
                                .last()
                                .expect("nonempty prompt evidence")
                                .app_id
                                .clone(),
                            verdict,
                        }
                    }
                }
            };
            // **高水位，只往前不往後。**
            //
            // 這裡原本是 `last_seen = now + 1`，時鐘往回跳的時候就會把游標
            // 一起拖回去——然後她把跳過的那一段整個重跑一遍。一次十分鐘的
            // NTP 校正在 `--every 30s` 之下是二十輪重問，每一輪只要有字就是
            // 一次外送：同一段畫面被送出去第二次，開跑時說的「最多問 N 次」
            // 當場變成假話，而預設一天只有 80 次。
            //
            // 停在高水位的代價是時鐘走回來之前每一輪都查不到東西——但那幾輪
            // 會誠實地說「時鐘往回跳了」，而且**一毛錢都不花**。
            last_seen = last_seen.max(now.saturating_add(1));

            tally.count(&look);
            // 時刻要讀得懂，而且要和 `sister hands log` 同一個格式——他會拿兩邊
            // 對時間。印 epoch 毫秒等於沒印。
            writeln!(
                out,
                "{}  {}",
                sister_core::model::stamp(now),
                look.message()
            )?;
            if let Look::Asked {
                verdict: Verdict::Happened { .. },
                ..
            } = look
            {
                break WatchEnd::Saw { tally };
            }
            if expired {
                // 最後那一眼看到的是「她已經不在錄了」的話，「沒等到」只算到
                // 她停下來為止——後面那段時間我對著的是一張凍住的畫面。
                let hopeless = matches!(&look, Look::NothingNew(blind) if blind.hopeless());
                break WatchEnd::Deadline { tally, hopeless };
            }
            sleep(every);
        };
        writeln!(out, "{}", end.message())?;
        if end.should_notify(opts.notify) {
            notify(out)?;
        }
        Ok(())
    }

    /// 這一輪要查的區間 `[from, to)`，往回多留 [`GRACE`] 那一段。
    ///
    /// **往回多看的那幾秒不是保險，是必需的。** `text_chunks.ts` 是抓那一幀的
    /// 時間，而那一列要等 OCR 跑完才進得了資料庫。游標貼著「現在」的話，每一
    /// 輪最新的那一小段永遠是在查詢跑完之後才落地，而下一輪的起點已經在它
    /// 後面了——所以它**再也不會被看到**，偏偏那正是「編譯剛剛跑完」那一刻。
    ///
    /// `None` ＝ 時鐘往回跳了，這個區間是反的。回一個空的 `Vec` 會讓呼叫端
    /// 把「沒查」講成「查了，沒有新的字」。
    fn window(last_seen: Millis, now: Millis) -> Option<(Millis, Millis)> {
        let from = last_seen.saturating_sub(GRACE);
        let to = now.saturating_add(1);
        (to > from).then_some((from, to))
    }

    /// 這一刻算在哪一天的帳上。
    fn day_of(ts: Millis) -> Result<String> {
        sister_core::local_day::local_day_key(ts).context("算不出今天的日期，不敢送")
    }

    /// 這一輪要看的那幾段字：**最新的那幾段**，由舊到新排好。
    ///
    /// 用 `Db::recent` 再自己夾時間，不用 `chunks_in_range`——後者是
    /// `ORDER BY ts ASC LIMIT n`，也就是**最舊的 n 段**。一個編譯到一半狂吐
    /// 訊息的終端機，一個間隔內輕鬆超過 n 段，於是她永遠在讀兩分鐘前的畫面，
    /// 而「All checks passed」就在她沒看的那一頭。更糟的是那個變數叫 `newest`。
    ///
    /// 拿不到全部的時候要講出來（回傳值第二格），因為「看了 200 段」和
    /// 「看了 200 段而且還有更多沒看」是兩件事。
    fn newest_since(
        db: &Db,
        from: Millis,
        to: Millis,
        limit: usize,
    ) -> Result<(Vec<sister_core::model::SearchHit>, bool)> {
        // 多撈一段，才分得出「剛好 limit 段」和「不只 limit 段」。
        let mut hits: Vec<sister_core::model::SearchHit> = db
            .recent(limit.saturating_add(1))?
            .into_iter()
            .filter(|hit| hit.ts >= from && hit.ts < to)
            .collect();
        let more = hits.len() > limit;
        hits.truncate(limit);
        // `recent` 給的是新的在前；送進 prompt 的證據要由舊到新，
        // 不然模型讀到的時間軸是倒的。
        hits.reverse();
        Ok((hits, more))
    }

    /// 看不到新畫面的時候，去把**為什麼**問出來。
    ///
    /// 暫停要先問：`Presence` 那個型別看不到暫停（那是旁邊另一個檔案），
    /// 而暫停的時候心跳照跳，所以只問 `presence` 會得到 `Live` ——
    /// 於是畫面會說「她確實正在錄」，而她的眼睛是閉著的。
    fn blind_reason(data_dir: &Path, now: Millis) -> Blind {
        if sister_core::pause::is_paused(data_dir) {
            return Blind::Paused;
        }
        match sister_core::heartbeat::presence(data_dir, now) {
            Presence::NeverStarted => Blind::NeverStarted,
            Presence::Unreadable => Blind::Unreadable,
            Presence::Live(Phase::Recording) => Blind::RecordingButQuiet,
            // 開機中不是「正在錄但畫面沒動」。兩個底下都是 `Live`，而
            // `Live(_)` 一個底線就把它們併成同一句話——如果她卡在開機，
            // 那句「她確實正在錄」會讓人一直等下去。
            Presence::Live(Phase::Booting) => Blind::Booting,
            // **錄製已經停了**，只剩解釋層在把最後一段想完。
            //
            // 這一臂原本寫成 `Blind::Stopped`，畫面上每兩分鐘跳一句「她在 X
            // 收工了」，掉了「還在想」那半段；我改掉的時候在這裡寫了一句
            // 「她正在忙，不是停了」——**那句話是反過來的假話**。
            // `heartbeat::beat_thinking` 的說明：「錄製迴圈已經停了……她一個
            // 畫面都不再抓」。兩半都要留著：錄製停了（所以再等也沒有新畫面），
            // 而且還有人佔著這個資料目錄（所以不是 `Stopped`）。
            Presence::Thinking { at: _, until } => Blind::Thinking { until },
            Presence::Stopped { at } => Blind::Stopped { at },
            Presence::Stalled { at, phase: _ } => Blind::Stalled { at },
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn prepared(name: &str, text: &str, consented: bool) -> (crate::ops::tmp::Tmp, Config) {
            prepared_at(name, 90_000, text, consented)
        }

        fn prepared_at(
            name: &str,
            ts: i64,
            text: &str,
            consented: bool,
        ) -> (crate::ops::tmp::Tmp, Config) {
            let tmp = crate::ops::tmp::Tmp::new(name);
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let session = db.start_session("test", "test").expect("session");
            db.conn()
                .execute(
                    "INSERT INTO text_chunks(ts,session_id,source_kind,app_id,text) VALUES(?1,?2,'ocr','Terminal.exe',?3)",
                    (ts, session, text),
                )
                .expect("chunk");
            drop(db);
            if consented {
                let mut consent = sister_core::consent::Consent::default();
                consent.grant(sister_core::consent::Sheet::CloudReading, 1);
                sister_core::consent::save(&tmp.0, &consent).expect("consent");
            }
            let mut config = Config::default();
            config.brain.command = "sh".into();
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":true,\"because\":\"畫面上出現完成\"}'".into(),
            ];
            (tmp, config)
        }

        fn opts(notify: bool) -> WatchOpts {
            WatchOpts {
                question: "完成嗎".into(),
                every: 30_000,
                stop_after: 0,
                quiet_for: None,
                dry_run: false,
                notify,
            }
        }

        #[test]
        fn a_row_capped_by_the_chunk_limit_is_recorded_as_truncated() {
            let (tmp, config) = prepared(
                "watch-chunk-limit-truncation",
                "第000段很短的畫面文字",
                true,
            );
            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let session: i64 = db
                .conn()
                .query_row("SELECT id FROM sessions LIMIT 1", [], |row| row.get(0))
                .expect("session");
            for n in 1..=200 {
                db.conn()
                    .execute(
                        "INSERT INTO text_chunks(ts,session_id,source_kind,app_id,text) VALUES(?1,?2,'ocr','Terminal.exe',?3)",
                        (90_000 + n, session, format!("第{n:03}段很短的畫面文字")),
                    )
                    .expect("chunk");
            }
            drop(db);

            let mut ticks = [100_000, 100_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &opts(false),
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");

            // **這條測試要證的不只是「truncated 是 true」，是「哪一把刀讓它變 true」。**
            //
            // 兩把刀都會寫同一個布林：200 列那一把（`more`）和位元組那一把
            // （`prompt.truncated`，上限是 `brain::MAX_PROMPT_BYTES`）。位元組那把
            // 要是也砍了，就算把實作改回只記位元組，這條測試還是綠的——它會為了
            // 錯的理由通過，而列上限那一刀從此沒有人看著。
            //
            // 螢幕上那兩句話是唯一分得開它們的地方，所以斷言直接打在那裡：
            // 列上限那句**要在**，位元組那句**不能在**。
            // （`chars_sent` 分不開：它數的是**字**，上限數的是**位元組**，一個
            // 中文字三個位元組，拿字數去斷言位元組上限什麼都證不到。這裡刻意
            // 不把那個常數抄成數字——抄下來的數字會在它改動的那天變成假話。）
            let printed = String::from_utf8(out).expect("輸出是 UTF-8");
            assert!(
                printed.contains("只拿最新的 200 段去問"),
                "列上限那一刀沒砍到，這一輪根本不是這條測試要測的那一輪：{printed}"
            );
            assert!(
                !printed.contains("畫面證據超過上限"),
                "位元組那一刀也砍了，於是這條測試分不出 truncated 是誰記上去的：{printed}"
            );

            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let rows = db.list_brain_outbound(10).expect("outbound log");
            assert_eq!(rows.len(), 1, "這一輪應該只寫一列外送紀錄");
            let row = &rows[0];
            assert!(
                row.truncated,
                "201 段只送最新 200 段，列上限丟了一段卻記成沒截斷"
            );
            assert!(
                row.chars_sent > 2_000,
                "送出的字數不像包含 200 段，可能只測到一段：{}",
                row.chars_sent
            );
        }

        /// **撞到列上限的那一輪，那個段數是下限，不是總數。**
        ///
        /// `available_chunks` 拿的是 `hits.len()`，而 `hits` 已經被 `HIT_LIMIT`
        /// 砍成 200 了。所以沒撞上限的時候它是「畫面上有幾段」，撞上限之後它
        /// 變成「我撈上來幾段」——**一個變數回答兩個問題**。畫面上會變成
        /// 上一行說「超過 200 段」、下一行說「有 200 段」。
        ///
        /// `watch.rs` 那兩條單元測試釘的是 `Look::message()` 自己，它們手捏
        /// `available_capped`，所以**接線斷掉它們一條都不會紅**（實測把
        /// `available_capped: more` 改成 `false`，整個 workspace 十六組測試
        /// 全綠）。這一條走真的 `run_with`，釘的就是那條線。
        #[test]
        fn a_round_capped_by_the_row_limit_calls_the_count_a_floor_not_a_total() {
            // 每段都要夠長，長到 200 段**也會**撞穿位元組上限——不然
            // `available_chunks == chunks`，那句省略根本不會印出來，這條測試
            // 會在一片空白上通過。
            let fat = "畫面文字".repeat(100);
            let (tmp, config) = prepared("watch-row-cap-floor", &fat, true);
            let db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let session: i64 = db
                .conn()
                .query_row("SELECT id FROM sessions LIMIT 1", [], |row| row.get(0))
                .expect("session");
            for n in 1..=200 {
                db.conn()
                    .execute(
                        "INSERT INTO text_chunks(ts,session_id,source_kind,app_id,text) VALUES(?1,?2,'ocr','Terminal.exe',?3)",
                        (90_000 + n, session, format!("{fat}{n:03}")),
                    )
                    .expect("chunk");
            }
            drop(db);

            let mut ticks = [100_000, 100_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &opts(false),
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");

            let printed = String::from_utf8(out).expect("輸出是 UTF-8");
            assert!(
                printed.contains("只拿最新的 200 段去問"),
                "這一輪沒撞到列上限，測不到這條測試要測的東西：{printed}"
            );
            assert!(
                printed.contains("超過 200 段"),
                "撞了列上限卻把 200 講成總數：{printed}"
            );
            assert!(
                !printed.contains("畫面上有 200 段"),
                "同一份輸出裡上一行說超過 200、下一行說有 200：{printed}"
            );
        }

        #[test]
        fn seeing_the_answer_writes_bell_only_when_requested() {
            for requested in [true, false] {
                let (tmp, config) = prepared("watch-notify-saw", "完成", true);
                let mut ticks = [100_000, 100_000].into_iter();
                let mut out = Vec::new();
                run_with(
                    &tmp.0,
                    &config,
                    &opts(requested),
                    &mut || ticks.next().expect("fake clock ran out"),
                    &mut |_| {},
                    &mut out,
                )
                .expect("run");
                assert_eq!(
                    out.contains(&b'\x07'),
                    requested,
                    "Saw 的通知和旗標不同步：{}",
                    String::from_utf8_lossy(&out)
                );
            }
        }

        /// **她死掉的時候那一聲也要響，不然沉默會被讀成「還在跑」。**
        ///
        /// 五種正常收尾各自都響過了，第六條出路是中途炸掉。`--notify` 的前提
        /// 就是他不在螢幕前面，所以「她死了又不出聲」正好是這支旗標要防的那件
        /// 事——他會一直等下去。
        ///
        /// 這裡用「資料目錄不見了」去引一個 `Err`：它走的是 `run_with` 包在
        /// 外面那一層，和迴圈中間任何一個 `?` 是同一條路。
        #[test]
        fn a_run_that_dies_still_rings_if_he_asked_to_be_told() {
            for requested in [true, false] {
                let missing = std::path::Path::new("/nonexistent-sister-data-dir-for-test");
                let config = Config::default();
                let mut out = Vec::new();
                let err = run_with(
                    missing,
                    &config,
                    &opts(requested),
                    &mut || 100_000,
                    &mut |_| {},
                    &mut out,
                )
                .expect_err("目錄不存在，這一趟本來就該失敗");
                assert!(
                    format!("{err:#}").contains("我們沒找到那個目錄"),
                    "紅的理由不是這條測試要引的那一個：{err:#}"
                );
                assert_eq!(
                    out.contains(&b'\x07'),
                    requested,
                    "她死掉那一聲和旗標不同步（requested={requested}）：{}",
                    String::from_utf8_lossy(&out)
                );
            }
        }

        #[test]
        fn a_quiet_screen_end_also_writes_bell() {
            let now = 1_756_200_000_000_i64;
            let (tmp, config) = prepared_at("watch-notify-quiet", now - 60_000, "舊字", true);
            sister_core::heartbeat::beat(&tmp.0, now).expect("recording");
            let mut ticks = [now, now].into_iter();
            let mut out = Vec::new();
            let mut options = opts(true);
            options.quiet_for = Some(30_000);
            run_with(
                &tmp.0,
                &config,
                &options,
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8_lossy(&out).into_owned();
            // **先證明這一輪真的是從 WentQuiet 收尾的。**
            // 少了這一句，`stop_after: 0` 讓它走 Deadline 也照樣會響，而測試
            // 的名字說的是另一件事——綠燈就會是綠得沒有意義的那一種。
            assert!(
                text.contains("畫面上已經"),
                "這一輪不是從畫面安靜收尾的：{text}"
            );
            assert!(out.contains(&b'\x07'), "WentQuiet 沒有通知：{text}");
        }

        /// **開跑那一行要真的印出來，而且要說這個組建做得到什麼。**
        ///
        /// 上一輪 `--quiet-for` 就是掉在這裡：旗標收下了、什麼都沒做、畫面上
        /// 一個字都沒有。`notification_notice` 那條單元測試只釘住那兩句話本身
        /// ——**把呼叫它的那三行整段刪掉，全部測試照樣綠**（量過）。一個沒有
        /// 人呼叫的純函式，證不出使用者看得到那句話。
        #[test]
        fn asking_for_notification_says_what_this_build_can_actually_do() {
            let expected = sister_core::watch::WatchEnd::notification_notice(cfg!(windows));
            let (tmp, config) = prepared("watch-notify-announce", "完成", true);
            let mut ticks = [100_000, 100_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &opts(true),
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains(expected), "開跑沒有說她會怎麼通知：{text}");

            // 沒要求的時候不可以憑空承諾一個訊號。
            let (tmp, config) = prepared("watch-notify-silent", "完成", true);
            let mut ticks = [100_000, 100_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &opts(false),
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(
                !text.contains(expected),
                "沒給 --notify 卻承諾了訊號：{text}"
            );
        }

        /// **預演沒有收尾可以通知，所以那句承諾不可以印出來。**
        ///
        /// `--dry-run` 印完要送的字就結束，一輪都不會盯。`notification_notice()`
        /// 那一句（「停下來時我會讓工作列那顆按鈕閃一下、響一聲」）原本印在早退
        /// 之前，是一句關於一場永遠不會發生的盯梢的承諾。但也不能就這樣安靜地
        /// 把旗標吃掉——兩條路都要有話講。
        ///
        /// 底下拿的是 `notification_notice()` 的回傳值本身，不是抄一份字串：
        /// 文案改字的那天（alpha.72 就改過一次）這條測試要跟著走，不是變成
        /// 在比對一句產品早就不說了的話。
        #[test]
        fn a_dry_run_does_not_promise_a_signal_it_will_never_send() {
            let promise = sister_core::watch::WatchEnd::notification_notice(cfg!(windows));
            let (tmp, config) = prepared("watch-notify-dry", "完成", true);
            let mut out = Vec::new();
            let mut options = opts(true);
            options.dry_run = true;
            run_with(
                &tmp.0,
                &config,
                &options,
                &mut || 100_000,
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(
                !text.contains(promise),
                "預演不會盯，卻承諾了收尾的時候會通知：{text}"
            );
            assert!(
                text.contains("預演只印出要送的字就結束"),
                "旗標被安靜地吃掉了：{text}"
            );
            assert!(!text.contains('\x07'), "預演不可以響：{text}");
        }

        #[test]
        fn startup_skips_never_write_bell() {
            let (no_consent, config) = prepared("watch-skip-consent", "完成", false);
            let (no_command, mut commandless) = prepared("watch-skip-command", "完成", true);
            commandless.brain.command.clear();
            let (no_budget, mut full) = prepared("watch-skip-budget", "完成", true);
            full.brain.daily_budget = 0;
            for (dir, config) in [
                (&no_consent.0, &config),
                (&no_command.0, &commandless),
                (&no_budget.0, &full),
            ] {
                let mut out = Vec::new();
                run_with(
                    dir,
                    config,
                    &opts(true),
                    &mut || 100_000,
                    &mut |_| {},
                    &mut out,
                )
                .expect("skip");
                assert!(!out.contains(&b'\x07'), "開跑即跳過卻通知了：{out:?}");
            }
        }

        /// **一整輪都叫不起來那支 CLI，收尾不可以說「沒有等到」。**
        ///
        /// 那是一句斷言，而她一次都沒有真的問到答案。這條測試走的是真的
        /// `run_with`：`brain.command` 指向一個不存在的執行檔，於是每一輪都是
        /// spawn 失敗；畫面上每一輪都誠實地說「這一輪不算數」，而舊版的收尾
        /// 把那三輪加總成「問了 3 次……時間到了，沒有等到」。
        #[test]
        fn a_whole_run_of_failed_spawns_does_not_add_up_to_a_verdict() {
            let (tmp, mut config) = prepared_at("watch-all-failed", 95_000, "第一段", true);
            // 每一輪都要有新的字，不然中間那幾輪會變成「沒東西可問」，
            // 而這條測試量的是「送出去了但沒拿到答案」那一格。
            add_chunk(&tmp.0, 125_000, "第二段");
            add_chunk(&tmp.0, 155_000, "第三段");
            config.brain.command = "definitely-not-a-real-binary-97531".into();
            config.brain.args = vec![];
            // 第一格是 `started`，後面三格才是三輪。
            let mut ticks = [100_000_i64, 100_000, 130_000, 160_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("根本沒問到大腦"), "{text}");
            assert!(
                !text.contains("沒有等到"),
                "她一次都沒問到答案，這句斷言說不出口：{text}"
            );
            assert!(text.contains("我不知道"), "{text}");
            // 而且那幾次都是真的送出去過（有花預算、有寫紀錄），所以不可以
            // 被算進「沒有新畫面可看」那一格。
            assert!(text.contains("沒拿到答案 3 次"), "{text}");
            assert!(text.contains("問到答案 0 次"), "{text}");
        }

        /// **到期那一刻要再看最後一眼。**
        ///
        /// 那一段字是在第 55 秒落地的，而上一輪在第 30 秒就查完了。舊版一到期
        /// 就直接印收尾，`[130_001, deadline)` 從來沒有被查過——一句「沒有等
        /// 到」，而證據躺在她自己的資料表裡。
        #[test]
        fn the_last_slice_before_the_deadline_still_gets_looked_at() {
            let (tmp, config) = prepared_at("watch-last-slice", 155_000, "build 完成", true);
            let mut ticks = [100_000_i64, 130_000, 160_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "編譯跑完了嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("等到了"), "最後那一段沒被看到：{text}");
            assert!(!text.contains("時間到了，沒有等到"), "{text}");
        }

        /// 她中途收工的話，「沒等到」只算到她停下來為止。
        #[test]
        fn a_deadline_reached_after_she_stopped_says_so() {
            let (tmp, mut config) = prepared("watch-hopeless", "字", true);
            // 大腦要回「還沒」，不然第一輪就等到了、根本走不到到期那一刻。
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            sister_core::heartbeat::beat_thinking(&tmp.0, 120_000, 200_000).expect("thinking");
            // `prepared` 那一段字在 90_000，第一輪就會被看到；之後每一輪都空手。
            let mut ticks = [100_000_i64, 130_000, 160_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("錄製已停"), "{text}");
            assert!(text.contains("凍住的畫面"), "{text}");
            assert!(
                !text.contains("時間到了，沒有等到"),
                "她中途就不錄了，剩下那段時間不能算進「沒等到」：{text}"
            );
        }

        /// **她還在錄的時候到期，那就是一句乾淨的「沒有等到」。**
        ///
        /// `hopeless` 那一格只有 `true` 那一面被釘住過——把收尾寫死成
        /// `let hopeless = true;` 的話，一個從頭到尾都在錄的正常 run 會在最後
        /// 說「她已經不在錄了，只算到她停下來為止」，而她整段時間都醒著。
        /// 使用者讀到那句話會去重開錄製，然後再等一次。
        #[test]
        fn a_deadline_while_she_is_still_recording_is_a_plain_no() {
            let (tmp, mut config) = prepared("watch-still-recording", "字", true);
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            // 最後那一眼看下去的時候，心跳是新鮮的：她確實還在錄。
            sister_core::heartbeat::beat(&tmp.0, 158_000).expect("beat");
            let mut ticks = [100_000_i64, 100_000, 160_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            // 有問到答案，所以走的不是「一次都沒問到」那一臂——這一行是
            // 上面那句斷言的前提，少了它整條測試會被那一臂接走。
            assert!(text.contains("問到答案 1 次"), "{text}");
            assert!(text.contains("時間到了，沒有等到"), "{text}");
            assert!(
                !text.contains("停下來為止"),
                "她整段時間都在錄，沒有停下來過：{text}"
            );
        }

        /// **一輪新畫面都沒有的時候，那幾輪要如實記在「沒有新畫面可看」那一格。**
        ///
        /// 三個數字加起來就是她跑了幾輪；哪一格漏記，收尾那句話就會少算，
        /// 而使用者正拿它跟開跑時說的「最多問 N 次」對。
        #[test]
        fn a_run_that_never_saw_a_chunk_counts_every_round_as_blind() {
            // 那一段字落在開跑前很久，任何一輪的視窗都碰不到它。
            let (tmp, config) = prepared_at("watch-all-blind", 1_000, "很久以前", true);
            let mut ticks = [100_000_i64, 100_000, 130_000, 160_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("沒有新畫面可看 3 次"), "{text}");
            assert!(text.contains("問到答案 0 次"), "{text}");
            assert!(text.contains("送出去但沒拿到答案 0 次"), "{text}");
            // 一次都沒送出去，所以帳上不可以有紀錄。
            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            let day = sister_core::local_day::local_day_key(100_000).unwrap();
            assert_eq!(db.brain_outbound_count_on(&day).unwrap(), 0);
        }

        /// **時鐘往回跳那一輪，畫面上要講的是「時鐘往回跳了」。**
        ///
        /// 那一輪**什麼都沒查**——區間是反的。接線接到 `blind_reason` 的話，
        /// 她會拿另一種狀態的句子來蓋一次沒發生過的查詢：這裡沒有心跳檔，
        /// 於是那句話會變成「她從來沒有開始錄」，而 recorder 好端端地跑著。
        #[test]
        fn a_backwards_clock_says_so_on_screen() {
            let (tmp, mut config) = prepared("watch-backwards", "字", true);
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            // 第三輪要有字可看，這樣**正常路徑上一次都不會走到 `blind_reason`**
            // ——於是畫面上只要出現它那句「沒有新的畫面可以看」，就一定是往回
            // 跳的那一輪被接到了錯的地方去。
            add_chunk(&tmp.0, 155_000, "第二段");
            // 第二輪的時鐘比第一輪早（NTP 校時／睡眠喚醒）。
            let mut ticks = [100_000_i64, 100_000, 90_000, 160_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("時鐘往回跳"), "{text}");
            assert!(
                !text.contains("沒有新的畫面可以看"),
                "那一輪沒查過任何東西，不可以拿另一種狀態的句子來蓋：{text}"
            );
            assert!(text.contains("沒有新畫面可看 1 次"), "{text}");
            // 往回跳的那一輪一毛錢都不花：帳上只有有字的那兩輪。
            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            let day = sister_core::local_day::local_day_key(100_000).unwrap();
            assert_eq!(db.brain_outbound_count_on(&day).unwrap(), 2, "{text}");
        }

        /// **午夜之後那道預算閘門要讀新的一天的帳，不是開跑那一天的。**
        ///
        /// 上面那條測試釘的是「記在哪一天」（寫），這條釘的是「照哪一天放行」
        /// （讀）。只釘寫的那一面的話，把閘門讀成 `day_of(started)` 照樣全綠：
        /// 昨天的額度滿了，她今天一次都不問，然後說「今天的外送預算先用完了」
        /// ——而今天的帳上是 0。
        #[test]
        fn the_budget_gate_after_midnight_reads_the_new_days_ledger() {
            let started = 1_756_200_000_000_i64;
            let tomorrow = started + 86_400_000;
            let (tmp, mut config) = prepared_at("watch-midnight-gate", started - 5_000, "字", true);
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            // 一天只有兩次，而昨天已經用掉一次。
            config.brain.daily_budget = 2;
            let day_one = sister_core::local_day::local_day_key(started).unwrap();
            let day_two = sister_core::local_day::local_day_key(tomorrow).unwrap();
            assert_ne!(day_one, day_two, "這兩個時刻要落在不同的本地日期");
            // 午夜前一小段落地的字：第二輪（午夜之後）看得到，第三輪已經
            // 滑出視窗——第三輪不再問，帳上的數字才只反映午夜那一輪。
            add_chunk(&tmp.0, tomorrow - 20_000, "午夜之後才被看到的字");
            {
                let mut db = Db::open(&Config::db_path(&tmp.0)).unwrap();
                db.insert_brain_outbound(&OutboundInsert {
                    ts: started - 60_000,
                    day_key: &day_one,
                    command: "sh",
                    args: &[],
                    segment_core_start: None,
                    chars_sent: 1,
                    truncated: false,
                    outcome: "ok",
                    duration_ms: 1,
                    error: None,
                    role: "watcher",
                })
                .unwrap();
            }

            // 第二輪落在午夜之後、而且**還沒到期**，所以撞到牆的話畫面上會是
            // 「預算先用完了」，不會被收尾那句「時間到了」蓋掉。
            let mut ticks = [started, started, tomorrow, tomorrow + 30_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 86_400_000 + 10_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(
                !text.contains("預算先用完"),
                "今天的帳上是 0，她卻拿昨天的額度把自己擋在門外：{text}"
            );
            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            assert_eq!(
                db.brain_outbound_count_on(&day_two).unwrap(),
                1,
                "午夜之後那一輪應該問得出去：{text}"
            );
            assert_eq!(
                db.brain_outbound_count_on(&day_one).unwrap(),
                2,
                "昨天的帳：本來一次，加上午夜前那一輪"
            );
        }

        /// **撤回第二張同意書，正在跑的那一場當場停下來。**
        ///
        /// `docs/PRIVACY.md`：「『隨時』是真的隨時，不是『下次重開』。」
        /// `sister record` 每 5 秒重讀一次就是為了那句話；而在 `sister watch`
        /// 之前，第二張同意書沒有任何長命的持票人（`interpret` 是一次性的）。
        ///
        /// 開跑時鑄一張票然後抱著跑八小時的話：他在另一個視窗按下撤回、螢幕
        /// 上回他「她一次都不會呼叫那支 CLI」（現在式），而這個迴圈在接下來
        /// 幾個小時裡把螢幕原文送出去兩百多次。
        ///
        /// 這條測試量的是**帳上寫了幾列**——撤回之後那幾輪一列都不可以多。
        #[test]
        fn revoking_the_second_sheet_stops_the_run_it_is_already_inside() {
            let (tmp, mut config) = prepared_at("watch-revoke", 95_000, "第一段", true);
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            // 每一輪都有新的字可問，所以「沒有多送」不能靠「沒東西可送」蒙混。
            add_chunk(&tmp.0, 125_000, "第二段");
            add_chunk(&tmp.0, 155_000, "第三段");
            let day = sister_core::local_day::local_day_key(100_000).unwrap();

            // 第一輪問完之後，他在另一個視窗把第二張收回去。
            let dir = tmp.0.clone();
            let mut round = 0;
            let mut ticks = [100_000_i64, 100_000, 130_000, 160_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {
                    round += 1;
                    if round == 1 {
                        let mut consent = sister_core::consent::Consent::default();
                        consent.revoke(sister_core::consent::Sheet::CloudReading);
                        sister_core::consent::save(&dir, &consent).expect("revoke");
                    }
                },
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();

            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            assert_eq!(
                db.brain_outbound_count_on(&day).unwrap(),
                1,
                "撤回之後她又送了：{text}"
            );
            // 而且要講對是哪一件事停下來的。
            assert!(text.contains("第二張同意書被收回了"), "{text}");
            assert!(
                !text.contains("沒有開始盯"),
                "她盯過了，也問到過一次答案：{text}"
            );
            assert!(text.contains("問到答案 1 次"), "{text}");
            assert!(
                !text.contains("時間到了，沒有等到"),
                "停下來的理由不是時間到：{text}"
            );
        }

        fn add_chunk(dir: &Path, ts: i64, text: &str) {
            let db = Db::open(&Config::db_path(dir)).expect("db");
            let session: i64 = db
                .conn()
                .query_row("SELECT MAX(id) FROM sessions", [], |r| r.get(0))
                .expect("session");
            db.conn()
                .execute(
                    "INSERT INTO text_chunks(ts,session_id,source_kind,app_id,text) VALUES(?1,?2,'ocr','Terminal.exe',?3)",
                    (ts, session, text),
                )
                .expect("chunk");
        }

        /// **往回多看的那幾秒不是保險，是必需的。**
        ///
        /// `text_chunks.ts` 是抓那一幀的時間，而那一列要等 OCR 跑完才進得了
        /// 資料庫。游標貼著「現在」的話，每一輪最新的那幾段永遠是在查詢跑完
        /// 之後才落地，而下一輪的起點已經在它後面了——它再也不會被看到。
        #[test]
        fn a_row_that_landed_late_is_still_inside_the_next_window() {
            // 上一輪查到 100_000 為止（游標停在 100_001）。
            let (from, to) = window(100_001, 130_000).expect("時鐘沒有往回跳");
            assert_eq!(to, 130_001);
            // **要擋的是 OCR 那一段延遲，量級是「秒」不是「毫秒」。**
            // 這台機器自己量到的 OCR 是每張 2407.6／2798.1 ms（AGENTS.md 的
            // 兩份 bench），所以往回留的那一段要蓋得住那麼久之前的一列。
            // 只釘 `99_998`（慢兩毫秒）的話，`GRACE = 1_500` 照樣全綠，而
            // 每一列真實的落地延遲都會落在洞裡。
            const OCR_LAG: Millis = 2_800;
            assert!(
                (from..to).contains(&(100_001 - OCR_LAG)),
                "OCR 慢了 {OCR_LAG} 毫秒才落地的那一列就永遠看不到了：{from}..{to}"
            );
            // 但也不能往回看到上上一輪去，那是白花的外送。
            assert!(!(from..to).contains(&90_000), "{from}..{to}");
        }

        /// **時鐘往回跳的時候游標不跟著走。**
        ///
        /// 跟著走的話，她會把跳過的那一段整個重跑一遍：同一段字被送出去第二
        /// 次、第三次，每一次都是一通外送，而預設一天只有 80 次。開跑時說的
        /// 「最多問 N 次」當場變成假話。
        ///
        /// 這條測試走真的 `run_with`，量的是**帳上到底寫了幾列**——自己在測試
        /// 裡把游標的算式再寫一遍的話，量到的是那份抄寫，接線改回
        /// `last_seen = now + 1` 照樣全綠。
        #[test]
        fn a_backwards_clock_does_not_replay_the_span_it_jumped_over() {
            let (tmp, mut config) = prepared("watch-no-replay", "那一段字", true);
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            // 第一輪在 100_000 看到 90_000 那一段（＝第一通外送）。之後時鐘
            // 往回跳到 90_000、再走到 95_000——游標若跟著往回，那兩輪的視窗
            // 會重新蓋住 90_000，同一段字就被再送一次。
            let mut ticks = [100_000_i64, 100_000, 90_000, 95_000, 160_000].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            let day = sister_core::local_day::local_day_key(100_000).unwrap();
            assert_eq!(
                db.brain_outbound_count_on(&day).unwrap(),
                1,
                "同一段字被送出去第二次——時鐘往回跳把游標一起拖回去了：{text}"
            );
        }

        /// **跨過午夜的那一輪要記在新的一天，不是開跑那一天。**
        ///
        /// `--stop-after 8h` 從晚上十點開跑，`day` 若是啟動時算好就不再動，
        /// 午夜之後的每一次外送都會記在昨天的帳上——而昨天的額度早就滿了。
        /// 結果是使用者設的每日上限被悄悄加倍，而 `sister brain log` 上今天那
        /// 一列是 0。`brain::run` 每次進去都重算（`local_day_key(now_ms())`）。
        #[test]
        fn a_run_that_crosses_midnight_bills_the_new_day() {
            let started = 1_756_200_000_000_i64;
            let tomorrow = started + 86_400_000;
            let (tmp, config) =
                prepared_at("watch-midnight", tomorrow - 400_000, "build 完成", true);
            let day_one = sister_core::local_day::local_day_key(started).unwrap();
            let day_two = sister_core::local_day::local_day_key(tomorrow).unwrap();
            assert_ne!(day_one, day_two, "這兩個時刻要落在不同的本地日期");

            let mut ticks = [started, started, tomorrow].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "編譯跑完了嗎".into(),
                    every: 30_000,
                    stop_after: 86_400_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");

            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            assert_eq!(
                db.brain_outbound_count_on(&day_two).unwrap(),
                1,
                "午夜之後那一次要記在新的一天"
            );
            assert_eq!(
                db.brain_outbound_count_on(&day_one).unwrap(),
                0,
                "記在昨天的帳上，等於把使用者設的每日上限偷偷加倍"
            );
        }

        /// 時鐘往回跳的那一輪什麼都沒查，不可以講成「她確實正在錄」。
        #[test]
        fn a_backwards_clock_is_not_a_clean_look() {
            assert_eq!(window(100_001, 90_000), None);
            // 往回跳得比 GRACE 還小的話，區間仍然成立，不要誤報。
            assert!(window(100_001, 99_000).is_some());
        }

        #[test]
        fn a_missing_data_dir_is_not_a_quiet_screen() {
            let dir =
                std::env::temp_dir().join(format!("sister-watch-missing-{}", std::process::id()));
            let mut out = Vec::new();
            let err = run_with(
                &dir,
                &Config::default(),
                &WatchOpts {
                    question: "x".into(),
                    every: 5_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || 1,
                &mut |_| {},
                &mut out,
            )
            .expect_err("missing");
            assert!(
                err.to_string()
                    .contains("不是她沒看到，是我們沒找到那個目錄")
            );
        }

        #[test]
        fn a_too_fast_interval_says_it_was_raised() {
            let (tmp, config) = prepared("watch-fast", "真實畫面", true);
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 5_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: true,
                    notify: false,
                },
                &mut || 100_000,
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            assert!(String::from_utf8(out).unwrap().contains("抬到 30 秒"));
        }

        #[test]
        fn a_truncated_prompt_says_so() {
            let (tmp, config) = prepared(
                "watch-truncated",
                &"畫面原文".repeat(sister_core::brain::MAX_PROMPT_BYTES),
                true,
            );
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: true,
                    notify: false,
                },
                &mut || 100_000,
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            assert!(String::from_utf8(out).unwrap().contains("不是全部"));
        }

        #[test]
        fn a_dry_run_sends_nothing_and_shows_the_real_text() {
            let (tmp, config) = prepared("watch-dry", "PRIVATE 原始畫面文字", true);
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: true,
                    notify: false,
                },
                &mut || 100_000,
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            let day = sister_core::local_day::local_day_key(100_000).unwrap();
            assert_eq!(db.brain_outbound_count_on(&day).unwrap(), 0);
            assert!(
                String::from_utf8(out)
                    .unwrap()
                    .contains("PRIVATE 原始畫面文字")
            );
        }

        #[test]
        fn no_consent_says_which_sheet_and_how_to_sign() {
            let (tmp, config) = prepared("watch-no-consent", "原文", false);
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || 100_000,
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("cloud-reading"));
            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            let day = sister_core::local_day::local_day_key(100_000).unwrap();
            assert_eq!(db.brain_outbound_count_on(&day).unwrap(), 0);
        }

        #[test]
        fn every_outbound_row_says_watcher() {
            let (tmp, config) = prepared("watch-role", "完成", true);
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 60_000,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || 100_000,
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            let day = sister_core::local_day::local_day_key(100_000).unwrap();
            assert_eq!(db.brain_outbound_count_on(&day).unwrap(), 1);
            assert_eq!(db.brain_outbound_count_on_role(&day, "watcher").unwrap(), 1);
            assert!(String::from_utf8(out).unwrap().contains("等到了"));
        }

        /// 「她正在讓大腦讀一段」不是「她收工了」；「開機中」不是「正在錄」。
        ///
        /// `Presence` 有六種，而 `Blind` 那一邊本來只接到四種：`Live(_)` 一個
        /// 底線把開機和穩定錄製併成一句，`Thinking` 被硬塞進 `Stopped`。
        /// 後者的畫面是每兩分鐘跳一句「她在 X 收工了」——她正忙著。
        #[test]
        fn thinking_and_booting_do_not_borrow_another_states_sentence() {
            let tmp = crate::ops::tmp::Tmp::new("watch-presence");
            let now = 1_000_000_i64;

            sister_core::heartbeat::beat_thinking(&tmp.0, now, now + 60_000).expect("thinking");
            let thinking = blind_reason(&tmp.0, now);
            assert_eq!(
                thinking,
                Blind::Thinking {
                    until: now + 60_000
                }
            );
            assert!(!thinking.message().contains("收工"), "{thinking:?}");

            sister_core::heartbeat::beat_booting(&tmp.0, now).expect("booting");
            let booting = blind_reason(&tmp.0, now);
            assert_eq!(booting, Blind::Booting);
            assert_ne!(booting.message(), Blind::RecordingButQuiet.message());

            sister_core::heartbeat::beat(&tmp.0, now).expect("recording");
            assert_eq!(blind_reason(&tmp.0, now), Blind::RecordingButQuiet);

            // 暫停要蓋過心跳：暫停的時候心跳照跳，只問 `presence` 會得到
            // `Live`，於是畫面會說「她確實正在錄」，而她的眼睛是閉著的。
            sister_core::pause::set_paused(&tmp.0, true, now).expect("pause");
            assert_eq!(blind_reason(&tmp.0, now), Blind::Paused);
        }

        fn quiet_config(name: &str) -> (crate::ops::tmp::Tmp, Config) {
            let tmp = crate::ops::tmp::Tmp::new(name);
            let mut consent = sister_core::consent::Consent::default();
            consent.grant(sister_core::consent::Sheet::CloudReading, 1);
            sister_core::consent::save(&tmp.0, &consent).expect("consent");
            let mut config = Config::default();
            config.brain.command = "sh".into();
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            (tmp, config)
        }

        fn run_quiet_once(tmp: &crate::ops::tmp::Tmp, config: &Config, now: Millis) -> String {
            let mut ticks = [now, now].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 0,
                    quiet_for: Some(60_000),
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            String::from_utf8(out).unwrap()
        }

        #[test]
        fn paused_never_masquerades_as_a_quiet_screen() {
            let now = 1_000_000;
            let (tmp, config) = prepared_at("watch-quiet-paused", now - 120_000, "舊字", true);
            sister_core::heartbeat::beat(&tmp.0, now).expect("recording");
            sister_core::pause::set_paused(&tmp.0, true, now).expect("pause");
            let said = run_quiet_once(&tmp, &config, now);
            assert!(said.contains("她被暫停了"), "{said}");
            assert!(!said.contains("畫面上已經"), "暫停不是安靜：{said}");
        }

        #[test]
        fn no_text_ever_is_not_a_gigantic_quiet_span() {
            let now = 1_756_200_000_000;
            let (tmp, config) = quiet_config("watch-quiet-empty");
            Db::open(&Config::db_path(&tmp.0)).expect("db");
            sister_core::heartbeat::beat(&tmp.0, now).expect("recording");
            let said = run_quiet_once(&tmp, &config, now);
            assert!(
                !said.contains("畫面上已經"),
                "從來沒字不能從 epoch 0 起算：{said}"
            );
            assert!(said.contains("沒有新的畫面可以看"), "{said}");
        }

        #[test]
        fn a_stopped_or_thinking_recorder_is_not_a_quiet_screen() {
            let now = 1_000_000;
            for (name, thinking) in [
                ("watch-quiet-stopped", false),
                ("watch-quiet-thinking", true),
            ] {
                let (tmp, config) = prepared_at(name, now - 120_000, "舊字", true);
                if thinking {
                    sister_core::heartbeat::beat_thinking(&tmp.0, now, now + 60_000)
                        .expect("thinking");
                } else {
                    sister_core::heartbeat::stop(&tmp.0, now - 30_000);
                }
                let said = run_quiet_once(&tmp, &config, now);
                assert!(!said.contains("畫面上已經"), "收工或收尾不是安靜：{said}");
                assert!(
                    said.contains("收工") || said.contains("錄製已停"),
                    "要沿用 Blind 原句：{said}"
                );
            }
        }

        #[test]
        fn a_truly_quiet_recording_ends_locally_with_last_text_evidence() {
            let now = 1_756_200_000_000;
            let last = now - 12 * 60_000;
            let (tmp, config) = prepared_at("watch-went-quiet", last, "舊字", true);
            sister_core::heartbeat::beat(&tmp.0, now).expect("recording");
            let said = run_quiet_once(&tmp, &config, now);
            assert!(said.contains("12 分鐘"), "{said}");
            assert!(said.contains("Terminal.exe"), "{said}");
            assert!(said.contains(&sister_core::model::stamp(last)), "{said}");
            assert!(!said.contains(&last.to_string()), "{said}");
            assert!(!said.contains("卡住"), "不准把觀察講成診斷：{said}");
            assert!(said.contains("我只知道畫面沒有動"), "{said}");
            let db = Db::open(&Config::db_path(&tmp.0)).unwrap();
            assert_eq!(
                db.brain_outbound_count_on(&day_of(now).unwrap()).unwrap(),
                0,
                "本機 quiet 判斷不可以吃外送預算"
            );
        }

        /// **一個安靜地什麼都不做的旗標，比沒有這個旗標更糟。**
        ///
        /// 那個判斷掛在「這一輪的視窗裡一段字都沒有」上，而視窗就是一個
        /// `every`。所以 `--quiet-for 30s --every 2m` 之下它一次都不會觸發——
        /// 使用者設了、以為她在盯，然後等了一小時才發現那一行從來沒出現過。
        /// 抬到量得出來的最短值，而且**要講**：安靜地夾住等於同一個謊。
        #[test]
        fn a_quiet_threshold_shorter_than_the_interval_is_raised_out_loud() {
            let now = 1_756_200_000_000_i64;
            // 畫面安靜了 90 秒；門檻設 30 秒，而她兩分鐘才看一次。
            let (tmp, config) = prepared_at("watch-quiet-raised", now - 90_000, "舊字", true);
            sister_core::heartbeat::beat(&tmp.0, now).expect("recording");
            let mut ticks = [now, now].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 120_000,
                    stop_after: 0,
                    quiet_for: Some(30_000),
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let said = String::from_utf8(out).unwrap();
            assert!(
                said.contains("已抬到 2 分鐘"),
                "安靜地夾住等於同一個謊：{said}"
            );
            assert!(said.contains("我量不出來"), "{said}");
            // **講出來的那個數字要是她真的會用的那一個。** 抬了門檻卻拿沒抬的
            // 那個去講，就是一個變數回答兩個問題。
            assert!(
                said.contains("畫面連續 2 分鐘沒有新的字"),
                "說明用的是抬起來之前的值：{said}"
            );
            assert!(!said.contains("畫面連續 30 秒"), "{said}");
        }

        /// 抬起來之後**真的生效的是抬過的那個門檻**，而報出來的時長是
        /// 她量到的那一段，不是門檻本身。
        #[test]
        fn the_raised_threshold_is_the_one_that_actually_fires() {
            let now = 1_756_200_000_000_i64;
            // 畫面安靜了 200 秒（＞抬起來的 120 秒門檻）。
            let last = now - 200_000;
            let (tmp, mut config) = prepared_at("watch-quiet-fires", last, "舊字", true);
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            sister_core::heartbeat::beat(&tmp.0, now).expect("recording");
            let mut ticks = [now, now].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 120_000,
                    stop_after: 0,
                    quiet_for: Some(30_000),
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let said = String::from_utf8(out).unwrap();
            assert!(said.contains("畫面上已經"), "門檻抬過之後沒有生效：{said}");
            // 報的是量到的 200 秒（＝3 分鐘），不是門檻的 2 分鐘。
            assert!(
                said.contains("畫面上已經 3 分鐘沒有出現新的字"),
                "報出來的是門檻不是量到的那一段：{said}"
            );
            assert!(said.contains(&sister_core::model::stamp(last)), "{said}");
            assert!(!said.contains("卡住"), "觀察不可以冒充診斷：{said}");
        }

        /// 時長只有一份定義：90 分鐘在 `watch` 和在別的命令要是同一句話。
        #[test]
        fn the_quiet_span_reads_the_same_as_every_other_span_in_the_product() {
            let ninety = 90 * 60_000;
            assert_eq!(crate::fmt::duration_ms(ninety), "1 小時 30 分");
            let said = sister_core::watch::WatchEnd::WentQuiet {
                tally: Default::default(),
                quiet_for: ninety,
                last_at: 1_756_200_000_000,
                last_app: None,
            }
            .message();
            assert!(
                said.contains(&crate::fmt::duration_ms(ninety)),
                "watch 自己寫了第二份時長格式：{said}"
            );
        }

        #[test]
        fn omitting_quiet_for_keeps_the_existing_watch_path() {
            let now = 1_000_000;
            let (tmp, config) = prepared_at("watch-no-quiet-option", now - 120_000, "舊字", true);
            sister_core::heartbeat::beat(&tmp.0, now).expect("recording");
            let mut ticks = [now, now].into_iter();
            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 0,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || ticks.next().expect("fake clock ran out"),
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let said = String::from_utf8(out).unwrap();
            assert!(
                !said.contains("畫面上已經"),
                "沒給選項不能啟動新行為：{said}"
            );
            assert!(said.contains("時間到了"), "應走原本 Deadline：{said}");
        }

        /// 一個間隔內字太多的時候，要拿**最新的**那幾段，而且要講出有沒有漏。
        ///
        /// `chunks_in_range` 是 `ORDER BY ts ASC LIMIT n`，也就是最舊的 n 段。
        /// 一個正在編譯、狂吐訊息的終端機，兩分鐘內輕鬆超過 n 段——於是她永遠
        /// 在讀兩分鐘前的畫面，而「All checks passed」就在她沒看的那一頭。
        /// 更糟的是拿來報 app 的那個變數叫 `newest`。
        #[test]
        fn a_noisy_screen_is_read_from_the_newest_end_and_says_what_it_skipped() {
            let tmp = crate::ops::tmp::Tmp::new("watch-newest");
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let session = db.start_session("test", "test").expect("session");
            for n in 0..10_i64 {
                db.conn()
                    .execute(
                        "INSERT INTO text_chunks(ts,session_id,source_kind,app_id,text) VALUES(?1,?2,'ocr','Terminal.exe',?3)",
                        (1_000 + n, session, format!("第 {n} 行")),
                    )
                    .expect("chunk");
            }

            let (hits, more) = newest_since(&db, 0, 2_000, 3).expect("newest");
            assert!(more, "十段只拿三段，要講出還有更多");
            assert_eq!(hits.len(), 3);
            // 由舊到新排好，而且是**最新的**那三段，不是最舊的三段。
            let texts: Vec<&str> = hits.iter().map(|h| h.text.as_str()).collect();
            assert_eq!(texts, vec!["第 7 行", "第 8 行", "第 9 行"], "{texts:?}");
            assert_eq!(
                hits.last().expect("nonempty").ts,
                1_009,
                "最後一段要真的是最新的那一段——報 app 和報時刻都靠它"
            );

            // 全部拿得下的時候不要憑空說有漏。
            let (all, more) = newest_since(&db, 0, 2_000, 50).expect("newest");
            assert_eq!(all.len(), 10);
            assert!(!more);
        }

        /// **真的送出去那一輪，畫面上那個段數要是「真的送出去幾段」。**
        ///
        /// 上面那條走的是 `--dry-run`，而 dry-run **根本不會建 `Look::Asked`**
        /// ——那句「看了 N 段字，最新的來自 X」是真實那一輪才印的。所以把
        /// `chunks: prompt.included_chunks` 改回 `chunks: hits.len()`，
        /// dry-run 那條照樣綠，而畫面上會說「看了 206 段字」，其中一百多段
        /// 從來沒有離開這台機器。
        ///
        /// 這正是這個 bug 原本的形狀：`newest_since` 挑最新的 200 段、位元組
        /// 上限又砍掉一大半，然後那句話替全部 206 段作證。
        #[test]
        fn a_real_round_counts_the_chunks_it_actually_sent_not_the_ones_it_saw() {
            let (tmp, mut config) =
                prepared_at("watch-real-newest-bytes", 80_000, "oldest-marker", true);
            config.brain.args = vec![
                "-c".into(),
                "printf '%s' '{\"happened\":false,\"because\":\"\"}'".into(),
            ];
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let session = db.start_session("test-2", "test").expect("session");
            for n in 1..=205_i64 {
                db.conn()
                    .execute(
                        "INSERT INTO text_chunks(ts,session_id,source_kind,app_id,text) VALUES(?1,?2,'ocr','Terminal.exe',?3)",
                        (80_000 + n, session, format!("marker-{n:03}-{}", "x".repeat(150))),
                    )
                    .expect("chunk");
            }
            drop(db);

            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 0,
                    quiet_for: None,
                    dry_run: false,
                    notify: false,
                },
                &mut || 100_000,
                &mut |_| {},
                &mut out,
            )
            .expect("run");
            let said = String::from_utf8(out).unwrap();

            // 有東西沒送，這件事要講出來。
            assert!(
                said.contains("證據上限只放得下"),
                "省略了一百多段卻沒講：{said}"
            );
            // 而且「看了 N 段」的 N 不可以是畫面上那個 N。
            assert!(
                !said.contains("看了 206 段字") && !said.contains("看了 200 段字"),
                "那句話在替沒有離開這台機器的字作證：{said}"
            );
            let sent = said
                .split("送出去的是最新的 ")
                .nth(1)
                .and_then(|rest| rest.split(' ').next())
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or_else(|| panic!("找不到實際送出的段數：{said}"));
            assert!(sent > 0 && sent < 200, "實際送出 {sent} 段，不合理：{said}");
            assert!(
                said.contains(&format!("看了 {sent} 段字")),
                "「送出去的是最新的 {sent} 段」和「看了 N 段字」對不起來：{said}"
            );
        }

        #[test]
        fn dry_run_prints_the_newest_byte_limited_evidence_and_both_omissions() {
            let (tmp, config) =
                prepared_at("watch-dry-newest-bytes", 80_000, "oldest-marker", true);
            let mut db = Db::open(&Config::db_path(&tmp.0)).expect("db");
            let session = db.start_session("test-2", "test").expect("session");
            for n in 1..=205_i64 {
                db.conn()
                    .execute(
                        "INSERT INTO text_chunks(ts,session_id,source_kind,app_id,text) VALUES(?1,?2,'ocr','Terminal.exe',?3)",
                        (80_000 + n, session, format!("marker-{n:03}-{}", "x".repeat(150))),
                    )
                    .expect("chunk");
            }
            drop(db);

            let mut out = Vec::new();
            run_with(
                &tmp.0,
                &config,
                &WatchOpts {
                    question: "完成嗎".into(),
                    every: 30_000,
                    stop_after: 0,
                    quiet_for: None,
                    dry_run: true,
                    notify: false,
                },
                &mut || 100_000,
                &mut |_| {},
                &mut out,
            )
            .expect("dry run");
            let said = String::from_utf8(out).unwrap();
            assert!(said.contains("字超過 200 段"), "列上限的省略沒有講：{said}");
            assert!(
                said.contains("畫面證據超過上限"),
                "位元組省略沒有講：{said}"
            );
            assert!(said.contains("marker-205-"), "最新一段沒有送出：{said}");
            assert!(!said.contains("oldest-marker"), "最舊一段不該送出：{said}");
            let older = said.find("marker-204-").expect("倒數第二段");
            let newest = said.find("marker-205-").expect("最新段");
            assert!(older < newest, "證據沒有由舊到新：{said}");
        }
    }
}

pub mod commitments {
    use super::*;

    pub fn run(
        data_dir: &Path,
        kill: Option<i64>,
        other: Option<i64>,
        note: Option<&str>,
        json: bool,
    ) -> Result<()> {
        let mut db = open_existing(data_dir)?;
        let now = sister_core::now_ms();
        if let Some(id) = kill {
            let n = sister_core::reviewer::kill_commitment(
                &mut db,
                id,
                note.unwrap_or("使用者結案"),
                now,
            )?;
            anyhow::ensure!(n > 0, "找不到還活著的承諾 #{id}");
            println!("承諾 #{id} 已結案。");
            return Ok(());
        }
        if let Some(id) = other {
            let n = sister_core::reviewer::snooze_commitment(&mut db, id, now)?;
            anyhow::ensure!(n > 0, "找不到還活著的承諾 #{id}");
            println!("承諾 #{id} 已降權（snooze）。不會再煩你。");
            return Ok(());
        }
        let rows = db.all_commitments()?;
        if json {
            println!("{}", serde_json::to_string_pretty(&rows_json(&rows))?);
            return Ok(());
        }
        if rows.is_empty() {
            println!("承諾表是空的。審閱層跑過之後才會有列。");
            return Ok(());
        }
        for c in &rows {
            let due = match (c.due_hint.as_deref(), c.due_source.as_deref()) {
                (Some(h), Some("explicit")) => format!("　期限 {h}（螢幕上寫的）"),
                (Some(h), Some("inferred")) => format!("　期限 {h}（她從上下文猜的）"),
                (Some(h), _) => format!("　期限 {h}"),
                (None, _) => String::new(),
            };
            let grave = if c.tombstoned_at.is_some() {
                "　〔墓碑：這段原件被忘掉了〕"
            } else {
                ""
            };
            println!("#{}  [{}] {}{}{}", c.id, c.status, c.text, due, grave);
        }
        Ok(())
    }

    fn rows_json(rows: &[sister_core::db::CommitmentRow]) -> serde_json::Value {
        serde_json::json!(
            rows.iter()
                .map(|c| serde_json::json!({
                    "id": c.id,
                    "text": c.text,
                    "kind": c.kind,
                    "status": c.status,
                    "due_hint": c.due_hint,
                    "due_source": c.due_source,
                    "tombstoned": c.tombstoned_at.is_some(),
                }))
                .collect::<Vec<_>>()
        )
    }
}

pub mod pause {
    use super::*;

    /// `sister pause` / `sister resume`。
    ///
    /// 存在的理由不只是「沒有 GUI 的時候也能停」：它讓「暫停」這件事在一台
    /// 無頭機器上就驗得完（旗標寫了沒、`record` 有沒有真的停），不必等到有人
    /// 在 Windows 上開得起字母人。
    pub fn run(data_dir: &Path, paused: bool) -> Result<()> {
        let before = sister_core::pause::is_paused(data_dir);
        sister_core::pause::set_paused(data_dir, paused, sister_core::now_ms())
            .with_context(|| format!("寫不進 {}", data_dir.display()))?;

        // 「本來就是這樣」和「剛剛被我改掉」要分得出來。前者常常代表使用者
        // 記錯了自己上次按了什麼，而那正是需要講清楚的時刻。
        match (before, paused) {
            (false, true) => println!(
                "⏸ 已暫停。正在跑的 `sister record` 會在下一個 tick 停下來，\
                 而且**不會自己恢復**——要她繼續請跑 `sister resume`。"
            ),
            (true, true) => {
                let since = sister_core::pause::paused_since(data_dir)
                    .map(|ts| format!("（從 {} 起）", crate::fmt::timestamp(ts)))
                    .unwrap_or_default();
                println!("⏸ 本來就在暫停中{since}，沒有變動。");
            }
            (true, false) => println!("▶ 已解除暫停。她從下一個 tick 開始重新記錄。"),
            (false, false) => println!("▶ 本來就沒有暫停，沒有變動。"),
        }
        Ok(())
    }
}

pub mod mark {
    use super::*;

    /// `sister mark`。
    ///
    /// 這個子命令存在的理由和 [`stop`] 一樣：那個按鈕在字母人上，而字母人在
    /// CI 開不起來。少了它，「標記一次魔法時刻」整條路唯一的入口會在一個沒有
    /// 人測得到的 GUI 開關後面——而它守的是 Phase 1 的**第一條**退場條件。
    ///
    /// 順帶它也是終端機使用者唯一的入口：`sister query 電話` 給了答案之後，
    /// 「這個我早就忘了」在他還坐在那裡的時候只有這一條路講得出來。
    /// `query_log`：他有沒有讓她記題庫。**空題庫有兩種，而它們的下一步相反。**
    ///
    /// 那個勾關著的時候，`sister query 電話` 照樣答得出來，只是不留下題目——
    /// 於是一個問了一整天的人跑 `sister mark`，會被告訴「先問她一題」。那句話
    /// 的每一個字都對，而它指向的下一步他已經做過一百次了。
    pub fn run(data_dir: &Path, id: Option<i64>, marked: bool, query_log: bool) -> Result<()> {
        let db = open_existing(data_dir)?;

        // 沒給題號就是「剛剛那一題」。**分得出三種**：那個勾關著、沒問過、
        // 打錯號碼。前兩種以前共用一句「先問她一題」。
        //
        // 那個勾關著的時候，「剛剛那一題」這個概念本身就不存在了——她照樣答得
        // 出來，只是一個字都沒留下。所以這一關要擋在**撈之前**，不能只擋在
        // 「撈不到」的那條岔路上：上一版寫成 `last_query().with_context(…)`，
        // 於是題庫裡只要還躺著一列關掉那個勾之前的舊題，這句話就整個被跳過，
        // `sister mark` 會安靜地標到那一題、印一句「記下來了」，然後那一次
        // 假的魔法時刻就算進退場條件裡。**這一格是補不回來的證據，寧可不記。**
        // 給了 `--id` 就是另一回事：他指名了一題，那一題也真的在。
        if !query_log && id.is_none() {
            // **最後那一句要先看一眼再說。** 它上一版是寫死的，於是一顆題庫空
            // 空如也的資料庫也會被告知「題庫裡還躺著關掉之前的舊題」，然後被
            // 指去 `sister queries` 看一個不存在的題號——而下一個指令會當場打
            // 臉（「題庫是空的」）。錯的方向剛好是最糟的那個：它叫他去做一件
            // 做不到的事，而做不到的時候他會以為是這個功能壞了。
            //
            // 問法就用 `last_query()`：底下那條沒關勾的路問的是同一件事，同一
            // 個函式。多開一個「有沒有舊題」的查法，就是同一個概念的第二份答
            // 案，而這個 repo 已經為那件事付過帳。
            let stale = db.last_query()?;
            anyhow::bail!(
                "`privacy.query_log` 是關著的，所以你剛剛問的那一題一個字都沒有留下來——\
                 標記是掛在題目上的，沒有題目就沒有地方掛。\n\
                 要記這一格的話，把那個勾打開（設定頁的「你問過她什麼」），\
                 從下一題開始才留得住。\n{}",
                match stale {
                    Some(q) => format!(
                        "（題庫裡還躺著關掉之前的舊題，最近的一題是 #{} 「{}」。\
                         要標的是那裡面的某一題的話，`sister queries` 看題號，\
                         再 `sister mark --id N`。）",
                        q.id,
                        crate::fmt::one_line(&q.question, 40)
                    ),
                    None =>
                        "（題庫是空的，關掉之前的舊題也一題都沒有，所以現在沒有任何一題標得到。）"
                            .to_string(),
                }
            );
        }
        let row = match id {
            Some(id) => db.query_by_id(id)?.with_context(|| {
                format!(
                    "題庫裡沒有 #{id}。`sister queries` 看得到有哪幾題——\
                     如果它本來在，那就是被 `sister forget` 帶走、或者過了保留期了。"
                )
            })?,
            None => db.last_query()?.with_context(|| {
                "還沒有任何一題可以標記。先問她一題（`sister query …` 或字母人的搜尋框），\
                 標記是掛在題目上的。"
                    .to_string()
            })?,
        };

        for line in mark_lines(row.id, &row.question, db.mark_query(row.id, marked)?) {
            println!("{line}");
        }
        Ok(())
    }

    /// 標記完講的那一兩句話。
    ///
    /// **印出那一題本身。** 不帶題號的那條路標到的是「最近那一題」，而他心裡
    /// 的「剛剛那一題」不一定是同一個——中間從字母人問過一題就分岔了。一句光禿
    /// 禿的「標好了」在那個情況下是對的字配著錯的事。
    ///
    /// **四種，不是兩種。** `changed` 那一半同樣是「兩個狀態印同一句話」：
    /// `--undo` 一個**存在但本來就沒標**的題號，上一版和真的收回一個標記印得
    /// 一模一樣。而那正是比較常打錯的那一種——`sister queries` 就把 `#N` 印在
    /// 旁邊——打錯的人會看到成功，然後不再回頭查，他真正的那個標記還掛在別的
    /// 題上、還算在退場條件裡。
    ///
    /// 是個回傳字串的函式而不是一串 `println!`，因為這四句話是這個子命令**唯
    /// 一**的產出：印出去就沒有人驗得到它們互相分得開。
    fn mark_lines(id: i64, question: &str, out: sister_core::db::MarkOutcome) -> Vec<String> {
        let q = crate::fmt::one_line(question, 40);
        match (out.marked, out.changed) {
            (true, true) => vec![
                format!("★ #{id} 「{q}」——記下來了：這一題你本來已經忘了。"),
                format!("  標錯了的話：sister mark --undo --id {id}"),
            ],
            (true, false) => vec![
                format!("★ #{id} 「{q}」——這一題你本來就標著了，沒有再算一次。"),
                format!("  收回的話：sister mark --undo --id {id}"),
            ],
            (false, true) => vec![format!("○ #{id} 「{q}」——收回了，這一題不再算在裡面。")],
            (false, false) => vec![
                format!("○ #{id} 「{q}」——這一題本來就沒有標記，沒有東西可以收回。"),
                "  標記在哪幾題：sister queries --marked".to_string(),
            ],
        }
    }

    #[cfg(test)]
    mod mark_tests {
        use super::*;
        use sister_core::db::{QueryLogEntry, SOURCE_CLI};

        /// 一顆有題庫的資料目錄。回傳題號，新的在後。
        fn asked(dir: &Path, questions: &[&str]) -> Vec<i64> {
            let db = Db::open(&crate::db_path(dir)).expect("open");
            questions
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    db.log_query(&QueryLogEntry {
                        ts: 1_000 + i as i64,
                        question: q,
                        shape: "keywords",
                        hits: 2,
                        latency_ms: 1,
                        source: SOURCE_CLI,
                    })
                    .expect("log")
                })
                .collect()
        }

        /// 不帶題號標到的是**最近那一題**，而且那一題真的被標起來。
        #[test]
        fn marking_without_an_id_marks_the_one_he_just_asked() {
            let t = crate::ops::tmp::Tmp::new("mark-last");
            let ids = asked(&t.0, &["先問的", "後問的"]);

            run(&t.0, None, true, true).expect("mark");

            let db = Db::open(&crate::db_path(&t.0)).expect("open");
            let marked = db.marked_queries(10).expect("marked");
            assert_eq!(marked.len(), 1);
            assert_eq!(marked[0].id, ids[1], "標到的不是剛剛問的那一題");
            assert_eq!(marked[0].question, "後問的");
        }

        /// `--undo` 走的是同一條路，收回的是同一題。
        #[test]
        fn undo_takes_the_same_question_back_off() {
            let t = crate::ops::tmp::Tmp::new("mark-undo");
            let ids = asked(&t.0, &["會被標起來再收回"]);

            run(&t.0, Some(ids[0]), true, true).expect("mark");
            run(&t.0, Some(ids[0]), false, true).expect("undo");

            let db = Db::open(&crate::db_path(&t.0)).expect("open");
            assert_eq!(db.query_log_stats().expect("stats").marked, 0);
        }

        /// **「還沒問過」和「題號打錯了」要講不同的話。**
        ///
        /// 兩種的下一步是相反的：一個要他去問一題，一個要他去查號碼。共用一句
        /// 「找不到」的話，一個剛裝好的人會被送去查一個他從來沒有過的號碼。
        #[test]
        fn nothing_to_mark_and_wrong_number_are_different_sentences() {
            let empty = crate::ops::tmp::Tmp::new("mark-none");
            // 一顆有資料庫、但一題都沒問過的。
            Db::open(&crate::db_path(&empty.0)).expect("open");
            let no_questions = run(&empty.0, None, true, true).expect_err("沒得標要炸");
            assert!(
                no_questions.to_string().contains("先問她一題"),
                "沒問過的時候要告訴他去問一題：{no_questions}"
            );

            let used = crate::ops::tmp::Tmp::new("mark-wrong-id");
            asked(&used.0, &["有這一題"]);
            let wrong = run(&used.0, Some(999), true, true).expect_err("題號不對要炸");
            let wrong = wrong.to_string();
            assert!(wrong.contains("999"), "要講出是哪一個題號：{wrong}");
            assert!(
                !wrong.contains("先問她一題"),
                "他問過了，不該叫他再去問一題：{wrong}"
            );
        }

        /// **第三種空題庫：那個勾關著。**
        ///
        /// 他問了一整天，`sister query` 每一次都答得出來，只是一題都沒留下。
        /// 這時候一句「先問她一題」的每一個字都對，而它指向的下一步他已經做過
        /// 一百次了——真正要動的是設定檔那個勾。這一種**講得出是哪一種**（設定
        /// 就在手上），所以不攤可能性，直接說。
        #[test]
        fn a_query_log_that_is_switched_off_is_not_a_man_who_never_asked() {
            let t = crate::ops::tmp::Tmp::new("mark-log-off");
            Db::open(&crate::db_path(&t.0)).expect("open");

            let off = run(&t.0, None, true, false).expect_err("沒得標要炸");
            let off = off.to_string();
            assert!(off.contains("query_log"), "要講出是哪個開關：{off}");
            assert!(
                !off.contains("先問她一題"),
                "他問過了，不該叫他再去問一題：{off}"
            );
        }

        /// 標記過的那一題，在 `sister queries` 那張清單上讀得回來。
        ///
        /// 這一條把兩個 surface 釘在一起：`mark` 寫進去的東西，`queries` 要看
        /// 得見。分開跑的話，一個「寫進另一張表」的改法可以讓兩邊都綠。
        #[test]
        fn what_mark_writes_is_what_queries_reads() {
            let t = crate::ops::tmp::Tmp::new("mark-roundtrip");
            let ids = asked(&t.0, &["甲", "乙"]);
            run(&t.0, Some(ids[0]), true, true).expect("mark");

            let db = Db::open(&crate::db_path(&t.0)).expect("open");
            let log = db.query_log(10).expect("log");
            let by_id = |id: i64| log.iter().find(|r| r.id == id).expect("在清單上");
            assert!(by_id(ids[0]).marked(), "標了的那一題在清單上沒有標記");
            assert!(!by_id(ids[1]).marked(), "沒標的那一題被標起來了");
            assert_eq!(db.query_log_stats().expect("stats").marked, 1);
        }

        /// **那個勾關著、而題庫裡還躺著舊題的時候，不准標到那一題。**
        ///
        /// 上一版的防呆寫在 `last_query().with_context(…)` 裡，只有**空**題庫
        /// 走得到。於是一個關掉那個勾、問了一整天的人跑 `sister mark`，會拿到
        /// 一句「★ #1「昨天那一題」——記下來了」：對的字，錯的題，而那一次假的
        /// 魔法時刻就算進 Phase 1 的第一條退場條件裡。
        ///
        /// 這一格是補不回來的證據，寧可不記。給了 `--id` 是另一回事——他指名
        /// 了一題，那一題也真的在，所以那條路照走。
        #[test]
        fn a_switched_off_log_must_not_hand_him_yesterdays_question() {
            let t = crate::ops::tmp::Tmp::new("mark-log-off-stale");
            let ids = asked(&t.0, &["關掉那個勾之前問的"]);

            let err = run(&t.0, None, true, false).expect_err("不可以標到舊的那一題");
            let err = err.to_string();
            assert!(err.contains("query_log"), "要講出是哪個開關：{err}");
            // **這一條要斷在會變的那一半上。** 上一版斷的是 `err.contains("--id")`，
            // 而那句話是寫死的常數——一顆空題庫也照樣印得出來，於是這個斷言在
            // 「舊題還在」和「一題都沒有」兩種情形下都是綠的，量到的是零。
            // 底下那條空題庫的測試現在釘著反面，兩條一起才分得出綠和紅。
            assert!(
                err.contains("關掉之前的舊題，最近的一題是") && err.contains("關掉那個勾之前問的"),
                "舊題還在，要講得出是哪一題：{err}"
            );

            let db = Db::open(&crate::db_path(&t.0)).expect("open");
            assert_eq!(
                db.query_log_stats().expect("stats").marked,
                0,
                "什麼都不該被標起來"
            );

            // 指名的那一條照走：他自己講出是哪一題，就不是「剛剛那一題」了。
            run(&t.0, Some(ids[0]), true, false).expect("指名的標得到");
            assert_eq!(db.query_log_stats().expect("stats").marked, 1);
        }

        /// **題庫空的時候，同一句話不可以宣布有舊題躺在那裡。**
        ///
        /// 上一條的錯誤訊息最後那一句是寫死的：「題庫裡還躺著關掉之前的舊題…
        /// `sister queries` 看題號」。一顆一題都沒問過、而 `query_log` 關著的
        /// 資料庫照樣拿得到它——然後他去跑 `sister queries`，得到「題庫是空
        /// 的」。前一句叫他去做的事，後一句說做不到。
        ///
        /// 這兩條測試是一對：上面那條釘「有舊題就要講出是哪一題」，這條釘
        /// 「沒有就不准說有」。少了任何一條，那句話都可以退回成一個常數。
        #[test]
        fn an_empty_log_must_not_be_told_old_questions_are_waiting() {
            let t = crate::ops::tmp::Tmp::new("mark-log-off-empty");
            // 開一顆有資料庫、但一題都沒問過的：`asked` 一列都不給。
            asked(&t.0, &[]);

            let err = run(&t.0, None, true, false)
                .expect_err("那個勾關著就不該標得成")
                .to_string();
            assert!(err.contains("query_log"), "要講出是哪個開關：{err}");
            assert!(
                !err.contains("還躺著"),
                "一題都沒有，不可以說題庫裡還躺著舊題：{err}"
            );
            assert!(err.contains("題庫是空的"), "要講出真正的狀況：{err}");
        }

        /// **收回一個本來就沒標的題號，不可以講得像收回了一個標記。**
        ///
        /// `#N` 現在印在 `sister queries` 上，所以打錯號碼是走得到的——而打錯
        /// 的人拿到一句「○ 收回了」之後就不會再查，他真正的那個標記還掛在別的
        /// 題上、還算在退場條件裡。兩句話一模一樣，下一步相反。
        #[test]
        fn taking_back_a_mark_that_was_never_there_says_so() {
            use sister_core::db::MarkOutcome;
            let said = |marked, changed| {
                mark_lines(7, "打錯的那一題", MarkOutcome { marked, changed }).join("\n")
            };

            let real = said(false, true);
            let noop = said(false, false);
            assert_ne!(real, noop, "真的收回和沒東西可收回印出一模一樣的話");
            assert!(
                noop.contains("本來就沒有標記"),
                "要講出這一題本來就沒標：{noop}"
            );
            assert!(
                real.contains("收回了，這一題不再算在裡面"),
                "真的收回了要講出來：{real}"
            );

            // 標的那一邊同理：重按一次不是「又記了一次」。
            let fresh = said(true, true);
            let again = said(true, false);
            assert_ne!(fresh, again, "第一次標和重按印出一模一樣的話");
            assert!(again.contains("本來就標著"), "重按要講出來：{again}");

            // 四種都講得出是哪一題——那是這幾句話存在的第一個理由。
            for s in [&real, &noop, &fresh, &again] {
                assert!(s.contains("#7") && s.contains("打錯的那一題"), "{s}");
            }
        }

        /// 打錯號碼收回的那一次，不可以動到真的那一個標記。
        #[test]
        fn undoing_the_wrong_number_leaves_the_real_mark_alone() {
            let t = crate::ops::tmp::Tmp::new("mark-undo-wrong");
            let ids = asked(&t.0, &["標起來的", "沒標的"]);
            run(&t.0, Some(ids[0]), true, true).expect("mark");
            run(&t.0, Some(ids[1]), false, true).expect("undo 一個沒標的");

            let db = Db::open(&crate::db_path(&t.0)).expect("open");
            assert_eq!(
                db.query_log_stats().expect("stats").marked,
                1,
                "打錯號碼不該動到真的那一個"
            );
            assert_eq!(db.marked_queries(10).expect("marked")[0].id, ids[0]);
        }
    }
}

pub mod queries {
    use super::*;

    /// `sister queries`。
    ///
    /// 題庫要看得見，理由和這整支 CLI 一樣：**可稽核**。它是唯一一張存著「他
    /// 自己打進去的字」的表，而 DATA_INVENTORY 的規則是「有什麼就要看得到
    /// 什麼」。一份存了東西卻沒有任何辦法讀出來的紀錄，和偷偷存著沒有差別。
    pub fn run(
        data_dir: &Path,
        limit: usize,
        only_empty: bool,
        only_marked: bool,
        json: bool,
    ) -> Result<()> {
        let db = open_existing(data_dir)?;
        let stats = db.query_log_stats()?;
        // 多撈一些再篩：`only_empty` 是在這一層過濾的，直接照 limit 撈的話，
        // 「最近 20 題裡剛好沒有空的」會印出一片空白，而題庫裡其實有。
        //
        // `--marked` **不走那條窗**：標記通常是稀有的（退場條件要的是 3 次），
        // 而「最近 400 題裡剛好沒有」對它是常態不是例外。從 `query_marks` 那
        // 邊直接撈，撈到的就是全部——那張表本來就小。
        //
        // `fetched` 是**濾之前**撈到幾列。底下那句「還有更早的」要靠它和
        // `stats.marked` 比——拿濾完的 `rows.len()` 去比的話，`--marked --empty`
        // 濾掉幾列就會被講成「limit 太小」，而那是兩個不同的下一步。
        let (rows, fetched) = if only_marked {
            let marked = db.marked_queries(limit)?;
            let fetched = marked.len();
            let rows: Vec<_> = marked
                .into_iter()
                .map(|m| m.id)
                // 標記是掛在題目上的（外鍵），所以撈不回題目只有一種可能：
                // 中間有人動了資料庫。安靜跳過會讓那一列從清單上消失而數字
                // 還在，所以讓它炸。
                .map(|id| {
                    db.query_by_id(id)?
                        .with_context(|| format!("#{id} 被標記著，題庫裡卻沒有這一題"))
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|r| !only_empty || r.hits == 0)
                .collect();
            (rows, fetched)
        } else {
            let rows: Vec<_> = db
                .query_log(if only_empty { limit * 20 } else { limit })?
                .into_iter()
                .filter(|r| !only_empty || r.hits == 0)
                .take(limit)
                .collect();
            let fetched = rows.len();
            (rows, fetched)
        };

        if json {
            // 撈一次就好。同一句查詢問三遍是「兩次獨立的查找會指到不同的東西」
            // 的溫床——中間有東西寫進去，數字和清單就對不上了。
            let instances = db.marked_queries(limit)?;
            let out = serde_json::json!({
                "total": stats.total,
                "empty": stats.empty,
                "clicked": stats.clicked,
                // `clicked` 的分母。用 `total` 算比例是錯的——見 QueryLogStats。
                "clickable": stats.clickable,
                // Phase 1 的「檢索 < 100ms」要能被腳本讀走，不然那條退場條件
                // 只能靠人去看終端機。
                "p50_ms": stats.p50_ms,
                "p95_ms": stats.p95_ms,
                "slow": stats.slow,
                "budget_ms": sister_core::db::RETRIEVAL_BUDGET_MS,
                // Phase 1 的第一條退場條件（「7 天內 ≥ 3 次答對我自己都忘掉的
                // 東西」）要能被腳本讀走，理由和上面那兩個延遲數字一樣。
                //
                // **這裡不出一個「條件過了沒」的布林。** 「7 天」數的是哪七天
                // （最近七天？最好的那七天？他第一次用起算的七天？）沒有一個
                // 答案是客觀的，而一個自己宣布退場條件過了的工具，只是把印象
                // 換成一個長得像數字的印象。實例帶著時間攤在這裡，那一格由人
                // 去讀。
                "marked": stats.marked,
                // `marked: 0` 有兩種：他從來沒按過，和他按過但現在一格都不剩
                // （自己 `--undo` 收回、或那幾題被 `forget`／保留期帶走）。對
                // 這條退場條件來說那是兩件完全不同的事——前者是還沒開始量，
                // 後者是量過的東西掉了、而且補不回來。人看的那一行早就分開講
                // 了；`--json` 少了這一格的話，腳本只看得到同一個 0。
                //
                // 和 `ever_recorded` / `ever_stored` 同一個形狀，理由也同一個。
                "ever_marked": db.ever_marked()?,
                // `marked` 是全部，`marked_instances` 最多只有 `limit` 列——所以
                // 一併出這一格，讓腳本自己看得出手上那幾列是不是全部。少了它，
                // 一份 `--limit 5` 的輸出和一份真的只有 5 次的輸出長得一模一樣。
                "marked_instances_truncated": (stats.marked as usize) > instances.len(),
                "marked_instances": instances.iter().map(|m| serde_json::json!({
                    "id": m.id,
                    "asked_ts": m.ts,
                    "marked_ts": m.marked_ts,
                    "question": m.question,
                    "hits": m.hits,
                })).collect::<Vec<_>>(),
                // `total: 0` 有三種（沒問過、query_log 關著、被忘掉了）。這一
                // 欄只在它是 `false` 的時候砍得掉第三種：她沒錄過的話就不可能
                // 被忘掉。反過來不成立——`true` 只是說她錄過，不是說他問過。
                // 人看的那一句走的是同一條線，一樣不替他選一個。
                "ever_recorded": db.ever_recorded()?,
                "ever_stored": db.ever_stored()?,
                "queries": rows.iter().map(|r| serde_json::json!({
                    "id": r.id,
                    "ts": r.ts,
                    "question": r.question,
                    "shape": r.shape,
                    "hits": r.hits,
                    "latency_ms": r.latency_ms,
                    "source": r.source,
                    "clicks": r.clicks,
                    "marked": r.marked(),
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        if stats.total == 0 {
            // 兩個原因不夠，有第三個。`queries` 是 `forget` **刻意**要帶走的
            // 一張表（`retention.rs` 那邊的理由寫得很清楚：他打進搜尋框的字往
            // 往比畫面更直接），保留期也照 `text_days` 清。而這張表存的是他自
            // 己打的字——一個剛把搜尋紀錄刪掉的人，最不該收到的一句話就是
            // 「問她幾個問題就會開始累積」。
            //
            // 但不宣布是哪一種。`ever_recorded` 答得出「她錄過」，答不出「他
            // 問過」——一個天天在錄、從來沒用過搜尋框的人（這個專案自己就是），
            // 拿到「你問過的那些題被忘掉了」一樣是假話。
            //
            // 所以照這個 repo 一路的規矩：不知道就把可能性攤開，不要替他選一
            // 個（見字母人那句「可能是剛開始，也可能是之前的被忘掉了或過期
            // 了」）。她沒錄過的話「被忘掉」不可能成立，那時候原本那兩句是完
            // 整的——所以只有錄過才多講第三種。
            //
            // **而 `ever_marked` 答得出「他問過」。** 上一版這裡寫著「要答得出
            // 來就得再留一個位元，那是拿一份殘骸去換一句更漂亮的話」——而那個
            // 位元後來為了另一件事（★ 的兩種零）留下來了，帳已經付過。標記只
            // 掛得上一題真的問過的題目，所以它翻成 1 就代表他問過。
            //
            // 這一格剛好是整個功能最要緊的那一種空：`forget`／保留期把題庫整
            // 個帶走的時候，Phase 1 第一條退場條件的證據跟著沒了。而上一版在
            // 這條路上一個字都不提標記（★ 那一段在 `stats.total == 0` 的
            // return 底下，根本輪不到），還把「還沒問過她任何問題」列成第一個
            // 可能——一個它手上這個位元剛剛否掉的可能。
            println!(
                "{}",
                if db.ever_marked()? {
                    "題庫是空的，但不是因為你還沒問過她：你問過，而且在裡面按過至少一次\
                     「★ 我本來已經忘了」。那幾題現在不在了——刪得掉題目的只有 \
                     `sister forget` 和保留期，所以是這兩件事其中之一。\n\
                     標記也一個都不剩，而那一格補不回來：它記的是你看到答案那一刻腦袋裡的\
                     狀態，事後補不出來。Phase 1 那條退場條件要是靠那幾次在算的，得從頭再數。"
                } else if db.ever_recorded()? {
                    "題庫是空的。可能是還沒問過她任何問題（`sister query …` 或字母人的\
                     搜尋框），可能是 `privacy.query_log` 關著，也可能是問過的那幾題被 \
                     `sister forget` 忘掉了、或過了保留期——她錄過，所以這三種都還在檯面上。"
                } else {
                    "題庫是空的。問她幾個問題（`sister query …` 或字母人的搜尋框）就會開始累積。\n\
                     如果你把 `privacy.query_log` 關掉了，那它永遠會是空的——那也是一個合理的選擇。"
                }
            );
            return Ok(());
        }

        // 「一筆都沒找到」的比例是重點。總數只說明他用了多少次。
        println!(
            "題庫：{} 題，其中 {} 題一筆都沒找到（{:.0}%）",
            stats.total,
            stats.empty,
            100.0 * stats.empty as f64 / stats.total as f64,
        );
        // 點開出處是檢索品質唯一不用人工標註就拿得到的訊號——但**只有字母人
        // 那邊點得動**。分母用總題數的話，開發時跑幾十次 `sister query` 就會
        // 把它壓成一個結構性的 0%，而那個 0% 講的是介面，不是她答得好不好。
        println!(
            "{}",
            match stats.clickable {
                0 => "出處：還沒有從字母人問過（終端機沒有出處可以點，所以這裡看不出答案有沒有用）"
                    .to_string(),
                n => format!(
                    "出處：從字母人問過 {} 題，其中 {} 題你點開了出處（{:.0}%）",
                    n,
                    stats.clicked,
                    100.0 * stats.clicked as f64 / n as f64,
                ),
            }
        );
        // PHASES.md Phase 1 的退場條件之一是「檢索 < 100ms」，而在這一行之前
        // 沒有任何東西量得出來——每一題花了幾毫秒從第一天就存著，只是沒有人
        // 讀。中位數說平常有多快，p95 說最糟的時候有多糟；平均值兩個都答不了，
        // 一次 4 秒的卡頓會把一整年拉成一個從沒發生過的數字。
        let budget = sister_core::db::RETRIEVAL_BUDGET_MS;
        let over = match stats.slow {
            0 => format!("（門檻 {budget} ms，沒有一題超過）"),
            n => format!("——**{n} 題超過 {budget} ms 的門檻**"),
        };
        // 存的是整數毫秒，所以「全部都是 0」的意思是每一題都在 1 ms 以內。
        // 照樣印「一半在 0 ms 以內」是對的數字配一句蠢話。
        if stats.p95_ms == 0 {
            println!("延遲：每一題都在 1 ms 以內{over}");
        } else {
            println!(
                "延遲：一半在 {} ms 以內，最慢的 5% 從 {} ms 起{over}",
                stats.p50_ms, stats.p95_ms
            );
        }
        // PHASES.md Phase 1 的**第一條**退場條件（「7 天內 ≥ 3 次答對我自己都
        // 忘掉的東西」）在這一行之前一樣沒有東西量得出來——而它跟上面那幾個數
        // 字有一個關鍵的差別：**這一格不是量出來的，是他按下去的。**
        //
        // 所以 0 在這裡不代表壞掉，它有兩種意思（他沒按過／她真的沒神奇過），
        // 而這兩種分不出來。分不出來就不要替他選一個——照這個 repo 一路的規矩，
        // 把可能性攤開，順便講出那個按鈕在哪裡。
        //
        // **但有第三種，而它分得出來。** 標記掛在 `queries` 底下，`forget` 和
        // 保留期都會連著帶走（`prune` 還是在錄製迴圈裡自己跑的，不必他動手）。
        // 一個按過三次、然後把那段時間忘掉的人，收到「去按按看」是這個 repo
        // 一路在修的那種話。`ever_marked` 剛好只答得出這一題。
        println!(
            "{}",
            match (stats.marked, db.ever_marked()?) {
                // **這裡不准說是哪一種。** `ever_marked` 只答得出「他按過」，
                // 答不出「後來是誰拿掉的」：他自己 `--undo` 收回的，和那幾題被
                // `forget`／保留期帶走的，在這張表上長得一模一樣。
                //
                // 第一版寫成「跟著那幾題一起被忘掉了」——而驗收清單上就有一步
                // 是叫他按一次再收回來，那一步走完會當場拿到這句話，然後被告知
                // 他的資料被刪掉了。修一個「兩種零」的時候造出第三種，是這個
                // repo 犯過最多次的一件事。
                (0, true) => "★ 魔法時刻：你按過，但現在一個都不剩。\n  \
                              （你自己收回的話，這樣就對了。如果不是——那幾題被 \
                              `sister forget` 帶走、或是過了保留期，標記跟著走了，\
                              而這一格補不回來。）"
                    .to_string(),
                (0, false) => "★ 魔法時刻：還沒有標記過任何一次。\n  \
                      （她答對了一件你早就忘掉的事的時候，`sister mark` 記下來——\
                      那是 Phase 1 退場條件唯一的量法，而一個禮拜之後補不回來。）"
                    .to_string(),
                (n, _) => format!(
                    "★ 魔法時刻：{n} 次（你自己標的）。`sister queries --marked` 看是哪幾題"
                ),
            }
        );
        println!();
        for r in &rows {
            println!(
                // 題號印在最前面，因為 `sister mark --id` 要用它——一個看得到
                // 卻叫不出名字的清單，等於只有「最近那一題」標得到。
                "  {} #{:<4} {} {}{}{}{}",
                crate::fmt::timestamp(r.ts),
                r.id,
                crate::fmt::pad(&crate::fmt::one_line(&r.question, 20), 24),
                if r.hits == 0 {
                    "一筆都沒有".to_string()
                } else {
                    format!("{} 筆", r.hits)
                },
                if r.shape == "recent" || r.shape == "range" {
                    "（時間）"
                } else {
                    ""
                },
                match r.clicks {
                    0 => String::new(),
                    n => format!("，點開了 {n} 個出處"),
                },
                // 點擊和標記各印各的。併成一句「有用」的話，這兩個訊號就再也
                // 分不出來了——而它們講的是相反的事（見 `MIGRATION_007`）。
                if r.marked() {
                    "  ★ 你本來已經忘了"
                } else {
                    ""
                },
            );
        }
        if only_marked {
            // 撈到的比題庫裡的少 = limit 卡住了。這是這個 repo 的規矩：**被切掉
            // 就要講**，不然一份被截斷的清單讀起來就是「全部」。先算，因為底下
            // 那句話的主詞範圍要靠它。
            let truncated = (fetched as i64) < stats.marked;
            if truncated {
                println!(
                    "  （只列出最近 {fetched} 次，總共 {} 次——要看全部請把 --limit 調大。）",
                    stats.marked
                );
            }
            // **0 有兩種，而表頭只講得出其中一種。** 一次都沒標過的時候表頭剛
            // 剛才講完（不重複講一次）；有標記、卻被 `--empty` 濾光的時候，一
            // 片空白配著一句「魔法時刻：3 次」是自相矛盾的——那三次都找得到東
            // 西，所以它們不會出現在「一筆都沒找到」的清單上。
            //
            // 主詞只能是**真的看過的那幾列**。上一版拿 `stats.marked` 當主詞，
            // 而證據只有 `limit` 撈到的那幾列：3 個標記、空手的那一次標得最早、
            // `--limit 2`，於是它一邊說「沒有一次是空手回來的」，一邊在上一行
            // 叫他把 limit 調大——而調大之後那一列就出現了。兩句話的下一步還
            // 剛好相反：一句叫他別看了，一句叫他再看一次。
            if rows.is_empty() && only_empty && fetched > 0 {
                if truncated {
                    println!(
                        "  剛剛看的那 {fetched} 次裡沒有空手的——更早的還沒看到，\
                         要看的話 --limit 調大一點。"
                    );
                } else {
                    println!(
                        "  你標記過的那 {fetched} 次沒有一次是空手回來的，所以配上 --empty 就什麼都不剩。"
                    );
                }
            }
        }
        // `--marked` **不走那個窗**（它從 `query_marks` 直接撈，撈到的就是全
        // 部），所以底下這一整段對它是假的：它會拿「最近 N 題」去描述一個根本
        // 沒開過的範圍，說那些空手的題「比這個範圍更早」（可能反而更新），然後
        // 叫他把 --limit 調大——而調到多大都不會出現，因為那幾題根本沒被標。
        // 真正的下一步是把 `--marked` 拿掉，而那句話沒有人說得出來。
        if rows.is_empty() && only_empty && !only_marked {
            // 上面那一行「多撈一些再篩」自己承認了這是一個**窗**，而這句話以前
            // 講得像整份題庫：`stats.empty` 有 1,200 題、但一題都不落在最近那幾
            // 百列裡的時候，印出來的和「她真的每一題都找得到東西」一模一樣。
            // 那兩件事的下一步相反——一個是「很好」，一個是「窗要開大才看得到」。
            // 分得開它們的數字兩行之前就在手上（表頭剛印過）。
            if stats.empty == 0 {
                println!("  題庫裡沒有半題是空手回來的。");
            } else {
                println!(
                    "  最近這 {} 題裡沒有空手的——不過整份題庫有 {} 題是（見上面那一行），\n  \
                     它們比這個範圍更早。要看的話 --limit 調大一點。",
                    // 窗是「撈 limit×20 列再篩」，可是題庫沒那麼多的時候真正掃過
                    // 的就是全部——講一個比實際大的數字，會讓「更早」變成假話。
                    (limit as i64 * 20).min(stats.total),
                    stats.empty,
                );
            }
        }
        Ok(())
    }
}

pub mod stop {
    use super::*;

    /// `sister stop`。
    ///
    /// 存在的理由和 `pause` 一樣：讓「請她收工」這條路在一台無頭機器上就驗得完
    /// ——CI 開得起 `sister record`，但開不起字母人。少了這個子命令，開始／停止
    /// 那整套機制唯一的入口會在一個沒有人測得到的 GUI 按鈕後面。
    pub fn run(data_dir: &Path) -> Result<()> {
        // 先問有沒有人在。沒有人在的時候仍然照寫——下一個起來的 recorder 會先
        // 清掉這個檔案（`BootBeat::start`），所以留著不會咬人——但要講出來，
        // 不然「我按了停止，可是它還在錄」和「我按了停止，本來就沒有東西在
        // 錄」看起來一模一樣。
        //
        // **問 `phase` 不是 `is_occupied`。** 上一版問「有沒有人佔著」，於是
        // 開機那幾分鐘印的是「正在跑的 `sister record` 會在下一個 tick 把
        // session 寫完再結束」——那時候既沒有 session 也沒有 tick，主迴圈根本
        // 還沒開始。三種狀態，三句話。
        //
        // **而這兩行的順序是有負載的：先讀心跳，再寫請求。** 反過來寫的話，中間
        // 那一瞬間 recorder 可能剛好收到請求、收工、`heartbeat::stop` 在
        // `recording.beat` 上蓋一塊墓碑——於是我們回頭一問，得到 `None`，印出「目前沒有
        // 任何 `sister record` 在跑」，而我們剛剛才成功停掉一場。他下一步會去做
        // 的事（再按一次、或者去工作管理員找）全部建立在那句話上。
        //
        // 這個順序沒有測試擋得住（那個競爭窗要用執行緒去搶，而搶得贏是機率問
        // 題，搶不贏就是一支偽綠的測試）。擋著它的只有這一段字——所以它寫在這
        // 裡，不在 commit message 裡。
        let seen = sister_core::heartbeat::presence(data_dir, sister_core::now_ms());
        sister_core::control::request_stop(data_dir)
            .with_context(|| format!("寫不進 {}", data_dir.display()))?;
        match seen {
            sister_core::heartbeat::Presence::Live(sister_core::heartbeat::Phase::Recording) => {
                println!(
                    "■ 已經請她收工。正在跑的 `sister record` 會在下一個 tick 把 session \
                     寫完再結束。"
                )
            }
            sister_core::heartbeat::Presence::Live(sister_core::heartbeat::Phase::Booting) => {
                println!(
                    "■ 已經請她收工。她現在還在開資料庫（第一次開一顆大的要重建索引，\
                     可能要幾分鐘），主迴圈還沒開始——這個請求會留著，等她開完就直接\
                     收工，一個字都不會記。"
                )
            }
            sister_core::heartbeat::Presence::Thinking { .. } => {
                println!(
                    "■ 錄製已經停了，解釋層還在想最後一段。迴圈已經跳出去了，\
                     想完就會自己收工——沒有請求可以再送。"
                )
            }
            sister_core::heartbeat::Presence::NeverStarted
            | sister_core::heartbeat::Presence::Unreadable
            | sister_core::heartbeat::Presence::Stopped { .. }
            | sister_core::heartbeat::Presence::Stalled { .. } => println!(
                "■ 目前沒有任何 `sister record` 在跑（心跳是停的）。停止的請求還是\
                 留下來了，但下一次開始記錄的時候會先把它清掉，不會影響到那一場。"
            ),
        }
        Ok(())
    }
}

pub mod prune {
    use super::*;
    use sister_core::config::Config;
    use sister_core::retention::PruneReport;

    /// 畫面檔的根目錄。**算式在 core**，這裡只是轉一手——字母人那邊的
    /// 「忘掉這一段」也要刪同一批檔案，兩個執行檔各拼一次遲早會指到不同地方。
    pub fn frames_dir(data_dir: &Path) -> std::path::PathBuf {
        Config::frames_dir(data_dir)
    }

    pub fn run(data_dir: &Path, config: &Config, dry_run: bool) -> Result<()> {
        let r = &config.retention;
        // 「畫面 30 天」單獨講會被讀成「30 天後那一幀就不存在了」，而真正
        // 消失的只有 PNG——時間、app、視窗標題、網址跟著文字走到 365 天。
        // 那幾樣東西本身就說得出他那天在幹嘛，所以這個差別不是細節。
        // `doctor` 的「保留期」那一段早就講對了（「到期只刪圖，字留著」），
        // 這裡是同一句話的第二個出口。設定頁上也寫著同一件事。
        println!(
            "保留期：畫面檔 {} 天（到期只丟圖）、文字與事實 {} 天（到期整列消失，\
             那一幀的時間與 app 也跟著走）",
            r.frames_days, r.text_days
        );

        // 還沒錄過東西不是錯誤。`prune` 是維護動作，「沒有東西要清」是它
        // 成功的結果之一——而 `query` 查不到資料庫才該報錯，因為那代表
        // 使用者以為自己有資料。同一個 helper 套在兩種語意上會弄錯其中一個。
        let path = crate::db_path(data_dir);
        if !path.exists() {
            println!("  還沒有資料庫（{}），沒有東西可以清。", path.display());
            return Ok(());
        }
        let mut db = Db::open(&path).with_context(|| format!("open {}", path.display()))?;
        let now = sister_core::now_ms();

        if dry_run {
            let report = db.prune_preview(now, r, Some(&frames_dir(data_dir)))?;
            print_report(&report, true);
            return Ok(());
        }
        let report = db.prune(now, r, Some(&frames_dir(data_dir)))?;
        print_report(&report, false);
        Ok(())
    }

    /// 資料庫說有圖、磁碟上找不到那個檔的那幾列（`None` = 沒有這種列）。
    ///
    /// 以前這幾列會被算成「刪掉了」，連大小都照著資料庫的欄位加進去——手動
    /// 清空過 `frames/`、或從一份沒帶 `--with-frames` 的備份還原之後，報告會
    /// 說「刪掉了 120 個畫面檔（1.2 GB）」，而磁碟上一個位元組都沒有釋放。
    ///
    /// 不是 ⚠：東西確實不在了，隱私上沒有缺口。但他拿這個數字對帳，而
    /// 「120」和「0」對他的意思完全不一樣。
    ///
    /// **預覽也要印。**這一句以前掛著 `!preview`，理由是「預覽那一支不知道
    /// 檔案在不在」——那是真的，因為它照著資料庫算。可是預覽正是那個**決定
    /// 要不要刪**的畫面：他看到「會放出 1.2 GB」才按下去，然後拿到一句
    /// 「刪掉了 0 個畫面檔」。兩支現在走同一套記帳（`count_files` 和
    /// `delete_files`），所以這一句在兩邊都說得出口。
    ///
    /// 但**不是同一句**：預覽那一版不可以說資料庫「已經跟著更新」——它一列
    /// 都沒動。時態錯的安慰話和沒講一樣糟。
    fn ghost_rows_line(missing: u64, preview: bool) -> Option<String> {
        (missing > 0).then(|| {
            format!(
                "  ? 另外 {missing} 列說自己有圖，但那個檔{}不在磁碟上了——\
                 \n    可能是有人手動清過 frames/，也可能是從一份沒帶畫面的備份還原的。\
                 \n    （{}）",
                if preview { "現在就已經" } else { "已經" },
                if preview {
                    "所以那幾列放不出任何空間；資料庫會在真的刪的時候跟著更新"
                } else {
                    "資料庫已經跟著更新，不會再指向它們"
                }
            )
        })
    }

    /// 刪掉的東西要一項一項講出來。
    ///
    /// 「清理完成」這種話等於沒說：使用者沒辦法分辨「沒有東西過期」和
    /// 「清理其實沒生效」——而這兩件事在磁碟上長得一模一樣。
    pub fn print_report(r: &PruneReport, preview: bool) {
        let verb = if preview { "會刪掉" } else { "刪掉了" };
        if r.is_empty() {
            println!("  沒有東西過期，什麼都沒動。");
            return;
        }
        if r.images_deleted > 0 {
            println!(
                "  {verb} {} 個畫面檔（{}）",
                r.images_deleted,
                crate::fmt::bytes(r.image_bytes_freed as i64)
            );
        }
        // 句子在 `ghost_rows_line`（那裡才驗得到預覽和事後不是同一句）。
        if let Some(line) = ghost_rows_line(r.missing, preview) {
            println!("{line}");
        }
        if r.frames_deleted > 0 {
            println!(
                "  {verb} {} 列畫面紀錄、{} 段文字、{} 個事實、{} 筆事件",
                r.frames_deleted, r.chunks_deleted, r.facts_deleted, r.events_deleted
            );
        } else if r.chunks_deleted + r.facts_deleted + r.events_deleted > 0 {
            println!(
                "  {verb} {} 段文字、{} 個事實、{} 筆事件",
                r.chunks_deleted, r.facts_deleted, r.events_deleted
            );
        }
        // 單獨一行，不併進上面那串。上面那些是她觀察到的東西，這一行是
        // **他自己打的字**——他有權利當場看到那句話也一起消失了。
        //
        // （註解裡的「他」和印出來的「你」是兩件事。這一行原本寫成「他自己
        // 問過的話」，是註解的人稱漏到輸出裡了——整個 CLI 對使用者一律講
        // 「你」，只有這一句在旁邊講他。）
        if r.queries_deleted > 0 {
            println!("  {verb} {} 題你自己問過的話（題庫）", r.queries_deleted);
        }
        // 又單獨一行，而且**排在題庫那一行後面貼著它**：它是那一行的一部分被
        // 帶走的東西，不是另一類資料。
        //
        // 為什麼值得自己一行：這份報告上的每一樣東西他都補得回來——畫面可以再
        // 錄，題目可以再問一次——只有這一格不行。那是他看到答案那一刻腦袋裡的
        // 狀態，也是 Phase 1 第一條退場條件唯一的證據。而 `prune` 是在錄製迴圈
        // 裡自己跑的：他可以在完全沒動手的情況下把那幾次弄不見。
        if r.marks_deleted > 0 {
            println!(
                "  {verb} {} 次「★ 我本來已經忘了」——這一格補不回來",
                r.marks_deleted
            );
        }
        // 也單獨一行。它刪的不是內容，是「那天 13:02 到 17:44 她在錄」——
        // 一份沒有任何內容、卻證明他那段時間坐在電腦前的紀錄。那張表以前
        // 誰都不刪，而 `forget` 的說明從第一天起就寫著「每一張表都清乾淨」。
        if r.sessions_deleted > 0 {
            println!(
                "  {verb} {} 場錄製的紀錄本身（那幾場已經一列都不剩了）",
                r.sessions_deleted
            );
        }
        // 刪不掉的檔案仍然躺在磁碟上，而使用者以為它已經不在了。
        // 這是整份報告裡唯一絕對不能安靜掉的一項。
        for f in &r.failed {
            println!("  ⚠  刪不掉，這個畫面還在磁碟上：{f}");
        }
        // 指向它們的那幾列**留著了**，所以下一輪會自己再試一次。少了這句，
        // 上面那幾個 ⚠ 讀起來像是要他自己去把檔案挖出來刪掉。
        if !preview && !r.failed.is_empty() {
            println!(
                "     （指向它們的紀錄先留著，不然就沒有人找得到那些檔案了。\
                 \n     下一輪會再試一次；一直失敗的話多半是防毒或權限擋住了）"
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// **預覽也要講那幾列幽靈，但不可以用事後那句話講。**
        ///
        /// 這一句以前只在真的刪完之後才印，理由是預覽照著資料庫算、看不出檔案
        /// 在不在——那正是這一輪修掉的東西。現在兩支都去 stat 一次，所以預覽
        /// 說得出「會刪掉 0 個畫面檔」，也就欠一句「那 120 列去哪了」。
        ///
        /// 但事後那一版說的是「資料庫已經跟著更新」，而預覽一列都沒動。
        /// 直接共用同一句，等於在一個什麼都還沒發生的畫面上報告完成式。
        #[test]
        fn a_preview_owns_up_to_the_ghost_rows_without_claiming_it_fixed_them() {
            let previewed = ghost_rows_line(120, true).expect("有 120 列就要講");
            assert!(previewed.contains("120"), "{previewed}");
            assert!(
                previewed.contains("放不出任何空間"),
                "他看的就是空間那個數字：{previewed}"
            );
            assert!(
                !previewed.contains("已經跟著更新"),
                "預覽一列都沒動，不准用完成式：{previewed}"
            );

            let after = ghost_rows_line(120, false).expect("有 120 列就要講");
            assert!(after.contains("已經跟著更新"), "{after}");

            for preview in [true, false] {
                assert!(
                    ghost_rows_line(0, preview).is_none(),
                    "一列都沒有的時候不要生出一句話來"
                );
            }
        }
    }
}

/// 把記憶整份帶走。
///
/// SPEC §11.8（資料主權）寫著「`sister export` 全量匯出……就算本專案死了，
/// 你的記憶還是你的」，而那個指令不存在。PRIVACY.md 給的替代說法是「整份記憶
/// 就是一個 `sister.db` 檔加一個 `frames/` 目錄」——**那句話在她正在錄的時候
/// 是錯的。** 資料庫跑在 WAL 模式，最近寫進去的東西還躺在旁邊的
/// `sister.db-wal` 裡；照那句話去複製一個檔案，備份會安靜地少掉最後那一段。
///
/// 而「安靜地少掉最近那一段」是備份最不該有的失效模式：他要等到真的需要那份
/// 備份的那一天才會發現。
///
/// 匯出的目的地**就是一個資料目錄**——不是另一種格式，是同一種。所以「還原」
/// 這個動作不需要任何工具：`sister --data-dir <匯出的目錄> query …` 直接就問
/// 得到。一個要靠原廠程式才讀得回來的匯出檔，不算資料主權。
pub mod export {
    use super::*;
    use sister_core::config::Config;

    pub fn run(data_dir: &Path, to: &Path, with_frames: bool) -> Result<()> {
        let path = crate::db_path(data_dir);
        if !path.exists() {
            anyhow::bail!("{} 裡沒有資料庫，沒有東西可以匯出", data_dir.display());
        }
        refuse_to_export_into_itself(data_dir, to)?;
        let db = Db::open(&path).with_context(|| format!("open {}", path.display()))?;

        std::fs::create_dir_all(to).with_context(|| format!("建立 {}", to.display()))?;
        let dest_db = Config::db_path(to);
        db.export_to(&dest_db)?;
        let db_bytes = std::fs::metadata(&dest_db).map(|m| m.len()).unwrap_or(0);
        drop(db);

        // **數字要從匯出檔身上讀，不是從來源。** 從來源讀比較快也比較順手，
        // 但那樣印出來的是「我想匯出的東西」，不是「他手上這份裡有什麼」——
        // 而這兩者不一樣的那一天，正是他最需要知道的那一天。
        //
        // 順手也就把匯出檔開起來讀過一次了：一份打不開的備份會在這裡當場失敗，
        // 而不是等到他真的需要它的時候。這和 `doctor` 那條「不宣稱，當場示範」
        // 是同一件事。
        let exported = Db::open(&dest_db)
            .with_context(|| format!("匯出寫完了，但打不開：{}", dest_db.display()))?;
        let s = exported.stats()?;
        println!("匯出到 {}", to.display());
        println!(
            "  ✓ sister.db   {}（{} 列畫面、{} 段文字、{} 個事實、{} 題你問過的話）",
            crate::fmt::bytes(db_bytes as i64),
            s.frames,
            s.chunks,
            s.facts,
            exported.query_log_stats().map(|q| q.total).unwrap_or(0),
        );

        // 畫面檔在資料庫外面，而它們通常比資料庫大好幾個數量級。預設不帶，
        // 但**一定要講**帶了沒——一份自稱「全量」卻少了幾 GB 截圖的匯出，
        // 和一份少了最後一小時的備份是同一種錯。
        let src_frames = Config::frames_dir(data_dir);
        if with_frames {
            let dst_frames = Config::frames_dir(to);
            let (n, bytes) = copy_frames(&src_frames, &dst_frames)?;
            // 數量比對不夠。她一邊錄一邊匯出（README 就是這樣教的，而且有一條
            // 測試守著那個情境）的時候，`frames/` 會長出比資料庫那個快照更新的
            // 檔案——於是「複製了 121 個、資料庫說有 120 列」看起來是滿的，
            // 而那 120 列裡真的少掉的那一張被兩張新的遮住了。所以逐條去對。
            //
            // 問的是**匯出檔**（不是來源）：這一行要回答的是「他手上這份備份
            // 自己說得通嗎」，而他將來還原的就是這一份。理由和上面那段
            // 「數字要從匯出檔身上讀」同一條。
            let mut missing = 0u64;
            let mut first_missing = None;
            exported.for_each_image_path(|rel| {
                if !dst_frames.join(rel).is_file() {
                    missing += 1;
                    if first_missing.is_none() {
                        first_missing = Some(rel.to_string());
                    }
                }
            })?;
            println!(
                "{}",
                frames_line(
                    n,
                    bytes,
                    s.frames_with_image as u64,
                    missing,
                    first_missing.as_deref(),
                    &src_frames
                )
            );
        } else {
            println!("{}", frames_skipped_line(s.frames_with_image as u64));
        }

        // **她動過的手也是記憶，而且它不在資料庫裡。** 少了這一份，底下那句
        // 「沒帶走的是 consent.toml 和 config.toml，那兩份是設定不是你的記憶」
        // 就變成一句漏講了東西的話——而漏掉的那個**是**記憶。
        //
        // 不像 `frames/` 那樣配一個開關：它是純文字，通常幾 KB，不會有人為了
        // 它的大小猶豫。沒按過那顆按鈕的話這個檔案不存在，那就什麼都不印——
        // 「她一次手都沒動過」不需要在匯出報告上佔一行。
        //
        // 檔名向 `ActionLog` 要，不在這裡再拼一次字串：`in_data_dir` 那一處是
        // 唯一寫死它的地方（見那支函式的註解）。
        let src_log = sister_hands::ActionLog::in_data_dir(data_dir);
        let dst_log = sister_hands::ActionLog::in_data_dir(to);
        match std::fs::copy(src_log.path(), dst_log.path()) {
            Ok(bytes) => println!(
                "  ✓ {}   {}（她按你的指示動過的手）",
                dst_log
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                crate::fmt::bytes(bytes as i64),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("複製 {} 失敗", src_log.path().display()));
            }
        }

        // 匯出的目錄就是一個資料目錄，所以「還原」不需要任何工具。這一行
        // 是整個指令的重點：他能自己驗證那份匯出是活的。
        println!("\n這個目錄本身就是一個資料目錄，直接問得到：");
        println!("  sister --data-dir {} query 電話", to.display());
        println!(
            "沒帶走的是 consent.toml（三張同意書的簽名）和 config.toml（設定）——\n\
             那兩份是這台機器的設定，不是你的記憶。"
        );

        // **這個指令是整個產品裡唯一會讓人變得不安全的地方。**
        //
        // 第一句承諾是「資料不離開這台機器」，而我們剛剛做的事，就是把她記得
        // 的全部東西寫到一個他指定的路徑。那個路徑很可能是 `~/Dropbox/backup`
        // 或 `%USERPROFILE%\OneDrive\...`——他會覺得自己在備份，實際上是在
        // 上傳，而且是一次上傳全部。
        //
        // 不去猜哪些目錄是雲端資料夾：各家的名字、各國的在地化、各版本的預設
        // 路徑都不一樣，而猜錯的方向特別糟——沒攔到的那次，他會因為「它沒說
        // 什麼」而更放心。所以就照 PRIVACY.md 那條界線原樣說一次，在他還記得
        // 自己剛剛打了什麼路徑的這一秒。
        println!(
            "\n這份匯出沒有加密，和原本那份一樣靠這顆硬碟的加密\n\
             （BitLocker / LUKS / FileVault）。它放到哪裡就只被保護到哪裡——\n\
             丟進雲端同步資料夾，她記得的東西就跟著上去了。"
        );
        Ok(())
    }

    /// `frames/` 那一行。`copied` 是剛剛複製過去的檔案數，`claimed` 是資料庫
    /// 裡有幾列說自己有圖，`missing` 是**逐條去對之後**目的地真的打不開的那
    /// 幾條（`example` 是其中第一條，講給他看的）。
    ///
    /// 「0 個畫面檔」單獨印出來是一句真話，但它回答不了他心裡真正那一題：
    /// **是本來就沒有，還是我剛剛弄丟了？** 這兩件事對備份來說天差地遠，而
    /// 資料庫知道答案。所以三種情況要長得不一樣，尤其少了的那一種——那時候
    /// 前面不該掛一個 ✓。
    ///
    /// `missing` 是後來補的，因為只比數量會被**多出來的檔案遮住**：她一邊錄
    /// 一邊匯出的時候 `frames/` 一直在長，而 `claimed` 是幾秒前那個快照。
    /// 「121 ≥ 120」於是掛上一個 ✓，即使那 120 條裡有一條根本沒過去。
    fn frames_line(
        copied: u64,
        bytes: u64,
        claimed: u64,
        missing: u64,
        example: Option<&str>,
        src: &Path,
    ) -> String {
        if missing > 0 {
            format!(
                "  ✗ frames/     {}（複製了 {copied} 個檔，但少了 {missing} 張）\n\
                 \x20    資料庫裡有 {missing} 列的圖在這份匯出裡打不開，例如 {}。\n\
                 \x20    數量看起來夠不代表對得上——一邊錄一邊匯出的時候 frames/ 還在長。\n\
                 \x20    來源在 {}，可以再跑一次。",
                crate::fmt::bytes(bytes as i64),
                example.unwrap_or("（沒記下是哪一條）"),
                src.display()
            )
        } else if copied < claimed {
            format!(
                "  ✗ frames/     {}（{copied} 個畫面檔，但資料庫裡有 {claimed} 列說自己有圖）\n\
                 \x20    少了 {} 張。可能是有人手動刪過 frames，也可能是這次沒複製完——\n\
                 \x20    來源在 {}，比對得出來。",
                crate::fmt::bytes(bytes as i64),
                claimed - copied,
                src.display()
            )
        } else if copied == 0 {
            // 「本來就只有字」是一句關於歷史的話，而這裡看到的只有此刻的資料庫。
            // 圖過了 `frames_days` 被清掉的那一份也長這樣，而那些圖存在過。
            "  ✓ frames/     一個檔都沒有——資料庫裡沒有任何一列說自己有圖，沒有畫面可以帶".into()
        } else {
            format!(
                "  ✓ frames/     {}（{copied} 個畫面檔）",
                crate::fmt::bytes(bytes as i64)
            )
        }
    }

    /// 沒加 `--with-frames` 的那一行。`claimed` 是資料庫裡有幾列說自己有圖。
    ///
    /// 舊版只有一句：「出處點開來看到的那些畫面留在原地——加 `--with-frames`」。
    /// 它在一台 text-only 的機器上（第三張同意書沒簽），或者一顆圖全都過了
    /// `frames_days` 的資料庫上，是**假的**——留在原地的是零張。而它上面兩行
    /// 才剛印過「52000 列畫面」，於是那句話讀起來像「五萬兩千張截圖沒帶走」。
    ///
    /// 更糟的是它指的那顆旋鈕。他照著加上 `--with-frames` 再跑一次，複製到的
    /// 是零個檔——一句叫他去按一個改不了結果的開關的建議。
    ///
    /// 兄弟分支 [`frames_line`] 早就分成三種了，而且有一條測試叫
    /// `zero_pictures_because_there_were_none_reads_differently_from_zero_because_they_are_gone`
    /// 專門守那個區別。這一支只是沒被套用到。
    fn frames_skipped_line(claimed: u64) -> String {
        if claimed == 0 {
            // 和 `frames_line` 的 `copied == 0` 那一支說同一件事、留同一個保留：
            // 「本來就只有字」是一句關於歷史的話，而這裡看得到的只有此刻。
            "  ⏸ frames/     沒帶，但也沒有東西可以帶——資料庫裡沒有任何一列說自己有圖".into()
        } else {
            format!(
                "  ⏸ frames/     沒帶。出處點開來看到的那 {claimed} 張畫面留在原地——加 `--with-frames`"
            )
        }
    }

    /// 匯出到資料目錄裡面是不行的。
    ///
    /// 最兇的那個寫法是 `--to <資料目錄>/frames/backup --with-frames`：
    /// `copy_frames` 一邊走 `frames/`、一邊在 `frames/` 底下長出新的目錄，
    /// 於是它一直往下複製自己。實測會蓋出兩百多層才停——**停下來的原因是檔名
    /// 太長（ENAMETOOLONG），不是我們攔住了它**，而那之前它已經在他的資料
    /// 目錄裡留下一堆垃圾。
    ///
    /// 但攔的理由不需要那麼技術性：**放在被備份的東西裡面的備份不是備份。**
    /// 那顆硬碟壞掉的時候兩份一起走，而那正是備份要處理的情況。
    ///
    /// 用 `std::path::absolute` 不用 `canonicalize`：目的地還不存在，
    /// `canonicalize` 會直接失敗。代價是 `..` 不會被化簡，所以
    /// `--to <資料目錄>/../<資料目錄>/frames/x` 這種寫法躲得過——它躲過的
    /// 方向是「以為不在裡面」，而那要靠刻意去繞。
    fn refuse_to_export_into_itself(data_dir: &Path, to: &Path) -> Result<()> {
        let (data, dest) = (
            std::path::absolute(data_dir).with_context(|| format!("{}", data_dir.display()))?,
            std::path::absolute(to).with_context(|| format!("{}", to.display()))?,
        );
        if dest.starts_with(&data) {
            anyhow::bail!(
                "不能匯出到 {} —— 那在資料目錄（{}）裡面。\n\
                 放在被備份的東西裡面的備份不是備份：那顆硬碟壞掉的時候兩份一起走。\n\
                 （而且 `--with-frames` 會在複製途中一直往下複製自己。）",
                to.display(),
                data_dir.display()
            );
        }
        Ok(())
    }

    /// 一層一層複製畫面檔。回傳 (檔案數, 位元組)。
    ///
    /// 畫面檔寫完就不再改，所以邊錄邊複製是安全的——不像資料庫。
    fn copy_frames(src: &Path, dest: &Path) -> Result<(u64, u64)> {
        if !src.exists() {
            return Ok((0, 0));
        }
        let mut n = 0;
        let mut bytes = 0;
        std::fs::create_dir_all(dest).with_context(|| format!("建立 {}", dest.display()))?;
        for entry in std::fs::read_dir(src).with_context(|| format!("讀 {}", src.display()))? {
            let entry = entry?;
            let (from, to) = (entry.path(), dest.join(entry.file_name()));
            if entry.file_type()?.is_dir() {
                let (sub_n, sub_bytes) = copy_frames(&from, &to)?;
                n += sub_n;
                bytes += sub_bytes;
            } else {
                bytes += std::fs::copy(&from, &to)
                    .with_context(|| format!("複製 {}", from.display()))?;
                n += 1;
            }
        }
        Ok((n, bytes))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 自稱「全量」的匯出，不可以把她動過的手留在原地。
        ///
        /// SPEC §11.8 說 `sister export` 是全量匯出，「就算本專案死了」他手上
        /// 那份也要是完整的。`action-log.jsonl` 不在資料庫裡，所以它不會被
        /// `export_to` 帶走——這一條守的是那一行接線。
        #[test]
        fn a_full_export_takes_the_action_log_too() {
            let src = crate::ops::tmp::Tmp::new("export-actionlog-src");
            let dst = crate::ops::tmp::Tmp::new("export-actionlog-dst");
            // `export` 要求來源有資料庫，所以先讓它長出一顆。
            Db::open(&crate::db_path(&src.0)).expect("db");
            let log = sister_hands::ActionLog::in_data_dir(&src.0);
            log.append(&sister_hands::ActionEvent::Executed {
                at_ms: 1_700_000_000_000,
                action: sister_hands::ActionSnapshot::OpenUrl {
                    url: "https://kept.example".into(),
                },
                result: sister_hands::ExecutionResult::Succeeded {
                    detail: "ok".into(),
                },
            })
            .expect("append");

            run(&src.0, &dst.0, false).expect("export");

            let copied = sister_hands::ActionLog::in_data_dir(&dst.0);
            let raw = std::fs::read_to_string(copied.path())
                .expect("匯出的目錄裡沒有 action log——那份備份少了她動過的手");
            assert!(raw.contains("kept.example"), "{raw}");
        }

        /// 一次手都沒動過的時候，匯出不該憑空生一個空檔案出來。
        ///
        /// 「這個檔案不存在」和「這個檔案是空的」在還原之後是兩句不同的話。
        #[test]
        fn exporting_a_machine_that_never_acted_does_not_invent_an_action_log() {
            let src = crate::ops::tmp::Tmp::new("export-noactions-src");
            let dst = crate::ops::tmp::Tmp::new("export-noactions-dst");
            Db::open(&crate::db_path(&src.0)).expect("db");
            run(&src.0, &dst.0, false).expect("export");
            assert!(
                !sister_hands::ActionLog::in_data_dir(&dst.0).path().exists(),
                "憑空生出了一個 action log",
            );
        }

        /// `--to <資料目錄>/frames/backup --with-frames` 會讓複製一直往下複製
        /// 自己，實測蓋出兩百多層才被檔名長度擋住。但攔它的理由更簡單：放在被
        /// 備份的東西裡面的備份不是備份。
        #[test]
        fn a_backup_inside_the_thing_it_backs_up_is_refused() {
            let data = Path::new("/tmp/sister-x/data");
            for bad in [
                "/tmp/sister-x/data",
                "/tmp/sister-x/data/frames/backup",
                "/tmp/sister-x/data/backup",
            ] {
                assert!(
                    refuse_to_export_into_itself(data, Path::new(bad)).is_err(),
                    "{bad} 在資料目錄裡面"
                );
            }
            for ok in ["/tmp/sister-x/backup", "/tmp/elsewhere", "/tmp/sister-x"] {
                assert!(
                    refuse_to_export_into_itself(data, Path::new(ok)).is_ok(),
                    "{ok} 不在資料目錄裡面"
                );
            }
        }

        /// 「0 個畫面檔」有兩種意思，而它們對一份備份來說天差地遠。
        #[test]
        fn zero_pictures_because_there_were_none_reads_differently_from_zero_because_they_are_gone()
        {
            let src = Path::new("/tmp/sister-x/data/frames");

            // 只記字的那份記憶：資料庫也說沒有圖，那就沒有壞掉。
            let none_to_take = frames_line(0, 0, 0, 0, None, src);
            assert!(none_to_take.starts_with("  ✓"), "{none_to_take}");
            // 講的是**現在資料庫裡有什麼**，不是「這份記憶本來就只有字」——
            // 圖過了保留期被清掉的那一份也長這樣，而那些圖存在過。
            assert!(
                none_to_take.contains("沒有任何一列說自己有圖"),
                "{none_to_take}"
            );
            assert!(
                !none_to_take.contains("本來就"),
                "不要講一句關於歷史的話：{none_to_take}"
            );

            // 資料庫說有 120 張，硬碟上一張都沒複製到。同樣是「0 個畫面檔」，
            // 但這一次他手上那份備份是缺的，而他要能當場看出來。
            let gone = frames_line(0, 0, 120, 0, None, src);
            assert!(gone.starts_with("  ✗"), "少了東西不該掛 ✓：{gone}");
            assert!(gone.contains("120"), "要講資料庫說有幾張：{gone}");
            assert!(gone.contains("少了 120 張"), "要講差多少：{gone}");
            assert!(
                gone.contains("/tmp/sister-x/data/frames"),
                "要講去哪裡比對：{gone}"
            );

            // 少一部分也是少。
            let partial = frames_line(118, 4096, 120, 0, None, src);
            assert!(partial.starts_with("  ✗"), "{partial}");
            assert!(partial.contains("少了 2 張"), "{partial}");

            // 全部帶到了就安靜地報數。多出來的（有人往 frames\ 丟過別的檔）
            // 不算少，也不值得為它多講一句。
            let all = frames_line(120, 1_048_576, 120, 0, None, src);
            assert!(all.starts_with("  ✓"), "{all}");
            assert!(all.contains("120 個畫面檔"), "{all}");
            assert!(frames_line(121, 1_048_576, 120, 0, None, src).starts_with("  ✓"));
        }

        /// **多出來的檔案會把少掉的那一張遮住。**
        ///
        /// 她一邊錄一邊匯出——README 就是這樣教的，而且 `db.rs` 有一條測試
        /// 專門守著那個情境。匯出開始的那一秒資料庫說有 120 列，複製走到
        /// 一半 recorder 又寫了三張新的，於是複製到 121 個檔。
        ///
        /// 只比數量的話 `121 >= 120`，掛一個 ✓。可是那 120 列裡有一張真的
        /// 沒過去（防毒攔了、路徑太長、磁碟滿了），而他手上這份備份缺了它。
        /// 一份自稱完整的備份，是最壞的那一種壞掉。
        #[test]
        fn a_surplus_of_new_files_must_not_hide_a_screenshot_that_did_not_make_it() {
            let src = Path::new("/tmp/sister-x/data/frames");

            let masked = frames_line(121, 1_048_576, 120, 1, Some("2026/08/19/0-abc.png"), src);
            assert!(
                masked.starts_with("  ✗"),
                "數量夠不代表對得上，這裡不能掛 ✓：{masked}"
            );
            assert!(masked.contains("1 列"), "要講有幾條打不開：{masked}");
            assert!(
                masked.contains("2026/08/19/0-abc.png"),
                "要講出是哪一條，他才驗得下去：{masked}"
            );
        }

        /// 沒帶走的那一行也要分得開「留在原地」和「本來就沒有」。
        ///
        /// 上面那一條測的是 `--with-frames` 那半邊，而同一個區別在**預設**
        /// 那半邊漏掉了——預設才是大部分人跑的那一條。
        #[test]
        fn nothing_left_behind_is_not_the_same_as_screenshots_left_behind() {
            // text-only 的機器，或者圖全過了保留期。留在原地的是零張。
            let nothing = frames_skipped_line(0);
            assert!(
                !nothing.contains("留在原地"),
                "零張畫面不可以說「留在原地」：{nothing}"
            );
            assert!(
                !nothing.contains("--with-frames"),
                "那顆旋鈕改不了這個結果，不要叫他去按：{nothing}"
            );

            // 真的有圖沒帶走：講得出幾張，旋鈕也真的有用。
            let left = frames_skipped_line(52_000);
            assert!(left.contains("52000"), "要講幾張：{left}");
            assert!(
                left.contains("--with-frames"),
                "這時候那顆旋鈕才有用：{left}"
            );
        }
    }
}

/// 「剛剛那段當作沒發生過。」
///
/// 這個子命令是補一個**已經被承諾出去的東西**。CLI 自己的說明第一句就寫著
/// 「讓你查得到、看得見出處、刪得掉」，PRIVACY.md 的〈停用不等於刪除〉寫著
/// 「已經記下來的還在，要另外刪」，而 `sister doctor` 更直接：關掉題庫的時候
/// 它會說「以前記的 N 題還在，`sister forget` 帶得走」——那個指令當時**並不
/// 存在**。刪除的實作一直都在（`Db::forget`），只是唯一的入口是字母人的
/// 時間軸，也就是一個在這台開發機上開不起來的視窗。
///
/// 一句「要另外刪」配上一個不存在的指令，比不提刪除更糟：他會以為自己刪過了。
pub mod forget {
    use super::*;

    /// 刪完之後，`sessions` 那張表上還剩什麼——沒有的話回 `None`。
    ///
    /// 見 [`sister_core::db::DbStats::only_session_shells_left`]。這一句只在
    /// 「窗裡的東西被刪光、而那張表還有列」的時候出現。
    ///
    /// `beat` 來自 `heartbeat::presence`，決定那一列是**當掉的**還是**活的**，以及
    /// 它什麼時候會走——見 [`session_shell_why`]。
    fn session_shell_note(
        s: &sister_core::db::DbStats,
        beat: sister_core::heartbeat::Presence,
    ) -> Option<String> {
        s.only_session_shells_left().then(|| {
            let (why, then) = session_shell_why(beat);
            format!(
                "  留著 {} 場錄製的紀錄本身：{}，裡面一列都不剩。\n     \
                 `sister stats` 的「工作階段」會是這個數字。{then}",
                s.sessions, why
            )
        })
    }

    pub fn run(data_dir: &Path, last: &str, yes: bool) -> Result<()> {
        let span = parse_span(last)?;

        // **這個「現在」只屬於刪除區間，所以它在這個大括號裡就死掉。**
        //
        // 區間的右界該錨在**他開口的那一秒**——他打 `--last 2h` 的意思是「從我
        // 按下去往回兩小時」，不是「從資料庫開完往回兩小時」。所以這裡取一次。
        //
        // 而下面還有第二個問題要問時間：心跳還新鮮嗎。那一題問的是**此刻**，
        // 兩題中間隔著一個 `Db::open`——一顆被硬砍掉的資料庫要跑 WAL 回復、一顆
        // 剛升級的要跑 migration，都可以是幾十秒到幾分鐘。拿這個舊的時戳去減，
        // 差值會偏小，於是一顆早就停掉的心跳被判成活的，`--yes` 前那句最重的話
        // （「她剛才一直在錄，所以這一刀之後寫進去的東西還在」）就變成對一組不
        // 存在的列下斷言。
        //
        // 一個變數回答兩個問題，就是這個 repo 一路在修的那件事。這裡沒有測試擋
        // 得住它（`Db::open` 要花多久沒辦法在測試裡注入），所以改用大括號擋：
        // `asked_at` 出了這一段就不存在，下面想重用它會編不過。
        let (from, to) = {
            let asked_at = sister_core::now_ms();
            (asked_at - span, asked_at)
        };

        // **action log 不住在資料庫裡，所以它的刪除不能掛在資料庫後面。**
        // 底下那個 `path.exists()` 會在沒有資料庫的時候直接印「沒有東西可以忘」
        // 然後回去——而 `action-log.jsonl` 是資料夾裡另一個檔案，裡面有完整的
        // 網址和檔案路徑。掛在後面的話，那句「沒有東西可以忘」會變成假話。
        //
        // 只在真的要刪的時候動手：預覽那條路一個位元組都不准碰（那是 `--yes`
        // 前面那段文案親口答應的）。
        //
        // 字母人那一側是同一句話（`apps/desktop/src-tauri/src/main.rs` 的
        // `forget`），兩邊都要做。
        //
        // 這一句會印在「要忘掉的是 X 到 Y」**前面**，因為那一行在資料庫檢查
        // 後面。所以它自己要把區間講完整，不可以寫成「那一段」——一句依賴下
        // 一行才讀得懂的話，就是一句在這個位置讀不懂的話。
        if yes {
            let gone = sister_hands::ActionLog::in_data_dir(data_dir).forget_range(from, to)?;
            if gone.removed_in_range > 0 || gone.removed_unreadable > 0 {
                println!(
                    "動作紀錄（{} 到 {}）：忘掉 {} 列{}。",
                    crate::fmt::timestamp(from.max(0)),
                    crate::fmt::timestamp(to),
                    gone.removed_in_range,
                    // 讀不懂的那幾列問不出時間，所以一起走了。這件事要講——不講
                    // 的話上面那個數字會被讀成「總共就刪了這些」。
                    if gone.removed_unreadable > 0 {
                        format!(
                            "，另外 {} 列讀不懂、問不出時間，一併刪掉",
                            gone.removed_unreadable
                        )
                    } else {
                        String::new()
                    }
                );
            }
        }

        // `query` 那邊查不到資料庫要報錯（他以為自己有資料），這裡不用：
        // 「沒有東西可以忘」是一個成功的結果。同一個 helper 套在兩種語意上
        // 會弄錯其中一個——`prune` 那邊有同一段註解。
        let path = crate::db_path(data_dir);
        if !path.exists() {
            println!("  還沒有資料庫（{}），沒有東西可以忘。", path.display());
            return Ok(());
        }
        let mut db = Db::open(&path).with_context(|| format!("open {}", path.display()))?;

        // 把區間用他看得懂的方式講一次。「最近 2 小時」到底是哪兩個時間點，
        // 只有在按下去之前看到才有意義。
        //
        // 而「看得懂」是這一行的全部意義。`--last 99999999d` 會把起點推到 1970
        // 年以前，這一行就印出 `ts:-8638212780293712`——那正是他要拿來決定按不
        // 按下去的那一行。功能上沒壞（往前刪到底就是刪光），壞的是他讀不到自己
        // 正要做什麼。夾在 0 之後也不印那個日期：`1969-12-31` 一樣看不懂，而且
        // 它想講的其實是「全部」，那就直接講「全部」。
        let from = from.max(0);
        if from == 0 {
            println!(
                "要忘掉的是**全部**，直到 {}——這個長度往回超過了她開始記錄的那一天。",
                crate::fmt::timestamp(to)
            );
        } else {
            println!(
                "要忘掉的是 {} 到 {}（{}）。",
                crate::fmt::timestamp(from),
                crate::fmt::timestamp(to),
                crate::fmt::duration_ms(span)
            );
        }

        // 她還在錄的話，這個指令做不到他以為的那件事，而且有兩個理由：
        //
        // 1. 忘掉的右界是「現在」，但下一個 tick 幾毫秒後就到——他最想忘掉的
        //    那一幀，很可能正好落在這一刀後面。
        // 2. 就算刀切得準，那個畫面通常還在螢幕上，下一秒又被記一次。
        //
        // 所以這句話要在**預覽**那一段就講：那才是他還能先去按暫停的時刻。
        // 刪完之後再講就只是一句「你剛剛白做了」。
        //
        // **問的是 `phase` 不是 `is_occupied`。** 上一版拿「有沒有人佔著」去印
        // 「她現在還在錄」，而 `heartbeat` 那份註解早就寫著這兩題不可以共用一個
        // 述詞。開機那幾分鐘（`Db::open` 在一顆一年份的資料庫上要跑好幾分鐘）她
        // 一列都還沒寫，於是 `--yes` 那一句「她剛才一直在錄，所以這一刀之後寫進
        // 去的東西還在」是在對一組**不存在的列**下斷言——而同一秒的 `doctor` 說
        // 「有一個 sister record 正在起來……還沒開始記東西」。
        //
        // 三種心跳三句話，而**下一步都一樣**（先 `sister pause`）：開機中的那一
        // 個再過幾秒就開始記，而他最想忘掉的那個畫面多半還在螢幕上。理由一樣要
        // 講對——一句理由錯了的正確建議，下一次他就不信了。
        //
        // **這裡重新問一次「現在」**，不是重用上面算區間用的那個。理由寫在
        // `let (from, to)` 那一段：中間隔著一個可以跑好幾分鐘的 `Db::open`。
        let beat = sister_core::heartbeat::presence(data_dir, sister_core::now_ms());
        let recording = matches!(
            beat,
            sister_core::heartbeat::Presence::Live(sister_core::heartbeat::Phase::Recording)
        );
        let booting = matches!(
            beat,
            sister_core::heartbeat::Presence::Live(sister_core::heartbeat::Phase::Booting)
        );

        // 「什麼都沒記到」以前直接 return，所以那句警告在**最需要它的那一次**
        // 反而不會出現：她一個 tick 一個 tick 寫，他剛剛做的那件事很可能還沒
        // 落進資料庫。那時候「不用忘」讀起來像「你沒事」，而三秒後就有事了。
        let report = db.forget_preview(from, to, Some(&prune::frames_dir(data_dir)))?;
        if report.is_empty() {
            // 講明主詞是**錄到的東西**。上面那一行可能剛印完「動作紀錄：忘掉
            // 1 列」——她那段時間動過手，只是沒錄到畫面或文字。光禿禿一句
            // 「什麼都沒記到」接在那行後面，兩句各自都是真的，湊起來在打架。
            println!("  那段時間裡她什麼畫面和文字都沒記到，不用忘。");
            // **「什麼都沒記到」不等於那段時間在資料庫裡沒有留下任何一列。**
            // 上一次刪完之後剩下的那個空殼，`started_at` 就落在這個窗裡，而
            // `count_empty_sessions` 帶著同一道守衛，所以預覽是空的——於是這
            // 一句把一列還躺在資料庫裡的紀錄講成「沒有東西」。
            //
            // 再跑一次 `forget` 確認，和刪完問一句「真的沒了嗎」一樣自然，而
            // 那正是這一批 bug 的第五次。時間軸那邊同一個分支已經接上了
            // （`timeline.js` 的「沒有東西被刪掉」），這裡當時漏了。
            if let Some(line) = session_shell_note(&db.stats()?, beat) {
                println!("{line}");
            }
            if recording {
                println!(
                    "\n⚠  **但她現在還在錄。** 剛剛那一段可能只是還沒寫進資料庫——\n   \
                     真的想清掉就先 `sister pause`，過一下再跑一次這個指令。"
                );
            } else if booting {
                println!(
                    "\n⚠  **有一個 sister record 正在起來**（多半在開資料庫），還沒開始記\n   \
                     東西——所以剛剛那一段真的沒有被記到。但它馬上就要開始記了：\n   \
                     真的不想被記就先 `sister pause`。"
                );
            }
            return Ok(());
        }

        if !yes {
            prune::print_report(&report, true);
            if recording {
                println!(
                    "\n⚠  **她現在還在錄。** 先 `sister pause` 再刪——不然你最想忘掉的\n   \
                     那一幀可能正好在這一刀後面被寫進去，而且那個畫面多半還在螢幕上，\n   \
                     下一個 tick 就又被記一次。處理完再 `sister resume`。"
                );
            } else if booting {
                println!(
                    "\n⚠  **有一個 sister record 正在起來**（多半在開資料庫）。它還沒開始\n   \
                     記，但你按下 `--yes` 的時候多半已經開始了——而你最想忘掉的那個畫面\n   \
                     多半還在螢幕上。先 `sister pause` 再刪，處理完再 `sister resume`。"
                );
            }
            // `--last` 是從**跑的那一刻**往回算，而 `--yes` 那一次是另一個
            // 行程、另一個「現在」。上面那兩個時間點會整段往後挪掉他讀這段
            // 話的時間，於是起點前面的那幾分鐘留了下來——而畫面剛剛才把它們
            // 算進「要忘掉的」裡面。差幾分鐘不是功能問題，是這一頁做了一個
            // 它不打算遵守的承諾。
            println!(
                "\n這是預覽，一個位元組都沒動。真的要忘掉就再跑一次，加上 `--yes`：\n  \
                 sister forget --last {last} --yes\n\
                 **沒有回收桶，也沒有復原。**\n\
                 （`--last` 是從跑的那一刻往回算，所以那一次的區間會比上面整段晚一點\n  \
                 ——你讀這段話的時間會從頭那邊掉出去。想連那幾分鐘一起忘就寫長一點。）"
            );
            return Ok(());
        }

        let report = db.forget(from, to, Some(&prune::frames_dir(data_dir)))?;
        prune::print_report(&report, false);
        // **沒被帶走的那一列，也要當場說。** 報告只講刪掉了什麼，於是「那一場
        // 當掉了所以留下來」這件事在這裡是靜音的——他下一次跑 `sister stats`
        // 才會看到一個「工作階段 1」站在一整排 0 旁邊，而那時候沒有任何東西
        // 把它接回這一刀。
        //
        // 預覽那邊不必配一句：`count_empty_sessions` 帶著同一道守衛（見
        // `retention.rs` 的 `delete_empty_sessions`），所以它從頭到尾就沒有
        // 答應過要刪這一列。這裡補的是「答應之外還剩什麼」。
        //
        // `beat` 是上面那一段讀好的（`heartbeat::phase`）。有它，這一句就不必
        // 印「當掉了，或是她此刻正在錄」——底下四行才剛用同一份心跳斷言「她剛
        // 才一直在錄」。
        if let Some(line) = session_shell_note(&db.stats()?, beat) {
            println!("{line}");
        }

        if recording {
            println!(
                "\n⚠  她剛才一直在錄，所以這一刀之後寫進去的東西還在——包含你可能\n   \
                 最想忘掉的最後那一幀。先 `sister pause`，再跑一次這個指令。"
            );
        } else if booting {
            // **不可以說「她剛才一直在錄」。** 開機那一段她一列都沒寫，所以這一
            // 刀後面沒有東西——那句話會對著一組不存在的列下斷言。但下一步一樣：
            // 它正要開始。
            println!(
                "\n⚠  這一刀切在那個 sister record 開始記之前，所以刪掉的是舊的。\n   \
                 但它正在起來，馬上就要開始記了——而你最想忘掉的那個畫面多半還在\n   \
                 螢幕上。先 `sister pause`。"
            );
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// `sister forget --yes` 要把 action log 那一段也帶走。
        ///
        /// **這一條守的是接線，不是 `forget_range` 本身。** 那支函式在
        /// `sister-hands` 有自己的測試；這裡要擋的是「有人把這一行從 `run`
        /// 裡拿掉」——那種 bug 底下每一條測試都還是綠的，而畫面上照樣寫著
        /// 「已經忘掉了」。這個 repo 有一整層接線是零執行覆蓋的，這一行不要
        /// 變成下一個。
        ///
        /// 故意連資料庫都不建：那條路會在 `path.exists()` 就掉頭回去印
        /// 「沒有東西可以忘」。如果 action log 的刪除掛在資料庫後面，這一條會
        /// 紅——而那正是它一開始被我放錯的位置。
        #[test]
        fn forgetting_takes_the_action_log_with_it_even_with_no_database() {
            let tmp = crate::ops::tmp::Tmp::new("forget-actionlog");
            let log = sister_hands::ActionLog::in_data_dir(&tmp.0);
            let now = sister_core::now_ms();
            let put = |ms: i64, url: &str| {
                log.append(&sister_hands::ActionEvent::Executed {
                    at_ms: ms,
                    action: sister_hands::ActionSnapshot::OpenUrl { url: url.into() },
                    result: sister_hands::ExecutionResult::Succeeded {
                        detail: "ok".into(),
                    },
                })
                .expect("append");
            };
            put(now - 3_600_000, "https://inside.example");
            put(now - 30 * 86_400_000, "https://longago.example");

            // 預覽不准動任何一個位元組——`--yes` 前面那段文案是這樣答應的。
            run(&tmp.0, "1d", false).expect("preview");
            let raw = std::fs::read_to_string(log.path()).expect("read");
            assert!(raw.contains("inside.example"), "預覽刪了東西：{raw}");

            run(&tmp.0, "1d", true).expect("forget");
            let raw = std::fs::read_to_string(log.path()).expect("read");
            assert!(
                !raw.contains("inside.example"),
                "那一小時開過的網址還在磁碟上：{raw}",
            );
            assert!(
                raw.contains("longago.example"),
                "範圍外的那一列不該被帶走：{raw}",
            );
        }

        #[test]
        fn the_units_it_understands() {
            assert_eq!(parse_span("30m").expect("30m"), 30 * 60_000);
            assert_eq!(parse_span("2h").expect("2h"), 2 * 3_600_000);
            assert_eq!(parse_span("7d").expect("7d"), 7 * 86_400_000);
            assert_eq!(parse_span(" 1d ").expect("空白"), 86_400_000);
        }

        /// 一個沒有單位的數字是這個指令最危險的輸入：猜錯一邊就是一次
        /// 不可逆的失憶。所以不猜。
        #[test]
        fn a_bare_number_is_refused_not_guessed() {
            let e = parse_span("30").expect_err("沒有單位就不該過");
            assert!(e.to_string().contains("少了單位"), "{e}");
        }

        #[test]
        fn nonsense_does_not_turn_into_a_deletion() {
            for s in ["", "h", "0h", "-3d", "abc", "2w", "2 h"] {
                assert!(parse_span(s).is_err(), "{s:?} 不該被讀成一段時間");
            }
        }

        /// **報告只講刪掉了什麼，於是「沒刪掉的那一列」是靜音的。**
        ///
        /// 最後一場當掉的話，`forget` 把窗裡的東西全帶走，卻印不出任何一句和
        /// 那一列有關的話——他要到下一次 `sister stats` 才看到「工作階段 1」，
        /// 而那時候已經沒有東西把它接回這一刀了。
        #[test]
        fn a_session_row_that_survives_the_erasure_is_disclosed_on_the_spot() {
            let shell = sister_core::db::DbStats {
                sessions: 1,
                ..Default::default()
            };
            use sister_core::heartbeat::{Phase, Presence};
            let gone = Presence::Stopped { at: None };
            let note = session_shell_note(&shell, gone).expect("留下來就要說");
            assert!(note.contains('1'), "{note}");
            assert!(
                note.contains("工作階段"),
                "要接得回 `stats` 上那一行：{note}"
            );

            // **那個下場是量出來的，不是推出來的。** 上一版寫「開錄時自動跑的
            // 那次 `sister prune` 會把它帶走」——而 `record` 的開機清理跑在
            // `start_session` **之前**，那一刀砍不到它（那時候它還是最新的一
            // 列）。見 `retention.rs` 那條照著真實順序寫的測試。
            assert!(
                !note.contains("開錄時"),
                "開機清理跑在 start_session 之前，砍不到這一列：{note}"
            );
            assert!(
                note.contains("再開始錄之後"),
                "要說清楚是「開始錄**之後**」那幾次清理才帶得走：{note}"
            );
            assert!(note.contains("當掉"), "沒有人佔著，那就是當掉了：{note}");

            // 她**正在錄**的時候：那一列是活的，下場也不一樣（收工時
            // `end_session` 自己會掃它）。不可以說她當掉了。
            let live =
                session_shell_note(&shell, Presence::Live(Phase::Recording)).expect("留下來就要說");
            assert!(!live.contains("當掉"), "她此刻正在錄：{live}");
            assert!(live.contains("收工"), "活的那一列走的是另一條路：{live}");

            // **而「正在起來」那一格跟當機走，不跟活的走。** 開機那幾分鐘她那
            // 一列還沒 INSERT，所以手上這一列不可能是她的——上一版收
            // `occupied = beat.is_some()`，於是這裡印「此刻有人佔著這個資料目錄
            // （她正在錄，或正在開機）」，而同一秒的 `doctor` 說那是上一次當機
            // 留下來的殼。
            let booting =
                session_shell_note(&shell, Presence::Live(Phase::Booting)).expect("留下來就要說");
            assert!(
                booting.contains("當掉"),
                "那一列不是正在起來的那一個：{booting}"
            );
            assert!(
                booting.contains("正在起來"),
                "而那個 recorder 看得到，等待也比較短：{booting}"
            );
            assert!(
                !booting.contains("正在錄"),
                "她還在開資料庫，一列都還沒寫：{booting}"
            );
            // **後半句也要釘。** 三句話各有一個「什麼時候會走」，而它們不是同
            // 一件事：活的那一列等她收工，這一列等**別人**開始錄。上一版只釘了
            // 前半句（`why`），於是把這裡的 `then` 換成活的那一格照樣全綠——
            // `sessions_line` 只取 `why`，而 CI 走的是 `forget` 的預覽那條路，
            // 根本印不到這一句。突變測試當場活下來（M33），unit 和 ci 雙雙。
            assert!(
                booting.contains("等它開始錄"),
                "等的是那個正在起來的 recorder，不是她：{booting}"
            );
            assert!(
                !booting.contains("收工"),
                "那一列不是她的，她收工不會帶走它：{booting}"
            );

            // 按下停止之後那兩分鐘：這一列是她的，行程還在。不可以說當掉、
            // 也不可以說沒有人佔著。
            let thinking = session_shell_note(
                &shell,
                Presence::Thinking {
                    at: 1,
                    until: 240_000,
                },
            )
            .expect("留下來就要說");
            assert!(!thinking.contains("當掉"), "她還在收尾：{thinking}");
            assert!(
                !thinking.contains("沒有任何 recorder"),
                "行程還在：{thinking}"
            );
            assert!(thinking.contains("想最後一段"), "{thinking}");

            // 四種心跳四句話，兩兩不同。任何兩種撞在一起，就是又有一組不同的
            // 處境被印成同一行。
            let four = [&note, &live, &booting, &thinking];
            for (i, a) in four.iter().enumerate() {
                for b in &four[i + 1..] {
                    assert_ne!(a, b, "四種狀態的下一步不一樣，不可以共用一句話");
                }
            }

            // 東西還在的時候沒有這一句：那一場不是殼，它本來就該留著。
            for beat in [
                gone,
                Presence::Live(Phase::Booting),
                Presence::Live(Phase::Recording),
                Presence::Thinking {
                    at: 1,
                    until: 240_000,
                },
            ] {
                assert!(
                    session_shell_note(
                        &sister_core::db::DbStats {
                            sessions: 1,
                            chunks: 6,
                            ..Default::default()
                        },
                        beat
                    )
                    .is_none(),
                    "她還記著 6 段字，那一場不是空殼"
                );
                // 整張表空了也沒有這一句——`delete_empty_sessions` 帶走了，
                // 而報告裡「刪掉了 N 場錄製的紀錄本身」那一行才是講它的。
                assert!(session_shell_note(&sister_core::db::DbStats::default(), beat).is_none());
            }
        }
    }
}

pub mod query {
    use super::*;
    use crate::fmt;
    // 下面的定點測試直接問 ★ 那一層；產品接線改走 `retrieval` 共用入口。
    #[cfg(test)]
    use sister_core::answer::answers;

    /// 她拿去比對的字是**黏出來的**的時候要補的那兩行（`None` = 沒黏過，閉嘴）。
    ///
    /// `question::terms` 會把「剛剛」「那個」剝掉，剝到不足兩個字還會往回退一
    /// 格——而那一格常常退進虛字裡：「剛剛那個板」→「個板」、「剛剛看到的人」
    /// →「的人」。標題那一行印的是**他打的字**，所以「『剛剛那個板』 20 筆
    /// 原文」讀起來像她找到了二十件關於板子的事，而那二十筆是拿「個板」比出
    /// 來的。空手的那一半更平：他打的字真的沒出現過，跟她根本沒找他打的字，
    /// 印出來是同一句「沒有找到」。前者他無能為力，後者他重打一個詞就好。
    ///
    /// 只在黏過的時候講。剝掉「剛剛那個」留下「優惠方案」是剝對了，每次都報
    /// 一句只會讓人學會忽略它。見 [`sister_core::question::terms_with_retreat`]。
    /// 時間範圍那一區。只有呼叫端同時握有 `TimeRange` 和算過的章節時才印，
    /// 所以空清單的意思是「算過、沒有段落」，不是「沒算過」。
    ///
    /// 章節是活動級：被 10 分鐘上限切碎的同質段已經併回去。時長用核心時間，
    /// 鐘面也用核心——margin 起迄在分鐘解析度上會變成「13:59–14:45、45 分鐘」
    /// 這種對不上的兩句話。
    fn chapter_lines(
        range: &sister_core::question::TimeRange,
        chapters: &[sister_core::activity::Activity],
    ) -> Vec<String> {
        let mut out = vec![format!(
            "你問的是「{}」，那段時間是 {} 到 {}",
            range.said,
            fmt::timestamp(range.from),
            fmt::timestamp(range.to)
        )];
        if chapters.is_empty() {
            // 算過了。不是「沒在錄」，也不是「你沒有紀錄」——那兩句她這一層
            // 沒查過。
            out.push("那段時間沒有切得出來的段落。".to_string());
            return out;
        }
        out.push(format!("那段時間分成 {} 段：", chapters.len()));
        for s in chapters {
            let dur = crate::fmt::duration_ms(s.core_ms().max(0));
            let how_long = if s.segment_count > 1 {
                format!("{dur}，{} 段併成", s.segment_count)
            } else {
                dur
            };
            let what = match (s.app.as_deref(), s.title.as_deref()) {
                (Some(a), Some(t)) => format!("{a}  「{t}」"),
                (Some(a), None) => a.to_string(),
                (None, Some(t)) => t.to_string(),
                (None, None) => s.host.clone().unwrap_or_else(|| "一段紀錄".to_string()),
            };
            out.push(format!(
                "  · {}–{}  {what}（{how_long}）",
                clock(s.core_started_at),
                clock(s.core_ended_at)
            ));
        }
        out
    }

    fn clock(ts: sister_core::Millis) -> String {
        use chrono::{Local, TimeZone};
        match Local.timestamp_millis_opt(ts).single() {
            Some(dt) => dt.format("%H:%M").to_string(),
            None => fmt::timestamp(ts),
        }
    }

    fn glued_note(text: &str) -> Option<[String; 2]> {
        let (asked, glued) = sister_core::question::terms_with_retreat(text);
        glued.then(|| {
            [
                format!("   我拿去比對的是「{asked}」——那是從你打的字黏出來的，不是一個詞。"),
                "   直接打你要的那個詞再問一次。".to_string(),
            ]
        })
    }

    /// 把「一筆都沒找到」的那幾個查得到的理由講成人話。
    ///
    /// 順序是有意的：她還沒開始記，就沒有第二句好講；記了才輪得到「你自己叫我
    /// 別看那個」和「那段時間我閉著眼」。全部都不成立的時候只剩一句實話——
    /// 她記了，而裡面就是沒有。那句話沒有安慰的成分，但它是真的。
    fn blind_lines(b: &sister_core::answer::BlindSpots) -> Vec<String> {
        let mut out = Vec::new();
        // 讀字斷掉要**單獨先問**，不能掛在 `chunks == 0` 底下。
        //
        // 那一支本來寫在下面那個 `if` 裡，於是它守的是「她一段字都沒有，而且
        // 看過畫面」。可是 OCR 全死的機器上 `chunks` 不是 0——`insert_focus`
        // 每次換視窗就寫一列視窗標題、一列網址進 `text_chunks`，兩種都不經過
        // OCR。真正壞掉的那台機器於是掉到最後那句「她記的每一段裡都沒有這個
        // 字」，和一台一切正常的機器一模一樣，而正確的下一步從來沒被講出口。
        //
        // 提早收工的理由和舊版一樣：畫面明明留下來了，暫停和排除都解釋不了
        // 「這幾張畫面裡沒有字」，多講只會把人帶偏。
        if b.ocr_is_dead() {
            out.push(format!(
                "她看過 {} 張畫面，但一個字都沒讀出來——讀字那一段是斷的，跑 `sister doctor` 看是哪一種。",
                b.frames
            ));
            return out;
        }
        if b.chunks == 0 {
            // 「一段字都沒有」有兩種，而它們的下一步是相反的。
            //
            // 她看過畫面卻一個字都沒讀出來，代表 OCR 這一段是斷的（關掉了、
            // 或者裝了讀不到）——那是這個專案已知的主要故障形狀。這時候叫他
            // 「先跑 `sister record`」，只會讓他再錄一天空的。
            //
            // 而「連畫面都沒有」也還有兩種：她從來沒錄過，或者錄過的東西被
            // `sister forget` 忘掉了／過了保留期。`sessions` 那張表不在任何
            // 保留期的射程內，所以這件事問得出來——問不清楚就會叫他重做一件
            // 他剛剛才故意做掉的事。
            //
            // ——但**不是兩種，是四種**，而舊版在這裡直接 return，把手上另外
            // 兩張稽核表丟掉了。從頭暫停到尾的那一小時、或者被一條排除規則
            // 整段擋掉的那一小時，走到的都是同一組數字（chunks=0、frames=0、
            // sessions>0），而它們得到的是「被 `sister forget` 忘掉了」。
            // 那是四種原因裡唯一一個假的，也是唯一一個會讓他以為東西被刪了
            // 的。底下 `excluded` 和 `paused_episodes` 兩段本來就會把真正的
            // 原因講出來，所以這裡改成不 return，讓它們接著講。
            let blocked = b.paused_episodes > 0 || !b.excluded.is_empty();
            out.push(if b.frames > 0 {
                // 上面那道 `ocr_is_dead()` 已經把「夠多張畫面、一行字都沒有」
                // 那一種攔走了，所以走到這裡的是張數還太少的時候。三張畫面上
                // 剛好都沒有字是完全正常的事——這裡不指控 OCR。
                format!(
                    "她留下了 {} 張畫面，但還沒有任何一段字——多半是才剛開始。",
                    b.frames
                )
            } else if b.ever_recorded && blocked {
                "她錄過，但那段時間一張畫面都沒留下來——底下是查得出來的原因。".to_string()
            } else if b.recording_now && !b.ever_stored {
                // **「她正開著」和「一列都沒存過」同時成立的那一格。** 底下那
                // 句攤開三種可能，而其中一種在這台機器上是**不可能**的：一列
                // 都沒進來過，就沒有東西可以被忘掉或過期。
                //
                // 這一格是上一版**自己造出來的**：`recording_now` 問在
                // `!ever_stored` 前面，於是後者永遠輪不到一台正在錄的機器，而
                // 前者那句話裡帶著一則它證得出來是假的指控。攤開可能性也要先
                // 把不可能的那幾種扣掉——不然「攤開」就只是換一種猜法。
                "她正開著，可是到現在一列內容都還沒落地——多半是剛開始，再等一下。".to_string()
            } else if b.ever_recorded && b.recording_now {
                // 她**正在**錄。那句「被忘掉了，或是過了保留期」少了一種可能，
                // 而且正好是最常見的那一種：他三秒前才把 recorder 開起來。
                // 第一次用的人問的第一個問題就落在這裡，然後被告知他的紀錄
                // 被忘掉了。
                //
                // 不挑一邊：清空過的資料庫上她照樣可能正在錄，那時候兩件事
                // 都成立。把可能性列出來，不要替他選一個。
                "她正開著，但手上一段字都沒有——可能是剛開始，也可能是之前的被忘掉了或過期了。"
                    .to_string()
            } else if b.booting_now {
                // **開機那幾分鐘。** `recording_now` 在這裡是 false（她一拍都
                // 還沒跑），所以這一格以前掉到底下那兩句：一台什麼都還沒開始
                // 的機器被送去看 `capture.enabled`，或被告知東西被忘掉了。而
                // 同一個資料目錄上 `sister facts` 說的是「再等一下」——兩個指
                // 令，相反的下一步。
                //
                // 排在 `ever_stored` / `ever_recorded` 那幾條**前面**，因為
                // 它們講的是過去，而他問這一題的時候在等現在。
                "有一個 sister record 正在起來（多半在開資料庫），還沒開始記東西——再等一下。"
                    .to_string()
            } else if b.ever_recorded && !b.ever_stored {
                // 她跑過，而一列內容都沒進來過。底下那句「被忘掉了」在這台
                // 機器上是**指控一件沒發生的事**——他一次都沒刪過東西，該看
                // 的是 `capture.enabled`。兩種 0 分得出來是因為 `ever_stored`
                // 活在 `meta` 裡，而觸發器保證它只在真的落地時才按下去。
                "她錄過，但一列內容都沒存進來過——先看 `capture.enabled`（`sister doctor` 會直接說）。"
                    .to_string()
            } else if b.ever_recorded {
                "她錄過，但現在資料庫裡是空的——被 `sister forget` 忘掉了，或是過了保留期。"
                    .to_string()
            } else {
                "她還沒記過任何東西——先跑 `sister record`。".to_string()
            });
            // 這裡不再提早收工。以前 `frames > 0` 會直接 return，因為那時候它
            // 確定是 OCR 斷了；現在那個確定的情況在函式最上面就 return 掉了，
            // 剩下的是「才剛開始」——而排除規則和暫停照樣可能是真正的原因。
        }
        if !b.excluded.is_empty() {
            // 排除是**他自己設的**規則，所以這句話不是道歉，是提醒他去哪裡找。
            let why = b
                .excluded
                .iter()
                .map(|(reason, n)| format!("{reason} {n} 段"))
                .collect::<Vec<_>>()
                .join("、");
            // 一行就好。一個用了三個月的資料庫幾乎一定有排除紀錄，所以這句話
            // 會很常出現——講成三行的東西，第二次就沒有人在看了。
            //
            // 不寫死「你自己的規則」：同一張稽核表裡還躺著兩道**自動**防線
            // （`screenshare app:` 和 `password field focused`），那兩種他沒
            // 有寫過任何規則。講成他寫的，他會去三張排除清單裡找一條不存在的
            // 規則。理由字串本來就帶著前綴，讓它自己說。
            let his_own = b.excluded.iter().any(|(r, _)| r.starts_with("excluded "));
            let whose = if his_own {
                "你的排除規則（和自動防線）"
            } else {
                "自動防線"
            };
            out.push(format!(
                "不過{whose}擋掉過東西（{why}）——要找的如果在那裡面，她本來就不會知道。"
            ));
        }
        // 「她以前暫停過幾段」和「她**現在**閉不閉得了眼」是兩件事，而 alpha.40
        // 為止它們共用一條 if/else 鏈——`paused_now` 只在 `paused_episodes == 0`
        // 的那條 else 裡說得出話（那條 else 的註解自己還寫著「這一條比上面那條
        // 更需要講」，然後坐在會被上面那條擋掉的位置上）。
        //
        // 於是這台機器：錄的時候暫停又解除過一次、後來在沒有人在錄的時候又按
        // 了一次暫停——`query` 只說
        //
        //     她也被暫停過 1 次、一共 5 分鐘，那幾段是空的。
        //
        // 過去式，話說完了。而 `sister stats` 在**同一個資料目錄**上說「但她現
        // 在是暫停的」。兩個指令、相反的結論，而他讀完 query 會去按開始記錄，
        // 然後錄一整天的空白。
        //
        // 所以底下是兩個獨立的 `if`，各自決定自己要不要出現。
        if b.paused_episodes > 0 {
            // 這個數字算短的原因有**兩個**，而且互相獨立：最後一段還沒收尾、
            // 開頭被保留期刪掉的那幾段算不出長度。舊版把它們排成 if/else，於是
            // 一顆兩種都有的資料庫永遠說不出後者（`paused_truncated` 那條在
            // `paused_open` 為真的時候到不了）。兩個都要說得出口。
            //
            // 每一條都寫成短短的名詞片語，不帶自己的括號——整串等一下會被塞進
            // 一對括號裡，再套一層就變成括號中的括號，沒有人讀得下去。
            // 「為什麼沒收尾」那段解釋留在 `sister stats`，那一格有空間展開。
            let mut why_short: Vec<String> = Vec::new();
            if b.paused_open {
                // 「沒收尾」有兩種，而它們的原因相反：她**現在**還停著（那一段
                // 本來就還在跑），或者解除的時候沒有人在錄（所以沒有人寫下那一
                // 筆）。寫死其中一句，另一半就是假話。
                why_short.push(if b.paused_now {
                    "最後一段到現在都還沒解除".to_string()
                } else {
                    "最後一段沒有收尾".to_string()
                });
            }
            if b.paused_truncated > 0 {
                why_short.push(format!("有 {} 段的開頭已被保留期刪掉", b.paused_truncated));
            }
            // 時間只有在 pause 配得到 resume 的時候才累加。所以「一共 0 秒」有
            // 兩種意思：真的只停了一瞬間，和**一段都還沒結束、根本無從加起**。
            // 只有在真的加得出東西的時候才報那個數字。
            let how_long = match (b.paused_ms, why_short.is_empty()) {
                (0, false) => format!("長度還算不出來（{}）", why_short.join("、")),
                (ms, true) => format!("一共 {}", crate::fmt::duration_ms(ms)),
                (ms, false) => format!(
                    "算得出來的加起來 {}（{}，所以這個數字算短了）",
                    crate::fmt::duration_ms(ms),
                    why_short.join("、")
                ),
            };
            out.push(format!(
                "她也被暫停過 {} 次、{how_long}，那幾段是空的。",
                b.paused_episodes
            ));
        }
        if b.paused_now {
            // 只有旗標答得出「現在」（見 `BlindSpots::paused_now`）。錄製當中按
            // 暫停、關掉 recorder、事後才解除——`CaptureResumed` 沒有人寫，資料
            // 庫從此永遠掛著一段沒收尾的暫停，所以 `paused_open` 不是「現在」。
            //
            // 這一句講的是**接下來**：他下一次按開始記錄，會開起來然後什麼都不
            // 記。所以它比上面那句重要，而且和上面那句同時成立。
            //
            // 指令要指得到。舊版寫的是 `sister pause --off`——`sister pause` 一
            // 個旗標都沒有，照著打會拿到 usage error（exit 2）。全 repo 只有那
            // 一處這樣寫，另外五處都是對的 `sister resume`。他當時正瞎著，而她
            // 給的唯一一條路走不通。
            let lead = if out.is_empty() { "她" } else { "而且她" };
            out.push(format!(
                "{lead}**現在是暫停的**（`sister resume` 解除）——這樣錄也不會記到東西。"
            ));
        }
        // 沒有任何理由的時候只剩一句實話。而「每一段」這三個字要看她這次
        // 到底翻了多少：只掃了 30 天卻說「每一段」，是把十二分之一講成全部。
        if out.is_empty() {
            out.push(if b.scan_horizon_days.is_some() {
                "她翻過的那幾段裡沒有這個字。".to_string()
            } else {
                "她記的每一段裡都沒有這個字。".to_string()
            });
        }
        if let Some(days) = b.scan_horizon_days {
            // 括號裡那句以前寫的是「產不出相鄰雙字的，例如單獨一個中文字」
            // ——那是 `bigram_query` 的條件，不是這句話成立的條件。純英數的
            // 查詢一個相鄰雙字都產不出來，卻由 trigram 蓋著整張表；用那個
            // 條件去印這一句，等於對每一個查錯誤碼、檔名、網址片段的人，把
            // 一次看完了 365 天的搜尋說成只翻了 30 天。見 `covered_by_index`。
            out.push(format!(
                "——但這種查法（每個詞都太短，索引比不出來：一個中文字，或兩個以內的英數）\
                 只能掃最近 {days} 天，更早的這次沒翻到。多打一個字就走得到索引。"
            ));
        }
        out
    }

    pub fn run(
        data_dir: &Path,
        text: &str,
        limit: usize,
        json: bool,
        query_log: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            !text.trim().is_empty(),
            "要查什麼？例如：sister query 客服電話"
        );
        let mut db = open_existing(data_dir)?;

        let now = sister_core::now_ms();
        let close = sister_core::reviewer::close_from_message(&mut db, text, now)?;
        let previous = sister_core::reviewer::followup_state(&db)?;
        let followup =
            match sister_core::followup::decide(&db.live_commitments()?, now, previous.as_ref()) {
                sister_core::followup::FollowupDecision::Ask {
                    commitment_id,
                    text,
                } => {
                    sister_core::reviewer::record_followup(&mut db, commitment_id, now)?;
                    Some(text)
                }
                sister_core::followup::FollowupDecision::NoEligibleCommitment
                | sister_core::followup::FollowupDecision::CoolingDown { .. } => None,
            };

        // 「剛剛發生什麼事」問的是時間，不是字。規則在 core，和字母人共用同一
        // 份——兩邊各判各的，同一句話遲早會在兩個地方得到兩種答案。
        // 計時涵蓋兩條路徑：使用者感受到的是整個回答的延遲，不是單一次查詢。
        let started = std::time::Instant::now();
        let retrieval = sister_core::retrieval::RetrievalProfile::TextAndFacts
            .retrieve(&mut db, text, limit)?;
        // 章節在檢索之後另算。不進 retrieval：recall harness 要求每一筆都
        // 對得回單一 `at_ms`，而章節是一個範圍。
        let asked_chapters = db.chapters_for_question(text, sister_core::now_ms())?;
        let elapsed = started.elapsed();
        let sister_core::retrieval::Retrieval {
            shape,
            terms,
            answers,
            hits,
            answers_truncated,
            hits_truncated,
            ..
        } = retrieval;

        // 進題庫。PHASES.md Phase 2 的退場條件要「≥ 30 題來自真實 query log」，
        // 而那種東西補建不回來——沒有人記得住自己上禮拜是用什麼字問的。
        //
        // 記不進去不算失敗：他要的是答案，不是一筆紀錄。但要講一次，不然
        // 題庫會安靜地停止累積，而唯一的症狀是幾個月後發現它是空的。
        if query_log
            && let Err(e) = db.log_query(&sister_core::db::QueryLogEntry {
                ts: sister_core::now_ms(),
                question: text,
                shape: shape.name(),
                // ★ 答案和原文一起數。只數 `hits` 的那一版把 `sister query
                // 電話` 記成「一筆都沒找到」——電話號碼是從 L1 事實那一欄
                // 來的，全文比對確實是 0 筆，而那正是這個產品最典型的一次
                // 成功。題庫裡最有價值的是「她答不出來」的那些題，所以這個
                // 數字必須數他**看到了什麼**，不是內部走了哪一條路。
                hits: answers.len() + hits.len(),
                latency_ms: elapsed.as_millis() as i64,
                source: sister_core::db::SOURCE_CLI,
            })
        {
            eprintln!("  ⚠ 這一題沒記進題庫：{e}");
        }

        if json {
            let out = serde_json::json!({
                "query": text,
                // 寫腳本的人要分得出這一份是比對來的還是時間來的：`recent`
                // 的 hits 和 query 裡的字沒有任何關係，當成搜尋結果去解讀
                // 會得到完全錯的結論。
                "shape": shape.name(),
                // 她實際拿去比對的字。`shape` 說了走哪條路，這一欄說的是
                // 那條路上她到底找了什麼——alpha.19 之後這兩者會不一樣：
                // 「剛剛那個優惠方案」剝成「優惠方案」才找得到。
                //
                // 終端機那一份是靠人看出來的（答案就在眼前），機器讀的這份
                // 沒有那個機會。而題庫正是 Phase 2 評測語料的來源，一份寫著
                // 「這題 0 筆」卻說不出她找了什麼的紀錄，事後沒有人查得動。
                "terms": terms,
                "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
                // 撈滿上限＝被切掉了。機器讀的那一份更要講：寫腳本的人
                // 看不到終端機上的那個「+」，會直接把長度當成總數。
                "limit": limit,
                // ★ 那一半靠 `Answers::truncated`（撈了 limit+1 筆才知道），
                // 原文那一半還是靠「撈滿上限」判斷。
                "truncated": answers_truncated || hits_truncated,
                "answers": answers.iter().map(|a| serde_json::json!({
                    "kind": a.latest.kind, "value": a.latest.normalized, "raw": a.latest.raw,
                    "sightings": a.sightings, "ts": a.latest.ts,
                    "frame_id": a.latest.frame_id, "chunk_id": a.latest.chunk_id,
                    "app_id": a.latest.app_id, "window_title": a.latest.window_title,
                    "url": a.latest.url,
                })).collect::<Vec<_>>(),
                "hits": hits.iter().map(|h| serde_json::json!({
                    "chunk_id": h.chunk_id, "ts": h.ts, "source": h.source_kind.as_str(),
                    "frame_id": h.frame_id, "app_id": h.app_id, "window_title": h.window_title,
                    "url": h.url, "snippet": h.snippet, "score": h.score,
                })).collect::<Vec<_>>(),
                // 新鍵。沒認到時間範圍是 `null`，認到但切不出段落是 `[]`——
                // 兩種零不可以長得一樣。
                "time_range": asked_chapters.as_ref().map(|(r, _)| serde_json::json!({
                    "from": r.from, "to": r.to, "said": r.said,
                })),
                "chapters": asked_chapters.as_ref().map(|(_, ch)| ch.iter().map(|s| serde_json::json!({
                    // 答案講的是核心時間。start_ts／end_ts 跟 core_* 同一對數字，
                    // 不要拿顯示範圍（含 5 秒 margin）去加總。
                    "start_ts": s.core_started_at, "end_ts": s.core_ended_at,
                    "core_start_ts": s.core_started_at, "core_end_ts": s.core_ended_at,
                    "core_ms": s.core_ms(),
                    "segment_count": s.segment_count,
                    "app": s.app, "title": s.title, "host": s.host,
                })).collect::<Vec<_>>()),
                "followup": followup,
                "closure": match &close {
                    sister_core::followup::CloseIntent::NotAClosure => "not_a_closure",
                    sister_core::followup::CloseIntent::Unrecognized => "unrecognized",
                    sister_core::followup::CloseIntent::Ambiguous { .. } => "ambiguous",
                    sister_core::followup::CloseIntent::Close { .. } => "closed",
                },
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            if let Some(line) = &followup {
                println!("\n{line}");
            }
            return Ok(());
        }

        // `20 筆原文` 和「一共就這 20 筆」是兩件事，而畫面上長得一模一樣。
        // 撈滿上限就代表**被切掉了**，說出來使用者才知道還有第二頁。
        let more = |t: bool| if t { "+" } else { "" };

        // 底下這幾筆跟他打的字一個都對不上，所以要先講為什麼。少了這一句，
        // 「剛剛發生什麼事」會得到一串不相干的東西，看起來就是答非所問。
        if shape == sister_core::question::Shape::Recent {
            println!(
                "🕘 「{text}」問的是時間，不是字——沒有比對，這是最後看到的 {} 件事，{:.1} ms",
                hits.len(),
                elapsed.as_secs_f64() * 1000.0
            );
            // 空手的時候不在這裡講話。下面那個 `hits.is_empty()` 已經會印
            // 「沒有找到。」加上 `blind_lines`，這裡再印一次就是同一件事講兩
            // 遍——而它以前印的是「什麼都還沒看到——先跑 `sister record`」，
            // 和底下那幾行講的還是不同的故事。
        } else if shape == sister_core::question::Shape::Range {
            println!(
                "🕘 「{text}」問的是一段日子，不是字——沒有拿時間詞去比對，這是那段時間看到的 {} 件事，{:.1} ms",
                hits.len(),
                elapsed.as_secs_f64() * 1000.0
            );
            // 空手的時候不在這裡講話。下面那個 `hits.is_empty()` 已經會印
            // 「沒有找到。」加上 `blind_lines`，這裡再印一次就是同一件事講兩
            // 遍——而它以前印的是「什麼都還沒看到——先跑 `sister record`」，
            // 和底下那幾行講的還是不同的故事。
        } else {
            println!(
                "🔍 「{text}」 {}{} 筆答案、{}{} 筆原文，{:.1} ms{}",
                answers.len(),
                // ★ 那一半知道得比較確定：`answers` 多撈了一筆才切，所以
                // 「剛好 limit 筆」不會被誤標成「還有更多」。
                if answers_truncated { "+" } else { "" },
                hits.len(),
                more(hits_truncated),
                elapsed.as_secs_f64() * 1000.0,
                if answers_truncated || hits_truncated {
                    format!("（+ 代表撈滿 {limit} 筆就停了，用 --limit 看更多）")
                } else {
                    String::new()
                }
            );
            // **她找的字不一定是他打的字，而上面那一行印的是他打的字。**
            //
            // `terms` 會把「剛剛」「那個」剝掉，剝到不足兩個字還會往回退一格
            // ——而那一格常常退進虛字裡：「剛剛那個板」→「個板」、「剛剛看到
            // 的人」→「的人」。於是上面那一行寫著「『剛剛那個板』 0 筆答案、
            // 20 筆原文」，而那 20 筆是拿「個板」比出來的，跟板子毫無關係。
            //
            // 空手的那一半更平：兩種完全不同的處境印出同一句「沒有找到」——
            // 他打的字真的沒出現過，跟她根本沒找他打的字。前者他無能為力，
            // 後者他只要把那個詞重打一次就好。所以這一句貼著標題印，兩種
            // 結果都蓋得到。
            //
            // `--json` 的 `terms` 欄位一直都在，而當時把終端機這一半豁免掉的
            // 理由寫的是「終端機那一份是靠人看出來的（答案就在眼前）」——那
            // 句話預設了她找的字認得出來，正好是這裡不成立的前提。
            //
            // 只在**黏過**的時候講。剝掉「剛剛那個」留下「優惠方案」是剝對
            // 了，每次都報一句只會讓人學會忽略它；黏出「個板」才是她找了一
            // 個不是詞的東西。句子在 `glued_note`（那裡才驗得到）。
            for line in glued_note(text).into_iter().flatten() {
                println!("{line}");
            }
        }

        if let Some((range, chapters)) = &asked_chapters {
            println!();
            for line in chapter_lines(range, chapters) {
                println!("{line}");
            }
        }

        if !answers.is_empty() {
            println!();
            // SPEC §8.2 的語氣規範：「我最後看到的是…」，不准講成斷言。
            // 字母人那一頁從第一天就印著這一句，終端機沒有——而 ★ 那幾筆
            // 正是最需要它的：一個孤零零的 `★ +886800080123` 讀起來像一句
            // 「這就是答案」，而她知道的只有「我在某個時間點的螢幕上看過它」。
            // 他問的是「昨天那筆金額」而她給的是今天那筆的時候，差別就在
            // 這一行。同一句話在兩個介面上不一樣，這個專案已經修過三次。
            println!("我最後看到的是：");
            for a in &answers {
                let f = &a.latest;
                let seen = if a.sightings > 1 {
                    format!("（看過 {} 次）", a.sightings)
                } else {
                    String::new()
                };
                println!(
                    "  ★ {}  「{}」{seen}",
                    f.normalized,
                    fmt::one_line(&f.raw, 40)
                );
                let mut src = format!(
                    "    ↳ {} · {} ({}) · {}",
                    f.kind,
                    fmt::timestamp(f.ts),
                    fmt::relative(f.ts),
                    fmt::context_line(f.app_id.as_deref(), f.window_title.as_deref())
                );
                if let Some(fid) = f.frame_id {
                    src.push_str(&format!(" · frame #{fid}"));
                }
                println!("{src}");
            }
        }

        if hits.is_empty() {
            let has_chapters = asked_chapters
                .as_ref()
                .is_some_and(|(_, ch)| !ch.is_empty());
            if answers.is_empty() && !has_chapters {
                println!("\n沒有找到。");
                // 這句話以前是「她可能當時沒在看，或那段被排除規則擋掉了」——
                // 兩個猜測、零個證據，而兩件事她其實都查得到。
                // 比對用的是 `terms`（剝掉「剛剛」「那個」），所以掃描界線
                // 也要照 `terms` 判——查 `search` 的是它，不是原句。
                let asked = sister_core::question::terms(text);
                for line in blind_lines(&sister_core::answer::blind_spots(&db, data_dir, asked)?) {
                    println!("{line}");
                }
            }
            return Ok(());
        }
        println!();

        for (i, h) in hits.iter().enumerate() {
            println!(
                "{:>2}. {}  ({})",
                i + 1,
                fmt::timestamp(h.ts),
                fmt::relative(h.ts)
            );
            println!(
                "    {}",
                fmt::context_line(h.app_id.as_deref(), h.window_title.as_deref())
            );
            println!("    {}", fmt::one_line(&h.snippet, 120));
            // 出處：每一句話都要能被追回去
            let mut src = format!("↳ {}", h.source_kind.as_str());
            if let Some(fid) = h.frame_id {
                src.push_str(&format!(" · frame #{fid}"));
            }
            if let Some(url) = &h.url {
                src.push_str(&format!(" · {}", fmt::one_line(url, 70)));
            }
            println!("    {src}\n");
        }
        match close {
            sister_core::followup::CloseIntent::Close { .. } => {
                println!("\n這張記憶已結案，不會再提。")
            }
            sister_core::followup::CloseIntent::Unrecognized => {
                println!("\n我認不出你指哪一張記憶，所以沒有動任何一張。")
            }
            sister_core::followup::CloseIntent::Ambiguous { .. } => {
                println!("\n這句話對得上不只一張記憶，所以沒有動任何一張。")
            }
            sister_core::followup::CloseIntent::NotAClosure => {}
        }
        if let Some(line) = followup {
            println!("\n{line}");
        }
        Ok(())
    }

    /// 「她現在瞎著」和「她以前暫停過」是兩件事，而 alpha.40 為止它們共用一條
    /// if/else 鏈。
    ///
    /// 這幾條在寫出來之前是**一條都沒有**的：我把 `paused_now` 從 else 搬出來、
    /// 重寫整個 `how_long`、改掉一個不存在的指令——`cargo test --workspace` 448
    /// 條全綠，一條都沒紅。沒有紅的原因不是修法安全，是這幾個分支從來沒有人測。
    #[cfg(test)]
    mod pause_sentence_tests {
        use super::*;
        use sister_core::answer::BlindSpots;

        fn lines(b: BlindSpots) -> String {
            blind_lines(&b).join("\n")
        }

        /// 錄的時候暫停又解除過一次，後來在沒有人在錄的時候又按了一次暫停。
        ///
        /// 舊版只說得出上面那句過去式的（`paused_now` 掛在 `else if` 裡，被
        /// `paused_episodes > 0` 擋掉），而 `sister stats` 在**同一顆資料庫**上
        /// 說「但她現在是暫停的」。兩個指令、相反的結論，而他讀完 query 那句
        /// 會去按開始記錄，然後錄一整天的空白。
        #[test]
        fn a_past_pause_may_not_swallow_the_present_one() {
            let out = lines(BlindSpots {
                chunks: 10,
                ever_recorded: true,
                ever_stored: true,
                paused_episodes: 1,
                paused_ms: 300_000,
                paused_now: true,
                ..Default::default()
            });
            assert!(out.contains("暫停過 1 次"), "以前那幾段還是要講：{out}");
            assert!(
                out.contains("現在是暫停的"),
                "而現在瞎著才是要命的那一句：{out}"
            );
        }

        /// 反面：她**現在沒有**暫停的時候，不可以憑空多一句說她瞎著。
        #[test]
        fn a_past_pause_alone_says_nothing_about_now() {
            let out = lines(BlindSpots {
                chunks: 10,
                ever_recorded: true,
                ever_stored: true,
                paused_episodes: 1,
                paused_ms: 300_000,
                paused_now: false,
                ..Default::default()
            });
            assert!(out.contains("暫停過 1 次"), "{out}");
            assert!(
                !out.contains("現在是暫停的"),
                "她沒有暫停，不可以這樣說：{out}"
            );
        }

        /// 她唯一給的那條路要走得通。舊版寫的是 `sister pause --off`——
        /// `sister pause` 一個旗標都沒有，照著打會拿到 usage error（exit 2）。
        /// 他當時正瞎著，而那是整段文字裡唯一一句可以動手的。
        #[test]
        fn the_only_remedy_names_a_command_that_exists() {
            let out = lines(BlindSpots {
                chunks: 10,
                ever_recorded: true,
                ever_stored: true,
                paused_now: true,
                ..Default::default()
            });
            assert!(out.contains("sister resume"), "{out}");
            assert!(!out.contains("pause --off"), "那個旗標不存在：{out}");
        }

        /// 這個數字算短的原因有兩個，而且互相獨立。舊版把它們排成 if/else，
        /// 於是「開頭被保留期刪掉」那條在 `paused_open` 為真時永遠到不了——
        /// 一顆兩種都有的資料庫，只聽得到其中一個。
        #[test]
        fn both_reasons_the_total_is_short_get_said() {
            let out = lines(BlindSpots {
                chunks: 10,
                ever_recorded: true,
                ever_stored: true,
                paused_episodes: 3,
                paused_ms: 300_000,
                paused_open: true,
                paused_truncated: 1,
                ..Default::default()
            });
            assert!(out.contains("最後一段沒有收尾"), "{out}");
            assert!(
                out.contains("開頭已被保留期刪掉"),
                "這一條以前到不了：{out}"
            );
            assert!(out.contains("算短了"), "{out}");
        }

        /// 「一段都還沒結束」不可以印成「已結束的加起來 0 秒」——那是替一個
        /// 空集合報一個數字。
        #[test]
        fn a_total_of_zero_is_not_reported_as_a_measurement() {
            let out = lines(BlindSpots {
                chunks: 10,
                ever_recorded: true,
                ever_stored: true,
                paused_episodes: 1,
                paused_ms: 0,
                paused_open: true,
                paused_now: true,
                ..Default::default()
            });
            assert!(out.contains("長度還算不出來"), "{out}");
            assert!(!out.contains("0 秒"), "一段都沒結束，不可以報 0 秒：{out}");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};

        fn frame(ts: i64, app: &str, text: &str) -> FrameCapture {
            FrameCapture {
                ts,
                monitor: 0,
                width: 1920,
                height: 1080,
                dhash: ts as u64,
                image: None,
                image_ext: "png",
                ocr: vec![OcrBlock {
                    text: text.into(),
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 20,
                    confidence: 0.9,
                }],
                focus: FocusSnapshot {
                    app_id: Some(app.into()),
                    app_name: Some(app.into()),
                    window_title: Some("t".into()),
                    url: None,
                    pid: None,
                    password_field: false,
                },
            }
        }

        const DAY: i64 = 86_400_000;

        /// 同一支號碼出現在三個畫面，是一個答案被看見三次，不是三個答案。
        ///
        /// 三個畫面隔了三天：`sightings` 數的是**段**，同一次坐著看三百張畫面
        /// 仍然只算一次（見 `Db::SAME_SITTING_MS`）。這幾個時戳原本是
        /// 1000/2000/3000 毫秒——那是「一列＝一次」那個年代寫的，而那正是
        /// 這一版要修掉的東西。
        fn seeded() -> Db {
            let mut db = Db::open_in_memory().unwrap();
            let sid = db.start_session("test", "0").unwrap();
            for (ts, app, text) in [
                (DAY, "chrome.exe", "客服專線 0800-080-123"),
                (2 * DAY, "slack.exe", "打 0800-080-123 就好"),
                (3 * DAY, "chrome.exe", "手機 0912-345-678，帳單 NT$13,450"),
            ] {
                db.insert_frame(sid, &frame(ts, app, text), None, 0)
                    .unwrap();
            }
            db
        }

        #[test]
        fn repeated_sightings_collapse_into_one_answer() {
            let db = seeded();
            let out = answers(&db, "電話", 10).unwrap();
            assert_eq!(out.items.len(), 2, "兩支不同號碼 → 兩個答案");
            assert!(!out.truncated, "只有兩個，沒有被切掉");

            let repeated = out
                .items
                .iter()
                .find(|a| a.latest.normalized == "+886800080123")
                .unwrap();
            assert_eq!(repeated.sightings, 2);
            // 出處要指向最後一次看到的地方，那才是使用者記得的場景
            assert_eq!(repeated.latest.app_id.as_deref(), Some("slack.exe"));
        }

        /// **「看過 N 次」數的要是他看過幾次，不是最近一個窗子裡有幾次。**
        ///
        /// 舊版抓最近 `limit * 4` 列回來，在**那 40 列的窗子裡**數重複，於是：
        ///
        /// * 一年內看過 200 次的號碼，最多講得出「看過 40 次」；
        /// * 更糟的是某一頁一次吐出 40 個新號碼——他媽媽的那支（一年來每週都
        ///   看到）整個掉出窗外，★ 清單裡沒有它，然後 fallback 會說「我記得的
        ///   東西裡沒有這件事」，對一個在資料庫裡出現幾百次的值。
        ///
        /// 而畫面上那句話的用途正是「1 次和 12 次是強度不同的答案」。
        #[test]
        fn sightings_counts_every_time_he_saw_it_not_just_the_recent_window() {
            let mut db = Db::open_in_memory().unwrap();
            let sid = db.start_session("test", "0").unwrap();
            // 那支號碼一年來看過 60 次，一天一次——超過舊版 10 * 4 = 40 的窗子。
            for i in 0..60 {
                db.insert_frame(
                    sid,
                    &frame(DAY + i * DAY, "chrome.exe", "打 0800-080-123"),
                    None,
                    0,
                )
                .unwrap();
            }
            // 中間某一頁一次吐出 45 支新號碼。舊版的窗子到這裡就滿了。
            for i in 0..45 {
                let text = format!("聯絡 09{:08}", 10_000_000 + i);
                db.insert_frame(sid, &frame(100 * DAY + i, "chrome.exe", &text), None, 0)
                    .unwrap();
            }
            // 後來又看到那支號碼一次（第 61 次）。這一筆讓它排進最新的前幾名，
            // 於是它一定會出現在答案裡——舊版也會，只是次數會講成 1。
            db.insert_frame(
                sid,
                &frame(200 * DAY, "chrome.exe", "打 0800-080-123"),
                None,
                0,
            )
            .unwrap();

            let out = answers(&db, "電話", 10).unwrap();
            let old = out
                .items
                .iter()
                .find(|a| a.latest.normalized == "+886800080123")
                .expect("最新那一筆就是它，一定在答案裡");
            assert_eq!(old.sightings, 61, "看過 61 次就是 61 次，不是窗子裡的 1 次");
            assert!(out.truncated, "45 支新號碼還在底下，不能裝作沒有");
        }

        /// **看了二十分鐘沒關掉，是看過一次。**
        ///
        /// 上一條把窗子拿掉了，但數的還是**列**——而一列是一張留下來的畫面。
        /// 螢幕上只要有東西在動（旁邊的聊天室、跑著的影片、捲一下網頁），
        /// 每一拍就多一張畫面，釘在側邊欄一動也不動的那支號碼就多算一次。
        /// 於是「看過 N 次」量到的其實是**螢幕上其他地方動得多勤**，而畫面上
        /// 那句話的用途是「1 次和 12 次是強度不同的答案」——它現在會把一次
        /// 二十分鐘的閒晃講成 300 次，蓋過那支他真的每週都看到的號碼。
        #[test]
        fn a_number_he_never_looked_away_from_was_seen_once() {
            let mut db = Db::open_in_memory().unwrap();
            let sid = db.start_session("test", "0").unwrap();
            // 一次坐著二十分鐘：每 4 秒一張畫面，號碼一直在那裡沒動。
            for i in 0..300 {
                db.insert_frame(
                    sid,
                    &frame(DAY + i * 4_000, "slack.exe", "打 0800-080-123"),
                    None,
                    0,
                )
                .unwrap();
            }
            let seen = |db: &Db| answers(db, "電話", 10).unwrap().items[0].sightings;
            assert_eq!(seen(&db), 1, "他就看了一次，只是沒關掉");

            // 隔一週再遇到一次，那才是第二次。
            db.insert_frame(
                sid,
                &frame(8 * DAY, "chrome.exe", "打 0800-080-123"),
                None,
                0,
            )
            .unwrap();
            assert_eq!(seen(&db), 2);

            // 邊界：走開的時間**剛好**到門檻就算另一次，差一毫秒就還是同一次。
            // 沒有這兩條，一個「差不多就好」的比較（`>` 寫成 `>=`、或者拿
            // 平均值去分段）改下去不會有任何測試紅。
            let just_under = 8 * DAY + sister_core::db::Db::SAME_SITTING_MS - 1;
            db.insert_frame(
                sid,
                &frame(just_under, "chrome.exe", "打 0800-080-123"),
                None,
                0,
            )
            .unwrap();
            assert_eq!(seen(&db), 2, "還沒滿十分鐘，算同一次");

            let just_over = just_under + sister_core::db::Db::SAME_SITTING_MS;
            db.insert_frame(
                sid,
                &frame(just_over, "chrome.exe", "打 0800-080-123"),
                None,
                0,
            )
            .unwrap();
            assert_eq!(seen(&db), 3, "隔滿十分鐘就是另一次");
        }

        #[test]
        fn answers_are_newest_first() {
            let db = seeded();
            let out = answers(&db, "電話", 10).unwrap();
            assert!(
                out.items
                    .windows(2)
                    .all(|w| w[0].latest.ts >= w[1].latest.ts)
            );
        }

        /// 這是本專案存在的理由之一：畫面上寫「專線」，使用者問「電話」。
        /// 全文檢索接不起來，L1 型別可以。
        #[test]
        fn wording_the_screen_never_used_still_finds_the_number() {
            let db = seeded();
            assert!(
                db.search("電話", 10).unwrap().is_empty(),
                "螢幕上沒有「電話」二字"
            );
            assert!(
                !answers(&db, "電話", 10).unwrap().items.is_empty(),
                "但答得出來"
            );
        }

        /// 空清單只能出現在「算過」之後。句子不准講成沒在錄、也不准講成沒有紀錄。
        #[test]
        fn empty_chapters_do_not_claim_she_was_not_recording() {
            let range = sister_core::question::TimeRange {
                from: 1_000,
                to: 2_000,
                said: "昨天下午".into(),
            };
            let out = chapter_lines(&range, &[]).join("\n");
            assert!(out.contains("你問的是「昨天下午」"), "{out}");
            assert!(out.contains("沒有切得出來的段落"), "{out}");
            assert!(!out.contains("沒在錄"), "沒查過這件事：{out}");
            assert!(!out.contains("沒有紀錄"), "沒查過這件事：{out}");
        }

        fn a_segment(
            core_start: sister_core::Millis,
            core_end: sister_core::Millis,
            app: &str,
            title: &str,
            kinds: Vec<sister_core::segment::CutKind>,
        ) -> sister_core::segment::Segment {
            sister_core::segment::Segment {
                started_at: core_start.saturating_sub(sister_core::segment::OVERLAP_MARGIN_MS),
                ended_at: core_end.saturating_add(sister_core::segment::OVERLAP_MARGIN_MS),
                core_started_at: core_start,
                core_ended_at: core_end,
                app: Some(app.into()),
                title: Some(title.into()),
                host: None,
                cut_kinds: kinds,
                confidence: None,
                event_ids: sister_core::segment::EventRefs::default(),
                last_edit: None,
            }
        }

        #[test]
        fn a_chapter_is_the_app_and_title_not_an_interpretation() {
            let range = sister_core::question::TimeRange {
                from: 1_000,
                to: 8_000_000,
                said: "昨天下午".into(),
            };
            let acts = sister_core::activity::group(&[a_segment(
                1_000,
                3_600_000 + 1_000,
                "Code.exe",
                "db.rs — AI-Sister",
                vec![],
            )]);
            let out = chapter_lines(&range, &acts).join("\n");
            assert!(out.contains("分成 1 段"), "{out}");
            assert!(out.contains("Code.exe"), "{out}");
            assert!(out.contains("db.rs — AI-Sister"), "{out}");
            assert!(!out.contains("專心寫程式"), "標題就是 app／title：{out}");
            assert!(!out.contains("段併成"), "一段就不要講併：{out}");
        }

        /// 5 個被上限切碎的 10 分鐘段，時長是核心 45 分鐘，不是 margin 相加的
        /// 50 分鐘，也不准講成「專心了 45 分鐘」。
        #[test]
        fn grouped_chapters_use_core_duration_and_say_how_many_segments() {
            let range = sister_core::question::TimeRange {
                from: 0,
                to: 3_600_000,
                said: "昨天下午".into(),
            };
            let cap = sister_core::segment::TIME_CAP_MS;
            let mut segs = Vec::new();
            for i in 0..4 {
                let start = i * cap;
                segs.push(a_segment(
                    start,
                    start + cap,
                    "code.exe",
                    "db.rs — AI-Sister",
                    if i == 0 {
                        vec![]
                    } else {
                        vec![sister_core::segment::CutKind::TimeCap]
                    },
                ));
            }
            segs.push(a_segment(
                4 * cap,
                4 * cap + 5 * 60_000,
                "code.exe",
                "db.rs — AI-Sister",
                vec![sister_core::segment::CutKind::TimeCap],
            ));
            let acts = sister_core::activity::group(&segs);
            assert_eq!(acts.len(), 1);
            let out = chapter_lines(&range, &acts).join("\n");
            assert!(out.contains("分成 1 段"), "{out}");
            assert!(out.contains("45 分鐘"), "核心 45 分鐘：{out}");
            assert!(out.contains("5 段併成"), "要講是併來的：{out}");
            assert!(!out.contains("50 分鐘"), "不准把 margin 加進去：{out}");
            assert!(!out.contains("專心"), "沒判斷過專心：{out}");
        }

        /// 上一條的直接後果，而題庫第一次上線就記反了：`search` 是 0 筆、
        /// ★ 答案有兩筆，於是**這個產品最典型的一次成功**被記成「一筆都
        /// 沒找到」。題庫的用處全在「她答不出來的是哪些題」，混進答得出來
        /// 的之後那份清單就沒有意義了。
        ///
        /// 這一條走完整條路（磁碟上的資料庫、`run`、再讀回來），因為錯的
        /// 不是任何一個函式，是接線。
        #[test]
        fn a_question_she_answered_is_not_a_question_she_failed() {
            let dir = crate::ops::tmp::Tmp::new("qlog");
            let path = crate::db_path(&dir.0);
            {
                let mut db = Db::open(&path).unwrap();
                let sid = db.start_session("test", "0").unwrap();
                db.insert_frame(
                    sid,
                    &frame(1_000, "chrome.exe", "客服專線 0800-080-123"),
                    None,
                    0,
                )
                .unwrap();
            }

            run(&dir.0, "電話", 10, false, true).unwrap();

            let db = Db::open(&path).unwrap();
            let rows = db.query_log(10).unwrap();
            assert_eq!(rows.len(), 1, "問過的話要進題庫");
            assert_eq!(rows[0].question, "電話");
            assert!(rows[0].hits > 0, "★ 答案也是答案");
            assert_eq!(
                db.query_log_stats().unwrap().empty,
                0,
                "她答出來了，就不算答不出來"
            );
        }

        /// 反面：真的一個字都沒對上的那些題**一定要留下來**。只記答得出來
        /// 的，題庫就退化成一份「她會的題目」清單，而那份清單沒有用處。
        #[test]
        fn the_question_she_missed_is_the_one_worth_keeping() {
            let dir = crate::ops::tmp::Tmp::new("qlog-miss");
            let path = crate::db_path(&dir.0);
            {
                let mut db = Db::open(&path).unwrap();
                let sid = db.start_session("test", "0").unwrap();
                db.insert_frame(
                    sid,
                    &frame(1_000, "chrome.exe", "客服專線 0800-080-123"),
                    None,
                    0,
                )
                .unwrap();
            }

            run(&dir.0, "完全不存在的東西zzz", 10, false, true).unwrap();

            let db = Db::open(&path).unwrap();
            let stats = db.query_log_stats().unwrap();
            assert_eq!((stats.total, stats.empty), (1, 1));
        }

        #[test]
        fn different_wording_selects_a_different_kind() {
            let db = seeded();
            let money = answers(&db, "帳單多少錢", 10).unwrap();
            assert_eq!(money.items.len(), 1);
            assert_eq!(money.items[0].latest.normalized, "TWD:13450");
        }

        /// 詞彙表認不出來時回空集合，不要亂猜一堆事實塞給使用者。
        #[test]
        fn unrecognised_wording_answers_nothing() {
            let db = seeded();
            let none = answers(&db, "天氣如何", 10).unwrap();
            assert!(none.items.is_empty());
            assert!(!none.truncated, "沒有東西可以切");
        }

        /// 切到上限**要說出來**。「剛好 1 筆」和「還有第二個答案」在畫面上
        /// 長得一模一樣，而後者的意思是她其實還知道別的。
        #[test]
        fn limit_is_respected_and_the_cut_is_visible() {
            let db = seeded();
            let one = answers(&db, "電話", 1).unwrap();
            assert_eq!(one.items.len(), 1);
            assert!(one.truncated, "seeded() 有兩支號碼，切掉了一支");

            let both = answers(&db, "電話", 2).unwrap();
            assert_eq!(both.items.len(), 2);
            assert!(!both.truncated, "剛好兩個，不是被切掉");
        }

        use sister_core::answer::BlindSpots;

        /// 「她錄過但現在是空的」有四種走法，而只有一種是「被刪掉了」。
        ///
        /// 這一條守的是**句子**那一層。核心那邊（`answer.rs`）守的是欄位有沒有
        /// 查出來，可是舊版的錯不在查——四個理由都在 `BlindSpots` 裡好好躺著，
        /// 是這裡看到 `chunks == 0` 就 return，把它們丟掉了。壞掉的是組句子的
        /// 那幾行，測試就要打在那幾行上。
        #[test]
        fn nothing_was_captured_and_she_names_the_reason_that_is_actually_true() {
            let recorded_then_erased = BlindSpots {
                chunks: 0,
                frames: 0,
                ever_recorded: true,
                // **這一個 `true` 是這個 fixture 的一半。** 少了它，同一組數字
                // 講的是底下 `nothing_ever_landed` 那台機器——而「被忘掉了」是
                // 那台機器上唯一一句假話。`..Default::default()` 讓它靜靜地變成
                // false 過一次，而這條測試當場就翻紅了。
                ever_stored: true,
                ..Default::default()
            };
            let lines = blind_lines(&recorded_then_erased).join("\n");
            assert!(
                lines.contains("forget") || lines.contains("保留期"),
                "什麼都沒擋，那就真的只剩這一個理由：{lines}"
            );

            // **同樣三個數字，而 `forget` 從來沒被執行過。**
            //
            // `capture.enabled = false` 的那台機器：她開場、跑完、收工，一列
            // 內容都沒進來過。以前這裡走到的是上面那一句，一句指控——他一次
            // 都沒刪過東西，而該看的是 `capture.enabled`。
            let nothing_ever_landed = BlindSpots {
                ever_stored: false,
                ..recorded_then_erased.clone()
            };
            let lines = blind_lines(&nothing_ever_landed).join("\n");
            assert!(
                !lines.contains("forget") && !lines.contains("保留期") && !lines.contains("忘掉"),
                "他什麼都沒刪過，不可以說東西被刪了：{lines}"
            );
            assert!(
                lines.contains("capture.enabled"),
                "真正的下一步要講出來：{lines}"
            );

            // 同樣三個數字，但她**正開著**。這一次「被忘掉了或過期了」不是假的，
            // 是不完整——少的那一種正好是最常見的那一種：他三秒前才按下開始。
            // 第一次用的人問的第一個問題就落在這裡。
            let just_started = BlindSpots {
                recording_now: true,
                ..recorded_then_erased.clone()
            };
            let lines = blind_lines(&just_started).join("\n");
            assert!(
                lines.contains("剛開始"),
                "她三秒前才開始，這個可能性一定要在句子裡：{lines}"
            );
            assert!(
                lines.contains("忘掉") || lines.contains("過期"),
                "清空過的資料庫上她照樣可能正在錄，另一邊不可以被砍掉：{lines}"
            );

            // **她正開著，而且一列都沒存過。** 上面那句攤開三種可能，而其中
            // 一種在這台機器上證得出來是假的：一列都沒進來過，就沒有東西可以
            // 被忘掉。這一格是上一版自己造出來的——`recording_now` 問在
            // `!ever_stored` 前面，於是後者永遠輪不到一台正在錄的機器。
            let just_started_and_never_stored = BlindSpots {
                ever_stored: false,
                ..just_started.clone()
            };
            let lines = blind_lines(&just_started_and_never_stored).join("\n");
            assert!(
                !lines.contains("忘掉")
                    && !lines.contains("過期")
                    && !lines.contains("forget")
                    && !lines.contains("保留期"),
                "一列都沒進來過，就沒有東西被忘掉——攤開可能性不等於可以攤開不可能的：{lines}"
            );
            assert!(
                lines.contains("剛開始"),
                "她三秒前才開始，這件事還是要講：{lines}"
            );
            assert!(
                !lines.contains("capture.enabled"),
                "她正開著，不要把她送去改一個沒問題的設定：{lines}"
            );

            // **她正在起來。** `recording_now` 在這幾分鐘是 false（她一拍都還
            // 沒跑），所以上面那兩格都輪不到，而底下那兩句一個把他送去改
            // `capture.enabled`、一個說東西被忘掉了——同一秒的 `sister facts`
            // 說的是「再等一下」。兩個指令，相反的下一步。
            //
            // 一顆一年份的資料庫 `Db::open` 要跑好幾分鐘，所以這不是幾百毫秒
            // 的窄縫，是他每天早上都會問到的那一段。
            for stored in [false, true] {
                let booting = BlindSpots {
                    recording_now: false,
                    booting_now: true,
                    ever_stored: stored,
                    ..recorded_then_erased.clone()
                };
                let lines = blind_lines(&booting).join("\n");
                assert!(
                    lines.contains("正在起來"),
                    "他在等一個看得見的東西，這一句就是那個東西：{lines}"
                );
                assert!(
                    !lines.contains("capture.enabled"),
                    "沒有設定要改，他只要等：{lines}"
                );
                assert!(
                    !lines.contains("正開著") && !lines.contains("正在錄"),
                    "她還在開資料庫，一拍都還沒跑：{lines}"
                );
                assert!(
                    !lines.contains("忘掉") && !lines.contains("保留期"),
                    "他一次都沒刪過東西：{lines}"
                );
            }

            // 同樣三個數字，但那段時間她是閉著眼睛的。這一次「被忘掉了」是假話。
            let paused_throughout = BlindSpots {
                paused_episodes: 1,
                paused_ms: 3_600_000,
                ..recorded_then_erased.clone()
            };
            let lines = blind_lines(&paused_throughout).join("\n");
            assert!(
                !lines.contains("forget") && !lines.contains("保留期"),
                "他什麼都沒刪過，不可以說東西被刪了：{lines}"
            );
            assert!(lines.contains("暫停"), "真正的理由要講出來：{lines}");

            // 排除規則整段擋掉也一樣。
            let blocked_by_a_rule = BlindSpots {
                excluded: vec![("excluded app: keepassxc".into(), 2)],
                ..recorded_then_erased.clone()
            };
            let lines = blind_lines(&blocked_by_a_rule).join("\n");
            assert!(
                !lines.contains("forget") && !lines.contains("保留期"),
                "被規則擋掉不等於被刪掉：{lines}"
            );
            assert!(lines.contains("keepassxc"), "要指得出是哪一條規則：{lines}");

            // OCR 斷掉是另一回事：畫面留下來了，暫停和排除都解釋不了它。
            let ocr_is_broken = BlindSpots {
                frames: 120,
                paused_episodes: 1,
                ..recorded_then_erased.clone()
            };
            let lines = blind_lines(&ocr_is_broken).join("\n");
            assert!(lines.contains("讀字"), "要指向 OCR：{lines}");
            assert!(
                !lines.contains("暫停"),
                "暫停解釋不了「這幾張畫面上沒有字」，多講只會把人帶偏：{lines}"
            );

            // **而真的壞掉的那台機器 `chunks` 不是 0。** 上面那一組是測試自己
            // 造出來的：只寫 frame、不寫 focus。真的 recorder 每次換視窗都會
            // 把視窗標題寫進 `text_chunks`，所以 OCR 全死的機器長的是這樣——
            // 而舊版的條件掛在 `chunks == 0` 底下，這一句永遠說不出口。
            let ocr_dead_on_a_real_machine = BlindSpots {
                chunks: 3_000, // 全是視窗標題
                ocr_blocks: 0,
                frames: 40_000,
                ever_recorded: true,
                ..Default::default()
            };
            let lines = blind_lines(&ocr_dead_on_a_real_machine).join("\n");
            assert!(
                lines.contains("讀字"),
                "這台機器唯一的正確診斷，一定要說得出口：{lines}"
            );
        }

        /// 剛開始的那幾秒不可以被指控 OCR 壞了——那句話會叫他去跑一次
        /// `doctor`，而 `doctor` 會說一切正常。下一次真的壞掉的時候，他已經
        /// 學會忽略這句話了。
        #[test]
        fn a_few_blank_screens_do_not_earn_an_accusation() {
            let just_started = BlindSpots {
                chunks: 0,
                ocr_blocks: 0,
                frames: 3,
                ever_recorded: true,
                ..Default::default()
            };
            let lines = blind_lines(&just_started).join("\n");
            assert!(!lines.contains("讀字"), "三張畫面還不夠指控引擎：{lines}");
            assert!(lines.contains("剛開始"), "但要講出真正的處境：{lines}");
        }

        /// **標題那一行印的是他打的字，而她找的是別的東西。**
        ///
        /// 「剛剛那個板」剝到剩「板」不足兩個字，往回退一格黏成「個板」。於是
        /// `🔍 「剛剛那個板」 0 筆答案、20 筆原文` 讀起來像她找到了二十件關於
        /// 板子的事。空手的那一半更平：他打的字真的沒出現過，跟她根本沒找他
        /// 打的字，印出來同樣是一句「沒有找到」。
        ///
        /// 反過來也要守住：剝掉「剛剛那個」留下「優惠方案」是剝對了，那時候
        /// 多印一句只會讓他學會忽略這一句話——而下一次它真的重要。
        #[test]
        fn a_needle_glued_out_of_a_particle_says_so_next_to_the_headline() {
            let note = glued_note("剛剛那個板").expect("黏出「個板」就要出聲");
            let said = note.join("\n");
            assert!(said.contains("個板"), "要指得出她到底拿什麼去比對：{said}");
            assert!(
                said.contains("再問一次"),
                "下一步是重打一個詞，不是去設定頁翻規則：{said}"
            );

            for asked_properly in ["剛剛那個優惠方案", "客服專線", "剛剛發生什麼事"]
            {
                assert!(
                    glued_note(asked_properly).is_none(),
                    "{asked_properly} 沒黏過東西，多講一句只會讓他學會忽略它"
                );
            }
        }

        /// 「我找不到」和「我沒去找」不可以是同一句話。
        #[test]
        fn a_thirty_day_scan_does_not_get_to_say_every_single_segment() {
            let looked_everywhere = BlindSpots {
                chunks: 8421,
                ..Default::default()
            };
            let lines = blind_lines(&looked_everywhere).join("\n");
            assert!(lines.contains("每一段"), "翻完了就可以這樣講：{lines}");

            let only_thirty_days = BlindSpots {
                scan_horizon_days: Some(30),
                ..looked_everywhere
            };
            let lines = blind_lines(&only_thirty_days).join("\n");
            assert!(
                !lines.contains("每一段"),
                "只翻了三十天，「每一段」是把十二分之一講成全部：{lines}"
            );
            assert!(lines.contains("30 天"), "界線要講出來：{lines}");
        }
    }
}

pub mod facts {
    use super::*;
    use crate::fmt;

    /// `sister facts` 什麼條件都沒給、卻一列都撈不到的時候，講哪一句。
    ///
    /// **`facts == 0` 不是「這顆資料庫是空的」**，而這一句上一版把兩者當成同
    /// 一件事：直接拿它去問 [`Emptiness`]。於是一台預設設定的機器——`keepassxc`
    /// 本來就在 `excluded_apps` 裡——只要那個密碼管理員在螢幕上出現過一次，
    /// 就會讀到「她錄過，而那段時間被排除規則擋掉或暫停了」，而她那 6 段字好
    /// 好地躺在 `sister stats` 上。被擋掉的是一個密碼視窗，不是他的事實。
    ///
    /// 所以先問她到底記到了什麼，再談「為什麼是空的」。三種都是不同的下一步：
    /// 有字沒事實是抽取的問題，有畫面沒字是讀字那一段斷了，兩個都 0 才輪到
    /// `Emptiness`。這道閘門寫在函式**裡面**，理由見 `doctor::bigram_verdict`。
    fn no_facts_line(frames: i64, chunks: i64, empty: Emptiness) -> String {
        if chunks > 0 {
            return format!(
                "這份記憶裡沒有任何事實——她記了 {chunks} 段字，\
                 但那些字裡沒有她抄得下來的東西（電話、金額、日期這一類）。"
            );
        }
        if frames > 0 {
            return format!(
                "這份記憶裡沒有任何事實——她留下了 {frames} 張畫面，\
                 但一個字都沒讀出來（`sister doctor` 的「已記錄」那一列會說得更清楚）。"
            );
        }
        match empty {
            Emptiness::Erased => {
                "這份記憶裡沒有任何事實了——她錄過，那些東西被 `sister forget` \
                 忘掉了，或是過了保留期。"
            }
            Emptiness::Blocked => {
                "這份記憶裡沒有任何事實——她錄過，而那段時間被排除規則擋掉或\
                 暫停了（`sister stats` 底下的排除稽核會列出來）。"
            }
            Emptiness::Barren => {
                "這份記憶裡沒有任何事實——她錄過，但一個字都沒真的存進來過\
                 （多半是 `capture.enabled = false`，`sister doctor` 會說）。"
            }
            // 同一組數字，相反的下一步：上面那句要他去改設定，這句要他**再
            // 等一下**。她三秒前才被開起來，而上一版對她說「多半是
            // `capture.enabled = false`」。
            Emptiness::Live => {
                "這份記憶裡還沒有任何事實——她正開著，可是到現在一列內容都還沒\
                 落地（剛開始的話再等一下，一直是這樣就跑一次 `sister doctor`）。"
            }
            // 而「正在起來」連第一拍都還沒跑，所以「一直是這樣就去看 doctor」
            // 那半句在這裡是多餘的：現在還沒有任何東西可以是壞的。
            Emptiness::Booting => {
                "這份記憶裡還沒有任何事實——有一個 sister record 正在起來\
                 （多半在開資料庫），還沒開始記東西。再等一下。"
            }
            Emptiness::Fresh => "這份記憶裡還沒有任何事實——她還沒錄過。",
        }
        .to_string()
    }

    pub fn run(
        data_dir: &Path,
        kind: Option<&str>,
        search: Option<&str>,
        limit: usize,
        json: bool,
    ) -> Result<()> {
        // **打錯的類別在這裡就要死掉，不能一路帶進 SQL。** `-k monney` 進得了
        // 查詢、比對不到任何一列、然後印「沒有符合的事實。」——和「你真的沒有
        // 這一類事實」同一句話。他會相信後者。
        //
        // 而且要往下傳的是**正規化之後**的名字，不是他打的那串字。`FromStr`
        // 收 `file-path`，資料庫裡存的卻是 `file_path`——只驗不換，等於把
        // 剛擋掉的那個病換一個入口再放進來一次（這一版寫壞過一次，就是這樣）。
        //
        // 先驗再開資料庫：一個打錯的參數不該還去碰他的檔案。
        let kind = canonical_kind(kind)?;

        let db = open_existing(data_dir)?;
        // 多要一筆。「剛好 200 筆」和「撈滿 200 筆就停了」是兩件事，而只看
        // `rows.len() >= limit` 分不出來：正好 200 筆的那一次會被告知「後面
        // 還有」，然後他加大 `--limit` 再跑一次，拿到同樣的 200 筆。
        // `query` 和字母人那兩條路早就是這樣撈的，只有這裡還在猜。
        let mut rows = match search {
            Some(s) => db.facts_search(kind, s, limit + 1)?,
            None => match kind {
                Some(k) => db.facts_by_kind(k, limit + 1)?,
                None => db.facts_search(None, "", limit + 1)?,
            },
        };
        let truncated = rows.len() > limit;
        rows.truncate(limit);

        if json {
            // 這裡以前直接印一個裸陣列。裸陣列講不出「後面還有」——
            // 拿到 200 筆的腳本只能假設一共就 200 筆。所以跟 query 一樣
            // 包一層信封，把上限和有沒有被切掉講明白。
            let out = serde_json::json!({
                "limit": limit,
                "truncated": truncated,
                // 空陣列有兩種，而讀 JSON 的沒有那一句人話可以讀。和
                // `stats --json` 是同一個欄位、同一個理由。
                "ever_recorded": db.ever_recorded()?,
                "ever_stored": db.ever_stored()?,
                "facts": rows.iter().map(|f| serde_json::json!({
                    "kind": f.kind, "raw": f.raw, "normalized": f.normalized,
                    "ts": f.ts, "source": f.source_kind,
                    "frame_id": f.frame_id, "app_id": f.app_id, "url": f.url,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        if rows.is_empty() {
            // 空答案要把條件唸回來。給了兩個條件而只印「沒有符合的事實」，
            // 他得自己拆一次才知道是哪一邊空的——而多數時候他會拆錯邊，
            // 以為「這台機器沒抓到電話」，其實是那個關鍵字沒出現過。
            println!(
                "{}",
                match (kind, search) {
                    (Some(k), Some(s)) =>
                        format!("這份記憶裡沒有 {k} 這一類、而且含有「{s}」的事實。"),
                    (Some(k), None) => format!("這份記憶裡沒有 {k} 這一類的事實。"),
                    (None, Some(s)) => format!("這份記憶裡沒有含有「{s}」的事實。"),
                    // 上面三句都自己帶著條件，所以它們講的必然是「這份記憶
                    // 裡」。這一句沒有條件可以帶，於是它得去講原因——而原因
                    // 有四種，見 `no_facts_line`。`facts` 這張表 `forget` 和
                    // 保留期都會清（`retention.rs` 兩邊都有 `DELETE FROM
                    // facts`），而 `facts.chunk_id` 還是 ON DELETE CASCADE，
                    // 所以「剛把一整天忘掉」也走到這裡。
                    (None, None) => {
                        let s = db.stats()?;
                        // 「一列都沒進來過」還要再分一次：她**現在**在不在。
                        // 這一頁手上有 `data_dir`，所以這一題問得出來——而且
                        // 問的是 `phase` 不是 `is_occupied`，「正在起來」和
                        // 「正在錄」是兩句不同的話。
                        let beat =
                            sister_core::heartbeat::presence(data_dir, sister_core::now_ms());
                        no_facts_line(s.frames, s.chunks, Emptiness::of(&db, &s, beat)?)
                    }
                }
            );
            return Ok(());
        }
        // 撈滿上限就是被切掉了。`{n} 筆事實` 和「一共就這 n 筆」在畫面上
        // 長得一模一樣，而使用者會拿後者去下結論。
        let more = if truncated {
            format!("（撈滿 {limit} 筆就停了，用 --limit 看更多）")
        } else {
            String::new()
        };
        println!("{} 筆事實{more}\n", rows.len());
        for f in &rows {
            println!(
                "{:<10} {:<24} 「{}」",
                f.kind,
                fmt::one_line(&f.normalized, 24),
                fmt::one_line(&f.raw, 40)
            );
            println!(
                "           {}  {}",
                fmt::timestamp(f.ts),
                fmt::context_line(f.app_id.as_deref(), f.window_title.as_deref())
            );
        }
        Ok(())
    }

    /// 他打的字 → 資料庫裡真正存的那個名字。
    ///
    /// 兩件事一起做是有理由的：**只驗不換**，等於把剛擋掉的那個病換一個入口
    /// 再放進來一次。`FromStr` 收 `file-path`（因為 `file_path` 打成連字號
    /// 而被拒絕只會讓人以為功能壞了），但資料庫那一欄存的是 `file_path`——
    /// 驗過了卻把原字串丟進 SQL，結果又是零筆加一句「你沒有這一類事實」。
    /// 這一版真的寫壞過一次，所以它現在是一個有名字、驗得到的函式。
    fn canonical_kind(kind: Option<&str>) -> Result<Option<&'static str>> {
        kind.map(|k| {
            k.parse::<sister_core::facts::FactKind>()
                .map(|k| k.as_str())
                .map_err(|e| anyhow::anyhow!(e))
        })
        .transpose()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn what_he_types_becomes_what_the_database_stores() {
            for typed in ["file-path", "file_path", "FILE_PATH", " file-path "] {
                assert_eq!(
                    canonical_kind(Some(typed)).expect("這幾個都該過"),
                    Some("file_path"),
                    "{typed} 沒有被換成資料庫裡的名字——查出來會是零筆"
                );
            }
        }

        #[test]
        fn a_typo_does_not_reach_the_database() {
            let e = canonical_kind(Some("monney")).expect_err("打錯的要在這裡就死掉");
            assert!(e.to_string().contains("money"), "要附上清單：{e}");
        }

        #[test]
        fn no_filter_stays_no_filter() {
            assert_eq!(canonical_kind(None).expect("沒給就是沒給"), None);
        }

        /// **她記了一整天，只是那些字裡沒有電話號碼。**
        ///
        /// 上一版拿 `facts == 0` 直接去問 `Emptiness`，而預設設定裡就有
        /// `keepassxc`。所以一台開過密碼管理員的機器讀到：
        ///
        /// ```text
        ///   $ sister facts
        ///   這份記憶裡沒有任何事實——她錄過，而那段時間被排除規則擋掉或暫停了。
        ///   $ sister stats
        ///   文字      6 段（4 個 OCR 區塊）
        /// ```
        ///
        /// 被擋掉的是一個密碼視窗，不是他的事實。`Emptiness` 只回答「為什麼是
        /// 空的」，而這一句手上那個 0 根本不是「空的」那個 0。
        #[test]
        fn text_she_kept_is_not_a_recording_the_rules_ate() {
            let said = no_facts_line(2, 6, Emptiness::Blocked);
            assert!(said.contains("6 段"), "她記了多少字要講出來：{said}");
            for lie in ["擋掉", "暫停", "忘掉", "保留期", "還沒錄過"] {
                assert!(
                    !said.contains(lie),
                    "那 6 段字好好地在資料庫裡，不可以說被「{lie}」：{said}"
                );
            }
            // 三種 `Emptiness` 一個字都不准動——她這裡不空，就沒有「為什麼空」
            // 這個問題。
            for e in Emptiness::ALL {
                assert_eq!(no_facts_line(2, 6, e), said);
            }

            // 有畫面、沒有字：讀字那一段斷了，而那也不是「還沒錄過」。
            let ocr_dead = no_facts_line(9, 0, Emptiness::Fresh);
            assert!(ocr_dead.contains("9 張"), "{ocr_dead}");
            assert!(!ocr_dead.contains("還沒錄過"), "她錄了 9 張：{ocr_dead}");

            // 兩個都 0，才輪到「為什麼」——而三種各自一句。
            let said = |e| no_facts_line(0, 0, e);
            // **走 `Emptiness::ALL`，不要寫陣列字面量。** 字面量不會因為 enum
            // 多一種變體而編不過，而上一版 `Barren` 加進來的時候，這裡還停在
            // 三種——守著「不可以兩種處境共用一句話」的測試，剛好漏掉的就是那
            // 次新加的那一種。
            let all = Emptiness::ALL.map(said);
            for (i, a) in all.iter().enumerate() {
                for (j, b) in all.iter().enumerate().skip(i + 1) {
                    assert_ne!(
                        a,
                        b,
                        "{:?} 和 {:?} 的下一步不一樣，不可以共用一句話",
                        Emptiness::ALL[i],
                        Emptiness::ALL[j]
                    );
                }
            }
            assert!(all[0].contains("還沒錄過"), "{}", all[0]);
            assert!(all[2].contains("forget"), "{}", all[2]);
            // `Live`（4）和 `Booting`（5）是相反的兩件事，而它們的下一步都不是
            // 「去改設定」——`Barren`（3）才是。開機那一段裡她連第一拍都還沒
            // 跑，所以連「一直是這樣就跑 doctor」都還太早：他只要再等一下。
            assert!(all[4].contains("她正開著"), "{}", all[4]);
            assert!(
                all[5].contains("正在起來") && !all[5].contains("capture.enabled"),
                "{}",
                all[5]
            );
        }
    }
}

pub mod stats {
    use super::*;
    use crate::fmt;
    use sister_core::config::Config;

    /// 「工作階段」整行——**數字和那個但書綁在一起**。
    ///
    /// 見 [`sister_core::db::DbStats::only_session_shells_left`]：清空之後還站
    /// 著的那幾列全是空殼，而這一頁上其他每一個數字都是 0。少了但書，這個 1
    /// 讀起來就像「她還記得那一場」，而正上方那個 ⚠ 才剛說完一列都不剩。
    ///
    /// `beat` 來自 `heartbeat::presence`——這一頁手上有 `data_dir`，所以它分得出
    /// 那一列是當掉的還是活的，不必印一個「或」。**收 `Presence` 不收布林**：上
    /// 一版這一頁自己壓成 `beat.is_some()`，於是同一份 `stats` 上面那個 ⚠ 說
    /// 「有一個 sister record 正在起來」、三行之下這一列說那一場開著是因為
    /// 「她正在錄，或正在開機」——而且同一秒的 `doctor` 說那一列是當機的殼。
    /// 再上一版收 `Option<Phase>`，`Thinking` 掉進 `None`，同一頁說她當掉了。
    fn sessions_line(
        s: &sister_core::db::DbStats,
        beat: sister_core::heartbeat::Presence,
    ) -> String {
        if s.only_session_shells_left() {
            // 只取「為什麼」。「什麼時候會走」留給 `forget`：那裡是他剛按下不
            // 可逆的按鈕、正在等一句交代的時刻，而這裡是一份足跡清單。
            let (why, _) = session_shell_why(beat);
            format!("  工作階段  {}（空殼：{}）", s.sessions, why)
        } else {
            format!("  工作階段  {}", s.sessions)
        }
    }

    /// 「事件」整行——理由和 [`sessions_line`] 一模一樣，只是換一張表。
    ///
    /// `system_events` 裡有兩種列講的是**那場錄製本身**（開始／結束），不是她
    /// 記下來的東西。一顆剛被清空的資料庫上，她一開始錄，「系統」那個位置就會
    /// 冒出一個 1——而正上方那個 ⚠ 才剛說完「一列都不剩」。兩句都是真的，湊在
    /// 同一頁上就是這個專案一路在修的那種謊。
    ///
    /// 條件是「剩下的每一列都是標籤」，不是「有標籤」：她真的記了東西的時候
    /// 這一行照樣是乾淨的一個數字，因為那時候那個但書是假的。
    fn events_line(s: &sister_core::db::DbStats) -> String {
        let head = format!(
            "  事件      焦點 {} · 剪貼簿 {} · 輸入 {} · 系統 {}",
            s.focus_events, s.clipboard_events, s.input_windows, s.system_events
        );
        if s.system_events > 0 && s.system_events == s.session_marks {
            format!("{head}（都是那幾場錄製自己的開始／結束）")
        } else {
            head
        }
    }

    /// 要 `Config` 是為了底下那一行「遮蔽」。
    ///
    /// `redaction_audit` 數的是「插了旗子的有幾列」，而**旗子只有在
    /// `privacy.redact_clipboard_secrets` 開著的時候才會插**。關掉之後那個
    /// 計數永遠是 0，於是這一頁會印出和一份乾淨資料庫一模一樣的句子——
    /// 而真相正好相反：那把 API key 就原封不動躺在 `clipboard_events` 裡。
    /// 這是整個產品裡讀反了代價最高的一句話，不能只靠資料庫猜。
    pub fn run(data_dir: &Path, config: &Config, json: bool) -> Result<()> {
        let db = open_existing(data_dir)?;
        let s = db.stats()?;
        // 一頁上三個地方要問「她現在在不在」（底下的 ⚠、`sessions_line`、
        // 還有 `signal_audit` 的「這一場／上一場」），而它們必須是同一個答案
        // ——分幾次問的話，中間她可以剛好收工。
        //
        // 讀在 `json` 那個分支**之前**，因為那份 JSON 也在回答同一題：它印
        // 的 `scope_started_at` 以前沒有任何欄位說得出那一場是不是還在錄。
        //
        // **底下每一個問它的人拿到的都是 [`Phase`] 本人。** 上一版這裡多一行
        // `let occupied = beat.is_some();`，然後把那個布林發給 `sessions_line`
        // ——於是開機那幾分鐘，這一頁上面那個 ⚠ 說「有一個 sister record 正在
        // 起來」，三行之下那一列說那一場開著是因為「此刻有人佔著這個資料目錄
        // （她正在錄，或正在開機）」，而同一秒的 `doctor` 說那一列是上一次當機
        // 留下來的殼。同一份修改，同一個檔案，同一頁——`doctor` 那邊改了、這邊
        // 沒有。**修完一個「布林湊不出三種答案」，要 grep 這一頁上還有誰收那個
        // 布林。**
        //
        // [`Phase`]: sister_core::heartbeat::Phase
        let beat = sister_core::heartbeat::presence(data_dir, sister_core::now_ms());

        let days_between = |a: Option<i64>, b: Option<i64>| match (a, b) {
            (Some(a), Some(b)) if b > a => (b - a) as f64 / 86_400_000.0,
            _ => 0.0,
        };
        let span_days = days_between(s.first_ts, s.last_ts);
        // 畫面自己的跨度。**不能用 `span_days`**：那一對時間來自
        // `text_chunks`（`retention.text_days` 管，預設 365 天），而
        // `image_bytes` 只涵蓋還留著畫面檔的那幾列（`frames_days`，預設
        // 30 天）。用滿一年之後兩者差 12 倍，而分子小分母大，印出來的
        // 「每天約」會小成十二分之一——那正好是 Phase 0 退出條件的判決，
        // 錯的方向是「看起來過了」。見 `DbStats::image_first_ts`。
        let image_days = days_between(s.image_first_ts, s.image_last_ts);
        let disk_total = s.db_bytes + s.image_bytes;
        // 不到半天就不外推。`None` = 「還答不出來」，不是 0，也不是「就是這麼多」。
        //
        // 舊版在不到半天的時候把**累計總量**塞進這個欄位，然後照樣蓋一個 ✓
        // 上去：錄兩小時長了 200MB，它會印「每天約 200.0 MB ✓」，而真實速率
        // 是 2.4 GB/天——超標八倍卻長得像通過。隔壁的 `footprint.rs` 早就
        // 為同一件事寫了規則（不到 60 秒不外推）並且有測試守著，是這裡沒跟上。
        //
        // 兩半各除以**自己的**跨度再相加，而不是把總量除以文字的跨度。
        // 兩個保留期各管一半，所以那兩個分母本來就不是同一個數字。
        //
        // 畫面那一半不到半天也一樣不外推，理由同上。`None` 的意思是「還答不
        // 出來」，於是底下整句話就不會蓋一個 ✓ 上去。
        // 「這台機器不留截圖」和「這台機器會留，但這份資料庫裡現在一張都沒有」
        // 是兩件事，而兩邊的 `image_bytes` 都是 0。
        //
        // 前者的每天成本真的是 0，蓋 ✓ 是對的。後者的是**還不知道**——而
        // 走到後者最常見的路徑，正是一份用超過 `frames_days`（預設 30 天）
        // 又停了一陣子沒錄的資料庫：圖被保留期清光，分子歸零，分母還是一年。
        // 這一頁於是拿一個只含文字的速率去蓋 Phase 0 那個 300MB/天的 ✓，
        // 而那個數字小是因為分子被刪掉了，不是因為她真的只用這麼多。
        //
        // 分得出來，因為 stats 手上有 config 也有 data dir。同意書是上限、
        // 設定是開關，兩個都要問（`keeps_images` 就是這兩件事的 and）。
        let keeps_images = sister_core::consent::load(data_dir).keeps_images(config);
        let img_rate = match s.image_bytes {
            0 if !keeps_images => Some(0.0),
            0 => None,
            _ if image_days >= 0.5 => Some(s.image_bytes as f64 / image_days),
            _ => None,
        };
        let per_day = match (span_days >= 0.5, img_rate) {
            (true, Some(i)) => Some(s.db_bytes as f64 / span_days + i),
            _ => None,
        };
        let audit = db.exclusion_audit()?;
        let redaction = db.redaction_audit()?;
        let pauses = db.pause_audit()?;

        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "frames": s.frames, "frames_collapsed": s.frames_collapsed,
                    "frames_with_image": s.frames_with_image,
                    "ocr_blocks": s.ocr_blocks, "chunks": s.chunks, "facts": s.facts,
                    "focus_events": s.focus_events, "clipboard_events": s.clipboard_events,
                    "input_windows": s.input_windows, "system_events": s.system_events,
                    "sessions": s.sessions,
                    // 上面每一個計數器都可能是 0，而 0 有三種（見 `Emptiness`）：
                    // 從來沒錄過、錄過但那段時間全被規則擋掉、錄過而被 `forget`
                    // 和保留期帶走了。人看的那一頁底下有一行 ⚠ 在講這件事；讀
                    // JSON 的（`check-audit.py`、未來的儀表板）沒有那一行可以
                    // 讀，只能自己從一堆 0 猜，而那正是猜不出來的東西。
                    //
                    // 這一欄只砍掉第三種。第二種要靠底下的 `exclusions` /
                    // `pauses` 自己判——它們本來就在同一份 JSON 裡。
                    "ever_recorded": db.ever_recorded()?,
                    "ever_stored": db.ever_stored()?,
                    "db_bytes": s.db_bytes,
                    "image_bytes": s.image_bytes, "span_days": span_days,
                    // 畫面自己的跨度。和 `span_days` 不一樣是正常的——
                    // 兩個保留期各管一半，見 `DbStats::image_first_ts`。
                    "image_span_days": image_days,
                    "bytes_per_day": per_day,  // null = 資料還不夠久，不外推
                    "redaction": {
                        "flagged": redaction.flagged,
                        // > 0 就是遮蔽沒生效——那不是統計數字，是故障
                        "leaked": redaction.leaked,
                    },
                    "exclusions": audit.iter().map(|a| serde_json::json!({
                        "reason": a.reason, "episodes": a.episodes,
                        "first_ts": a.first_ts, "last_ts": a.last_ts,
                    })).collect::<Vec<_>>(),
                    "pauses": {
                        "episodes": pauses.episodes,
                        // 只含已結束的那幾段；`truncated > 0` 時這是下限
                        "total_ms": pauses.total_ms,
                        "open_since": pauses.open_since,
                        "truncated": pauses.truncated,
                    },
                    // 這三個訊號在 Phase 0 沒有讀者，所以也沒有回歸保護：
                    // 哪天 recorder 不再寫 focus_events，`stats` 照樣印一個
                    // 很小的數字，沒有一個測試會紅。`doctor` 會講，但 doctor
                    // 要有人去跑——一個要有人記得去跑的檢查，遲早沒人跑。
                    "signals": db.signal_audit(beat)?.iter().map(|a| serde_json::json!({
                        "name": a.name, "rows": a.rows,
                        "populated": a.populated,
                        // 三種下場，不是一個布林：`too_early` 以前和 `alive`
                        // 一樣是 `broken: false`，於是「驗過了」和「還看不出
                        // 來」在機器讀的那一份裡也是同一個值。
                        "verdict": a.verdict.as_str(),
                        // 數字描述的是最後一場，不是全表。少了這一欄，讀
                        // 這份 JSON 的人（包括未來的我）會把 `rows` 當成
                        // 這顆資料庫的總量，而那正是舊版真正在數的東西。
                        "scope_started_at": a.scope_started_at,
                        // 而那一場**可能還在錄**：這幾個數字是一份還在長的東西
                        // 的中途快照。人看的那一頁靠「這一場／上一場」講這件事
                        // （`signal_line`）；讀 JSON 的以前只能猜，而 doctor 的
                        // 分母正好把那一場扣掉了——同一份報告裡兩個相鄰的數字
                        // 描述兩個不相交的集合，就是這批 bug 的形狀。
                        "scope_is_live": a.scope_is_live,
                    })).collect::<Vec<_>>(),
                }))?
            );
            return Ok(());
        }

        println!("📊 AI-Sister 足跡\n");
        // **整頁都是 0 的時候，要先講這是哪一種 0。**
        //
        // 一顆剛裝好的資料庫和一顆剛被 `forget --last 24h` 清空的資料庫，這一頁
        // 上逐字相同：畫面 0、文字 0、事實 0，然後底下三行說「這份紀錄裡沒有任
        // 何一段擷取被隱私規則擋下來 / 沒有暫停過 / 沒有任何剪貼簿內容被判定為
        // 疑似秘密」。每一句都是真的——講的是現在這顆資料庫——而湊起來那一頁在
        // 說「這台機器上什麼都沒發生過」，對一個五分鐘前才親手刪掉一整天的人。
        //
        // 那一天他的排除規則**擋過兩段**。刪掉的是證據，不是歷史。
        //
        // 問 `nothing_recorded_left()` 而不是 `frames == 0 && chunks == 0`：後者
        // 是這句話的第一版，而它自己就是同一個 bug。一場全程被排除規則擋掉的錄
        // 製也是畫面 0、文字 0，可是那一頁上有「工作階段 1」「輸入 6」「系統 7」
        // 和一整段排除稽核——這一行於是印在一頁有數字的東西上面，還把「規則擋
        // 掉的」說成「你刪掉的」。兩句都是假的，而它們解釋的是**完全相反**的下
        // 一步：一個要他去改 config，一個要他知道東西真的沒了。
        //
        // 而這句話講的是**她記下來的東西**，不是「這個檔案是空的」。中間那一版
        // 寫成後者，於是得把他打進搜尋框的字也算成「一列」——`forget` 完接著問
        // 一句「真的沒了嗎」就有一列，這個 ⚠ 就整段消失了。話講準，那張表就不
        // 必進那個述詞（見 `DbStats::nothing_recorded_left`）。
        //
        // 而「一列都不剩」還要再分一次：**東西被拿走了**，和**東西從來沒進來
        // 過**。走 `Emptiness` 而不是自己問 `ever_recorded`——這一行以前自己
        // 問，於是一台 `capture.enabled = false` 的機器（她跑完、一個字都沒
        // 記到、`forget` 從來沒被執行過）被這個 ⚠ 告知東西被忘掉了。
        match Emptiness::of(&db, &s, beat)? {
            Emptiness::Erased => println!(
                "  ⚠  她**錄過**，而她記下來的東西現在一列都不剩——被 \
                 `sister forget` 忘掉了，或是過了保留期。\n     \
                 底下那些 0、還有「沒有發生過」那幾句，講的是現在這顆資料庫，\
                 不是那幾天。\n"
            ),
            // 這句話**不去提**刪除，連否認都不提。「沒有東西被忘掉」讀起來
            // 是在回答一個他還沒問的問題，而那個問題本來就是上一版自己嚇出
            // 來的。而且那個「忘掉」讓 CI 那道「這一頁不准出現任何一句指控」
            // 分不出否認和指控——一句需要例外的斷言，守不住東西。
            Emptiness::Barren => println!(
                "  ⚠  她**錄過**，可是一列內容都沒有存進來過。\n     \
                 先看 `capture.enabled`（`sister doctor` 會直接說），\
                 底下那些 0 講的是現在這顆資料庫。\n"
            ),
            // 這一句不提設定，因為她此刻正開著——上面那句會把一個剛按下開始
            // 記錄的人，送去改一個沒問題的設定。
            Emptiness::Live => println!(
                "  ⚠  她**正開著**，可是到現在一列內容都還沒落地。\n     \
                 剛開始的話再等一下；一直是這樣就跑一次 `sister doctor`。\
                 底下那些 0 講的是現在這顆資料庫。\n"
            ),
            // 上面那一句說「她正開著」，這一句不能跟著說——`Db::open` 在一顆
            // 存了一年的資料庫上要跑好幾分鐘，那幾分鐘裡她一拍都還沒跑。
            // 「一直是這樣就跑 doctor」也拿掉：現在沒有任何東西可以是壞的。
            Emptiness::Booting => println!(
                "  ⚠  有一個 sister record **正在起來**（多半在開資料庫），\
                 還沒開始記東西。\n     \
                 底下那些 0 講的是現在這顆資料庫，不含它等一下要記的東西。\n"
            ),
            Emptiness::Blocked | Emptiness::Fresh => {}
        }
        if let (Some(a), Some(b)) = (s.first_ts, s.last_ts) {
            println!(
                "  期間      {} → {}  （{:.1} 天）",
                fmt::timestamp(a),
                fmt::timestamp(b),
                span_days
            );
        }
        // 整句在 `sessions_line`——數字和那個但書要嘛一起印，要嘛一起不印。
        // 拆成「印數字」加「如果……再印一句」的話，那個 `if` 就落在呼叫端，而
        // 這一批 bug 七次有七次犯在呼叫端。
        println!("{}", sessions_line(&s, beat));
        println!(
            "  畫面      {} 張保留，{} 張因重複被折疊",
            s.frames, s.frames_collapsed
        );
        if s.frames + s.frames_collapsed > 0 {
            let ratio = s.frames_collapsed as f64 / (s.frames + s.frames_collapsed) as f64;
            println!("            去重擋掉了 {:.0}% 的畫面", ratio * 100.0);
        }
        // 「4 張保留」配上底下的「畫面檔 0 B」看起來像壞了，其實多半不是。
        // 全部都有圖的時候不講：那是預期，多一行只是雜訊。
        //
        // 但這裡**不講是為什麼**。`frames_with_image == 0` 底下躺著三件事：
        // 第三張同意書沒簽（只記字）、圖過了 `frames_days` 被清掉了（預設 30
        // 天，用超過一個月的人都會走到）、以及寫圖真的失敗。以前這裡寫的是
        // 「一張圖都沒留（只記了上面的字）」——那是第一種的說法，而它在第二種
        // 情況下是一句關於歷史的假話：那些圖存在過。
        //
        // stats 手上只有資料庫，沒有 config，三者分不出來。分得出來的是
        // `sister doctor`（它讀得到同意書和設定），所以把人指過去。
        if s.frames > 0 && s.frames_with_image < s.frames {
            let how = if s.frames_with_image == 0 {
                "現在一張圖都沒有，只剩上面的字（為什麼：`sister doctor`）".to_string()
            } else {
                format!(
                    "其中 {} 張現在還有圖，其餘只剩上面的字",
                    s.frames_with_image
                )
            };
            println!("            {how}");
        }
        println!(
            "  文字      {} 段（{} 個 OCR 區塊）",
            s.chunks, s.ocr_blocks
        );
        println!("  事實      {}", s.facts);
        println!("{}", events_line(&s));
        println!();

        // 排除稽核。這一段的重點不是「有幾條規則」——那是設定檔，doctor 會念。
        // 這裡回答的是「它們到底生效過沒有」，而錄製結束之後只有資料庫答得出來。
        //
        // 數的是「段」：踏進 keepassxc 待十分鐘算一段。被擋掉的畫面**張數**
        // 沒有存進資料庫，所以這裡不講張數——講了就是把一個 2 說成 8000。
        // 底下這三段的**零狀態**都講成了「從來沒有發生過」，而這份資料庫只
        // 記得保留期以內的事。稽核紀錄跟著 `retention.text_days` 走（見
        // DATA_INVENTORY），所以設了 30 天的人看到的「從來沒有被暫停過」，
        // 真正的意思是「最近 30 天沒有」——他去年整個月都閉著眼睛也一樣。
        //
        // 改法和她那句空答案同一條：講**紀錄**，不講世界。
        if audit.is_empty() {
            println!("  排除      這份紀錄裡沒有任何一段擷取被隱私規則擋下來");
            println!("            （規則有沒有寫對是另一回事，跑 `sister doctor` 當場驗）");
        } else {
            let total: i64 = audit.iter().map(|a| a.episodes).sum();
            println!("  排除      隱私規則生效過 {total} 段（不是張數，張數沒存）");
            for a in &audit {
                let when = if a.first_ts == a.last_ts {
                    fmt::timestamp(a.first_ts)
                } else {
                    format!(
                        "{} → {}",
                        fmt::timestamp(a.first_ts),
                        fmt::timestamp(a.last_ts)
                    )
                };
                println!(
                    "            {:>4} 段  {}",
                    a.episodes,
                    fmt::one_line(&a.reason, 46)
                );
                println!("                    {when}");
            }
        }

        // 暫停稽核。和排除同一個道理，只是這次擋下擷取的是使用者自己。
        // 「我那天到底有沒有記得按暫停」是他事後唯一問得出口的問題，而錄製
        // 結束之後只有資料庫答得出來。
        if pauses.episodes == 0 {
            println!("  暫停      這份紀錄裡沒有暫停過");
            // 但「紀錄裡沒有」不等於「現在沒有」。他在沒有 recorder 在跑的
            // 時候按下暫停，沒有任何一筆事件記得這件事——而下一次 `sister
            // record` 會開起來然後什麼都不記，他手上只有這一行可以看。
            if sister_core::pause::is_paused(data_dir) {
                println!(
                    "            但她**現在是暫停的**（按下去的時候沒有人在錄，所以紀錄裡看不到）"
                );
                println!(
                    "            `sister resume` 解除。不解除的話，接下來錄的每一分鐘都是空的。"
                );
            }
        } else {
            println!(
                "  暫停      {} 段，已結束的加起來 {}",
                pauses.episodes,
                crate::fmt::duration_ms(pauses.total_ms)
            );
            // 「最後一筆 CapturePaused 沒有配到 CaptureResumed」和「她現在
            // 是暫停的」是**兩件事**，而舊版把前者印成了後者。
            //
            // 那兩筆事件是 recorder 寫的，暫停旗標是桌面程式（或 `sister
            // pause`）寫的，兩者不同行程。錄製當中按暫停、然後關掉 record、
            // 再解除——解除的時候沒有人在跑 recorder，`CaptureResumed` 就沒
            // 有人寫。資料庫從此永遠掛著一段沒收尾的暫停，而這一行會永遠
            // 印「她現在就是閉著眼睛的」，即使她正在錄。
            //
            // 現在就是不是暫停，`paused.flag` 當場答得出來（見 `pause.rs`：
            // 讀不到一律當成暫停）。所以先問旗標，再決定那段沒收尾的紀錄
            // 該怎麼講。
            if let Some(since) = pauses.open_since {
                if sister_core::pause::is_paused(data_dir) {
                    println!(
                        "            最後一段還沒結束（{} 起）——她現在就是閉著眼睛的",
                        fmt::timestamp(since)
                    );
                } else {
                    println!(
                        "            最後一段沒有收尾（{} 起，沒有對應的解除紀錄）",
                        fmt::timestamp(since)
                    );
                    println!(
                        "            但她現在**沒有**暫停——多半是解除的時候 `sister record` 沒在跑，"
                    );
                    println!("            所以沒有人把那一筆寫下來。上面那段時間因此算短了。");
                }
            } else if sister_core::pause::is_paused(data_dir) {
                // 反過來也會發生，而這個方向更值得講：他按了暫停，但那一刻
                // 沒有 recorder 在跑，所以紀錄裡看不到。下一次 `sister record`
                // 會開著眼睛啟動然後什麼都不記，而他不知道為什麼。
                println!(
                    "            但她現在是暫停的（紀錄裡看不到，因為按下去的時候沒有人在錄）"
                );
            }
            // 這一行讓上面那個時間是「下限」這件事看得見，而不是靜靜地少算。
            if pauses.truncated > 0 {
                println!(
                    "            其中 {} 段的開頭已被保留期刪掉，長度算不出來",
                    pauses.truncated
                );
            }
        }

        // 秘密遮蔽。問的不是「旗子插了幾次」，是「插了旗子的那幾列，字還在不在」。
        // 前者是我們寫入時的自我宣稱，後者是資料庫此刻的實際狀態。
        //
        // 這兩個問題以前串成一條 if / else if / else，於是設定一關，
        // `leaked` 那句就永遠印不出來。可是 `leaked > 0` 講的是**已經躺在
        // 資料庫裡的那幾列**：旗子插了、字卻沒被清掉。關掉一個只影響「以後」
        // 的開關，不會讓那把 API key 消失，只會讓整個產品裡唯一會喊的那個人
        // 閉嘴——而且是在他剛剛做了一個放寬隱私的動作之後閉嘴。
        //
        // 所以先喊故障，再講設定。`--json` 本來就是無條件印 `leaked` 的，
        // 這一頁只是沒跟上。
        if redaction.leaked > 0 {
            println!(
                "  遮蔽      ⚠ {} 次剪貼簿內容被判定為疑似秘密，而內容**仍然留在資料庫裡**",
                redaction.leaked
            );
            println!("            遮蔽沒生效。`sister forget` 挖得掉，但得先知道它在那裡。");
            if !config.privacy.redact_clipboard_secrets {
                println!(
                    "            （而且 redact_clipboard_secrets 現在是關的：以後複製的東西連旗子都不會插）"
                );
            }
        } else if !config.privacy.redact_clipboard_secrets {
            println!("  遮蔽      **關掉了**（privacy.redact_clipboard_secrets = false）——");
            println!("            沒有人在看剪貼簿裡有沒有 API key，所以這一欄數不出東西來。");
            println!("            數不到不等於沒有：複製過的東西原樣進了資料庫。");
            // 開過又關掉的那份資料庫，和從來沒開過的那份，這一行不一樣。
            if redaction.flagged > 0 {
                println!(
                    "            （以前開著的時候擋下過 {} 次，那幾列的 text 現在是空的）",
                    redaction.flagged
                );
            }
        } else if redaction.flagged == 0 {
            println!("  遮蔽      這份紀錄裡沒有任何剪貼簿內容被判定為疑似秘密");
        } else {
            // `leaked > 0` 已經在最上面喊過了，走到這裡的一定是零。
            println!(
                "  遮蔽      {} 次剪貼簿內容被判定為疑似秘密",
                redaction.flagged
            );
            println!("            這幾列的 text 現在都是空的（當場查的，不是相信旗子）");
        }
        println!();
        println!(
            "  資料庫    {}\n  畫面檔    {}",
            fmt::bytes(s.db_bytes),
            fmt::bytes(s.image_bytes)
        );
        // 「畫面檔」那個數字是資料庫加出來的（`SUM(image_bytes)`），不是去
        // 量硬碟。保留期清圖的時候會把那一欄一起歸零，所以平常兩者對得上。
        //
        // 對不上的那一種很具體：`sister export`（沒加 `--with-frames`）之後，
        // 對著那個匯出目錄跑 stats。資料庫整份帶走了、圖一張都沒帶，於是這
        // 一行說「畫面檔 1.2 GB」，而那個資料夾裡是空的——正好是他打開這一頁
        // 想確認的那件事。
        //
        // 不逐檔去驗（那是十萬次 stat，而 stats 要快）。只看 `frames/` 這個
        // 根目錄在不在、空不空：一次 `read_dir`，答得出這個唯一夠糟的情況。
        if s.image_bytes > 0 {
            let root = Config::frames_dir(data_dir);
            let empty = match std::fs::read_dir(&root) {
                Ok(mut it) => it.next().is_none(),
                // 讀不到就不猜。權限不足的時候喊「圖不見了」是製造假警報。
                Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            };
            if empty {
                println!(
                    "            ⚠ 但 {} 是空的（或不在）——上面那個數字是資料庫加出來的，",
                    root.display()
                );
                println!(
                    "            不是去量硬碟。沒帶 `--with-frames` 的匯出就長這樣：字都在，圖沒來。"
                );
            }
        }
        // Phase 0 的退出條件之一：每天 < 300MB
        let budget = 300 * 1024 * 1024;
        match per_day {
            Some(p) => {
                println!(
                    "  每天約    {} {}  （Phase 0 預算 300 MB/天）",
                    fmt::bytes(p as i64),
                    if p as i64 <= budget { "✓" } else { "✗" }
                );
                // 兩個分母差得夠多就講出來。少了這一句，一份用滿一年的
                // 資料庫會印出一個看起來很小的數字，而它小是因為畫面早就
                // 被保留期清掉了，不是因為她真的只用這麼多。
                if s.image_bytes > 0 && image_days + 1.0 < span_days {
                    println!(
                        "            （文字有 {:.0} 天、畫面只剩 {:.0} 天——保留期不一樣，\
                         \n              所以這兩半是各算各的天數再相加，不是總量除以 {:.0}）",
                        span_days, image_days, span_days
                    );
                }
            }
            None if span_days < 0.5 => println!(
                "  每天約    還不知道（只錄了 {:.1} 小時，不到半天不外推）\
                 \n            目前一共 {}。要驗 Phase 0 的 300 MB/天，得先錄滿一天。",
                span_days * 24.0,
                fmt::bytes(disk_total)
            ),
            // 文字夠久、畫面不夠：多半是剛簽第三張同意書，或者保留期剛把舊圖
            // 清光。整句不外推——把畫面當成 0 加進去，會蓋出一個假的 ✓。
            //
            // 但「畫面不夠」有兩種，而舊版把它們印成同一句。
            //
            // 一份圖全被保留期清光的資料庫（用超過 `frames_days` 又停了一陣子
            // 沒錄）手上是**零列**有圖，於是 `image_days` 是對一個空集合做出來
            // 的 0.0——印成「還留著圖的只有 0.0 小時」，讀起來像量到的一段很短
            // 的時間。跟在後面那句「再錄滿半天才算得出來」在這裡是對的（再錄
            // 就會有），可是同一句話在**每一次寫圖都失敗**的機器上（磁碟滿、
            // 資料夾被鎖）也一字不差地出現，而那台機器再錄一年也不會有。
            //
            // 所以零列就直說零列，不要拿 0.0 小時充當一個測量值。
            None if s.image_bytes == 0 => println!(
                "  每天約    還不知道（文字有 {:.0} 天，但現在一張圖都沒留著）\
                 \n            目前一共 {}。可能是剛開始存圖，也可能是舊的過了 frames_days\
                 \n            被清掉了，或者每一次存圖都失敗——`sister doctor` 分得出來。",
                span_days,
                fmt::bytes(disk_total)
            ),
            None => println!(
                "  每天約    還不知道（文字有 {:.0} 天，但還留著圖的只有 {:.1} 小時）\
                 \n            目前一共 {}。畫面那一半得再錄滿半天才算得出來。",
                span_days,
                image_days * 24.0,
                fmt::bytes(disk_total)
            ),
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use sister_core::db::DbStats;

        /// **一整排 0 旁邊那個 1。**
        ///
        /// 他跑完 `sister forget --last 24h --yes`，而最後一場錄製當掉過
        /// （`ended_at IS NULL` 而且是最新的一場）。`delete_empty_sessions` 不
        /// 准碰那一列，於是這一頁長成：
        ///
        /// ```text
        ///   ⚠  她**錄過**，而她記下來的東西現在一列都不剩
        ///   工作階段  1
        ///   畫面      0 張保留，0 張因重複被折疊
        ///   文字      0 段
        /// ```
        ///
        /// 那個 1 是這一頁上唯一一個不是 0 的數字，讀起來像「有一場還在」。
        #[test]
        fn the_session_that_outlived_an_erasure_is_marked_as_a_shell() {
            let erased = DbStats {
                sessions: 1,
                ..Default::default()
            };
            use sister_core::heartbeat::{Phase, Presence};
            let gone = Presence::Stopped { at: None };
            let line = sessions_line(&erased, gone);
            assert!(line.contains('1'), "數字還是要印：{line}");
            assert!(
                line.contains("空殼"),
                "清空之後剩下的那一列是個殼，這一行得說出來：{line}"
            );

            // **沒有人佔著這個資料目錄，那一列就不可能是「正在錄的那一場」。**
            // 上一版兩種都印（「當掉了，或是她此刻正在錄」），而這一頁手上有
            // `data_dir`——分得出來還印一個「或」，是把自己的懶惰講成他的功課。
            assert!(line.contains("當掉"), "沒有人佔著，那就是當掉了：{line}");
            assert!(
                !line.contains("正在錄"),
                "沒有 recorder 佔著這個目錄，不可以說她可能正在錄：{line}"
            );

            // 反過來：她真的在錄的時候不准說她當機——這是這幾句話裡唯一會嚇到
            // 人的錯。
            let live = sessions_line(&erased, Presence::Live(Phase::Recording));
            assert!(live.contains("空殼"), "{live}");
            assert!(
                !live.contains("當掉"),
                "她此刻正在錄，不可以說她當掉了：{live}"
            );

            // **而「正在起來」跟當機走。** 她那一列要等 `Db::open` 回來才
            // INSERT，所以開機那幾分鐘手上這一列一定是上一次留下來的殼——上一
            // 版這裡收 `occupied = beat.is_some()`，於是這一列印「她正在錄，或
            // 正在開機」，而同一份報告上面那個 ⚠ 說「有一個 sister record 正在
            // 起來」。
            let booting = sessions_line(&erased, Presence::Live(Phase::Booting));
            assert!(booting.contains("空殼"), "{booting}");
            assert!(booting.contains("當掉"), "那一列不是她的：{booting}");
            assert!(
                !booting.contains("正在錄"),
                "她還在開資料庫，一列都還沒寫：{booting}"
            );
            let thinking = sessions_line(
                &erased,
                Presence::Thinking {
                    at: 1,
                    until: 240_000,
                },
            );
            assert!(thinking.contains("空殼"), "{thinking}");
            assert!(!thinking.contains("當掉"), "她還在收尾：{thinking}");
            assert!(
                !thinking.contains("沒有任何 recorder"),
                "行程還在：{thinking}"
            );
            assert!(thinking.contains("想最後一段"), "{thinking}");
            for (a, b) in [
                (&line, &live),
                (&line, &booting),
                (&line, &thinking),
                (&live, &booting),
                (&live, &thinking),
                (&booting, &thinking),
            ] {
                assert_ne!(a, b, "四種心跳四個下場，不可以共用一句話");
            }

            // 而她真的記著東西的時候，這個但書一個字都不准出現：那會把一場
            // 好好的錄製講成一個殼。
            for has in [
                DbStats {
                    sessions: 1,
                    chunks: 6,
                    ..Default::default()
                },
                DbStats {
                    sessions: 2,
                    frames: 1,
                    ..Default::default()
                },
            ] {
                for beat in [
                    gone,
                    Presence::Live(Phase::Booting),
                    Presence::Live(Phase::Recording),
                ] {
                    let line = sessions_line(&has, beat);
                    assert!(!line.contains("空殼"), "她記著東西：{line}");
                    assert_eq!(line, format!("  工作階段  {}", has.sessions));
                }
            }

            // 一列都沒有的時候也不准講——沒有殼可以講。
            assert_eq!(sessions_line(&DbStats::default(), gone), "  工作階段  0");
        }

        /// **「系統 1」和上面那個 ⚠ 不可以同時只講一半。**
        ///
        /// 清空之後她再開始錄，`Recorder::new` 寫的那一列 `session_start` 會讓
        /// 這一行冒出一個 1，而正上方那個 ⚠ 才剛說完「她記下來的東西一列都不
        /// 剩」。兩句各自都是真的。
        #[test]
        fn the_only_system_event_left_says_what_it_is() {
            let just_started = DbStats {
                sessions: 1,
                system_events: 1,
                session_marks: 1,
                ..Default::default()
            };
            let line = events_line(&just_started);
            assert!(line.contains("系統 1"), "數字還是要印：{line}");
            assert!(
                line.contains("開始／結束"),
                "那個 1 是那場錄製自己的標籤，這一行得說出來：{line}"
            );

            // 她真的記到東西的那一刻，但書就得消失——那時候「系統 3」講的是
            // 鎖定、睡眠、被規則擋掉那幾種，是他那天真的發生過的事。
            let recording = DbStats {
                sessions: 1,
                system_events: 3,
                session_marks: 1,
                frames: 12,
                ..Default::default()
            };
            let line = events_line(&recording);
            assert!(!line.contains("開始／結束"), "{line}");
            assert_eq!(line, "  事件      焦點 0 · 剪貼簿 0 · 輸入 0 · 系統 3");

            // 一列都沒有的時候也不准講。
            assert_eq!(
                events_line(&DbStats::default()),
                "  事件      焦點 0 · 剪貼簿 0 · 輸入 0 · 系統 0"
            );
        }
    }
}

pub mod doctor {
    use super::*;
    use crate::fmt;
    use sister_core::config::Config;
    use sister_core::db::SignalVerdict;

    fn line(ok: bool, label: &str, detail: &str) {
        mark(if ok { "✓" } else { "✗" }, label, detail);
    }

    /// 第三種狀態：**還沒驗到**。
    ///
    /// ✓ 和 ✗ 都是斷言，而有些東西 doctor 當下沒辦法斷言——例如網址擷取，
    /// 前景不是瀏覽器的時候根本沒有東西可讀。把它畫成 ✓ 是說謊（那正是
    /// 上一版 `✓ OCR 語言` 的錯法），畫成 ✗ 則是製造一則使用者每次都會
    /// 看到、於是很快就學會忽略的假警報。所以給它自己的符號。
    fn mark(sym: &str, label: &str, detail: &str) {
        // `{label:<22}` 補的是**位元組**，而這一頁的標籤幾乎都是中文：
        // 「本機記錄」佔 12 個位元組、8 個字寬，「現在有沒有在看」佔 21 個
        // 位元組、14 個字寬——同一份補白讓兩行的說明差了 6 格。
        println!("  {sym} {} {detail}", fmt::pad(label, 16));
    }

    /// 「現在有沒有在看」那一列的符號和句子。
    ///
    /// **這一支拿不到 `data_dir`，而那是它存在的理由。** 上一版這段程式長在
    /// `doctor::run` 裡，於是它伸手又讀了一次心跳——而這一頁最上面已經讀過一
    /// 次了。兩次讀之間她可以從 `Booting` 跳到 `Recording`、心跳可以過期、
    /// `heartbeat::stop` 可以在上面蓋一塊墓碑，於是同一份報告上「上一次錄製」說
    /// 「她現在還在跑」，八行之下這一列說「沒有任何 sister record 在跑」。
    /// 實測撞得到（把心跳檔反覆建立／刪除，六次 doctor 裡出現三次），而在那
    /// 台 `Db::open` 要跑好幾分鐘的機器上，這兩行之間隔的就是那幾分鐘。
    ///
    /// 那種錯**測不到**：它是一場競賽，重跑一次就不見了，而突變測試（把這裡
    /// 改回自己讀一次）兩道 gate 都殺不掉——CI 上那個心跳檔在兩次讀之間不會
    /// 變。所以守它的不是測試，是型別：這支函式手上沒有路徑，寫不出第二次
    /// 讀。同 `recorded_verdict` / `crash_verdict`，把判斷搬進一支收「已經量
    /// 好的東西」的函式。
    ///
    /// `paused` 是 `Some(從什麼時候起)`＝暫停中。它排在最前面，因為它壓過其他
    /// 每一種：暫停的時候有沒有 recorder 佔著都無所謂，她根本沒在看。
    fn watching_verdict(
        paused: Option<String>,
        beat: sister_core::heartbeat::Presence,
    ) -> (&'static str, String) {
        use sister_core::heartbeat::{Phase, Presence};
        // 這一行是**唯一**會告訴使用者「你上禮拜按的暫停還開著」的地方。暫停
        // 不會自己過期（見 `sister_core::pause`），所以那條路很真實，而它的症
        // 狀是「所有數字都是 0」——最容易被讀成「程式壞了」。
        if let Some(since) = paused {
            return (
                "⏸",
                format!("**暫停中**（{since}）。這段期間什麼都不會被記錄"),
            );
        }
        match beat {
            Presence::Live(Phase::Recording) => ("✓", "有一個 sister record 正在跑".to_string()),
            // 「正在起來」不可以印成「正在跑」。一顆一年份的資料庫，migration
            // 003 重建 bigram 索引可以跑好幾分鐘，而那幾分鐘裡她一個字都沒記
            // ——使用者照著那句話去做一件他想被記住的事，之後問「剛剛發生什麼
            // 事」會拿到一片空白。
            Presence::Live(Phase::Booting) => (
                "…",
                "有一個 sister record **正在起來**（多半在開資料庫）。\
                 還沒開始記東西，等它印出第一行再做要被記住的事"
                    .to_string(),
            ),
            Presence::Thinking { .. } => (
                "…",
                "錄製已停，解釋層還在想最後一段（行程還在，不要再開一個）".to_string(),
            ),
            // 「沒有暫停」以前就印到這裡為止，而那句話讀起來是「她在看」——和
            // 字母人那句「在聽」是同一個謊。沒有暫停**不等於**有人在錄：
            // `sister record` 是另一個行程，沒有人開它的時候旗標一樣是乾淨的。
            //
            // 但這裡不能畫 ✗。doctor 最常見的用法就是「開始之前先檢查一下」，
            // 那時候沒有人在錄是**正常的**——畫成失敗就是一則每次都會出現、於
            // 是很快就被學會忽略的假警報，正是這個檔案上面幾行在講的那種。
            Presence::NeverStarted
            | Presence::Unreadable
            | Presence::Stopped { .. }
            | Presence::Stalled { .. } => (
                "?",
                "沒有暫停，但也沒有任何 sister record 在跑（還沒開始的話，這是正常的）".to_string(),
            ),
        }
    }

    /// 「已記錄」那一列的符號和句子，**一起**算出來。
    ///
    /// 拆成函式是因為這一列的兩半（畫 `?` 還是 `✓`、後面接哪一句）以前是兩段
    /// 各自寫的 match，而這個 repo 已經被那個形狀咬過：符號說一件事、句子說
    /// 另一件事。同一個 match 出來的東西不可能對不起來。
    fn recorded_verdict(
        frames: i64,
        chunks: i64,
        empty: Emptiness,
        detail: &str,
    ) -> (&'static str, String) {
        match (frames, chunks) {
            // 「一個字都還沒進來」和「進來過，被你刪掉了」在這一行上長得一樣，
            // 而它們的下一步剛好相反：前者要他去按開始錄，後者他剛剛才親手把
            // 東西刪掉。分得出來是因為 `Emptiness` 底下那個位元活在 `meta` 裡，
            // `forget` 和 `prune` 都不碰它（見 `Db::ever_recorded`）。
            //
            // 但「畫面 0、文字 0」**不等於**「一列都不剩」：一場全程被排除規則
            // 擋掉（或全程暫停）的錄製走到的也是 (0, 0)，而那顆資料庫裡有工作
            // 階段、有事件、有一整段排除稽核。那時候說「被 forget 忘掉了」是
            // 假的，而且指錯方向——他該去看的是那幾條規則。
            (0, 0) => match empty {
                Emptiness::Erased => (
                    "?",
                    format!("{detail}——錄過，但現在一列都不剩（`forget` 或保留期）"),
                ),
                Emptiness::Blocked => (
                    "?",
                    format!("{detail}——她錄了，但那段時間被規則擋掉或暫停了（見底下「隱私」）"),
                ),
                Emptiness::Barren => (
                    "!",
                    format!("{detail}——她錄過，但一個字都沒存進來（先看 `capture.enabled`）"),
                ),
                // `!` 是「有東西不對」，而她才剛開始的話沒有東西不對。
                Emptiness::Live => ("?", format!("{detail}——她此刻正在錄，到現在還沒有一列落地")),
                Emptiness::Booting => (
                    "?",
                    format!("{detail}——有一個 sister record 正在起來，還沒開始記東西"),
                ),
                Emptiness::Fresh => ("?", format!("{detail}（還沒有任何內容）")),
            },
            // 「0 張畫面 · 0 段文字 ✓」是這個專案一路在修的那個災難本身
            // ——錄了一整天、資料庫在長大、一個字都沒進去。有資料庫卻沒
            // 內容不該是打勾。
            (_, 0) => (
                "✗",
                format!("{detail}——有畫面卻一個字都沒有，OCR 沒讀到東西"),
            ),
            _ => ("✓", detail.to_string()),
        }
    }

    /// 「零當機」那一列的符號和句子。
    ///
    /// Phase 0 的退場條件是「連續 7 天自我錄製、零當機」，而在這之前那句話的
    /// 驗證方式是使用者自己記得有沒有當過。資料庫一直知道答案：Ctrl-C 走的是
    /// 正常收尾，所以沒有 `ended_at` 的那幾段，剩下的解釋只有被殺、當機、
    /// 關機、拔電。
    /// # 這一格為什麼收 `CrashAudit` 而不是兩個數字
    ///
    /// 因為那兩個數字數的是**活下來的那幾場**。一場「開起來、還沒讀到第一張畫
    /// 面就死掉」的錄製沒有內容，於是下一次 `prune` 連它的紀錄一起刪掉，分子
    /// 和分母同時少一——她死得越早，這一格讀起來越乾淨。整段理由在
    /// [`sister_core::db::migration_006`] 的註解裡，那裡也有實測。
    ///
    /// # 這一支為什麼不收 `occupied`
    ///
    /// 上一版收了，然後只把它扣進 `a.crashed(occupied)`——分母、`rows_unfinished`、
    /// `last_crash` 三個一起印出來的數字全都還含著正在錄的那一場，於是同一行
    /// 先說「那一場沒有算進去」再報出那一場的開始時間當作當機時間。扣除搬進
    /// [`sister_core::db::Db::crash_audit`] 之後，這裡連那個布林都拿不到，
    /// 也就沒有東西可以只扣一半。`a.live` 只用來講「扣掉了」。
    fn crash_verdict(a: &sister_core::db::CrashAudit, empty: Emptiness) -> (&'static str, String) {
        // **她正在錄的是第一場。** `started` 已經把她扣掉了，所以這裡的 0 是
        // 「在她之前沒有別的」，不是「計數器答不出來」——而底下那一段把兩者
        // 當成同一件事。少了這一格，一台全新的機器在她的第一次錄製期間（前
        // 幾張畫面已經落地）印出來的是：
        //
        // ```text
        // ✓ 已記錄     4 張畫面 · 9 段文字
        // ? 零當機     那幾場的紀錄已經不在了（`forget` 或保留期），現在算不出來
        // ? 上一次錄製 02:52:51 開始，沒有收尾——她現在還在跑
        // ```
        //
        // 一台從來沒刪過東西的機器被指控刪過東西，夾在「4 張畫面」和「她現在
        // 還在跑」中間。**扣掉一個數字就是替那個數字的 0 造一個新的意思**，
        // 而 `> 0` 這一行是上一版留下來的、沒有被重讀的那一行。
        //
        // `ended == 0` 是這句話自己的前提，不是多餘的保險：「在這之前沒有錄
        // 過」和「有一場好好地收尾過」不可能同時成立。真的資料庫走不到那個組
        // 合（`ended <= started`，而 `started` 只扣掉正在錄的那一場），手改過
        // 的可以——而少了這一格，那顆手改過的會被告知她正在錄人生第一場。不
        // 寫這個條件就是替一句話宣告一個它自己沒有檢查的前提。
        if a.live && a.started == 0 && a.ended == 0 {
            return if a.floor {
                // 升上來那天一列都不剩，所以「在她之前沒有別的」是猜的。
                (
                    "?",
                    "她現在正在錄。升上來那天一場紀錄都沒剩，所以在那之前有沒有當過，算不出來"
                        .to_string(),
                )
            } else {
                (
                    "✓",
                    "她正在錄第一場——在這之前沒有錄過，所以還沒有當機可以數".to_string(),
                )
            };
        }
        // 計數器答得出來的時候就讓它答——它才是撐得過刪列的那一組數字。
        //
        // 走到這裡的 `started == 0` 有兩種：這一版之後才出生、而且真的沒開過
        // 的資料庫（精確的 0），和升級那天列已經被掃光的資料庫（回填只數得到
        // 還在的列，所以也是 0）。兩種都得往下走 `Emptiness`——它分得出來，
        // 而這裡分不出來。
        if a.started > 0 {
            // **升上來的那一顆數不到升級之前被清掉的那幾場**，而 ✓ 那一句
            // （「全部正常收尾」）正是在對那個看不見的集合下斷言——這一格自己
            // 就是這批 bug 的形狀：句子沒錯，範圍錯了。同一句補在兩條路上，
            // 因為 ✗ 那邊少報的是當機數，✓ 那邊少報的是「有沒有當過」。
            //
            // 寫成條件句不是定指。第一版寫「升上來之前**被清掉的那幾場**不在
            // 這個數字裡」，而那台機器可能一場都沒被清過——那句話替它宣告了一
            // 場沒發生的刪除，和第十三次同一個形狀，只是換了一組字所以躲過了
            // CI 那條 `LIE` pattern。migration 本來就不可能知道有沒有被刪過。
            //
            // 「**這裡的**數字」不是「這個數字」。✗ 那一條有兩個數字（分母和
            // 當機數），而且下面那個 `live` 補充自己一個數字都沒有——上一版排成
            // `{live}{scope}`，於是它讀起來是這樣：
            //
            //     ——當機、關機、拔電。現在正在錄的那一場沒有算進去（這個數字
            //     是升上來那天數到的……）
            //
            // 「這個數字」最近的先行詞變成了「正在錄的那一場」，而那一句根本
            // 沒有數字。兩句各自都是真的，貼在一起指到了錯的東西。排序也跟著
            // 換成 `{scope}{live}`：範圍聲明黏著數字，扣除聲明收在句尾。
            let scope = if a.floor {
                "（這裡的數字是升上來那天數到的；升級之前如果有錄製被清掉，不在裡面）"
            } else {
                ""
            };
            // 正在錄的那一場已經在 `crash_audit` 裡從每一個數字扣掉了。這一句
            // 只是把「扣掉了」講出來——兩條路都要講，`✓` 那條上一版漏了，於是
            // 它印在「她現在還在跑」的正上方說「2 段全部正常收尾」。
            let live = if a.live {
                "。現在正在錄的那一場沒有算進去"
            } else {
                ""
            };
            let crashed = a.crashed();
            if crashed == 0 {
                return (
                    "✓",
                    format!("{} 段錄製全部正常收尾{scope}{live}", a.started),
                );
            }
            return (
                // 這裡以前畫的是 `!`，因為「另一個終端機正在錄」和「當掉了」
                // 在磁碟上長得一模一樣。心跳分得出來，所以那個理由沒了——而一
                // 條永遠翻不成 ✗ 的檢查讀起來像涵蓋，實際上是一格空白（#49
                // 就是這個形狀）。
                "✗",
                format!(
                    "{} 段錄製裡有 {crashed} 段沒有回來{}——當機、關機、拔電{scope}{live}",
                    a.started,
                    // 沒有留下紀錄的那幾場報不出時間，而且是故意的：一個
                    // 「她那天幾點幾分當過」的時間戳，正是那一列被刪掉要拿掉
                    // 的東西。所以這裡的時間只涵蓋還留著紀錄的那幾場。
                    //
                    // 兩邊都不含正在錄的那一場，所以這個減法對得起來。上一版
                    // `crashed` 扣過而 `rows_unfinished` 沒扣，於是同一個量在
                    // 相隔兩行的地方印出 3 和 4。
                    match (a.last_crash, crashed > a.rows_unfinished) {
                        (Some(t), false) => format!("（最後一次 {}）", fmt::timestamp(t)),
                        (Some(t), true) => format!(
                            "（還留著紀錄的最後一次 {}，另外 {} 段連紀錄都沒留下）",
                            fmt::timestamp(t),
                            crashed - a.rows_unfinished
                        ),
                        // 一段紀錄都沒留下：她開起來、什麼都沒存到就死了，然後
                        // 那幾列被清空場那一刀帶走。這一格是唯一還講得出這件事
                        // 的地方。
                        (None, _) => "（那幾場連紀錄都沒留下，所以報不出時間）".to_string(),
                    },
                ),
            );
        }
        match (a.rows, a.rows_unfinished) {
            // **「分母沒了」不是一種，是四種**，而上一版只講得出其中一種。
            //
            // 一場什麼都沒存到的錄製收工時會把自己那一列刪掉，所以
            // `capture.enabled = false` 的機器、和此刻才剛被開起來的
            // recorder，走到的都是 `(0, _)`——然後兩個都被告知那幾場的紀錄
            // 「不在了」，一個沒發生過的刪除。
            //
            // 兩個布林（`ever` + `stored`）換成 [`Emptiness`] 本人，是因為
            // 那兩個布林在呼叫端拼裝，而拼錯不會有人紅——這一批 bug 十次有
            // 十次犯在呼叫端。
            (0, _) => (
                "?",
                match empty {
                    Emptiness::Barren => {
                        "她錄過，但一個字都沒存進來，那幾場沒有留下紀錄——先看 `capture.enabled`"
                    }
                    Emptiness::Live => "她此刻正在錄，還沒有東西落地，所以還沒有一場算得出來",
                    // 她連第一拍都還沒開始，所以這裡沒有「還沒算出來」的東
                    // 西——是還沒有東西可以算。
                    Emptiness::Booting => {
                        "有一個 sister record 正在起來，還沒開始記東西，所以還沒有一場算得出來"
                    }
                    // 一場都不剩**不等於**沒錄過。`sessions` 那張表會跟著它
                    // 自己那幾列一起被 `forget` 和保留期帶走
                    // （`retention::delete_empty_sessions`），所以清空過的資
                    // 料庫上這裡是 0——而「還沒錄過」對一個五分鐘前才刪掉一
                    // 整天的人是假的。分母沒了就說分母沒了，不要順便宣布一個
                    // 沒有證據的「零當機」。
                    //
                    // `Blocked` 跟著走同一句：它代表這顆資料庫裡有稽核紀錄
                    // （所以她確實錄過），而那幾場的殼卻不在了。
                    Emptiness::Erased | Emptiness::Blocked => {
                        "那幾場的紀錄已經不在了（`forget` 或保留期），現在算不出來"
                    }
                    Emptiness::Fresh => "還沒錄過",
                }
                .to_string(),
            ),
            // 這兩條是**退路**，不是主線：schema 6 之後 `started` 一定 ≥ 列數，
            // 所以有列就走得到上面。走到這裡代表計數器不見了（有人手動刪
            // `meta`，或是在 `migrate` 之前拿到了 `Db`）——那就照上一版的說法
            // 說，不要用一個讀不到的計數器去宣布一個假的「零當機」。
            (n, 0) => ("✓", format!("{n} 段錄製全部正常收尾")),
            (n, u) => (
                "!",
                format!(
                    "{n} 段錄製裡有 {u} 段沒有正常收尾{}——當機、關機、拔電，\
                     或者現在正有另一個 sister 在錄",
                    a.last_crash
                        .map(|t| format!("（最後一次 {}）", fmt::timestamp(t)))
                        .unwrap_or_default()
                ),
            ),
        }
    }

    /// 「上一次錄製」那一列：什麼時候、怎麼結束的。
    ///
    /// 上面那一列（[`crash_verdict`]）只數得出「有幾段沒收尾」，答不了「上一段
    /// 是怎麼結束的」——而這兩件事的下一步差很多：按了停止什麼都不用做，同意書
    /// 被撤回的話她從現在起什麼都不會記。
    ///
    /// # 為什麼四個分支和後面那句補充要綁在一起
    ///
    /// 四個分支以前各自 `mark`／`line` 一次，於是後面那句補充要補四遍——而這一
    /// 批 bug 十次有十次犯在呼叫端。符號和句子一起算出來、印只印一次。
    ///
    /// 而且那句補充和「她現在還在跑」是**同一個布林的兩面**：`a.live` 為真的時
    /// 候，手上這一列就是正在錄的那一場（[`sister_core::db::Db::last_session`]
    /// 和 [`sister_core::db::Db::crash_audit`] 都取 `MAX(id)`），它確定是最後一
    /// 次。分開問就會分開錯：上一版一句問 `occupied`、一句只問 `traceless() >
    /// 0`，於是同一格先說「她現在還在跑」，再說「這個時間不一定是最後一次」。
    fn last_session_verdict(
        last: &sister_core::db::LastSession,
        a: &sister_core::db::CrashAudit,
    ) -> (&'static str, String) {
        let since = fmt::timestamp(last.started_at);
        let (sym, mut said) =
            match (last.ended_at, last.reason.as_deref()) {
                (Some(t), Some(r)) => (
                    "✓",
                    format!(
                        "{since} 開始，{} 結束——{}",
                        fmt::timestamp(t),
                        sister_core::model::EndReason::describe(r)
                    ),
                ),
                // 有收尾時間、卻沒有理由。這裡以前一律怪到「那一版還沒有在記」頭
                // 上，而那句話有一半機率是冤枉的：`sessions` 這張表永遠不會被刪，
                // `system_events` 會（保留期和 `sister forget` 都刪）。上一場比
                // `text_days` 還舊、或者 `sister forget --last 2d` 剛好蓋過去，理
                // 由就會憑空消失。
                //
                // 分得出來的證據是「那一場還剩幾筆事件」：一筆都不剩，就是整場被
                // 清掉了，不是那一版沒寫。
                (Some(t), None) if last.events_left == 0 => (
                    "?",
                    format!(
                        "{since} 開始，{} 結束——理由查不出來了\n\
                     \x20                    （那一場的事件紀錄已經被清掉：\
                     保留期，或 `sister forget` 蓋到那段時間）",
                        fmt::timestamp(t)
                    ),
                ),
                // 事件還在、就是沒有 `session_end`：alpha.17 以前寫下的紀錄。不是
                // 錯誤，只是那個版本答不出來——說出「那時候還沒有在記」，比留一個
                // 看起來像故障的空白好。
                (Some(t), None) => (
                    "?",
                    format!(
                        "{since} 開始，{} 結束（{} 那一版還沒有在記為什麼停）",
                        fmt::timestamp(t),
                        last.app_version
                    ),
                ),
                // 這一頁手上有心跳，所以「不是當掉，就是它現在還在跑」那個「或」
                // 不用留給他猜。
                //
                // **三種，不是兩種。** 開機那一段（可以長達幾分鐘）裡，這一列是
                // 上一次當機留下來的殼，而同時真的有一個 recorder 正在起來——
                // 「她現在還在跑」是錯的（那不是這一列），「現在沒有任何 recorder
                // 佔著這個資料目錄」也是錯的（有）。兩句話都有人印過。
                (None, _) => {
                    (
                        "?",
                        format!(
                    "{since} 開始，沒有收尾——{}",
                    match (a.live, a.beat) {
                        (_, sister_core::heartbeat::Presence::Thinking { .. }) =>
                            "錄製已停，解釋層還在想最後一段（行程還在）".to_string(),
                        (true, sister_core::heartbeat::Presence::Live(_)) =>
                            "她現在還在跑".to_string(),
                        // `live` 是 true 卻不是 Live/Thinking：算錯了，但這一句
                        // 仍往「不敢說她當掉」倒。
                        (true, sister_core::heartbeat::Presence::NeverStarted
                        | sister_core::heartbeat::Presence::Unreadable
                        | sister_core::heartbeat::Presence::Stopped { .. }
                        | sister_core::heartbeat::Presence::Stalled { .. }) =>
                            "她現在還在跑".to_string(),
                        (false, sister_core::heartbeat::Presence::Live(
                            sister_core::heartbeat::Phase::Booting,
                        )) =>
                            "她當掉了。現在有一個 sister record 正在起來，那一場的紀錄還沒進來"
                                .to_string(),
                        // 心跳說在錄、而這一列不是它的：`forget` 剛好把它那一
                        // 列帶走之類的角落。不猜原因，只講看得到的。
                        (false, sister_core::heartbeat::Presence::Live(
                            sister_core::heartbeat::Phase::Recording,
                        )) =>
                            "她當掉了。現在有一個 sister record 在跑，但這一列不是它的".to_string(),
                        (false, sister_core::heartbeat::Presence::NeverStarted
                        | sister_core::heartbeat::Presence::Unreadable
                        | sister_core::heartbeat::Presence::Stopped { .. }
                        | sister_core::heartbeat::Presence::Stalled { .. }) =>
                            "她當掉了（現在沒有任何 recorder 佔著這個資料目錄）".to_string(),
                    }
                ),
                    )
                }
            };
        // **「上一次」是一個承諾，而這裡拿得到的是「還留著紀錄的上一次」。**
        // 一場什麼都沒存到的錄製收工時連紀錄一起走（`delete_empty_sessions`），
        // 所以真的最後那一場可能根本不在這張表裡——而這一列會若無其事地指著更
        // 早的那一場，講一個過期的時間。開機即死的迴圈裡，那個時間可以是三天前。
        if !a.live && a.traceless() > 0 {
            // **不准在這裡猜原因。** 第一版寫的是「沒存到東西，或被 `forget`
            // 帶走了」，而 CI 那顆 `ci-live`（`capture.enabled = false` 然後她
            // 又開起來）當場紅了：那台機器一次都沒刪過東西，而這一句遞給他兩
            // 個原因、其中一個是一場沒發生過的刪除。這一列要講的只是**這個時
            // 間不一定是最後一次**，原因是另一列的事。
            said.push_str(&format!(
                "\n\x20                    （這是還留著紀錄的最後一場；\
                 另外 {} 場沒有留下紀錄，所以這個時間不一定是最後一次）",
                a.traceless()
            ));
        }
        (sym, said)
    }

    /// 「兩個字的中文」那一列。
    ///
    /// **`with_cjk == 0` 不是「這顆資料庫是空的」**，而這一列上一版把兩者當成
    /// 同一件事：直接拿 `with_cjk == 0` 去問 [`Emptiness`]，於是一台跑英文、
    /// 順手擋了 keepassxc 的機器讀到「沒有中文可以驗——那段時間被規則擋掉
    /// 了」，而它的 6 段英文好好地躺在兩行之上的 ✓ 裡。
    ///
    /// 所以那道「先確定真的一段字都沒有」的閘門寫在這支函式**裡面**。放在呼叫
    /// 端的話它就是下一個沒有人測得到的判斷——這一批修改到目前為止，每一個死
    /// 在呼叫端的判斷都是靠人眼發現的。
    fn bigram_verdict(
        indexed: i64,
        with_cjk: i64,
        chunks: i64,
        empty: Emptiness,
    ) -> (&'static str, String) {
        if with_cjk > 0 {
            return if indexed >= with_cjk {
                (
                    "✓",
                    format!("{with_cjk} 行中文全都進了索引，多舊的都查得到"),
                )
            } else {
                (
                    "✗",
                    format!(
                        "{indexed}/{with_cjk} 行進了索引——回填沒跑完，\
                         沒進去的那些用「帳單」「電話」這種兩個字的詞叫不出來。\
                         三個字以上不受影響，L1 抽出來的事實也不受影響",
                    ),
                )
            };
        }
        // 一個中文詞都沒有。最平凡的原因是她記了一整天英文——那時候「為什麼
        // 是空的」根本不是問題，因為她這裡不空。
        if chunks > 0 {
            return (
                "?",
                format!("她記的 {chunks} 段字裡一個中文詞都沒有，這個索引還沒被用到"),
            );
        }
        // 到這裡才是真的一段字都不剩，「為什麼」也才有意義。`text_chunks` 是
        // `forget` 和保留期都清的表，所以「等你錄過再驗一次」對一個剛把一整天
        // 忘掉的人，是叫他重做一件他刻意做掉的事——那句話只對其中一種成立。
        (
            "?",
            match empty {
                Emptiness::Erased => {
                    "一段字都不剩，所以沒有中文可以驗——她錄過，那些字被忘掉了或過了保留期"
                }
                Emptiness::Blocked => {
                    "一段字都沒有，所以沒有中文可以驗——那段時間被規則擋掉或暫停了"
                }
                Emptiness::Barren => "一段字都沒有，所以沒有中文可以驗——她錄過，但一個字都沒存進來",
                Emptiness::Live => "一段字都沒有，所以沒有中文可以驗——她此刻正在錄，還沒有東西落地",
                Emptiness::Booting => {
                    "一段字都沒有，所以沒有中文可以驗——有一個 sister record 正在起來，還沒開始記東西"
                }
                Emptiness::Fresh => "資料庫裡還沒有中文，等你錄過再驗一次",
            }
            .to_string(),
        )
    }

    /// 「你問過她什麼」那一列。
    ///
    /// `has_db` 是「這張表問得到嗎」。少了它，資料庫打不開的時候
    /// `unwrap_or_default()` 會給出一個 0，然後這裡掛著 ✓ 說「還沒問過任何
    /// 問題」——那是他一整年的題庫，而畫面上寫著沒有。
    ///
    /// `ever` 只在 `total == 0` 那一格說話，理由和 `sister queries` 那一頁一
    /// 樣：`forget` 和保留期都會把題庫帶走，所以那個 0 不夠格斷言「還沒問
    /// 過」。而它也只能砍掉一種可能——`ever_recorded` 答得出「她錄過」，答
    /// **不**出「他問過」（一個天天在錄、從來沒用過搜尋框的人也是這個 0），
    /// 所以錄過的時候就把可能性攤開，不要替他選一個。
    fn query_log_verdict(
        has_db: bool,
        on: bool,
        ever: bool,
        q: &sister_core::db::QueryLogStats,
    ) -> (&'static str, String) {
        match (has_db, on, q.total > 0) {
            (false, on, _) => (
                "?",
                format!("（設定是{}）", if on { "要記" } else { "不記" }),
            ),
            (_, true, true) => (
                "✓",
                format!(
                    "記著，已經 {} 題（{} 題她答不出來、{}）",
                    q.total,
                    q.empty,
                    // 出處只有字母人那邊點得動。不講來源的話，「0 題你點開了
                    // 出處」會被讀成「她的答案沒一次有用」——而真相可能只是
                    // 這些題全是從終端機問的。
                    match q.clickable {
                        0 => "還沒有從字母人問過".to_string(),
                        n => format!("字母人那邊 {}/{} 題點開了出處", q.clicked, n),
                    }
                ),
            ),
            // 錄過的機器上，這一句退回**只講現在**：題庫是空的，沒了。
            //
            // 三種可能不在這裡攤開，是刻意的。他天天在錄、從來沒用過搜尋框
            // 的話（這個專案自己就是），把「可能被 `forget` 忘掉了」印在每一
            // 次 doctor 上，就是一則他每次都會看到、於是很快就學會忽略的假
            // 警報——正是這個檔案上面 `mark` 那段在講的東西。攤開的地方是
            // `sister queries`，那一頁是他為了這件事才打開的。
            (_, true, false) if ever => (
                "✓",
                "記著，但題庫現在一題都沒有（`sister queries` 會把可能的原因列出來）".to_string(),
            ),
            // 她沒錄過的話，`forget` 和保留期都不可能帶走過任何東西——這時候
            // 「還沒問過」是完整的，而且它比上面那句有用：它在說下一步。
            (_, true, false) => ("✓", "記著（還沒問過任何問題）".to_string()),
            (_, false, true) => (
                "⏸",
                // 指令要指得到真的存在的東西，而且要連代價一起講。`forget`
                // 刪的是**一段時間**，不是一張表——他想清掉的是題庫，但那段
                // 時間裡的字、事實、畫面會一起走。少講後半句，他會按下去才發現。
                format!(
                    "**不記了**（privacy.query_log = false）。以前記的 {} 題還在——\
                     `sister forget --last 30d` 帶得走，但那會連同那 30 天的其他記憶一起忘掉",
                    q.total
                ),
            ),
            // 這一格不提「還沒問過」：關著的時候那張表本來就不會長，講不出
            // 也不需要講他問過幾題。
            (_, false, false) => ("⏸", "不記（privacy.query_log = false）".to_string()),
        }
    }

    /// 三個訊號稽核裡的一列。和上面兩支同一個理由：符號和句子一起算。
    ///
    /// `empty` 在這裡只管一件事——**沒有最後一場的時候，那是哪一種沒有**。
    /// `signal_audit` 的範圍是 `sessions` 的最後一列，而那張表會跟著它記下來
    /// 的東西一起消失（`retention::delete_empty_sessions`）。清空過的資料庫上
    /// 「零當機」那一列說「那幾場的紀錄已經不在了」，而這三列以前說「還沒有
    /// 任何一場」——**同一份報告，四行之隔，兩句互相打臉**。
    ///
    /// 而修那一次的時候我只餵了一個 `ever`，於是這三列變成**四行之隔、兩句
    /// 互相打臉的另一種**：`capture.enabled = false` 那台機器上，一行之上寫
    /// 「那幾場沒有留下紀錄」，這三行寫「那幾場的紀錄**不在了**」——它一次
    /// 都沒刪過東西。收 [`Emptiness`] 而不是收一個布林，就是為了讓「這是哪
    /// 一種沒有」只有一個答案，而且是上面那一列用的同一個。
    fn signal_line(a: &sister_core::db::SignalAudit, empty: Emptiness) -> (&'static str, String) {
        if a.scope_started_at.is_none() {
            // 沒有範圍就沒有分母，底下三種判決一句都套不上去（`rows` 必然是
            // 0，也就必然是 `TooEarly`）。只講那件唯一還說得出口的事——而
            // 「那件事」有四種。
            let why = match empty {
                Emptiness::Erased | Emptiness::Blocked => "那幾場的紀錄不在了",
                Emptiness::Barren => "那幾場一列內容都沒留下",
                Emptiness::Live => "她此刻正在錄，還沒有東西落地",
                Emptiness::Booting => "有一個 sister record 正在起來，還沒開始記東西",
                // 真的還沒有任何一場。這一句底下那個 `when` 也講得出來，
                // 讓它去講，不要在這裡多一句。
                Emptiness::Fresh => "",
            };
            if !why.is_empty() {
                return ("?", format!("{why}，現在沒有範圍可以驗"));
            }
        }
        // **「上一場」在她還在錄的時候是假的，而且是同一份報告裡的第二種假。**
        // 四行之上那一列（[`crash_verdict`]）的分母把正在錄的那一場扣掉了，於
        // 是「2 段錄製全部正常收尾」底下接著三行「上一場（02:45:42 起）」——問
        // 那 2 場裡哪一場是上一場，答案是都不是。那一場既不在分母裡，也還沒
        // 結束。
        //
        // 這個位元不在這裡算（`a.scope_is_live` 是 `signal_audit` 交出來的），
        // 理由和 `crash_audit` 收 `beat` 一樣：算在印字的地方，就會只餵給一起
        // 印出來的其中一個數字。
        let when = match (a.scope_started_at, a.scope_is_live) {
            (Some(ts), true) => format!("這一場（{} 起，還在錄）", crate::fmt::timestamp(ts)),
            (Some(ts), false) => format!("上一場（{} 起）", crate::fmt::timestamp(ts)),
            // 她正在開機的時候走的也是這裡：心跳在，但她那一列還沒 INSERT，
            // 所以「最後一場」是上一次留下來的那一場，或者一場都沒有。
            (None, _) => "還沒有任何一場".to_string(),
        };
        // 三種，不是兩種。「驗過了，是好的」和「資料還太少，看不出來」
        // 以前都印 ✓——那正是這一整節要抓的形狀，出現在抓它的工具上。
        match a.verdict {
            SignalVerdict::Alive => (
                "✓",
                format!(
                    "{when} {} 列，{} {}",
                    a.rows, a.populated, a.populated_label
                ),
            ),
            SignalVerdict::Broken => (
                "✗",
                format!("{when} {} 列，但沒有一列有內容——{}", a.rows, a.note),
            ),
            // 沒有列 ≠ 壞掉。可能只是這台機器沒有那個能力（replay
            // 讀不到 pid），或還沒錄到。不知道就說不知道。
            SignalVerdict::TooEarly if a.rows == 0 => ("?", format!("{when}裡沒有這種資料")),
            // 有列、都是空的，但還不夠多。她三秒前才開始的樣子，和
            // 真的壞掉的樣子，在這個列數上長得一模一樣——所以不猜。
            SignalVerdict::TooEarly => (
                "?",
                format!("{when}只有 {} 列、而且都是空的——還看不出來", a.rows),
            ),
        }
    }

    /// doctor 要用到的能力摘要。平台差異只收在這一個地方。
    ///
    /// 探測 OCR 要真的把引擎建起來，所以整份報告只問一次。
    #[derive(Default)]
    struct Caps {
        url: bool,
        /// 對現在的前景視窗真的問一次網址的結果。`None` = 本平台問不了。
        url_probe: Option<(&'static str, &'static str, String)>,
        /// 現在的前景視窗是誰：`(app, 標題)`。`None` = 本平台問不到，
        /// 兩個欄位各自可能是空字串（讀得到視窗但讀不到那一項）。
        ///
        /// `excluded_apps` / `excluded_titles` 比對的就是這兩個字串，所以
        /// 「規則有幾條」和「規則會不會生效」是兩件事：讀不到字串的話，
        /// 那些規則一條都不會命中——而數量照樣印得出來。
        focus_probe: Option<(String, String)>,
        /// 輸入 hook：`None` = 本平台沒有這個東西。
        /// 不是「沒試過」——doctor 現在會真的裝一次（見 [`caps`]）。
        input_hooks: Option<bool>,
        ocr: bool,
        /// 這台機器上**問過**了沒有。
        ///
        /// `#[cfg(not(windows))]` 那半邊回的是 `Caps::default()`，而
        /// default 的 `ocr_language` 是 `None`、`ocr_available` 是空的——
        /// 和「Windows 上真的問了，答案是一個語言包都沒裝」一模一樣。
        /// 那兩件事的下一步是相反的（一個是去 Windows 設定裝語言，一個是
        /// 換一台機器），所以印出來不能長一樣。
        ///
        /// 這條線 300 行前的 `capabilities::write` 旁邊就寫過了：那半邊的
        /// default 是「這個平台問不出來」，不是「問了，答案是做不到」。
        /// 少了這個欄位，讀出來的那一頁就正好犯了它自己記下來的錯。
        ocr_probed: bool,
        ocr_language: Option<String>,
        ocr_available: Vec<String>,
        /// **實測**出來的檢查列：(過了沒, 標籤, 說明)。
        ///
        /// 刻意在這裡就判定完、只留下要印的字，是為了把平台專屬的型別
        /// （`WindowsOcr`、`RawFrame`…）全部關在 `caps()` 裡面。
        /// doctor 的輸出段落不該長出一堆 `#[cfg]`。
        ocr_probes: Vec<(bool, &'static str, String)>,
        /// 記了不該記的（排除規則失效）
        broken_privacy: Vec<String>,
        /// 其實什麼都沒記住，但你不會發現
        degraded: Vec<String>,
    }

    /// 把一段辨識結果縮成一行可以印的樣子。
    ///
    /// doctor 是使用者自己對著自己的螢幕跑的，所以印出畫面上的字並不是外洩；
    /// 但完整倒出來會把終端機洗掉，而且真正要回答的問題只有一個：
    /// 「讀到的是人話，還是亂碼」。一行就夠了。
    #[cfg(windows)]
    fn sample(lines: &[String]) -> String {
        const MAX: usize = 36;
        let Some(first) = lines.iter().find(|l| !l.trim().is_empty()) else {
            return String::new();
        };
        let mut s: String = first.chars().take(MAX).collect();
        if first.chars().count() > MAX {
            s.push('…');
        }
        format!("「{s}」")
    }

    /// doctor 不蓋掉那份能力報告的理由，加上底下那幾則警告是誰講的。
    #[cfg(any(windows, test))]
    pub(crate) struct Keeper {
        pub why: &'static str,
        pub whose: &'static str,
    }

    /// doctor 該不該蓋掉 `capabilities.json`。`Some` = 不要蓋。
    ///
    /// **這一支刻意沒有 `#[cfg(windows)]`**，雖然唯一的呼叫端（[`caps`]）有。
    /// 判斷錯掉的代價是「一則真的隱私警告被換成一片乾淨」，而那種東西不可以
    /// 只活在一個開發機編不到、CI 也沒有任何測試踩得到的分支裡——那正是這個
    /// repo 一路在修的形狀，只是換成長在建置設定上。抽出來之後 Linux 那顆
    /// runner 每次 push 都驗一次。
    ///
    /// `any(windows, test)` 而不是 `allow(dead_code)`：Linux 上的 `sister`
    /// **執行檔**確實沒有人呼叫它（唯一的呼叫端是 `#[cfg(windows)]`），那是
    /// 真的死碼，不該被消音；Linux 上的**測試**用得到，所以它在那裡活著。
    /// 一句 `allow` 會把這兩件事一起蓋掉，連以後真的變成死碼那天也一起。
    #[cfg(any(windows, test))]
    pub(crate) fn keep_capabilities(
        beat: sister_core::heartbeat::Presence,
        previous: Option<&sister_core::capabilities::Report>,
    ) -> Option<Keeper> {
        use sister_core::heartbeat::{Phase, Presence};
        // 先問完再 match：塞進 guard 裡會長到 rustfmt 得把它拆成三行，而那三行
        // 讀起來會比它要講的事複雜。
        let has_evidence =
            previous.is_some_and(sister_core::capabilities::Report::has_session_evidence);
        match beat {
            Presence::Live(Phase::Recording) => Some(Keeper {
                why: "她正在錄，能力報告交給那個行程寫",
                whose: "正在錄的那個行程說",
            }),
            // 開機中的那一個還沒寫過任何東西，讀到的是**上一場**留下的。把它
            // 說成「正在錄的那個行程說」會讓他去停一個沒有在做那件事的行程。
            Presence::Live(Phase::Booting) => Some(Keeper {
                why: "她正在起來，等她開完自己會寫一份",
                whose: "上一場留下的報告說",
            }),
            // 迴圈已經跳出，但那個行程收工前還會寫最後一份。蓋掉的話，這一場
            // 路上發生的事（UIA 半路投降了）會被一份全新的探測換掉。
            Presence::Thinking { .. } => Some(Keeper {
                why: "錄製已停，解釋層還在想最後一段——能力報告交給那個行程收工時寫",
                whose: "正在收尾的那個行程說",
            }),
            Presence::NeverStarted
            | Presence::Unreadable
            | Presence::Stopped { .. }
            | Presence::Stalled { .. }
                if has_evidence =>
            {
                Some(Keeper {
                    why: "上一場路上發生的事只有那一場問得到，這裡問不出來",
                    whose: "上一場留下的報告說",
                })
            }
            Presence::NeverStarted
            | Presence::Unreadable
            | Presence::Stopped { .. }
            | Presence::Stalled { .. } => None,
        }
    }

    #[cfg(windows)]
    fn caps(data_dir: &Path, config: &Config) -> Caps {
        use sister_capture::traits::{Ocr, ScreenSource};
        use sister_capture::windows::input::{HookState, WindowsInput};
        use sister_capture::windows::{Capabilities, ocr::WindowsOcr, screen::WindowsScreen};

        // 不宣稱，直接裝一次。以前 doctor 永遠印「輸入 hook 沒裝上」，
        // 因為它根本沒去裝——那是一則恆真的假警報，而假警報會連坐旁邊
        // 那則真的警告一起被忽略。hook 只數次數不看內容，裝一次很便宜。
        // doctor 只想知道 hook 裝不裝得上，聚合視窗多長無關緊要
        let _ = WindowsInput::start(sister_core::now_ms(), config.capture.input_window_secs);
        let input_hooks = match WindowsInput::state() {
            HookState::Active => Some(true),
            // NotStarted 在上一行之後不可能發生；真發生了也是「沒裝上」
            HookState::Failed | HookState::NotStarted => Some(false),
        };

        let c = Capabilities::current(config);
        // 順手留一份給設定頁。README 的 quickstart 第一句就是「跑一次 doctor」，
        // 而在那之前設定頁只答得出「還不知道」——一個剛裝好、正在填排除規則的
        // 人，正是最需要知道「這台機器讀不到網址」的那個人。寫的是和 `record`
        // 一模一樣的那一份（`caps.report()`），不是另一種定義。
        //
        // 只有這裡寫，`#[cfg(not(windows))]` 那半邊不寫：那邊的 `Caps::default()`
        // 是「這個平台問不出來」，不是「問了，答案是做不到」。把前者寫成後者，
        // 就是這整個模組在對付的那種謊。
        //
        // **有人在錄的時候不寫。** `c.report()` 是一份開機探測，它的
        // `url_capture` / `browser_ticks` / `url_reads` 全是預設值——也就是
        // 「這一場什麼都還沒發生」。正在跑的那個 recorder 每分鐘寫進去的那份
        // 帶著真的證據（UIA 半路投降了、位址列一個都沒讀到），拿這份蓋掉它，
        // 等於把一則真的警告換成一片乾淨。而「覺得怪怪的所以跑一下 doctor」
        // 正是那件事最會發生的時候。
        //
        // 而且要**把那個行程講的話唸出來**。底下 `url_probe` 那一段是拿一份
        // 全新的 `WindowsFocus` 去問的，也就是一個**全新的 UIA**——正在錄的
        // 那個行程手上那份可能早就投降了，而兩份是不同的物件。doctor 於是會
        // 對著一台網銀正在被錄進去的機器印一個 ✓。這一行是那件事唯一的出口。
        //
        // **「她停了」不等於「可以蓋了」。** 上一版這道閘門只問 `is_recording`，
        // 於是有兩種情況會漏掉，而兩種都會弄丟同一樣東西：
        //
        // 1. 她**正在起來**。心跳是 `Booting`，`is_recording` 回 false，於是
        //    doctor 在她開資料庫的那幾分鐘蓋掉上一場的報告——而她開完之後自己
        //    也要寫一份，這兩個寫手還會互相蓋。
        // 2. 她**已經收工**了。這是最常見的那一條：覺得怪怪的 → 按停止 →
        //    跑 doctor → 打開設定頁。而報告裡那幾個欄位（`gave_up`、
        //    `browser_ticks`、`url_reads`）記的是**已經結束的那一場**路上發生
        //    的事，doctor 手上這份全新的 UIA 永遠問不出來（見
        //    `Report::has_session_evidence`）。
        //
        // 所以判斷的是「有沒有東西可以弄丟」，不是「她在不在」。
        let previous = sister_core::capabilities::read(data_dir);
        let beat = sister_core::heartbeat::presence(data_dir, sister_core::now_ms());
        if let Some(Keeper { why, whose }) = keep_capabilities(beat, previous.as_ref()) {
            println!("  （{why}——這裡不蓋掉它。）");
            for line in previous
                .as_ref()
                .map(|r| r.broken_privacy_rules(&config.privacy))
                .unwrap_or_default()
            {
                println!(
                    "  ⚠  {whose}：{}\n\
                     \x20    （底下那次 UIA 實測是這裡新開的一份，它答得出來不代表她答得出來）",
                    line.message
                );
            }
        } else if let Err(e) = sister_core::capabilities::write(data_dir, &c.report()) {
            eprintln!("⚠  寫不出能力報告（設定頁會說「還不知道」）：{e:#}");
        }
        let mut probes = Vec::new();
        let url_probe;
        let focus_probe;

        // UIA：一樣不宣稱。真的對現在的前景視窗問一次網址。
        // `✓ UIA 建得起來` 這句話的價值是零——使用者要知道的是
        // 「我的網銀規則現在到底會不會生效」。
        {
            use sister_capture::traits::FocusSource;
            let mut source = sister_capture::windows::focus::WindowsFocus::new();
            let snapshot = source.snapshot(sister_core::now_ms()).unwrap_or_default();
            let app = snapshot.app_key();
            // 排除規則比對的就是這兩個字串。讀得到才代表那些規則跑得動。
            focus_probe = Some((
                app.clone(),
                snapshot.window_title.clone().unwrap_or_default(),
            ));
            url_probe = Some(match (&snapshot.url, source.url_capture_alive()) {
                (Some(url), _) => (
                    "✓",
                    "讀你現在的網址",
                    format!("{app} → {}", crate::fmt::one_line(url, 60)),
                ),
                (None, false) => (
                    "✗",
                    "讀你現在的網址",
                    "UIA 卡住太多次，已經放棄——excluded_urls 這一整組規則不生效".to_string(),
                ),
                // 以下兩種都**不是失敗**，是「這一刻沒東西可測」。畫成 ✗ 的話，
                // 從終端機跑 doctor 永遠會看到它（前景就是那個終端機），
                // 於是它變成一則恆真的警告——那種東西會把整個警告區塊一起
                // 教壞。但也絕不能畫成 ✓：那會讓使用者以為驗過了。
                (None, true) if app.is_empty() => (
                    "?",
                    "讀你現在的網址",
                    "現在沒有前景視窗，這一刻測不出來".to_string(),
                ),
                (None, true) => (
                    "?",
                    "讀你現在的網址",
                    format!(
                        "前景是 {app}，不是瀏覽器（或位址列是空的）。\
                         把瀏覽器切到前景再跑一次 doctor 才驗得到"
                    ),
                ),
            });
        }

        if c.ocr && config.capture.ocr {
            let mut ocr = WindowsOcr::new(&config.capture.ocr_languages);

            // 第一關：引擎讀不讀得到字。圖是編進執行檔的，答案是已知的。
            match ocr.self_test() {
                Ok(lines) => {
                    let text = lines.join(" ");
                    let missing: Vec<_> = WindowsOcr::SELF_TEST_EXPECTS
                        .iter()
                        .filter(|w| !text.contains(**w))
                        .collect();
                    probes.push(if lines.is_empty() {
                        (
                            false,
                            "內建圖自我測試",
                            "一行都沒讀到——引擎建得起來，但它讀不出字".to_string(),
                        )
                    } else if missing.is_empty() {
                        (
                            true,
                            "內建圖自我測試",
                            format!("{} 行，內容正確 {}", lines.len(), sample(&lines)),
                        )
                    } else {
                        (
                            false,
                            "內建圖自我測試",
                            format!(
                                "讀到 {} 行，但少了 {:?}——讀得到字，讀錯了。實際：{}",
                                lines.len(),
                                missing,
                                sample(&lines)
                            ),
                        )
                    });
                }
                Err(e) => probes.push((false, "內建圖自我測試", format!("失敗：{e:#}"))),
            }

            // 第二關：你**現在這台螢幕**讀不讀得到。跟錄製走同一條路
            // （同一顆引擎、同一個原生解析度的抓圖），所以「內建圖過了但這關
            // 沒過」就直接指向畫面本身，而不是引擎或語言包。
            let mut screen = WindowsScreen::new();
            let grabbed = screen.grab(sister_core::now_ms());
            let grabbed_edge = match &grabbed {
                Ok(Some(f)) => Some(f.width.max(f.height)),
                _ => None,
            };
            let probe = match grabbed {
                Err(e) => (false, "讀你現在的螢幕", format!("抓不到畫面：{e:#}")),
                Ok(None) => (
                    false,
                    "讀你現在的螢幕",
                    "抓不到畫面（工作站鎖定時本來就不抓）".to_string(),
                ),
                Ok(Some(frame)) => {
                    let (w, h) = (frame.width, frame.height);
                    // 「讀不出字」和「圖上本來就沒字」在報告裡長得一模一樣。
                    // 亮度範圍把它們分開：全黑的擷取 lo == hi。
                    let contrast = match frame.luma_span() {
                        Some((lo, hi)) if lo == hi => {
                            format!(
                                "；而且整張圖只有一個顏色（亮度 {lo}），這是一次失敗的擷取，不是一張沒有字的畫面"
                            )
                        }
                        Some((lo, hi)) => format!("；畫面亮度 {lo}–{hi}"),
                        None => String::new(),
                    };
                    match ocr.recognize(&frame) {
                        Ok(lines) if lines.is_empty() => (
                            false,
                            "讀你現在的螢幕",
                            format!(
                                "{w}×{h} → 0 行。錄製會照跑、畫面會留下，\
                                 但搜尋永遠是空的{contrast}"
                            ),
                        ),
                        Ok(lines) => {
                            let texts: Vec<String> = lines.into_iter().map(|b| b.text).collect();
                            (
                                true,
                                "讀你現在的螢幕",
                                format!("{w}×{h} → {} 行 {}", texts.len(), sample(&texts)),
                            )
                        }
                        Err(e) => (
                            false,
                            "讀你現在的螢幕",
                            format!("{w}×{h} 辨識失敗：{e:#}{contrast}"),
                        ),
                    }
                }
            };
            probes.push(probe);

            // 只在真的卡到的時候講。平常這是一個沒有人需要知道的數字。
            //
            // 比的是**剛剛真的抓到的那張圖**，不是設定檔裡的數字。OCR 吃的是
            // 原生解析度的像素，`max_long_edge` 只管存檔——拿它來比等於比錯
            // 對象，而且會比出一條永遠不會成立的規則：一條讀起來很對、卻
            // 一輩子命中不了任何東西的檢查，正是這個專案在獵的那種 bug。
            let limit = ocr.max_dimension();
            if let Some(edge) = grabbed_edge
                && edge > limit
            {
                probes.push((
                    false,
                    "影像尺寸上限",
                    format!("剛剛抓到的畫面長邊 {edge} 超過引擎上限 {limit}：每一張畫面都會被拒絕"),
                ));
            }
        }

        Caps {
            url: c.url,
            url_probe,
            focus_probe,
            input_hooks,
            ocr: c.ocr,
            ocr_probed: true,
            ocr_language: c.ocr_language.clone(),
            ocr_available: c.ocr_languages_available.clone(),
            ocr_probes: probes,
            // 終端機沒有「排除網址那一格」和「輸入節奏那一格」的分別，
            // 所以 `about` 在這裡用不到——一行一行印就是了。分格是設定頁
            // 的需要（見 `capabilities::About`）。
            broken_privacy: c
                .broken_privacy_rules(&config.privacy)
                .into_iter()
                .map(|b| b.message)
                .collect(),
            degraded: c.silently_degraded(config),
        }
    }

    #[cfg(not(windows))]
    fn caps(data_dir: &Path, config: &Config) -> Caps {
        let _ = (data_dir, config);
        Caps::default()
    }

    /// 「同意書／畫面暫存」那一行的字。
    ///
    /// 拉成一支函式不是為了短，是為了讓它和 [`frames_kept_words`] 一起被
    /// 一條測試釘住：同一張報告上這兩行講的是同一件事的兩半（「你准不准」
    /// 和「實際上會不會發生」），而它們歷史上各自漂了一次。
    ///
    /// 三張同意書裡只有這一張的答案要看設定檔——同意是**上限**不是開關，
    /// 所以「已同意」後面必須接著講設定檔把它擋在哪裡，不然使用者會拿著
    /// 一個 ✓ 去等一批永遠不會出現的截圖。
    fn frame_sheet_words(
        allows: bool,
        signed: bool,
        enabled: bool,
        store_images: bool,
    ) -> &'static str {
        match (allows, enabled, store_images) {
            // 這一行以前只問同意書，於是它說「會把截圖寫到硬碟」，而底下
            // 「保留畫面檔」那一行說「否（text-only 模式）」——同一張體檢
            // 報告上兩句互相打臉的話，而看的人沒有辦法知道該信哪一句。
            // `Consent::keeps_images` 的文件裡記的就是這根釘子。
            //
            // 補上 `capture.enabled` 是同一根釘子的第二次：它關著的時候每個
            // tick 直接回 `Tick::Disabled`，連螢幕都不會碰。少了它，這一行
            // 會在一台什麼都不錄的機器上印「會把截圖寫到硬碟」。
            (true, true, true) => "已同意——會把截圖寫到硬碟",
            (true, true, false) => "已同意，但設定檔的 store_images 關著 → 這台機器現在不會留截圖",
            (true, false, _) => "已同意，但 capture.enabled 關著 → 她連螢幕都不會看，截圖無從留起",
            // 「沒簽」和「簽了但條文改版」在這一行以前長得一模一樣，而上面
            // 「本機記錄」那一行分得出來——同一個檔案的同一種狀態，兩行給
            // 兩種說法，而使用者會照著比較好懂的那一句去做事。
            (false, _, _) if signed => "條文改版，舊簽名失效 → 只記螢幕上的字，不留截圖",
            (false, _, _) => "未同意 → 只記螢幕上的字，不留截圖",
        }
    }

    /// 「隱私／保留畫面檔」那一行的字。必須和 `Consent::keeps_images` 同意。
    fn frames_kept_words(allows: bool, enabled: bool, store_images: bool) -> &'static str {
        match (allows, enabled, store_images) {
            (true, true, true) => "是",
            (true, true, false) => "否（text-only 模式）",
            (true, false, _) => "否——capture.enabled 關著，這台機器根本不會開始錄",
            (false, _, true) => "否——設定要留，但第三張同意書沒簽（同意書說了算）",
            (false, _, false) => "否（沒簽第三張，設定也關著）",
        }
    }

    /// `loaded` 是 `Result` 而不是 `&Config`，因為**設定檔壞掉也要能跑**。
    ///
    /// 以前這裡收的是已經載好的值，於是 `main` 得先 `load_config(..)?`——
    /// 一個 TOML 語法錯就讓 `sister doctor` 整個停在門口，只吐一行解析錯誤。
    /// 而他打開 doctor 正是因為有東西壞了，「設定檔壞了」就是其中一種：用
    /// 它自己要診斷的東西把它擋在門外，等於在最需要它的那一刻把它關掉。
    /// OCR 讀不讀得到字、抓不抓得到網址、同意書簽了沒——這些跟那份設定檔
    /// 一點關係都沒有，卻一起消失了。
    ///
    /// 但也不能安靜地改用預設值跑：底下「排除的 app 9 條規則」會被讀成他
    /// 自己寫的那 9 條（實際上是內建的，他寫的 30 條一條都沒載進來），而他
    /// 看完就走了。這正是 `load_config` 當初拒絕的那個情境。
    ///
    /// 所以兩件事都要：**照跑，而且在最上面說清楚底下的設定值都是預設值。**
    /// 字母人的設定頁這一版修的是同一件事（讀不出來就整頁灰掉並說原因，而
    /// 不是白畫面），這裡是它在終端機這一邊的另一半。
    pub fn run(
        data_dir: &Path,
        loaded: Result<Config>,
        config_path: Option<PathBuf>,
    ) -> Result<()> {
        println!("🩺 AI-Sister 環境檢查\n");
        let broken = loaded.as_ref().err().map(|e| format!("{e:#}"));
        let config = &loaded.unwrap_or_default();
        if let Some(why) = &broken {
            println!("⚠  設定檔讀不出來，所以底下每一個跟設定有關的數字都是**內建預設值**，");
            println!("   不是你寫的那一份。修好它之前，那幾行不能拿來判斷你的規則有沒有生效。");
            println!("   原因：{why}\n");
        }
        let caps = caps(data_dir, config);

        println!("環境");
        line(
            true,
            "版本",
            &format!("sister {}", env!("CARGO_PKG_VERSION")),
        );
        line(true, "平台", std::env::consts::OS);

        let backend = crate::ops::record::backend_name();
        line(
            backend.is_some(),
            "擷取後端",
            backend.unwrap_or("無（此平台尚未實作；可用 sister replay 驗證管線）"),
        );

        // 同意書排在最前面，因為它是唯一一個「答案是否定的話，底下每一行都
        // 不重要」的檢查——她根本不會開始錄，OCR 讀不讀得到字就不是問題了。
        println!("\n同意書");
        let consent = sister_core::consent::load(data_dir);
        let signed_local = consent
            .get(sister_core::consent::Sheet::LocalRecording)
            .is_some();
        line(
            consent.allows_recording(),
            "本機記錄",
            if consent.allows_recording() {
                "已同意——她可以錄"
            } else if signed_local {
                "條文改版，舊簽名失效 → `sister record` 會拒絕啟動"
            } else {
                "未同意 → `sister record` 會拒絕啟動（`sister consent --grant local-recording`）"
            },
        );
        let signed_frames = consent
            .get(sister_core::consent::Sheet::FrameStorage)
            .is_some();
        line(
            consent.allows_frames(),
            "畫面暫存",
            frame_sheet_words(
                consent.allows_frames(),
                signed_frames,
                config.capture.enabled,
                config.capture.store_images,
            ),
        );
        // 走 `allows_cloud()` 而不是 `cloud_reading.is_some()`：條文改版之後
        // 舊簽名整份失效，而這一格猜錯了是東西送出去了。
        let cloud_words = if consent.allows_cloud() {
            match config.brain.cli() {
                Some((cmd, _)) => format!("已同意——去識別化後的字會交給 `{cmd}`"),
                None => "已同意，但還沒設定 [brain] command，一次都不會呼叫".to_string(),
            }
        } else if consent.cloud_reading.is_some() {
            "條文改版，舊簽名失效 → 視同未同意，一次都不會呼叫那支 CLI".to_string()
        } else {
            "未同意 → 解釋層一次都不會呼叫那支 CLI".to_string()
        };
        line(consent.allows_cloud(), "上雲解讀", &cloud_words);

        println!("\n位置");
        let db_file = crate::db_path(data_dir);
        line(
            data_dir.exists(),
            "資料目錄",
            &format!(
                "{}{}",
                data_dir.display(),
                if data_dir.exists() {
                    ""
                } else {
                    "（尚未建立）"
                }
            ),
        );
        line(db_file.exists(), "資料庫", &db_file.display().to_string());
        // `--config` 指到哪就印哪。舊版一律印 `default_path()`，於是一個
        // 用 `--config` 跑的人會看到一個他沒在用的路徑，後面還接一句
        // 「用預設值」——而底下的保留期天數明明是他自訂的那一份。
        // doctor 是出事時唯一的稽核面，這一行不能指錯地方。
        match config_path.or_else(Config::default_path) {
            // 「不存在」和「存在但讀不出來」是兩件很不一樣的事，而兩邊底下
            // 印的都是預設值。長得一樣的話，一個打錯了 TOML 的人會以為他根本
            // 沒建過設定檔，然後再建一個新的。
            Some(p) => line(
                broken.is_none(),
                "設定檔",
                &format!(
                    "{}{}",
                    p.display(),
                    match (&broken, p.exists()) {
                        (Some(_), _) => "（讀不出來，底下是預設值）",
                        (None, false) => "（不存在，用預設值）",
                        (None, true) => "",
                    }
                ),
            ),
            None => line(false, "設定檔", "無法決定路徑"),
        }

        println!("\n儲存");
        let probe = Db::open_in_memory().context("open probe database")?;
        line(true, "SQLite", &probe.sqlite_version());

        // 開著不關：底下「保留期」那一段還要拿它去問「現在有多少已過期」。
        //
        // **打不開不能是 `?`。** doctor 正是他在「有東西不對勁」的時候會跑的
        // 那一個指令，而 `?` 會讓它印到一半就死掉，只留下一句 `run migrations`
        // ——真正的理由（例如「這份資料庫比這個執行檔新」）被吞在 context
        // 底下，而他要的正是那一句。開不起來就報出來，然後把剩下的印完：
        // 底下每一段本來就都處理得了「沒有資料庫」。
        //
        // 底下每一段的「沒有資料庫」以前都印同一句「還沒有資料庫」。但
        // **「還沒開始錄」和「有記憶，只是這個執行檔打不開它」是相反的兩件
        // 事**——後者他手上是有東西的，講成沒有等於在他最慌的那一刻再騙他
        // 一次。所以理由帶著走。
        // 底下有好幾列要分「她跑過、什麼都沒存到」「她**正在錄**、還沒存到」和
        // 「她**正在起來**、還沒開始記」——同一份報告裡它們必須是同一個答案，
        // 所以只問一次，而且底下每一個要問的人拿到的都是 [`Phase`] 本人。
        //
        // **不要在這裡把它壓成布林。** 上一版壓成 `occupied = beat.is_some()`
        // 再發下去，於是「已記錄」那一列在開機那幾分鐘說「她此刻正在錄，到現在
        // 還沒有一列落地」，兩列之後「上一次錄製」說「她當掉了，現在有一個
        // sister record 正在起來」——同一份報告裡她同時在錄和還沒在錄。一個布
        // 林湊不出三種答案，而少掉的那一種永遠是「正在來、還沒好」。
        //
        // 「零當機」要問的又是另一題：那一列要扣掉「她現在正在錄的那一場」，而
        // 那道扣除只有在**她的那一列已經進資料庫**之後才成立。開機那一段
        // （`Phase::Booting`，`Db::open` 在一顆一年份的資料庫上可以跑好幾分鐘）
        // 裡目錄是有人佔著的，她的列卻還沒有——那時候扣掉的會是上一次當機留下
        // 來的殼。收 `Phase` 就分得出來，收布林就分不出來。
        //
        // 心跳**只讀一次**。再讀一次的話，兩次讀之間她可以從 `Booting` 跳到
        // `Recording`，於是同一份報告的上半和下半描述兩個不同的瞬間——一份自相
        // 矛盾的報告，而且重跑一次就不見了。
        // 心跳**只讀一次**，而且**不壓扁**。上一版這裡把 `Presence` 收成
        // `Option<Phase>`，`Thinking` 掉進 `None`，於是同一份報告六行之隔：
        // 「零當機」說她當掉了、「現在有沒有在看」說行程還在。
        let beat = sister_core::heartbeat::presence(data_dir, sister_core::now_ms());

        let (db, no_db) = match db_file.exists().then(|| Db::open(&db_file)) {
            Some(Ok(d)) => (Some(d), "還沒有資料庫"),
            Some(Err(e)) => {
                // 這句話裡有換行（`bail!` 就是那樣寫的），而 `mark` 只縮排第
                // 一行。續行補齊到同一欄，不然報告會在最重要的那一段散掉。
                line(
                    false,
                    "資料庫",
                    &format!("打不開：{e:#}").replace('\n', "\n                     "),
                );
                (None, "資料庫打不開（見上面那一行），不是沒有")
            }
            None => (None, "還沒有資料庫"),
        };

        // **問使用者那一顆，不是問一顆現做的。** 舊版查的是上面那個
        // in-memory probe，於是它證明的是「這份程式碼建得出索引」，
        // 而不是「你的資料庫裡有索引」。索引掉了的話 `db.rs` 會安靜地
        // 退回 LIKE 全表掃描——答案還是對的，只是一年份的資料要掃到天亮，
        // 而 doctor 會說 ✓。
        let fts_of = |d: &Db| {
            d.conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('text_fts','text_fts_uni')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0)
        };
        match &db {
            Some(d) => {
                let n = fts_of(d);
                line(
                    n == 2,
                    "FTS5 雙索引",
                    &if n == 2 {
                        "trigram + unicode61（你的資料庫裡）".to_string()
                    } else {
                        format!("你的資料庫裡只有 {n}/2 個——搜尋會退回全表掃描，資料還在但會很慢")
                    },
                );
            }
            // 還沒有資料庫時，能證明的只有「這台機器的 SQLite 支援 FTS5」
            None => mark(
                "?",
                "FTS5 雙索引",
                &format!(
                    "{no_db}；這份程式碼建得出來（{}/2），等你錄過再驗一次",
                    fts_of(&probe)
                ),
            ),
        }

        // 兩個字的中文詞現在有 bigram 索引（schema 3）。這裡不宣稱「有索引」，
        // 而是把回填的覆蓋率印出來——回填沒跑到的話，舊資料會安靜地叫不出來。
        match &db {
            Some(d) => {
                let (indexed, with_cjk) = d.bigram_coverage()?;
                let s = d.stats()?;
                let (sym, said) =
                    bigram_verdict(indexed, with_cjk, s.chunks, Emptiness::of(d, &s, beat)?);
                mark(sym, "兩個字的中文", &said);
            }
            None => mark("?", "兩個字的中文", no_db),
        }
        if let Some(db) = &db {
            // 兩個數字並排印出來、讓讀者自己比對，是把判斷丟給人。
            // 對不上的時候（別的版本開過這顆資料庫）程式照樣會去查它，
            // 那正是需要一個 ✗ 的時候。
            let have = db.schema_version()?;
            let want = sister_core::db::SCHEMA_VERSION;
            line(
                have == want,
                "Schema 版本",
                &if have == want {
                    format!("{have}")
                } else {
                    format!("{have}，但這支程式是 {want}——可能是別的版本開過這顆資料庫")
                },
            );
            let s = db.stats()?;
            let detail = format!(
                "{} 張畫面 · {} 段文字 · {}",
                s.frames,
                s.chunks,
                fmt::bytes(s.db_bytes + s.image_bytes)
            );
            // **同一頁上的四列，要問同一個問題同一次。** 上一版這裡是
            // `ever_recorded` + `ever_stored` 兩個布林，各列各自拼裝——於是
            // 「已記錄」那一列改對了、四行之下的「視窗焦點」還在講另一個故事。
            let empty = Emptiness::of(db, &s, beat)?;
            let (sym, said) = recorded_verdict(s.frames, s.chunks, empty, &detail);
            mark(sym, "已記錄", &said);

            let audit = db.crash_audit(beat)?;
            let (sym, said) = crash_verdict(&audit, empty);
            mark(sym, "零當機", &said);

            // 「她停了」後面永遠跟著同一個問題：什麼時候、為什麼。上面那一列
            // 只數得出「有幾段沒收尾」，答不了「上一段是怎麼結束的」——而這兩
            // 件事的下一步差很多：按了停止什麼都不用做，同意書被撤回的話她從
            // 現在起什麼都不會記。
            if let Some(last) = db.last_session()? {
                let (sym, said) = last_session_verdict(&last, &audit);
                mark(sym, "上一次錄製", &said);
            }

            // 上面那一列擋的是「一個字都沒有」。它擋不住的是**有一堆列、
            // 每一列都是空殼**——`COUNT(*)` 對這兩種故障的回答一模一樣。
            //
            // 這三個訊號（焦點、輸入節奏、文字座標）在 Phase 0 沒有任何讀者，
            // 它們是 Phase 1 之後才要用的原料。沒人讀不是問題，**沒人讀所以
            // 沒人驗**才是：真的壞掉的那一天，這裡是唯一會講話的地方。
            // 數字講的是**最後那一場**，不是這顆資料庫的一輩子。掃全表的
            // 那一版在一顆用過三個月的資料庫上永遠翻不成 ✗（見
            // `signal_audit` 的文件），而這一行要讓那個界線看得見——否則
            // 「12 列」讀起來像全部，而他上禮拜二壞掉的那一場藏在裡面。
            for a in db.signal_audit(beat)? {
                // 「沒有最後一場」在同一份報告的上面四行已經有答案了。
                //
                // `signal_audit` 的範圍是 `sessions` 的最後一列，而那張表現在
                // 會跟著它記下來的東西一起消失（`delete_empty_sessions`）。所以
                // 清空過的資料庫上，「零當機」那一列說「那幾場的紀錄已經不在
                // 了」，這三列說「還沒有任何一場」——**同一份報告，四行之隔，
                // 兩句互相打臉**。而這兩句正是 `recorded_verdict` /
                // `crash_verdict` 被拆出來要防的那個形狀，只是它們管不到這裡。
                let (sym, said) = signal_line(&a, empty);
                mark(sym, a.name, &said);
            }
        }

        println!("\n隱私");
        // 排在最前面，因為它壓過底下每一條：暫停的時候，那些規則生不生效
        // 都無所謂——她根本沒在看。
        //
        // 而且這一行是**唯一**會告訴使用者「你上禮拜按的暫停還開著」的地方。
        // 暫停不會自己過期（見 `sister_core::pause`），所以那條路很真實，而
        // 它的症狀是「所有數字都是 0」——最容易被讀成「程式壞了」。
        let paused = sister_core::pause::is_paused(data_dir).then(|| {
            sister_core::pause::paused_since(data_dir)
                .map(|ts| format!("從 {} 起", crate::fmt::timestamp(ts)))
                .unwrap_or_else(|| "不知道從什麼時候開始".to_string())
        });
        let (sym, said) = watching_verdict(paused, beat);
        mark(sym, "現在有沒有在看", &said);
        // 題庫是整個資料庫裡唯一一張存著**你自己打進去的字**的表，所以它得
        // 出現在這一頁上。doctor 的工作是把真相攤開，而一張存了東西、卻沒有
        // 任何地方提起的表，跟偷偷存著沒有差別。
        //
        // 開關和已經存下來的內容要分兩句講：關掉之後舊的仍然在。少了後半句，
        // 「我關掉了」會被讀成「那些也一起沒了」。
        let qlog = db
            .as_ref()
            .and_then(|d| d.query_log_stats().ok())
            .unwrap_or_default();
        // 「還沒問過任何問題」是一句**斷言**，而 `total == 0` 撐不起它：`forget`
        // 和保留期都會把題庫帶走（`sister queries` 那一頁已經把三種可能攤開
        // 了）。兩個 surface 讀的是同一顆資料庫，不可以一邊列出「可能被忘掉
        // 了」、一邊掛著 ✓ 說那件事沒發生過。
        let ever = db
            .as_ref()
            .map(|d| d.ever_recorded())
            .transpose()?
            .unwrap_or(false);
        let (sym, said) = query_log_verdict(db.is_some(), config.privacy.query_log, ever, &qlog);
        mark(
            sym,
            "你問過她什麼",
            &if db.is_some() {
                said
            } else {
                format!("{no_db}{said}")
            },
        );
        // 「9 條規則 ✓」是 THREAT_MODEL 明文禁止的那種寫法：規則的**數量**
        // 從來不是問題，規則**會不會命中**才是。這些規則比對的是前景 app
        // 名稱，所以讀不到名稱的時候它們一條都不生效——而數量照樣是 9。
        let (sym, note) = match &caps.focus_probe {
            Some((app, _)) if !app.is_empty() => ("✓", format!("，現在讀到的是 {app}")),
            Some(_) => ("?", "，但現在沒有前景視窗，這一刻測不出來".to_string()),
            None => (
                "✗",
                "（本平台讀不到前景 app，這些規則目前不生效）".to_string(),
            ),
        };
        mark(
            sym,
            "排除的 app",
            &format!("{} 條規則{note}", config.privacy.excluded_apps.len()),
        );
        // 規則數量不等於規則有效。沒有 URL 擷取能力時這些規則一條都不會跑，
        // 而使用者看到「16 條規則 ✓」只會更放心——那正是最糟的結果。
        //
        // 但「UIA 建得起來」也不夠格畫 ✓：那只證明了一個 COM 物件生得出來，
        // 沒有證明我們從位址列上讀得到任何東西（Firefox 的樹長得就不一樣）。
        // 這就是上一版 `✓ OCR 語言 zh-Hant-TW` 的錯法，只是換了個能力。
        // 所以 ✓ 只給「下面那一列真的讀到了網址」的情況。
        let demonstrated = caps.url_probe.as_ref().is_some_and(|(s, ..)| *s == "✓");
        let (sym, note) = match (caps.url, demonstrated) {
            (false, _) => ("✗", "（本平台無法讀取網址，這些規則目前不生效）"),
            (true, true) => ("✓", ""),
            // UIA 在，但這一刻沒能證明讀得到。可能只是前景不是瀏覽器。
            (true, false) => ("?", "（還沒驗到——見下面那一列）"),
        };
        mark(
            sym,
            "排除的網址",
            &format!("{} 條規則{note}", config.privacy.excluded_urls.len()),
        );
        // 規則寫錯的方式是安靜的：少擋了不會有任何症狀。這裡把「寫了也不會
        // 命中」的規則挑出來，因為使用者自己永遠不會發現。
        let suspicious = sister_core::config::suspicious_url_rules(&config.privacy.excluded_urls);
        if !suspicious.is_empty() {
            for (rule, why) in &suspicious {
                line(false, "  規則不會命中", &format!("{rule} — {why}"));
            }
        }
        // 規則數量與規則寫法都對，還有第三個問題：**網址到底讀不讀得到**。
        // 這一列是真的去問了現在的前景視窗，不是宣稱 UIA 建得起來。
        if let Some((sym, label, detail)) = &caps.url_probe {
            mark(sym, label, detail);
        }
        // 標題和 app 來自同一次 snapshot，但**失敗方式不一樣**：有些視窗
        // 讀得到 exe 名稱卻沒有標題。分開報，才不會讓 app 的 ✓ 幫標題背書。
        let (sym, note) = match &caps.focus_probe {
            Some((_, title)) if !title.is_empty() => (
                "✓",
                format!("，現在讀到的是「{}」", crate::fmt::one_line(title, 40)),
            ),
            Some(_) => ("?", "，但現在這個視窗沒有標題可比對".to_string()),
            None => (
                "✗",
                "（本平台讀不到視窗標題，這些規則目前不生效）".to_string(),
            ),
        };
        mark(
            sym,
            "排除的標題",
            &format!("{} 條規則{note}", config.privacy.excluded_titles.len()),
        );
        line(
            config.privacy.pause_on_screenshare,
            "螢幕分享時暫停",
            if config.privacy.pause_on_screenshare {
                "開啟"
            } else {
                "關閉（旁人的畫面會被錄到）"
            },
        );
        line(
            config.privacy.redact_clipboard_secrets,
            "剪貼簿秘密偵測",
            if config.privacy.redact_clipboard_secrets {
                "開啟"
            } else {
                "關閉（API key 會落地）"
            },
        );
        // 問同意書，不是問設定檔。設定檔說的是「我想留圖」，同意書說的是
        // 「可不可以」——上面那一區已經印過第三張沒簽了，這一行要是還印「是」，
        // 同一張報告就自己打自己。
        //
        // ✗/✓ 走 `keeps_images`（含 `capture.enabled`），句子以前走一個
        // 兩元 match（不含）。於是一台 `enabled = false` 的機器上印出來的是
        //
        //     ✗ 保留畫面檔     是
        //
        // 一行之內符號說不、字說是，而讀的人沒有辦法知道該信哪一邊。同一個
        // 問題要嘛一個答案，要嘛兩個都不要印。
        line(
            consent.keeps_images(config),
            "保留畫面檔",
            frames_kept_words(
                consent.allows_frames(),
                config.capture.enabled,
                config.capture.store_images,
            ),
        );

        // OCR 語言決定她讀不讀得懂你的螢幕，所以要把**實際挑中的**那個印出來，
        // 不是印設定檔裡的偏好清單——兩者不一致正是問題所在。
        println!("\n讀字");
        if !config.capture.ocr {
            line(false, "OCR", "已關閉（畫面會留下，但上面的字不會進資料庫）");
        } else if !caps.ocr_probed {
            // 這個平台沒有 OCR 後端可以問。以前這裡照樣走下面那條路，於是
            // 在 Linux 上印出「無：這台機器沒有安裝任何 OCR 語言」——那是
            // 一句關於 Windows 語言包的話，講給一台不裝 Windows 語言包的
            // 機器聽。開發機每跑一次 doctor 就看一次，久了就學會忽略它，
            // 而真的在 Windows 上少裝語言包的時候，長得一模一樣。
            mark(
                "?",
                "OCR 語言",
                &format!("問不到：{} 上沒有擷取後端", std::env::consts::OS),
            );
        } else {
            line(
                caps.ocr,
                "OCR 語言",
                &match &caps.ocr_language {
                    Some(tag) => format!("{tag}（實際使用）"),
                    None => "無：這台機器沒有安裝任何 OCR 語言".to_string(),
                },
            );
            line(
                !caps.ocr_available.is_empty(),
                "已安裝的語言",
                &if caps.ocr_available.is_empty() {
                    "（無）".to_string()
                } else {
                    caps.ocr_available.join("、")
                },
            );
            // 上面兩行講的都是「引擎建得起來」。下面這些是真的去讀了。
            // 這個分野是實測換來的：語言 ✓、錄了一分鐘、資料庫裡零個字。
            for (ok, label, detail) in &caps.ocr_probes {
                line(*ok, label, detail);
            }
        }

        println!("\n節奏");
        // 設定檔寫 50，程式跑 200。以前 doctor 只是不提這件事，於是使用者
        // 調了一個不會生效的旋鈕，而畫面上沒有任何地方看得出來。
        let asked = config.capture.min_interval_ms;
        let used = asked.max(crate::ops::MIN_TICK_MS);
        line(
            config.capture.enabled,
            "多久看一次螢幕",
            // 總開關關著的時候答案是「不看」，而不是「50 ms」。印出一個
            // 節奏等於回答一個沒有人問的問題，還答錯了——這一格問的是
            // 「她多久看一次」，不是「設定檔裡那個數字是多少」。
            &if !config.capture.enabled {
                format!("不看——capture.enabled 關著（設定檔裡寫的是 {used} ms）")
            } else if used == asked {
                format!("{used} ms")
            } else {
                format!(
                    "{used} ms（你設的是 {asked} ms，但下限是 {} ms）",
                    crate::ops::MIN_TICK_MS
                )
            },
        );
        if let Some(ok) = caps.input_hooks {
            line(
                ok,
                "輸入 hook",
                if ok {
                    "裝得上（doctor 剛剛真的裝了一次；只數次數，不看按了什麼）"
                } else {
                    "裝不上：打字節奏這一路訊號會是空的"
                },
            );
        }

        if !caps.broken_privacy.is_empty() {
            println!("\n⚠ 目前失效的隱私保護");
            for w in &caps.broken_privacy {
                println!("  ✗ {w}");
            }
        }

        // 「她會不會記錯」和「她到底有沒有記住」是兩回事，分開報。
        if !caps.degraded.is_empty() || !config.capture.enabled {
            println!("\n⚠ 看起來正常，但其實記不住東西");
            // 這一節以前只由 `Capabilities::silently_degraded` 餵，而那一支
            // 只看得到 OCR。於是 `capture.enabled = false` ——一個關掉之後
            // 每個 tick 直接回 `Tick::Disabled`、連螢幕都不會碰的旗標——
            // 在整份 doctor 裡一個字都沒出現過，那台機器拿到的是一張全綠的
            // 體檢報告。`record` 和 `replay` 都會對著它大聲喊，只有 doctor
            // 不會，而 doctor 正是 `stats` 叫人來看的那一頁。
            //
            // 它排在最前面：底下每一條「讀不讀得到字」都預設她會去看螢幕。
            if !config.capture.enabled {
                println!(
                    "  ✗ 設定檔的 capture.enabled = false：`sister record` 會啟動、會印出「開始記錄」，"
                );
                println!(
                    "     然後每一個 tick 直接跳過。沒有畫面、沒有字、沒有截圖——這份報告上其他"
                );
                println!("     每一個 ✓，講的都是一台不會開始的機器。");
            }
            for w in &caps.degraded {
                println!("  ✗ {w}");
            }
        }

        // 先講「她會存多少圖」，再講「存下來的留多久」。順序是刻意的：
        // 使用者遲早會點到一筆沒有圖的搜尋結果，而那不是壞掉——與其等他
        // 來問，不如在他按下 record 之前就講清楚。
        println!("\n畫面檔");
        // `store_images = false` 的時候，下面兩條設定一條都不會跑。照樣印
        // 「最快 5 秒一張 ✓」等於在回答一個沒有人問的問題——而且答錯了。
        //
        // `capture.enabled = false` 是同一件事再往外一層：連螢幕都不會碰，
        // 所以「多久存一張」的答案是「不存」，不是「5 秒」。這一段以前只
        // 問了內層那個旗標。
        if !config.capture.enabled {
            mark(
                "—",
                "一張都不存",
                "capture.enabled = false：她不會去看螢幕，所以沒有東西可以存",
            );
        } else if config.capture.store_images {
            line(
                true,
                "多久存一張",
                &format!(
                    "最快 {:.0} 秒一張（其餘只留字，搜尋不受影響）",
                    config.capture.image_min_interval_ms as f64 / 1000.0
                ),
            );
            line(
                true,
                "一天最多存",
                &if config.capture.max_image_mb_per_day == 0 {
                    "不設上限——磁碟要自己盯".to_string()
                } else {
                    format!(
                        "{} MB（用完就只留字，隔天歸零）",
                        config.capture.max_image_mb_per_day
                    )
                },
            );
        } else {
            mark(
                "—",
                "一張都不存",
                "store_images = false：只留字。下面的保留期只管得到文字",
            );
        }

        println!("\n保留期");
        // 這裡以前還有兩個 `0 =>` 的分支，講「0 天＝寫下去就刪」。它們現在
        // 到不了：`Config::load` 直接拒絕 0，理由是它在別的工具裡幾乎都代表
        // 「不限制」，而在這裡是相反的意思（見 `RetentionConfig::check`）。
        // 留著一個描述已經不存在的行為的分支，比沒有那個分支更糟。
        let (frames_days, text_days) = (config.retention.frames_days, config.retention.text_days);
        line(
            true,
            "畫面",
            &format!("{frames_days} 天（到期只刪圖，字留著）"),
        );
        line(
            true,
            "文字與事實",
            &format!("{text_days} 天（到期整列消失）"),
        );
        // 「畫面留得比文字久」是寫得出來但做不到的一組數字：文字到期的時候
        // 那一列整個消失，掛在上面的圖也跟著被刪掉。上面兩行各自都照著設定
        // 印，兩行都是真的——**只有把它們並排看才看得出來大的那個沒有生效**。
        // 這一段正是 doctor 存在的理由：把設定和實際做得到的事對一次。
        if frames_days > text_days {
            mark(
                "?",
                "這兩個數字打架",
                &format!(
                    "畫面寫 {frames_days} 天，但文字只留 {text_days} 天——\
                     文字到期時整列一起消失，圖也跟著走。實際上畫面只有 {text_days} 天。"
                ),
            );
        }
        // 這兩個數字以前是**純粹的宣稱**——設定檔裡寫著 30 天，而沒有任何
        // 一行程式碼會刪掉任何東西。同樣不宣稱、直接示範：現在就去問資料庫
        // 「這一刻有多少東西已經過期」。`?` 代表還沒有資料庫可以問。
        //
        // `prune_preview` 回六個計數器，這裡以前只看兩個。一段全部被排除規則
        // 擋掉（不寫 frame 列、只留下 system_events）而過了期的日子，兩個
        // 計數器都是 0 → doctor 說「沒有——都還在保留期內」，然後下一句
        // `sister prune` 印出「刪掉了 0 段文字、0 個事實、37 筆事件」。
        // 這一行的存在意義就是「不宣稱，直接示範」，所以它得看完整份。
        match db.as_ref().map(|d| {
            d.prune_preview(
                sister_core::now_ms(),
                &config.retention,
                Some(&prune::frames_dir(data_dir)),
            )
        }) {
            Some(Ok(r)) if r.is_empty() => line(true, "現在有多少已過期", "沒有——都還在保留期內"),
            Some(Ok(r)) => {
                let what = [
                    (r.images_deleted, "個畫面檔"),
                    (r.frames_deleted, "列畫面紀錄"),
                    (r.chunks_deleted, "段文字"),
                    (r.facts_deleted, "個事實"),
                    (r.events_deleted, "筆事件"),
                    (r.queries_deleted, "題你問過的話"),
                ]
                .iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, unit)| format!("{n} {unit}"))
                .collect::<Vec<_>>()
                .join("、");
                mark(
                    "?",
                    "現在有多少已過期",
                    &format!("{what}。跑 `sister prune` 讓它們消失"),
                )
            }
            Some(Err(e)) => line(false, "現在有多少已過期", &format!("問不出來：{e:#}")),
            None => mark("?", "現在有多少已過期", no_db),
        }
        Ok(())
    }

    #[cfg(test)]
    mod doctor_tests {
        use super::*;
        use sister_core::capabilities::{Report, UrlCapture};
        use sister_core::heartbeat::{Phase, Presence};

        /// 一台剛裝好、還沒錄過的機器上，doctor 要寫得進去。
        ///
        /// README quickstart 第一句就是「跑一次 doctor」，而在那之前設定頁只
        /// 答得出「還不知道」——一個正在填排除規則的新使用者，正是最需要知道
        /// 「這台機器讀不到網址」的那個人。護著證據不可以把這條路一起關掉。
        #[test]
        fn a_machine_with_nothing_to_lose_still_gets_its_report_written() {
            assert!(keep_capabilities(Presence::Stopped { at: None }, None).is_none());
            assert!(
                keep_capabilities(Presence::Stopped { at: None }, Some(&Report::default()))
                    .is_none()
            );
            // 只有開機探測、沒有歷史——一樣沒有東西可以弄丟。
            let probes_only = Report {
                at: 1,
                url: true,
                input_hook_failed: true,
                ..Default::default()
            };
            assert!(
                keep_capabilities(Presence::Stopped { at: None }, Some(&probes_only)).is_none()
            );
        }

        /// 她收工之後，那一場路上發生的事仍然只有那一場問得到。
        ///
        /// 這是使用者動線裡最壞的一條，也是上一版唯一漏掉的那一條：覺得怪怪的
        /// → 按停止 → 跑 doctor → 打開設定頁。上一版的閘門問的是
        /// `is_recording`，於是這裡回 false，doctor 蓋掉——他親手刪掉了自己要
        /// 找的東西。
        #[test]
        fn a_finished_session_still_owns_what_only_it_could_have_seen() {
            let gave_up = Report {
                url_capture: UrlCapture {
                    gave_up: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let k = keep_capabilities(Presence::Stopped { at: None }, Some(&gave_up))
                .expect("心跳停了不代表可以蓋");
            assert_eq!(k.whose, "上一場留下的報告說");
        }

        /// 開機那幾分鐘也不行，而且那幾則話不可以說成是她講的。
        ///
        /// `Booting` 的時候她一個字都還沒寫，讀到的是上一場留下的。說成「正在
        /// 錄的那個行程說」會讓他去停一個沒有在做那件事的行程。
        ///
        /// **這句話有一個它自己驗不到的前提**：開機那幾分鐘裡真的沒有人寫過
        /// 那個檔。而 alpha.38 為止是有的——`windows_record` 探測完能力就馬上
        /// 蓋一份上去，於是這一行說的「上一場留下的」指著一份幾秒前才被這個
        /// 行程寫掉、乾淨的、時戳很新的報告。那次的修法是把那個寫入挪到
        /// `boot.hand_off()` 之後（那裡有整段說明），不是改這句話。
        ///
        /// 這種前提放在測試裡驗不了，只能寫在這裡讓下一個人看到——那個寫入
        /// 一挪回去，這一行就又變成謊話，而沒有任何東西會紅。
        #[test]
        fn while_she_is_opening_the_database_the_warnings_are_last_sessions() {
            let k =
                keep_capabilities(Presence::Live(Phase::Booting), None).expect("開機中不可以蓋");
            assert_eq!(k.whose, "上一場留下的報告說");
            let k =
                keep_capabilities(Presence::Live(Phase::Recording), None).expect("正在錄不可以蓋");
            assert_eq!(k.whose, "正在錄的那個行程說");
            let k = keep_capabilities(
                Presence::Thinking {
                    at: 1,
                    until: 240_000,
                },
                None,
            )
            .expect("想最後一段也不可以蓋");
            assert_eq!(k.whose, "正在收尾的那個行程說");
        }

        /// 三種不蓋的理由，要是三句不一樣的話。
        ///
        /// 它們回答的是同一個問題——「為什麼設定頁上這一份沒有跟著更新」——而
        /// 使用者會拿那句話決定下一步：正在錄的那個要去按停止才問得到新的，
        /// 正在起來的再等幾分鐘就有，已經收工的那一份是歷史、不會再變了。三句
        /// 壓成一句，那三個下一步就沒有分別了。這是 `stop`、`forget`、`stats`
        /// 那幾處同一條規則的第四個現場。
        #[test]
        fn three_reasons_not_to_overwrite_are_three_different_sentences() {
            let evidence = Report {
                url_capture: UrlCapture {
                    gave_up: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let kept = [
                keep_capabilities(Presence::Live(Phase::Recording), None),
                keep_capabilities(Presence::Live(Phase::Booting), None),
                keep_capabilities(
                    Presence::Thinking {
                        at: 1,
                        until: 240_000,
                    },
                    None,
                ),
                keep_capabilities(Presence::Stopped { at: None }, Some(&evidence)),
            ];
            let whys: Vec<&str> = kept
                .iter()
                .map(|k| k.as_ref().expect("這四種都不可以蓋").why)
                .collect();
            assert!(whys.iter().all(|w| !w.is_empty()), "理由不可以是空的");
            for (i, a) in whys.iter().enumerate() {
                for b in &whys[i + 1..] {
                    assert_ne!(a, b, "三種狀態要三句話，這兩個一樣：{a}");
                }
            }
        }

        /// 設定檔壞掉的時候，doctor 還是要把整份檢查跑完。
        ///
        /// 以前 `main` 在門口就 `load_config(..)?`，所以一個 TOML 語法錯會讓
        /// `sister doctor` 只吐一行解析錯誤然後結束——而他打開 doctor 正是因為
        /// 有東西壞了。OCR 讀不讀得到字、抓不抓得到網址、同意書簽了沒，這些跟
        /// 那份設定檔一點關係都沒有，卻一起消失了。
        ///
        /// 釘的是「回 Ok」而不是輸出的字：輸出會一直改，而這條要擋的是「它
        /// 早退了」。
        #[test]
        fn a_broken_config_does_not_shut_the_door_on_the_tool_you_opened_because_things_are_broken()
        {
            let dir = crate::ops::tmp::Tmp::new("doctor-broken-config");
            let broken = Err(anyhow::anyhow!("TOML parse error at line 1"));
            run(&dir.0, broken, Some(dir.0.join("config.toml")))
                .expect("設定檔壞掉不可以讓整份環境檢查停在門口");
        }

        /// 手捏一份 [`CrashAudit`]，四個數字。
        ///
        /// 全 0 的那一份就是「分母沒了」的那一顆：計數器也是 0（升級之前列就
        /// 已經被掃光了，回填只數得到還在的列），所以句子只剩 [`Emptiness`]
        /// 答得出來。
        fn audit(
            started: i64,
            ended: i64,
            rows: i64,
            rows_unfinished: i64,
        ) -> sister_core::db::CrashAudit {
            sister_core::db::CrashAudit {
                started,
                ended,
                rows,
                rows_unfinished,
                last_crash: None,
                // 預設沒有人在錄。要驗「扣掉了要說扣掉了」的那幾格自己蓋
                // `live: true` 上去——而且上面那四個數字**要先扣好**，因為
                // 真的那一支就是這樣交出來的。
                live: false,
                // 「有沒有人佔著這個資料目錄」是另一題，預設沒有。要驗
                // 「她正在開機」那幾格的自己蓋 `Presence::Live(Phase::Booting)` 上去。
                beat: Presence::Stopped { at: None },
                floor: false,
            }
        }

        /// **一顆全是 0 的資料庫有兩種，而 doctor 這兩列以前只認得其中一種。**
        ///
        /// `sister forget --last 24h --yes` 跑完之後，`frames`、`chunks`、
        /// `sessions` 全是 0——和一顆剛裝好、從來沒錄過的資料庫逐字相同。舊版
        /// 這兩列於是說「（還沒有任何內容）」和「還沒錄過」，對一個五分鐘前
        /// 才親手刪掉一整天的人。第二列還是 #52 打進來的回歸：在 `sessions`
        /// 那張表開始跟著保留期一起被清掉之前，`crash_audit` 的 0 真的只代表
        /// 沒錄過。
        ///
        /// 釘的是**分岔**本身：同樣三個數字、`ever` 一翻，兩列都得換句話講。
        #[test]
        fn an_erased_database_and_a_brand_new_one_do_not_get_the_same_two_lines() {
            let d = "0 張畫面 · 0 段文字 · 176.0 KB";

            let (never_sym, never) = recorded_verdict(0, 0, Emptiness::Fresh, d);
            let (erased_sym, erased) = recorded_verdict(0, 0, Emptiness::Erased, d);
            assert_ne!(
                never, erased,
                "同樣是 0，刪過的和沒錄過的不可以拿到同一句話"
            );
            assert!(
                erased.contains("forget") || erased.contains("保留期"),
                "他刪掉的是證據不是歷史，這一句要說得出東西去哪了：{erased}"
            );
            assert!(
                !never.contains("forget") && !never.contains("保留期"),
                "從來沒錄過的機器上不可以暗示他刪過東西：{never}"
            );
            // 兩邊都還是「不知道」而不是打勾——空的就是空的。
            assert_eq!((never_sym, erased_sym), ("?", "?"));

            let (never_sym, never) = crash_verdict(&audit(0, 0, 0, 0), Emptiness::Fresh);
            let (erased_sym, erased) = crash_verdict(&audit(0, 0, 0, 0), Emptiness::Erased);
            assert_ne!(never, erased, "「還沒錄過」對清空過的資料庫是假話");
            assert!(
                never.contains("還沒錄過"),
                "真的沒錄過的時候還是要講得出來：{never}"
            );
            assert!(
                !erased.contains("還沒錄過"),
                "他錄過，只是那幾場的紀錄被帶走了：{erased}"
            );
            assert_eq!((never_sym, erased_sym), ("?", "?"), "兩邊都算不出當機率");

            // **分母是 0 的第三種：她跑過，但一個字都沒存到。**
            //
            // 一場什麼都沒存到的錄製收工時會刪掉自己那一列，所以這裡也是
            // `(0, _) && ever`——而「紀錄已經不在了」在這台機器上是指控。
            // 少了這一條，把那一支拿掉只有 CI 那顆 fixture 抓得到。
            let (barren_sym, barren) = crash_verdict(&audit(0, 0, 0, 0), Emptiness::Barren);
            assert!(
                !barren.contains("不在了") && !barren.contains("forget"),
                "他一次都沒刪過東西：{barren}"
            );
            assert!(
                barren.contains("capture.enabled"),
                "真正的下一步要講出來：{barren}"
            );
            assert_ne!(barren, erased, "「被拿走了」和「沒進來過」不是同一句話");
            assert_eq!(barren_sym, "?", "照樣算不出當機率");

            // **第四種：她此刻正在錄，還沒有東西落地。** `Barren` 那句要他
            // 去改設定，而她三秒前才被開起來——那台機器上沒有東西需要改。
            let (live_sym, live) = crash_verdict(&audit(0, 0, 0, 0), Emptiness::Live);
            assert!(
                !live.contains("不在了") && !live.contains("forget"),
                "她剛開始錄，沒有東西被刪過：{live}"
            );
            assert!(
                !live.contains("capture.enabled"),
                "不要把一個剛按下開始記錄的人送去改一個沒問題的設定：{live}"
            );
            assert_ne!(
                live, barren,
                "「跑完了什麼都沒存到」和「才剛開始」不是同一句話"
            );
            assert_eq!(live_sym, "?");

            // 而 `empty` 只准在分母是 0 的時候說話。錄過的資料庫上照樣要數得
            // 出來，不可以整列被那個判斷蓋掉。
            for e in Emptiness::ALL {
                assert_eq!(crash_verdict(&audit(7, 7, 7, 0), e).0, "✓");
            }
            for e in Emptiness::ALL {
                assert_eq!(recorded_verdict(9, 4, e, d), ("✓", d.to_string()));
                assert_eq!(recorded_verdict(9, 0, e, d).0, "✗");
            }
        }

        /// **最該被算進去的那一種當機，剛好就是會把自己的證據刪掉的那一種。**
        ///
        /// 開起來、還沒讀到第一張畫面就死掉——那一場沒有內容，於是下一次
        /// `prune` 連紀錄一起刪掉（那是 #52 要的行為，那一列是「他那天下午在
        /// 電腦前」的證明）。分子和分母同時少一，於是**她死得越早，這一格讀
        /// 起來越乾淨**，一台卡在開機當機迴圈裡的機器會收斂到 ✓。實測過的原始
        /// 數字（六場：一場正常＋五場開機即死）：掃之前「6 段裡有 5 段」，掃
        /// 之後「2 段裡有 1 段」。
        ///
        /// 這裡釘的是句子。數字撐不撐得過那一刀，`sister-core` 那邊的
        /// `a_crash_that_stored_nothing_still_counts_after_its_row_is_swept`
        /// 拿真的 `prune` 釘。
        #[test]
        fn the_crashes_that_left_no_record_are_still_in_the_sentence() {
            // 掃過之後：計數器記得六場、五場沒回來，而列只剩兩條。
            let (sym, said) = crash_verdict(&audit(6, 1, 2, 1), Emptiness::Live);
            assert_eq!(sym, "✗", "五次當機不可以畫成一個問號");
            assert!(said.contains('6') && said.contains('5'), "數字要對：{said}");
            assert!(
                !said.contains("2 段錄製裡"),
                "分母不可以是活下來的那幾場：{said}"
            );
            // 報得出時間的只有還留著紀錄的那幾場，而句子要說得出這個界線——
            // 「最後一次 X」讀起來像五次當機的最後一次，實際上是**還看得到的
            // 那一次**的最後一次。
            let with_time = crash_verdict(
                &sister_core::db::CrashAudit {
                    last_crash: Some(1_000),
                    ..audit(6, 1, 2, 1)
                },
                Emptiness::Live,
            )
            .1;
            assert!(
                with_time.contains("還留著紀錄的最後一次"),
                "四場沒有時間可以報，那就不可以把一個時間講成全部：{with_time}"
            );
            assert!(
                with_time.contains('4'),
                "少掉的那幾場要數得出來：{with_time}"
            );

            // 一場紀錄都沒留下的極端：她開機就死了五次，全被掃掉。
            let none_left = crash_verdict(&audit(5, 0, 0, 0), Emptiness::Barren).1;
            assert!(
                none_left.contains("連紀錄都沒留下"),
                "沒有時間就說沒有時間，不要留一段空白：{none_left}"
            );
            assert!(
                !none_left.contains("capture.enabled"),
                "分母沒了的那句話不適用——這裡數得出來：{none_left}"
            );

            // **正在錄的那一場不是一次當機**，而那個扣除發生在
            // `Db::crash_audit` 裡——這一支收到的每個數字都已經扣好了。所以這
            // 幾格驗的是剩下的那半件事：**扣掉了就要說扣掉了**。不說的話，開
            // 著四場的機器只看得到 3，而他數得出來自己開過幾次。
            //
            // 同一顆磁碟、同一組列，差別只在有沒有心跳：
            let live = crash_verdict(
                &sister_core::db::CrashAudit {
                    live: true,
                    ..audit(3, 3, 3, 0)
                },
                Emptiness::Live,
            );
            assert_eq!(live.0, "✓", "唯一沒收尾的那一場正開著：{}", live.1);
            let dead = crash_verdict(&audit(4, 3, 4, 1), Emptiness::Live);
            assert_eq!(dead.0, "✗", "沒有人佔著，那就是當掉了：{}", dead.1);
            assert!(
                live.1.contains("沒有算進去"),
                "少報的那一場要交代，不然 4 段錄製印出 3：{}",
                live.1
            );
            assert!(
                !dead.1.contains("沒有算進去"),
                "沒有東西被扣掉的時候多這一句，是替一個沒有的問題道歉：{}",
                dead.1
            );
            // ✗ 那一條上一版也漏過同一句：`crashed` 扣了，分母沒扣。
            assert!(
                crash_verdict(
                    &sister_core::db::CrashAudit {
                        live: true,
                        ..audit(4, 2, 4, 2)
                    },
                    Emptiness::Live
                )
                .1
                .contains("沒有算進去"),
                "扣掉一場就要說扣掉了，不然那個數字對不起來"
            );

            // **升上來的那一顆數不到升級之前被清掉的那幾場**，所以它不可以說
            // 「全部」。這一格自己就是這批 bug 的形狀：句子沒錯，範圍錯了。
            let floored = |started, ended| {
                crash_verdict(
                    &sister_core::db::CrashAudit {
                        floor: true,
                        ..audit(started, ended, started, started - ended)
                    },
                    Emptiness::Live,
                )
            };
            let clean = floored(6, 6);
            assert_eq!(clean.0, "✓");
            assert!(
                clean.1.contains("升上來那天"),
                "看不見的那幾場裡有沒有當機，它不知道：{}",
                clean.1
            );
            // **兩句補充黏在一起的時候，順序自己會說話。** 「這裡的數字是升上
            // 來那天數到的」如果排在「現在正在錄的那一場沒有算進去」後面，最近
            // 的先行詞就變成一句根本沒有數字的話。兩句各自都是真的。
            for (s, e) in [(6, 6), (6, 4)] {
                let both = crash_verdict(
                    &sister_core::db::CrashAudit {
                        floor: true,
                        live: true,
                        ..audit(s, e, s, s - e)
                    },
                    Emptiness::Live,
                )
                .1;
                let (scope_at, live_at) = (
                    both.find("升上來那天").expect(&both),
                    both.find("現在正在錄").expect(&both),
                );
                assert!(
                    scope_at < live_at,
                    "範圍聲明要黏著數字，不是黏著一句沒有數字的話：{both}"
                );
            }
            assert!(
                floored(6, 4).1.contains("升上來那天"),
                "✗ 那邊少報的是當機數，同一句話一樣要補"
            );
            // 而全新的那一顆不准講這句——它的數字是精確的，多一句範圍聲明
            // 就是替一個沒有的問題道歉。
            for live in [true, false] {
                for (s, e) in [(6, 6), (6, 4)] {
                    assert!(
                        !crash_verdict(
                            &sister_core::db::CrashAudit {
                                live,
                                ..audit(s, e, s, s - e)
                            },
                            Emptiness::Live
                        )
                        .1
                        .contains("升上來那天")
                    );
                }
            }
        }

        /// **她現在還在跑的時候，「上一次」不是一個近似值。**
        ///
        /// 兩個條件同時成立的那一格：有幾場當機連紀錄都沒留下（所以這一列
        /// 平常要聲明「這個時間不一定是最後一次」），而她此刻正在錄（所以手
        /// 上這一列就是最後一次，那句聲明是假的）。
        ///
        /// CI 那兩顆 fixture 各站一半——`ci-live` 造得出心跳卻不看這一列，
        /// `ci-crashloop` 看這一列卻從不寫心跳，交叉的那一格沒有人站著。這
        /// 條測試站那一格。
        #[test]
        fn while_she_is_still_running_the_last_recording_is_not_an_estimate() {
            let running = sister_core::db::LastSession {
                started_at: 1_700_000_000_000,
                ended_at: None,
                reason: None,
                app_version: "test".into(),
                events_left: 3,
            };
            // 六場開過、一場收好尾、只剩一列還看得到——四場當機連紀錄都沒
            // 留下。加上正在錄的那一場（已經從每個數字裡扣掉了）。
            let live = sister_core::db::CrashAudit {
                live: true,
                ..audit(6, 1, 1, 0)
            };
            let (sym, said) = last_session_verdict(&running, &live);
            assert_eq!(sym, "?");
            assert!(said.contains("她現在還在跑"), "{said}");
            assert!(
                !said.contains("不一定是最後一次"),
                "那一列就在手上，替一個看得見的事實道歉：{said}"
            );

            // 同一顆資料庫、同一列，心跳沒了：她當掉了，而這個時間確實只是
            // 「還留著紀錄的最後一場」。
            let dead = audit(7, 1, 2, 1);
            let (sym, said) = last_session_verdict(&running, &dead);
            assert_eq!(sym, "?");
            assert!(said.contains("她當掉了"), "{said}");
            assert!(
                said.contains("不一定是最後一次") && said.contains('5'),
                "五場沒留下紀錄，這個時間就不是最後一次：{said}"
            );

            // 收好尾的那一列不受影響——`live` 兩邊都不准讓它變成「還在跑」。
            let ended = sister_core::db::LastSession {
                ended_at: Some(1_700_000_060_000),
                reason: Some("user_stop".into()),
                ..running.clone()
            };
            for a in [&live, &dead] {
                let (sym, said) = last_session_verdict(&ended, a);
                assert_eq!(sym, "✓", "{said}");
                assert!(
                    !said.contains("還在跑") && !said.contains("當掉了"),
                    "{said}"
                );
            }
        }

        /// **開機那幾分鐘，「有人佔著」和「她的列在不在」是相反的兩件事。**
        ///
        /// `BootBeat` 一起來就開始寫心跳，然後 `Db::open` 在一顆大的資料庫上
        /// 可以跑幾分鐘——那段時間裡心跳說「有」、而 `sessions` 最新那一列還
        /// 是上一次當機留下來的殼。上一版只有一個布林，於是這一格印出「她現
        /// 在還在跑」，指著三天前那一場當機的開始時間。
        ///
        /// 兩個布林湊不出三種答案，所以帶回來的是 [`Phase`] 本人。
        ///
        /// [`Phase`]: sister_core::heartbeat::Phase
        #[test]
        fn while_she_is_booting_the_crashed_row_is_not_her() {
            use sister_core::heartbeat::Phase;
            let crashed = sister_core::db::LastSession {
                started_at: 1_700_000_000_000,
                ended_at: None,
                reason: None,
                app_version: "test".into(),
                events_left: 3,
            };
            // 開機中：心跳在（`Booting`），但她那一列還沒 INSERT，所以
            // `crash_audit` 一個數字都沒扣——`live` 是 false。
            let booting = sister_core::db::CrashAudit {
                beat: Presence::Live(Phase::Booting),
                ..audit(3, 2, 3, 1)
            };
            let (sym, said) = last_session_verdict(&crashed, &booting);
            assert_eq!(sym, "?");
            assert!(
                said.contains("她當掉了"),
                "手上這一列是上一次當機的殼，不是正在起來的那一個：{said}"
            );
            assert!(
                !said.contains("她現在還在跑"),
                "「有人佔著資料目錄」不等於「這一列是他的」：{said}"
            );
            assert!(
                said.contains("正在起來"),
                "而那個正在起來的 recorder 看得到，不要說成「沒有任何 recorder」：{said}"
            );
            assert!(
                !said.contains("沒有任何 recorder"),
                "上一版兩句話都印過，這一句在開機中是假的：{said}"
            );

            // 三種心跳三句話，兩兩不同。任何兩種撞在一起就是又有一組不同的
            // 情況被印成同一行——這批 bug 的形狀。
            let says = |a: sister_core::db::CrashAudit| last_session_verdict(&crashed, &a).1;
            let thinking = says(sister_core::db::CrashAudit {
                live: true,
                beat: Presence::Thinking {
                    at: 1,
                    until: 240_000,
                },
                ..audit(3, 2, 3, 1)
            });
            assert!(
                thinking.contains("想最後一段"),
                "按下停止之後那兩分鐘不是當機：{thinking}"
            );
            assert!(!thinking.contains("當掉了"), "{thinking}");
            assert!(
                !thinking.contains("沒有任何 recorder"),
                "行程還在：{thinking}"
            );
            let three = [
                says(sister_core::db::CrashAudit {
                    live: true,
                    beat: Presence::Live(Phase::Recording),
                    ..audit(3, 2, 3, 1)
                }),
                says(booting),
                says(sister_core::db::CrashAudit {
                    beat: Presence::Live(Phase::Recording),
                    ..audit(3, 2, 3, 1)
                }),
                says(audit(3, 2, 3, 1)),
                thinking,
            ];
            for (i, a) in three.iter().enumerate() {
                for b in &three[i + 1..] {
                    assert_ne!(a, b, "五種情況要有五句話");
                }
            }
        }

        /// 「現在有沒有在看」有四種答案，而它們兩兩不同。
        ///
        /// 這一條和 [`watching_verdict`] 那段註解是一組：那支函式被拆出來不是
        /// 為了好看，是為了讓「在這裡再讀一次心跳」寫不出來（它手上沒有路
        /// 徑）。拆出來之後順手把四種答案釘住——`Booting` 是這一版才有的第四
        /// 種，而它以前是被 `Recording` 那一句吃掉的。
        #[test]
        fn there_are_four_answers_to_whether_she_is_watching_right_now() {
            use sister_core::heartbeat::Phase;
            let all = [
                watching_verdict(
                    Some("從 08-20 03:00 起".into()),
                    Presence::Stopped { at: None },
                ),
                // 暫停壓過心跳：她佔著這個目錄，可是什麼都不會被記錄。少了這
                // 一格，一個按著暫停的人會看到「有一個 sister record 正在跑」
                // 然後以為自己被記著。
                watching_verdict(
                    Some("從 08-20 03:00 起".into()),
                    Presence::Live(Phase::Recording),
                ),
                watching_verdict(None, Presence::Live(Phase::Recording)),
                watching_verdict(None, Presence::Live(Phase::Booting)),
                watching_verdict(
                    None,
                    Presence::Thinking {
                        at: 1,
                        until: 240_000,
                    },
                ),
                watching_verdict(None, Presence::Stopped { at: None }),
            ];
            assert_eq!(all[0], all[1], "暫停中就是暫停中，心跳不改變這一句");
            assert_eq!(all[0].0, "⏸");
            assert_eq!(all[2].0, "✓");
            assert_eq!(all[3].0, "…", "「正在起來」不是 ✓——她一個字都還沒記");
            assert_eq!(all[4].0, "…", "想最後一段不是 ✓——她不抓畫面了");
            assert_eq!(all[5].0, "?", "沒人在錄是正常的，不可以畫成失敗");
            assert!(
                all[4].1.contains("想最後一段"),
                "想最後一段要講出來：{}",
                all[4].1
            );
            assert!(
                !all[4].1.contains("沒有任何 sister record"),
                "行程還在：{}",
                all[4].1
            );
            let five = [&all[0], &all[2], &all[3], &all[4], &all[5]];
            for (i, a) in five.iter().enumerate() {
                for b in &five[i + 1..] {
                    assert_ne!(a.1, b.1, "五種處境要有五句話");
                }
            }
            assert!(
                !all[3].1.contains("正在跑"),
                "「正在起來」印成「正在跑」，他就會照著那句話去做一件想被記住的事：{}",
                all[3].1
            );
        }

        /// **同一份報告的四列，在開機那幾分鐘要說同一件事。**
        ///
        /// 上一版 `doctor` 把心跳壓成 `occupied = beat.is_some()` 再發給
        /// [`Emptiness::of`]，於是開機中的機器印出來的是：
        ///
        /// ```text
        /// ? 已記錄     0 張畫面 · 0 段文字——她此刻正在錄，到現在還沒有一列落地
        /// ? 兩個字的中文 一段字都沒有……——她此刻正在錄，還沒有東西落地
        /// ? 零當機     她此刻正在錄，還沒有東西落地，所以還沒有一場算得出來
        /// ? 上一次錄製 …… 她當掉了。現在有一個 sister record 正在起來
        /// ```
        ///
        /// 前三列說她在錄，第四列說她還沒開始——四行之內自相矛盾。`crashed`
        /// 那一列早就收 [`Phase`] 本人所以是對的，另外三列收的是那個被壓扁的
        /// 布林。**一個布林湊不出三種答案，而少掉的那一種永遠是「正在來、還
        /// 沒好」。**
        ///
        /// 這一條把那三列一起釘住：`Booting` 底下沒有任何一列可以宣稱她正在
        /// 錄，也沒有任何一列可以宣稱有東西被刪過。
        ///
        /// [`Phase`]: sister_core::heartbeat::Phase
        #[test]
        fn while_she_is_booting_no_row_may_say_she_is_recording() {
            use sister_core::db::{SignalAudit, SignalVerdict};
            let d = "0 張畫面 · 0 段文字 · 168.0 KB";
            let signal = SignalAudit {
                name: "視窗焦點",
                rows: 0,
                populated: 0,
                populated_label: "列知道自己是哪個 app",
                verdict: SignalVerdict::TooEarly,
                note: "",
                scope_started_at: None,
                scope_is_live: false,
            };

            // doctor 上每一列會拿 `Emptiness` 去講話的地方，一個都不能漏。
            let rows = [
                recorded_verdict(0, 0, Emptiness::Booting, d),
                crash_verdict(
                    &sister_core::db::CrashAudit {
                        beat: Presence::Live(sister_core::heartbeat::Phase::Booting),
                        ..audit(0, 0, 0, 0)
                    },
                    Emptiness::Booting,
                ),
                bigram_verdict(0, 0, 0, Emptiness::Booting),
                signal_line(&signal, Emptiness::Booting),
            ];
            for (_, said) in &rows {
                assert!(
                    !said.contains("正在錄"),
                    "她還在開資料庫，第一拍都還沒跑：{said}"
                );
                assert!(
                    said.contains("正在起來"),
                    "而那個 recorder 看得到，說得出他在等什麼：{said}"
                );
                for lie in ["forget", "保留期", "不在了", "capture.enabled"] {
                    assert!(
                        !said.contains(lie),
                        "沒有人刪過東西，也沒有設定需要改——他要的是再等一下：{said}"
                    );
                }
            }

            // 而 `Live` 那一組四列全部相反：她真的在錄。少了這半邊，把上面四
            // 句都改成「正在起來」也是綠的——而那會在她真的在錄的時候騙他。
            let live = [
                recorded_verdict(0, 0, Emptiness::Live, d),
                crash_verdict(
                    &sister_core::db::CrashAudit {
                        live: true,
                        beat: Presence::Live(sister_core::heartbeat::Phase::Recording),
                        ..audit(3, 2, 3, 1)
                    },
                    Emptiness::Live,
                ),
                bigram_verdict(0, 0, 0, Emptiness::Live),
                signal_line(&signal, Emptiness::Live),
            ];
            for (i, (_, said)) in live.iter().enumerate() {
                assert!(said.contains("正在錄"), "她真的在錄：{said}");
                assert_ne!(
                    said, &rows[i].1,
                    "「正在錄」和「正在起來」是相反的兩件事，不可以共用一句話"
                );
            }
        }

        /// **她正在錄的那一場是第一場——而扣掉之後的 0 被讀成「分母沒了」。**
        ///
        /// `crash_audit` 把正在錄的那一場從 `started` 扣掉，所以一台全新機器
        /// 在她第一次錄製期間的 `started` 是 0。那個 0 走進底下那段
        /// `Emptiness` 判斷，而畫面已經落地（`Live` 走不到，`Erased` 才是它讀
        /// 到的），於是一台從來沒刪過東西的機器被告知「那幾場的紀錄已經不在
        /// 了（`forget` 或保留期）」。
        ///
        /// **扣掉一個數字就是替那個數字的 0 造一個新的意思。** 第十次之後每一
        /// 次都是這樣來的。
        #[test]
        fn her_first_recording_is_not_a_database_someone_emptied() {
            // **走 `Emptiness::ALL`，不要寫陣列字面量。** 上一版寫死五個，
            // 於是 `Booting` 加進來的那天這條測試對它一個字都沒問——而
            // 「她正在錄第一場」這一格本來就該蓋過底下每一種 `Emptiness`。
            for empty in Emptiness::ALL {
                let (sym, said) = crash_verdict(
                    &sister_core::db::CrashAudit {
                        live: true,
                        beat: Presence::Live(sister_core::heartbeat::Phase::Recording),
                        ..audit(0, 0, 0, 0)
                    },
                    empty,
                );
                assert_eq!(sym, "✓", "在她之前沒有別的，那是真的零當機：{said}");
                assert!(
                    !said.contains("forget") && !said.contains("保留期"),
                    "沒有人刪過任何東西：{said}"
                );
                assert!(
                    !said.contains("算不出來"),
                    "算得出來——扣掉的那一場是她自己：{said}"
                );
                assert!(said.contains("第一場"), "{said}");
            }

            // 但升上來的那一顆真的答不出來：回填只數得到還在的列，升上來那天
            // 一列都不剩的話，「在她之前沒有別的」是猜的。
            let (sym, said) = crash_verdict(
                &sister_core::db::CrashAudit {
                    live: true,
                    beat: Presence::Live(sister_core::heartbeat::Phase::Recording),
                    floor: true,
                    ..audit(0, 0, 0, 0)
                },
                Emptiness::Erased,
            );
            assert_eq!(sym, "?", "{said}");
            assert!(
                said.contains("升上來那天") && said.contains("算不出來"),
                "{said}"
            );

            // 沒有人在錄的那一顆不准走這條捷徑——它的 0 就是舊的那四種。
            let (sym, said) = crash_verdict(&audit(0, 0, 0, 0), Emptiness::Erased);
            assert_eq!(sym, "?", "{said}");
            assert!(!said.contains("第一場"), "{said}");

            // 而「收過一次尾」和「在這之前沒有錄過」不可能同時是真的。真的資
            // 料庫走不到這個組合，手改過的可以——那顆不准被告知她正在錄人生第
            // 一場。
            let (sym, said) = crash_verdict(
                &sister_core::db::CrashAudit {
                    live: true,
                    beat: Presence::Live(sister_core::heartbeat::Phase::Recording),
                    ..audit(0, 1, 0, 0)
                },
                Emptiness::Erased,
            );
            assert_eq!(sym, "?", "{said}");
            assert!(
                !said.contains("第一場"),
                "有一場收過尾，這就不是第一場：{said}"
            );
        }

        /// **第三種 0：她錄了，而那段時間被規則整段擋掉了。**
        ///
        /// 這一條守的是這批修改**自己犯過**的錯。第一版把「畫面 0、文字 0」
        /// 直接當成「被 `forget` 刪光了」，於是一場全程踏在 keepassxc 上的錄製
        /// 拿到的是「錄過，但現在一列都不剩（`forget` 或保留期）」——而那顆
        /// 資料庫裡有工作階段、有事件、有一整段排除稽核，一個位元組都沒被刪。
        ///
        /// 指錯方向的代價很具體：真正該做的下一步是去看那幾條規則，而那句話
        /// 告訴他東西已經沒了、沒什麼好看的。
        #[test]
        fn a_recording_the_rules_ate_is_not_a_recording_he_deleted() {
            let d = "0 張畫面 · 0 段文字 · 168.0 KB";
            let said = |e| recorded_verdict(0, 0, e, d).1;

            let blocked = said(Emptiness::Blocked);
            assert!(
                !blocked.contains("forget") && !blocked.contains("保留期"),
                "沒有人刪過任何東西，規則擋掉的不可以說成他刪掉的：{blocked}"
            );
            assert!(
                blocked.contains("規則") || blocked.contains("暫停"),
                "而該看的地方要指出來——他下一步就是去改那幾條：{blocked}"
            );
            // 三種各自一句，兩兩不同。任何兩種撞在一起，就是又有一組不同的
            // 情況被印成同一行。
            // **走 `Emptiness::ALL`，不要寫陣列字面量。** 字面量不會因為 enum
            // 多一種變體而編不過，而上一版 `Barren` 加進來的時候，這裡還停在
            // 三種——守著「不可以兩種處境共用一句話」的測試，剛好漏掉的就是那
            // 次新加的那一種。
            let all = Emptiness::ALL.map(said);
            for (i, a) in all.iter().enumerate() {
                for (j, b) in all.iter().enumerate().skip(i + 1) {
                    assert_ne!(
                        a,
                        b,
                        "{:?} 和 {:?} 的下一步不一樣，不可以共用一句話",
                        Emptiness::ALL[i],
                        Emptiness::ALL[j]
                    );
                }
            }
        }

        /// **同一份報告，四行之隔，兩句互相打臉。**
        ///
        /// 「零當機」修好之後，清空過的資料庫上這份 doctor 是這樣的：
        ///
        /// ```text
        ///   ? 零當機     那幾場的紀錄已經不在了（`forget` 或保留期），現在算不出來
        ///   ? 視窗焦點   還沒有任何一場裡沒有這種資料
        /// ```
        ///
        /// 兩列讀的是同一張 `sessions` 表的同一個 0。修一半比不修更難發現，
        /// 因為上面那一列看起來已經處理過了。
        ///
        /// **alpha.34 之後上面那一列通常不長這樣了**：計數器（`sessions_started`
        /// / `sessions_ended`）撐得過 `forget`，所以清空之後那一格照樣答得出
        /// 「1 段錄製全部正常收尾」。上面那一句還在的路是**升級之前就已經被清
        /// 空**的那一顆——回填只數得到還在的列，所以它的計數器是 0，然後就掉回
        /// 這裡。這條測試餵的 `audit(0, 0, 0, 0)` 就是那一顆。
        #[test]
        fn the_signal_rows_do_not_contradict_the_crash_row_four_lines_above_them() {
            use sister_core::db::{SignalAudit, SignalVerdict};
            let erased = SignalAudit {
                name: "視窗焦點",
                rows: 0,
                populated: 0,
                populated_label: "列知道自己是哪個 app",
                verdict: SignalVerdict::TooEarly,
                note: "",
                scope_started_at: None,
                scope_is_live: false,
            };

            let (_, said) = signal_line(&erased, Emptiness::Erased);
            let (_, crash) = crash_verdict(&audit(0, 0, 0, 0), Emptiness::Erased);
            assert!(
                !said.contains("還沒有任何一場"),
                "上面那一列剛說完紀錄被帶走了，這一列不可以說她沒錄過：{said}"
            );
            assert!(
                said.contains("不在了"),
                "說得出「算不出來」就說得出為什麼算不出來：{said}"
            );
            // 兩列講的是同一件事，用詞不必一樣，但不可以互相否定。
            assert!(
                crash.contains("不在了") && said.contains("不在了"),
                "同一張表的同一個 0，兩列要指向同一個解釋：\n  {crash}\n  {said}"
            );

            // **而這一條上一版只釘住了 `Erased` 那一格。** `capture.enabled =
            // false` 那台機器走到的是 `Barren`，於是一行之上寫「那幾場沒有留
            // 下紀錄」、這三行寫「那幾場的紀錄**不在了**」——同一份報告，四行
            // 之隔，兩句互相打臉的**另一種**，而它一次都沒刪過東西。
            //
            // 所以四種各釘一次：這一列不可以說出上面那一列否認掉的事。
            for (e, forbidden) in [
                (Emptiness::Barren, "不在了"),
                (Emptiness::Live, "不在了"),
                (Emptiness::Fresh, "不在了"),
            ] {
                let (_, said) = signal_line(&erased, e);
                let (_, crash) = crash_verdict(&audit(0, 0, 0, 0), e);
                assert!(
                    !crash.contains(forbidden),
                    "前提壞了——這一格的「零當機」本來就不該提「{forbidden}」：{crash}"
                );
                assert!(
                    !said.contains(forbidden),
                    "{e:?}：上面那一列說沒有東西被拿走，這一列不可以說有：{said}"
                );
            }
            // 真的沒錄過的那一台照舊。
            let (_, fresh) = signal_line(&erased, Emptiness::Fresh);
            assert!(
                fresh.contains("還沒有任何一場"),
                "全新的機器上這句話還是對的：{fresh}"
            );

            // 有範圍的時候 `empty` 一個字都不准動——三種判決都要照原樣講。
            let alive = SignalAudit {
                rows: 12,
                populated: 12,
                verdict: SignalVerdict::Alive,
                scope_started_at: Some(1_700_000_000_000),
                ..erased
            };
            for e in Emptiness::ALL {
                let (sym, said) = signal_line(&alive, e);
                assert_eq!(sym, "✓");
                assert!(said.contains("12 列") && said.contains("上一場"), "{said}");
            }
        }

        /// **「上一場」在她還在錄的時候，指的是一場不存在的錄製。**
        ///
        /// 同一份 doctor：
        ///
        /// ```text
        ///   ✓ 零當機     2 段錄製全部正常收尾。現在正在錄的那一場沒有算進去
        ///   ✓ 視窗焦點   上一場（02:45:42 起） 31 列，31 列知道自己是哪個 app
        /// ```
        ///
        /// 02:45:42 就是被扣掉的那一場。問「那 2 場裡哪一場是上一場」，答案是
        /// 都不是——它既不在分母裡，也還沒結束。兩行各自都對，貼在一起指向一
        /// 個空集合，第十七次。
        ///
        /// 位元算在 [`sister_core::db::Db::signal_audit`] 裡（規則 24），所以
        /// 這一條測的是「拿到之後有沒有講出來」。
        #[test]
        fn while_she_is_recording_those_three_rows_are_this_session_not_the_last_one() {
            use sister_core::db::{SignalAudit, SignalVerdict};
            let running = SignalAudit {
                name: "視窗焦點",
                rows: 31,
                populated: 31,
                populated_label: "列知道自己是哪個 app",
                verdict: SignalVerdict::Alive,
                note: "",
                scope_started_at: Some(1_700_000_000_000),
                scope_is_live: true,
            };
            let (_, said) = signal_line(&running, Emptiness::Live);
            assert!(
                !said.contains("上一場"),
                "她還在錄，這幾個數字就是這一場的：{said}"
            );
            assert!(
                said.contains("這一場") && said.contains("還在錄"),
                "而且要講出來——不然那個時間讀起來像一場已經結束的：{said}"
            );
            assert!(said.contains("31 列"), "數字照舊：{said}");

            // 四種判決都要跟著換稱呼。少一種就是留一格空白給下一次。
            for v in [
                SignalVerdict::Alive,
                SignalVerdict::Broken,
                SignalVerdict::TooEarly,
            ] {
                for rows in [0, 31] {
                    let (_, said) = signal_line(
                        &SignalAudit {
                            verdict: v,
                            rows,
                            ..running
                        },
                        Emptiness::Live,
                    );
                    assert!(!said.contains("上一場"), "{v:?}/{rows}：{said}");
                }
            }

            // 而同一份報告上面那一列的分母正好把這一場扣掉了。兩句話要讀得起
            // 來：一句說「沒有算進去」，另一句說「那一場就是這一場」。
            let (_, crash) = crash_verdict(
                &sister_core::db::CrashAudit {
                    live: true,
                    beat: Presence::Live(sister_core::heartbeat::Phase::Recording),
                    ..audit(2, 2, 2, 0)
                },
                Emptiness::Live,
            );
            assert!(crash.contains("沒有算進去"), "{crash}");
            assert!(
                !crash.contains("上一場") && !said.contains("沒有算進去"),
                "兩列各講各的一半，不要互相蓋台：\n  {crash}\n  {said}"
            );

            // 她停了之後同一顆資料庫、同一列，回到「上一場」。
            let (_, stopped) = signal_line(
                &SignalAudit {
                    scope_is_live: false,
                    ..running
                },
                Emptiness::Erased,
            );
            assert!(
                stopped.contains("上一場") && !stopped.contains("還在錄"),
                "{stopped}"
            );
        }

        /// 上面兩條測的是「拿到 `Emptiness` 之後講什麼」。這一條測的是**資料庫
        /// 自己分不分得出來**——而那正是變異實驗裡唯一活下來的縫：把
        /// `Emptiness::of` 開頭那道排除／暫停的問題拿掉，兩條單元測試都還是綠
        /// 的，而真的跑一次 `doctor` 會說「（還沒有任何內容）」。
        #[test]
        fn the_database_itself_can_tell_which_kind_of_empty_it_is() {
            use sister_core::model::{SystemEvent, SystemKind};

            let mut db = Db::open_in_memory().expect("db");
            assert_eq!(
                Emptiness::of(
                    &db,
                    &db.stats().expect("stats"),
                    Presence::Stopped { at: None }
                )
                .expect("of"),
                Emptiness::Fresh,
                "剛開的資料庫"
            );

            let s = db.start_session("test", "0").expect("session");
            db.insert_system(
                s,
                &SystemEvent {
                    ts: 1_000,
                    kind: SystemKind::Excluded,
                    detail: Some("excluded app: keepassxc".into()),
                },
            )
            .expect("blocked");
            assert_eq!(
                Emptiness::of(
                    &db,
                    &db.stats().expect("stats"),
                    Presence::Stopped { at: None }
                )
                .expect("of"),
                Emptiness::Blocked,
                "她錄了，是規則擋掉的——證據就在這顆資料庫裡"
            );

            // 忘掉一切之後，那份證據也走了，剩下的只有 meta 裡那一個位元。
            db.forget(0, 2_000, None).expect("forget");
            db.end_session(s).expect("end");
            assert_eq!(
                Emptiness::of(
                    &db,
                    &db.stats().expect("stats"),
                    Presence::Stopped { at: None }
                )
                .expect("of"),
                Emptiness::Erased,
                "這時候才輪到「被 forget 忘掉了」"
            );
        }

        /// **而一顆從來沒存進去過東西的資料庫，長得和上面最後那一步一模一樣。**
        ///
        /// `sessions 0 / system_events 0 / frames 0`、`ever_recorded = 1`——
        /// 逐位元和「清空過」相同，只差 `meta` 裡多不多一個 key。這一條守的就
        /// 是那一個 key：拿掉它，`Emptiness::of` 會在一台 `capture.enabled =
        /// false`、`forget` 從來沒被執行過的機器上回 `Erased`，而上面那條測試
        /// 照樣全綠——它餵的每一顆資料庫都真的存過東西。
        #[test]
        fn a_database_that_never_stored_anything_is_not_an_erased_one() {
            use sister_core::model::{SystemEvent, SystemKind};

            let mut db = Db::open_in_memory().expect("db");
            // `Recorder::new` 的第一個動作，然後 `finish` 的最後一個動作。
            // 中間一列內容都沒有，因為 `capture.enabled = false`。
            let s = db.start_session("test", "0").expect("session");
            for kind in [SystemKind::SessionStart, SystemKind::SessionEnd] {
                db.insert_system(
                    s,
                    &SystemEvent {
                        ts: 1_000,
                        kind,
                        detail: None,
                    },
                )
                .expect("mark");
            }
            db.end_session(s).expect("end");

            let stats = db.stats().expect("stats");
            assert!(
                stats.nothing_recorded_left() && db.ever_recorded().expect("ever"),
                "前提：它走的是和「清空過」同一條路：{stats:?}"
            );
            assert_eq!(
                Emptiness::of(&db, &stats, Presence::Stopped { at: None }).expect("of"),
                Emptiness::Barren,
                "他一次都沒刪過東西，不可以說東西被刪了"
            );
            // **同一顆資料庫，她正開著的時候是另一種。** `Barren` 那幾句要他
            // 去看 `capture.enabled`，而一個三秒前才按下開始記錄的人，機器上
            // 沒有東西需要改——那台機器要的是「再等一下」。
            assert_eq!(
                Emptiness::of(
                    &db,
                    &stats,
                    Presence::Live(sister_core::heartbeat::Phase::Recording)
                )
                .expect("of"),
                Emptiness::Live,
                "她此刻正佔著這個目錄，不是「跑完了什麼都沒存到」"
            );
            // **而「正在起來」是第三種，不是上面兩種之一。** 上一版這裡收的是
            // 一個布林（`beat.is_some()`），於是開機那幾分鐘走進 `Live`，報告
            // 上印出「她此刻正在錄，到現在還沒有一列落地」——她還沒開始錄。一
            // 個布林湊不出三種答案，而少掉的那一種永遠是「正在來、還沒好」。
            assert_eq!(
                Emptiness::of(
                    &db,
                    &stats,
                    Presence::Live(sister_core::heartbeat::Phase::Booting)
                )
                .expect("of"),
                Emptiness::Booting,
                "她在開資料庫，還沒開始記——既不是「正在錄」也不是「跑完沒存到」"
            );

            // 反面：同一顆資料庫，只要真的存過一列，清空之後就回得到 `Erased`。
            let s = db.start_session("test", "0").expect("session");
            db.insert_system(
                s,
                &SystemEvent {
                    ts: 2_000,
                    kind: SystemKind::Lock,
                    detail: None,
                },
            )
            .expect("content");
            db.forget(0, 3_000, None).expect("forget");
            db.end_session(s).expect("end");
            assert_eq!(
                Emptiness::of(
                    &db,
                    &db.stats().expect("stats"),
                    Presence::Stopped { at: None }
                )
                .expect("of"),
                Emptiness::Erased,
                "存過就是存過——那個位元撐得過 forget"
            );
        }

        /// **這台機器上沒有中文，不代表這台機器上沒有東西。**
        ///
        /// 上一版拿 `with_cjk == 0` 直接去問 `Emptiness`，於是一台跑英文、順手
        /// 擋了 keepassxc 的機器，doctor 長這樣：
        ///
        /// ```text
        ///   ? 兩個字的中文   資料庫裡沒有中文可以驗——那段時間被規則擋掉或暫停了
        ///   ✓ 已記錄         2 張畫面 · 6 段文字 · 168.0 KB
        /// ```
        ///
        /// 兩行之隔、同一顆資料庫：沒有任何跟中文有關的東西被擋掉，那裡就是
        /// 沒有中文。這是這一批修改**自己犯的第三次**同一個錯，而前兩次都是
        /// 把判斷留在呼叫端、沒有任何測試碰得到——所以這條測的是那道閘門。
        #[test]
        fn an_english_only_machine_is_not_told_the_rules_ate_its_chinese() {
            // 6 段字、一個中文詞都沒有，而這台機器上排除規則真的命中過。
            let (sym, said) = bigram_verdict(0, 0, 6, Emptiness::Blocked);
            assert_eq!(sym, "?", "沒有中文可以驗不是失敗，但也不是打勾");
            assert!(said.contains("6 段"), "她記了多少字要講出來：{said}");
            for lie in ["規則", "暫停", "忘掉", "保留期"] {
                assert!(
                    !said.contains(lie),
                    "那 6 段字好好地在資料庫裡，不可以說被「{lie}」帶走了：{said}"
                );
            }
            // 同一件事對「被清空」也成立：清空過、但後來又記了英文的機器，
            // 這一列講的還是英文。
            assert_eq!(bigram_verdict(0, 0, 6, Emptiness::Erased).1, said);

            // 真的一段字都不剩，才輪到「為什麼」——而三種的下一步都不一樣。
            let said = |e| bigram_verdict(0, 0, 0, e).1;
            // **走 `Emptiness::ALL`，不要寫陣列字面量。** 字面量不會因為 enum
            // 多一種變體而編不過，而上一版 `Barren` 加進來的時候，這裡還停在
            // 三種——守著「不可以兩種處境共用一句話」的測試，剛好漏掉的就是那
            // 次新加的那一種。
            let all = Emptiness::ALL.map(said);
            for (i, a) in all.iter().enumerate() {
                for (j, b) in all.iter().enumerate().skip(i + 1) {
                    assert_ne!(
                        a,
                        b,
                        "{:?} 和 {:?} 的下一步不一樣，不可以共用一句話",
                        Emptiness::ALL[i],
                        Emptiness::ALL[j]
                    );
                }
            }
            assert!(
                !all[0].contains("忘掉") && !all[0].contains("保留期"),
                "全新的機器上不可以暗示他刪過東西：{}",
                all[0]
            );

            // 有中文的時候 `Emptiness` 一個字都不准動：覆蓋率就是覆蓋率。
            for e in Emptiness::ALL {
                assert_eq!(bigram_verdict(12, 12, 40, e).0, "✓");
                assert_eq!(bigram_verdict(3, 12, 40, e).0, "✗");
                assert!(bigram_verdict(3, 12, 40, e).1.contains("3/12"));
            }
        }

        /// **一個 surface 把刪除列出來，另一個掛著綠勾說那件事沒發生過。**
        ///
        /// `replay` → `sister query 帳單` → `sister forget --last 24h --yes`
        /// （`forget` 自己會說「刪掉了 1 題你自己問過的話」）之後：
        ///
        /// ```text
        ///   sister queries → 題庫是空的。…也可能是問過的那幾題被 `sister forget` 忘掉了
        ///   sister doctor  → ✓ 你問過她什麼   記著（還沒問過任何問題）
        /// ```
        #[test]
        fn doctor_does_not_green_check_a_question_log_that_forget_took() {
            let none = sister_core::db::QueryLogStats::default();
            let erased = query_log_verdict(true, true, true, &none).1;
            let fresh = query_log_verdict(true, true, false, &none).1;
            // 差別在**語氣**：一邊是「還沒問過」這句斷言，一邊退回只講現在。
            assert_ne!(erased, fresh, "同樣是 0，錄過的和沒錄過的講的不是同一件事");
            assert!(
                !erased.contains("還沒問過"),
                "他問過的那幾題可能剛被帶走，這裡不可以斷言那沒發生過：{erased}"
            );
            assert!(
                erased.contains("queries"),
                "不攤開就要指得出攤開的地方在哪：{erased}"
            );
            assert!(
                fresh.contains("還沒問過"),
                "她沒錄過就不可能被忘掉，那時候這句話是完整的，不必打折：{fresh}"
            );

            // `ever` 只准在 total == 0 那一格說話。
            let some = sister_core::db::QueryLogStats {
                total: 7,
                empty: 2,
                ..Default::default()
            };
            assert_eq!(
                query_log_verdict(true, true, false, &some),
                query_log_verdict(true, true, true, &some),
                "題庫裡有東西的時候，那個旗標不該改變任何一個字"
            );

            // 資料庫打不開的時候，`unwrap_or_default()` 給的 0 什麼都不代表。
            for ever in [false, true] {
                let (sym, said) = query_log_verdict(false, true, ever, &none);
                assert_eq!(sym, "?", "問不到的東西不可以打勾");
                assert!(!said.contains("還沒問過"), "那可能是他一整年的題庫：{said}");
            }
        }

        /// 設定檔好好的那條路也要走一次，不然上面那條可能只是「錯誤路徑能跑」。
        #[test]
        fn a_healthy_config_still_runs_the_whole_check() {
            let dir = crate::ops::tmp::Tmp::new("doctor-ok-config");
            run(&dir.0, Ok(Config::default()), None).expect("正常設定當然要跑得完");
        }

        /// 八種狀態，一種都不能讓符號和句子講不同的話。
        ///
        /// 這條擋的是一個**已經發生過兩次**的東西：`✗` 由
        /// `Consent::keeps_images`（三個布林）算，句子由旁邊一個手寫的
        /// match 算，而那個 match 少看一個布林。於是
        ///
        /// ```text
        ///   ✗ 保留畫面檔     是
        /// ```
        ///
        /// 一行之內符號說不、字說是。這種東西每次都是「加一個旗標的時候
        /// 忘了改另一半」，所以測的不是某一句話長什麼樣（那會一直改），
        /// 而是**兩半永遠對得起來**——把任何一支的參數少傳一個，或是把
        /// 「否」寫成「是」，這條就紅。
        #[test]
        fn the_mark_and_the_words_never_disagree_about_whether_screenshots_get_written() {
            for allows in [false, true] {
                for enabled in [false, true] {
                    for store in [false, true] {
                        let mut c = Config::default();
                        c.capture.enabled = enabled;
                        c.capture.store_images = store;
                        let mut consent = sister_core::consent::Consent::default();
                        if allows {
                            consent.grant(sister_core::consent::Sheet::FrameStorage, 1);
                        }

                        let truth = consent.keeps_images(&c);
                        assert_eq!(
                            truth,
                            allows && enabled && store,
                            "keeps_images 自己就該是這三個布林的 AND"
                        );

                        let kept = frames_kept_words(allows, enabled, store);
                        assert_eq!(
                            truth,
                            kept.starts_with("是"),
                            "保留畫面檔：符號說 {truth}，字說「{kept}」\
                             （allows={allows} enabled={enabled} store={store}）"
                        );

                        // 上面同意書那一區的 ✓ 問的是另一件事（「你簽了沒」），
                        // 但那一行的**句子**要負責把設定檔擋在哪裡講出來，
                        // 不然一個 ✓ 會被讀成「會有截圖」。
                        let sheet = frame_sheet_words(allows, allows, enabled, store);
                        assert_eq!(
                            allows,
                            sheet.starts_with("已同意"),
                            "畫面暫存：allows={allows}，字說「{sheet}」"
                        );
                        if allows && !truth {
                            assert!(
                                sheet.contains('但'),
                                "簽了卻不會留圖，這一行不能只說「已同意」：「{sheet}」"
                            );
                        }
                    }
                }
            }
        }

        /// 條文改版之後，三張同意書要給同一種答案。
        ///
        /// 以前 `上雲解讀` 那一行讀的是 `cloud_reading.is_some()`，少了
        /// `current()`。於是一個上個月簽完三張的人，會在同一頁上讀到
        /// 「本機記錄：舊簽名失效」「畫面暫存：未同意」「上雲解讀：已同意」
        /// ——三行講同一個檔案，三種答案，而說「已同意」的那一張正好是
        /// 唯一一張猜錯了會讓東西離開這台機器的。
        #[test]
        fn a_version_bump_voids_all_three_sheets_and_all_three_lines_say_so() {
            let mut consent = sister_core::consent::Consent::default();
            for s in [
                sister_core::consent::Sheet::LocalRecording,
                sister_core::consent::Sheet::CloudReading,
                sister_core::consent::Sheet::FrameStorage,
            ] {
                consent.grant(s, 1);
            }
            assert!(consent.allows_cloud(), "剛簽完當然算數");

            consent.version = sister_core::consent::VERSION.wrapping_add(1);
            assert!(!consent.current());
            assert!(!consent.allows_recording());
            assert!(!consent.allows_frames());
            assert!(
                !consent.allows_cloud(),
                "第二張不能是三張裡唯一一張不看版本的"
            );

            // 而且要說得出**為什麼**：「沒簽」和「簽了但失效」的下一步不一樣，
            // 一個是去簽，一個是去重簽。長得一樣的話兩邊都會卡住。
            let words = frame_sheet_words(false, true, true, true);
            assert!(words.contains("改版"), "「{words}」讀起來像從來沒簽過");
        }
    }
}

/// 一次擷取到底貴在哪一段。
///
/// 這一段本來只活在 `screen.rs` 的 `#[cfg(test)]` 裡。而唯一有那台
/// 2560×1440 機器的人手上只有一個下載來的 exe——一個只有 `cargo test`
/// 碰得到的量尺，對他等於不存在，於是 #18 卡在「我推測是 GetDIBits」。
/// 量尺要跟著產品出貨。
pub mod bench {
    use super::*;

    #[cfg(any(windows, test))]
    use sister_capture::traits::Ocr;

    /// 一張畫面的大小。具名欄位讓寬高在呼叫端對調時不會安靜通過。
    #[cfg(any(windows, test))]
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct Pixels {
        w: u32,
        h: u32,
    }

    #[cfg(any(windows, test))]
    impl Pixels {
        fn long(self) -> u32 {
            self.w.max(self.h)
        }
        fn short(self) -> u32 {
            self.w.min(self.h)
        }
        fn label(self) -> String {
            format!("{}x{}", self.w, self.h)
        }
    }

    /// 一行字出現幾次。集合會把螢幕上重複的「確定」吃掉，讓真的漏字變成零。
    #[cfg(any(windows, test))]
    type Lines = std::collections::BTreeMap<String, usize>;

    #[cfg(any(windows, test))]
    fn lines_of(texts: impl IntoIterator<Item = String>) -> Lines {
        let mut lines = Lines::new();
        for text in texts {
            let text = text.trim();
            if !text.is_empty() {
                *lines.entry(text.to_string()).or_default() += 1;
            }
        }
        lines
    }

    /// 兩次原生都讀到的行。會自己變的時鐘與通知不該拿來當尺。
    #[cfg(any(windows, test))]
    #[derive(Default)]
    struct Baseline(Lines);

    #[cfg(any(windows, test))]
    impl Baseline {
        fn from_natives(first: &Lines, second: &Lines) -> Self {
            Self(
                first
                    .iter()
                    .filter_map(|(line, count)| {
                        second
                            .get(line)
                            .map(|other| (line.clone(), (*count).min(*other)))
                    })
                    .collect(),
            )
        }
        fn is_empty(&self) -> bool {
            self.0.is_empty()
        }
        fn total(&self) -> usize {
            self.0.values().sum()
        }
        fn missing_in(&self, row: &Lines) -> usize {
            self.0
                .iter()
                .map(|(line, count)| count.saturating_sub(*row.get(line).unwrap_or(&0)))
                .sum()
        }
        fn extra_in(&self, row: &Lines, volatile: &Lines) -> usize {
            row.iter()
                .map(|(line, count)| {
                    count.saturating_sub(
                        self.0.get(line).unwrap_or(&0) + volatile.get(line).unwrap_or(&0),
                    )
                })
                .sum()
        }
    }

    /// OCR 一次要 2–3 秒；五張 ×（1+3）＝20 次已要一分多鐘，而這整段
    /// 畫面都不能動。再加只會讓兩次原生隔得更遠、共同基準線更薄。
    #[cfg(any(windows, test))]
    const MAX_OCR_ROUNDS: u32 = 3;

    #[cfg(any(windows, test))]
    #[derive(Clone, Copy)]
    struct Rounds {
        requested: Option<u32>,
        used: u32,
    }

    #[cfg(any(windows, test))]
    impl Rounds {
        fn choose(requested: Option<u32>) -> Self {
            Self {
                requested,
                used: requested.unwrap_or(MAX_OCR_ROUNDS).clamp(1, MAX_OCR_ROUNDS),
            }
        }
    }

    #[cfg(any(windows, test))]
    enum Shot {
        Read {
            size: Pixels,
            avg_ms: f64,
            lines: Lines,
            wobbled: bool,
        },
        NoFrame,
        CaptureFailed(String),
        OcrFailed {
            size: Pixels,
            why: String,
            too_large: bool,
        },
    }

    #[cfg(any(windows, test))]
    struct Row {
        label: String,
        shot: Shot,
    }

    #[cfg(any(windows, test))]
    struct Skipped {
        long_edge: u32,
        why: String,
    }

    #[cfg(any(windows, test))]
    struct OcrTable {
        screen: ScreenSizes,
        rounds: Rounds,
        planned: usize,
        taken: usize,
        skipped: Vec<Skipped>,
        rows: Vec<Row>,
        baseline: Baseline,
        volatile_first: usize,
        volatile_second: usize,
        volatile: Lines,
    }

    #[cfg(any(windows, test))]
    const CANDIDATES: [u32; 3] = [1568, 1280, 1024];
    #[cfg(any(windows, test))]
    const COL_SIZE: &str = "尺寸";
    #[cfg(any(windows, test))]
    const COL_PIXELS: &str = "像素";
    #[cfg(any(windows, test))]
    const COL_MS: &str = "OCR 平均";
    #[cfg(any(windows, test))]
    const COL_MISSING: &str = "少了幾行";
    #[cfg(any(windows, test))]
    const COL_EXTRA: &str = "多了幾行";
    #[cfg(any(windows, test))]
    fn guide_indent() -> String {
        crate::fmt::pad("", crate::fmt::display_width("怎麼讀："))
    }

    #[cfg(any(windows, test))]
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct ScreenSizes {
        native: Pixels,
        ocr: Pixels,
    }

    #[cfg(any(windows, test))]
    impl ScreenSizes {
        fn from_native(native: Pixels) -> Self {
            let (w, h) = sister_capture::scale::fit(
                native.w,
                native.h,
                sister_capture::scale::OCR_LONG_EDGE,
            );
            Self {
                native,
                ocr: Pixels { w, h },
            }
        }
    }

    #[cfg(any(windows, test))]
    fn plan(screen: ScreenSizes) -> (Vec<u32>, Vec<Skipped>) {
        let native = screen.ocr;
        let mut use_edges = Vec::new();
        let mut skipped = Vec::new();
        for edge in CANDIDATES {
            if edge >= native.long() {
                let measured_long = if screen.native == screen.ocr {
                    format!("OCR 實際長邊 {}", native.long())
                } else {
                    format!(
                        "OCR 受 {} 上限夾過的長邊 {}",
                        sister_capture::scale::OCR_LONG_EDGE,
                        native.long()
                    )
                };
                skipped.push(Skipped {
                    long_edge: edge,
                    why: format!("候選長邊 {edge} 不小於 {measured_long}，那不是縮小"),
                });
                continue;
            }
            let (w, h) = sister_capture::scale::fit(native.w, native.h, edge);
            let size = Pixels { w, h };
            if size.short() < sister_capture::scale::OCR_MIN_SHORT_EDGE {
                let clipped = if screen.native == screen.ocr {
                    String::new()
                } else {
                    format!(
                        "OCR 原圖已受 {} 長邊上限夾為 {}；",
                        sister_capture::scale::OCR_LONG_EDGE,
                        screen.ocr.label()
                    )
                };
                skipped.push(Skipped {
                    long_edge: edge,
                    why: format!(
                        "{clipped}縮完短邊只有 {}，低於 OCR 下限 {}",
                        size.short(),
                        sister_capture::scale::OCR_MIN_SHORT_EDGE
                    ),
                });
            } else {
                use_edges.push(edge);
            }
        }
        (use_edges, skipped)
    }

    #[cfg(any(windows, test))]
    trait Shots {
        fn screen_size(&mut self) -> Option<ScreenSizes>;
        fn shoot(&mut self, long_edge: u32) -> anyhow::Result<Option<sister_capture::RawFrame>>;
    }

    #[cfg(any(windows, test))]
    trait Clock {
        fn now_ms(&mut self) -> f64;
    }

    /// 先把所有畫面抓在同一個短時間窗，再開始慢很多的 OCR。代價是同時握著
    /// 最多兩張全螢幕加三張縮圖 RGBA；1920×1080 的這一組約 28 MB。
    #[cfg(any(windows, test))]
    fn measure(
        shots: &mut impl Shots,
        ocr: &mut impl Ocr,
        clock: &mut impl Clock,
        say: &mut impl FnMut(&str),
        requested_rounds: Option<u32>,
    ) -> Option<OcrTable> {
        let screen = shots.screen_size()?;
        let native = screen.ocr;
        let rounds = Rounds::choose(requested_rounds);
        let (edges, skipped) = plan(screen);
        let planned = edges.len() + 2;
        say(&format!(
            "{}。接下來要抓 {planned} 張，每張 1 次熱身 + {} 次計時的 OCR（共 {} 次），過程中請不要動畫面。",
            if screen.native == screen.ocr {
                format!("原生 {}", screen.native.label())
            } else {
                format!(
                    "螢幕原始 {}，OCR 受 {} 長邊上限夾為 {}",
                    screen.native.label(),
                    sister_capture::scale::OCR_LONG_EDGE,
                    screen.ocr.label()
                )
            },
            rounds.used,
            planned * (1 + rounds.used as usize)
        ));
        if !skipped.is_empty() {
            say(&format!("（{} 個候選被跳過，理由在表裡）", skipped.len()));
        }

        enum Grab {
            Frame(sister_capture::RawFrame),
            None,
            Failed(String),
        }
        let labels_edges = std::iter::once(("原生 A".to_string(), native.long()))
            .chain(edges.iter().map(|e| (format!("縮到 {e}"), *e)))
            .chain(std::iter::once(("原生 B".to_string(), native.long())))
            .collect::<Vec<_>>();
        let grabs = labels_edges
            .iter()
            .map(|(_, edge)| match shots.shoot(*edge) {
                Ok(Some(frame)) => Grab::Frame(frame),
                Ok(None) => Grab::None,
                Err(error) => Grab::Failed(format!("{error:#}")),
            })
            .collect::<Vec<_>>();

        let mut rows = Vec::with_capacity(planned);
        for ((label, _), grab) in labels_edges.into_iter().zip(grabs) {
            let shot = match grab {
                Grab::None => Shot::NoFrame,
                Grab::Failed(why) => Shot::CaptureFailed(why),
                Grab::Frame(frame) => {
                    let size = Pixels {
                        w: frame.width,
                        h: frame.height,
                    };
                    if let Err(error) = ocr.recognize(&frame) {
                        let too_large = error
                            .chain()
                            .any(|cause| cause.is::<sister_capture::traits::OcrImageTooLarge>());
                        Shot::OcrFailed {
                            size,
                            why: format!("{error:#}"),
                            too_large,
                        }
                    } else {
                        let mut total = 0.0;
                        let mut readings = Vec::new();
                        let mut failed = None;
                        for _ in 0..rounds.used {
                            let start = clock.now_ms();
                            match ocr.recognize(&frame) {
                                Ok(blocks) => {
                                    total += clock.now_ms() - start;
                                    readings.push(lines_of(blocks.into_iter().map(|b| b.text)));
                                }
                                Err(error) => {
                                    let too_large = error.chain().any(|cause| {
                                        cause.is::<sister_capture::traits::OcrImageTooLarge>()
                                    });
                                    failed = Some((format!("{error:#}"), too_large));
                                    break;
                                }
                            }
                        }
                        match failed {
                            Some((why, too_large)) => Shot::OcrFailed {
                                size,
                                why,
                                too_large,
                            },
                            None => {
                                // `used == 1` 時只有一次計時讀取，`windows(2)` 為空，
                                // 因而沒有足夠資料宣稱同一張圖重讀不一致。
                                let wobbled = readings.windows(2).any(|pair| pair[0] != pair[1]);
                                Shot::Read {
                                    size,
                                    avg_ms: total / rounds.used as f64,
                                    lines: readings.pop().unwrap_or_default(),
                                    wobbled,
                                }
                            }
                        }
                    }
                }
            };
            rows.push(Row { label, shot });
        }
        let native_lines = |row: &Row| match &row.shot {
            Shot::Read { lines, .. } => lines.clone(),
            _ => Lines::new(),
        };
        let first = native_lines(&rows[0]);
        let second = native_lines(rows.last().expect("至少有兩列原生"));
        let baseline = Baseline::from_natives(&first, &second);
        let mut volatile = Lines::new();
        for (line, count) in first.iter().chain(second.iter()) {
            if !baseline.0.contains_key(line) {
                volatile
                    .entry(line.clone())
                    .and_modify(|seen| *seen = (*seen).max(*count))
                    .or_insert(*count);
            }
        }
        let volatile_first = first.values().sum::<usize>() - baseline.total();
        let volatile_second = second.values().sum::<usize>() - baseline.total();
        let taken = rows
            .iter()
            .filter(|row| matches!(row.shot, Shot::Read { .. } | Shot::OcrFailed { .. }))
            .count();
        Some(OcrTable {
            screen,
            rounds,
            planned,
            taken,
            skipped,
            rows,
            baseline,
            volatile_first,
            volatile_second,
            volatile,
        })
    }

    #[cfg(any(windows, test))]
    fn render(table: &OcrTable, language: Option<&str>) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        writeln!(
            out,
            "\nOCR 縮圖表（{}）：每種尺寸 1 次熱身不計時 + {} 次計時取平均。\n\
             \x20\x20上限 {MAX_OCR_ROUNDS}（OCR 一次 2–3 秒，再加會拖長量測，也會讓共同基準線更薄）。",
            if table.screen.native == table.screen.ocr {
                format!("原生 {}", table.screen.native.label())
            } else {
                format!(
                    "螢幕原始 {}；OCR 用的大小 {}，長邊上限 {}",
                    table.screen.native.label(),
                    table.screen.ocr.label(),
                    sister_capture::scale::OCR_LONG_EDGE
                )
            },
            table.rounds.used,
        )
        .unwrap();
        if table
            .rounds
            .requested
            .is_some_and(|n| n != table.rounds.used)
        {
            writeln!(
                out,
                "你給了 --rounds {}，{}到 {}。",
                table.rounds.requested.unwrap(),
                if table.rounds.requested.unwrap() < table.rounds.used {
                    "提"
                } else {
                    "壓"
                },
                table.rounds.used
            )
            .unwrap();
        }
        writeln!(
            out,
            "OCR 語言：{}",
            language.unwrap_or("引擎沒有回答語言標籤")
        )
        .unwrap();
        for skipped in &table.skipped {
            writeln!(out, "跳過縮到 {}：{}。", skipped.long_edge, skipped.why).unwrap();
        }
        let accuracy = !table.baseline.is_empty();
        if accuracy {
            writeln!(
                out,
                "  {}{}{}{}{}",
                crate::fmt::pad(COL_SIZE, 14),
                crate::fmt::pad(COL_PIXELS, 14),
                crate::fmt::pad(COL_MS, 14),
                crate::fmt::pad(COL_MISSING, 12),
                COL_EXTRA
            )
            .unwrap();
        } else {
            writeln!(
                out,
                "  {}{}{}",
                crate::fmt::pad(COL_SIZE, 14),
                crate::fmt::pad(COL_PIXELS, 14),
                COL_MS
            )
            .unwrap();
        }
        for row in &table.rows {
            let (pixels, ms, missing, extra) = match &row.shot {
                Shot::Read {
                    size,
                    avg_ms,
                    lines,
                    ..
                } => (
                    size.label(),
                    format!("{avg_ms:.1} ms"),
                    table.baseline.missing_in(lines).to_string(),
                    table.baseline.extra_in(lines, &table.volatile).to_string(),
                ),
                Shot::NoFrame => (
                    "—".into(),
                    "這一刻沒有畫面".into(),
                    String::new(),
                    String::new(),
                ),
                Shot::CaptureFailed(why) => (
                    "—".into(),
                    format!("抓圖失敗：{why}"),
                    String::new(),
                    String::new(),
                ),
                Shot::OcrFailed { size, why, .. } => (
                    size.label(),
                    format!("OCR 失敗：{why}"),
                    String::new(),
                    String::new(),
                ),
            };
            let line = if accuracy {
                format!(
                    "  {}{}{}{}{}",
                    crate::fmt::pad(&row.label, 14),
                    crate::fmt::pad(&pixels, 14),
                    crate::fmt::pad(&ms, 14),
                    crate::fmt::pad(&missing, 12),
                    extra
                )
            } else {
                format!(
                    "  {}{}{}",
                    crate::fmt::pad(&row.label, 14),
                    crate::fmt::pad(&pixels, 14),
                    ms
                )
            };
            writeln!(out, "{}", line.trim_end()).unwrap();
        }
        if !accuracy {
            let first = &table.rows.first().expect("至少有原生 A").shot;
            let second = &table.rows.last().expect("至少有原生 B").shot;
            if matches!(first, Shot::Read { .. }) && matches!(second, Shot::Read { .. }) {
                let count = |shot: &Shot| match shot {
                    Shot::Read { lines, .. } => lines.values().sum::<usize>(),
                    _ => unreachable!(),
                };
                let first_count = count(first);
                let second_count = count(second);
                if first_count == 0 && second_count == 0 {
                    writeln!(
                        out,
                        "兩次原生都沒有讀到任何一行字；畫面上可能真的沒有字，也可能引擎讀不出來，\n\
                                  `sister doctor` 會分得出來。\n\
                                  所以不印準確度兩欄。"
                    )
                    .unwrap();
                } else if first_count == 0 || second_count == 0 {
                    writeln!(
                        out,
                        "原生 A 讀到 {first_count} 行、原生 B 讀到 {second_count} 行；其中一次原生沒有讀到字，\n\
                         先用 `sister doctor` 確認引擎能讀字，所以不印準確度兩欄。"
                    )
                    .unwrap();
                } else {
                    writeln!(
                        out,
                        "原生 A 讀到 {first_count} 行、原生 B 讀到 {second_count} 行，兩邊沒有一行相同；\n\
                         所以不印準確度兩欄。"
                    )
                    .unwrap();
                }
            } else {
                let fact = |label: &str, shot: &Shot| match shot {
                    Shot::Read { lines, .. } => {
                        format!("{label} 讀到 {} 行", lines.values().sum::<usize>())
                    }
                    Shot::NoFrame => {
                        format!("{label} 這一刻沒有畫面（請解鎖，並把前景視窗移到螢幕上）")
                    }
                    Shot::CaptureFailed(why) => {
                        format!("{label} 抓圖失敗：{why}（請解鎖，並把前景視窗移到螢幕上）")
                    }
                    Shot::OcrFailed { why, too_large, .. } => format!(
                        "{label} OCR 失敗：{why}{}",
                        if *too_large {
                            "（請看引擎上限那則錯誤）"
                        } else {
                            ""
                        }
                    ),
                };
                writeln!(
                    out,
                    "{}；{}。\n\
                     所以沒有共同基準線，不印準確度兩欄。",
                    fact("原生 A", first),
                    fact("原生 B", second)
                )
                .unwrap();
            }
        }
        writeln!(
            out,
            "抓到 {} 張（計畫 {} 張）。",
            table.taken, table.planned
        )
        .unwrap();
        if table.taken < table.planned {
            let missing = table
                .rows
                .iter()
                .filter(|r| matches!(r.shot, Shot::NoFrame | Shot::CaptureFailed(_)))
                .map(|r| r.label.as_str())
                .collect::<Vec<_>>()
                .join("、");
            if !missing.is_empty() {
                writeln!(out, "沒抓到：{missing}。").unwrap();
            }
        }
        if !table.baseline.is_empty() {
            writeln!(
                out,
                "基準線：兩次原生都讀到的 {} 行。",
                table.baseline.total()
            )
            .unwrap();
        }
        if !table.baseline.is_empty() && table.volatile_first + table.volatile_second > 0 {
            writeln!(
                out,
                "另有 {}+{} 行只出現在其中一次（會自己變的東西：時鐘、通知），已排除。",
                table.volatile_first, table.volatile_second
            )
            .unwrap();
        }
        let wobbled = table
            .rows
            .iter()
            .filter_map(|r| {
                matches!(r.shot, Shot::Read { wobbled: true, .. }).then_some(r.label.as_str())
            })
            .collect::<Vec<_>>();
        if !wobbled.is_empty() {
            writeln!(
                out,
                "同一張圖重讀時結果不一致：{}；表裡採最後一次。",
                wobbled.join("、")
            )
            .unwrap();
        }
        let indent = guide_indent();
        if table.planned == 2 {
            let not_smaller = table.skipped.iter().any(|s| s.why.contains("不是縮小"));
            let too_short = table
                .skipped
                .iter()
                .any(|s| s.why.contains("低於 OCR 下限"));
            let reason = match (not_smaller, too_short) {
                (true, false) => "候選都不小於 OCR 實際大小",
                (false, true) => "候選縮完的短邊都低於 OCR 下限",
                (true, true) => "候選不是不夠小，就是縮完短邊低於 OCR 下限",
                (false, false) => "這次沒有候選可量",
            };
            writeln!(
                out,
                "為什麼只有兩列：{reason}，所以表裡只有原生 A、原生 B 兩列。"
            )
            .unwrap();
        }
        if accuracy {
            let noise = if table.volatile_first + table.volatile_second > 0 {
                format!(
                    "上面的「另有 {}+{} 行」是這台機器這次觀察到的揮發雜訊底線；",
                    table.volatile_first, table.volatile_second
                )
            } else {
                "這次兩列原生沒有觀察到揮發行；".to_string()
            };
            writeln!(out, "怎麼讀：「{COL_MS}」是一次 OCR 的平均耗時；「{COL_MISSING}」是兩次原生都有的完整行字串、\n\
                           {indent}這一列沒有完全相同命中的行；一字、內部空白或拆併行不同，都可能同時算一行少了、一行多了。\n\
                           {indent}它量的是穩定完整行，不等於證明同樣數量的字消失；「{COL_EXTRA}」是這一列超過基準線與已知揮發行後仍多出的行。\n\
                           {indent}時鐘、通知等會自己變的行也可能落在「{COL_EXTRA}」；{noise}\n\
                           {indent}縮圖列明顯高過底線時，才值得懷疑是讀錯。「{COL_MISSING}」與「{COL_EXTRA}」要一起看。\n\
                           {indent}「{COL_SIZE}」與「{COL_PIXELS}」說明實際量到哪一列畫面。").unwrap();
        } else {
            writeln!(
                out,
                "怎麼讀：「{COL_MS}」是一次 OCR 的平均耗時；這次沒有共同基準線，不能比較準確度。\n\
                           {indent}「{COL_SIZE}」與「{COL_PIXELS}」說明實際量到哪一列畫面。"
            )
            .unwrap();
        }
        let timed = table
            .rows
            .iter()
            .filter(|row| matches!(row.shot, Shot::Read { .. }))
            .count();
        match timed {
            0 => {
                let capture_failed = table.rows.iter().any(|row| {
                    matches!(row.shot, Shot::NoFrame | Shot::CaptureFailed(_))
                });
                let ocr_failed = table
                    .rows
                    .iter()
                    .any(|row| matches!(row.shot, Shot::OcrFailed { .. }));
                let next = match (capture_failed, ocr_failed) {
                    (true, false) => "請解鎖、回到一般互動桌面，把前景視窗移到螢幕上後再跑一次。",
                    (false, true) => "畫面抓得到，但 OCR 每次都失敗；請跑 `sister doctor` 的內建圖自我測試。",
                    (true, true) => "有些畫面沒抓到，另一些抓到後 OCR 失敗；請先解鎖並把前景視窗移到螢幕上，再跑 `sister doctor` 的內建圖自我測試。",
                    (false, false) => "這次沒有成功的計時結果。",
                };
                writeln!(out, "這張表沒有量到任何毫秒；{next}")
            }
            1 => writeln!(out, "這張表只量到一個毫秒數，沒有第二個數字可以比較。"),
            _ => writeln!(out, "這張表量到 {timed} 個毫秒數，可以比較速度。"),
        }
        .unwrap();
        out
    }

    #[cfg(windows)]
    struct MonotonicClock(std::time::Instant);

    #[cfg(windows)]
    impl Clock for MonotonicClock {
        fn now_ms(&mut self) -> f64 {
            self.0.elapsed().as_secs_f64() * 1000.0
        }
    }

    #[cfg(windows)]
    impl Shots for sister_capture::windows::screen::WindowsScreen {
        fn screen_size(&mut self) -> Option<ScreenSizes> {
            let (w, h) = Self::frame_size()?;
            Some(ScreenSizes::from_native(Pixels { w, h }))
        }
        fn shoot(&mut self, long_edge: u32) -> anyhow::Result<Option<sister_capture::RawFrame>> {
            self.capture(sister_core::now_ms(), long_edge)
        }
    }

    #[cfg(windows)]
    pub fn run(requested_rounds: Option<u32>) -> Result<()> {
        const DEFAULT_GDI_ROUNDS: u32 = 8;
        let rounds = requested_rounds.unwrap_or(DEFAULT_GDI_ROUNDS).max(1);
        if requested_rounds.is_some_and(|n| n != rounds) {
            println!(
                "你給了 --rounds {}，GDI 表改用 {rounds}。",
                requested_rounds.unwrap()
            );
        }
        let rows = sister_capture::windows::screen::bench_grab(rounds);
        if rows.is_empty() {
            println!(
                "量不到：現在抓不到畫面（鎖屏、沒有互動桌面，或前景視窗不在任何一台螢幕上）。"
            );
            println!("解鎖之後在一般的桌面 session 裡再跑一次。");
            return Ok(());
        }
        let (w, h) = (rows[0].width, rows[0].height);
        println!("原生 {w}x{h}，每種抓法 {rounds} 次（另有一輪熱身不計時）：\n");
        println!("  抓法                    建立    BitBlt  GetDIBits    合計");
        for r in &rows {
            println!(
                "  {:<20}{:7.1}{:9.1}{:10.1}{:9.1} ms",
                r.label,
                r.make_ms,
                r.blt_ms,
                r.dib_ms,
                r.total_ms()
            );
        }
        // 每一段各自通往完全不同的下一步，所以要講怎麼讀——一張沒有讀法的
        // 表，會被讀成「有數字了」然後放著。
        println!(
            "\n怎麼讀（{} 像素，同樣大小的 memcpy 約 1.5 ms）：",
            w as i64 * h as i64
        );
        println!(
            "  建立最大      → 每一拍新建再刪掉一張 {:.1} MB 的 bitmap 太貴，改成快取重用。",
            (w as f64 * h as f64 * 4.0) / 1024.0 / 1024.0
        );
        println!(
            "  BitBlt 最大   → 跟顯示驅動要畫面的過路費。如果「純 SRCCOPY」明顯比\n\
             \x20                 「含 CAPTUREBLT」便宜，那個旗標就是在付一筆 Win8 之後\n\
             \x20                 已經免費的錢（桌面 DC 本來就含分層視窗）。兩者一樣貴的話\n\
             \x20                 GDI 這條路到頭了，只有 DXGI Desktop Duplication 救得了。"
        );
        println!(
            "  GetDIBits 最大 → device-dependent bitmap 的轉格式讀回。改成\n\
             \x20                 CreateDIBSection，讓 BitBlt 直接寫進我們自己的記憶體，\n\
             \x20                 這一段整個消失。"
        );
        println!("\n以下 OCR 表使用預設語言偏好。");
        let config = sister_core::config::Config::default();
        let mut ocr = sister_capture::windows::ocr::WindowsOcr::new(&config.capture.ocr_languages);
        if !ocr.is_available() {
            println!("這台機器沒有可用的 OCR 引擎，這張表量不到，跑 `sister doctor` 看是缺什麼。");
            return Ok(());
        }
        let mut screen = sister_capture::windows::screen::WindowsScreen::new();
        let mut clock = MonotonicClock(std::time::Instant::now());
        let mut say = |s: &str| println!("{s}");
        match measure(
            &mut screen,
            &mut ocr,
            &mut clock,
            &mut say,
            requested_rounds,
        ) {
            Some(table) => println!("{}", render(&table, ocr.language().as_deref())),
            None => println!("現在抓不到畫面，OCR 縮圖表量不到。"),
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn run(_rounds: Option<u32>) -> Result<()> {
        // 不是「這台機器很慢」，是「這台機器沒有這條路可以量」。兩者長得
        // 一樣的話，開發機上跑一次會得到一張空表然後被當成「都是 0，很好」。
        println!("這個平台沒有 GDI 擷取後端，沒有東西可以量。這條路目前只有 Windows。");
        Ok(())
    }

    #[cfg(test)]
    mod bench_tests {
        use super::*;
        use sister_core::model::OcrBlock;
        use std::{cell::RefCell, collections::VecDeque, rc::Rc};

        type Events = Rc<RefCell<Vec<String>>>;

        struct FakeShots {
            screen: Option<ScreenSizes>,
            replies: VecDeque<anyhow::Result<Option<sister_capture::RawFrame>>>,
            events: Events,
        }

        impl Shots for FakeShots {
            fn screen_size(&mut self) -> Option<ScreenSizes> {
                self.screen
            }
            fn shoot(&mut self, edge: u32) -> anyhow::Result<Option<sister_capture::RawFrame>> {
                self.events.borrow_mut().push(format!("shoot:{edge}"));
                self.replies
                    .pop_front()
                    .unwrap_or_else(|| Ok(Some(frame(edge))))
            }
        }

        enum Reply {
            Lines(Vec<&'static str>),
            Fail(&'static str),
            TooLarge,
        }
        struct FakeOcr {
            replies: VecDeque<Reply>,
            events: Events,
        }
        impl Ocr for FakeOcr {
            fn recognize(
                &mut self,
                _frame: &sister_capture::RawFrame,
            ) -> anyhow::Result<Vec<OcrBlock>> {
                self.events.borrow_mut().push("recognize".into());
                match self
                    .replies
                    .pop_front()
                    .unwrap_or(Reply::Lines(vec!["甲", "乙"]))
                {
                    Reply::Lines(lines) => Ok(lines.into_iter().map(block).collect()),
                    Reply::Fail(why) => anyhow::bail!(why),
                    Reply::TooLarge => {
                        Err(anyhow::Error::new(sister_capture::traits::OcrImageTooLarge)
                            .context("畫面 5120x1440 超過引擎上限 4096"))
                    }
                }
            }
        }
        struct FakeClock {
            now: f64,
            jumps: VecDeque<f64>,
        }
        impl Clock for FakeClock {
            fn now_ms(&mut self) -> f64 {
                let answer = self.now;
                self.now += self.jumps.pop_front().unwrap_or(10.0);
                answer
            }
        }

        fn block(text: &str) -> OcrBlock {
            OcrBlock {
                text: text.into(),
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                confidence: 1.0,
            }
        }
        fn frame(edge: u32) -> sister_capture::RawFrame {
            sister_capture::RawFrame {
                ts: 0,
                monitor: 0,
                width: edge,
                height: edge * 9 / 16,
                rgba: None,
                dhash: 0,
            }
        }
        fn shots(native: Pixels, events: &Events) -> FakeShots {
            FakeShots {
                screen: Some(ScreenSizes::from_native(native)),
                replies: VecDeque::new(),
                events: events.clone(),
            }
        }
        fn repeated(groups: Vec<Vec<&'static str>>, rounds: u32) -> VecDeque<Reply> {
            groups
                .into_iter()
                .flat_map(|lines| (0..=rounds).map(move |_| Reply::Lines(lines.clone())))
                .collect()
        }
        fn run_groups(groups: Vec<Vec<&'static str>>, rounds: u32) -> (OcrTable, String, Events) {
            let events = Events::default();
            let mut s = shots(Pixels { w: 1920, h: 1080 }, &events);
            let mut o = FakeOcr {
                replies: repeated(groups, rounds.clamp(1, MAX_OCR_ROUNDS)),
                events: events.clone(),
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let mut said: Vec<String> = Vec::new();
            let table = measure(
                &mut s,
                &mut o,
                &mut c,
                &mut |x| said.push(x.to_string()),
                Some(rounds),
            )
            .unwrap();
            (table, said.join("\n"), events)
        }

        #[test]
        fn multiset_accuracy_and_normal_missing_rows_reach_render() {
            let (table, _, _) = run_groups(
                vec![
                    vec!["確定", "確定", "取消"],
                    vec!["確定", "取消"],
                    vec!["確定", "確定", "取消"],
                    vec!["確定", "取消"],
                    vec!["確定", "確定", "取消"],
                ],
                1,
            );
            let out = render(&table, Some("zh-Hant"));
            assert!(
                out.lines().any(|l| l.contains("縮到 1568")
                    && l.split_whitespace().rev().take(2).eq(["0", "1"])),
                "{out}"
            );
            assert!(
                out.lines().any(|l| l.contains("縮到 1280")
                    && l.split_whitespace().rev().take(2).eq(["0", "0"])),
                "{out}"
            );
        }

        #[test]
        fn native_multiset_intersection_uses_the_smaller_count_in_render() {
            let (table, _, _) = run_groups(
                vec![
                    vec!["確定", "確定", "取消"],
                    vec!["確定", "取消"],
                    vec!["取消"],
                    vec!["確定", "取消"],
                    vec!["確定", "取消"],
                ],
                1,
            );
            let out = render(&table, Some("zh-Hant"));
            let complete = out.lines().find(|line| line.contains("縮到 1568")).unwrap();
            assert!(
                complete.split_whitespace().rev().take(2).eq(["0", "0"]),
                "{out}"
            );
            let missing = out.lines().find(|line| line.contains("縮到 1280")).unwrap();
            assert!(
                missing.split_whitespace().rev().take(2).eq(["0", "1"]),
                "{out}"
            );
        }

        #[test]
        fn volatile_lines_are_excluded_but_stable_runs_do_not_claim_exclusion() {
            let (table, _, _) = run_groups(
                vec![
                    vec!["甲", "12:34"],
                    vec!["甲"],
                    vec!["甲"],
                    vec!["甲"],
                    vec!["甲", "12:35"],
                ],
                1,
            );
            let out = render(&table, None);
            assert!(out.contains("另有 1+1 行"));
            assert!(
                out.lines().any(|l| l.contains("縮到 1568")
                    && l.split_whitespace().rev().take(2).eq(["0", "0"]))
            );
            let (stable, _, _) = run_groups(
                vec![vec!["甲"], vec!["甲"], vec!["甲"], vec!["甲"], vec!["甲"]],
                1,
            );
            assert!(!render(&stable, None).contains("只出現在其中一次"));
        }

        #[test]
        fn empty_baseline_hides_accuracy_but_keeps_timing() {
            let (table, _, _) = run_groups(
                vec![vec!["甲"], vec!["甲"], vec!["甲"], vec!["甲"], vec!["乙"]],
                1,
            );
            let out = render(&table, None);
            assert!(!out.contains(COL_MISSING));
            assert!(!out.contains(COL_EXTRA));
            assert!(!out.contains("0/0"));
            assert!(!out.contains("NaN"));
            assert!(out.contains("10.0 ms"));
            assert!(out.contains("原生 A 讀到 1 行、原生 B 讀到 1 行"));
            assert!(!out.contains("基準線："));
            assert!(!out.contains("只出現在其中一次"));
            let (nonempty, _, _) = run_groups(vec![vec!["甲"]; 5], 1);
            assert!(render(&nonempty, None).contains(COL_MISSING));
        }

        #[test]
        fn empty_natives_are_distinguished_from_disjoint_natives() {
            let (empty, _, _) = run_groups(vec![vec![]; 5], 1);
            let empty_out = render(&empty, None);
            assert!(
                empty_out.contains("兩次原生都沒有讀到任何一行字"),
                "{empty_out}"
            );
            assert!(!empty_out.contains("沒有一行相同"), "{empty_out}");

            let (disjoint, _, _) = run_groups(
                vec![vec!["甲"], vec!["甲"], vec!["甲"], vec!["甲"], vec!["乙"]],
                1,
            );
            let disjoint_out = render(&disjoint, None);
            assert!(disjoint_out.contains("沒有一行相同"), "{disjoint_out}");
            assert!(
                !disjoint_out.contains("兩次原生都沒有讀到任何一行字"),
                "{disjoint_out}"
            );
        }

        #[test]
        fn all_skipped_changes_reading_guide_and_prints_every_reason() {
            let events = Events::default();
            let mut s = shots(Pixels { w: 1024, h: 576 }, &events);
            let mut o = FakeOcr {
                replies: repeated(vec![vec!["甲"], vec!["甲"]], 1),
                events,
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let mut said: Vec<String> = Vec::new();
            let t = measure(
                &mut s,
                &mut o,
                &mut c,
                &mut |x| said.push(x.into()),
                Some(1),
            )
            .unwrap();
            let out = render(&t, None);
            assert_eq!(t.rows.len(), 2);
            assert_eq!(
                format!("{}\n{out}", said.join("\n"))
                    .matches("跳過縮到")
                    .count(),
                3
            );
            assert!(
                said.iter()
                    .any(|line| line == "（3 個候選被跳過，理由在表裡）")
            );
            assert!(out.contains("候選都不小於 OCR 實際大小"));
            assert!(out.contains("原生 A、原生 B 兩列"));
            assert!(out.contains("少了幾行"));
            assert!(out.contains("多了幾行"));
            assert!(out.contains("完整行字串"));
            assert!(out.contains("不等於證明同樣數量的字消失"));
            assert!(!out.contains("縮下去真的會賠掉"));
            let (with, _, _) = run_groups(vec![vec!["甲"]; 5], 1);
            let with = render(&with, None);
            assert!(with.contains("完整行字串"));
            assert!(!with.contains("縮下去真的會賠掉"));
        }

        #[test]
        fn missing_capture_keeps_row_labels_and_real_taken_count() {
            let events = Events::default();
            let mut s = shots(Pixels { w: 1920, h: 1080 }, &events);
            s.replies = VecDeque::from([
                Ok(Some(frame(1920))),
                Ok(None),
                Ok(Some(frame(1280))),
                Ok(Some(frame(1024))),
                Ok(Some(frame(1920))),
            ]);
            let mut o = FakeOcr {
                replies: repeated(vec![vec!["甲"]; 4], 1),
                events,
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let t = measure(&mut s, &mut o, &mut c, &mut |_| {}, Some(1)).unwrap();
            let out = render(&t, None);
            assert!(out.contains("縮到 1568"));
            assert!(out.contains("縮到 1280"));
            assert!(
                !out.lines()
                    .find(|l| l.contains("縮到 1568"))
                    .unwrap()
                    .contains("0.0 ms")
            );
            assert!(out.contains("抓到 4 張（計畫 5 張）"));
            assert!(out.lines().all(|line| line == line.trim_end()), "{out:?}");
        }

        #[test]
        fn rounds_average_order_ascii_and_column_constants_are_real_output() {
            assert_eq!(Rounds::choose(Some(0)).used, 1);
            assert_eq!(Rounds::choose(Some(2)).used, 2);
            assert_eq!(Rounds::choose(Some(100)).used, MAX_OCR_ROUNDS);
            let events = Events::default();
            let mut s = shots(Pixels { w: 1920, h: 1080 }, &events);
            let mut o = FakeOcr {
                replies: repeated(vec![vec!["甲"]; 5], 3),
                events: events.clone(),
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::from([10.0, 10.0, 20.0, 20.0, 30.0, 30.0]),
            };
            let mut said: Vec<String> = Vec::new();
            let t = measure(
                &mut s,
                &mut o,
                &mut c,
                &mut |x| {
                    events.borrow_mut().push("say".into());
                    said.push(x.into())
                },
                Some(3),
            )
            .unwrap();
            assert_eq!(
                match &t.rows[0].shot {
                    Shot::Read { avg_ms, .. } => *avg_ms,
                    _ => -1.0,
                },
                20.0
            );
            let log = events.borrow();
            let last_shoot = log.iter().rposition(|x| x.starts_with("shoot")).unwrap();
            let first_ocr = log.iter().position(|x| x == "recognize").unwrap();
            assert!(
                log.iter().position(|x| x == "say").unwrap()
                    < log.iter().position(|x| x.starts_with("shoot")).unwrap()
            );
            assert!(last_shoot < first_ocr);
            assert!(said[0].contains("接下來要抓 5 張"), "{said:?}");
            assert!(said[0].contains("共 20 次"), "{said:?}");
            drop(log);
            let out = render(&t, None);
            assert!(!out.contains("這次計畫"), "{out}");
            assert!(!out.contains("共跑 20 次 OCR"), "{out}");
            assert!(out.contains("1568x882"));
            assert!(!out.contains('×'));
            for col in [COL_SIZE, COL_PIXELS, COL_MS, COL_MISSING, COL_EXTRA] {
                assert!(out.matches(col).count() >= 2, "{col}: {out}");
            }
            let capped = OcrTable {
                rounds: Rounds::choose(Some(100)),
                ..t
            };
            let capped_out = render(&capped, None);
            assert!(
                capped_out.contains(&format!("上限 {MAX_OCR_ROUNDS}")),
                "{capped_out}"
            );
            let declared_limit = capped_out
                .lines()
                .find_map(|line| {
                    line.trim_start()
                        .strip_prefix("上限 ")?
                        .split_once('（')?
                        .0
                        .parse::<u32>()
                        .ok()
                })
                .expect("表頭應宣布 rounds 上限");
            assert_eq!(declared_limit, capped.rounds.used);
            assert_eq!(capped.rounds.used, MAX_OCR_ROUNDS);
            assert!(
                capped_out.contains(&format!("壓到 {MAX_OCR_ROUNDS}")),
                "{capped_out}"
            );

            let events = Events::default();
            let mut s = shots(Pixels { w: 1100, h: 700 }, &events);
            let mut o = FakeOcr {
                replies: repeated(vec![vec!["甲"]; 3], 1),
                events,
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let mut reduced: Vec<String> = Vec::new();
            measure(
                &mut s,
                &mut o,
                &mut c,
                &mut |line| reduced.push(line.into()),
                Some(1),
            )
            .unwrap();
            assert!(reduced[0].contains("接下來要抓 3 張"), "{reduced:?}");
            assert!(reduced[0].contains("共 6 次"), "{reduced:?}");
            assert!(reduced.iter().any(|line| line.starts_with("（2 個候選")));
        }

        #[test]
        fn zero_rounds_and_ocr_failure_take_guarded_paths() {
            let (zero, _, _) = run_groups(vec![vec!["甲"]; 5], 0);
            let zero_out = render(&zero, None);
            assert!(!zero_out.contains("NaN"));
            assert!(zero_out.contains("你給了 --rounds 0，提到 1"), "{zero_out}");
            assert!(!zero_out.contains("--rounds 0，壓"), "{zero_out}");
            let (high, _, _) = run_groups(vec![vec!["甲"]; 5], 100);
            let high_out = render(&high, None);
            assert!(
                high_out.contains(&format!("你給了 --rounds 100，壓到 {MAX_OCR_ROUNDS}")),
                "{high_out}"
            );
            assert!(!high_out.contains("--rounds 100，提"), "{high_out}");
            let typed = anyhow::Error::new(sister_capture::traits::OcrImageTooLarge)
                .context("畫面 5120x1440 超過引擎上限 4096");
            let typed_text = format!("{typed:#}");
            assert!(!typed_text.contains('['), "{typed_text}");
            assert!(
                typed
                    .chain()
                    .any(|cause| { cause.is::<sister_capture::traits::OcrImageTooLarge>() })
            );
            let events = Events::default();
            let mut s = shots(Pixels { w: 1024, h: 576 }, &events);
            let mut o = FakeOcr {
                replies: VecDeque::from([
                    Reply::TooLarge,
                    Reply::Lines(vec!["甲"]),
                    Reply::Lines(vec!["甲"]),
                ]),
                events,
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let t = measure(&mut s, &mut o, &mut c, &mut |_| {}, Some(1)).unwrap();
            let out = render(&t, None);
            assert!(out.contains("OCR 失敗：畫面 5120x1440 超過引擎上限 4096"));
            assert!(!out.contains('['), "{out}");
            assert!(
                !out.lines()
                    .find(|l| l.contains("原生 A"))
                    .unwrap()
                    .contains("ms")
            );
            assert!(out.contains("原生 A OCR 失敗：畫面 5120x1440 超過引擎上限 4096"));
            assert!(out.contains("請看引擎上限那則錯誤"));
            assert!(!out.contains("沒抓到或 OCR 失敗"));
            assert!(!out.contains("基準線："));
        }

        #[test]
        fn reread_instability_names_only_rows_that_actually_wobble() {
            let run = |replies: VecDeque<Reply>| {
                let events = Events::default();
                let mut s = shots(Pixels { w: 1024, h: 576 }, &events);
                let mut o = FakeOcr { replies, events };
                let mut c = FakeClock {
                    now: 0.0,
                    jumps: VecDeque::new(),
                };
                let table = measure(&mut s, &mut o, &mut c, &mut |_| {}, Some(2)).unwrap();
                render(&table, None)
            };
            let wobbled = run(VecDeque::from([
                Reply::Lines(vec!["甲"]),
                Reply::Lines(vec!["甲"]),
                Reply::Lines(vec!["乙"]),
                Reply::Lines(vec!["乙"]),
                Reply::Lines(vec!["乙"]),
                Reply::Lines(vec!["乙"]),
            ]));
            assert!(
                wobbled.contains("同一張圖重讀時結果不一致：原生 A"),
                "{wobbled}"
            );
            assert!(!wobbled.contains("不一致：原生 B"), "{wobbled}");

            let stable = run(VecDeque::from([
                Reply::Lines(vec!["甲"]),
                Reply::Lines(vec!["甲"]),
                Reply::Lines(vec!["甲"]),
                Reply::Lines(vec!["甲"]),
                Reply::Lines(vec!["甲"]),
                Reply::Lines(vec!["甲"]),
            ]));
            assert!(!stable.contains("重讀時結果不一致"), "{stable}");
        }

        #[test]
        fn plan_explains_both_skip_classes_and_their_opposites() {
            let sizes = |pixels| ScreenSizes {
                native: pixels,
                ocr: pixels,
            };
            let (_, too_large) = plan(sizes(Pixels { w: 1024, h: 576 }));
            assert!(too_large.iter().any(|s| s.why.contains("不小於 OCR")));
            let (used, too_short) = plan(sizes(Pixels { w: 1920, h: 10 }));
            assert!(too_short.iter().any(|s| s.why.contains("低於 OCR 下限")));
            assert!(used.is_empty());
            let (used, skipped) = plan(sizes(Pixels { w: 1920, h: 1080 }));
            assert_eq!(used, CANDIDATES);
            assert!(skipped.is_empty());

            let (used, skipped) = plan(sizes(Pixels { w: 4000, h: 100 }));
            assert!(used.is_empty());
            assert_eq!(skipped.len(), 3);
            assert!(skipped.iter().all(|s| s.why.contains("縮完短邊")));
        }

        #[test]
        fn default_rounds_are_real_and_do_not_accuse_the_user() {
            let events = Events::default();
            let mut s = shots(Pixels { w: 1024, h: 576 }, &events);
            let mut o = FakeOcr {
                replies: repeated(vec![vec!["甲"], vec!["甲"]], MAX_OCR_ROUNDS),
                events,
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let table = measure(&mut s, &mut o, &mut c, &mut |_| {}, None).unwrap();
            let out = render(&table, None);
            assert_eq!(table.rounds.used, MAX_OCR_ROUNDS);
            assert!(!out.contains("你給了 --rounds"), "{out}");
            assert!(out.contains(&format!("+ {MAX_OCR_ROUNDS} 次計時")), "{out}");
        }

        #[test]
        fn language_answer_and_absence_are_both_rendered() {
            let (table, _, _) = run_groups(vec![vec!["甲"]; 5], 1);
            let answered = render(&table, Some("zh-Hant-TW"));
            assert!(answered.contains("OCR 語言：zh-Hant-TW"));
            assert!(!answered.contains("引擎沒有回答語言標籤"));
            let absent = render(&table, None);
            assert!(absent.contains("OCR 語言：引擎沒有回答語言標籤"));
            assert!(!absent.contains("OCR 語言：zh-Hant-TW"));
        }

        #[test]
        fn volatile_clock_values_are_noise_not_unconditional_misreads() {
            let (table, _, _) = run_groups(
                vec![
                    vec!["甲", "12:34"],
                    vec!["甲", "12:35"],
                    vec!["甲", "12:36"],
                    vec!["甲", "12:37"],
                    vec!["甲", "12:38"],
                ],
                1,
            );
            let out = render(&table, None);
            assert!(!out.contains("是讀到了但讀錯"), "{out}");
            assert!(out.contains("會自己變的行也可能落在「多了幾行」"), "{out}");
            assert!(out.contains("雜訊底線"), "{out}");

            let (stable, _, _) = run_groups(vec![vec!["甲"]; 5], 1);
            let stable = render(&stable, None);
            assert!(!stable.contains("另有 "), "{stable}");
            assert!(!stable.contains("只出現在其中一次"), "{stable}");
        }

        #[test]
        fn capture_failures_and_missing_labels_reach_the_table_and_summary() {
            let events = Events::default();
            let mut s = shots(Pixels { w: 1920, h: 1080 }, &events);
            s.replies = VecDeque::from([
                Err(anyhow::anyhow!("GetDC 爆掉")),
                Ok(None),
                Ok(Some(frame(1280))),
                Ok(Some(frame(1024))),
                Ok(Some(frame(1920))),
            ]);
            let mut o = FakeOcr {
                replies: repeated(vec![vec!["甲"]; 3], 1),
                events,
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let out = render(
                &measure(&mut s, &mut o, &mut c, &mut |_| {}, Some(1)).unwrap(),
                None,
            );
            let native = out.lines().find(|l| l.contains("原生 A")).unwrap();
            assert!(native.contains("抓圖失敗：GetDC 爆掉"), "{out}");
            assert!(out.contains("沒抓到：原生 A、縮到 1568。"), "{out}");
        }

        #[test]
        fn normalization_multisets_and_last_reading_reach_public_output() {
            let (trimmed, _, _) = run_groups(vec![vec!["  甲  ", "", "   ", "甲"]; 5], 1);
            assert!(render(&trimmed, None).contains("基準線：兩次原生都讀到的 2 行"));

            let (duplicates, _, _) = run_groups(vec![vec!["確定", "確定", "取消"]; 5], 1);
            assert!(render(&duplicates, None).contains("基準線：兩次原生都讀到的 3 行"));

            let events = Events::default();
            let mut s = shots(Pixels { w: 1024, h: 576 }, &events);
            let mut o = FakeOcr {
                replies: VecDeque::from([
                    Reply::Lines(vec!["熱身 A"]),
                    Reply::Lines(vec!["不同 A"]),
                    Reply::Lines(vec!["共同"]),
                    Reply::Lines(vec!["熱身 B"]),
                    Reply::Lines(vec!["不同 B"]),
                    Reply::Lines(vec!["共同"]),
                ]),
                events,
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let out = render(
                &measure(&mut s, &mut o, &mut c, &mut |_| {}, Some(2)).unwrap(),
                None,
            );
            assert!(out.contains("基準線：兩次原生都讀到的 1 行"), "{out}");
        }

        #[test]
        fn timing_footer_distinguishes_zero_one_and_many_measurements() {
            let run = |replies: VecDeque<anyhow::Result<Option<sister_capture::RawFrame>>>,
                       ocr_replies: VecDeque<Reply>| {
                let events = Events::default();
                let mut s = shots(Pixels { w: 1920, h: 1080 }, &events);
                s.replies = replies;
                let mut o = FakeOcr {
                    replies: ocr_replies,
                    events,
                };
                let mut c = FakeClock {
                    now: 0.0,
                    jumps: VecDeque::new(),
                };
                render(
                    &measure(&mut s, &mut o, &mut c, &mut |_| {}, Some(1)).unwrap(),
                    None,
                )
            };
            let none = run((0..5).map(|_| Ok(None)).collect(), VecDeque::new());
            assert!(none.contains("沒有量到任何毫秒"), "{none}");
            assert!(none.contains("請解鎖"), "{none}");
            assert!(!none.contains("sister doctor"), "{none}");
            assert!(!none.contains("毫秒仍可比較"));
            let one = run(
                VecDeque::from([
                    Ok(Some(frame(1920))),
                    Ok(None),
                    Ok(None),
                    Ok(None),
                    Ok(None),
                ]),
                VecDeque::new(),
            );
            assert!(
                one.contains("只量到一個毫秒數，沒有第二個數字可以比較"),
                "{one}"
            );
            let ocr_only = run(
                VecDeque::new(),
                (0..5).map(|_| Reply::Fail("RecognizeAsync 爆掉")).collect(),
            );
            assert!(
                ocr_only.contains("畫面抓得到，但 OCR 每次都失敗"),
                "{ocr_only}"
            );
            assert!(ocr_only.contains("sister doctor"), "{ocr_only}");
            assert!(!ocr_only.contains("請解鎖"), "{ocr_only}");
            let mixed = run(
                VecDeque::from([
                    Ok(None),
                    Ok(Some(frame(1568))),
                    Ok(None),
                    Ok(Some(frame(1024))),
                    Ok(None),
                ]),
                VecDeque::from([
                    Reply::Fail("RecognizeAsync 爆掉"),
                    Reply::Fail("RecognizeAsync 爆掉"),
                ]),
            );
            assert!(mixed.contains("有些畫面沒抓到"), "{mixed}");
            assert!(mixed.contains("OCR 失敗"), "{mixed}");
            assert!(mixed.contains("請先解鎖"), "{mixed}");
            assert!(mixed.contains("sister doctor"), "{mixed}");
            let many = run(VecDeque::new(), VecDeque::new());
            assert!(many.contains("量到 5 個毫秒數，可以比較速度"), "{many}");
        }

        #[test]
        fn ocr_error_kinds_taken_count_and_fact_lines_are_truthful() {
            let events = Events::default();
            let mut s = shots(Pixels { w: 1024, h: 576 }, &events);
            let mut o = FakeOcr {
                replies: VecDeque::from([
                    Reply::Fail("RecognizeAsync: 0x8007000E"),
                    Reply::Lines(vec!["甲"; 40]),
                    Reply::Lines(vec!["甲"; 40]),
                ]),
                events,
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            let table = measure(&mut s, &mut o, &mut c, &mut |_| {}, Some(1)).unwrap();
            let out = render(&table, None);
            assert!(
                out.lines()
                    .find(|l| l.contains("原生 A"))
                    .unwrap()
                    .contains("OCR 失敗：RecognizeAsync"),
                "{out}"
            );
            assert!(out.contains("原生 B 讀到 40 行"), "{out}");
            assert!(!out.contains("請看引擎上限那則錯誤"), "{out}");
            assert_eq!(table.taken, 2);
            assert!(!out.contains("沒抓到：。"));
        }

        #[test]
        fn screen_size_absence_and_native_ocr_order_are_observable() {
            let events = Events::default();
            let mut missing = FakeShots {
                screen: None,
                replies: VecDeque::new(),
                events: events.clone(),
            };
            let mut o = FakeOcr {
                replies: VecDeque::new(),
                events: events.clone(),
            };
            let mut c = FakeClock {
                now: 0.0,
                jumps: VecDeque::new(),
            };
            assert!(measure(&mut missing, &mut o, &mut c, &mut |_| {}, None).is_none());

            let derived = ScreenSizes::from_native(Pixels { w: 5120, h: 1440 });
            assert_eq!(derived.native, Pixels { w: 5120, h: 1440 });
            assert_eq!(derived.ocr, Pixels { w: 4096, h: 1152 });
            assert!(derived.ocr.w <= derived.native.w && derived.ocr.h <= derived.native.h);
            assert!(derived.ocr.long() <= sister_capture::scale::OCR_LONG_EDGE);

            let mut clipped = FakeShots {
                screen: Some(derived),
                replies: VecDeque::new(),
                events: events.clone(),
            };
            let table = measure(&mut clipped, &mut o, &mut c, &mut |_| {}, Some(1)).unwrap();
            let out = render(&table, None);
            assert!(out.contains("螢幕原始 5120x1440"), "{out}");
            assert!(
                out.contains("OCR 用的大小 4096x1152，長邊上限 4096"),
                "{out}"
            );
            assert!(!out.contains("原生 4096x1152"), "{out}");

            let (_, clipped_skip) = plan(ScreenSizes {
                native: Pixels { w: 5120, h: 10 },
                ocr: Pixels { w: 4096, h: 8 },
            });
            assert!(
                clipped_skip
                    .iter()
                    .all(|s| s.why.contains("4096") && s.why.contains("上限夾")),
                "{:?}",
                clipped_skip.iter().map(|s| &s.why).collect::<Vec<_>>()
            );

            let (normal, _, _) = run_groups(vec![vec!["甲"]; 5], 1);
            let normal = render(&normal, None);
            assert!(normal.contains("OCR 縮圖表（原生 1920x1080）"));
            assert!(!normal.contains("OCR 用的大小"));
        }

        #[test]
        fn asymmetric_volatility_and_duplicate_extras_keep_counts() {
            let (volatile, _, _) = run_groups(
                vec![
                    vec!["甲", "A", "B"],
                    vec!["甲"],
                    vec!["甲"],
                    vec!["甲"],
                    vec!["甲", "C"],
                ],
                1,
            );
            assert!(render(&volatile, None).contains("另有 2+1 行"));

            let (extra, _, _) = run_groups(
                vec![
                    vec!["甲"],
                    vec!["甲", "乙", "乙"],
                    vec!["甲"],
                    vec!["甲"],
                    vec!["甲"],
                ],
                1,
            );
            let out = render(&extra, None);
            let row = out.lines().find(|l| l.contains("縮到 1568")).unwrap();
            assert!(row.split_whitespace().last() == Some("2"), "{out}");
        }

        #[test]
        fn one_empty_native_is_not_called_disjoint_and_guide_indent_matches() {
            let (table, _, _) = run_groups(
                vec![vec![], vec!["甲"], vec!["甲"], vec!["甲"], vec!["甲"]],
                1,
            );
            let out = render(&table, None);
            assert!(out.contains("其中一次原生沒有讀到字"), "{out}");
            assert!(!out.contains("兩邊沒有一行相同"), "{out}");

            let (accuracy, _, _) = run_groups(vec![vec!["甲"]; 5], 1);
            let out = render(&accuracy, None);
            let heading = out
                .lines()
                .find(|line| line.starts_with("怎麼讀："))
                .expect("輸出裡應有讀法標題");
            let heading_prefix = heading.split_once('「').unwrap().0;
            let expected = crate::fmt::display_width(heading_prefix);
            for line in out
                .lines()
                .filter(|l| l.contains("這一列沒有完全相同命中") || l.contains("時鐘、通知"))
            {
                let actual = line.chars().take_while(|c| *c == ' ').count();
                assert_eq!(actual, expected, "{line:?}\n{out}");
            }
        }

        #[test]
        fn all_skipped_public_guide_names_too_short_and_mixed_reasons() {
            let render_size = |pixels: Pixels| {
                let events = Events::default();
                let mut s = shots(pixels, &events);
                let mut o = FakeOcr {
                    replies: repeated(vec![vec!["甲"], vec!["甲"]], 1),
                    events,
                };
                let mut c = FakeClock {
                    now: 0.0,
                    jumps: VecDeque::new(),
                };
                render(
                    &measure(&mut s, &mut o, &mut c, &mut |_| {}, Some(1)).unwrap(),
                    None,
                )
            };
            let short = render_size(Pixels { w: 1920, h: 10 });
            assert!(short.contains("候選縮完的短邊都低於 OCR 下限"), "{short}");
            assert!(!short.contains("沒有比它更小"), "{short}");

            let mixed = render_size(Pixels { w: 1400, h: 43 });
            assert!(
                mixed.contains("候選不是不夠小，就是縮完短邊低於 OCR 下限"),
                "{mixed}"
            );
            assert!(mixed.contains("原生 A、原生 B 兩列"), "{mixed}");
            assert!(
                mixed.contains("少了幾行") && mixed.contains("多了幾行"),
                "{mixed}"
            );
        }
    }
}

pub mod replay {
    use super::*;
    use sister_capture::{Recorder, ReplayBackend, Scenario, Tick};
    use sister_core::config::Config;
    use sister_core::eval::{
        AnnotationPreview, EvalReport, ExpectedOutcome, Fraction, QuestionSet, annotation_previews,
        evidence_surfaces,
    };
    use sister_core::moments::{ConfirmPrivateTextReviewed, MomentLabel, MomentSet, SpeakCategory};
    use sister_core::replay::{Corpus, DraftCorpus, ReviewStatus};
    use std::fs::OpenOptions;
    use std::io::{BufRead, Seek, SeekFrom, Write};
    use std::str::FromStr;

    /// 真實資料的安全出口：DB API 直接回 [`DraftCorpus`](sister_core::replay::DraftCorpus)，
    /// 呼叫端碰不到尚未去敏的中間 corpus。
    pub fn export_corpus(
        data_dir: &Path,
        last: &str,
        output: Option<&Path>,
        questions_output: Option<&Path>,
        name: Option<&str>,
    ) -> Result<()> {
        let span = parse_span(last)?;
        let to = sister_core::now_ms();
        let from = to
            .checked_sub(span)
            .context("replay 匯出區間超出可表示的時間")?;
        let name = name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("recent-{}", last.trim()));

        let db = open_existing(data_dir)?;
        let draft = db.export_replay(&name, from, to)?;
        let path = output
            .map(Path::to_path_buf)
            .unwrap_or_else(|| next_draft_path(data_dir));

        let question_set = match questions_output {
            Some(question_path) => {
                anyhow::ensure!(
                    question_path != path,
                    "corpus 和 question set 不可寫到同一個檔案"
                );
                anyhow::ensure!(
                    !path.exists() && !question_path.exists(),
                    "輸出檔已存在；不會覆寫既有檔案"
                );
                let rows = db.query_log_between(from, to)?;
                Some(QuestionSet::draft_from_query_log(
                    &format!("{name}-queries"),
                    draft.as_corpus(),
                    from,
                    &rows,
                )?)
            }
            None => None,
        };
        write_new_json(&path, &draft)?;
        if let (Some(question_path), Some(question_set)) = (questions_output, &question_set) {
            write_new_question_set(question_path, question_set)?;
        }

        let corpus = draft.as_corpus();
        let redactions = &corpus.redactions;
        println!(
            "完成：{} 個事件、{:.1} 秒 → {}",
            corpus.events.len(),
            corpus.duration_ms as f64 / 1000.0,
            path.display()
        );
        println!(
            "  自動去敏 {} 處（金額 {}、電話 {}、email {}、類 ID {}、秘密 {}）",
            redactions.total(),
            redactions.money,
            redactions.phone,
            redactions.email,
            redactions.id_like,
            redactions.secrets
        );
        println!("  沒有圖片、圖片路徑或資料庫 row id。");
        println!(
            "  ⚠  這是 private Draft（review: draft），只能留在本機；人工逐項審查並改成 reviewed 前不要分享。"
        );
        if let (Some(question_path), Some(question_set)) = (questions_output, question_set) {
            println!(
                "  題庫草稿：{} 題 query log → {}",
                question_set.questions.len(),
                question_path.display()
            );
            println!(
                "  ⚠  題目保留你輸入的原話、沒有自動去敏；expected 全是 null，人工填成 answer/no_answer 前不能評測。"
            );
        }
        Ok(())
    }

    pub fn import_corpus(
        data_dir: &Path,
        corpus_path: &Path,
        dry_run: bool,
        days_ago: f64,
        start: Option<i64>,
    ) -> Result<()> {
        anyhow::ensure!(
            days_ago.is_finite() && days_ago >= 0.0,
            "--days-ago 必須是大於或等於 0 的有限數字"
        );
        let bytes = std::fs::read(corpus_path)
            .with_context(|| format!("read {}", corpus_path.display()))?;
        let corpus: Corpus = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse replay corpus {}", corpus_path.display()))?;
        corpus.validate()?;

        let origin = match start {
            Some(origin) => origin,
            None => {
                let ago = (days_ago * 86_400_000.0).round();
                anyhow::ensure!(ago <= i64::MAX as f64, "--days-ago 太大，無法換成時間戳");
                sister_core::now_ms()
                    .checked_sub(corpus.duration_ms)
                    .and_then(|now| now.checked_sub(ago as i64))
                    .context("replay 匯入起點超出可表示的時間")?
            }
        };
        let mut db = if dry_run {
            Db::open_in_memory()?
        } else {
            let path = crate::db_path(data_dir);
            Db::open(&path).with_context(|| format!("open {}", path.display()))?
        };

        println!(
            "▶ 匯入「{}」：{} 個事件、{:.1} 秒{}",
            corpus.name,
            corpus.events.len(),
            corpus.duration_ms as f64 / 1000.0,
            if dry_run {
                "（dry-run，不寫入）"
            } else {
                ""
            }
        );
        println!("  時間軸起點：{}", crate::fmt::timestamp(origin));
        if corpus.review == ReviewStatus::Draft {
            println!(
                "  ⚠  這份 corpus 還是 private Draft；本機驗證可以，人工審查成 Reviewed 前不要分享。"
            );
        }

        let imported = db.import_replay(&corpus, origin)?;
        println!(
            "完成：匯入 {} 個事件，其中 {} 張文字畫面；從文字重建 {} 個 L1 事實。",
            imported.events, imported.frames, imported.facts
        );
        if dry_run {
            println!("  dry-run 已走完整個 DB／FTS／L1 寫入流程，結果隨行程結束丟棄。");
        }
        Ok(())
    }

    pub fn question_status(corpus_path: &Path, questions_path: &Path) -> Result<()> {
        let (corpus, questions) = read_corpus_and_questions(corpus_path, questions_path)?;
        render_question_status(&corpus, &questions);
        Ok(())
    }

    pub fn annotate_questions(
        corpus_path: &Path,
        questions_path: &Path,
        output: &Path,
        k: usize,
        all: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            output != corpus_path && output != questions_path,
            "--to 必須是另一個新檔案；不會原地改寫 corpus 或題庫"
        );
        let (corpus, questions) = read_corpus_and_questions(corpus_path, questions_path)?;
        anyhow::ensure!(
            questions.review == ReviewStatus::Draft,
            "Reviewed question set 不可直接重新標註；請從審查前的 Draft 產生新版"
        );
        // 先確認目的地真的建得起來，再讓人花時間逐題標。裡面先放一份合法的
        // 原始 Draft；之後每完成一題就 checkpoint。在 prompt 等下一個答案時
        // Ctrl-C，仍然找得到上一題完成後的版本。
        let mut draft_output = QuestionDraftOutput::reserve(output, &questions)?;
        let previews = annotation_previews(&corpus, &questions, k)?;

        println!("⚠  這裡會顯示沒有自動去敏的題目原話；只在私下的終端操作。");
        render_question_status(&corpus, &questions);
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let (annotated, changed) = annotate_with_io(
            &corpus,
            &questions,
            &previews,
            all,
            &mut stdin.lock(),
            &mut stdout.lock(),
            &mut |questions| draft_output.checkpoint(questions),
        )?;
        if changed == 0 {
            draft_output.discard()?;
            println!("沒有新增或更正標註；沒有建立輸出檔。");
            return Ok(());
        }

        debug_assert_eq!(draft_output.last_questions, annotated);
        draft_output.finish();
        println!("完成：這次寫下 {} 題標註 → {}", changed, output.display());
        render_question_status(&corpus, &annotated);
        println!("  仍是 private Draft；全部標完後再跑 `replay questions review`。");
        Ok(())
    }

    pub fn review_questions(
        corpus_path: &Path,
        questions_path: &Path,
        output: &Path,
        confirmed_private_text_reviewed: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            confirmed_private_text_reviewed,
            "題目原話沒有自動去敏。逐題確認問題、答案與 evidence 都可分享後，重跑並加上 --confirm-private-text-reviewed"
        );
        anyhow::ensure!(
            output != corpus_path && output != questions_path,
            "--to 必須是另一個新檔案；不會原地改寫 corpus 或題庫"
        );
        let (corpus, questions) = read_corpus_and_questions(corpus_path, questions_path)?;
        let reviewed = questions.reviewed(&corpus)?;
        write_new_question_set(output, &reviewed)?;
        println!("完成：Reviewed question set → {}", output.display());
        println!(
            "  這代表你已人工確認題目原話、答案與 evidence；corpus 的 {:?} 狀態仍是另一份獨立審查。",
            corpus.review
        );
        Ok(())
    }

    fn read_corpus(path: &Path) -> Result<Corpus> {
        let corpus_bytes =
            std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let corpus: Corpus = serde_json::from_slice(&corpus_bytes)
            .with_context(|| format!("parse replay corpus {}", path.display()))?;
        corpus.validate()?;
        Ok(corpus)
    }

    fn read_corpus_and_questions(
        corpus_path: &Path,
        questions_path: &Path,
    ) -> Result<(Corpus, QuestionSet)> {
        let corpus = read_corpus(corpus_path)?;
        let question_bytes = std::fs::read(questions_path)
            .with_context(|| format!("read {}", questions_path.display()))?;
        let questions: QuestionSet = serde_json::from_slice(&question_bytes)
            .with_context(|| format!("parse question set {}", questions_path.display()))?;
        questions.validate(&corpus)?;
        Ok((corpus, questions))
    }

    fn read_corpus_and_moments(
        corpus_path: &Path,
        moments_path: &Path,
    ) -> Result<(Corpus, MomentSet)> {
        let corpus = read_corpus(corpus_path)?;
        let moment_bytes = std::fs::read(moments_path)
            .with_context(|| format!("read {}", moments_path.display()))?;
        let moments: MomentSet = serde_json::from_slice(&moment_bytes)
            .with_context(|| format!("parse moment set {}", moments_path.display()))?;
        moments.validate(&corpus)?;
        Ok((corpus, moments))
    }

    pub fn draft_moments(corpus_path: &Path, output: &Path) -> Result<()> {
        anyhow::ensure!(
            output != corpus_path,
            "--to 必須是另一個新檔案；不會原地改寫 corpus"
        );
        let corpus = read_corpus(corpus_path)?;
        let name = format!("{}-moments", corpus.name);
        let set = MomentSet::draft_from_corpus(&name, &corpus)?;
        write_new_moment_set(output, &set)?;
        println!("完成：時刻集草稿 → {}", output.display());
        render_moment_status(&corpus, &set);
        println!("  仍是 private Draft；標完後再跑 `replay moments review`。");
        Ok(())
    }

    pub fn moment_status(corpus_path: &Path, moments_path: &Path, json: bool) -> Result<()> {
        let (corpus, moments) = read_corpus_and_moments(corpus_path, moments_path)?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&moments.status_view(&corpus))?
            );
            return Ok(());
        }
        render_moment_status(&corpus, &moments);
        Ok(())
    }

    pub fn annotate_moments(
        corpus_path: &Path,
        moments_path: &Path,
        output: &Path,
        all: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            output != corpus_path && output != moments_path,
            "--to 必須是另一個新檔案；不會原地改寫 corpus 或時刻集"
        );
        let (corpus, moments) = read_corpus_and_moments(corpus_path, moments_path)?;
        anyhow::ensure!(
            moments.review == ReviewStatus::Draft,
            "Reviewed moment set 不可直接重新標註；請從審查前的 Draft 產生新版"
        );
        let mut draft_output = MomentDraftOutput::reserve(output, &moments)?;
        println!("⚠  這裡會顯示沒有自動去敏的畫面文字與標註原話；只在私下的終端操作。");
        render_moment_status(&corpus, &moments);
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let (annotated, changed) = annotate_moments_with_io(
            &corpus,
            &moments,
            all,
            &mut stdin.lock(),
            &mut stdout.lock(),
            &mut |moments| draft_output.checkpoint(moments),
        )?;
        if changed == 0 {
            draft_output.discard()?;
            println!("沒有新增或更正標註；沒有建立輸出檔。");
            return Ok(());
        }

        debug_assert_eq!(draft_output.last_moments, annotated);
        draft_output.finish();
        println!("完成：這次寫下 {} 個標註 → {}", changed, output.display());
        render_moment_status(&corpus, &annotated);
        println!("  仍是 private Draft；全部標完後再跑 `replay moments review`。");
        Ok(())
    }

    pub fn review_moments(
        corpus_path: &Path,
        moments_path: &Path,
        output: &Path,
        confirmed: ConfirmPrivateTextReviewed,
    ) -> Result<()> {
        anyhow::ensure!(
            confirmed.is_confirmed(),
            "提醒原文與 why 沒有自動去敏。逐個確認承諾、該講／不該講的理由都可分享後，重跑並加上 --confirm-private-text-reviewed"
        );
        anyhow::ensure!(
            output != corpus_path && output != moments_path,
            "--to 必須是另一個新檔案；不會原地改寫 corpus 或時刻集"
        );
        let (corpus, moments) = read_corpus_and_moments(corpus_path, moments_path)?;
        let reviewed = moments.reviewed(&corpus)?;
        write_new_moment_set(output, &reviewed)?;
        println!("完成：Reviewed moment set → {}", output.display());
        println!(
            "  這代表你已人工確認提醒原文、why 與 evidence；corpus 的 {:?} 狀態仍是另一份獨立審查。",
            corpus.review
        );
        Ok(())
    }

    fn render_moment_status(corpus: &Corpus, moments: &MomentSet) {
        let counts = moments.counts();
        println!(
            "時刻集「{}」（{:?}）：總共 {} 個時刻。",
            moments.name, moments.review, counts.total
        );
        println!("  未標：{}", counts.unlabeled);
        println!(
            "  已標承諾：{}（其中有講明時間：{}）",
            counts.commitments, counts.commitments_with_due
        );
        println!("  已標該講：{}", counts.should_speak);
        println!("  已標不該講：{}", counts.should_stay_quiet);
        println!(
            "  綁定 corpus「{}」（{:?}），fingerprint 已核對。",
            corpus.name, corpus.review
        );
    }

    fn annotate_moments_with_io(
        corpus: &Corpus,
        moments: &MomentSet,
        all: bool,
        input: &mut impl BufRead,
        output: &mut impl Write,
        checkpoint: &mut impl FnMut(&MomentSet) -> Result<()>,
    ) -> Result<(MomentSet, usize)> {
        let selected: Vec<String> = moments
            .moments
            .iter()
            .filter(|moment| all || moment.label.is_none())
            .map(|moment| moment.id.clone())
            .collect();
        let mut next = moments.clone();
        let mut changed = 0usize;
        let mut quit = false;

        if selected.is_empty() {
            writeln!(
                output,
                "全部時刻都已有標註；要更正請加 --all。也可以用 add@ 新增機器沒提的時刻。"
            )?;
        }

        for (position, moment_id) in selected.iter().enumerate() {
            if quit {
                break;
            }
            let (at_ms, candidate, label) = {
                let Some(moment) = next.moments.iter().find(|moment| moment.id == *moment_id)
                else {
                    continue;
                };
                (moment.at_ms, moment.candidate, moment.label.clone())
            };
            writeln!(
                output,
                "\n[{}/{}] {}  +{} ms\n候選理由：{}",
                position + 1,
                selected.len(),
                moment_id,
                at_ms,
                candidate.describe()
            )?;
            if let Some(moment) = next.moments.iter().find(|moment| moment.id == *moment_id) {
                render_moment_evidence(output, corpus, moment)?;
            }
            match &label {
                None => writeln!(output, "現有標註：未標")?,
                Some(label) => writeln!(output, "現有標註：{}", moment_label_summary(label))?,
            }
            writeln!(
                output,
                "輸入：c <提醒>｜c@<ms> <提醒>｜s <類別> <原因>｜q <原因>｜add@<ms> …｜skip｜?｜w"
            )?;

            loop {
                write!(output, "> ")?;
                output.flush()?;
                let mut line = String::new();
                if input.read_line(&mut line)? == 0 {
                    quit = true;
                    break;
                }
                let command = line.trim();
                if command == "w" {
                    quit = true;
                    break;
                }
                if command == "skip" {
                    break;
                }
                if command == "?" {
                    write_moment_annotate_help(output)?;
                    continue;
                }
                match apply_moment_annotate_command(
                    &mut next,
                    corpus,
                    moment_id,
                    command,
                    output,
                    checkpoint,
                    &mut changed,
                )? {
                    AnnotateOutcome::Labeled => break,
                    AnnotateOutcome::Continue => {}
                }
            }
        }

        if !quit {
            writeln!(output, "輸入 add@<ms> 新增機器沒提的時刻，或 ?／w。")?;
            loop {
                write!(output, "> ")?;
                output.flush()?;
                let mut line = String::new();
                if input.read_line(&mut line)? == 0 {
                    break;
                }
                let command = line.trim();
                if command == "w" {
                    break;
                }
                if command == "?" {
                    write_moment_annotate_help(output)?;
                    continue;
                }
                if command == "skip" {
                    writeln!(output, "這裡沒有下一個機器候選；要離開請輸入 w。")?;
                    continue;
                }
                apply_moment_annotate_command(
                    &mut next,
                    corpus,
                    "",
                    command,
                    output,
                    checkpoint,
                    &mut changed,
                )?;
            }
        }
        Ok((next, changed))
    }

    enum AnnotateOutcome {
        Labeled,
        Continue,
    }

    fn write_moment_annotate_help(output: &mut impl Write) -> Result<()> {
        writeln!(
            output,
            "c <提醒內容>           標承諾（沒講時間）\n\
             c@<毫秒> <提醒內容>    標帶相對時間的承諾\n\
             s <類別> <原因>        標該講；類別：{}（或 a–e）\n\
             q <原因>               標不該講\n\
             add@<毫秒> c <提醒>    新增並標承諾（沒講時間）\n\
             add@<毫秒> c@<due> <提醒>  新增並標帶相對時間的承諾\n\
             add@<毫秒> s <類別> <原因> 新增並標該講\n\
             add@<毫秒> q <原因>    新增並標不該講\n\
             skip                   跳過\n\
             w                      存檔離開",
            SpeakCategory::names("、")
        )?;
        Ok(())
    }

    fn apply_moment_annotate_command(
        next: &mut MomentSet,
        corpus: &Corpus,
        current_id: &str,
        command: &str,
        output: &mut impl Write,
        checkpoint: &mut impl FnMut(&MomentSet) -> Result<()>,
        changed: &mut usize,
    ) -> Result<AnnotateOutcome> {
        match parse_add_moment(command) {
            Ok(Some((at_ms, label))) => {
                let added_id = next.next_hand_picked_id();
                match next.with_hand_picked(corpus, at_ms, label) {
                    Ok(labeled) => {
                        *next = labeled;
                        checkpoint(next)?;
                        *changed += 1;
                        let added = next
                            .moments
                            .iter()
                            .find(|moment| moment.id == added_id)
                            .expect("with_hand_picked 剛寫下的 id 必須找得到");
                        writeln!(
                            output,
                            "已加入 {}  +{} ms\n候選理由：{}",
                            added.id,
                            added.at_ms,
                            added.candidate.describe()
                        )?;
                        render_hand_picked_attachment(output, corpus, added)?;
                        if let Some(label) = &added.label {
                            writeln!(output, "現有標註：{}", moment_label_summary(label))?;
                        }
                        return Ok(AnnotateOutcome::Continue);
                    }
                    Err(error) => {
                        writeln!(output, "不能新增這個時刻：{error:#}")?;
                        return Ok(AnnotateOutcome::Continue);
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                writeln!(output, "{error}")?;
                return Ok(AnnotateOutcome::Continue);
            }
        }

        if current_id.is_empty() {
            writeln!(output, "不認得；請輸入 add@、? 或 w。")?;
            return Ok(AnnotateOutcome::Continue);
        }

        match parse_moment_label(command) {
            Ok(Some(label)) => {
                let replace = next
                    .moments
                    .iter()
                    .find(|moment| moment.id == current_id)
                    .is_some_and(|moment| moment.label.is_some());
                match next.with_label(corpus, current_id, label, replace) {
                    Ok(labeled) => {
                        *next = labeled;
                        checkpoint(next)?;
                        *changed += 1;
                        Ok(AnnotateOutcome::Labeled)
                    }
                    Err(error) => {
                        writeln!(output, "不能存這個標註：{error:#}")?;
                        Ok(AnnotateOutcome::Continue)
                    }
                }
            }
            Ok(None) => {
                writeln!(output, "不認得；請輸入 c、c@、s、q、add@、skip、? 或 w。")?;
                Ok(AnnotateOutcome::Continue)
            }
            Err(error) => {
                writeln!(output, "{error}")?;
                Ok(AnnotateOutcome::Continue)
            }
        }
    }

    fn parse_moment_label(command: &str) -> Result<Option<MomentLabel>, String> {
        if let Some(rest) = command.strip_prefix("c@") {
            let Some((ms, remind)) = rest.split_once(char::is_whitespace) else {
                return Err("格式是：c@<毫秒> 提醒內容".into());
            };
            let due_at_ms = ms
                .parse::<sister_core::Millis>()
                .map_err(|_| format!("c@ 後面的毫秒必須是整數，不是 {ms}"))?;
            let remind = remind.trim();
            if remind.is_empty() {
                return Err("承諾 remind 不可為空。".into());
            }
            return Ok(Some(MomentLabel::Commitment {
                remind: remind.to_string(),
                due_at_ms: Some(due_at_ms),
            }));
        }
        if let Some(remind) = command.strip_prefix("c ") {
            let remind = remind.trim();
            if remind.is_empty() {
                return Err("承諾 remind 不可為空。".into());
            }
            return Ok(Some(MomentLabel::Commitment {
                remind: remind.to_string(),
                due_at_ms: None,
            }));
        }
        if let Some(rest) = command.strip_prefix("s ") {
            let Some((category, why)) = rest.trim().split_once(char::is_whitespace) else {
                return Err(format!(
                    "格式是：s <類別> 原因（類別：{}）",
                    SpeakCategory::names("、")
                ));
            };
            let category = SpeakCategory::from_str(category)?;
            let why = why.trim();
            if why.is_empty() {
                return Err("該講的 why 不可為空。".into());
            }
            return Ok(Some(MomentLabel::ShouldSpeak {
                category,
                why: why.to_string(),
            }));
        }
        if let Some(why) = command.strip_prefix("q ") {
            let why = why.trim();
            if why.is_empty() {
                return Err("不該講的 why 不可為空。".into());
            }
            return Ok(Some(MomentLabel::ShouldStayQuiet {
                why: why.to_string(),
            }));
        }
        Ok(None)
    }

    fn parse_add_moment(
        command: &str,
    ) -> Result<Option<(sister_core::Millis, MomentLabel)>, String> {
        let Some(rest) = command.strip_prefix("add@") else {
            return Ok(None);
        };
        let rest = rest.trim();
        let Some((ms, label_cmd)) = rest.split_once(char::is_whitespace) else {
            return Err(
                "格式是：add@<毫秒> c <提醒>｜add@<毫秒> c@<due> <提醒>｜add@<毫秒> s <類別> <原因>｜add@<毫秒> q <原因>"
                    .into(),
            );
        };
        let at_ms = ms
            .parse::<sister_core::Millis>()
            .map_err(|_| format!("add@ 後面的毫秒必須是整數，不是 {ms}"))?;
        match parse_moment_label(label_cmd.trim()) {
            Ok(Some(label)) => Ok(Some((at_ms, label))),
            Ok(None) => Err("add@ 後面要接 c、c@、s 或 q（一步建立並標好，不要只寫時間）".into()),
            Err(error) => Err(error),
        }
    }

    fn render_moment_evidence(
        output: &mut impl Write,
        corpus: &Corpus,
        moment: &sister_core::moments::LabeledMoment,
    ) -> Result<()> {
        if moment.evidence.is_empty() {
            writeln!(output, "沒有 evidence event。")?;
            return Ok(());
        }
        for (shown, reference) in moment.evidence.iter().enumerate() {
            if shown == 5 {
                writeln!(
                    output,
                    "  …另有 {} 個 evidence event 未列出",
                    moment.evidence.len() - shown
                )?;
                break;
            }
            let Some(event) = corpus.events.get(reference.event_index) else {
                writeln!(
                    output,
                    "evidence event {} 不在 corpus 裡",
                    reference.event_index
                )?;
                continue;
            };
            let surfaces = evidence_surfaces(event);
            if surfaces.is_empty() {
                writeln!(output, "event {} 沒有可見文字表面", reference.event_index)?;
                continue;
            }
            writeln!(output, "event {} 的可見表面：", reference.event_index)?;
            for (kind, text) in surfaces {
                writeln!(output, "  {}｜{}", kind.as_str(), concise(&text, 220))?;
            }
        }
        Ok(())
    }

    fn render_hand_picked_attachment(
        output: &mut impl Write,
        corpus: &Corpus,
        moment: &sister_core::moments::LabeledMoment,
    ) -> Result<()> {
        let Some(reference) = moment.evidence.first() else {
            writeln!(output, "沒有掛上 evidence——這不該發生")?;
            return Ok(());
        };
        match corpus.events.get(reference.event_index) {
            Some(event) => writeln!(
                output,
                "掛到 event {}（該 event 的 at_ms={}；時刻 at_ms={}）",
                reference.event_index,
                event.at_ms(),
                moment.at_ms
            )?,
            None => writeln!(
                output,
                "掛到 event {}，但那個 event 不在 corpus 裡",
                reference.event_index
            )?,
        }
        render_moment_evidence(output, corpus, moment)?;
        Ok(())
    }

    fn moment_label_summary(label: &MomentLabel) -> String {
        match label {
            MomentLabel::Commitment { remind, due_at_ms } => match due_at_ms {
                Some(due) => format!("承諾「{remind}」due_at_ms={due}"),
                None => format!("承諾「{remind}」（沒講時間）"),
            },
            MomentLabel::ShouldSpeak { category, why } => {
                format!("該講 {} — {why}", category.as_str())
            }
            MomentLabel::ShouldStayQuiet { why } => format!("不該講 — {why}"),
        }
    }

    fn render_question_status(corpus: &Corpus, questions: &QuestionSet) {
        let mut answers = 0usize;
        let mut no_answers = 0usize;
        let mut unlabeled = 0usize;
        for question in &questions.questions {
            match question.expected {
                Some(ExpectedOutcome::Answer { .. }) => answers += 1,
                Some(ExpectedOutcome::NoAnswer) => no_answers += 1,
                None => unlabeled += 1,
            }
        }
        println!(
            "題庫「{}」（{:?}）：總共 {} 題；answer {}、no_answer {}、未標 {}。",
            questions.name,
            questions.review,
            questions.questions.len(),
            answers,
            no_answers,
            unlabeled
        );
        println!(
            "  綁定 corpus「{}」（{:?}），fingerprint 已核對。",
            corpus.name, corpus.review
        );
    }

    fn annotate_with_io(
        corpus: &Corpus,
        questions: &QuestionSet,
        previews: &[AnnotationPreview],
        all: bool,
        input: &mut impl BufRead,
        output: &mut impl Write,
        checkpoint: &mut impl FnMut(&QuestionSet) -> Result<()>,
    ) -> Result<(QuestionSet, usize)> {
        anyhow::ensure!(
            previews.len() == questions.questions.len()
                && previews
                    .iter()
                    .zip(&questions.questions)
                    .all(|(preview, question)| preview.question_id == question.id),
            "標註提示和題庫的題目沒有一一對上"
        );

        let selected: Vec<_> = questions
            .questions
            .iter()
            .enumerate()
            .filter(|(_, question)| all || question.expected.is_none())
            .map(|(index, _)| index)
            .collect();
        if selected.is_empty() {
            writeln!(output, "全部題目都已有標註；要更正請加 --all。")?;
            return Ok((questions.clone(), 0));
        }

        let mut next = questions.clone();
        let mut changed = 0usize;
        let mut quit = false;
        for (position, &index) in selected.iter().enumerate() {
            if quit {
                break;
            }
            let question = &next.questions[index];
            writeln!(
                output,
                "\n[{}/{}] {}\n問題：{}",
                position + 1,
                selected.len(),
                question.id,
                question.question
            )?;
            if let Some(observed) = &question.observed {
                writeln!(
                    output,
                    "當時產品輸出（只供提示，不是正解）：shape={}、{} 筆、開過 {} 個出處、★={}",
                    observed.shape,
                    observed.product_results,
                    observed.opened_sources,
                    if observed.marked_forgotten {
                        "是"
                    } else {
                        "否"
                    }
                )?;
            }
            if let Some(expected) = &question.expected {
                writeln!(output, "現有標註：{}", expected_summary(expected))?;
            }
            render_preview(output, &previews[index])?;
            writeln!(
                output,
                "輸入：a EVENT 答案｜n 沒有答案｜f 文字 搜 evidence｜e EVENT 看 evidence｜s 跳過｜q 存檔離開"
            )?;

            loop {
                write!(output, "> ")?;
                output.flush()?;
                let mut line = String::new();
                if input.read_line(&mut line)? == 0 {
                    quit = true;
                    break;
                }
                let command = line.trim();
                if command == "q" {
                    quit = true;
                    break;
                }
                if command == "s" {
                    break;
                }
                if command == "n" {
                    let replace = next.questions[index].expected.is_some();
                    next = next.with_no_answer(corpus, &question.id, replace)?;
                    checkpoint(&next)?;
                    changed += 1;
                    break;
                }
                if let Some(rest) = command.strip_prefix("a ") {
                    let Some((event, answer)) = rest.trim().split_once(char::is_whitespace) else {
                        writeln!(output, "格式是：a EVENT 答案")?;
                        continue;
                    };
                    let Ok(event_index) = event.parse::<usize>() else {
                        writeln!(output, "EVENT 必須是 corpus 裡的非負整數 index。")?;
                        continue;
                    };
                    let answer = answer.trim();
                    if answer.is_empty() {
                        writeln!(output, "答案不可為空。")?;
                        continue;
                    }
                    let replace = next.questions[index].expected.is_some();
                    match next.with_answer(
                        corpus,
                        &question.id,
                        event_index,
                        vec![answer.to_string()],
                        replace,
                    ) {
                        Ok(labeled) => {
                            next = labeled;
                            checkpoint(&next)?;
                            changed += 1;
                            break;
                        }
                        Err(error) => {
                            writeln!(output, "不能存這個標註：{error:#}")?;
                            continue;
                        }
                    }
                }
                if let Some(needle) = command.strip_prefix("f ").map(str::trim) {
                    if needle.is_empty() {
                        writeln!(output, "格式是：f 要找的文字")?;
                    } else {
                        render_evidence_search(output, corpus, needle)?;
                    }
                    continue;
                }
                if let Some(event) = command.strip_prefix("e ").map(str::trim) {
                    match event.parse::<usize>() {
                        Ok(event_index) => render_event_evidence(output, corpus, event_index)?,
                        Err(_) => writeln!(output, "格式是：e EVENT")?,
                    }
                    continue;
                }
                writeln!(output, "不認得；請輸入 a、n、f、e、s 或 q。")?;
            }
        }
        Ok((next, changed))
    }

    fn render_preview(output: &mut impl Write, preview: &AnnotationPreview) -> Result<()> {
        if preview.returned.is_empty() {
            writeln!(output, "產品檢索候選：0 筆（這不等於正解是 no_answer）")?;
            return Ok(());
        }
        writeln!(output, "產品檢索候選（只供提示）：")?;
        for item in &preview.returned {
            writeln!(
                output,
                "  rank {}｜event {}｜{:?}/{}｜+{} ms｜{}",
                item.rank,
                item.event_indexes
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                item.channel,
                item.source_kind,
                item.at_ms,
                concise(&item.values.join(" / "), 180)
            )?;
        }
        Ok(())
    }

    fn render_evidence_search(
        output: &mut impl Write,
        corpus: &Corpus,
        needle: &str,
    ) -> Result<()> {
        let needle = needle.to_lowercase();
        let mut matched = 0usize;
        for (event_index, event) in corpus.events.iter().enumerate() {
            let surfaces: Vec<_> = evidence_surfaces(event)
                .into_iter()
                .filter(|(_, text)| text.to_lowercase().contains(&needle))
                .collect();
            if surfaces.is_empty() {
                continue;
            }
            matched += 1;
            if matched <= 20 {
                writeln!(
                    output,
                    "  event {}｜{}",
                    event_index,
                    concise(
                        &surfaces
                            .iter()
                            .map(|(_, text)| text.as_str())
                            .collect::<Vec<_>>()
                            .join(" / "),
                        220
                    )
                )?;
            }
        }
        match matched {
            0 => writeln!(output, "  沒有 evidence surface 含這段文字。")?,
            1..=20 => writeln!(output, "  找到 {matched} 個 event。")?,
            _ => writeln!(output, "  找到 {matched} 個 event；只顯示前 20 個。")?,
        }
        Ok(())
    }

    fn render_event_evidence(
        output: &mut impl Write,
        corpus: &Corpus,
        event_index: usize,
    ) -> Result<()> {
        let Some(event) = corpus.events.get(event_index) else {
            writeln!(output, "corpus 沒有 event {event_index}。")?;
            return Ok(());
        };
        let surfaces = evidence_surfaces(event);
        if surfaces.is_empty() {
            writeln!(
                output,
                "event {event_index} 沒有可作 answer evidence 的文字。"
            )?;
            return Ok(());
        }
        writeln!(output, "event {event_index} 的 evidence：")?;
        for (kind, text) in surfaces {
            writeln!(output, "  {}｜", kind.as_str())?;
            for line in text.lines() {
                writeln!(output, "    {line}")?;
            }
        }
        Ok(())
    }

    fn expected_summary(expected: &ExpectedOutcome) -> String {
        match expected {
            ExpectedOutcome::Answer { any_of, evidence } => format!(
                "answer event {} = {}",
                evidence
                    .first()
                    .map(|reference| reference.event_index.to_string())
                    .unwrap_or_else(|| "沒填".into()),
                any_of.join(" / ")
            ),
            ExpectedOutcome::NoAnswer => "no_answer".into(),
        }
    }

    fn concise(text: &str, limit: usize) -> String {
        let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if one_line.chars().count() <= limit {
            return one_line;
        }
        let mut shortened: String = one_line.chars().take(limit).collect();
        shortened.push('…');
        shortened
    }

    pub struct EvaluateOpts<'a> {
        pub k: usize,
        pub runs: usize,
        pub json: bool,
        pub output: Option<&'a Path>,
        pub ab: bool,
        pub brain: Option<sister_core::eval::BrainEval>,
    }

    pub fn evaluate_corpus(
        corpus_path: &Path,
        questions_path: &Path,
        opts: EvaluateOpts<'_>,
    ) -> Result<()> {
        let (corpus, questions) = read_corpus_and_questions(corpus_path, questions_path)?;

        let report = if opts.ab {
            sister_core::eval::evaluate_ab(
                &corpus,
                &questions,
                opts.k,
                opts.runs,
                opts.brain.as_ref(),
            )?
        } else {
            sister_core::eval::evaluate(&corpus, &questions, opts.k, opts.runs)?
        };
        let json = opts.json;
        let output = opts.output;
        if json {
            warn_private_report(&report);
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(());
        }
        if let Some(path) = output {
            write_new_report(path, &report)?;
        }
        render_eval(&report, output);
        Ok(())
    }

    fn render_eval(report: &EvalReport, output: Option<&Path>) {
        let review = match report.corpus.review {
            ReviewStatus::Draft => "Draft",
            ReviewStatus::Reviewed => "Reviewed",
        };
        let question_review = match report.question_set.review {
            ReviewStatus::Draft => "Draft",
            ReviewStatus::Reviewed => "Reviewed",
        };
        println!(
            "評測「{}」：{} 題（{}），corpus「{}」{} 個事件（{}），k={}，每題 {} 次計時",
            report.question_set.name,
            report.question_set.questions,
            question_review,
            report.corpus.name,
            report.corpus.events,
            review,
            report.parameters.k,
            report.parameters.runs
        );
        let sources = &report.question_set.sources;
        println!(
            "  題目來源：query log {}、人工標註 {}、腳本埋題 {}",
            sources.query_log, sources.hand_labeled, sources.planted
        );
        println!(
            "  輸入指紋：corpus {}；questions {}",
            report.corpus.fingerprint, report.question_set.fingerprint
        );
        println!();
        println!(
            "  配置              找回率@k       答案正確率      出處正確率      延遲 p50 / p95"
        );
        for config in &report.configurations {
            println!(
                "  {:<17} {:<14} {:<15} {:<15} {:>7.2} / {:>7.2} ms",
                config.name,
                fraction(&config.metrics.recall_at_k),
                fraction(&config.metrics.answer_accuracy),
                fraction(&config.metrics.citation_accuracy),
                config.metrics.latency.p50_ms,
                config.metrics.latency.p95_ms,
            );
            println!(
                "    模型：{}",
                sister_core::eval::format_model_usage(&config.metrics.model)
            );
            let failed: Vec<_> = config
                .questions
                .iter()
                .filter(|question| !question.answer_correct)
                .map(|question| question.id.as_str())
                .collect();
            if !failed.is_empty() {
                println!("    答錯／空手：{}", failed.join("、"));
            }
        }
        if let Some(ab) = &report.ab {
            println!();
            println!(
                "  A/B：{} vs {}，題庫 {} 題全跑，跳過 {} 題。",
                ab.baseline,
                ab.treatment,
                ab.questions_total,
                ab.questions_skipped.len()
            );
            if !ab.questions_skipped.is_empty() {
                for skipped in &ab.questions_skipped {
                    println!("    跳過 {}：{}", skipped.id, skipped.reason);
                }
            }
            match ab.accuracy_delta_pt {
                Some(delta) => println!(
                    "    答案正確率差值：{delta:+.1} pt（門檻 +{:.0} pt）",
                    sister_core::eval::AB_ACCURACY_WIN_PT
                ),
                None => println!("    答案正確率差值：沒有分母，還沒量到。"),
            }
            println!(
                "    誤承諾率：{}",
                sister_core::eval::format_false_commitment(&ab.false_commitment)
            );
            if let Some(skip) = &ab.brain.skip {
                println!(
                    "    解釋層跳過：{skip}（jobs {}，成功 {}）",
                    ab.brain.interpreter_jobs, ab.brain.interpreter_success
                );
            } else {
                println!(
                    "    解釋層：jobs {}，成功 {}",
                    ab.brain.interpreter_jobs, ab.brain.interpreter_success
                );
            }
            if let Some(skip) = &ab.brain.reviewer_skip {
                println!("    審閱層跳過：{skip}");
            }
            println!("    {}", sister_core::eval::format_ab_gate(&ab.gate));
        }
        println!(
            "  尚未量到：提醒誤報／漏報、斷句 F1、Reviewer 回查率、CPU／RAM／電池／磁碟；report 裡是 null，不是 0。"
        );
        if report.corpus.review == ReviewStatus::Draft
            || report.question_set.review == ReviewStatus::Draft
        {
            println!("  ⚠  corpus 或題庫是 private Draft；報告也含文字，人工審查前不要分享。");
        }
        if let Some(path) = output {
            println!("  完整 JSON：{}", path.display());
        }
    }

    fn warn_private_report(report: &EvalReport) {
        if report.corpus.review == ReviewStatus::Draft
            || report.question_set.review == ReviewStatus::Draft
        {
            eprintln!("⚠ corpus 或題庫是 private Draft；這份 JSON 也只能留在本機。");
        }
    }

    fn fraction(value: &Fraction) -> String {
        match value.rate {
            Some(rate) => format!("{}/{} ({:.1}%)", value.passed, value.total, rate * 100.0),
            None => "沒這類題".to_string(),
        }
    }

    fn write_new_json(path: &Path, value: &DraftCorpus) -> Result<()> {
        write_new_bytes(path, serde_json::to_vec_pretty(value)?)
    }

    fn write_new_report(path: &Path, value: &EvalReport) -> Result<()> {
        write_new_bytes(path, serde_json::to_vec_pretty(value)?)
    }

    fn write_new_question_set(path: &Path, value: &QuestionSet) -> Result<()> {
        write_new_bytes(path, serde_json::to_vec_pretty(value)?)
    }

    fn write_new_moment_set(path: &Path, value: &MomentSet) -> Result<()> {
        write_new_bytes(path, serde_json::to_vec_pretty(value)?)
    }

    /// 互動標註的目的檔。來源永遠不動；目的地在第一題出現前就以 create-new
    /// 保留，之後每題都同步成一份可重新讀取的完整 Draft。
    struct QuestionDraftOutput {
        path: PathBuf,
        file: Option<std::fs::File>,
        last_bytes: Vec<u8>,
        last_questions: QuestionSet,
        keep: bool,
    }

    impl QuestionDraftOutput {
        fn reserve(path: &Path, questions: &QuestionSet) -> Result<Self> {
            let file = open_new_file(path)?;
            let mut output = Self {
                path: path.to_path_buf(),
                file: Some(file),
                last_bytes: Vec::new(),
                last_questions: questions.clone(),
                keep: false,
            };
            let bytes = question_set_bytes(questions)?;
            if let Err(error) = output.replace_bytes(&bytes) {
                return Err(error).with_context(|| format!("write and sync {}", path.display()));
            }
            output.last_bytes = bytes;
            Ok(output)
        }

        fn checkpoint(&mut self, questions: &QuestionSet) -> Result<()> {
            let bytes = question_set_bytes(questions)?;
            if let Err(error) = self.replace_bytes(&bytes) {
                // 一般 I/O 錯誤還有機會把上一份完整 checkpoint 救回來。若連救回
                // 都失敗，Drop 會拿掉這顆可能半截的目的檔；來源 Draft 仍完整。
                if self.replace_bytes(&self.last_bytes.clone()).is_err() {
                    self.keep = false;
                }
                return Err(error).with_context(|| format!("checkpoint {}", self.path.display()));
            }
            self.last_bytes = bytes;
            self.last_questions = questions.clone();
            self.keep = true;
            Ok(())
        }

        fn replace_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            let file = self.file.as_mut().expect("reserved output still open");
            file.seek(SeekFrom::Start(0))?;
            file.write_all(bytes)?;
            file.set_len(bytes.len() as u64)?;
            file.sync_all()
        }

        fn discard(mut self) -> Result<()> {
            drop(self.file.take());
            std::fs::remove_file(&self.path)
                .with_context(|| format!("remove unused {}", self.path.display()))?;
            self.keep = true;
            Ok(())
        }

        fn finish(mut self) {
            self.keep = true;
            drop(self.file.take());
        }
    }

    impl Drop for QuestionDraftOutput {
        fn drop(&mut self) {
            if !self.keep {
                drop(self.file.take());
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    /// 互動標註時刻集的目的檔。來源永遠不動；零變更正常退出不留一份假成果。
    struct MomentDraftOutput {
        path: PathBuf,
        file: Option<std::fs::File>,
        last_bytes: Vec<u8>,
        last_moments: MomentSet,
        keep: bool,
    }

    impl MomentDraftOutput {
        fn reserve(path: &Path, moments: &MomentSet) -> Result<Self> {
            let file = open_new_file(path)?;
            let mut output = Self {
                path: path.to_path_buf(),
                file: Some(file),
                last_bytes: Vec::new(),
                last_moments: moments.clone(),
                keep: false,
            };
            let bytes = moment_set_bytes(moments)?;
            if let Err(error) = output.replace_bytes(&bytes) {
                return Err(error).with_context(|| format!("write and sync {}", path.display()));
            }
            output.last_bytes = bytes;
            Ok(output)
        }

        fn checkpoint(&mut self, moments: &MomentSet) -> Result<()> {
            let bytes = moment_set_bytes(moments)?;
            if let Err(error) = self.replace_bytes(&bytes) {
                if self.replace_bytes(&self.last_bytes.clone()).is_err() {
                    self.keep = false;
                }
                return Err(error).with_context(|| format!("checkpoint {}", self.path.display()));
            }
            self.last_bytes = bytes;
            self.last_moments = moments.clone();
            self.keep = true;
            Ok(())
        }

        fn replace_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            let file = self.file.as_mut().expect("reserved output still open");
            file.seek(SeekFrom::Start(0))?;
            file.write_all(bytes)?;
            file.set_len(bytes.len() as u64)?;
            file.sync_all()
        }

        fn discard(mut self) -> Result<()> {
            drop(self.file.take());
            std::fs::remove_file(&self.path)
                .with_context(|| format!("remove unused {}", self.path.display()))?;
            self.keep = true;
            Ok(())
        }

        fn finish(mut self) {
            self.keep = true;
            drop(self.file.take());
        }
    }

    impl Drop for MomentDraftOutput {
        fn drop(&mut self) {
            if !self.keep {
                drop(self.file.take());
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    fn moment_set_bytes(value: &MomentSet) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn question_set_bytes(value: &QuestionSet) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn write_new_bytes(path: &Path, mut bytes: Vec<u8>) -> Result<()> {
        bytes.push(b'\n');
        let mut file = open_new_file(path)?;
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            drop(file);
            // 這一輪用 create_new 建的半截檔不是使用者原本的資料；留下它會讓
            // 下一次匯出只看到「拒絕覆寫」，也可能被誤認成完整 corpus。
            let _ = std::fs::remove_file(path);
            return Err(error).with_context(|| format!("write and sync {}", path.display()));
        }
        Ok(())
    }

    fn open_new_file(path: &Path) -> Result<std::fs::File> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .with_context(|| format!("建立 {} 失敗（不會覆寫已存在的檔案）", path.display()))?;
        Ok(file)
    }

    /// 預設檔名不帶真實 epoch；分享 corpus 時，檔名也不該把已相對化的時間補回去。
    fn next_draft_path(data_dir: &Path) -> PathBuf {
        let root = data_dir.join("replay-drafts");
        let first = root.join("replay.sister-replay-draft.json");
        if !first.exists() {
            return first;
        }
        for n in 2u32.. {
            let candidate = root.join(format!("replay-{n}.sister-replay-draft.json"));
            if !candidate.exists() {
                return candidate;
            }
        }
        unreachable!("u32 檔名已全部存在")
    }

    pub fn run(
        data_dir: &Path,
        config: Config,
        scenario_path: &Path,
        interval_ms: i64,
        dry_run: bool,
        days_ago: f64,
        start: Option<i64>,
    ) -> Result<()> {
        anyhow::ensure!(interval_ms > 0, "--interval-ms must be positive");
        let scenario = Scenario::load(scenario_path)?;
        let duration = scenario.duration_ms();
        // 腳本寫的是相對時間；落地前換算成真實時間，否則整段記憶會停在 1970 年
        let origin = start
            .unwrap_or_else(|| sister_core::now_ms() - duration - (days_ago * 86_400_000.0) as i64);
        println!(
            "▶ 重播「{}」：{} 步、{:.1} 秒，每 {} ms 一次 tick{}",
            scenario.name,
            scenario.steps.len(),
            duration as f64 / 1000.0,
            interval_ms,
            if dry_run {
                "（dry-run，不寫入）"
            } else {
                ""
            }
        );
        println!("  時間軸起點：{}", crate::fmt::timestamp(origin));

        let db = if dry_run {
            Db::open_in_memory()?
        } else {
            let path = crate::db_path(data_dir);
            Db::open(&path).with_context(|| format!("open {}", path.display()))?
        };
        let image_dir = if dry_run {
            None
        } else {
            Some(sister_capture::frames::frames_root(data_dir))
        };

        // `record` 開場就講這句（見那邊的 `capture.enabled` 判斷），`replay`
        // 以前完全安靜——而它一樣會寫進真的資料庫、真的 frames/。61 個
        // `Tick::Disabled` 被吞進 `Duplicate | Paused | Idle` 那一條 arm，於是
        // 印出來的是「完成：61 tick → 保留 0、重複 0、排除 0、無畫面 0」，
        // exit 0，和「腳本裡本來就沒東西」長得一模一樣。
        let disabled = !config.capture.enabled;
        if disabled {
            println!(
                "⚠  capture.enabled = false：接下來每一個 tick 都會直接跳過，\
                 這次重播不會記錄任何東西。改成 true 才會真的錄。"
            );
        }

        let backend = ReplayBackend::with_origin(scenario, origin);
        let mut rec = Recorder::new(backend, db, config, image_dir)?;

        let mut offset = 0i64;
        while offset <= duration {
            let ts = origin + offset;
            match rec.tick(ts)? {
                Tick::Kept {
                    frame_id,
                    ocr_blocks,
                    facts,
                } => {
                    println!(
                        "  {offset:>7} ms  保留 frame #{frame_id}（{ocr_blocks} 段文字、{facts} 個事實）"
                    );
                }
                Tick::Excluded { reason } => println!("  {offset:>7} ms  排除：{reason}"),
                Tick::NoScreen => println!("  {offset:>7} ms  沒有畫面"),
                // 重播不看暫停旗標——腳本要能重跑出同一份結果，而旗標是
                // 執行當下的環境狀態。所以 `Paused` 在這條路上到不了。
                Tick::Duplicate { .. } | Tick::Disabled | Tick::Paused | Tick::Idle => {}
            }
            offset += interval_ms;
        }
        // 重播一定是跑完整份腳本才結束的：沒有人按得了停止，也沒有 Ctrl-C
        // 以外的出口。`--days-ago` 這種參數改的是時間戳，不是長度。
        rec.finish(sister_core::model::EndReason::Duration)?;

        let s = rec.stats();
        println!(
            "\n完成：{} tick → 保留 {}、重複 {}、排除 {}、無畫面 {}",
            s.ticks, s.kept, s.duplicates, s.excluded, s.no_screen
        );
        if disabled {
            // 收尾再講一次。開場那句話在 61 行 tick 輸出的上面，捲上去才看得到。
            println!(
                "  ⚠  這 {} 個 tick 全部被 capture.enabled = false 跳過了，\
                 一個字都沒記進去。",
                s.ticks
            );
        }
        // 排在最前面：一場整拍都在炸的錄製，底下每一行都在描述一個空集合。
        if let Some(line) = tick_failures_line(s) {
            println!("{line}");
        }
        report_idle(s);
        report_exclusions(s);
        if s.secrets_redacted > 0 {
            println!("  偵測到 {} 次疑似秘密，內容未落地。", s.secrets_redacted);
        }
        Ok(())
    }

    #[cfg(test)]
    mod corpus_tests {
        use super::*;
        use sister_core::db::{QueryLogEntry, SOURCE_DESKTOP};
        use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};

        fn seed(dir: &Path) -> Result<()> {
            let mut db = Db::open(&crate::db_path(dir))?;
            let session = db.start_session("test", "test")?;
            let ts = sister_core::now_ms() - 1_000;
            let secret = "ghp_16CharsAtLeastHereOk123";
            db.insert_frame(
                session,
                &FrameCapture {
                    ts,
                    monitor: 0,
                    width: 1920,
                    height: 1080,
                    dhash: 42,
                    image: None,
                    image_ext: "png",
                    ocr: vec![OcrBlock {
                        text: format!(
                            "一般 replay 文字；客服 0912-345-678；帳單 NT$13,450；信箱 ted@example.com；token={secret}"
                        ),
                        x: 1,
                        y: 2,
                        w: 300,
                        h: 40,
                        confidence: 0.95,
                    }],
                    focus: FocusSnapshot {
                        app_id: Some("terminal.exe".into()),
                        window_title: Some("一般 replay 視窗".into()),
                        ..Default::default()
                    },
                },
                Some("C:/private/frames/secret.png"),
                999,
            )?;
            db.end_session(session)?;
            Ok(())
        }

        #[test]
        fn export_is_private_draft_without_source_secrets_or_image_paths() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-export");
            seed(&tmp.0)?;
            let output = tmp.0.join("corpus.sister-replay-draft.json");
            export_corpus(&tmp.0, "1d", Some(&output), None, Some("測試語料"))?;

            let json = std::fs::read_to_string(&output)?;
            assert!(json.contains("一般 replay 文字"), "{json}");
            assert!(json.contains("\"review\": \"draft\""), "{json}");
            for forbidden in [
                "0912-345-678",
                "NT$13,450",
                "ted@example.com",
                "ghp_16CharsAtLeastHereOk123",
                "C:/private/frames/secret.png",
                "image_path",
            ] {
                assert!(!json.contains(forbidden), "洩漏 {forbidden}: {json}");
            }
            Ok(())
        }

        #[test]
        fn export_refuses_to_overwrite_an_existing_file() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-no-overwrite");
            seed(&tmp.0)?;
            let output = tmp.0.join("existing.json");
            std::fs::write(&output, "keep me")?;
            assert!(export_corpus(&tmp.0, "1d", Some(&output), None, None).is_err());
            assert_eq!(std::fs::read_to_string(output)?, "keep me");
            Ok(())
        }

        #[test]
        fn corpus_export_can_make_an_unlabeled_query_log_draft_in_the_same_window() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-query-draft");
            seed(&tmp.0)?;
            let db = Db::open(&crate::db_path(&tmp.0))?;
            let now = sister_core::now_ms();
            let first = db.log_query(&QueryLogEntry {
                ts: now - 800,
                question: "PRIVATE_QUERY_WORDS_123 要保留嗎",
                shape: "keywords",
                hits: 0,
                latency_ms: 2,
                source: SOURCE_DESKTOP,
            })?;
            db.log_click(first, 999_999, 0)?;
            db.mark_query(first, true)?;
            db.log_query(&QueryLogEntry {
                ts: now - 700,
                question: "PRIVATE_QUERY_WORDS_123 要保留嗎",
                shape: "keywords",
                hits: 3,
                latency_ms: 1,
                source: "cli",
            })?;
            drop(db);

            let corpus_path = tmp.0.join("day.sister-replay-draft.json");
            let questions_path = tmp.0.join("day.sister-questions-draft.json");
            export_corpus(
                &tmp.0,
                "1d",
                Some(&corpus_path),
                Some(&questions_path),
                Some("day"),
            )?;

            let corpus: Corpus = serde_json::from_slice(&std::fs::read(&corpus_path)?)?;
            let questions: QuestionSet = serde_json::from_slice(&std::fs::read(&questions_path)?)?;
            questions.validate(&corpus)?;
            assert_eq!(questions.review, ReviewStatus::Draft);
            assert_eq!(questions.questions.len(), 2, "重複問法是兩個真實實例");
            assert_eq!(questions.questions[0].id, "query-0001");
            assert_eq!(questions.questions[1].id, "query-0002");
            assert!(
                questions
                    .questions
                    .iter()
                    .all(|question| question.expected.is_none())
            );
            let observed = questions.questions[0].observed.as_ref().expect("observed");
            assert_eq!(observed.product_results, 0, "空手不能偷變成 NoAnswer");
            assert_eq!(observed.opened_sources, 1);
            assert!(observed.marked_forgotten);
            assert!(
                questions.questions[0]
                    .question
                    .contains("PRIVATE_QUERY_WORDS_123"),
                "這一版明講題目原話沒有自動去敏，不能偷偷改成另一個契約"
            );
            assert!(sister_core::eval::evaluate(&corpus, &questions, 5, 1).is_err());

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&questions_path)?.permissions().mode() & 0o777,
                    0o600
                );
            }
            Ok(())
        }

        #[test]
        fn asking_for_a_question_draft_with_no_queries_creates_neither_file() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-empty-query-draft");
            seed(&tmp.0)?;
            let corpus_path = tmp.0.join("day.sister-replay-draft.json");
            let questions_path = tmp.0.join("day.sister-questions-draft.json");
            assert!(
                export_corpus(
                    &tmp.0,
                    "1d",
                    Some(&corpus_path),
                    Some(&questions_path),
                    None,
                )
                .is_err()
            );
            assert!(!corpus_path.exists() && !questions_path.exists());
            Ok(())
        }

        #[test]
        fn default_draft_names_do_not_put_real_timestamps_back_in_the_filename() {
            let tmp = crate::ops::tmp::Tmp::new("replay-default-name");
            let first = next_draft_path(&tmp.0);
            assert_eq!(
                first.file_name().and_then(|name| name.to_str()),
                Some("replay.sister-replay-draft.json")
            );
            std::fs::create_dir_all(first.parent().expect("parent")).expect("mkdir");
            std::fs::write(&first, "draft").expect("first");
            let second = next_draft_path(&tmp.0);
            assert_eq!(
                second.file_name().and_then(|name| name.to_str()),
                Some("replay-2.sister-replay-draft.json")
            );
        }

        #[test]
        fn exported_corpus_import_rebuilds_search_and_facts() -> Result<()> {
            let source = crate::ops::tmp::Tmp::new("replay-source");
            let target = crate::ops::tmp::Tmp::new("replay-target");
            seed(&source.0)?;
            let output = source.0.join("corpus.sister-replay-draft.json");
            export_corpus(&source.0, "1d", Some(&output), None, None)?;
            import_corpus(&target.0, &output, false, 0.0, Some(1_700_000_000_000))?;

            let db = Db::open(&crate::db_path(&target.0))?;
            assert!(!db.search("一般 replay 文字", 10)?.is_empty());
            let stats = db.stats()?;
            assert_eq!(stats.frames, 1);
            assert!(stats.facts >= 3, "{stats:?}");
            assert_eq!(stats.frames_with_image, 0);
            Ok(())
        }

        #[test]
        fn annotator_labels_a_whole_draft_without_trusting_product_hints() -> Result<()> {
            let corpus: Corpus = serde_json::from_str(include_str!(
                "../../../scenarios/recall-baseline.corpus.json"
            ))?;
            let mut questions: QuestionSet = serde_json::from_str(include_str!(
                "../../../scenarios/recall-baseline.questions.json"
            ))?;
            questions.review = ReviewStatus::Draft;
            for question in &mut questions.questions {
                question.expected = None;
            }
            let previews = annotation_previews(&corpus, &questions, 5)?;
            let commands = b"f 0800\ne 0\na 0 0800-000-123\na 0 NT$1,350\na 1 ERR_DEPLOY_42\na 2 release candidate alpha\nn\n";
            let mut input = std::io::Cursor::new(commands);
            let mut output = Vec::new();
            let mut checkpoints = Vec::new();
            let (labeled, changed) = annotate_with_io(
                &corpus,
                &questions,
                &previews,
                false,
                &mut input,
                &mut output,
                &mut |questions| {
                    checkpoints.push(questions.clone());
                    Ok(())
                },
            )?;

            assert_eq!(changed, 5);
            assert_eq!(checkpoints.len(), 5, "每完成一題就要 checkpoint");
            assert_eq!(checkpoints.last(), Some(&labeled));
            assert!(
                labeled
                    .questions
                    .iter()
                    .all(|question| question.expected.is_some())
            );
            assert_eq!(labeled.review, ReviewStatus::Draft);
            let screen = String::from_utf8(output)?;
            assert!(
                screen.contains("event 0") && screen.contains("只供提示"),
                "{screen}"
            );
            assert!(screen.contains("產品檢索候選：0 筆（這不等於正解是 no_answer）"));
            assert!(questions.questions.iter().all(|q| q.expected.is_none()));
            Ok(())
        }

        #[test]
        fn annotator_reserves_its_destination_and_keeps_each_completed_checkpoint() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-question-checkpoint");
            let corpus: Corpus = serde_json::from_str(include_str!(
                "../../../scenarios/recall-baseline.corpus.json"
            ))?;
            let mut questions: QuestionSet = serde_json::from_str(include_str!(
                "../../../scenarios/recall-baseline.questions.json"
            ))?;
            questions.review = ReviewStatus::Draft;
            questions.questions[0].expected = None;

            let existing = tmp.0.join("existing.json");
            std::fs::write(&existing, "keep me")?;
            assert!(QuestionDraftOutput::reserve(&existing, &questions).is_err());
            assert_eq!(std::fs::read_to_string(&existing)?, "keep me");

            let unused = tmp.0.join("unused.json");
            QuestionDraftOutput::reserve(&unused, &questions)?.discard()?;
            assert!(!unused.exists(), "零變更正常退出不留一份假成果");

            let checkpoint = tmp.0.join("checkpoint.json");
            let mut output = QuestionDraftOutput::reserve(&checkpoint, &questions)?;
            let labeled = questions.with_answer(
                &corpus,
                "phone-synonym",
                0,
                vec!["0800-000-123".into()],
                false,
            )?;
            output.checkpoint(&labeled)?;
            drop(output);
            let saved: QuestionSet = serde_json::from_slice(&std::fs::read(&checkpoint)?)?;
            assert_eq!(saved, labeled);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&checkpoint)?.permissions().mode() & 0o777,
                    0o600
                );
            }
            Ok(())
        }

        #[test]
        fn review_needs_explicit_privacy_confirmation_and_writes_a_new_file() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-question-review");
            let corpus_path = tmp.0.join("fixture.corpus.json");
            let questions_path = tmp.0.join("fixture.questions-draft.json");
            let reviewed_path = tmp.0.join("fixture.questions.json");
            std::fs::write(
                &corpus_path,
                include_str!("../../../scenarios/recall-baseline.corpus.json"),
            )?;
            let mut questions: QuestionSet = serde_json::from_str(include_str!(
                "../../../scenarios/recall-baseline.questions.json"
            ))?;
            questions.review = ReviewStatus::Draft;
            std::fs::write(&questions_path, serde_json::to_vec_pretty(&questions)?)?;

            assert!(
                review_questions(&corpus_path, &questions_path, &reviewed_path, false).is_err()
            );
            assert!(!reviewed_path.exists());
            review_questions(&corpus_path, &questions_path, &reviewed_path, true)?;
            let reviewed: QuestionSet = serde_json::from_slice(&std::fs::read(&reviewed_path)?)?;
            assert_eq!(reviewed.review, ReviewStatus::Reviewed);
            assert_eq!(questions.review, ReviewStatus::Draft);
            assert!(
                review_questions(&corpus_path, &questions_path, &reviewed_path, true).is_err(),
                "Reviewed 輸出不可被覆寫"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&reviewed_path)?.permissions().mode() & 0o777,
                    0o600
                );
            }
            Ok(())
        }

        #[test]
        fn evaluate_writes_the_three_real_profiles_and_nulls_for_unmeasured_metrics() -> Result<()>
        {
            let tmp = crate::ops::tmp::Tmp::new("replay-evaluate");
            let corpus = tmp.0.join("fixture.corpus.json");
            let questions = tmp.0.join("fixture.questions.json");
            let report = tmp.0.join("fixture.sister-eval-report.json");
            std::fs::write(
                &corpus,
                include_str!("../../../scenarios/recall-baseline.corpus.json"),
            )?;
            std::fs::write(
                &questions,
                include_str!("../../../scenarios/recall-baseline.questions.json"),
            )?;

            evaluate_corpus(
                &corpus,
                &questions,
                EvaluateOpts {
                    k: 5,
                    runs: 1,
                    json: false,
                    output: Some(&report),
                    ab: false,
                    brain: None,
                },
            )?;
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&report)?)?;
            assert_eq!(value["configurations"][0]["name"], "baseline_text");
            assert_eq!(value["configurations"][1]["name"], "facts");
            assert_eq!(value["configurations"][2]["name"], "facts_session");
            assert_eq!(
                value["configurations"][1]["metrics"]["answer_accuracy"]["passed"],
                5
            );
            assert!(value["configurations"][0]["metrics"]["cpu_percent"].is_null());

            assert!(
                evaluate_corpus(
                    &corpus,
                    &questions,
                    EvaluateOpts {
                        k: 5,
                        runs: 1,
                        json: false,
                        output: Some(&report),
                        ab: false,
                        brain: None,
                    },
                )
                .is_err(),
                "既有 report 不可被覆寫"
            );
            Ok(())
        }

        fn moment_fixture() -> Corpus {
            use sister_core::model::OcrBlock;
            use sister_core::replay::{Event, FORMAT_VERSION, RedactionSummary, ReplayFocus};
            Corpus {
                format_version: FORMAT_VERSION,
                name: "moment-cli".into(),
                duration_ms: 2_000,
                review: ReviewStatus::Reviewed,
                redactions: RedactionSummary::default(),
                events: vec![
                    Event::Frame {
                        at_ms: 200,
                        monitor: 0,
                        width: 800,
                        height: 600,
                        dhash: 1,
                        dup_run: 0,
                        focus: ReplayFocus {
                            app_id: Some("chat.exe".into()),
                            window_title: Some("LINE".into()),
                            ..Default::default()
                        },
                        ocr: vec![OcrBlock {
                            text: "下午5點接她".into(),
                            x: 0,
                            y: 0,
                            w: 300,
                            h: 20,
                            confidence: 1.0,
                        }],
                    },
                    Event::Frame {
                        at_ms: 1_500,
                        monitor: 0,
                        width: 800,
                        height: 600,
                        dhash: 2,
                        dup_run: 0,
                        focus: ReplayFocus {
                            app_id: Some("editor.exe".into()),
                            window_title: Some("notes".into()),
                            ..Default::default()
                        },
                        ocr: vec![OcrBlock {
                            text: "普通工作筆記".into(),
                            x: 0,
                            y: 30,
                            w: 300,
                            h: 20,
                            confidence: 1.0,
                        }],
                    },
                ],
            }
        }

        #[test]
        fn moment_draft_refuses_overwrite_and_starts_unlabeled() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-moment-draft");
            let corpus_path = tmp.0.join("fixture.corpus.json");
            let moments_path = tmp.0.join("fixture.moments-draft.json");
            let corpus = moment_fixture();
            std::fs::write(&corpus_path, serde_json::to_vec_pretty(&corpus)?)?;

            draft_moments(&corpus_path, &moments_path)?;
            let drafted: MomentSet = serde_json::from_slice(&std::fs::read(&moments_path)?)?;
            assert_eq!(drafted.review, ReviewStatus::Draft);
            assert!(drafted.moments.iter().all(|moment| moment.label.is_none()));
            assert_eq!(drafted.counts().unlabeled, drafted.counts().total);
            assert_eq!(drafted.counts().should_speak, 0);
            assert!(draft_moments(&corpus_path, &moments_path).is_err());
            Ok(())
        }

        #[test]
        fn moment_annotator_labels_without_writing_on_zero_changes() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-moment-annotate");
            let corpus = moment_fixture();
            let draft = MomentSet::draft_from_corpus("moment-cli-moments", &corpus)?;
            let commands = b"skip\nw\n";
            let mut input = std::io::Cursor::new(commands);
            let mut output = Vec::new();
            let (labeled, changed) = annotate_moments_with_io(
                &corpus,
                &draft,
                false,
                &mut input,
                &mut output,
                &mut |_| Ok(()),
            )?;
            assert_eq!(changed, 0);
            assert!(labeled.moments.iter().all(|moment| moment.label.is_none()));
            let screen = String::from_utf8(output)?;
            assert!(screen.contains("未標"), "{screen}");
            assert!(screen.contains("畫面 OCR 抽出 DateTimeMention"), "{screen}");

            let unused = tmp.0.join("unused.json");
            MomentDraftOutput::reserve(&unused, &draft)?.discard()?;
            assert!(!unused.exists(), "零變更正常退出不留一份假成果");
            Ok(())
        }

        #[test]
        fn moment_annotator_accepts_commitment_speak_and_quiet() -> Result<()> {
            let corpus = moment_fixture();
            let draft = MomentSet::draft_from_corpus("moment-cli-moments", &corpus)?;
            assert!(
                draft.counts().total >= 1,
                "fixture 必須真的抽出 DateTimeMention"
            );
            let commands = "?\nc@3600000 五點接她\nskip\nskip\nskip\n";
            let mut input = std::io::Cursor::new(commands.as_bytes());
            let mut output = Vec::new();
            let mut checkpoints = Vec::new();
            let (labeled, changed) = annotate_moments_with_io(
                &corpus,
                &draft,
                false,
                &mut input,
                &mut output,
                &mut |moments| {
                    checkpoints.push(moments.clone());
                    Ok(())
                },
            )?;
            assert_eq!(changed, 1);
            assert_eq!(checkpoints.len(), 1);
            assert!(matches!(
                &labeled.moments[0].label,
                Some(MomentLabel::Commitment {
                    due_at_ms: Some(3_600_000),
                    remind,
                }) if remind == "五點接她"
            ));
            assert_eq!(
                labeled.counts().unlabeled,
                labeled.counts().total - 1,
                "其餘時刻若還沒標，未標數量要跟該講 0 分開"
            );
            let screen = String::from_utf8(output)?;
            assert!(
                screen.contains("該講類別") || screen.contains("commitment_due"),
                "{screen}"
            );
            Ok(())
        }

        #[test]
        fn moment_annotator_add_attaches_the_event_it_actually_picked() -> Result<()> {
            let corpus = moment_fixture();
            let draft = MomentSet::draft_from_corpus("moment-cli-moments", &corpus)?;
            let machine_ids: Vec<_> = draft
                .moments
                .iter()
                .map(|moment| moment.id.clone())
                .collect();
            let commands = "add@1200 q 正在寫沒有訊號\nw\n";
            let mut input = std::io::Cursor::new(commands.as_bytes());
            let mut output = Vec::new();
            let mut checkpoints = Vec::new();
            let (labeled, changed) = annotate_moments_with_io(
                &corpus,
                &draft,
                false,
                &mut input,
                &mut output,
                &mut |moments| {
                    checkpoints.push(moments.clone());
                    Ok(())
                },
            )?;
            assert_eq!(changed, 1, "add@ 必須算進 changed，才會留檔");
            assert_eq!(checkpoints.len(), 1);
            let hand = labeled
                .moments
                .iter()
                .find(|moment| moment.id == "moment-hand-0001")
                .expect("hand id namespace");
            assert_eq!(
                hand.evidence[0].event_index, 0,
                "1200ms 之前最近的是 event 0（200ms），不是 event 1（1500ms）"
            );
            assert!(
                machine_ids
                    .iter()
                    .all(|id| labeled.moments.iter().any(|moment| moment.id == *id))
            );
            let screen = String::from_utf8(output)?;
            assert!(
                screen.contains("掛到 event 0（該 event 的 at_ms=200；時刻 at_ms=1200）"),
                "必須印出程式真的挑中的 event，不是「大概是前一個」：{screen}"
            );
            assert!(
                screen.contains("下午5點接她"),
                "可見表面必須是掛上的那個 event 的 OCR：{screen}"
            );
            assert!(!screen.contains("普通工作筆記"), "{screen}");

            let mut input = std::io::Cursor::new("add@0 q 語料還沒開始\nw\n".as_bytes());
            let mut output = Vec::new();
            let (still, still_changed) = annotate_moments_with_io(
                &corpus,
                &draft,
                false,
                &mut input,
                &mut output,
                &mut |_| Ok(()),
            )?;
            assert_eq!(still_changed, 0);
            assert_eq!(still.moments.len(), draft.moments.len());
            let rejected = String::from_utf8(output)?;
            assert!(rejected.contains("之前一個 event 都沒有"), "{rejected}");
            Ok(())
        }

        #[test]
        fn moment_review_needs_explicit_privacy_confirmation() -> Result<()> {
            let tmp = crate::ops::tmp::Tmp::new("replay-moment-review");
            let corpus_path = tmp.0.join("fixture.corpus.json");
            let draft_path = tmp.0.join("fixture.moments-draft.json");
            let reviewed_path = tmp.0.join("fixture.moments.json");
            let corpus = moment_fixture();
            std::fs::write(&corpus_path, serde_json::to_vec_pretty(&corpus)?)?;
            let draft = MomentSet::draft_from_corpus("moment-cli-moments", &corpus)?;
            let mut labeled = draft.clone();
            for moment in &draft.moments {
                labeled = labeled.with_label(
                    &corpus,
                    &moment.id,
                    MomentLabel::ShouldStayQuiet {
                        why: "現在不該開口".into(),
                    },
                    false,
                )?;
            }
            std::fs::write(&draft_path, serde_json::to_vec_pretty(&labeled)?)?;

            assert!(
                review_moments(
                    &corpus_path,
                    &draft_path,
                    &reviewed_path,
                    ConfirmPrivateTextReviewed::NOT_CONFIRMED,
                )
                .is_err()
            );
            assert!(!reviewed_path.exists());
            review_moments(
                &corpus_path,
                &draft_path,
                &reviewed_path,
                ConfirmPrivateTextReviewed::CONFIRMED,
            )?;
            let reviewed: MomentSet = serde_json::from_slice(&std::fs::read(&reviewed_path)?)?;
            assert_eq!(reviewed.review, ReviewStatus::Reviewed);
            assert_eq!(reviewed.counts().unlabeled, 0);
            assert_eq!(reviewed.counts().should_speak, 0);
            assert_eq!(reviewed.counts().should_stay_quiet, reviewed.counts().total);
            Ok(())
        }
    }
}

pub mod record {
    use super::*;
    use sister_core::config::Config;

    /// 這台機器上可用的擷取後端名稱。
    pub fn backend_name() -> Option<&'static str> {
        #[cfg(windows)]
        {
            Some("windows-gdi")
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// 盯著設定檔有沒有被改過。
    ///
    /// 為什麼需要它：設定只在開機時讀一次，所以一個開著 `record` 三天的人
    /// 中途加一條排除規則，**那條規則三天內都不會生效**——而且不會有任何
    /// 一行字提到。他會以為網銀已經被擋掉了。
    ///
    /// 看的是 mtime **加上檔案大小**。只看 mtime 的話，同一秒內存兩次的編輯
    /// 有機會被漏掉（很多檔案系統的 mtime 解析度就是一秒）；大小補得住絕大
    /// 多數的漏網之魚，而且不用去讀整個檔案算雜湊。補不住的情況（同一秒、
    /// 同樣大小、內容不同）留在這裡講清楚，因為它真的存在。
    ///
    /// **故意不放在 `#[cfg(windows)]` 裡面**，雖然只有 Windows 的錄製迴圈會用
    /// 它。理由和 `sister-shell` 這個 crate 存在的理由是同一個：這是純邏輯，
    /// 而這台開發機是 Linux——放進 cfg 裡就等於它的測試永遠不會執行。
    #[derive(Default)]
    #[cfg_attr(not(windows), allow(dead_code))]
    struct ConfigWatch {
        stamp: Option<(std::time::SystemTime, u64)>,
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    impl ConfigWatch {
        fn new(path: Option<&Path>) -> Self {
            Self {
                stamp: path.and_then(Self::read_stamp),
            }
        }

        fn read_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
            let meta = std::fs::metadata(path).ok()?;
            Some((meta.modified().ok()?, meta.len()))
        }

        /// 檔案動過了嗎。**檔案從有變沒有也算動過**——那通常是使用者刪掉了
        /// 設定檔，而那代表「回到預設值」，是一個真的狀態變化。
        fn changed(&mut self, path: &Path) -> bool {
            let now = Self::read_stamp(path);
            if now == self.stamp {
                return false;
            }
            self.stamp = now;
            true
        }
    }

    /// 這個資料目錄上已經有人在錄了嗎。有的話就 `Err`。
    ///
    /// 和 `gate` 一樣拆成獨立函式、一樣不放 `#[cfg(windows)]`：一道只有目標
    /// 平台才執行得到的閘門，等於一道沒有被執行過的閘門。
    fn already_recording(data_dir: &Path) -> Result<()> {
        let now = sister_core::now_ms();
        // 心跳只讀一次。上一版先 `presence` 再 `occupied_why`，中間那個
        // recorder 只要把 `beat_thinking` 換成墓碑，第二次讀就是 `Stopped`
        // → `occupied_why` 回 `None` → `.expect("Thinking 一定佔著")` panic。
        let seen = sister_core::heartbeat::presence(data_dir, now);
        match seen {
            sister_core::heartbeat::Presence::Thinking { .. } => anyhow::bail!(
                "{}",
                sister_core::heartbeat::occupied_why_of(seen, now).unwrap_or_else(|| {
                    "錄製已經停了，解釋層還在想最後一段。想完就會收工，這期間不要再開一個。".into()
                })
            ),
            sister_core::heartbeat::Presence::Live(_) => {
                // 講得出「多久以前」，那個人才判斷得出這是不是自己剛剛開的那一個。
                let ago = sister_core::heartbeat::last_beat(data_dir)
                    .map(|ts| format!("最後一次心跳是 {} 秒前", (now - ts).max(0) / 1000))
                    .unwrap_or_else(|| "而且它剛剛還在動".to_string());
                anyhow::bail!(
                    "已經有一個 sister record 在這個資料目錄上跑了（{ago}）。\n\n\
                     兩個一起錄會對同一顆資料庫各寫一份，而唯一看得出來的症狀是\n\
                     磁碟用得比講好的快一倍——所以這裡直接擋下來。\n\n\
                     要換手的話先請那一個收工：\n    \
                     sister stop\n\n\
                     如果你確定那個行程已經死了，等 {} 秒它的心跳就會自己過期。\n\
                     （心跳檔：{}）\n",
                    sister_core::heartbeat::STALE_AFTER_MS / 1000,
                    sister_core::heartbeat::beat_path(data_dir).display()
                )
            }
            sister_core::heartbeat::Presence::NeverStarted
            | sister_core::heartbeat::Presence::Unreadable
            | sister_core::heartbeat::Presence::Stopped { .. }
            | sister_core::heartbeat::Presence::Stalled { .. } => Ok(()),
        }
    }

    mod record_meanings {
        use sister_core::config::Config;

        /// 設定檔原本寫的「要不要留圖」，尚未被同意書修改。
        ///
        /// tuple 欄位留在這個子 module 裡；父 module 直接寫 `WantsImages(bool)`
        /// 會得到 E0423，只能走有來源名稱的建構子。開機值由 `gate` 在降級前
        /// 建好；熱重載建構子只接受 `Config::reload` 剛從磁碟讀出、沒有經過
        /// `gate` / `consent::downgrade` 的那一份設定。
        ///
        /// 開機建構子有 Linux 也會跑的雙向閨門測試，專門把它和
        /// `ocr` / `enabled` 分開；熱重載建構子仍只在 `#[cfg(windows)]`
        /// 被編譯，`check-windows.sh` 不會執行它的斷言。
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[cfg_attr(not(windows), allow(dead_code))]
        pub(super) struct WantsImages(bool);

        #[cfg_attr(not(windows), allow(dead_code))]
        impl WantsImages {
            pub(super) fn before_consent_downgrade(config: &Config) -> Self {
                Self(config.capture.store_images)
            }

            #[cfg(windows)]
            pub(super) fn from_reloaded_config(config: &Config) -> Self {
                Self(config.capture.store_images)
            }

            pub(super) fn enabled(self) -> bool {
                self.0
            }

            #[cfg(test)]
            pub(super) fn from_raw(value: bool) -> Self {
                Self(value)
            }
        }

        /// Recorder 這一刻實際是否正在寫圖。
        ///
        /// tuple 欄位留在這個子 module 裡；父 module 直接拿裸 `bool` 建構會
        /// 得到 E0423，只能走 recorder 來源的建構子。
        ///
        /// 這擋不住本 module 內讀錯 recorder 狀態或取反；正式建構子只在
        /// `#[cfg(windows)]` 被呼叫，而 `check-windows.sh` 只編譯、不執行斷言。
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[cfg_attr(not(windows), allow(dead_code))]
        pub(super) struct StoringImages(bool);

        #[cfg_attr(not(windows), allow(dead_code))]
        impl StoringImages {
            #[cfg(windows)]
            pub(super) fn from_recorder<B: sister_capture::Backend>(
                rec: &sister_capture::Recorder<B>,
            ) -> Self {
                Self(rec.stores_images())
            }

            pub(super) fn enabled(self) -> bool {
                self.0
            }

            #[cfg(test)]
            pub(super) fn from_raw(value: bool) -> Self {
                Self(value)
            }
        }

        /// 設定檔裡的 OCR 開關。
        ///
        /// tuple 欄位留在這個子 module 裡；父 module 直接拿 recorder 的裸
        /// `bool` 建構會得到 E0423，只能走 Config 來源的建構子。
        ///
        /// 建構子有 Linux 也會跑的雙向測試，每個方向都讓 `ocr`
        /// 和其他 capture 開關相反，守住摘要不會誤報讀字已關閉。
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[cfg_attr(not(windows), allow(dead_code))]
        pub(super) struct OcrEnabled(bool);

        #[cfg_attr(not(windows), allow(dead_code))]
        impl OcrEnabled {
            pub(super) fn from_config(config: &Config) -> Self {
                Self(config.capture.ocr)
            }

            pub(super) fn enabled(self) -> bool {
                self.0
            }
        }

        /// Recorder 統計中的總拍數與實際做事拍數。
        ///
        /// 欄位留在這個子 module 裡；父 module 寫 `TickCounts { .. }` 會得到
        /// E0451，只能走統計來源的建構子。建構子內的欄位對應仍可能寫錯，
        /// 但有 Linux 也會跑的測試守住；`check-windows.sh` 本身只編譯。
        #[cfg(any(windows, test))]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) struct TickCounts {
            total: u64,
            working: u64,
        }

        #[cfg(any(windows, test))]
        impl TickCounts {
            pub(super) fn from_stats(stats: &sister_capture::RecorderStats) -> Self {
                Self {
                    total: stats.ticks,
                    working: stats.working_ticks,
                }
            }

            pub(super) fn total(self) -> u64 {
                self.total
            }

            pub(super) fn working(self) -> u64 {
                self.working
            }

            pub(super) fn idle(self) -> u64 {
                self.total.saturating_sub(self.working)
            }
        }

        /// 這一場實際寫掉的圖片 bytes。
        ///
        /// tuple 欄位留在這個子 module 裡；父 module 直接拿額度的裸 `u64`
        /// 建構會得到 E0423，只能走 RecorderStats 來源的建構子。
        ///
        /// 建構子有 Linux 也會跑的測試，fixture 把每個 `u64`
        /// 統計欄位設成不同值，守住摘要拿到的是真正圖片 bytes。
        #[cfg(any(windows, test))]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[cfg_attr(not(windows), allow(dead_code))]
        pub(super) struct ImageBytesWritten(u64);

        #[cfg(any(windows, test))]
        #[cfg_attr(not(windows), allow(dead_code))]
        impl ImageBytesWritten {
            pub(super) fn from_stats(stats: &sister_capture::RecorderStats) -> Self {
                Self(stats.image_bytes)
            }

            pub(super) fn bytes(self) -> u64 {
                self.0
            }

            #[cfg(test)]
            pub(super) fn from_raw_bytes(bytes: u64) -> Self {
                Self(bytes)
            }
        }

        /// 每日圖片額度換算後的 bytes。
        ///
        /// tuple 欄位留在這個子 module 裡；父 module 直接建構會得到 E0423，
        /// 而產物與 [`ImageBytesWritten`] 不同型，對調兩個引數會得到 E0308。
        /// `from_mb` 仍接受裸 `u64`，所以擋不住傳入語意錯誤但同型的 MB 數值；
        /// 但 MB 到 bytes 的換算與「0 代表不設限」都有 Linux 會跑的寫死值斷言。
        #[cfg(any(windows, test))]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[cfg_attr(not(windows), allow(dead_code))]
        pub(super) struct ImageBudgetBytes(u64);

        #[cfg(any(windows, test))]
        #[cfg_attr(not(windows), allow(dead_code))]
        impl ImageBudgetBytes {
            pub(super) fn from_mb(mb: u64) -> Self {
                Self(mb * 1024 * 1024)
            }

            pub(super) fn bytes(self) -> u64 {
                self.0
            }

            #[cfg(test)]
            pub(super) fn from_raw_bytes(bytes: u64) -> Self {
                Self(bytes)
            }
        }
    }

    #[cfg(windows)]
    use record_meanings::OcrEnabled;
    #[cfg(any(windows, test))]
    use record_meanings::{ImageBudgetBytes, ImageBytesWritten, TickCounts};

    use record_meanings::{StoringImages, WantsImages};

    /// 同意書那道閘門。過不了就 `Err`，過得了就回一份**可能被降級過**的設定，
    /// 以及設定檔原本寫的留圖意願。
    ///
    /// 拆成獨立函式而不是寫在 `run` 裡，是為了讓它在這台 Linux 開發機上跑得到
    /// （`run` 的後半段整段 `#[cfg(windows)]`）。一道只有目標平台才執行得到的
    /// 隱私閘門，等於一道沒有被執行過的閘門。
    fn gate(data_dir: &Path, mut config: Config) -> Result<(Config, WantsImages)> {
        let consent = sister_core::consent::load(data_dir);

        if !consent.allows_recording() {
            let why = if consent
                .get(sister_core::consent::Sheet::LocalRecording)
                .is_some()
            {
                format!(
                    "同意書條文改版了（現在是第 {} 版），之前簽的那一張不再算數。",
                    sister_core::consent::VERSION
                )
            } else {
                "還沒有人同意讓她記錄這台機器的螢幕。".to_string()
            };
            anyhow::bail!(
                "{why}\n\n  「{}」\n\n\
                 要她開始記錄，請跑：\n    \
                 sister consent --grant local-recording\n\n\
                 想連截圖一起留（否則她只記螢幕上的字）：\n    \
                 sister consent --grant local-recording --grant frame-storage\n\n\
                 看目前簽了哪幾張：\n    \
                 sister consent\n",
                sister_core::consent::Sheet::LocalRecording.wording()
            );
        }

        // 使用者在設定檔裡自己寫的那個意思，**還沒有被同意書修改過**。
        //
        // 第三張中途簽回來的時候要靠它才知道「他本來就想留圖」。若等下面的
        // downgrade 做完才問，答案永遠是「不要」——因為開機時沒簽，它已經
        // 被按成 false，於是「撤回得掉、簽回來卻要重開」，而使用者分不出這
        // 兩件事有什麼不同。
        let wants_images_by_config = WantsImages::before_consent_downgrade(&config);

        // 第三張沒簽不是「不能錄」，是「只記字不留圖」（SPEC §11.1 的
        // 「0 天 = 只留 OCR 文字」）。降級要講出來——安靜地少存一半東西，
        // 使用者只會以為截圖功能壞了。
        if consent.downgrade(&mut config) {
            println!(
                "  第三張同意書沒簽：這一次只記螢幕上的字，不會寫任何截圖。\n  \
                 （要留圖請跑 `sister consent --grant frame-storage`）"
            );
        }
        Ok((config, wants_images_by_config))
    }

    /// 錄到一半，同意書變了要做什麼。
    ///
    /// 抽成一支純函式**不是為了整潔**：它原本長在錄製迴圈裡，而那個迴圈需要
    /// 一個真的擷取後端才跑得起來——也就是說在這台開發機上永遠測不到，在
    /// Windows 上也只能靠手動點。一道測不到的隱私閘門，和沒有那道閘門的差別
    /// 只在文件上。
    // 用它的那個迴圈在 `#[cfg(windows)]` 裡，所以在這台開發機上「沒有人呼叫」
    // ——但測試呼叫得到，而那正是抽出來的目的。
    #[cfg_attr(not(windows), allow(dead_code))]
    #[derive(Debug, PartialEq, Eq)]
    pub enum Recheck {
        /// 什麼都沒變。
        Same,
        /// 第一張被撤回了：停。
        Stop,
        /// 第三張變了：換成留圖（true）或只留字（false）。
        Images(bool),
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn recheck(
        consent: &sister_core::consent::Consent,
        wants: WantsImages,
        storing: StoringImages,
    ) -> Recheck {
        // 第一張先看。它被撤回的時候，第三張是什麼已經不重要了。
        if !consent.allows_recording() {
            return Recheck::Stop;
        }
        let should = wants.enabled() && consent.allows_frames();
        if should == storing.enabled() {
            return Recheck::Same;
        }
        Recheck::Images(should)
    }

    /// 這一拍該不該叫醒慢路徑。只看 [`sister_capture::Tick`]，不查資料庫。
    ///
    /// - 新畫面（`Kept`）或鎖屏（`NoScreen`）：段落可能剛關上。
    /// - 剛從 `Idle` 恢復：SPEC §5.1「長停留後恢復」。
    /// - `Idle` 本身不叫——她還在閉眼，沒有新的資訊價值。
    #[cfg(any(windows, test))]
    fn should_ping_brain(tick: &sister_capture::Tick, was_idle: &mut bool) -> bool {
        use sister_capture::Tick::*;
        match tick {
            Idle => {
                *was_idle = true;
                false
            }
            Disabled | Paused => false,
            Kept { .. } | NoScreen => {
                *was_idle = false;
                true
            }
            Duplicate { .. } | Excluded { .. } => {
                let left_idle = *was_idle;
                *was_idle = false;
                left_idle
            }
        }
    }

    pub fn run(
        data_dir: &Path,
        config: Config,
        config_path: Option<PathBuf>,
        duration: Option<u64>,
    ) -> Result<()> {
        // 同意書擋在平台檢查**前面**。沒有同意就不該錄，這件事和這台機器有
        // 沒有擷取後端無關；而且放後面的話，這道閘門在非 Windows 上永遠碰不到。
        let (config, wants_images_by_config) = gate(data_dir, config)?;

        // **一個資料目錄只准一個 recorder。** 這道閘門以前只長在字母人那一邊
        // ——所以從字母人按兩次會被擋，但開兩個終端機各打一次 `sister record`
        // 不會。兩個行程對同一顆資料庫各錄一份，唯一看得出來的症狀是磁碟用得
        // 比講好的快一倍，而使用者會以為是保留期壞了。
        //
        // 敢直接擋是因為心跳自己會過期：一個當掉的 recorder 留下的時戳
        // 16 秒後就不算數，所以這裡不會把人鎖在門外。
        already_recording(data_dir)?;

        #[cfg(not(windows))]
        {
            let _ = (
                data_dir,
                config,
                config_path,
                duration,
                wants_images_by_config,
            );
            anyhow::bail!(
                "這個平台（{}）還沒有擷取後端。\n\n\
                 Phase 0 的目標平台是 Windows；核心與錄製迴圈本身是平台無關的，\n\
                 可以用腳本完整驗證：\n\n    \
                 sister replay scenarios/bill-lookup.json\n",
                std::env::consts::OS
            )
        }
        #[cfg(windows)]
        {
            windows_record(
                data_dir,
                config,
                config_path,
                duration,
                wants_images_by_config,
            )
        }
    }

    /// Ctrl-C 只設一個旗標，真正的收尾留給主迴圈。
    ///
    /// console handler 跑在另一條執行緒上，在那裡碰資料庫等於在 SQLite
    /// 交易中間插隊。設一個 bool 是這裡唯一安全的動作。
    #[cfg(windows)]
    static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    #[cfg(windows)]
    fn install_ctrl_c_handler() {
        use windows::Win32::System::Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler,
        };
        use windows::core::BOOL;

        unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
            match ctrl_type {
                CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
                    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
                    true.into()
                }
                _ => false.into(),
            }
        }
        let _ = unsafe { SetConsoleCtrlHandler(Some(handler), true) };
    }

    /// 開機那一段的心跳。
    ///
    /// 從 `Db::open` 到主迴圈第一次 `beat` 中間可能隔著好幾分鐘（見
    /// `windows_record` 裡的說明）。這個守衛替那段空窗蓋心跳，好讓
    /// `is_recording` 看得見一個「正在起來」的 recorder，而不是看見一片空白
    /// 然後放行第二個。
    ///
    /// 交棒之後這條執行緒就停了，心跳改由主迴圈自己蓋——**心跳要跟著那個真的
    /// 在做事的迴圈**，不然一個迴圈已經卡死的行程會永遠說自己在錄。
    ///
    /// 只有 `windows_record` 用得到它，但這裡**不加 `#[cfg(windows)]`**：它管的
    /// 是三個檔案動作，和平台無關，而唯一會回歸的方式（開機那一下沒蓋、或失敗
    /// 之後沒收）在這台開發機上測得到。加了 cfg 就要等到 Windows 才發現。
    // 代價是非 Windows 的正式編譯裡沒有人用它（用它的 `windows_record` 掛著
    // `cfg(windows)`），所以那邊得把 dead_code 關掉。
    /// 交棒的憑據：拿得到它，就代表心跳已經從開機守衛換到主迴圈了。
    ///
    /// 存在的理由是**順序**。開機那份能力報告不可以在開機那段寫（整段理由在
    /// 呼叫它的那一行旁邊），而一句註解擋不住下一次搬動——這一族的 bug 已經
    /// 犯過二十幾次，每一次都是「兩行各自都對，湊起來在說謊」。所以讓那個寫
    /// 入拿一個只有 [`BootBeat::hand_off`] 生得出來的東西：搬回上面去就編不過。
    #[cfg_attr(not(windows), allow(dead_code))]
    struct HandedOff;

    #[cfg_attr(not(windows), allow(dead_code))]
    struct BootBeat {
        alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        dir: PathBuf,
        handed_off: bool,
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    impl BootBeat {
        /// **蓋不上第一拍就不要回來。**
        ///
        /// `safe_to_kill_spawn` 的整條命，靠的是「心跳蓋在 `Db::open` 之前」。
        /// 上一版把那一拍寫成 `let _ =`，於是那條不變式講的是**順序**（真的）
        /// 而不是**效果**（假的）：寫失敗的時候呼叫端照樣往下走進 `Db::open`，
        /// 而磁碟上留著的是**上一場**的狀態——乾淨收工的墓碑、當掉的過期心
        /// 跳、全新安裝的空目錄，三種全部滿足 `at < spawned_at`，三種全部放行
        /// 落刀。補蓋的那條執行緒要等滿一個 `BEAT_EVERY_MS`，所以那個瞎掉的
        /// 窗口是**五秒**，不是幾微秒。
        ///
        /// 所以這裡重試，重試不成就讓 `record` 開不起來。這不是把小事鬧大：
        /// `recording.beat` 和 `sister.db` 躺在同一個資料夾裡，一秒之內連
        /// 三十個位元組都放不進去的地方，等一下也放不下一顆資料庫。與其開進
        /// 一個注定失敗的 `Db::open`、順便把那把刀的綠燈一起交出去，不如當場
        /// 講清楚。
        ///
        /// 一秒是個判斷，不是量出來的：短到使用者不會覺得卡，長到足夠讓防毒
        /// 軟體放開一個三十位元組的檔案。真的是磁碟滿了的話，等多久都一樣。
        fn start(data_dir: &Path) -> Result<Self> {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::time::{Duration, Instant};

            let dir = data_dir.to_path_buf();
            // **舊的停止請求在這裡清掉，不在主迴圈那一邊。**
            //
            // `stop.request` 是一個沒有時戳的檔案，它在不在就是全部的協定
            // （見 `control` 模組）。於是同一個位元要回答兩個問題：「這是我
            // 起來之前留下的嗎（丟掉）」還是「這是衝著我來的嗎（照做）」。
            // 分得開它們的**只有時間**——清理排在開機窗打開之前，之後寫進來
            // 的每一個請求就都是衝著這一場來的。
            //
            // 上一版清在 `Db::open` **之後**，於是整段開機是一個洞：他按下
            // 停止，`sister stop` 看得見這個守衛剛蓋的心跳、回一句「已經請
            // 她收工」，然後她開完資料庫、把那個請求刪掉、錄一整天。那顆一
            // 年份的資料庫要開好幾分鐘，所以這個洞不是理論上的。
            sister_core::control::clear_stop(&dir);
            // 第一下蓋在呼叫者這條執行緒上，不是丟給新執行緒去蓋：呼叫的人回
            // 去之後下一行就是 `Db::open`，中間不該留一段「心跳還沒出現」的
            // 空窗——那正是要補的洞。
            let mut last_err = None;
            for attempt in 0..10 {
                if attempt > 0 {
                    std::thread::sleep(Duration::from_millis(100));
                }
                match sister_core::heartbeat::beat_booting(&dir, sister_core::now_ms()) {
                    Ok(()) => {
                        last_err = None;
                        break;
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            if let Some(e) = last_err {
                return Err(e).with_context(|| {
                    format!(
                        "開不起來：{} 這個資料夾一秒之內連一行心跳都寫不進去。\
                         硬碟滿了，或者那個資料夾不給寫——資料庫也在同一個資料夾裡，\
                         所以先把它弄好再開始錄",
                        dir.display()
                    )
                });
            }
            let alive = std::sync::Arc::new(AtomicBool::new(true));
            let thread = std::thread::spawn({
                let alive = alive.clone();
                let dir = dir.clone();
                move || {
                    let every = Duration::from_millis(sister_core::heartbeat::BEAT_EVERY_MS as u64);
                    let mut last = Instant::now();
                    // 睡小段而不是睡滿一個間隔：交棒或失敗之後這條執行緒要在
                    // 零點幾秒內收乾淨，`Drop` 才不會卡著等它。
                    while alive.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(200));
                        if last.elapsed() >= every {
                            let _ =
                                sister_core::heartbeat::beat_booting(&dir, sister_core::now_ms());
                            last = Instant::now();
                        }
                    }
                }
            });
            Ok(Self {
                alive,
                thread: Some(thread),
                dir,
                handed_off: false,
            })
        }

        /// 主迴圈接手。之後 drop 不會再把心跳收掉——那是還在跑的 recorder 的
        /// 心跳，不是這個守衛的。
        fn hand_off(&mut self) -> HandedOff {
            self.handed_off = true;
            self.stop_thread();
            HandedOff
        }

        fn stop_thread(&mut self) {
            self.alive.store(false, std::sync::atomic::Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    impl Drop for BootBeat {
        fn drop(&mut self) {
            // 先把執行緒等停，再決定要不要收心跳。不等的話它有機會在 `stop`
            // 之後又蓋一次，於是磁碟上留下一個沒有人在跑的新鮮心跳——正好是
            // 這整段想避免的東西。
            self.stop_thread();
            if !self.handed_off {
                // 沒交棒就走了（`Db::open` 炸了之類）：這次開機沒成功，蓋一塊
                // 墓碑，不然接下來 16 秒字母人會說她在錄。
                sister_core::heartbeat::stop(&self.dir, sister_core::now_ms());
            }
        }
    }

    /// 把開機那份能力報告落地。**只有交棒之後叫得動**（見 [`HandedOff`]）。
    #[cfg(windows)]
    fn write_boot_report(
        _: &HandedOff,
        data_dir: &Path,
        report: &sister_core::capabilities::Report,
    ) {
        if let Err(e) = sister_core::capabilities::write(data_dir, report) {
            eprintln!("⚠  寫不出能力報告（設定頁會說「還不知道」）：{e:#}");
        }
    }

    #[cfg(windows)]
    fn windows_record(
        data_dir: &Path,
        config: Config,
        config_path: Option<PathBuf>,
        duration: Option<u64>,
        // 設定檔原本寫的「要不要留圖」。傳進來而不是讀 `config.capture`，
        // 因為那一份已經被 `gate` 按照同意書改過了——見 `run` 那邊的說明。
        //
        // `mut`：它也吃熱重載。凍在這裡的話，錄到一半把 `store_images` 關掉完全
        // 沒有作用，而 `sister doctor` 讀的是磁碟上那一份，會照著新設定說
        // 「保留畫面檔 ✗ 否（text-only 模式）」——他關掉了截圖、工具說關掉了，
        // 而她還在一張一張寫。
        mut wants_images_by_config: WantsImages,
    ) -> Result<()> {
        use sister_capture::windows::{self, Capabilities};
        use sister_capture::{Recorder, Tick};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("create {}", data_dir.display()))?;

        // **開機也要有心跳。** 底下這一段（`Db::open` 的 migration、能力探測、
        // 開場那次 prune）在一顆存了一年文字的資料庫上可能要跑好幾分鐘——
        // migration 003 要把整張 `text_chunks` 重讀一次重建 bigram 索引。而
        // 第一次 `heartbeat::beat` 排在那全部之後，所以**正在開機的 recorder
        // 對 `is_recording` 是隱形的**。
        //
        // 後果不是畫面難看：字母人等 25 秒之後說「她沒有起來」並把喚醒鈕放
        // 回來，第二下穿過那道 `is_recording` 閘門（recorder 沒有 lock file，
        // 也沒有 single instance），於是兩個 `sister record` 打同一顆資料庫。
        // 唯一的症狀是磁碟用得比講好的快一倍。
        let mut boot = BootBeat::start(data_dir)?;
        let mut db = Db::open(&crate::db_path(data_dir))?;
        // **先建後端、再問能力。** 反過來的話，「輸入 hook 裝上了沒」永遠
        // 是在 hook 還沒裝之前問的，於是永遠回報失敗——一則恆假的警告。
        let backend = windows::backend(&config)?;

        // 缺席的能力會讓某些排除規則整組失效，或讓她其實什麼都沒記住。
        // 這兩件事都要在開始錄之前講，不是藏在 doctor 裡等使用者自己去發現。
        let caps = Capabilities::current(&config);
        // **這裡不寫能力報告。** 它排在底下 `boot.hand_off()` 之後，理由寫在
        // 那裡——這個順序是有意義的，不是誰順手擺的。
        for warning in caps.broken_privacy_rules(&config.privacy) {
            println!("⚠  {}", warning.message);
        }
        for warning in caps.silently_degraded(&config) {
            println!("⚠  {warning}");
        }
        // 總開關關著的時候，每個 tick 都直接回 `Disabled`，而摘要的四個
        // 欄位剛好全部是 0——和「錄得好好的、只是螢幕沒變」長得一模一樣。
        // 一個打字打錯、或一個沒關回來的暫停，就能讓她整天什麼都沒記，
        // 而且沒有任何一行字提過。
        if !config.capture.enabled {
            println!(
                "⚠  capture.enabled = false：接下來每一個 tick 都會直接跳過，\
                 什麼都不會記錄。改成 true 才會真的開始錄。"
            );
        }
        // 暫停是**不會自己過期**的（見 `pause` 模組），所以「上禮拜按了暫停、
        // 這禮拜開起來發現整週都沒錄」是一條真實的路。開場就要講，而且要講
        // 從什麼時候開始——不然使用者只會看到一個永遠是 0 的摘要。
        if sister_core::pause::is_paused(data_dir) {
            match sister_core::pause::paused_since(data_dir) {
                Some(ts) => println!(
                    "⚠  目前是暫停狀態（從 {} 起），不會記錄任何東西。\
                     在字母人上按一下暫停鍵解除，或刪掉 {}。",
                    crate::fmt::timestamp(ts),
                    sister_core::pause::flag_path(data_dir).display()
                ),
                None => println!(
                    "⚠  目前是暫停狀態，不會記錄任何東西。刪掉 {} 可解除。",
                    sister_core::pause::flag_path(data_dir).display()
                ),
            }
        }
        // doctor 會挑出「寫了也不會命中」的網址規則，但一個只跑 record 的人
        // 永遠看不到那份清單——而那正是把網銀畫面錄一整年的那種規則。
        for (rule, why) in sister_core::config::suspicious_url_rules(&config.privacy.excluded_urls)
        {
            println!("⚠  這條排除規則寫了也不會命中：{rule} — {why}");
        }

        // 開錄之前先讓過期的東西消失。放在這裡而不是收工時，是因為收工
        // 有太多種方式不會發生（Ctrl-C、當機、關機、拔電），而開錄只有
        // 一種方式：她真的開始了。一個只在乾淨結束時才生效的保留期，
        // 就是一個在最需要它的機器上永遠不生效的保留期。
        // 清理**一律**帶著畫面資料夾，即使現在是 text-only。
        // `store_images = false` 的意思是「不要再寫新的圖」，不是「以前
        // 寫的那些不存在」——不帶的話那些舊圖會永遠留在磁碟上。
        let frames_root = crate::ops::prune::frames_dir(data_dir);
        match db.prune(sister_core::now_ms(), &config.retention, Some(&frames_root)) {
            Ok(report) if !report.is_empty() => {
                println!("○ 保留期清理");
                crate::ops::prune::print_report(&report, false);
            }
            Ok(_) => {}
            // 清不掉不該擋住錄製，但也絕對不能安靜——這整個模組的存在
            // 理由就是「說好會消失的東西沒有消失」不可以沒有人知道。
            Err(e) => println!("⚠  保留期清理失敗，過期的資料還在：{e:#}"),
        }

        // 用上面算好的 `frames_root`，不是再 `join("frames")` 一次。同一個路徑
        // 在這個函式裡現在有三個用途（開錄、定期清理、第三張同意書中途切換），
        // 各自算一遍的話，只要有一次拼錯，症狀就是「她寫圖寫到 A、清理清 B」。
        let images = config.capture.store_images.then(|| frames_root.clone());
        let interval =
            Duration::from_millis(config.capture.min_interval_ms.max(crate::ops::MIN_TICK_MS));
        // config 等一下會被 Recorder 吃掉，但收尾的摘要與定期清理還需要這幾項
        let config_ocr = OcrEnabled::from_config(&config);
        let image_budget_mb = config.capture.max_image_mb_per_day;
        let retention = config.retention.clone();
        let prune_images = frames_root.clone();
        // 腦走慢路徑：自己一條執行緒、自己一條資料庫連線。熱路徑只留 ping。
        let brain_cfg = config.brain.clone();
        let mut rec = Recorder::new(backend, db, config, images)?;

        install_ctrl_c_handler();
        // 磁碟歸因的開場快照排在 Recorder 建好之後：schema／migration／session_start
        // 都是開機成本，不是這場錄製長出來的資料。失敗留成 Err，收尾照樣繼續；
        // 一個量不到的開場不能在最後冒充 0 B。
        let db_disk_at_start = rec.db().disk_snapshot().map_err(|e| format!("{e:#}"));
        let sidecars_at_start = crate::disk_attribution::sidecar_snapshot(data_dir);
        // 從這一刻開始算足跡。Phase 0 要留下 CPU／RAM／磁碟三類實測；RAM 與
        // 磁碟另有阻擋上限。在這之前沒有辦法知道它們是多少——一個量不到的
        // 數字不是基準，也不是預算。
        let mut footprint = sister_capture::footprint::Footprint::new();
        let deadline = duration.map(|d| Instant::now() + Duration::from_secs(d));
        println!(
            "● 錄製中（{}），每 {} ms 一次。Ctrl-C 停止。",
            backend_name().unwrap_or("?"),
            interval.as_millis()
        );

        // 保留期要在錄製途中反覆跑，不能只在開機時跑一次。一個連續開著
        // 三十天的行程，只清一次的話從第 31 天起保留期就等於不存在——
        // 而且它跑得越久、越沒有人重開，這個洞就越大。六小時一次：夠密到
        // 不會累積出一整天的過期資料，夠疏到不會變成足跡本身的一部分。
        const PRUNE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
        let mut last_prune = Instant::now();

        // 能力報告也一樣不能只在開機時寫一次，而且理由更硬：UIA 卡三次之後
        // 就**永久**投降，從那一刻起 `excluded_urls` 一條都不生效——沒有錯誤、
        // 沒有例外，只是網銀跟登入頁開始被錄進去。以前唯一問過這件事的是收工
        // 時的一行 `println!`，印進沒有人會開的 `record.log`；而使用者是在設定
        // 頁上打那些規則的，那一頁拿著一份開機時的「一切正常」，一句話都不說。
        //
        // 一分鐘一次：這個檔案兩百多個位元組，寫一次是一次 write + 一次 rename。
        // 對一個要開一整天的迴圈來說成本是零，而使用者能忍受的「我的網銀規則
        // 什麼時候壞掉的」誤差，一分鐘綽綽有餘。
        const CAPS_EVERY: Duration = Duration::from_secs(60);
        let mut last_caps = Instant::now();
        // 開機那一份在交棒之後才寫（見那裡），而它沒有這一場的證據。這個閉包是
        // 「開機探測 + 這一場路上發生的事」的唯一組法——收工時也用同一個，
        // 不然兩邊會慢慢長成兩份不一樣的報告。
        //
        // `..caps.report()` 帶的是開機那幾欄，而它自己會蓋上現在的時間戳
        // ——這個檔案上的 `at` 講的是「這份報告描述的是哪一刻」，不是
        // 「開機是什麼時候」。設定頁那句話就是照著它寫的。
        let caps_report = |s: &sister_capture::RecorderStats,
                           live: sister_core::capabilities::UrlCapture| {
            sister_core::capabilities::Report {
                url_capture: live,
                browser_ticks: s.browser_ticks,
                url_reads: s.url_reads,
                ..caps.report()
            }
        };

        // 設定檔熱重載。5 秒一次是因為它的使用情境是「我剛剛在設定頁按了儲存」
        // ——那個人正看著螢幕等它生效。一次 `stat` 的成本在這個間隔下是零。
        const CONFIG_EVERY: Duration = Duration::from_secs(5);
        let watched = config_path.clone().or_else(Config::default_path);
        let mut config_watch = ConfigWatch::new(watched.as_deref());
        let mut last_config_check = Instant::now();
        // 同意書用同一個節拍。它不看 mtime，所以自己一個計時器。
        const CONSENT_EVERY: Duration = Duration::from_secs(5);
        let mut last_consent_check = Instant::now();
        // 設定檔剛換掉 `store_images`：別等下一個節拍。同意書和設定檔是同一道
        // 閘門的兩半（`recheck`），所以「設定改了」也是它的觸發條件之一。
        let mut consent_dirty = false;
        // 心跳。字母人是另一個行程，它沒有別的辦法知道「現在到底有沒有人在
        // 錄」——`sessions.ended_at` 在當掉的時候永遠停在 NULL，而閒置時
        // 資料庫本來就會好一陣子沒有新資料。開機那一段由 `BootBeat` 蓋著，
        // 這裡把它接過來：交棒之後那個執行緒就停了，心跳從此跟著這個迴圈走
        // ——一個蓋得動心跳但迴圈已經卡死的行程，不該還在說自己在錄。
        let handed_off = boot.hand_off();
        let _ = sister_core::heartbeat::beat(data_dir, sister_core::now_ms());
        let mut last_beat = Instant::now();

        // 開機那份能力報告寫在這裡，**不是**在上面探測完的那一刻。
        //
        // 那份報告只有開機探測，沒有任何一場的證據（`has_session_evidence`
        // 是 false），而它蓋掉的那一份可能有——「UIA 在昨天下午三點卡住太多
        // 次，從那之後位址列一個字都讀不到」這種事，只有跑過那一場的行程知道，
        // 拿一份全新的 UIA 去問永遠問不出來（見 `Report::has_session_evidence`
        // 那整段）。
        //
        // 那一段是為了擋 `doctor` 寫的，而 `doctor` 是使用者偶爾跑一次的東西。
        // 這裡是**每次開機都會跑**的那一個，它從來沒有經過那道閘門。
        //
        // 而且時間點正好最壞：`Db::open` 在一顆大資料庫上要跑好幾分鐘，那幾
        // 分鐘裡心跳是 `Booting`，於是設定頁和 doctor 都會照著
        // `keep_capabilities` 說「上一場留下的報告說」——指著一份這個行程幾秒
        // 前才蓋上去的、乾淨的、時戳很新的報告。兩句話各自都對，湊起來是替一
        // 件沒發生的事背書。
        //
        // 挪到交棒之後，那三句話就都回到真的：開機中讀到的**真的**是上一場
        // 留下的，而從這一行開始心跳是 `Recording`，這份報告**真的**是正在錄
        // 的那個行程寫的。
        //
        // 為什麼還是要寫這一份（而不是等 60 秒後第一次 `CAPS_EVERY`）：底下
        // 那幾行 `⚠` 印在 stdout，而字母人開起來的 recorder，stdout 是
        // `record.log`——一個沒有人會開的檔案。使用者是在設定頁上打那些排除
        // 規則的，所以那一頁才是這句話該出現的地方。
        //
        // 寫不出來不擋錄製：少一行警告，比少一場記錄好。
        //
        // 那個 `handed_off` 不是裝飾：它是 `hand_off()` 唯一的產物，而這一行
        // 要用到它。把這段搬回上面去就編不過——一句註解擋不住下一次搬動，
        // 一個型別可以。
        write_boot_report(&handed_off, data_dir, &caps.report());
        // 舊的停止請求**已經清掉了**，清在 `BootBeat::start` 裡——開機窗打開
        // 的那一刻，不是這裡。清在這裡的話，他在開機那幾分鐘按的停止會被自己
        // 刪掉；理由寫在那支函式上面。
        //
        // 保留期也吃熱重載（設定頁的 TTL 那一欄），所以它不能再是 `let`。
        let mut retention = retention;

        let mut wake = match sister_core::wakeup::Handle::maybe_spawn(
            data_dir,
            brain_cfg.clone(),
            sister_core::now_ms(),
        ) {
            Ok(h) => h,
            Err(e) => {
                println!("⚠ 解釋層執行緒開不起來（錄製照跑）：{e:#}");
                None
            }
        };
        if wake.is_none() && !sister_core::wakeup::armed(&brain_cfg) {
            println!("  解釋層這一場一次都不會醒：還沒設定 [brain] command。");
        }
        let mut was_idle = false;

        let mut last_report = Instant::now();
        // 為什麼停的。預設是 Ctrl-C，因為 `while` 那個條件是唯一一條不經過
        // 任何 `break` 的出口——那條路上沒有地方可以設定它。
        let mut end_reason = sister_core::model::EndReason::Interrupted;
        while !STOP.load(Ordering::SeqCst) {
            if let Some(d) = deadline
                && Instant::now() >= d
            {
                end_reason = sister_core::model::EndReason::Duration;
                break;
            }

            // 字母人（或別的什麼人）按了「停止」。跟暫停用同一個節拍去問，因為
            // 它們是同一種東西：一次 `stat`，而按下去的那個人正看著螢幕等它生效。
            if sister_core::control::take_stop(data_dir) {
                println!("  ■ 收到停止的請求，這就收工。");
                end_reason = sister_core::model::EndReason::Requested;
                break;
            }

            // 暫停鍵在**另一個行程**上（字母人），所以唯一的辦法是每個 tick
            // 去問一次那個旗標檔。一次 `stat` 是微秒等級，相對於一個 tick
            // 裡最便宜的一步都還小兩個數量級——不值得為它做快取。
            let now = sister_core::now_ms();
            match rec.set_paused(sister_core::pause::is_paused(data_dir), now) {
                Ok(true) if rec.is_paused() => println!("  ⏸ 已暫停——她不看了，直到你解除。"),
                Ok(true) => println!("  ▶ 已解除暫停。"),
                Ok(false) => {}
                // 事件寫不進去就不能宣稱已經暫停：使用者會以為停了，實際上
                // 下一行 tick 照錄。寧可整個 tick 跳過。
                Err(e) => {
                    tracing::warn!("暫停狀態切換失敗：{e:#}");
                    std::thread::sleep(interval);
                    continue;
                }
            }

            match rec.tick(now) {
                // 單次 tick 失敗不該終止 session：抓不到畫面的原因多半是
                // 暫時的（切換使用者、顯示器休眠），下一秒就好了
                Err(e) => tracing::warn!("tick failed: {e:#}"),
                Ok(tick) => {
                    if should_ping_brain(&tick, &mut was_idle)
                        && let Some(w) = wake.as_ref()
                    {
                        w.ping();
                    }
                    if let Tick::Kept {
                        frame_id,
                        ocr_blocks,
                        facts,
                    } = tick
                    {
                        tracing::debug!("frame #{frame_id}：{ocr_blocks} 段文字、{facts} 個事實");
                    }
                }
            }

            // 設定改了就當場換上。**讀不出來就維持原樣，絕不退回預設值**——
            // 預設值比任何一份使用者自訂的 blocklist 都寬鬆，所以一個打錯的
            // TOML 會安靜地把排除規則全部拿掉。那是這裡唯一不能犯的錯。
            if let Some(path) = watched.as_deref()
                && last_config_check.elapsed() >= CONFIG_EVERY
            {
                last_config_check = Instant::now();
                if config_watch.changed(path) {
                    // 三種答案裡有兩種是「別動」，而那條規則住在 core
                    // （`Config::reload`），因為這個迴圈只在 Windows 上編譯——
                    // 開發機和 CI 都跑不進來，寫在這裡等於沒有人驗得到。
                    use sister_core::config::Reload;
                    match Config::reload(path) {
                        Reload::Fresh(fresh) => {
                            println!(
                                "  ⟳ 設定檔換了：排除 {} 個 app、{} 條網址；\
                                 保留期 畫面 {} 天、文字 {} 天",
                                fresh.privacy.excluded_apps.len(),
                                fresh.privacy.excluded_urls.len(),
                                fresh.retention.frames_days,
                                fresh.retention.text_days
                            );
                            // 新規則裡「寫了也不會命中」的那幾條要當場講。
                            // 這是使用者最可能剛剛打錯字的那一刻。
                            for (rule, why) in sister_core::config::suspicious_url_rules(
                                &fresh.privacy.excluded_urls,
                            ) {
                                println!("    ⚠  這條寫了也不會命中：{rule} — {why}");
                            }
                            retention = fresh.retention.clone();
                            // `capture.store_images` 以前不在這裡，於是它凍在
                            // 開機那一刻——設定頁上關掉截圖、`doctor` 照著磁碟
                            // 上那一份說「text-only 模式」，而這個迴圈還在一張
                            // 一張寫。真正動手的是下面那道 `recheck`（同意書仍
                            // 然是上限），這裡只負責把他寫下的意思送過去，並且
                            // 叫它別等下一個節拍。
                            let fresh_wants_images = WantsImages::from_reloaded_config(&fresh);
                            if wants_images_by_config != fresh_wants_images {
                                wants_images_by_config = fresh_wants_images;
                                consent_dirty = true;
                            }
                            rec.set_privacy(fresh.privacy);
                            if let Some(w) = wake.as_ref() {
                                w.set_config(fresh.brain.clone());
                            } else if sister_core::wakeup::armed(&fresh.brain) {
                                match sister_core::wakeup::Handle::maybe_spawn(
                                    data_dir,
                                    fresh.brain.clone(),
                                    sister_core::now_ms(),
                                ) {
                                    Ok(h) => wake = h,
                                    Err(e) => {
                                        println!("  ⚠ 解釋層執行緒開不起來（錄製照跑）：{e:#}")
                                    }
                                }
                            }
                        }
                        Reload::Missing => println!(
                            "  ⚠  設定檔不見了（{}）。**繼續用舊的那一份**——\
                             真要回到預設值請放一個空的設定檔。",
                            path.display()
                        ),
                        Reload::Broken(why) => println!(
                            "  ⚠  設定檔讀不出來，**繼續用舊的那一份**（不是預設值）：{why}"
                        ),
                    }
                }
            }

            // 同意書也吃熱重載，而且它比設定檔更不能等。PRIVACY.md 上寫的是
            // 「各自獨立、各自隨時撤得掉」——只在開機時讀一次的話，那句話真正
            // 的意思是「下次重開的時候才撤得掉」，而剛按下撤回的那個人，正是
            // 最不該被要求等待的那一個。
            //
            // 不做 mtime 去抖：這個檔案是幾百個位元組，而 `consent::load` 的
            // 失敗方向是「當作沒簽」。少一層快取就少一種「檔案已經變了、我還
            // 拿著舊答案」的可能。
            // 「我還活著」。**暫停中也要蓋**——暫停是她閉著眼睛，不是她走了，
            // 而字母人要分得出這兩件事：一個要按「繼續」，一個要去開 recorder。
            if last_beat.elapsed().as_millis() as i64 >= sister_core::heartbeat::BEAT_EVERY_MS {
                last_beat = Instant::now();
                if let Err(e) = sister_core::heartbeat::beat(data_dir, sister_core::now_ms()) {
                    // 蓋不動不值得停止錄製——真正的工作還在做。但要講一次，
                    // 因為字母人從現在起會說「沒有人在記錄」，而那是錯的。
                    eprintln!("  ⚠ 心跳寫不進去（字母人會以為沒有人在錄）：{e}");
                }
            }

            // 能力報告：見 `CAPS_EVERY`。寫不出來**不重試也不吵**——這一行的
            // 失敗方向是設定頁上那句警告晚一分鐘出現，而它一分鐘後就會再試
            // 一次。在這裡 `eprintln!` 的話，一顆磁碟滿了的機器會每分鐘吐一行
            // 到 `record.log` 裡，把真正的原因埋掉。
            if last_caps.elapsed() >= CAPS_EVERY {
                last_caps = Instant::now();
                let report = caps_report(
                    rec.stats(),
                    sister_capture::Backend::url_capture(rec.backend()),
                );
                let _ = sister_core::capabilities::write(data_dir, &report);
            }

            if consent_dirty || last_consent_check.elapsed() >= CONSENT_EVERY {
                let by_config = std::mem::take(&mut consent_dirty);
                last_consent_check = Instant::now();
                let consent = sister_core::consent::load(data_dir);
                match recheck(
                    &consent,
                    wants_images_by_config,
                    StoringImages::from_recorder(&rec),
                ) {
                    // 他剛剛改了 `store_images`，而實際行為沒有跟著變 = 另一個
                    // 條件在擋。安靜掉的話，他會以為那一行寫了就生效了——而這是
                    // 一句只要他不去翻 frames/ 就永遠不會被戳破的話。
                    Recheck::Same if by_config => println!(
                        "  ⟳ 設定檔的 store_images 改了，但實際行為沒變：{}",
                        if wants_images_by_config.enabled() {
                            "第三張同意書沒簽，所以還是只記字。\
                             （要留圖請跑 `sister consent --grant frame-storage`）"
                        } else {
                            "第三張同意書本來就沒簽，這一輪本來就沒在留圖。"
                        }
                    ),
                    Recheck::Same => {}
                    // 撤回不是暫停。暫停是「先別看」，撤回是「我收回那句話」
                    // ——所以這裡停的是整場錄製，和開機時那道閘門對稱。當成
                    // 暫停處理的話，稽核紀錄上會留下一筆理由是假的 pause。
                    Recheck::Stop => {
                        println!(
                            "\n⏹ 第一張同意書被撤回了，錄製到此為止。\n  \
                             要再開始請跑：sister consent --grant local-recording"
                        );
                        end_reason = sister_core::model::EndReason::ConsentRevoked;
                        break;
                    }
                    // 底下兩句以前一律說「第三張同意書」，因為那時候只有同意書
                    // 動得了它。現在設定檔也動得了，而說錯的話他會去撤一張已經
                    // 撤過的同意書、或去改一個根本沒關的設定。
                    //
                    // 講的是**現在這兩個條件長什麼樣**，不是猜剛剛動的是哪一個
                    // ——後者需要記住上一輪的值，而那是一份會和事實漂開的副本。
                    Recheck::Images(true) => {
                        println!("  ⟳ 設定檔和第三張同意書現在都說要留圖：從這一刻起會留截圖。");
                        rec.set_image_dir(Some(frames_root.clone()));
                    }
                    Recheck::Images(false) => {
                        let why = if !consent.allows_frames() {
                            "第三張同意書被撤回了"
                        } else {
                            "設定檔把 store_images 關掉了"
                        };
                        println!(
                            "  ⟳ {why}：從這一刻起只記螢幕上的字，\
                             不會再寫任何截圖。（先前寫下的那些還在，要清掉請用\
                             時間軸的「忘掉這一段」或 sister prune。）"
                        );
                        rec.set_image_dir(None);
                    }
                }
            }

            if last_prune.elapsed() >= PRUNE_EVERY {
                last_prune = Instant::now();
                match rec
                    .db_mut()
                    .prune(sister_core::now_ms(), &retention, Some(&prune_images))
                {
                    Ok(r) if !r.is_empty() => {
                        println!("  ○ 保留期清理");
                        crate::ops::prune::print_report(&r, false);
                    }
                    Ok(_) => {}
                    Err(e) => println!("  ⚠  保留期清理失敗，過期的資料還在：{e:#}"),
                }
            }

            // 每一拍量一次，不是每分鐘一次。`peak_rss` 這個名字承諾的是峰值，
            // 而一分鐘取一次樣的話，任何短於一分鐘的尖峰都和取平均一樣看不
            // 見——2560×1440 的一次抓圖握著 14.7 MB 的 RGBA 加 GDI bitmap 加
            // 縮圖緩衝，只有幾十毫秒，也就是取樣週期的 0.007%。
            //
            // 成本：`sample()` 是一次 /proc 讀取（Linux）或兩個 Win32 呼叫
            // （Windows），比這個迴圈每拍都付的剪貼簿輪詢還便宜。CPU 那邊不
            // 受影響——它是拿 `first` 和 `latest` 兩個累計值相減算的，多量
            // 幾次只會讓 `latest` 更新。
            footprint.tick();

            if last_report.elapsed() >= Duration::from_secs(60) {
                last_report = Instant::now();
                // 暫停時如果照原樣印那四個數字，讀起來就是「一切正常，只是
                // 這一分鐘沒有新東西」——那正是暫停最危險的失效模式：
                // 使用者以為她還在錄。
                if rec.is_paused() {
                    println!("  ⏸ 仍在暫停中，這一分鐘沒有記錄任何東西。");
                } else {
                    let s = rec.stats();
                    println!(
                        "  … {} tick：保留 {}、重複 {}、排除 {}、資料庫留下 {} 行字{}",
                        s.ticks,
                        s.kept,
                        s.duplicates,
                        s.excluded,
                        s.ocr_blocks,
                        match (footprint.cpu_percent(), footprint.peak_rss_bytes()) {
                            (Some(cpu), Some(rss)) => format!(
                                "；CPU {cpu:.1}%、RAM 峰值 {}",
                                crate::fmt::bytes(rss as i64)
                            ),
                            _ => String::new(),
                        }
                    );
                }
            }

            std::thread::sleep(interval);
        }

        // 腦在另一條執行緒。錄製迴圈停了之後才請它把最後一段想完——
        // 等它的時候不再抓畫面，所以不佔熱路徑。
        //
        // **先講一句再等。** `Handle::shutdown` 會把心跳改成「沒在錄、但還
        // 佔著」（想最後一段），上限是兩輪 CLI、每輪 120 秒。不講的話那兩
        // 分鐘裡字母人只看得到「沒在錄」，他按下開始會撞上佔著閘門。
        if wake.is_some() {
            println!("{}", sister_core::wakeup::shutdown_wait_notice());
        }
        let wake_report = match wake.take() {
            Some(h) => h.shutdown(),
            None if sister_core::wakeup::armed(&brain_cfg) => sister_core::wakeup::Report {
                armed: true,
                open_failed: Some("執行緒沒起來".into()),
                ..sister_core::wakeup::Report::unarmed()
            },
            None => sister_core::wakeup::Report::unarmed(),
        };

        // 走人之前先把心跳收掉。留給 16 秒的逾時去猜的話，那段時間裡字母人
        // 會說她還在錄，而她已經走了——**說她還在錄卻沒在錄**，是這兩個狀態
        // 裡比較危險的那一個。放在 `finish()` 之前，因為那一步會寫資料庫，
        // 可能失敗，而失敗不該讓一個錯的「還在錄」留在磁碟上。
        //
        // 想最後一段的那一拍（`beat_thinking`）到這裡結束：腦已經加入，墓碑
        // 蓋上去之後 `is_occupied` 才放開。順序不能倒——倒了的話墓碑會在 CLI
        // 還跑著的時候放行第二個 recorder。
        sister_core::heartbeat::stop(data_dir, sister_core::now_ms());

        let stats = rec.stats().clone();
        // 收工前問一次「這段路上掉了什麼」。`doctor` 只看得到開機那一瞬間，
        // 而 UIA 會在半路上永久投降——那之後 excluded_urls 一條都不生效，
        // 卻沒有任何地方會講。見 `Backend::url_capture`。
        //
        // 最後再蓋一次報告：迴圈那個一分鐘的節拍剛好可以錯過最後一分鐘，
        // 而「最後一分鐘才壞掉」和「壞了一整天」對設定頁是同一件事。
        let final_report = caps_report(&stats, sister_capture::Backend::url_capture(rec.backend()));
        let _ = sister_core::capabilities::write(data_dir, &final_report);
        // 句子只有一個出處（`capabilities::Report`）。這裡和設定頁印的是同一
        // 份，拿的也是同一份規則清單——`rec.privacy()` 是熱重載之後的那一份，
        // 不是開機時那一份，不然中途新加的規則不會被算進「幾條失效了」。
        let lost = final_report.broken_privacy_rules(rec.privacy());
        // **先收下這個結果，不要在這裡 `?` 出去。**
        //
        // `finish()` 要寫兩列進資料庫（session_end 事件、`end_session`），所以
        // 它會在磁碟滿、資料庫被鎖、檔案被防毒抓走的時候失敗——而那些正好是
        // 底下這整段摘要**最有用**的時候：保留了幾張、排除了幾次、OCR 活著沒、
        // 磁碟吃了多少，還有那幾行「錄製途中失去的能力」。
        //
        // 裸的 `?` 讓它們一行都印不出來。他錄了一整天，最後看到的只有一句
        // `database is locked`——而那一天到底錄到了什麼，沒有第二個地方可以問。
        let finished = rec.finish(end_reason);

        // 先凍結足跡，再跑 dbstat 與目錄掃描。歸因本身是診斷成本，不屬於這場
        // 錄製；若掃完才取樣，CPU 分子停在舊樣本、牆上時間卻繼續走，會把平均
        // CPU 悄悄壓低。磁碟的收尾快照仍排在 finish() 後，才能包含 session_end。
        footprint.tick();
        let footprint_elapsed = FootprintElapsedSecs::from_footprint(&footprint);
        let footprint_cpu_seconds = footprint.cpu_seconds_used();
        let mut footprint_measured =
            FootprintMeasured::measure(&footprint, &stats, None, image_budget_mb);
        let disk_attribution = crate::disk_attribution::AttributionInput {
            db_before: db_disk_at_start,
            db_after: rec.db().disk_snapshot().map_err(|e| format!("{e:#}")),
            sidecars_before: sidecars_at_start,
            sidecars_after: crate::disk_attribution::sidecar_snapshot(data_dir),
            session_image_bytes: stats.image_bytes,
        };
        footprint_measured.disk.delta_bytes = disk_attribution.budget_delta_bytes().ok();
        let footprint_report = footprint_measured.report(footprint_elapsed);
        let disk_attribution_report = crate::disk_attribution::render(&disk_attribution);

        println!(
            "\n完成：{} tick → 保留 {}、重複 {}、排除 {}、無畫面 {}",
            stats.ticks, stats.kept, stats.duplicates, stats.excluded, stats.no_screen
        );
        // 排在最前面：一場整拍都在炸的錄製，底下每一行都在描述一個空集合。
        if let Some(line) = tick_failures_line(&stats) {
            println!("{line}");
        }
        report_idle(&stats);
        report_exclusions(&stats);
        let storing_images = StoringImages::from_recorder(&rec);
        report_ocr(&stats, config_ocr, storing_images);
        // 問 recorder 而不是問開機時的設定：第三張同意書中途可能被撤回或簽回來，
        // 而這一段的用途是「她剛剛到底有沒有在寫圖」。拿開機時那份來答，會在
        // 使用者中途撤回之後喊「保留了 12 張畫面卻一張圖都沒寫」的假警報。
        report_images(&stats, rec.timings(), image_budget_mb, storing_images);
        report_timings(
            rec.timings(),
            TickCounts::from_stats(&stats),
            footprint_cpu_seconds,
        );
        report_footprint(&footprint_report);
        for line in disk_attribution_report {
            println!("{line}");
        }
        for line in &lost {
            println!("  ⚠  錄製途中失去的能力：{}", line.message);
        }
        println!("{}", sister_core::wakeup::format_report(&wake_report));
        // 話講完了才把錯誤帶出去。多一句是因為這個失敗有一個看不見的後果：
        // 沒有 `end_session` 的那一場，之後每一個地方都會把它算成「當機」
        // （見 `Db::last_session` 那段）。他明天打開設定頁會看到一句「上一次
        // 錄製沒有正常結束」，而剛剛的收工是好好的——錯的是這一行。
        if finished.is_err() {
            println!("\n上面那些是真的，但這一場在資料庫裡沒有收乾淨——之後她會把它算成一次當機。");
        }
        finished?;
        Ok(())
    }

    /// 她自己佔了多少。
    ///
    /// Phase 0 的驗收條件裡還有兩個足跡上限（RAM < 400MB、磁碟 < 300MB/天），
    /// 而在這一段出現之前**沒有任何辦法知道它們是多少**。CPU 仍然照實量，
    /// 但 Ted 在 alpha.46 真機量到活躍寫程式 44.0% 後選擇保留記憶密度；
    /// 2026-08-23 起它不再是 Phase 0 的阻擋門檻，也沒有另造一條 45% 接受線。
    /// 一個量不到的預算不是預算，是一句話——而 README 上遲早要寫這些數字，
    /// 那就必須是她自己量出來的，不是我開工作管理員瞄一眼記下來的。
    ///
    /// 量不到就不印。印一個 0 或一個從三分鐘外推出來的「每天 300MB」，
    /// 都會變成一個很有說服力的假消息，而且會被抄進文件裡。
    /// Phase 0 的驗收預算（PHASES.md）。
    ///
    /// 寫在程式裡而不是只寫在文件裡，是因為文件不會在超標的時候出聲。
    /// 實測那次是 RAM 401 MB、磁碟 11.4 GB/天，而摘要照樣平鋪直敘地印出來，
    /// 沒有任何一個字說「超過哪條仍有效的門」——要靠讀的人自己記得預算，
    /// 再自己心算。她應該自己講。
    #[cfg(any(windows, test))]
    const BUDGET_RSS_BYTES: u64 = 400 * 1024 * 1024;
    #[cfg(any(windows, test))]
    const BUDGET_DISK_PER_DAY: f64 = 300.0 * 1024.0 * 1024.0;

    /// 磁碟外推的三個數字，圖那一半已經夾在它自己的天花板上。
    ///
    /// **外推不可以穿過自己的天花板。** `bytes_per_day` 做的是「這段的速率
    /// × 86400」，而圖那一半跑不到 86400 秒就會自己停：`max_image_mb_per_day`
    /// 是一道真的門，額度在開機時從資料庫的 `image_bytes_since(今天)` 補回來
    /// ——重開也不會歸零，所以它真的是一天的上限，不是一次執行的上限。
    ///
    /// 不夾的話，一段十分鐘的爆量會外推成「11.4 GB/天」：算術上正確，而那
    /// 一天實際上不可能發生。更糟的是接在後面的建議「調小
    /// `max_image_mb_per_day`」——那個上限根本碰不到，調它不會讓任何數字變小。
    /// 這正是這個專案的招牌失效方式：每一行都對，合起來說謊。
    ///
    /// 不是 `#[cfg(windows)]` 而是 `any(windows, test)`：`report_footprint`
    /// 整個是 Windows 限定的，而這段算術是它唯一會算錯的地方。跟著一起
    /// 關在 Windows 門後的話，開發機一次都跑不到，也就沒有任何一個回退
    /// 測試驗得了它——上面那個 11.4 GB 的錯就是這樣活下來的。
    #[cfg(any(windows, test))]
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct DiskProjection {
        /// 一天總量，圖那一半已經夾過。
        per_day: f64,
        /// 圖那一半，夾過。`None` = 這段有東西被刪掉，拆不開。
        images: Option<f64>,
        /// 圖那一半，沒夾。和 `images` 不同就代表門會關。
        images_raw: Option<f64>,
    }

    #[cfg(any(windows, test))]
    impl DiskProjection {
        /// `cap_bytes` = 0 代表不設限（和 `max_image_mb_per_day = 0` 同義）。
        fn clamp(raw_total: f64, raw_images: Option<f64>, cap_bytes: ImageBudgetBytes) -> Self {
            let images = raw_images.map(|img| match cap_bytes.bytes() {
                0 => img,
                cap => img.min(cap as f64),
            });
            // 圖被夾掉多少，總量就跟著少多少——其他那一半沒有天花板，原樣留著。
            let per_day = match (raw_images, images) {
                (Some(raw), Some(capped)) => raw_total - (raw - capped),
                _ => raw_total,
            };
            Self {
                per_day,
                images,
                images_raw: raw_images,
            }
        }

        /// 一天的圖額度照這段的速度撐多久（秒）。`None` = 沒被夾住。
        fn budget_lasts_secs(&self) -> Option<f64> {
            let (raw, capped) = (self.images_raw?, self.images?);
            (raw > capped && capped > 0.0).then(|| capped / raw * 86_400.0)
        }
    }

    /// 足跡回報必須自己帶著量測條件走。沒有畫面時明講量不到，不能讓 `0×0`
    /// 同時表示「真的解析度是零」和「這場沒抓到畫面」。負載則不是程式能從
    /// 畫面推知的事，留一個明確待填欄位，讓貼回來的人不能只貼漂亮的數字。
    #[cfg(any(windows, test))]
    fn footprint_context(version: &str, frame_size: Option<(u32, u32)>) -> String {
        let screen = match frame_size {
            Some((width, height)) => format!("最後一次抓到的畫面 {width}×{height}"),
            None => "螢幕解析度量不到（這場沒有成功抓到畫面）".to_string(),
        };
        format!("版本 sister {version}；{screen}；負載：程式量不到，貼回時請註明當時在做什麼")
    }

    /// CPU 平均值的百分比。獨立型別讓它和其他 `f64` 接錯時直接編譯失敗。
    #[cfg(any(windows, test))]
    #[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
    struct CpuPercent(f64);

    /// 足跡凍結時已經過的牆上秒數。建構子自己問 `Footprint`，不讓 Windows
    /// 接線層把同樣是 `f64` 的 CPU 秒數塞進每日外推分母。
    #[cfg(any(windows, test))]
    #[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
    struct FootprintElapsedSecs(f64);

    #[cfg(any(windows, test))]
    impl FootprintElapsedSecs {
        fn from_footprint(f: &sister_capture::footprint::Footprint) -> Self {
            Self(f.elapsed_secs())
        }
    }

    /// 這一場量到的所有數字。相同 primitive、不同意思的欄位各自包成 newtype，
    /// 對調會是型別錯誤；同型別 getter 接錯則由
    /// `measure_wires_stats_timings_and_scalars_to_their_fields` 釘住。
    #[cfg(any(windows, test))]
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct FootprintMeasured {
        /// 最後一次抓到的畫面大小。`None` = 這場一張都沒抓到（不是 0×0）。
        frame_size: Option<(u32, u32)>,
        /// 這段期間的平均 CPU。`None` = 量不到或還不夠久。
        cpu_percent: Option<CpuPercent>,
        /// 期間內看過的最大 RSS。`None` = 量不到。
        peak_rss_bytes: Option<u64>,
        disk: DiskMeasured,
    }

    /// 磁碟那三個數字。全部是**這段期間的實測位元組**，不是速率——
    /// 換算成「一天」是 `footprint_lines` 的 `per_day` 參數的事。
    #[cfg(any(windows, test))]
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct DiskMeasured {
        /// 這段期間磁碟淨長了多少。**可能是負的**：保留期清理跑過
        /// （`PRUNE_EVERY` 六小時一次），或另一支 sister 也在寫同一顆資料庫。
        /// `None` = 快照或相減量不到；不能拿 `0` 冒充量到了而且沒長。原因由
        /// 下方磁碟歸因逐項說明，這裡不猜。
        delta_bytes: Option<i64>,
        /// 這段期間寫進去的圖。
        image_bytes: ImageBytesWritten,
        /// 一天的圖額度（`capture.max_image_mb_per_day` × MB）。`0` = 不設限。
        image_cap_bytes: ImageBudgetBytes,
        /// 這一場期間，每日圖上限是否至少關過一次門。來自
        /// `RecorderStats::images_over_budget > 0`，**不等於今天已經用完**：這個
        /// 計數器會跨日累加。這裡只拿它收起已被事實推翻的未來式關門預測；要分辨
        /// 關門的是哪一天，是十行前 `report_images` 的工作。
        image_budget_closed_this_session: bool,
    }

    /// 把量到的數字排成她要印的那幾行。回傳整塊（含開頭那行「條件：」），
    /// 用 `\n` 分行、結尾不留換行。
    ///
    /// `per_day` 是「這段期間的 N 個位元組相當於一天幾個位元組」的換算，
    /// **由呼叫端傳進來**：`Footprint::bytes_per_day` 的分母是
    /// `Instant::now()` 起算的牆上時間，測試沒有辦法讓它「已經過了 60 秒」。
    /// 傳函式而不是傳兩個算好的 `Option<f64>`，是因為那兩個算好的值型別一樣、
    /// 位置相鄰，對調的話編得過（上一版就是這樣印出負數的每天用量）；傳一個
    /// 函式進來，「總量的速率」和「圖的速率」就都在這個函式裡算，呼叫端接不錯。
    ///
    /// `per_day` 回 `None` = 這段太短，不外推（見 `Footprint::bytes_per_day`
    /// 的「< 60 秒不回答」）。那時候整個磁碟那一段要**消失**，不可以印 `0`。
    ///
    /// `any(windows, test)` 而不是 `windows`：整個 `report_footprint` 是
    /// Windows 限定的，而它裡面每一個 ⚠ 判斷在開發機上一次都跑不到。
    /// 對抗式稽核在這一段做了 22 次改壞，17 次四道閘門全綠。
    #[cfg(any(windows, test))]
    fn footprint_lines(m: &FootprintMeasured, per_day: impl Fn(u64) -> Option<f64>) -> String {
        let FootprintMeasured {
            frame_size,
            cpu_percent,
            peak_rss_bytes,
            disk:
                DiskMeasured {
                    delta_bytes: disk_delta,
                    image_bytes,
                    image_cap_bytes,
                    image_budget_closed_this_session,
                },
        } = *m;
        /// 超標的就標出來。合格的不標——每一項都掛一個記號等於沒有記號。
        fn mark(over_budget: bool) -> &'static str {
            if over_budget { "⚠ " } else { "" }
        }

        let mut lines = vec![format!(
            "  條件：{}",
            footprint_context(env!("CARGO_PKG_VERSION"), frame_size)
        )];
        let mut parts = Vec::new();
        let mut breached = Vec::new();
        let mut phase_zero_budget_breached = false;
        if let Some(CpuPercent(cpu)) = cpu_percent {
            // CPU 是量測值，不是 Phase 0 判決。不能把 alpha.46 某一種工作負載的
            // 44.0% 偷換成所有工作負載都適用的 45% 門檻。
            parts.push(format!("CPU 平均 {cpu:.1}%"));
        }
        if let Some(rss) = peak_rss_bytes {
            let rss_over = rss > BUDGET_RSS_BYTES;
            parts.push(format!(
                "{}RAM 峰值 {}",
                mark(rss_over),
                crate::fmt::bytes(rss as i64)
            ));
            if rss_over {
                phase_zero_budget_breached = true;
                breached.push(format!(
                    "RAM {} 超過預算 {}",
                    crate::fmt::bytes(rss as i64),
                    crate::fmt::bytes(BUDGET_RSS_BYTES as i64)
                ));
            }
        }
        match disk_delta {
            None => parts.push("磁碟總量量不到（詳見下方磁碟歸因）".to_string()),
            Some(disk_delta) if disk_delta < 0 => {
                // 淨少了：錄到一半觸發保留期清理（`PRUNE_EVERY` 是 6 小時，所以
                // 任何一段長一點的錄製都會遇到），或者另一支 sister 也在寫。
                //
                // 這裡以前是 `disk_delta.max(0)`，於是它印成「磁碟 0 B/天」——
                // 而 `over(0.0, …)` 回傳空字串、`breached` 也不會多一條，所以
                // 那一行看起來是**通過預算**。一個沒量到的數字長得和一個漂亮的
                // 數字一模一樣，就出現在我們正在調查的那個預算上，旁邊還配著
                // 一個 CPU 的 ⚠ 讓它更像是真的過了。
                parts.push(format!(
                    "磁碟 這段量不出來（淨少了 {}——清理和寫入混在一起，減出來的數字沒有意義）",
                    crate::fmt::bytes(-disk_delta)
                ));
            }
            Some(disk_delta) => {
                if let Some(raw_per_day) = per_day(disk_delta as u64) {
                    // 圖與資料庫要分開講。合成一個數字的話，「磁碟 11.4 GB/天」
                    // 沒辦法回答唯一有用的那個問題——該去縮圖，還是該去縮索引。
                    // 實測那次就是這樣：一個很嚇人、但指不出方向的數字。
                    let grew = disk_delta;
                    let image_bytes = image_bytes.bytes();
                    let rest = grew - image_bytes as i64;

                    let proj = DiskProjection::clamp(
                        raw_per_day,
                        per_day(image_bytes).filter(|_| rest >= 0),
                        image_cap_bytes,
                    );
                    let (per_day, img_raw, img_capped) =
                        (proj.per_day, proj.images_raw, proj.images);
                    // 減出負數代表這段期間有東西被刪掉了（錄到一半觸發保留期清理，
                    // 或另一支 sister 在跑）。這時候「畫面 X、其他 -3 MB」是**算術
                    // 上正確、意義上胡說**的一行——寧可承認拆不開。
                    let breakdown = if rest < 0 {
                        format!(
                            "這段實際長了 {}，但同時也有東西被刪掉，拆不開",
                            crate::fmt::bytes(grew)
                        )
                    } else {
                        format!(
                            "這段實際長了 {}：畫面 {}、其他 {}",
                            crate::fmt::bytes(grew),
                            crate::fmt::bytes(image_bytes as i64),
                            crate::fmt::bytes(rest)
                        )
                    };
                    let disk_over = per_day > BUDGET_DISK_PER_DAY;
                    parts.push(format!(
                        "{}磁碟 {}/天（{breakdown}）",
                        mark(disk_over),
                        crate::fmt::bytes(per_day as i64),
                    ));
                    if disk_over {
                        phase_zero_budget_breached = true;
                        breached.push(format!(
                            "磁碟 {}/天 超過預算 {}/天（{:.0} 倍）",
                            crate::fmt::bytes(per_day as i64),
                            crate::fmt::bytes(BUDGET_DISK_PER_DAY as i64),
                            per_day / BUDGET_DISK_PER_DAY
                        ));
                        // 超標的時候要指出**是哪一半**，因為那兩半的下一步完全不同，
                        // 而其中一半有天花板、另一半沒有：
                        //
                        //   畫面有 `max_image_mb_per_day`（預設 250 MB），所以它一天
                        //   最多就是那麼多。任何遠大於它的數字，算術上就不可能是圖。
                        //   資料庫沒有任何節流、沒有預算、也沒有自己的計數器。
                        //
                        // 實測那次是 11.4 GB/天，也就是圖的上限的 46 倍——不看這一行
                        // 的話，唯一提到節流的是「另外 N 張只留了字（間隔未到）」，
                        // 而那句話讀起來像是在講省下來的磁碟。
                        let cap = img_capped.map(|img| {
                    let image_rate = if img_raw != img_capped {
                        format!("{}/天（上限，已夾）", crate::fmt::bytes(img as i64))
                    } else {
                        format!("{}/天", crate::fmt::bytes(img as i64))
                    };
                    if img * 2.0 < per_day {
                        format!(
                            "而且大部分不是圖：畫面 {image_rate}、其他（資料庫、索引、事實）{}/天。\
                             畫面那半有天花板（capture.max_image_mb_per_day），另一半沒有。",
                            crate::fmt::bytes((per_day - img) as i64),
                        )
                    } else {
                        format!(
                            "主要是圖：畫面 {image_rate}。調小 capture.max_image_mb_per_day \
                             或拉長 image_min_interval_ms。",
                        )
                    }
                });
                        if let Some(line) = cap {
                            breached.push(line);
                        }
                    }
                    // 夾到了就要說出來，而且**和有沒有超磁碟預算無關**。
                    //
                    // 被夾住的意思是：照這段速度，每日圖額度會在某個時刻寫滿，然後從
                    // 那一刻起只留字。那是一次會靜靜發生的降級——時間軸後半段點開來沒有畫面
                    // ——而使用者唯一能看到的線索，本來只有一個已經被夾好、看起來很
                    // 乖的 250 MB/天。與其讓那個數字獨自去說謊，不如把「幾分鐘後就滿」
                    // 直接印出來。
                    if let (Some(secs), Some(raw), Some(capped)) =
                        (proj.budget_lasts_secs(), img_raw, img_capped)
                    {
                        let budget_consequence = if image_budget_closed_this_session {
                            "這一場期間圖額度已經關過門；詳情見上面的圖額度摘要——".to_string()
                        } else {
                            format!(
                                "照這個速度，一天的圖額度大約 {} 就寫滿，之後她整天只留字、不留畫面——",
                                crate::fmt::duration_ms((secs * 1000.0) as i64),
                            )
                        };
                        breached.push(format!(
                        "這段寫圖的速度相當於 {}/天，是上限（capture.max_image_mb_per_day）的 {:.0} 倍。\
                         {budget_consequence}\
                         上面那個磁碟數字已經按這道門夾過了，看起來乖是因為門會關，不是因為她寫得少。",
                        crate::fmt::bytes(raw as i64),
                        raw / capped,
                    ));
                    }
                }
            }
        }
        if parts.is_empty() {
            return lines.join("\n");
        }
        lines.push(format!("  足跡：{}", parts.join("、")));
        for line in &breached {
            lines.push(format!("  ⚠  {line}"));
        }
        if phase_zero_budget_breached {
            lines.push(
                "        （Phase 0 的驗收條件見 docs/PHASES.md。\
                 短時間的錄製外推一整天本來就會偏高，真正算數的是整天的實測）"
                    .to_string(),
            );
        }
        lines.join("\n")
    }

    #[cfg(any(windows, test))]
    impl FootprintMeasured {
        /// 把這一場的量測接成一份 `FootprintMeasured`。
        ///
        /// 這個函式存在的唯一理由是**接線要測得到**。接線留在 Windows 薄殼時，
        /// `cpu_percent()` 換成同型別的 `cpu_seconds_used()` 曾讓四道閘門全綠，
        /// 最後卻把累積秒數印成百分比。現在 Linux 會編到這裡，測試也直接核對
        /// 每一條來源。
        fn measure(
            f: &sister_capture::footprint::Footprint,
            stats: &sister_capture::RecorderStats,
            disk_delta_bytes: Option<i64>,
            image_budget_mb: u64,
        ) -> Self {
            Self {
                frame_size: stats.last_frame_size,
                cpu_percent: f.cpu_percent().map(CpuPercent),
                peak_rss_bytes: f.peak_rss_bytes(),
                disk: DiskMeasured {
                    delta_bytes: disk_delta_bytes,
                    image_bytes: ImageBytesWritten::from_stats(stats),
                    image_cap_bytes: ImageBudgetBytes::from_mb(image_budget_mb),
                    image_budget_closed_this_session: stats.images_over_budget > 0,
                },
            }
        }

        /// 這一份量測要印出來的整塊字（`report_footprint` 印的就是它）。分母用
        /// 凍結時的牆上秒數，收尾的 dbstat／目錄掃描不會改變外推結果。
        fn report(&self, elapsed: FootprintElapsedSecs) -> String {
            footprint_lines(self, |bytes| bytes_per_day_at(elapsed, bytes))
        }
    }

    /// 用已凍結的足跡時間換算每日增長。60 秒以前不外推；`ElapsedSecs` 是
    /// newtype，呼叫端不能把 CPU 秒數或別的 `f64` 對調進來。
    #[cfg(any(windows, test))]
    fn bytes_per_day_at(elapsed: FootprintElapsedSecs, bytes: u64) -> Option<f64> {
        (elapsed.0 >= 60.0).then(|| bytes as f64 / elapsed.0 * 86_400.0)
    }

    /// Windows 這一層只負責印已經排好的字；快照、來源接線與每日換算都在
    /// `any(windows, test)` 的純邏輯裡，Linux 測試能真的執行。
    #[cfg(windows)]
    fn report_footprint(report: &str) {
        println!("{report}");
    }

    /// 畫面檔寫了幾張、跳過幾張。
    ///
    /// `images_throttled` 一定要講出來，因為使用者遲早會點到一筆沒有圖的
    /// 搜尋結果，然後合理地以為壞掉了。講出來它是設計，不講它就是 bug。
    #[cfg(windows)]
    fn report_images(
        stats: &sister_capture::RecorderStats,
        timings: &sister_capture::timings::Timings,
        budget_mb: u64,
        store_images: StoringImages,
    ) {
        let written = timings.store.calls;

        // 「保留了 12 張畫面、磁碟上一張圖都沒有」原本會讓這一整段消失，
        // 因為每個計數剛好都是 0。那正好是最需要說話的時候：資料夾沒權限、
        // 磁碟滿了、路徑被佔用，症狀全都長這樣。
        if written == 0 && stats.kept > 0 && store_images.enabled() {
            println!(
                "  ⚠  畫面：保留了 {} 張，但一張圖都沒有寫成{}",
                stats.kept,
                if stats.image_failures > 0 {
                    format!("（失敗 {} 次）", stats.image_failures)
                } else {
                    String::new()
                }
            );
            if let Some(e) = &stats.last_image_error {
                println!("        最後一次的原因：{e}");
            }
            return;
        }
        if written == 0
            && stats.images_throttled == 0
            && stats.images_over_budget == 0
            && stats.image_failures == 0
        {
            return;
        }
        let mut line = format!(
            "  畫面：寫了 {written} 張（{}",
            crate::fmt::bytes(stats.image_bytes as i64)
        );
        if let Some(each) = timings.store.per_call().filter(|_| written > 0) {
            // 每張多大，是「該不該換編碼格式」唯一問得出答案的數字。
            line += &format!(
                "，平均一張 {}、{:.0} ms",
                crate::fmt::bytes((stats.image_bytes / written) as i64),
                each.as_secs_f64() * 1000.0
            );
        }
        line.push('）');
        if stats.images_throttled > 0 {
            line += &format!("，另外 {} 張只留了字（間隔未到）", stats.images_throttled);
        }
        println!("{line}");

        // **寫成功過就不講失敗，是把「後來壞掉了」整段藏起來。** 上面那個
        // `written == 0` 的分支只涵蓋「從頭到尾一張都沒寫成」；磁碟寫滿了、
        // 防毒或 OneDrive 中途鎖住 `frames/` 的那一種，是先成功 500 張再連續
        // 失敗 5000 次，而 `written > 0` 讓那 5000 次連同 `last_image_error`
        // 一起被丟掉。`report_ocr` 的失敗是無條件印的，這裡沒有理由不一樣。
        if stats.image_failures > 0 {
            println!(
                "  ⚠  另外 {} 次想存圖卻失敗了——那幾個時刻只剩下字。",
                stats.image_failures
            );
            if let Some(e) = &stats.last_image_error {
                println!("        最後一次的原因：{e}");
            }
        }

        // 這一句要單獨佔一行、而且要講得像一件事，不是像一個統計欄位。
        if stats.images_over_budget > 0 {
            println!(
                "  ⚠  {}的畫面額度（{budget_mb} MB）用完了，{}{} 張只留了字。\
                 文字與搜尋不受影響；要留更多圖就調大 capture.max_image_mb_per_day",
                // 「今天」在一場跨了五天的 session 上是假的，而開著不關正是
                // 這個產品的預設用法。見 `RecorderStats::images_over_budget_days`。
                if stats.images_over_budget_days > 1 {
                    format!("這 {} 天每天", stats.images_over_budget_days)
                } else {
                    "今天".to_string()
                },
                if stats.images_over_budget_days > 1 {
                    "一共 "
                } else {
                    "之後的 "
                },
                stats.images_over_budget
            );
        }
    }

    /// CPU 花到哪一段去了。
    ///
    /// 存在的理由：足跡那行說得出「CPU 平均 27.1%」，但那是一個**沒有下一步**
    /// 的數字。我為了它猜過兩次原因，兩次都猜 PNG 編碼，兩次都猜錯——實測
    /// PNG 編一張只要 1.7ms。一個超標九倍的預算配上一份說不出錢花到哪裡的
    /// 報告，只會讓人去改那個最好改的地方，而不是那個最貴的地方。
    ///
    /// **這張表拆的是牆上時間，不是 CPU。** 兩者差很多：CI 上量到 0.8 秒的
    /// tick 時間只對應 0.27 秒 CPU，三分之二是卡在顯示驅動裡等。所以標題
    /// 那行要把 CPU 秒數一起印出來——不然一份拆得很細的耗時表會被讀成
    /// 「CPU 花在哪裡」，而使用者抱怨的明明是後者。
    #[cfg(windows)]
    fn report_timings(
        t: &sister_capture::timings::Timings,
        counts: TickCounts,
        cpu_secs: Option<f64>,
    ) {
        let ticks = counts.total();
        let working = counts.working();
        let total = t.total();
        if total.is_zero() || ticks == 0 {
            return;
        }
        let cpu = match cpu_secs {
            // 「等」跟「算」差得夠遠才值得多說一句；差不多的時候多印只是雜訊
            Some(c) if c < total.as_secs_f64() * 0.8 => {
                format!("；其中真的燒掉 {c:.1} 秒 CPU，其餘是等（多半在顯示驅動裡）")
            }
            Some(c) => format!("；同期間燒掉 {c:.1} 秒 CPU"),
            None => String::new(),
        };
        // 每拍成本要除以**真的做事的**那些拍。暫停和關閉的空轉拍幾微秒就
        // 回來，把它們算進分母只會讓這個迴圈看起來比實際便宜——一段暫停了
        // 七小時的錄製會印出「每 tick 8 ms」，而真的做事的那一拍要 60 ms，
        // 於是這個數字把調查指向別的地方。
        let idle_ticks = counts.idle();
        let skipped = if idle_ticks > 0 {
            format!("（其中 {idle_ticks} 拍是暫停或關閉，沒做事）")
        } else {
            String::new()
        };
        println!(
            "  時間：{ticks} tick{skipped} 佔了 {:.1} 秒（做事的每拍 {:.0} ms）{cpu}",
            total.as_secs_f64(),
            total.as_secs_f64() * 1000.0 / working.max(1) as f64
        );
        // CJK 是雙寬字元，`{:<6}` 只數字元數會對不齊
        let pad = |name: &str| {
            let cols: usize = name.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
            " ".repeat(7usize.saturating_sub(cols))
        };
        for (name, s) in t.ranked() {
            let per_call = s.per_call().unwrap_or_default();
            println!(
                "        {name}{}{:>6.2} 秒 / {:>3.0}%　{:>5} 次，每次 {:.1} ms",
                pad(name),
                s.total.as_secs_f64(),
                s.total.as_secs_f64() / total.as_secs_f64() * 100.0,
                s.calls,
                per_call.as_secs_f64() * 1000.0
            );
        }
        // 這一行是上面那份排名對自己的誠實度檢查。它小，代表排名真的解釋了
        // CPU 花到哪裡；它大，代表最貴的東西根本沒被量到，而排第一的那一項
        // 只是「被量到的裡面最大的」——那正是一份效能報告最會騙人的形狀。
        if let Some(rest) = t.unattributed() {
            let pct = rest.as_secs_f64() / total.as_secs_f64() * 100.0;
            println!(
                "        其他{}{:>6.2} 秒 / {pct:>3.0}%　{}",
                pad("其他"),
                rest.as_secs_f64(),
                if pct >= 25.0 {
                    "⚠ 這段沒歸因到任何階段，上面的排名解釋不了它"
                } else {
                    "（沒歸因到任何階段）"
                }
            );
        }
    }

    /// 這一段的存在理由：上面那行摘要在「12 張畫面、上面的字一個都沒讀到」
    /// 的時候，看起來和一切正常一模一樣。
    ///
    /// 保留了幾張畫面是**容量**，讀到了幾行字才是**記憶**。只印前者，等於
    /// 讓一個安靜的失敗長期偽裝成成功——那正是這個專案最主要的失敗形狀。
    /// 讀字關掉的時候，括號裡那句話。
    ///
    /// 「畫面留下了」在 text-only 模式下是假的，而那兩個開關**各自獨立**——
    /// `consent::downgrade` 只碰 `store_images`，從來不碰 `ocr`，所以兩個都關
    /// 是走得到的：設定檔關掉 ocr，加上只簽了第一張同意書（也就是
    /// `sister consent --grant local-recording` 那條預設的路）。
    ///
    /// 那是最糟的一次說謊：這一整段是使用者判斷「剛剛那幾個小時到底留下了
    /// 什麼」的唯一地方，而在這個組合下整份摘要**只有這一句**提到畫面
    /// （`report_images` 在一張都沒寫的時候整段不印），偏偏它說有。開機時是
    /// 印過一句「第三張沒簽，只記螢幕上的字」，但那是幾千行 tick 以前的事。
    ///
    /// 拆出來、而且在測試建置下也編得到：`report_ocr` 只在 Windows 上存在，
    /// 而一句只有目標平台才驗得到的話，等於一句沒有被驗過的話。同一個理由
    /// 和同一個 `any(windows, test)` 見 [`DiskProjection`]。
    #[cfg(any(windows, test))]
    fn ocr_off_words(stores_images: StoringImages) -> &'static str {
        if stores_images.enabled() {
            "畫面留下了，但上面的字沒有進資料庫"
        } else {
            "而這一次也沒有留畫面——這段時間等於什麼都沒記下來"
        }
    }

    /// changed-region gate 的原始計數變成一句可以核對的話。
    ///
    /// 像素比例一定拿實際累計相除，不用「幾個 crop」猜：三個小角落和三張
    /// 幾乎全幅的圖，crop 數完全一樣，成本卻不是同一件事。
    #[cfg(any(windows, test))]
    fn ocr_work_line(stats: &sister_capture::RecorderStats) -> Option<String> {
        let successful = stats.ocr_full_frames + stats.ocr_region_frames + stats.ocr_reused_frames;
        if successful == 0
            && stats.ocr_rejected_region_frames == 0
            && stats.ocr_candidate_pixels == 0
        {
            return None;
        }
        let fallback = if stats.ocr_full_fallbacks > 0 {
            format!("，其中 {} 張是局部沒有把握後退回", stats.ocr_full_fallbacks)
        } else {
            String::new()
        };
        let rejected = if stats.ocr_rejected_region_frames > 0 {
            format!(
                "、局部未採用 {} 張／{} 區",
                stats.ocr_rejected_region_frames, stats.ocr_rejected_regions
            )
        } else {
            String::new()
        };
        let pixels = if stats.ocr_candidate_pixels > 0 {
            let retry = if stats.ocr_input_pixels > stats.ocr_candidate_pixels {
                "；失敗後的重試也算，所以可以超過 100%"
            } else {
                ""
            };
            format!(
                "；OCR 嘗試收到 {} / 全幅候選 {} 像素（{:.1}%{retry}）",
                stats.ocr_input_pixels,
                stats.ocr_candidate_pixels,
                stats.ocr_input_pixels as f64 / stats.ocr_candidate_pixels as f64 * 100.0
            )
        } else {
            String::new()
        };
        Some(format!(
            "        OCR 路徑：成功全幅 {} 張{fallback}、成功局部 {} 張／{} 區、沿用 {} 張{rejected}{pixels}",
            stats.ocr_full_frames,
            stats.ocr_region_frames,
            stats.ocr_regions,
            stats.ocr_reused_frames,
        ))
    }

    #[cfg(windows)]
    fn report_ocr(
        stats: &sister_capture::RecorderStats,
        ocr_enabled: OcrEnabled,
        stores_images: StoringImages,
    ) {
        if !ocr_enabled.enabled() {
            println!("  讀字：已關閉（{}）", ocr_off_words(stores_images));
            return;
        }
        println!(
            "  讀字：資料庫留下 {} 行{}",
            stats.ocr_blocks,
            if stats.ocr_failures > 0 {
                format!("，{} 次失敗", stats.ocr_failures)
            } else {
                String::new()
            }
        );
        if let Some(line) = ocr_work_line(stats) {
            println!("{line}");
        }
        if let Some(e) = &stats.last_ocr_error {
            println!("        最後一次的錯誤：{e}");
        }
        if stats.ocr_blocks == 0 && stats.kept > 0 {
            println!(
                "  ⚠  保留了 {} 張畫面，但資料庫沒有留下任何螢幕文字——這些畫面搜不到。\
                 跑 `sister doctor` 看是引擎讀不出字，還是讀不到你這台螢幕。",
                stats.kept
            );
        }
    }
    #[cfg(test)]
    mod record_tests {
        use super::record_meanings::{ImageBytesWritten, OcrEnabled};
        use super::{
            BootBeat, ConfigWatch, CpuPercent, DiskMeasured, DiskProjection, FootprintElapsedSecs,
            FootprintMeasured, ImageBudgetBytes, StoringImages, TickCounts, WantsImages,
            already_recording, bytes_per_day_at, footprint_context, footprint_lines, ocr_off_words,
            ocr_work_line, should_ping_brain,
        };
        use crate::ops::tmp::Tmp;
        use sister_core::config::Config;
        use sister_core::consent::{Consent, Sheet};
        use sister_core::heartbeat;

        const MB: f64 = 1024.0 * 1024.0;
        const MB_I: i64 = 1024 * 1024;
        const MB_U: u64 = 1024 * 1024;
        const CAP_250MB: u64 = 250 * 1024 * 1024;

        fn written_bytes(bytes: u64) -> ImageBytesWritten {
            ImageBytesWritten::from_raw_bytes(bytes)
        }

        fn budget_bytes(bytes: u64) -> ImageBudgetBytes {
            ImageBudgetBytes::from_raw_bytes(bytes)
        }

        #[test]
        fn brain_ping_follows_information_value_not_every_tick() {
            use sister_capture::Tick;
            let mut idle = false;
            assert!(!should_ping_brain(&Tick::Idle, &mut idle), "還在閒置不該叫");
            assert!(idle);
            assert!(
                should_ping_brain(&Tick::Duplicate { run: 1 }, &mut idle),
                "長停留後恢復要叫"
            );
            assert!(!idle);
            assert!(
                !should_ping_brain(&Tick::Duplicate { run: 2 }, &mut idle),
                "同一畫面的重複不是新資訊"
            );
            assert!(should_ping_brain(
                &Tick::Kept {
                    frame_id: 1,
                    ocr_blocks: 0,
                    facts: 0,
                },
                &mut idle
            ));
            assert!(should_ping_brain(&Tick::NoScreen, &mut idle));
            assert!(!should_ping_brain(&Tick::Paused, &mut idle));
            assert!(!should_ping_brain(&Tick::Disabled, &mut idle));
        }

        #[test]
        fn tick_counts_keep_total_and_working_stats_in_their_own_fields() {
            let stats = sister_capture::RecorderStats {
                ticks: 100,
                working_ticks: 7,
                ..Default::default()
            };
            let counts = TickCounts::from_stats(&stats);
            assert_eq!(counts.total(), 100);
            assert_eq!(counts.working(), 7);
        }

        #[test]
        fn idle_tick_count_saturates_when_broken_stats_claim_more_work_than_ticks() {
            let stats = sister_capture::RecorderStats {
                ticks: 100,
                working_ticks: 7,
                ..Default::default()
            };
            assert_eq!(TickCounts::from_stats(&stats).idle(), 93);

            let broken = sister_capture::RecorderStats {
                ticks: 7,
                working_ticks: 100,
                ..Default::default()
            };
            assert_eq!(TickCounts::from_stats(&broken).idle(), 0);
        }

        /// OCR 其實開著時不能因為留圖或擷取關閉，就騙使用者說讀字已關閉。
        #[test]
        fn the_summary_does_not_lie_about_whether_reading_words_is_enabled() {
            let mut config = Config::default();
            config.capture.ocr = true;
            config.capture.store_images = false;
            config.capture.enabled = false;
            assert!(OcrEnabled::from_config(&config).enabled());

            config.capture.ocr = false;
            config.capture.store_images = true;
            config.capture.enabled = true;
            assert!(!OcrEnabled::from_config(&config).enabled());
        }

        #[test]
        fn the_ocr_summary_distinguishes_full_regions_and_reuse_by_measured_pixels() {
            let stats = sister_capture::RecorderStats {
                ocr_full_frames: 1,
                ocr_full_fallbacks: 1,
                ocr_region_frames: 2,
                ocr_regions: 3,
                ocr_reused_frames: 4,
                ocr_rejected_region_frames: 5,
                ocr_rejected_regions: 6,
                ocr_candidate_pixels: 10_000,
                ocr_input_pixels: 2_500,
                ..Default::default()
            };
            let line = ocr_work_line(&stats).expect("有量到 gate");
            assert!(line.contains("成功全幅 1 張"), "{line}");
            assert!(line.contains("成功局部 2 張／3 區"), "{line}");
            assert!(line.contains("沿用 4 張"), "{line}");
            assert!(line.contains("局部未採用 5 張／6 區"), "{line}");
            assert!(line.contains("2500 / 全幅候選 10000"), "{line}");
            assert!(line.contains("25.0%"), "{line}");
            assert!(line.contains("退回"), "{line}");
            assert!(!line.contains("可以超過 100%"), "{line}");

            let retried = sister_capture::RecorderStats {
                ocr_candidate_pixels: 10_000,
                ocr_input_pixels: 12_500,
                ..Default::default()
            };
            let retried = ocr_work_line(&retried).expect("失敗後有重試");
            assert!(retried.contains("125.0%"), "{retried}");
            assert!(retried.contains("重試也算"), "{retried}");
            assert!(ocr_work_line(&Default::default()).is_none());
        }

        /// 收尾摘要不能把留下幾張圖當成寫了幾個 bytes。
        #[test]
        fn the_summary_reports_image_bytes_instead_of_an_unrelated_counter() {
            let stats = sister_capture::RecorderStats {
                ticks: 1,
                working_ticks: 2,
                kept: 3,
                duplicates: 4,
                excluded: 5,
                no_screen: 6,
                skipped_idle: 7,
                idle_asked: 8,
                idle_unknown: 9,
                browser_ticks: 10,
                url_reads: 11,
                title_clock_ticks: 12,
                clipboard_events: 13,
                secrets_redacted: 14,
                focus_events: 15,
                image_bytes: 16,
                images_throttled: 17,
                images_over_budget: 18,
                images_over_budget_days: 19,
                image_failures: 20,
                ocr_blocks: 21,
                tick_failures: 22,
                ocr_failures: 23,
                ..Default::default()
            };
            assert_eq!(ImageBytesWritten::from_stats(&stats).bytes(), 16);
        }

        /// 收尾摘要的圖片磁碟預算不能少算 1024 倍，而 0 要保留「不設限」的意思。
        #[test]
        fn the_disk_budget_shown_to_the_user_keeps_megabytes_and_unlimited_zero_honest() {
            assert_eq!(ImageBudgetBytes::from_mb(250).bytes(), 262_144_000);
            assert_eq!(ImageBudgetBytes::from_mb(0).bytes(), 0);
        }

        /// CPU／RAM 沒量到、磁碟則真的量到零的基準。測試用
        /// `..nothing_measured()` 疊上自己要的那幾項。
        /// **具名不是排版偏好**：`image_bytes` 和 `image_cap_bytes` 都是 `u64`，
        /// 做成位置參數的話對調編得過——那正是這一整段程式碼被抽出來的原因，
        /// 測試自己不可以再造一個。
        fn nothing_measured() -> FootprintMeasured {
            FootprintMeasured {
                frame_size: Some((2560, 1440)),
                cpu_percent: None,
                peak_rss_bytes: None,
                disk: DiskMeasured {
                    delta_bytes: Some(0),
                    image_bytes: written_bytes(0),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
            }
        }

        fn ten_minute_day(bytes: u64) -> Option<f64> {
            Some(bytes as f64 / 600.0 * 86_400.0)
        }

        /// 1. RAM／磁碟超標要標出；CPU 永遠照實量，但不再是 Phase 0 門檻。
        #[test]
        fn footprint_marks_every_breach_and_no_passing_budget() {
            let over = FootprintMeasured {
                cpu_percent: Some(CpuPercent(46.0)),
                peak_rss_bytes: Some(401 * MB_U),
                disk: DiskMeasured {
                    delta_bytes: Some(3584 * MB_I),
                    image_bytes: written_bytes(200 * MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&over, |bytes| Some(bytes as f64));
            assert!(out.contains("CPU 平均 46.0%"), "CPU 仍要照實顯示：{out}");
            assert!(
                !out.contains("⚠ CPU") && !out.contains("CPU 46.0% 超過預算"),
                "CPU 不得被偷換成另一條 Phase 0 門檻：{out}"
            );
            for warning in ["⚠ RAM 峰值 401.0 MB", "⚠ 磁碟 3.5 GB/天"] {
                assert!(out.contains(warning), "兩種超標都必須直接帶 ⚠：{out}");
            }
            for warning in [
                "RAM 401.0 MB 超過預算 400.0 MB",
                "磁碟 3.5 GB/天 超過預算 300.0 MB/天（12 倍）",
            ] {
                assert!(out.contains(warning), "超標原因必須整句說清楚：{out}");
            }
            assert!(
                out.contains("docs/PHASES.md"),
                "有超標時必須附 Phase 0 註腳：{out}"
            );

            let passing = FootprintMeasured {
                cpu_percent: Some(CpuPercent(44.0)),
                peak_rss_bytes: Some(399 * MB_U),
                disk: DiskMeasured {
                    delta_bytes: Some(MB_I),
                    image_bytes: written_bytes(0),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&passing, ten_minute_day);
            assert!(!out.contains('⚠'), "全部合格時不該有任何警告：{out}");
            assert!(
                !out.contains("docs/PHASES.md"),
                "全部合格時不該附超標註腳：{out}"
            );
        }

        /// 2. 圖比淨成長多代表拆不開；圖速率不能拿去把總量扣成負數。
        #[test]
        fn deleted_bytes_do_not_create_a_negative_daily_projection() {
            let m = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(4 * MB_I),
                    image_bytes: written_bytes(40 * MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&m, ten_minute_day);
            assert!(out.contains("磁碟 576.0 MB/天"), "總量照實外推：{out}");
            assert!(!out.contains("磁碟 -"), "磁碟速率不可以是負的：{out}");
            assert!(
                !out.contains("-5173673984 B"),
                "負的位元組數不可以復活：{out}"
            );
        }

        /// 3、5. 十分鐘爆量必須夾在圖額度上，並明講關門的倍數與時間；
        /// 沒撞門的反例則兩件事都不該發生。
        #[test]
        fn projection_clamps_its_ceiling_and_explains_when_it_will_close() {
            let burst = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(81 * MB_I),
                    image_bytes: written_bytes(79 * MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&burst, ten_minute_day);
            assert!(
                out.contains("538.0 MB/天"),
                "圖額度夾過後才是可能發生的總量：{out}"
            );
            assert!(
                !out.contains("11.4 GB/天"),
                "不可以外推穿過自己的天花板：{out}"
            );
            assert!(
                out.contains("這段寫圖的速度") && out.contains("46 倍") && out.contains("31 分鐘"),
                "撞門要講出倍數與大約何時寫滿：{out}"
            );

            let quiet = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(2 * MB_I),
                    image_bytes: written_bytes(MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&quiet, ten_minute_day);
            assert!(
                !out.contains("這段寫圖的速度"),
                "沒撞門不該聲稱額度會寫滿：{out}"
            );
            assert!(
                !out.contains("538.0 MB/天"),
                "反例不該被夾成爆量場景的數字：{out}"
            );
        }

        /// 4. 超標時必須按圖所佔比例指出真正該處理的那一半。
        #[test]
        fn disk_advice_distinguishes_images_from_everything_else() {
            let mostly_other = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(4 * 1024 * MB_I),
                    image_bytes: written_bytes(200 * MB_U),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&mostly_other, |bytes| Some(bytes as f64));
            assert!(
                out.contains(
                    "而且大部分不是圖：畫面 200.0 MB/天、其他（資料庫、索引、事實）3.8 GB/天。畫面那半有天花板（capture.max_image_mb_per_day），另一半沒有。"
                ),
                "圖只佔一小半時要指向其他資料：{out}"
            );
            assert!(
                !out.contains("主要是圖"),
                "圖只佔一小半不能反過來歸因：{out}"
            );

            let mostly_images = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(400 * MB_I),
                    image_bytes: written_bytes(350 * MB_U),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&mostly_images, |bytes| Some(bytes as f64));
            assert!(
                out.contains(
                    "主要是圖：畫面 350.0 MB/天。調小 capture.max_image_mb_per_day 或拉長 image_min_interval_ms。"
                ),
                "圖佔絕大部分時要指向圖：{out}"
            );
            assert!(
                !out.contains("而且大部分不是圖"),
                "圖佔絕大部分不能歸因給其他資料：{out}"
            );
        }

        /// 6. 有刪除時承認拆不開；沒有刪除時才列出畫面與其他。
        #[test]
        fn disk_breakdown_only_splits_when_the_subtraction_is_meaningful() {
            let deleted = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(4 * MB_I),
                    image_bytes: written_bytes(40 * MB_U),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&deleted, ten_minute_day);
            assert!(
                out.contains("但同時也有東西被刪掉，拆不開"),
                "負的其餘空間必須承認拆不開：{out}"
            );
            assert!(!out.contains("：畫面"), "拆不開時不能印假的分類：{out}");

            let clean = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(40 * MB_I),
                    image_bytes: written_bytes(4 * MB_U),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&clean, ten_minute_day);
            assert!(
                out.contains("這段實際長了 40.0 MB：畫面 4.0 MB、其他 36.0 MB"),
                "能拆時要把兩半都列出來：{out}"
            );
            assert!(!out.contains("拆不開"), "沒有刪除時不該說拆不開：{out}");
        }

        /// 7. 淨減少要把負號解讀成方向，不能再把負號印進數量。
        #[test]
        fn a_net_decrease_is_reported_as_a_positive_magnitude() {
            let shrank = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(-4 * MB_I),
                    image_bytes: written_bytes(0),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&shrank, |_| None);
            assert!(
                out.contains(
                    "磁碟 這段量不出來（淨少了 4.0 MB——清理和寫入混在一起，減出來的數字沒有意義）"
                ),
                "淨減少的量要用正的大小表示：{out}"
            );
            assert!(!out.contains("/天"), "淨減少的這場不准出現每天用量：{out}");
            assert!(!out.contains("-4194304"), "不該洩漏負的原始位元組數：{out}");
            assert!(!out.contains("淨少了 -"), "方向已由『淨少了』表達：{out}");

            let grew = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(4 * MB_I),
                    image_bytes: written_bytes(0),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&grew, ten_minute_day);
            assert!(
                !out.contains("量不出來"),
                "正成長可以外推時不該走淨減少分支：{out}"
            );
        }

        /// 8. 不到 60 秒時下游整段不印磁碟；同一份數字量得出速率時才印。
        #[test]
        fn missing_daily_rate_hides_the_entire_disk_section() {
            let m = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(4 * MB_I),
                    image_bytes: written_bytes(MB_U),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&m, |_| None);
            assert!(!out.contains("磁碟"), "不能把沒量到偽裝成磁碟數字：{out}");
            assert!(!out.contains("0 B/天"), "沒量到不是零：{out}");

            let out = footprint_lines(&m, ten_minute_day);
            assert!(
                out.contains("磁碟"),
                "同樣數字有速率後就要出現磁碟段：{out}"
            );
        }

        /// 9. 真的零要照實顯示；快照失敗則必須是另一句話。
        #[test]
        fn a_measured_zero_stays_a_passing_zero() {
            let m = nothing_measured();
            let out = footprint_lines(&m, |_| Some(0.0));
            assert!(
                out.contains("磁碟 0 B/天（這段實際長了 0 B：畫面 0 B、其他 0 B）"),
                "真的零目前就是這樣顯示：{out}"
            );
            assert!(!out.contains('⚠'), "真的零目前視為通過預算：{out}");

            let missing = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: None,
                    ..m.disk
                },
                ..m
            };
            let missing = footprint_lines(&missing, |_| Some(0.0));
            assert!(
                missing.contains("磁碟總量量不到（詳見下方磁碟歸因）"),
                "量不到要明講：{missing}"
            );
            assert!(
                !missing.contains("磁碟 0 B/天"),
                "沒量到不能冒充真的零：{missing}"
            );
        }

        /// 10. 所有量測都缺席時，只留下不可省略的條件行。
        #[test]
        fn an_empty_footprint_has_only_its_context_line() {
            let m = FootprintMeasured {
                frame_size: None,
                ..nothing_measured()
            };
            let out = footprint_lines(&m, |_| None);
            assert!(out.starts_with("  條件："), "第一行必須是條件：{out}");
            assert_eq!(out.lines().count(), 1, "只能有條件這一行：{out}");
            assert!(!out.contains("足跡："), "空集合不印足跡：{out}");
            assert!(!out.contains('⚠'), "空集合不印警告：{out}");
        }

        #[test]
        fn footprint_lines_passes_the_measured_frame_size_to_its_context() {
            let measured = footprint_lines(&nothing_measured(), |_| None);
            assert!(
                measured.contains("最後一次抓到的畫面 2560×1440"),
                "量到的解析度必須接進條件行：{measured}"
            );

            let missing = FootprintMeasured {
                frame_size: None,
                ..nothing_measured()
            };
            let missing = footprint_lines(&missing, |_| None);
            assert!(
                missing.contains("螢幕解析度量不到"),
                "缺席要照實說：{missing}"
            );
            assert!(
                !missing.contains("2560"),
                "缺席不能沿用測試底稿的解析度：{missing}"
            );
        }

        /// `stats`、`Footprint` 與兩個純量必須各自接進正確欄位。
        #[test]
        fn measure_wires_stats_footprint_and_scalars_to_their_fields() {
            let stats = sister_capture::RecorderStats {
                last_frame_size: Some((3840, 2160)),
                image_bytes: 7 * MB_U,
                images_over_budget: 3,
                ..Default::default()
            };
            let f = sister_capture::footprint::Footprint::new();
            let m = FootprintMeasured::measure(&f, &stats, Some(-37), 250);
            assert_eq!(m.frame_size, Some((3840, 2160)));
            assert_eq!(
                m.cpu_percent,
                f.cpu_percent().map(CpuPercent),
                "CPU 那一格接的是 Footprint"
            );
            assert_eq!(m.disk.image_bytes.bytes(), 7 * MB_U);
            assert_eq!(m.disk.image_cap_bytes.bytes(), 250 * 1024 * 1024);
            assert!(m.disk.image_budget_closed_this_session);
            assert_eq!(m.disk.delta_bytes, Some(-37));
        }

        /// CPU 欄位必須接平均百分比，不能接成這場累積用掉的 CPU 秒數。
        #[test]
        fn measure_uses_cpu_percent_instead_of_cpu_seconds_used() {
            let f = sister_capture::footprint::Footprint::new();
            let from_percent = f.cpu_percent();
            let from_seconds = f.cpu_seconds_used();
            // 前提：這兩個 getter 現在答得不一樣。相同的話這條測試分不出接錯，
            // 要當場紅掉而不是靜靜地通過。
            assert_ne!(
                from_percent, from_seconds,
                "兩個 getter 必須先有可辨識的輸出，這條接線測試才算數"
            );
            let stats = sister_capture::RecorderStats::default();
            let m = FootprintMeasured::measure(&f, &stats, Some(0), 0);
            assert!(!m.disk.image_budget_closed_this_session);
            assert_eq!(
                m.cpu_percent,
                from_percent.map(CpuPercent),
                "CPU 欄位必須原樣接 cpu_percent()"
            );
            assert_eq!(
                m.peak_rss_bytes,
                f.peak_rss_bytes(),
                "RSS 欄位必須原樣接 peak_rss_bytes()"
            );
        }

        /// `report()` 要交出整塊足跡字；未滿 60 秒時磁碟段必須整段消失。
        #[test]
        fn report_returns_the_whole_block_and_hides_short_disk_projection() {
            let f = sister_capture::footprint::Footprint::new();
            let m = FootprintMeasured {
                frame_size: None,
                disk: DiskMeasured {
                    delta_bytes: Some(MB_I),
                    image_bytes: written_bytes(0),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = m.report(FootprintElapsedSecs::from_footprint(&f));
            assert_eq!(
                out,
                format!(
                    "  條件：版本 sister {}；螢幕解析度量不到（這場沒有成功抓到畫面）；負載：程式量不到，貼回時請註明當時在做什麼",
                    env!("CARGO_PKG_VERSION")
                )
            );
        }

        /// 每日外推的 60 秒門檻與分母都用凍結值，不會被收尾診斷拖長。
        #[test]
        fn frozen_elapsed_time_controls_the_daily_projection() {
            assert_eq!(bytes_per_day_at(FootprintElapsedSecs(59.999), 60), None);
            assert_eq!(
                bytes_per_day_at(FootprintElapsedSecs(60.0), 60),
                Some(86_400.0)
            );
            assert_eq!(
                bytes_per_day_at(FootprintElapsedSecs(120.0), 60),
                Some(43_200.0)
            );
        }

        /// 整塊黃金輸出釘住行序、分隔、警告前綴與註腳縮排。
        #[test]
        fn remaining_budgets_breached_match_the_whole_golden_block() {
            let m = FootprintMeasured {
                cpu_percent: Some(CpuPercent(46.0)),
                peak_rss_bytes: Some(401 * MB_U),
                disk: DiskMeasured {
                    delta_bytes: Some(400 * MB_I),
                    image_bytes: written_bytes(350 * MB_U),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&m, |bytes| Some(bytes as f64));
            assert_eq!(
                out,
                format!(
                    concat!(
                        "  條件：版本 sister {}；最後一次抓到的畫面 2560×1440；負載：程式量不到，貼回時請註明當時在做什麼\n",
                        "  足跡：CPU 平均 46.0%、⚠ RAM 峰值 401.0 MB、⚠ 磁碟 400.0 MB/天（這段實際長了 400.0 MB：畫面 350.0 MB、其他 50.0 MB）\n",
                        "  ⚠  RAM 401.0 MB 超過預算 400.0 MB\n",
                        "  ⚠  磁碟 400.0 MB/天 超過預算 300.0 MB/天（1 倍）\n",
                        "  ⚠  主要是圖：畫面 350.0 MB/天。調小 capture.max_image_mb_per_day 或拉長 image_min_interval_ms。\n",
                        "        （Phase 0 的驗收條件見 docs/PHASES.md。短時間的錄製外推一整天本來就會偏高，真正算數的是整天的實測）"
                    ),
                    env!("CARGO_PKG_VERSION")
                )
            );
        }

        /// 兩項門檻都合格的整塊黃金輸出不得混入警告、說明或 Phase 0 註腳。
        #[test]
        fn all_budgets_passing_match_the_whole_golden_block() {
            let m = FootprintMeasured {
                cpu_percent: Some(CpuPercent(44.0)),
                peak_rss_bytes: Some(399 * MB_U),
                disk: DiskMeasured {
                    delta_bytes: Some(MB_I),
                    image_bytes: written_bytes(0),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&m, |bytes| Some(bytes as f64));
            assert_eq!(
                out,
                format!(
                    concat!(
                        "  條件：版本 sister {}；最後一次抓到的畫面 2560×1440；負載：程式量不到，貼回時請註明當時在做什麼\n",
                        "  足跡：CPU 平均 44.0%、RAM 峰值 399.0 MB、磁碟 1.0 MB/天（這段實際長了 1.0 MB：畫面 0 B、其他 1.0 MB）"
                    ),
                    env!("CARGO_PKG_VERSION")
                )
            );
        }

        /// RAM／磁碟的 `over()` 與 breach 判斷必須同守嚴格 `>`：等於合格，
        /// 多一格才同時出現摘要警告、說明與 Phase 0 註腳；CPU 沒有這道門。
        #[test]
        fn exact_budget_passes_and_one_step_over_breaches_on_both_surfaces() {
            let exact = FootprintMeasured {
                cpu_percent: Some(CpuPercent(99.0)),
                peak_rss_bytes: Some(400 * MB_U),
                ..nothing_measured()
            };
            let out = footprint_lines(&exact, |bytes| {
                (bytes == 0).then_some(300.0 * 1024.0 * 1024.0)
            });
            assert!(!out.contains('⚠'), "剛好等於兩項預算仍然合格：{out}");
            assert!(!out.contains("docs/PHASES.md"), "合格不附註腳：{out}");
            assert!(out.contains("CPU 平均 99.0%"), "CPU 高低都只照實量：{out}");

            let above = FootprintMeasured {
                cpu_percent: Some(CpuPercent(99.0)),
                peak_rss_bytes: Some(400 * MB_U + 1),
                ..nothing_measured()
            };
            let out = footprint_lines(&above, |bytes| {
                (bytes == 0).then_some(300.0 * 1024.0 * 1024.0 + 1.0)
            });
            for needle in ["⚠ RAM 峰值", "⚠ 磁碟", "docs/PHASES.md"] {
                assert!(
                    out.contains(needle),
                    "多一格就必須越過「{needle}」那道門：{out}"
                );
            }
            assert!(!out.contains("⚠ CPU"), "CPU 不是 Phase 0 門檻：{out}");
            assert!(
                !out.contains("CPU 99.0% 超過預算"),
                "CPU 沒有新造門檻：{out}"
            );
            for needle in ["RAM 400.0 MB 超過預算", "磁碟 300.0 MB/天 超過預算"] {
                assert!(
                    out.contains(needle),
                    "摘要與說明必須一致越界「{needle}」：{out}"
                );
            }
        }

        /// RAM、磁碟各自超標要有 Phase 0 註腳；CPU 量測與圖額度建議則沒有。
        #[test]
        fn each_phase_zero_breach_and_image_ceiling_advice_choose_the_right_footnote() {
            let capped_but_passing = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(2 * 1024 * MB_I),
                    image_bytes: written_bytes(2 * 1024 * MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&capped_but_passing, ten_minute_day);
            assert!(
                out.contains("這段寫圖的速度"),
                "撞圖額度的建議仍要保留：{out}"
            );
            assert!(
                !out.contains("docs/PHASES.md"),
                "磁碟合格不能掛超標註腳：{out}"
            );

            let cpu_only = FootprintMeasured {
                cpu_percent: Some(CpuPercent(99.0)),
                ..nothing_measured()
            };
            let out = footprint_lines(&cpu_only, |_| None);
            assert!(out.contains("CPU 平均 99.0%"), "CPU 仍要列出：{out}");
            assert!(!out.contains('⚠'), "CPU 不再觸發 Phase 0 警告：{out}");
            assert!(!out.contains("docs/PHASES.md"), "CPU 不掛超標註腳：{out}");

            let individual_breaches = [(
                "RAM",
                FootprintMeasured {
                    peak_rss_bytes: Some(400 * MB_U + 1),
                    ..nothing_measured()
                },
            )];
            for (budget, measured) in individual_breaches {
                let out = footprint_lines(&measured, |_| None);
                assert!(
                    out.contains("docs/PHASES.md"),
                    "只有 {budget} 超標也必須附註腳：{out}"
                );
            }

            let disk_breached = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(2 * 1024 * MB_I + 4 * MB_I),
                    image_bytes: written_bytes(2 * 1024 * MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&disk_breached, ten_minute_day);
            assert!(
                out.contains("磁碟 826.0 MB/天 超過預算"),
                "其他資料超標要有說明：{out}"
            );
            assert!(
                out.contains("docs/PHASES.md"),
                "磁碟真的超標才附註腳：{out}"
            );
        }

        /// 額度在這一場還沒關過門才預測時間；已關過門就指回上面的摘要。
        #[test]
        fn spent_image_budget_uses_past_tense_instead_of_predicting_another_close() {
            let mut m = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(81 * MB_I),
                    image_bytes: written_bytes(79 * MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&m, ten_minute_day);
            assert!(
                out.contains("一天的圖額度大約 31 分鐘 就寫滿"),
                "未撞門維持原句：{out}"
            );

            m.disk.image_budget_closed_this_session = true;
            let out = footprint_lines(&m, ten_minute_day);
            assert!(
                out.contains("這一場期間圖額度已經關過門；詳情見上面的圖額度摘要"),
                "已撞門要指回上面的摘要：{out}"
            );
            assert!(
                !out.contains("今天"),
                "這個計數器不能判定今天是否撞門：{out}"
            );
            assert!(!out.contains("就寫滿"), "已撞門不能再預測未來關門：{out}");
            assert!(
                out.contains("上面那個磁碟數字已經按這道門夾過了"),
                "仍須解釋磁碟數字：{out}"
            );
        }

        /// 歸因只在圖量嚴格少於總量一半時翻到「大部分不是圖」。
        #[test]
        fn disk_attribution_straddles_the_two_to_one_boundary() {
            let m = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(400 * MB_I),
                    image_bytes: written_bytes(200 * MB_U),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let image = 200.0 * MB;
            let below = footprint_lines(&m, |bytes| {
                Some(if bytes == 200 * MB_U {
                    image
                } else {
                    image * 2.0 - 1.0
                })
            });
            assert!(
                below.contains("主要是圖"),
                "兩倍門檻少一點仍歸給圖：{below}"
            );
            assert!(
                !below.contains("大部分不是圖"),
                "門檻下不能提前翻面：{below}"
            );

            let above = footprint_lines(&m, |bytes| {
                Some(if bytes == 200 * MB_U {
                    image
                } else {
                    image * 2.0 + 1.0
                })
            });
            assert!(
                above.contains("大部分不是圖"),
                "兩倍門檻多一點就歸給其他：{above}"
            );
            assert!(!above.contains("主要是圖"), "門檻上不能留在另一面：{above}");
        }

        /// 被夾的圖速率要在歸因數字旁標明上限；未夾時既有兩句一字不變。
        #[test]
        fn clamped_image_rate_is_labeled_but_uncapped_wording_stays_unchanged() {
            let capped = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(60 * MB_I),
                    image_bytes: written_bytes(40 * MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&capped, ten_minute_day);
            assert!(
                out.contains("畫面 250.0 MB/天（上限，已夾）"),
                "夾過的數字要就地揭露：{out}"
            );

            let capped_primary = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(500 * MB_I),
                    image_bytes: written_bytes(400 * MB_U),
                    image_cap_bytes: budget_bytes(CAP_250MB),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&capped_primary, |bytes| Some(bytes as f64));
            assert!(
                out.contains("主要是圖：畫面 250.0 MB/天（上限，已夾）"),
                "主要是圖那句也要把 clamp 標在數字旁：{out}"
            );

            let uncapped = FootprintMeasured {
                disk: DiskMeasured {
                    delta_bytes: Some(400 * MB_I),
                    image_bytes: written_bytes(350 * MB_U),
                    image_cap_bytes: budget_bytes(0),
                    image_budget_closed_this_session: false,
                },
                ..nothing_measured()
            };
            let out = footprint_lines(&uncapped, |bytes| Some(bytes as f64));
            assert!(out.contains("主要是圖：畫面 350.0 MB/天。調小 capture.max_image_mb_per_day 或拉長 image_min_interval_ms。"), "沒夾過維持原句：{out}");
            assert!(!out.contains("上限，已夾"), "沒夾過不能貼標記：{out}");
        }

        /// 讀字關掉、圖也沒在寫的那一場，摘要裡唯一提到畫面的那句話說有留。
        ///
        /// 兩個開關獨立（`downgrade` 只碰 `store_images`），所以這個組合走得到，
        /// 而且它正好是預設路徑：設定檔關掉 ocr ＋ 只簽第一張同意書。
        #[test]
        fn a_session_that_saved_nothing_does_not_get_to_say_the_pictures_are_kept() {
            let text_only = ocr_off_words(StoringImages::from_raw(false));
            assert!(
                !text_only.contains("畫面留下了"),
                "一張都沒寫，不可以說畫面留下了：{text_only}"
            );
            assert!(
                text_only.contains("沒有留畫面"),
                "要講出真正的處境，不是含糊帶過：{text_only}"
            );
            // 另一半不可以跟著壞掉：圖有在寫的時候，那句話本來就是對的。
            assert!(ocr_off_words(StoringImages::from_raw(true)).contains("畫面留下了"));
        }

        /// 他那次實測的形狀：錄十分鐘，外推出「11.4 GB/天」。
        ///
        /// 那個數字算術上正確、事實上不可能——圖那一半寫到 250 MB 就會撞上
        /// `max_image_mb_per_day` 那道門停下來。不夾的話，報告會用一個永遠
        /// 不會發生的數字宣告 Phase 0 的磁碟預算爆掉 38 倍，然後建議他去調
        /// 一個根本碰不到的上限。
        #[test]
        fn a_ten_minute_burst_must_not_be_extrapolated_through_its_own_ceiling() {
            // 十分鐘寫了 79 MB 的圖 + 2 MB 的資料庫，外推成一天
            let images = 79.0 * MB * 144.0; // 11.4 GB/天
            let rest = 2.0 * MB * 144.0; // 288 MB/天
            let p = DiskProjection::clamp(
                images + rest,
                Some(images),
                ImageBudgetBytes::from_raw_bytes(CAP_250MB),
            );

            assert_eq!(p.images, Some(CAP_250MB as f64), "圖那一半要被門夾住");
            assert!(
                p.per_day < 600.0 * MB,
                "夾過的一天總量應該是 250MB 的圖 + 288MB 的其他，不是 11.4 GB：{}",
                p.per_day
            );
            // 而且被夾掉的只有圖：其他那一半沒有天花板，一個位元組都不准動。
            assert!(
                (p.per_day - (CAP_250MB as f64 + rest)).abs() < 1.0,
                "其他那一半被動到了：{}",
                p.per_day
            );
        }

        /// 夾住這件事本身要說出來，而且要說得出「什麼時候關門」。
        ///
        /// 夾完之後那個數字看起來很乖（250 MB/天，預算內），可是它乖的原因
        /// 是門會關——今天某個時刻之後她只留字、不留畫面。那是一次會靜靜
        /// 發生的降級，只看夾過的數字完全看不出來。
        #[test]
        fn hitting_the_ceiling_can_say_when_the_door_shuts() {
            let images = 11.4 * 1024.0 * MB; // 11.4 GB/天
            let p = DiskProjection::clamp(
                images,
                Some(images),
                ImageBudgetBytes::from_raw_bytes(CAP_250MB),
            );
            let secs = p.budget_lasts_secs().expect("被夾住就要答得出來");
            // 250MB ÷ 11.4GB/天 ≈ 一天的 2.1%，約 30 分鐘
            assert!(
                (25.0 * 60.0..40.0 * 60.0).contains(&secs),
                "半小時上下才對，算出來是 {:.0} 分鐘",
                secs / 60.0
            );
        }

        /// 沒撞到門就不要亂夾，也不要多印一句警告。
        #[test]
        fn a_quiet_day_is_left_exactly_as_measured() {
            let images = 40.0 * MB;
            let p = DiskProjection::clamp(
                images + 10.0 * MB,
                Some(images),
                ImageBudgetBytes::from_raw_bytes(CAP_250MB),
            );
            assert_eq!(p.images, Some(images));
            assert_eq!(p.per_day, images + 10.0 * MB);
            assert_eq!(
                p.budget_lasts_secs(),
                None,
                "沒夾住就沒有「幾點關門」這回事"
            );
        }

        /// `max_image_mb_per_day = 0` 在設定裡的意思是**不設限**，不是「一張都不寫」。
        ///
        /// 把 0 當成上限的話，每一次外推都會被夾成 0，報告會宣布她不佔磁碟。
        #[test]
        fn zero_means_no_ceiling_not_a_ceiling_of_zero() {
            let images = 11.4 * 1024.0 * MB;
            let p =
                DiskProjection::clamp(images, Some(images), ImageBudgetBytes::from_raw_bytes(0));
            assert_eq!(p.images, Some(images), "0 = 不設限");
            assert_eq!(p.per_day, images);
            assert_eq!(p.budget_lasts_secs(), None);
        }

        #[test]
        fn footprint_context_names_the_version_and_measured_frame_size() {
            assert_eq!(
                footprint_context("0.1.0-alpha.43", Some((2560, 1440))),
                "版本 sister 0.1.0-alpha.43；最後一次抓到的畫面 2560×1440；負載：程式量不到，貼回時請註明當時在做什麼"
            );
        }

        #[test]
        fn footprint_context_does_not_turn_a_missing_screen_into_zero() {
            let line = footprint_context("0.1.0-alpha.43", None);
            assert!(line.contains("螢幕解析度量不到（這場沒有成功抓到畫面）"));
            assert!(!line.contains("0×0"));
        }

        /// 拆不開的時候（這段有東西被刪掉）不要假裝拆得開。
        #[test]
        fn a_span_that_also_deleted_things_stays_unsplit() {
            let p = DiskProjection::clamp(
                500.0 * MB,
                None,
                ImageBudgetBytes::from_raw_bytes(CAP_250MB),
            );
            assert_eq!(p.images, None);
            assert_eq!(p.per_day, 500.0 * MB, "拆不開就不准動總量");
            assert_eq!(p.budget_lasts_secs(), None);
        }

        /// 正在開機的 recorder：**佔著**這個資料目錄，但**還沒在錄**。
        ///
        /// 這兩件事以前是同一個述詞。顧「第二個 recorder」的那一版把開機
        /// 算成在錄——開機那一段（migration、能力探測、開場那次 prune）在
        /// 一顆一年份的資料庫上要好幾分鐘，而字母人整段時間顯示「在聽」。
        /// 使用者照著那三個字去做他想被記住的事，之後問「剛剛發生什麼事」
        /// 拿到一片空白。那是 `heartbeat` 模組開頭說的、這個產品唯一不能
        /// 說的那種謊。
        #[test]
        fn a_recorder_that_is_still_opening_the_database_occupies_but_does_not_record() {
            let dir = Tmp::new("boot-beat");
            let now = sister_core::now_ms;
            assert!(!heartbeat::is_occupied(&dir.0, now()), "還沒開始，沒有心跳");
            assert!(!heartbeat::is_recording(&dir.0, now()));

            let boot = BootBeat::start(&dir.0).expect("開機心跳");
            // 立刻，不是等第一個 5 秒間隔——那個間隔正是要補的洞。
            assert!(
                heartbeat::is_occupied(&dir.0, now()),
                "開機的第一瞬間就要擋得住第二個 recorder"
            );
            assert!(
                !heartbeat::is_recording(&dir.0, now()),
                "她一個字都還沒記，指示燈不准說「在聽」"
            );
            assert_eq!(
                heartbeat::phase(&dir.0, now()),
                Some(heartbeat::Phase::Booting)
            );
            drop(boot);
        }

        /// **蓋不上第一拍就不准往下走。**
        ///
        /// `heartbeat::safe_to_kill_spawn` 的整條命是「心跳蓋在 `Db::open` 之
        /// 前」。上一版那一拍是 `let _ =`：寫失敗照樣回來，呼叫端下一行就開資
        /// 料庫，而磁碟上留著的是**上一場**的狀態——那三種（乾淨的墓碑、當掉
        /// 的過期心跳、全新的空目錄）全部放行落刀。所以那條不變式講的是順序，
        /// 不是效果。
        ///
        /// 這一條同時釘住兩件事：寫不進去要回 `Err`，而且**磁碟上不可以留下
        /// 一個看起來像「有人正在開機」的檔案**——那會讓下一個 recorder 以為
        /// 這裡有人佔著。
        #[cfg(unix)]
        #[test]
        fn a_boot_beat_that_cannot_be_stamped_stops_the_recording_instead_of_lying() {
            use std::os::unix::fs::PermissionsExt;

            let dir = Tmp::new("boot-unwritable");
            let shut = |mode| {
                std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(mode))
                    .expect("chmod");
            };
            shut(0o500); // 讀得到、進得去，但寫不進去
            let probe = std::fs::write(dir.0.join("probe"), b"x");
            let started = BootBeat::start(&dir.0);
            let after = heartbeat::presence(&dir.0, sister_core::now_ms());
            shut(0o755);

            assert!(
                probe.is_err(),
                "前提沒成立：這個資料夾還寫得進去（用 root 跑的話這條測試什麼都驗不到）"
            );
            let err = started.err().expect("寫不進心跳就不該回來");
            let said = format!("{err:#}");
            assert!(
                said.contains(&dir.0.display().to_string()),
                "講不出是哪個資料夾寫不進去的話，這句話沒有下一步：{said}"
            );
            assert_eq!(
                after,
                heartbeat::Presence::NeverStarted,
                "第一拍蓋不上就不該在磁碟上留下任何東西"
            );
        }

        /// 主迴圈接手之後，同一個檔案要改口說「在錄」。
        ///
        /// 少了這一條，上面那個修法就會把指示燈**永遠**釘在「沒在錄」——
        /// 一個一樣糟、而且方向相反的謊。
        #[test]
        fn once_the_loop_takes_over_the_same_file_says_she_is_recording() {
            let dir = Tmp::new("boot-took-over");
            let now = sister_core::now_ms;
            let mut boot = BootBeat::start(&dir.0).expect("開機心跳");
            assert!(!heartbeat::is_recording(&dir.0, now()));

            boot.hand_off();
            // 交棒之後蓋的第一下由主迴圈負責（`windows_record` 進迴圈前那一行）。
            heartbeat::beat(&dir.0, now()).expect("beat");
            assert!(
                heartbeat::is_recording(&dir.0, now()),
                "主迴圈在跑了，這時候才可以說在聽"
            );
            assert!(heartbeat::is_occupied(&dir.0, now()), "在錄的當然也佔著");
            drop(boot);
        }

        #[test]
        fn two_terminals_do_not_get_two_recorders_on_one_database() {
            // 字母人那一邊早就擋了，但 `sister record` 自己沒有——所以開兩個
            // 終端機各打一次就成立。兩個行程對同一顆資料庫各錄一份，唯一的
            // 症狀是磁碟用得比講好的快一倍，而使用者會以為是保留期壞了。
            let dir = Tmp::new("two-recorders");
            already_recording(&dir.0).expect("沒有人在錄的時候要放行");
            let _first = BootBeat::start(&dir.0).expect("開機心跳");
            let err = already_recording(&dir.0).expect_err("第二個要被擋下來");
            let said = format!("{err}");
            // 擋下來還要講得出下一步是什麼。一句「不行」會讓人去刪資料庫。
            assert!(said.contains("sister stop"), "要指路：{said}");
            assert!(said.contains("秒"), "要講得出多久以前／等多久：{said}");
        }

        #[test]
        fn the_second_recorder_is_stopped_by_run_itself_not_just_by_the_helper() {
            // 上一條測的是那個判斷；這一條測**它真的被叫到了**。少了這一行，
            // 閘門會是一支寫得很好、卻沒有人呼叫的函式——那是這個專案獵的
            // 另一種 bug：讀起來很對，一輩子命中不了任何東西。
            //
            // 在 Linux 上 `run` 走完同意書之後會抱怨「這個平台還沒有擷取
            // 後端」。所以斷言不是「有沒有錯」，是**錯的是哪一件**。
            let dir = Tmp::new("run-guard");
            let mut c = Consent::default();
            c.grant(Sheet::LocalRecording, 1);
            sister_core::consent::save(&dir.0, &c).expect("save");
            let _first = BootBeat::start(&dir.0).expect("開機心跳");

            let err = super::run(&dir.0, Config::default(), None, None)
                .expect_err("已經有人在錄，第二個不該起得來");
            let said = format!("{err}");
            assert!(
                said.contains("已經有一個 sister record"),
                "擋下來的該是那道閘門，不是平台檢查：{said}"
            );
        }

        #[test]
        fn thinking_blocks_the_second_recorder_without_claiming_she_is_recording() {
            let dir = Tmp::new("already-thinking");
            let now = sister_core::now_ms();
            sister_core::heartbeat::beat_thinking(&dir.0, now, now + 240_000).expect("thinking");
            let err = already_recording(&dir.0).expect_err("想最後一段還佔著");
            let said = format!("{err}");
            assert!(said.contains("想最後一段"), "{said}");
            assert!(
                !said.contains("已經有一個 sister record 在這個資料目錄上跑了"),
                "心跳說沒在錄，兩句會對打：{said}"
            );
        }

        #[test]
        fn a_tombstone_does_not_panic_the_second_recorder() {
            // 上一版 `already_recording` 讀兩次心跳，第二次餵給 `.expect()`。
            // 墓碑那一種 `occupied_why` 回 `None`——只要兩次讀之間有人蓋墓碑，
            // `sister record` 啟動時就 panic。讀一次之後這一種是 Ok，不是炸。
            let dir = Tmp::new("already-tomb");
            sister_core::heartbeat::stop(&dir.0, sister_core::now_ms());
            already_recording(&dir.0).expect("墓碑不是佔著，更不可以 panic");
        }

        #[test]
        fn a_dead_recorder_does_not_lock_the_door_forever() {
            // 敢直接擋，靠的是心跳自己會過期。它不會過期的話，一次當機就等於
            // 從此錄不了——而那個人手上只有一句「已經有一個在跑了」，指著一個
            // 早就不存在的行程。
            let dir = Tmp::new("stale-lock");
            let long_ago = sister_core::now_ms() - sister_core::heartbeat::STALE_AFTER_MS - 1;
            sister_core::heartbeat::beat(&dir.0, long_ago).expect("beat");
            already_recording(&dir.0).expect("過期的心跳不算有人在錄");
        }

        #[test]
        fn a_boot_that_died_does_not_leave_a_heartbeat_behind() {
            // `Db::open` 炸掉之後 `windows_record` 直接 `?` 出去。那時候磁碟上
            // 留著一個剛蓋的心跳，而接下來 16 秒（`STALE_AFTER_MS`）裡字母人會
            // 說她在錄——說她在錄卻沒在錄，是這兩個狀態裡比較危險的那一個。
            let dir = Tmp::new("boot-died");
            drop(BootBeat::start(&dir.0).expect("開機心跳"));
            assert!(
                !heartbeat::is_occupied(&dir.0, sister_core::now_ms()),
                "沒交棒就走了＝這次開機沒成功，心跳要收掉"
            );
        }

        #[test]
        fn handing_off_leaves_the_heartbeat_for_the_loop_that_took_over() {
            // 反過來也要成立：交棒之後這個守衛收工，但心跳是**還在跑的那個
            // 迴圈**的，不能跟著一起被清掉——清掉的話第二個 recorder 就會在
            // 那道閘門上穿過去。
            //
            // 問的是「還佔著嗎」而不是「在錄嗎」：主迴圈還沒蓋自己的第一下，
            // 檔案裡仍然寫著 boot。那個狀態的正確答案就是「佔著、還沒在錄」。
            let dir = Tmp::new("boot-handoff");
            let mut boot = BootBeat::start(&dir.0).expect("開機心跳");
            boot.hand_off();
            drop(boot);
            assert!(
                heartbeat::is_occupied(&dir.0, sister_core::now_ms()),
                "交棒之後心跳歸主迴圈管，守衛不准動它"
            );
        }

        /// 開機那幾分鐘按下去的「停止」，不可以被她自己刪掉。
        ///
        /// 順序照抄 `windows_record`：開機窗打開 → `Db::open`（一顆一年份的
        /// 資料庫要好幾分鐘）→ 交棒 → 主迴圈第一次 `take_stop`。他就是在中間
        /// 那一段按下去的，而 `sister stop` 那時候看得見這個守衛蓋的心跳，所以
        /// 它回的是「已經請她收工」。那句話得是真的。
        #[test]
        fn a_stop_pressed_while_she_opens_the_database_still_stops_her() {
            let dir = Tmp::new("stop-during-boot");
            let mut boot = BootBeat::start(&dir.0).expect("開機心跳");
            sister_core::control::request_stop(&dir.0).expect("request");
            boot.hand_off();
            assert!(
                sister_core::control::take_stop(&dir.0),
                "開機那幾分鐘按的停止，等她開完就要生效"
            );
        }

        /// 反面：**沒有人在跑**的時候留下來的那一個，仍然不可以殺掉下一場。
        ///
        /// 這兩條只差 `request_stop` 站在 `BootBeat::start` 的哪一邊——那個先
        /// 後就是唯一分得開「我起來之前留下的」和「衝著我來的」的東西
        /// （`stop.request` 裡沒有時戳）。少了這一條，把清理整個拿掉也是綠的。
        #[test]
        fn a_stop_left_over_from_before_she_started_does_not_kill_this_run() {
            let dir = Tmp::new("stop-before-boot");
            sister_core::control::request_stop(&dir.0).expect("request");
            let mut boot = BootBeat::start(&dir.0).expect("開機心跳");
            boot.hand_off();
            assert!(
                !sister_core::control::take_stop(&dir.0),
                "起來之前留下的請求要清掉，不然她一開始就自己結束、而畫面上只看到閃一下"
            );
        }

        #[test]
        fn a_file_nobody_touched_does_not_look_touched() {
            // 這條顧的是成本：回報「改了」會讓錄製迴圈重讀並重印一行字。
            // 每 5 秒都誤報一次的話，那行字就變成雜訊，真的改動反而看不見。
            let dir = Tmp::new("quiet");
            let path = dir.file("[capture]\nenabled = true\n");
            let mut w = ConfigWatch::new(Some(&path));
            assert!(!w.changed(&path));
            assert!(!w.changed(&path));
        }

        #[test]
        fn editing_the_blocklist_registers() {
            let dir = Tmp::new("edited");
            let path = dir.file("[privacy]\nexcluded_apps = []\n");
            let mut w = ConfigWatch::new(Some(&path));
            dir.file("[privacy]\nexcluded_apps = [\"keepassxc\", \"1password\"]\n");
            assert!(w.changed(&path), "檔案變長了，一定要看得出來");
            assert!(!w.changed(&path), "看過一次之後就不該再報一次");
        }

        #[test]
        fn deleting_the_config_counts_as_a_change() {
            // 刪掉設定檔 = 回到預設值，那是一個真的狀態變化，不是「沒事發生」。
            let dir = Tmp::new("deleted");
            let path = dir.file("[privacy]\nexcluded_apps = [\"keepassxc\"]\n");
            let mut w = ConfigWatch::new(Some(&path));
            std::fs::remove_file(&path).expect("remove");
            assert!(w.changed(&path));
            assert!(!w.changed(&path), "已經不存在了，不該每次都報");
        }

        #[test]
        fn a_config_that_never_existed_stays_quiet() {
            // 沒有設定檔是正常狀態（用預設值跑）。它不該每 5 秒喊一次。
            let dir = Tmp::new("absent");
            let path = dir.0.join("config.toml");
            let mut w = ConfigWatch::new(Some(&path));
            assert!(!w.changed(&path));

            // 但**新建**一個要看得出來——那正是第一次跑設定頁儲存的那一刻。
            dir.file("[privacy]\nexcluded_apps = []\n");
            assert!(w.changed(&path));
        }

        // ── 同意書那道閘門 ──────────────────────────────────────────
        //
        // 這幾條在 Linux 上跑得到，而 `record` 的後半段整段是 #[cfg(windows)]。
        // 一道只有目標平台才執行得到的隱私閘門，等於一道沒被執行過的閘門。

        /// 沒簽第一張，她連平台檢查都走不到——而且錯誤訊息要講得出下一步。
        #[test]
        fn recording_refuses_to_start_without_the_first_signature() {
            let dir = Tmp::new("gate-none");
            let err = super::gate(&dir.0, Config::default()).expect_err("該被擋下來");
            let msg = err.to_string();
            assert!(
                msg.contains("sister consent --grant local-recording"),
                "擋下來還不夠，要說得出怎麼過去：{msg}"
            );
        }

        /// 簽了第一張、沒簽第三張 = 照錄，但一張圖都不寫。
        ///
        /// 這是三張同意書裡唯一一個「降級而不是拒絕」的路徑（SPEC §11.1 的
        /// 「0 天 = 只留 OCR 文字」）。做成拒絕會讓一個只是不想留截圖的人
        /// 完全用不了她。
        #[test]
        fn without_the_screenshot_sheet_she_records_words_but_keeps_no_pictures() {
            let dir = Tmp::new("gate-words");
            let mut c = Consent::default();
            c.grant(Sheet::LocalRecording, 1);
            sister_core::consent::save(&dir.0, &c).expect("save");

            let mut config = Config::default();
            config.capture.store_images = true;
            let (out, _) = super::gate(&dir.0, config).expect("第一張簽了就該放行");
            assert!(!out.capture.store_images, "第三張沒簽就不可以寫圖");
        }

        /// 撤得回來、也要簽得回去：降級後仍要留著設定檔原本的意思。
        ///
        /// 第三張開機時沒簽，執行用的 Config 必須立刻關掉留圖；但中途簽回
        /// 第三張時，唯一能回答「使用者原本想不想留圖」的是降級前的值。
        /// 兩個答案必須由同一次 `gate` 一起交出來，不能讓呼叫端事後猜。
        #[test]
        fn pictures_can_be_revoked_and_signed_back_without_restarting() {
            let dir = Tmp::new("gate-sign-back");
            let mut consent = Consent::default();
            consent.grant(Sheet::LocalRecording, 1);
            sister_core::consent::save(&dir.0, &consent).expect("save");

            let mut config = Config::default();
            config.capture.store_images = true;
            config.capture.ocr = false;
            config.capture.enabled = false;
            let (downgraded, wants_images) =
                super::gate(&dir.0, config).expect("第一張簽了就該放行");

            assert!(!downgraded.capture.store_images, "第三張沒簽就不可以寫圖");
            assert!(
                wants_images.enabled(),
                "中途簽回第三張時仍要知道設定檔原本要求留圖"
            );

            let dir = Tmp::new("gate-signed-but-disabled");
            let mut consent = Consent::default();
            consent.grant(Sheet::LocalRecording, 1);
            consent.grant(Sheet::FrameStorage, 1);
            sister_core::consent::save(&dir.0, &consent).expect("save");

            let mut config = Config::default();
            config.capture.store_images = false;
            config.capture.ocr = true;
            config.capture.enabled = true;
            let (unchanged, wants_images) =
                super::gate(&dir.0, config).expect("三張都簽了就該放行");

            assert!(
                !unchanged.capture.store_images,
                "同意是上限，不能打開設定關掉的留圖"
            );
            assert!(
                !wants_images.enabled(),
                "三張都簽了也不代表使用者現在想留圖"
            );
        }

        /// 兩張都簽了才留圖。
        ///
        /// 沒有這一條的話，一個「永遠把 store_images 關掉」的閘門也會通過
        /// 上面那條測試——然後截圖功能就安靜地整個消失了。
        #[test]
        fn both_sheets_signed_still_keeps_the_screenshots() {
            let dir = Tmp::new("gate-both");
            let mut c = Consent::default();
            c.grant(Sheet::LocalRecording, 1);
            c.grant(Sheet::FrameStorage, 1);
            sister_core::consent::save(&dir.0, &c).expect("save");

            let mut config = Config::default();
            config.capture.store_images = true;
            let (out, _) = super::gate(&dir.0, config).expect("兩張都簽了");
            assert!(out.capture.store_images);
        }

        /// 條文改版之後，舊的簽名擋不住這道閘門。
        #[test]
        fn a_signature_on_old_wording_does_not_open_the_gate() {
            let dir = Tmp::new("gate-stale");
            let mut c = Consent::default();
            c.grant(Sheet::LocalRecording, 1);
            c.version = sister_core::consent::VERSION + 1;
            sister_core::consent::save(&dir.0, &c).expect("save");

            let err = super::gate(&dir.0, Config::default()).expect_err("該被擋下來");
            assert!(err.to_string().contains("改版"), "{err}");
        }

        // ---------- 錄到一半撤回 ----------
        //
        // PRIVACY.md 寫的是「各自獨立、各自隨時撤得掉」。只在開機時讀一次的
        // 話，那句話真正的意思是「下次重開的時候才撤得掉」——而按下撤回的
        // 那個人，正是最不該被要求等待的那一個。

        use super::{Recheck, recheck};

        fn signed(sheets: &[Sheet]) -> Consent {
            let mut c = Consent::default();
            for s in sheets {
                c.grant(*s, 1);
            }
            c
        }

        #[test]
        fn revoking_the_first_sheet_stops_a_recording_already_in_progress() {
            let c = signed(&[]);
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(true),
                    StoringImages::from_raw(true),
                ),
                Recheck::Stop
            );
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(false),
                    StoringImages::from_raw(false),
                ),
                Recheck::Stop,
                "本來就沒在寫圖也一樣要停：停的是整場，不是截圖那一半"
            );
        }

        #[test]
        fn revoking_the_screenshot_sheet_only_stops_the_pictures() {
            let c = signed(&[Sheet::LocalRecording]);
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(true),
                    StoringImages::from_raw(true),
                ),
                Recheck::Images(false)
            );
        }

        /// 撤得回來、也要簽得回去。
        ///
        /// 這條抓的是一個真的差點做錯的實作：開機時第三張沒簽，`gate` 會把
        /// `store_images` 按成 false；如果中途拿那份被改過的設定去問「他想不想
        /// 留圖」，答案永遠是不想——於是撤回是即時的、簽回來卻要重開，而使用者
        /// 分不出這兩件事有什麼不同。所以問的必須是**設定檔原本的**那個意思。
        #[test]
        fn signing_the_screenshot_sheet_mid_run_starts_the_pictures_again() {
            let c = signed(&[Sheet::LocalRecording, Sheet::FrameStorage]);
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(true),
                    StoringImages::from_raw(false),
                ),
                Recheck::Images(true)
            );
        }

        /// 同意書說可以，但設定檔說不要——那就是不要。
        /// 同意是**上限**，不是開關：簽了不代表使用者現在就想留圖。
        #[test]
        fn consent_never_turns_on_something_the_config_turned_off() {
            let c = signed(&[Sheet::LocalRecording, Sheet::FrameStorage]);
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(false),
                    StoringImages::from_raw(false),
                ),
                Recheck::Same
            );
        }

        /// 錄到一半把設定檔的 `store_images` 關掉，要當場停止寫圖。
        ///
        /// 第一張與第三張仍簽著只代表「可以」；設定檔此刻說不要，就不能讓
        /// recorder 已在寫圖的舊狀態把它重新打開。
        #[test]
        fn turning_off_store_images_mid_run_stops_writing_pictures_immediately() {
            let c = signed(&[Sheet::LocalRecording, Sheet::FrameStorage]);
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(false),
                    StoringImages::from_raw(true),
                ),
                Recheck::Images(false)
            );
        }

        #[test]
        fn nothing_changed_means_nothing_happens() {
            let c = signed(&[Sheet::LocalRecording, Sheet::FrameStorage]);
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(true),
                    StoringImages::from_raw(true),
                ),
                Recheck::Same
            );
            let c = signed(&[Sheet::LocalRecording]);
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(true),
                    StoringImages::from_raw(false),
                ),
                Recheck::Same
            );
        }

        /// 條文改版 = 三張一起失效，所以它和「撤回第一張」走同一條路。
        #[test]
        fn a_wording_change_mid_run_stops_her_too() {
            let mut c = signed(&[Sheet::LocalRecording, Sheet::FrameStorage]);
            c.version = sister_core::consent::VERSION + 1;
            assert_eq!(
                recheck(
                    &c,
                    WantsImages::from_raw(true),
                    StoringImages::from_raw(true),
                ),
                Recheck::Stop
            );
        }
    }
}
