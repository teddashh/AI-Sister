//! 各個子命令的實作。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use sister_core::db::Db;

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

/// 把「排除 80」拆成「是誰擋的」。
///
/// 摘要上那個數字本身沒有錯，但它回答不了使用者唯一會問的問題。而排除
/// 恰恰是這個專案最容易安靜地過度生效的地方——規則寫寬了、UIA 一直答不出
/// 密碼欄狀態、某個 app 名稱剛好是別人的子字串，症狀全都長得一樣：
/// 她什麼都記不住，摘要上只有一個沒有解釋的數字。
///
/// 印出來的理由字串和寫進 `system_events` 的是同一串，所以看到什麼就能
/// 拿什麼去資料庫裡查。
/// 把「她有多少時間是閉著眼睛的」講出來。
///
/// 這個數字不講就是 bug。省電和停止工作在帳面上長得一模一樣：tick 照跑、
/// 沒有錯誤、CPU 很漂亮，而畫面上完全看不出來她其實有 87% 的時間沒看螢幕。
/// 那正是 alpha.4 那種「✓ 但什麼都沒產出」的失效形狀，只是這次是我們自己
/// 刻意造出來的——所以更該說。
fn report_idle(stats: &sister_capture::RecorderStats) {
    if stats.skipped_idle == 0 {
        return;
    }
    let pct = stats.skipped_idle as f64 / stats.ticks.max(1) as f64 * 100.0;
    println!(
        "  省下：{} 次沒碰螢幕（{pct:.0}%——那段時間你沒動鍵盤滑鼠；最多每 5 秒仍會看一次）",
        stats.skipped_idle
    );
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

pub mod consent {
    use super::*;
    use sister_core::consent::{Consent, Sheet};
    use std::str::FromStr;

    /// `sister consent [--grant …] [--revoke …]`。
    ///
    /// 在無頭機器上把整條同意流程走完的入口。字母人上那三張卡片按下去之後
    /// 改的是**同一個檔案**，所以這裡驗得過的東西，那邊也就驗過了。
    pub fn run(data_dir: &Path, grant: &[String], revoke: &[String], json: bool) -> Result<()> {
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
            print_json(data_dir, &c)
        } else {
            print_human(data_dir, &c, changing);
            Ok(())
        }
    }

    fn parse(names: &[String]) -> Result<Vec<Sheet>> {
        names
            .iter()
            .map(|n| Sheet::from_str(n).map_err(anyhow::Error::msg))
            .collect()
    }

    fn print_human(data_dir: &Path, c: &Consent, changed: bool) {
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
                Some(ts) => println!("      {} 同意", crate::fmt::timestamp(ts)),
                None => println!("      {}", sheet.without()),
            }
        }

        println!();
        if c.allows_recording() {
            let frames = if c.allows_frames() {
                "會留截圖"
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

    fn print_json(data_dir: &Path, c: &Consent) -> Result<()> {
        let out = serde_json::json!({
            "path": sister_core::consent::path(data_dir).display().to_string(),
            "version": c.version,
            "current": c.current(),
            "allows_recording": c.allows_recording(),
            "allows_frames": c.allows_frames(),
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

pub mod queries {
    use super::*;

    /// `sister queries`。
    ///
    /// 題庫要看得見，理由和這整支 CLI 一樣：**可稽核**。它是唯一一張存著「他
    /// 自己打進去的字」的表，而 DATA_INVENTORY 的規則是「有什麼就要看得到
    /// 什麼」。一份存了東西卻沒有任何辦法讀出來的紀錄，和偷偷存著沒有差別。
    pub fn run(data_dir: &Path, limit: usize, only_empty: bool, json: bool) -> Result<()> {
        let db = open_existing(data_dir)?;
        let stats = db.query_log_stats()?;
        // 多撈一些再篩：`only_empty` 是在這一層過濾的，直接照 limit 撈的話，
        // 「最近 20 題裡剛好沒有空的」會印出一片空白，而題庫裡其實有。
        let rows = db.query_log(if only_empty { limit * 20 } else { limit })?;
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|r| !only_empty || r.hits == 0)
            .take(limit)
            .collect();

        if json {
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
                "queries": rows.iter().map(|r| serde_json::json!({
                    "id": r.id,
                    "ts": r.ts,
                    "question": r.question,
                    "shape": r.shape,
                    "hits": r.hits,
                    "latency_ms": r.latency_ms,
                    "source": r.source,
                    "clicks": r.clicks,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        if stats.total == 0 {
            println!(
                "題庫是空的。問她幾個問題（`sister query …` 或字母人的搜尋框）就會開始累積。\n\
                 如果你把 `privacy.query_log` 關掉了，那它永遠會是空的——那也是一個合理的選擇。"
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
        println!();
        for r in &rows {
            println!(
                "  {}  {} {}{}{}",
                crate::fmt::timestamp(r.ts),
                crate::fmt::pad(&crate::fmt::one_line(&r.question, 20), 24),
                if r.hits == 0 {
                    "一筆都沒有".to_string()
                } else {
                    format!("{} 筆", r.hits)
                },
                if r.shape == "recent" {
                    "（時間）"
                } else {
                    ""
                },
                match r.clicks {
                    0 => String::new(),
                    n => format!("，點開了 {n} 個出處"),
                }
            );
        }
        if rows.is_empty() && only_empty {
            println!("  最近這些題她都答得出來。");
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
        // 先問有沒有人在。沒有人在的時候仍然照寫——`record` 起來的時候會先清掉
        // 這個檔案，所以留著不會咬人——但要講出來，不然「我按了停止，可是它還
        // 在錄」和「我按了停止，本來就沒有東西在錄」看起來一模一樣。
        let running = sister_core::heartbeat::is_recording(data_dir, sister_core::now_ms());
        sister_core::control::request_stop(data_dir)
            .with_context(|| format!("寫不進 {}", data_dir.display()))?;
        if running {
            println!(
                "■ 已經請她收工。正在跑的 `sister record` 會在下一個 tick 把 session \
                 寫完再結束。"
            );
        } else {
            println!(
                "■ 目前沒有任何 `sister record` 在跑（心跳是停的）。停止的請求還是\
                 留下來了，但下一次開始記錄的時候會先把它清掉，不會影響到那一場。"
            );
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
            let report = db.prune_preview(now, r)?;
            print_report(&report, true);
            return Ok(());
        }
        let report = db.prune(now, r, Some(&frames_dir(data_dir)))?;
        print_report(&report, false);
        Ok(())
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
        // 資料庫說有圖、磁碟上找不到那個檔。以前這幾列會被算成「刪掉了」，
        // 連大小都照著資料庫的欄位加進去——手動清空過 `frames/`、或從一份
        // 沒帶 `--with-frames` 的備份還原之後，報告會說「刪掉了 120 個畫面檔
        // （1.2 GB）」，而磁碟上一個位元組都沒有釋放。
        //
        // 不是 ⚠：東西確實不在了，隱私上沒有缺口。但他拿這個數字對帳，
        // 而「120」和「0」對他的意思完全不一樣。
        if !preview && r.missing > 0 {
            println!(
                "  ? 另外 {} 列說自己有圖，但那個檔已經不在磁碟上了——\
                 \n    可能是有人手動清過 frames/，也可能是從一份沒帶畫面的備份還原的。\
                 \n    （資料庫已經跟著更新，不會再指向它們）",
                r.missing
            );
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
        // 刪不掉的檔案仍然躺在磁碟上，而使用者以為它已經不在了。
        // 這是整份報告裡唯一絕對不能安靜掉的一項。
        for f in &r.failed {
            println!("  ⚠  刪不掉，這個畫面還在磁碟上：{f}");
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
            let (n, bytes) = copy_frames(&src_frames, &Config::frames_dir(to))?;
            println!(
                "{}",
                frames_line(n, bytes, s.frames_with_image as u64, &src_frames)
            );
        } else {
            println!("  ⏸ frames/     沒帶。出處點開來看到的那些畫面留在原地——加 `--with-frames`",);
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
    /// 裡有幾列說自己有圖。
    ///
    /// 「0 個畫面檔」單獨印出來是一句真話，但它回答不了他心裡真正那一題：
    /// **是本來就沒有，還是我剛剛弄丟了？** 這兩件事對備份來說天差地遠，而
    /// 資料庫知道答案。所以三種情況要長得不一樣，尤其少了的那一種——那時候
    /// 前面不該掛一個 ✓。
    fn frames_line(copied: u64, bytes: u64, claimed: u64, src: &Path) -> String {
        if copied < claimed {
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
            let none_to_take = frames_line(0, 0, 0, src);
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
            let gone = frames_line(0, 0, 120, src);
            assert!(gone.starts_with("  ✗"), "少了東西不該掛 ✓：{gone}");
            assert!(gone.contains("120"), "要講資料庫說有幾張：{gone}");
            assert!(gone.contains("少了 120 張"), "要講差多少：{gone}");
            assert!(
                gone.contains("/tmp/sister-x/data/frames"),
                "要講去哪裡比對：{gone}"
            );

            // 少一部分也是少。
            let partial = frames_line(118, 4096, 120, src);
            assert!(partial.starts_with("  ✗"), "{partial}");
            assert!(partial.contains("少了 2 張"), "{partial}");

            // 全部帶到了就安靜地報數。多出來的（有人往 frames\ 丟過別的檔）
            // 不算少，也不值得為它多講一句。
            let all = frames_line(120, 1_048_576, 120, src);
            assert!(all.starts_with("  ✓"), "{all}");
            assert!(all.contains("120 個畫面檔"), "{all}");
            assert!(frames_line(121, 1_048_576, 120, src).starts_with("  ✓"));
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
    use sister_core::Millis;

    /// `30m`／`2h`／`7d` → 毫秒。
    ///
    /// **單位不可以省。** 「`--last 30`」看起來像 30 分鐘，也一樣像 30 天，
    /// 而猜錯的那一邊是一次刪掉一個月的記憶。這個指令沒有回收桶。
    fn parse_span(s: &str) -> Result<Millis> {
        const MIN: Millis = 60_000;
        let s = s.trim();
        let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
        let n: i64 = num
            .parse()
            .ok()
            .filter(|n| *n > 0)
            .with_context(|| format!("看不懂「{s}」——要像 `30m`、`2h`、`7d` 這樣寫"))?;
        let mult = match unit {
            "m" | "min" => MIN,
            "h" | "hr" => 60 * MIN,
            "d" | "day" => 24 * 60 * MIN,
            "" => anyhow::bail!("「{s}」少了單位。`{s}m` 是 {s} 分鐘、`{s}d` 是 {s} 天，差很多"),
            other => anyhow::bail!("看不懂單位「{other}」——只認得 m（分）、h（時）、d（天）"),
        };
        n.checked_mul(mult)
            .with_context(|| format!("「{s}」太長了"))
    }

    pub fn run(data_dir: &Path, last: &str, yes: bool) -> Result<()> {
        let span = parse_span(last)?;
        let now = sister_core::now_ms();
        let (from, to) = (now - span, now);

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

        let report = db.forget_preview(from, to)?;
        if report.is_empty() {
            println!("  那段時間裡她什麼都沒記到，不用忘。");
            return Ok(());
        }

        // 她還在錄的話，這個指令做不到他以為的那件事，而且有兩個理由：
        //
        // 1. 忘掉的右界是「現在」，但下一個 tick 幾毫秒後就到——他最想忘掉的
        //    那一幀，很可能正好落在這一刀後面。
        // 2. 就算刀切得準，那個畫面通常還在螢幕上，下一秒又被記一次。
        //
        // 所以這句話要在**預覽**那一段就講：那才是他還能先去按暫停的時刻。
        // 刪完之後再講就只是一句「你剛剛白做了」。
        let recording = sister_core::heartbeat::is_recording(data_dir, now);

        if !yes {
            prune::print_report(&report, true);
            if recording {
                println!(
                    "\n⚠  **她現在還在錄。** 先 `sister pause` 再刪——不然你最想忘掉的\n   \
                     那一幀可能正好在這一刀後面被寫進去，而且那個畫面多半還在螢幕上，\n   \
                     下一個 tick 就又被記一次。處理完再 `sister resume`。"
                );
            }
            println!(
                "\n這是預覽，一個位元組都沒動。真的要忘掉就再跑一次，加上 `--yes`：\n  \
                 sister forget --last {last} --yes\n\
                 **沒有回收桶，也沒有復原。**"
            );
            return Ok(());
        }

        let report = db.forget(from, to, Some(&prune::frames_dir(data_dir)))?;
        prune::print_report(&report, false);

        if recording {
            println!(
                "\n⚠  她剛才一直在錄，所以這一刀之後寫進去的東西還在——包含你可能\n   \
                 最想忘掉的最後那一幀。先 `sister pause`，再跑一次這個指令。"
            );
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

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
    }
}

pub mod query {
    use super::*;
    use crate::fmt;
    // ★ 那一層住在 core，字母人用的是同一份（見 `sister_core::answer`）。
    use sister_core::answer::answers;

    /// 把「一筆都沒找到」的那幾個查得到的理由講成人話。
    ///
    /// 順序是有意的：她還沒開始記，就沒有第二句好講；記了才輪得到「你自己叫我
    /// 別看那個」和「那段時間我閉著眼」。全部都不成立的時候只剩一句實話——
    /// 她記了，而裡面就是沒有。那句話沒有安慰的成分，但它是真的。
    fn blind_lines(b: &sister_core::answer::BlindSpots) -> Vec<String> {
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
            return vec![if b.frames > 0 {
                format!(
                    "她看過 {} 張畫面，但一個字都沒讀出來——讀字那一段是斷的，跑 `sister doctor` 看是哪一種。",
                    b.frames
                )
            } else if b.sessions > 0 {
                "她錄過，但現在資料庫裡是空的——被 `sister forget` 忘掉了，或是過了保留期。"
                    .to_string()
            } else {
                "她還沒記過任何東西——先跑 `sister record`。".to_string()
            }];
        }
        let mut out = Vec::new();
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
        if b.paused_episodes > 0 {
            // 時間只有在 pause 配得到 resume 的時候才累加，所以「一共 0 秒」
            // 有兩種意思：真的只暫停了一瞬間，或者三天前那次暫停到現在都沒
            // 解除（暫停不會自己過期，那是設計）。後者才是他要找的東西不在
            // 裡面的真正原因，而報成 0 秒剛好把它藏起來。
            //
            // 開頭被保留期刪掉的那幾段也一樣：算進了段數、沒算進時間。
            let how_long = if b.paused_open && b.paused_ms == 0 {
                "而且到現在都還沒解除——她此刻就是閉著眼睛的".to_string()
            } else if b.paused_open {
                format!(
                    "已結束的加起來 {}，最後一段到現在都還沒解除",
                    crate::fmt::duration_ms(b.paused_ms)
                )
            } else if b.paused_truncated > 0 {
                format!(
                    "算得出來的加起來 {}（有 {} 段的開頭已被保留期刪掉，長度算不出來）",
                    crate::fmt::duration_ms(b.paused_ms),
                    b.paused_truncated
                )
            } else {
                format!("一共 {}", crate::fmt::duration_ms(b.paused_ms))
            };
            out.push(format!(
                "她也被暫停過 {} 次、{how_long}，那幾段是空的。",
                b.paused_episodes
            ));
        }
        if out.is_empty() {
            out.push("她記的每一段裡都沒有這個字。".to_string());
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
        let db = open_existing(data_dir)?;

        // 「剛剛發生什麼事」問的是時間，不是字。規則在 core，和字母人共用同一
        // 份——兩邊各判各的，同一句話遲早會在兩個地方得到兩種答案。
        use sister_core::question::Shape;
        let shape = sister_core::question::shape(text);

        // 計時涵蓋兩條路徑：使用者感受到的是整個回答的延遲，不是單一次查詢。
        let started = std::time::Instant::now();
        let (answers, hits) = match shape {
            // L1 的事實是「這個值是什麼」，回答不了「剛剛」。硬跑一次只會
            // 拿電話號碼去回答一個沒有人問號碼的問題。
            Shape::Recent => (Default::default(), db.recent(limit)?),
            // 比對用 `terms`（剝掉頭尾的「剛剛」「那個」），★ 答案用原句：
            // `kinds_for_query` 是在整句話裡找「電話」這種說法，剝字只會少看
            // 到東西，不會多看到。
            Shape::Keywords => (
                answers(&db, text, limit)?,
                db.search(sister_core::question::terms(text), limit)?,
            ),
        };
        let (answers, answers_truncated) = (answers.items, answers.truncated);
        let elapsed = started.elapsed();

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
                "terms": match shape {
                    Shape::Recent => serde_json::Value::Null,
                    Shape::Keywords => sister_core::question::terms(text).into(),
                },
                "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
                // 撈滿上限＝被切掉了。機器讀的那一份更要講：寫腳本的人
                // 看不到終端機上的那個「+」，會直接把長度當成總數。
                "limit": limit,
                // ★ 那一半靠 `Answers::truncated`（撈了 limit+1 筆才知道），
                // 原文那一半還是靠「撈滿上限」判斷。
                "truncated": answers_truncated || hits.len() >= limit,
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
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        // `20 筆原文` 和「一共就這 20 筆」是兩件事，而畫面上長得一模一樣。
        // 撈滿上限就代表**被切掉了**，說出來使用者才知道還有第二頁。
        let more = |n: usize| if n >= limit { "+" } else { "" };

        // 底下這幾筆跟他打的字一個都對不上，所以要先講為什麼。少了這一句，
        // 「剛剛發生什麼事」會得到一串不相干的東西，看起來就是答非所問。
        if shape == Shape::Recent {
            println!(
                "🕘 「{text}」問的是時間，不是字——沒有比對，這是最後看到的 {} 件事，{:.1} ms",
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
                more(hits.len()),
                elapsed.as_secs_f64() * 1000.0,
                if answers_truncated || hits.len() >= limit {
                    format!("（+ 代表撈滿 {limit} 筆就停了，用 --limit 看更多）")
                } else {
                    String::new()
                }
            );
        }

        if !answers.is_empty() {
            println!();
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
            if answers.is_empty() {
                println!("\n沒有找到。");
                // 這句話以前是「她可能當時沒在看，或那段被排除規則擋掉了」——
                // 兩個猜測、零個證據，而兩件事她其實都查得到。
                for line in blind_lines(&sister_core::answer::blind_spots(&db)?) {
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
        Ok(())
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

        /// 同一支號碼出現在三個畫面，是一個答案被看見三次，不是三個答案。
        fn seeded() -> Db {
            let mut db = Db::open_in_memory().unwrap();
            let sid = db.start_session("test", "0").unwrap();
            for (ts, app, text) in [
                (1_000, "chrome.exe", "客服專線 0800-080-123"),
                (2_000, "slack.exe", "打 0800-080-123 就好"),
                (3_000, "chrome.exe", "手機 0912-345-678，帳單 NT$13,450"),
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
            // 那支號碼一年來看過 60 次——超過舊版 10 * 4 = 40 的窗子。
            for i in 0..60 {
                db.insert_frame(
                    sid,
                    &frame(1_000 + i, "chrome.exe", "打 0800-080-123"),
                    None,
                    0,
                )
                .unwrap();
            }
            // 中間某一頁一次吐出 45 支新號碼。舊版的窗子到這裡就滿了。
            for i in 0..45 {
                let text = format!("聯絡 09{:08}", 10_000_000 + i);
                db.insert_frame(sid, &frame(100_000 + i, "chrome.exe", &text), None, 0)
                    .unwrap();
            }
            // 昨天又看到那支號碼一次（第 61 次）。這一筆讓它排進最新的前幾名，
            // 於是它一定會出現在答案裡——舊版也會，只是次數會講成 1。
            db.insert_frame(
                sid,
                &frame(200_000, "chrome.exe", "打 0800-080-123"),
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
    }
}

pub mod facts {
    use super::*;
    use crate::fmt;

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
        let rows = match search {
            Some(s) => db.facts_search(kind, s, limit)?,
            None => match kind {
                Some(k) => db.facts_by_kind(k, limit)?,
                None => db.facts_search(None, "", limit)?,
            },
        };

        if json {
            // 這裡以前直接印一個裸陣列。裸陣列講不出「後面還有」——
            // 拿到 200 筆的腳本只能假設一共就 200 筆。所以跟 query 一樣
            // 包一層信封，把上限和有沒有被切掉講明白。
            let out = serde_json::json!({
                "limit": limit,
                "truncated": rows.len() >= limit,
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
                    (None, None) =>
                        "這份記憶裡還沒有任何事實——她還沒錄過，或錄到的畫面上沒有抄得下來的東西。"
                            .into(),
                }
            );
            return Ok(());
        }
        // 撈滿上限就是被切掉了。`{n} 筆事實` 和「一共就這 n 筆」在畫面上
        // 長得一模一樣，而使用者會拿後者去下結論。
        let more = if rows.len() >= limit {
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
    }
}

pub mod stats {
    use super::*;
    use crate::fmt;
    use sister_core::config::Config;

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
        let img_rate = match s.image_bytes {
            0 => Some(0.0),
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
                    "sessions": s.sessions, "db_bytes": s.db_bytes,
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
                    "signals": db.signal_audit()?.iter().map(|a| serde_json::json!({
                        "name": a.name, "rows": a.rows,
                        "populated": a.populated, "broken": a.broken,
                    })).collect::<Vec<_>>(),
                }))?
            );
            return Ok(());
        }

        println!("📊 AI-Sister 足跡\n");
        if let (Some(a), Some(b)) = (s.first_ts, s.last_ts) {
            println!(
                "  期間      {} → {}  （{:.1} 天）",
                fmt::timestamp(a),
                fmt::timestamp(b),
                span_days
            );
        }
        println!("  工作階段  {}", s.sessions);
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
        println!(
            "  事件      焦點 {} · 剪貼簿 {} · 輸入 {} · 系統 {}",
            s.focus_events, s.clipboard_events, s.input_windows, s.system_events
        );
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
        } else {
            println!(
                "  暫停      {} 段，已結束的加起來 {}",
                pauses.episodes,
                fmt::duration_ms(pauses.total_ms)
            );
            if let Some(since) = pauses.open_since {
                println!(
                    "            最後一段還沒結束（{} 起）——她現在就是閉著眼睛的",
                    fmt::timestamp(since)
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
        if !config.privacy.redact_clipboard_secrets {
            println!("  遮蔽      **關掉了**（privacy.redact_clipboard_secrets = false）——");
            println!("            沒有人在看剪貼簿裡有沒有 API key，所以這一欄數不出東西來。");
            println!("            數不到不等於沒有：複製過的東西原樣進了資料庫。");
        } else if redaction.flagged == 0 {
            println!("  遮蔽      這份紀錄裡沒有任何剪貼簿內容被判定為疑似秘密");
        } else {
            println!(
                "  遮蔽      {} 次剪貼簿內容被判定為疑似秘密",
                redaction.flagged
            );
            if redaction.leaked > 0 {
                println!(
                    "            ⚠ 其中 {} 次內容**仍然留在資料庫裡**——遮蔽沒生效",
                    redaction.leaked
                );
            } else {
                println!("            這幾列的 text 現在都是空的（當場查的，不是相信旗子）");
            }
        }
        println!();
        println!(
            "  資料庫    {}\n  畫面檔    {}",
            fmt::bytes(s.db_bytes),
            fmt::bytes(s.image_bytes)
        );
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
}

pub mod doctor {
    use super::*;
    use crate::fmt;
    use sister_core::config::Config;

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
        if let Err(e) = sister_core::capabilities::write(data_dir, &c.report()) {
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
            ocr_language: c.ocr_language.clone(),
            ocr_available: c.ocr_languages_available.clone(),
            ocr_probes: probes,
            broken_privacy: c.broken_privacy_rules(config),
            degraded: c.silently_degraded(config),
        }
    }

    #[cfg(not(windows))]
    fn caps(data_dir: &Path, config: &Config) -> Caps {
        let _ = (data_dir, config);
        Caps::default()
    }

    pub fn run(data_dir: &Path, config: &Config, config_path: Option<PathBuf>) -> Result<()> {
        println!("🩺 AI-Sister 環境檢查\n");
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
        line(
            consent.allows_frames(),
            "畫面暫存",
            if consent.allows_frames() {
                "已同意——會把截圖寫到硬碟"
            } else {
                "未同意 → 只記螢幕上的字，不留截圖"
            },
        );
        // 第二張的 ✓ 講的是「沒有東西會離開這台機器」，不是「你同意了」。
        // 寫成 ✗ 會讓人以為少裝了什麼；而句子從否定改成肯定，是為了讓那個
        // 勾勾對應到一個真的成立的好狀態。
        line(
            true,
            "上雲解讀",
            if consent.cloud_reading.is_some() {
                "已同意，但這份程式裡沒有任何連外路徑，所以它還沒有作用"
            } else {
                "沒有任何東西會離開這台機器（未同意，而且程式裡本來就沒有連外路徑）"
            },
        );

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
            Some(p) => line(
                true,
                "設定檔",
                &format!(
                    "{}{}",
                    p.display(),
                    if p.exists() {
                        ""
                    } else {
                        "（不存在，用預設值）"
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
                if with_cjk == 0 {
                    mark("?", "兩個字的中文", "資料庫裡還沒有中文，等你錄過再驗一次");
                } else if indexed >= with_cjk {
                    mark(
                        "✓",
                        "兩個字的中文",
                        &format!("{with_cjk} 行中文全都進了索引，多舊的都查得到"),
                    );
                } else {
                    mark(
                        "✗",
                        "兩個字的中文",
                        &format!(
                            "{indexed}/{with_cjk} 行進了索引——回填沒跑完，\
                             沒進去的那些用「帳單」「電話」這種兩個字的詞叫不出來。\
                             三個字以上不受影響，L1 抽出來的事實也不受影響",
                        ),
                    );
                }
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
            // 「0 張畫面 · 0 段文字 ✓」是這個專案一路在修的那個災難本身
            // ——錄了一整天、資料庫在長大、一個字都沒進去。有資料庫卻沒
            // 內容不該是打勾。
            let detail = format!(
                "{} 張畫面 · {} 段文字 · {}",
                s.frames,
                s.chunks,
                fmt::bytes(s.db_bytes + s.image_bytes)
            );
            match (s.frames, s.chunks) {
                (0, 0) => mark("?", "已記錄", &format!("{detail}（還沒有任何內容）")),
                (_, 0) => mark(
                    "✗",
                    "已記錄",
                    &format!("{detail}——有畫面卻一個字都沒有，OCR 沒讀到東西"),
                ),
                _ => line(true, "已記錄", &detail),
            }

            // Phase 0 的退場條件是「連續 7 天自我錄製、零當機」，而在這
            // 之前那句話的驗證方式是使用者自己記得有沒有當過。資料庫一直
            // 知道答案：Ctrl-C 走的是正常收尾，所以沒有 `ended_at` 的那幾
            // 段，剩下的解釋只有被殺、當機、關機、拔電。
            let (all_sessions, unfinished, last_crash) = db.crash_audit()?;
            match (all_sessions, unfinished) {
                (0, _) => mark("?", "零當機", "還沒錄過"),
                (n, 0) => line(true, "零當機", &format!("{n} 段錄製全部正常收尾")),
                (n, u) => mark(
                    // 不畫 ✗。此刻另一個終端機正在錄的話，那一段也沒有
                    // `ended_at`，長得跟當機一模一樣——而我沒有一條不會
                    // 因為 PID 重用而說謊的路可以分辨。不知道就說不知道，
                    // 不要為了讓輸出好看而猜一個。
                    "!",
                    "零當機",
                    &format!(
                        "{n} 段錄製裡有 {u} 段沒有正常收尾{}——當機、關機、拔電，\
                         或者現在正有另一個 sister 在錄",
                        last_crash
                            .map(|t| format!("（最後一次 {}）", fmt::timestamp(t)))
                            .unwrap_or_default()
                    ),
                ),
            }

            // 「她停了」後面永遠跟著同一個問題：什麼時候、為什麼。上面那一列
            // 只數得出「有幾段沒收尾」，答不了「上一段是怎麼結束的」——而這兩
            // 件事的下一步差很多：按了停止什麼都不用做，同意書被撤回的話她從
            // 現在起什麼都不會記。
            if let Some(last) = db.last_session()? {
                let since = fmt::timestamp(last.started_at);
                match (last.ended_at, last.reason.as_deref()) {
                    (Some(t), Some(r)) => line(
                        true,
                        "上一次錄製",
                        &format!(
                            "{since} 開始，{} 結束——{}",
                            fmt::timestamp(t),
                            sister_core::model::EndReason::describe(r)
                        ),
                    ),
                    // 有收尾時間、卻沒有理由。這裡以前一律怪到「那一版還沒有
                    // 在記」頭上，而那句話有一半機率是冤枉的：`sessions` 這張
                    // 表永遠不會被刪，`system_events` 會（保留期和 `sister
                    // forget` 都刪）。上一場比 `text_days` 還舊、或者
                    // `sister forget --last 2d` 剛好蓋過去，理由就會憑空消失。
                    //
                    // 分得出來的證據是「那一場還剩幾筆事件」：一筆都不剩，
                    // 就是整場被清掉了，不是那一版沒寫。
                    (Some(t), None) if last.events_left == 0 => mark(
                        "?",
                        "上一次錄製",
                        &format!(
                            "{since} 開始，{} 結束——理由查不出來了\n\
                             \x20                    （那一場的事件紀錄已經被清掉：\
                             保留期，或 `sister forget` 蓋到那段時間）",
                            fmt::timestamp(t)
                        ),
                    ),
                    // 事件還在、就是沒有 `session_end`：alpha.17 以前寫下的
                    // 紀錄。不是錯誤，只是那個版本答不出來——說出「那時候還沒
                    // 有在記」，比留一個看起來像故障的空白好。
                    (Some(t), None) => mark(
                        "?",
                        "上一次錄製",
                        &format!(
                            "{since} 開始，{} 結束（{} 那一版還沒有在記為什麼停）",
                            fmt::timestamp(t),
                            last.app_version
                        ),
                    ),
                    (None, _) => mark(
                        "?",
                        "上一次錄製",
                        &format!("{since} 開始，沒有收尾——不是當掉，就是它現在還在跑"),
                    ),
                }
            }

            // 上面那一列擋的是「一個字都沒有」。它擋不住的是**有一堆列、
            // 每一列都是空殼**——`COUNT(*)` 對這兩種故障的回答一模一樣。
            //
            // 這三個訊號（焦點、輸入節奏、文字座標）在 Phase 0 沒有任何讀者，
            // 它們是 Phase 1 之後才要用的原料。沒人讀不是問題，**沒人讀所以
            // 沒人驗**才是：真的壞掉的那一天，這裡是唯一會講話的地方。
            for a in db.signal_audit()? {
                if a.rows == 0 {
                    // 沒有列 ≠ 壞掉。可能只是這台機器沒有那個能力（replay
                    // 讀不到 pid），或還沒錄到。不知道就說不知道。
                    mark("?", a.name, "還沒有資料");
                } else if a.broken {
                    mark(
                        "✗",
                        a.name,
                        &format!("{} 列，但沒有一列有內容——{}", a.rows, a.note),
                    );
                } else {
                    line(
                        true,
                        a.name,
                        &format!("{} 列，{} {}", a.rows, a.populated, a.populated_label),
                    );
                }
            }
        }

        println!("\n隱私");
        // 排在最前面，因為它壓過底下每一條：暫停的時候，那些規則生不生效
        // 都無所謂——她根本沒在看。
        //
        // 而且這一行是**唯一**會告訴使用者「你上禮拜按的暫停還開著」的地方。
        // 暫停不會自己過期（見 `sister_core::pause`），所以那條路很真實，而
        // 它的症狀是「所有數字都是 0」——最容易被讀成「程式壞了」。
        if sister_core::pause::is_paused(data_dir) {
            let since = sister_core::pause::paused_since(data_dir)
                .map(|ts| format!("從 {} 起", crate::fmt::timestamp(ts)))
                .unwrap_or_else(|| "不知道從什麼時候開始".to_string());
            mark(
                "⏸",
                "現在有沒有在看",
                &format!("**暫停中**（{since}）。這段期間什麼都不會被記錄"),
            );
        } else if sister_core::heartbeat::is_recording(data_dir, sister_core::now_ms()) {
            line(true, "現在有沒有在看", "有一個 sister record 正在跑");
        } else {
            // 「沒有暫停」以前就印到這裡為止，而那句話讀起來是「她在看」——
            // 和字母人那句「在聽」是同一個謊。沒有暫停**不等於**有人在錄：
            // `sister record` 是另一個行程，沒有人開它的時候旗標一樣是乾淨的。
            //
            // 但這裡不能畫 ✗。doctor 最常見的用法就是「開始之前先檢查一下」，
            // 那時候沒有人在錄是**正常的**——畫成失敗就是一則每次都會出現、
            // 於是很快就被學會忽略的假警報，正是這個檔案上面幾行在講的那種。
            mark(
                "?",
                "現在有沒有在看",
                "沒有暫停，但也沒有任何 sister record 在跑（還沒開始的話，這是正常的）",
            );
        }
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
        //
        // 第一個欄位是「這張表問得到嗎」。少了它，資料庫打不開的時候
        // `unwrap_or_default()` 會給出一個 0，然後這裡掛著 ✓ 說「還沒問過任何
        // 問題」——那是他一整年的題庫，而畫面上寫著沒有。
        match (db.is_some(), config.privacy.query_log, qlog.total > 0) {
            (false, on, _) => mark(
                "?",
                "你問過她什麼",
                &format!("{no_db}（設定是{}）", if on { "要記" } else { "不記" }),
            ),
            (_, true, true) => line(
                true,
                "你問過她什麼",
                &format!(
                    "記著，已經 {} 題（{} 題她答不出來、{}）",
                    qlog.total,
                    qlog.empty,
                    // 出處只有字母人那邊點得動。不講來源的話，「0 題你點開了
                    // 出處」會被讀成「她的答案沒一次有用」——而真相可能只是
                    // 這些題全是從終端機問的。
                    match qlog.clickable {
                        0 => "還沒有從字母人問過".to_string(),
                        n => format!("字母人那邊 {}/{} 題點開了出處", qlog.clicked, n),
                    }
                ),
            ),
            (_, true, false) => line(true, "你問過她什麼", "記著（還沒問過任何問題）"),
            (_, false, true) => mark(
                "⏸",
                "你問過她什麼",
                // 指令要指得到真的存在的東西，而且要連代價一起講。`forget`
                // 刪的是**一段時間**，不是一張表——他想清掉的是題庫，但那段
                // 時間裡的字、事實、畫面會一起走。少講後半句，他會按下去才發現。
                &format!(
                    "**不記了**（privacy.query_log = false）。以前記的 {} 題還在——\
                     `sister forget --last 30d` 帶得走，但那會連同那 30 天的其他記憶一起忘掉",
                    qlog.total
                ),
            ),
            (_, false, false) => mark("⏸", "你問過她什麼", "不記（privacy.query_log = false）"),
        }
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
        line(
            consent.keeps_images(config),
            "保留畫面檔",
            match (config.capture.store_images, consent.allows_frames()) {
                (true, true) => "是",
                (true, false) => "否——設定要留，但第三張同意書沒簽（同意書說了算）",
                (false, _) => "否（text-only 模式）",
            },
        );

        // OCR 語言決定她讀不讀得懂你的螢幕，所以要把**實際挑中的**那個印出來，
        // 不是印設定檔裡的偏好清單——兩者不一致正是問題所在。
        println!("\n讀字");
        if !config.capture.ocr {
            line(false, "OCR", "已關閉（畫面會留下，但上面的字不會進資料庫）");
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
            true,
            "多久看一次螢幕",
            &if used == asked {
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
        if !caps.degraded.is_empty() {
            println!("\n⚠ 看起來正常，但其實記不住東西");
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
        if config.capture.store_images {
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
        match db
            .as_ref()
            .map(|d| d.prune_preview(sister_core::now_ms(), &config.retention))
        {
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
}

pub mod replay {
    use super::*;
    use sister_capture::{Recorder, ReplayBackend, Scenario, Tick};
    use sister_core::config::Config;

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
        report_idle(s);
        report_exclusions(s);
        if s.secrets_redacted > 0 {
            println!("  偵測到 {} 次疑似秘密，內容未落地。", s.secrets_redacted);
        }
        Ok(())
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
        if !sister_core::heartbeat::is_recording(data_dir, now) {
            return Ok(());
        }
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
        );
    }

    /// 同意書那道閘門。過不了就 `Err`，過得了就回一份**可能被降級過**的設定。
    ///
    /// 拆成獨立函式而不是寫在 `run` 裡，是為了讓它在這台 Linux 開發機上跑得到
    /// （`run` 的後半段整段 `#[cfg(windows)]`）。一道只有目標平台才執行得到的
    /// 隱私閘門，等於一道沒有被執行過的閘門。
    fn gate(data_dir: &Path, mut config: Config) -> Result<Config> {
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

        // 第三張沒簽不是「不能錄」，是「只記字不留圖」（SPEC §11.1 的
        // 「0 天 = 只留 OCR 文字」）。降級要講出來——安靜地少存一半東西，
        // 使用者只會以為截圖功能壞了。
        if consent.downgrade(&mut config) {
            println!(
                "  第三張同意書沒簽：這一次只記螢幕上的字，不會寫任何截圖。\n  \
                 （要留圖請跑 `sister consent --grant frame-storage`）"
            );
        }
        Ok(config)
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
    pub fn recheck(consent: &sister_core::consent::Consent, wants: bool, storing: bool) -> Recheck {
        // 第一張先看。它被撤回的時候，第三張是什麼已經不重要了。
        if !consent.allows_recording() {
            return Recheck::Stop;
        }
        let should = wants && consent.allows_frames();
        if should == storing {
            return Recheck::Same;
        }
        Recheck::Images(should)
    }

    pub fn run(
        data_dir: &Path,
        config: Config,
        config_path: Option<PathBuf>,
        duration: Option<u64>,
    ) -> Result<()> {
        // 使用者在設定檔裡自己寫的那個意思，**還沒有被同意書修改過**。
        //
        // 第三張中途簽回來的時候要靠它才知道「他本來就想留圖」。拿 `gate` 改過
        // 的那一份來問，答案永遠是「不要」——因為開機時沒簽，它就已經被按成
        // false 了，於是「撤回得掉、簽回來卻要重開」，而使用者分不出這兩件事
        // 有什麼不同。
        let wants_images_by_config = config.capture.store_images;

        // 同意書擋在平台檢查**前面**。沒有同意就不該錄，這件事和這台機器有
        // 沒有擷取後端無關；而且放後面的話，這道閘門在非 Windows 上永遠碰不到。
        let config = gate(data_dir, config)?;

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
    #[cfg_attr(not(windows), allow(dead_code))]
    struct BootBeat {
        alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        dir: PathBuf,
        handed_off: bool,
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    impl BootBeat {
        fn start(data_dir: &Path) -> Self {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::time::{Duration, Instant};

            let dir = data_dir.to_path_buf();
            // 第一下蓋在呼叫者這條執行緒上，不是丟給新執行緒去蓋：呼叫的人回
            // 去之後下一行就是 `Db::open`，中間不該留一段「心跳還沒出現」的
            // 空窗——那正是要補的洞。
            let _ = sister_core::heartbeat::beat(&dir, sister_core::now_ms());
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
                            let _ = sister_core::heartbeat::beat(&dir, sister_core::now_ms());
                            last = Instant::now();
                        }
                    }
                }
            });
            Self {
                alive,
                thread: Some(thread),
                dir,
                handed_off: false,
            }
        }

        /// 主迴圈接手。之後 drop 不會再把心跳收掉——那是還在跑的 recorder 的
        /// 心跳，不是這個守衛的。
        fn hand_off(&mut self) {
            self.handed_off = true;
            self.stop_thread();
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
                // 沒交棒就走了（`Db::open` 炸了之類）：這次開機沒成功，把心跳
                // 收掉，不然接下來 16 秒字母人會說她在錄。
                sister_core::heartbeat::stop(&self.dir);
            }
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
        wants_images_by_config: bool,
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
        let mut boot = BootBeat::start(data_dir);
        let mut db = Db::open(&crate::db_path(data_dir))?;
        // **先建後端、再問能力。** 反過來的話，「輸入 hook 裝上了沒」永遠
        // 是在 hook 還沒裝之前問的，於是永遠回報失敗——一則恆假的警告。
        let backend = windows::backend(&config)?;

        // 缺席的能力會讓某些排除規則整組失效，或讓她其實什麼都沒記住。
        // 這兩件事都要在開始錄之前講，不是藏在 doctor 裡等使用者自己去發現。
        let caps = Capabilities::current(&config);
        // 留一份給設定頁。底下那幾行 `⚠` 印在 stdout——而字母人開起來的
        // recorder，stdout 是 `record.log`，一個沒有人會開的檔案。使用者是在
        // 設定頁上打那些排除規則的，所以那一頁才是這句話該出現的地方。
        // 寫不出來不擋錄製：少一行警告，比少一場記錄好。
        if let Err(e) = sister_core::capabilities::write(data_dir, &caps.report()) {
            eprintln!("⚠  寫不出能力報告（設定頁會說「還不知道」）：{e:#}");
        }
        for warning in caps.broken_privacy_rules(&config) {
            println!("⚠  {warning}");
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
        let config_ocr = config.capture.ocr;
        let image_budget_mb = config.capture.max_image_mb_per_day;
        let retention = config.retention.clone();
        let prune_images = frames_root.clone();
        let mut rec = Recorder::new(backend, db, config, images)?;

        install_ctrl_c_handler();
        // 從這一刻開始算足跡。Phase 0 的驗收條件裡有三個數字，而在這之前
        // 沒有任何辦法知道它們是多少——一個量不到的預算不是預算，是一句話。
        let mut footprint = sister_capture::footprint::Footprint::new();
        let disk_at_start = rec
            .db()
            .stats()
            .map(|s| s.db_bytes + s.image_bytes)
            .unwrap_or(0);
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

        // 設定檔熱重載。5 秒一次是因為它的使用情境是「我剛剛在設定頁按了儲存」
        // ——那個人正看著螢幕等它生效。一次 `stat` 的成本在這個間隔下是零。
        const CONFIG_EVERY: Duration = Duration::from_secs(5);
        let watched = config_path.clone().or_else(Config::default_path);
        let mut config_watch = ConfigWatch::new(watched.as_deref());
        let mut last_config_check = Instant::now();
        // 同意書用同一個節拍。它不看 mtime，所以自己一個計時器。
        const CONSENT_EVERY: Duration = Duration::from_secs(5);
        let mut last_consent_check = Instant::now();
        // 心跳。字母人是另一個行程，它沒有別的辦法知道「現在到底有沒有人在
        // 錄」——`sessions.ended_at` 在當掉的時候永遠停在 NULL，而閒置時
        // 資料庫本來就會好一陣子沒有新資料。開機那一段由 `BootBeat` 蓋著，
        // 這裡把它接過來：交棒之後那個執行緒就停了，心跳從此跟著這個迴圈走
        // ——一個蓋得動心跳但迴圈已經卡死的行程，不該還在說自己在錄。
        boot.hand_off();
        let _ = sister_core::heartbeat::beat(data_dir, sister_core::now_ms());
        let mut last_beat = Instant::now();
        // 有人在沒有 recorder 在跑的時候按了「停止」，那個請求會留在磁碟上。
        // 不先清掉的話，這一場會在第一個 tick 就自己結束——而畫面上唯一看得到
        // 的是她閃了一下就不見了。
        sister_core::control::clear_stop(data_dir);
        // 保留期也吃熱重載（設定頁的 TTL 那一欄），所以它不能再是 `let`。
        let mut retention = retention;

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
                Ok(Tick::Kept {
                    frame_id,
                    ocr_blocks,
                    facts,
                }) => {
                    tracing::debug!("frame #{frame_id}：{ocr_blocks} 段文字、{facts} 個事實");
                }
                Ok(_) => {}
            }

            // 設定改了就當場換上。**讀不出來就維持原樣，絕不退回預設值**——
            // 預設值比任何一份使用者自訂的 blocklist 都寬鬆，所以一個打錯的
            // TOML 會安靜地把排除規則全部拿掉。那是這裡唯一不能犯的錯。
            if let Some(path) = watched.as_deref()
                && last_config_check.elapsed() >= CONFIG_EVERY
            {
                last_config_check = Instant::now();
                if config_watch.changed(path) {
                    // 檔案不見了**不算**「請用預設值」。
                    //
                    // `Config::load` 對不存在的路徑會回傳預設值，那在開機時是
                    // 對的（沒有設定檔本來就跑預設）。但在這裡照做的話，一個
                    // 「先刪再寫」的編輯器只要被這 5 秒的輪詢夾中一次，使用者
                    // 整份 blocklist 就被換成比它寬鬆的預設值——而畫面上只會
                    // 印一行「排除 0 個 app」。要回預設值請寫一個空的設定檔。
                    if !path.exists() {
                        println!(
                            "  ⚠  設定檔不見了（{}）。**繼續用舊的那一份**——\
                             真要回到預設值請放一個空的設定檔。",
                            path.display()
                        );
                    } else {
                        match Config::load(path) {
                            Ok(fresh) => {
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
                                rec.set_privacy(fresh.privacy);
                            }
                            Err(e) => println!(
                                "  ⚠  設定檔讀不出來，**繼續用舊的那一份**（不是預設值）：{e:#}"
                            ),
                        }
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

            if last_consent_check.elapsed() >= CONSENT_EVERY {
                last_consent_check = Instant::now();
                let consent = sister_core::consent::load(data_dir);
                match recheck(&consent, wants_images_by_config, rec.stores_images()) {
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
                    Recheck::Images(true) => {
                        println!("  ⟳ 第三張同意書簽回來了：從這一刻起會留截圖。");
                        rec.set_image_dir(Some(frames_root.clone()));
                    }
                    Recheck::Images(false) => {
                        println!(
                            "  ⟳ 第三張同意書被撤回了：從這一刻起只記螢幕上的字，\
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

            if last_report.elapsed() >= Duration::from_secs(60) {
                footprint.tick();
                last_report = Instant::now();
                // 暫停時如果照原樣印那四個數字，讀起來就是「一切正常，只是
                // 這一分鐘沒有新東西」——那正是暫停最危險的失效模式：
                // 使用者以為她還在錄。
                if rec.is_paused() {
                    println!("  ⏸ 仍在暫停中，這一分鐘沒有記錄任何東西。");
                } else {
                    let s = rec.stats();
                    println!(
                        "  … {} tick：保留 {}、重複 {}、排除 {}、讀到 {} 行字{}",
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

        // 走人之前先把心跳收掉。留給 16 秒的逾時去猜的話，那段時間裡字母人
        // 會說她還在錄，而她已經走了——**說她還在錄卻沒在錄**，是這兩個狀態
        // 裡比較危險的那一個。放在 `finish()` 之前，因為那一步會寫資料庫，
        // 可能失敗，而失敗不該讓一個錯的「還在錄」留在磁碟上。
        sister_core::heartbeat::stop(data_dir);

        let stats = rec.stats().clone();
        // 收工前問一次「這段路上掉了什麼」。`doctor` 只看得到開機那一瞬間，
        // 而 UIA 會在半路上永久投降——那之後 excluded_urls 一條都不生效，
        // 卻沒有任何地方會講。見 `Backend::degradations`。
        let lost = sister_capture::Backend::degradations(rec.backend());
        rec.finish(end_reason)?;
        println!(
            "\n完成：{} tick → 保留 {}、重複 {}、排除 {}、無畫面 {}",
            stats.ticks, stats.kept, stats.duplicates, stats.excluded, stats.no_screen
        );
        report_idle(&stats);
        report_exclusions(&stats);
        report_ocr(&stats, config_ocr);
        // 問 recorder 而不是問開機時的設定：第三張同意書中途可能被撤回或簽回來，
        // 而這一段的用途是「她剛剛到底有沒有在寫圖」。拿開機時那份來答，會在
        // 使用者中途撤回之後喊「保留了 12 張畫面卻一張圖都沒寫」的假警報。
        report_images(&stats, rec.timings(), image_budget_mb, rec.stores_images());
        // 先取最後一次足跡樣本，時間表才拿得到「這段期間燒了多少 CPU」，
        // 而那是把牆上時間和 CPU 分開講的前提。
        footprint.tick();
        report_timings(rec.timings(), stats.ticks, footprint.cpu_seconds_used());
        report_footprint(
            &footprint,
            rec.db()
                .stats()
                .map(|s| s.db_bytes + s.image_bytes)
                .unwrap_or(0)
                - disk_at_start,
            stats.image_bytes,
        );
        for line in &lost {
            println!("  ⚠  錄製途中失去的能力：{line}");
        }
        Ok(())
    }

    /// 她自己佔了多少。
    ///
    /// Phase 0 的驗收條件裡有三個數字（CPU < 3%、RAM < 400MB、
    /// 磁碟 < 300MB/天），而在這一段出現之前**沒有任何辦法知道它們是多少**。
    /// 一個量不到的預算不是預算，是一句話——而 README 上遲早要寫這些數字，
    /// 那就必須是她自己量出來的，不是我開工作管理員瞄一眼記下來的。
    ///
    /// 量不到就不印。印一個 0 或一個從三分鐘外推出來的「每天 300MB」，
    /// 都會變成一個很有說服力的假消息，而且會被抄進文件裡。
    /// Phase 0 的驗收預算（PHASES.md）。
    ///
    /// 寫在程式裡而不是只寫在文件裡，是因為文件不會在超標的時候出聲。
    /// 實測那次是 CPU 27.1%、磁碟 11.4 GB/天，而摘要照樣平鋪直敘地印出來，
    /// 沒有任何一個字說「這超標九倍」——要靠讀的人自己記得預算是多少，
    /// 再自己心算。她應該自己講。
    #[cfg(windows)]
    const BUDGET_CPU_PCT: f64 = 3.0;
    #[cfg(windows)]
    const BUDGET_RSS_BYTES: u64 = 400 * 1024 * 1024;
    #[cfg(windows)]
    const BUDGET_DISK_PER_DAY: f64 = 300.0 * 1024.0 * 1024.0;

    #[cfg(windows)]
    fn report_footprint(
        f: &sister_capture::footprint::Footprint,
        disk_delta: i64,
        image_bytes: u64,
    ) {
        /// 超標的就標出來。合格的不標——每一項都掛一個記號等於沒有記號。
        fn over(actual: f64, budget: f64) -> &'static str {
            if actual > budget { "⚠ " } else { "" }
        }

        let mut parts = Vec::new();
        let mut breached = Vec::new();
        if let Some(cpu) = f.cpu_percent() {
            parts.push(format!("{}CPU 平均 {cpu:.1}%", over(cpu, BUDGET_CPU_PCT)));
            if cpu > BUDGET_CPU_PCT {
                breached.push(format!(
                    "CPU {cpu:.1}% 超過預算 {BUDGET_CPU_PCT:.0}%（{:.0} 倍）",
                    cpu / BUDGET_CPU_PCT
                ));
            }
        }
        if let Some(rss) = f.peak_rss_bytes() {
            parts.push(format!(
                "{}RAM 峰值 {}",
                over(rss as f64, BUDGET_RSS_BYTES as f64),
                crate::fmt::bytes(rss as i64)
            ));
            if rss > BUDGET_RSS_BYTES {
                breached.push(format!(
                    "RAM {} 超過預算 {}",
                    crate::fmt::bytes(rss as i64),
                    crate::fmt::bytes(BUDGET_RSS_BYTES as i64)
                ));
            }
        }
        if let Some(per_day) = f.bytes_per_day(disk_delta.max(0) as u64) {
            // 圖與資料庫要分開講。合成一個數字的話，「磁碟 11.4 GB/天」
            // 沒辦法回答唯一有用的那個問題——該去縮圖，還是該去縮索引。
            // 實測那次就是這樣：一個很嚇人、但指不出方向的數字。
            let grew = disk_delta.max(0);
            let rest = grew - image_bytes as i64;
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
            parts.push(format!(
                "{}磁碟 {}/天（{breakdown}）",
                over(per_day, BUDGET_DISK_PER_DAY),
                crate::fmt::bytes(per_day as i64),
            ));
            if per_day > BUDGET_DISK_PER_DAY {
                breached.push(format!(
                    "磁碟 {}/天 超過預算 {}/天（{:.0} 倍）",
                    crate::fmt::bytes(per_day as i64),
                    crate::fmt::bytes(BUDGET_DISK_PER_DAY as i64),
                    per_day / BUDGET_DISK_PER_DAY
                ));
            }
        }
        if parts.is_empty() {
            return;
        }
        println!("  足跡：{}", parts.join("、"));
        for line in &breached {
            println!("  ⚠  {line}");
        }
        if !breached.is_empty() {
            println!(
                "        （Phase 0 的驗收條件見 docs/PHASES.md。\
                 短時間的錄製外推一整天本來就會偏高，真正算數的是整天的實測）"
            );
        }
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
        store_images: bool,
    ) {
        let written = timings.store.calls;

        // 「保留了 12 張畫面、磁碟上一張圖都沒有」原本會讓這一整段消失，
        // 因為每個計數剛好都是 0。那正好是最需要說話的時候：資料夾沒權限、
        // 磁碟滿了、路徑被佔用，症狀全都長這樣。
        if written == 0 && stats.kept > 0 && store_images {
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
        if written == 0 && stats.images_throttled == 0 && stats.images_over_budget == 0 {
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

        // 這一句要單獨佔一行、而且要講得像一件事，不是像一個統計欄位。
        if stats.images_over_budget > 0 {
            println!(
                "  ⚠  今天的畫面額度（{budget_mb} MB）用完了，之後的 {} 張只留了字。\
                 文字與搜尋不受影響；要留更多圖就調大 capture.max_image_mb_per_day",
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
    fn report_timings(t: &sister_capture::timings::Timings, ticks: u64, cpu_secs: Option<f64>) {
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
        println!(
            "  時間：{ticks} tick 佔了 {:.1} 秒（每 tick {:.0} ms）{cpu}",
            total.as_secs_f64(),
            total.as_secs_f64() * 1000.0 / ticks as f64
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
    #[cfg(windows)]
    fn report_ocr(stats: &sister_capture::RecorderStats, ocr_enabled: bool) {
        if !ocr_enabled {
            println!("  讀字：已關閉（畫面留下了，但上面的字沒有進資料庫）");
            return;
        }
        println!(
            "  讀字：{} 行{}",
            stats.ocr_blocks,
            if stats.ocr_failures > 0 {
                format!("，{} 次失敗", stats.ocr_failures)
            } else {
                String::new()
            }
        );
        if let Some(e) = &stats.last_ocr_error {
            println!("        最後一次的錯誤：{e}");
        }
        if stats.ocr_blocks == 0 && stats.kept > 0 {
            println!(
                "  ⚠  保留了 {} 張畫面，但一行字都沒讀到——這些畫面搜不到。\
                 跑 `sister doctor` 看是引擎讀不出字，還是讀不到你這台螢幕。",
                stats.kept
            );
        }
    }
    #[cfg(test)]
    mod record_tests {
        use super::{BootBeat, ConfigWatch, already_recording};
        use crate::ops::tmp::Tmp;
        use sister_core::config::Config;
        use sister_core::consent::{Consent, Sheet};
        use sister_core::heartbeat;

        #[test]
        fn a_recorder_that_is_still_opening_the_database_is_already_visible() {
            // 這一條顧的是「第二個 recorder」。開機那一段（migration、能力探測、
            // 開場那次 prune）在大的資料庫上要好幾分鐘，而以前第一次心跳排在
            // 那全部之後——那幾分鐘裡她對 `is_recording` 是隱形的，字母人於是
            // 放行第二個 `sister record` 打同一顆資料庫。
            let dir = Tmp::new("boot-beat");
            assert!(
                !heartbeat::is_recording(&dir.0, sister_core::now_ms()),
                "還沒開始，不該有心跳"
            );
            let boot = BootBeat::start(&dir.0);
            // 立刻，不是等第一個 5 秒間隔——那個間隔正是要補的洞。
            assert!(
                heartbeat::is_recording(&dir.0, sister_core::now_ms()),
                "開機的第一瞬間就要看得見她在起來"
            );
            drop(boot);
        }

        #[test]
        fn two_terminals_do_not_get_two_recorders_on_one_database() {
            // 字母人那一邊早就擋了，但 `sister record` 自己沒有——所以開兩個
            // 終端機各打一次就成立。兩個行程對同一顆資料庫各錄一份，唯一的
            // 症狀是磁碟用得比講好的快一倍，而使用者會以為是保留期壞了。
            let dir = Tmp::new("two-recorders");
            already_recording(&dir.0).expect("沒有人在錄的時候要放行");
            let _first = BootBeat::start(&dir.0);
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
            let _first = BootBeat::start(&dir.0);

            let err = super::run(&dir.0, Config::default(), None, None)
                .expect_err("已經有人在錄，第二個不該起得來");
            let said = format!("{err}");
            assert!(
                said.contains("已經有一個 sister record"),
                "擋下來的該是那道閘門，不是平台檢查：{said}"
            );
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
            drop(BootBeat::start(&dir.0));
            assert!(
                !heartbeat::is_recording(&dir.0, sister_core::now_ms()),
                "沒交棒就走了＝這次開機沒成功，心跳要收掉"
            );
        }

        #[test]
        fn handing_off_leaves_the_heartbeat_for_the_loop_that_took_over() {
            // 反過來也要成立：交棒之後這個守衛收工，但心跳是**還在跑的那個
            // 迴圈**的，不能跟著一起被清掉——清掉的話她剛起來就自己說沒在錄。
            let dir = Tmp::new("boot-handoff");
            let mut boot = BootBeat::start(&dir.0);
            boot.hand_off();
            drop(boot);
            assert!(
                heartbeat::is_recording(&dir.0, sister_core::now_ms()),
                "交棒之後心跳歸主迴圈管，守衛不准動它"
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
            let out = super::gate(&dir.0, config).expect("第一張簽了就該放行");
            assert!(!out.capture.store_images, "第三張沒簽就不可以寫圖");
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
            let out = super::gate(&dir.0, config).expect("兩張都簽了");
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
            assert_eq!(recheck(&c, true, true), Recheck::Stop);
            assert_eq!(
                recheck(&c, false, false),
                Recheck::Stop,
                "本來就沒在寫圖也一樣要停：停的是整場，不是截圖那一半"
            );
        }

        #[test]
        fn revoking_the_screenshot_sheet_only_stops_the_pictures() {
            let c = signed(&[Sheet::LocalRecording]);
            assert_eq!(recheck(&c, true, true), Recheck::Images(false));
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
            assert_eq!(recheck(&c, true, false), Recheck::Images(true));
        }

        /// 同意書說可以，但設定檔說不要——那就是不要。
        /// 同意是**上限**，不是開關：簽了不代表使用者現在就想留圖。
        #[test]
        fn consent_never_turns_on_something_the_config_turned_off() {
            let c = signed(&[Sheet::LocalRecording, Sheet::FrameStorage]);
            assert_eq!(recheck(&c, false, false), Recheck::Same);
        }

        #[test]
        fn nothing_changed_means_nothing_happens() {
            let c = signed(&[Sheet::LocalRecording, Sheet::FrameStorage]);
            assert_eq!(recheck(&c, true, true), Recheck::Same);
            let c = signed(&[Sheet::LocalRecording]);
            assert_eq!(recheck(&c, true, false), Recheck::Same);
        }

        /// 條文改版 = 三張一起失效，所以它和「撤回第一張」走同一條路。
        #[test]
        fn a_wording_change_mid_run_stops_her_too() {
            let mut c = signed(&[Sheet::LocalRecording, Sheet::FrameStorage]);
            c.version = sister_core::consent::VERSION + 1;
            assert_eq!(recheck(&c, true, true), Recheck::Stop);
        }
    }
}
