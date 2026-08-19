//! `sister` — Phase 0 的全部使用者介面。
//!
//! 這支 CLI 存在的理由不是方便，而是**可稽核**：使用者必須能自己看見
//! 她記了什麼、記了多少、以及每一句話的出處（SPEC §11.4）。
//! 在有 GUI 之前，這裡就是唯一的驗證入口。

mod fmt;
mod ops;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use sister_core::config::Config;

#[derive(Parser)]
#[command(
    name = "sister",
    version,
    about = "AI-Sister：一個記得住的本機夥伴",
    long_about = "AI-Sister — 本機優先的長期記憶。\n\n\
                  Phase 0 只做一件事：把螢幕上發生過的事忠實記下來，\n\
                  並且讓你查得到、看得見出處、刪得掉。"
)]
struct Cli {
    /// 資料目錄（預設為系統慣例位置）
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// 設定檔路徑
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// 顯示詳細日誌
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 開始錄製（需要平台擷取後端）
    Record {
        /// 錄多久後自動停止（秒）。省略則持續到 Ctrl-C。
        #[arg(long, value_name = "SECS")]
        duration: Option<u64>,
    },

    /// 重播一份腳本，把結果寫進資料庫
    ///
    /// 這是無頭機器上驗證整條管線的方式，也是 replay 評測的入口。
    Replay {
        /// 腳本檔（JSON）
        scenario: PathBuf,
        /// tick 間隔（毫秒）
        #[arg(long, default_value_t = 1000)]
        interval_ms: i64,
        /// 寫進暫時的記憶體資料庫，不碰真正的資料
        #[arg(long)]
        dry_run: bool,
        /// 把腳本的第 0 秒對應到「幾天前」。預設 0 = 剛剛結束。
        #[arg(long, default_value_t = 0.0, value_name = "DAYS")]
        days_ago: f64,
        /// 直接指定腳本零點的 epoch 毫秒（給評測用，蓋過 --days-ago）
        #[arg(long, value_name = "EPOCH_MS")]
        start: Option<i64>,
    },

    /// 全文檢索。每一筆結果都附上出處。
    Query {
        /// 要找的字
        text: Vec<String>,
        #[arg(short, long, default_value_t = 10, value_parser = at_least_one)]
        limit: usize,
        /// 輸出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 查 L1 事實（金額、電話、日期……）
    Facts {
        // 清單從 FactKind::ALL 長出來，不手抄。手抄的那份不會編不過，它只會
        // 在加了一類事實之後，安靜地變成一份少一項的選單。
        #[arg(short, long, help = format!("只看某一類：{}", sister_core::facts::FactKind::names(" / ")))]
        kind: Option<String>,
        /// 在原文或正規化值裡做子字串比對
        #[arg(short, long)]
        search: Option<String>,
        #[arg(short, long, default_value_t = 20, value_parser = at_least_one)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    /// 你問過她什麼——本機題庫（PHASES.md Phase 2 的評測語料就從這裡長出來）
    ///
    /// **一筆都沒找到的那些題目是這裡面最有價值的**：找得回來的只證明她現在
    /// 能做什麼，找不回來的才是下一版要修的東西。可以用 `privacy.query_log`
    /// 關掉，關掉之後這裡就不會再累積。
    Queries {
        #[arg(short, long, default_value_t = 20, value_parser = at_least_one)]
        limit: usize,
        /// 只看她一筆都沒找到的那些
        #[arg(long)]
        empty: bool,
        #[arg(long)]
        json: bool,
    },

    /// 她記了多少東西、佔了多少空間
    Stats {
        #[arg(long)]
        json: bool,
    },

    /// 讓過期的東西真的消失（保留期由 config 的 retention 決定）
    ///
    /// 錄製開始時本來就會自動跑一次。這個子命令是給「我現在就要它消失」
    /// 以及「先讓我看看會刪掉什麼」用的。
    Prune {
        /// 只印出會刪掉什麼，一個位元組都不動
        #[arg(long)]
        dry_run: bool,
    },

    /// 把記憶整份帶走（SPEC §11.8 資料主權）。
    ///
    /// **不要自己複製 `sister.db`。** 資料庫跑在 WAL 模式，她正在錄的時候，
    /// 最近寫進去的東西還躺在旁邊的 `sister.db-wal` 裡——只複製主檔的備份會
    /// 安靜地少掉最後那一段，而你會在真的需要它的那天才發現。
    ///
    /// 匯出的目的地就是一個資料目錄，不是另一種格式：
    /// `sister --data-dir <匯出的目錄> query 電話` 直接問得到。
    Export {
        /// 匯出到哪個目錄。裡面已經有 `sister.db` 就拒絕，不覆蓋。
        #[arg(long, value_name = "目錄")]
        to: PathBuf,

        /// 連畫面檔一起帶走。它們通常比資料庫大好幾個數量級，先看 `sister stats`。
        #[arg(long)]
        with_frames: bool,
    },

    /// 忘掉最近一段時間——當作那段時間沒發生過。
    ///
    /// 和 `prune` 不一樣：`prune` 刪的是保留期已經答應要刪的東西，這個刪的是
    /// 他本來想留、現在改變主意的東西。所以預設**只看不刪**，真的要動要加
    /// `--yes`；`prune` 反過來，預設就做。兩個預設不同不是不一致，是因為搞錯
    /// 的後果不一樣。
    ///
    /// 沒有回收桶，也沒有復原。那段時間裡的字、事實、畫面檔、事件、連他自己
    /// 問過的話，全部一起走。
    Forget {
        /// 往回算多久：`30m`、`2h`、`7d`。**單位不可以省。**
        #[arg(long, value_name = "多久")]
        last: String,

        /// 真的刪。不加就只是預覽。
        #[arg(long)]
        yes: bool,
    },

    /// 叫她閉眼睛。正在跑的 `record` 下一個 tick 就會停下來。
    ///
    /// 暫停**不會自己過期**——她會一直停到有人 `sister resume`（或在字母人
    /// 上按一下）為止。這是刻意的：一個會自己醒來的暫停等於沒有暫停。
    Pause,

    /// 解除暫停。
    Resume,

    /// 請正在跑的 `record` 收工。
    ///
    /// 和 `pause` 不一樣：暫停是「先別看，但留在這裡」，這個是「今天到此為止」
    /// ——那個行程會結束。乾淨地收尾（寫完 session、收掉心跳），所以下一個
    /// tick 才會停，不是立刻。
    Stop,

    /// 三張同意書：現在簽了哪幾張、沒簽會怎樣。
    ///
    /// 不帶參數就是印出目前的狀態。**第一張沒簽，`sister record` 不會開始錄。**
    Consent {
        /// 簽下去。可以重複：`--grant local-recording --grant frame-storage`
        #[arg(long, value_name = "SHEET")]
        grant: Vec<String>,
        /// 撤回。撤回本機記錄之後她就停了。
        #[arg(long, value_name = "SHEET")]
        revoke: Vec<String>,
        #[arg(long)]
        json: bool,
    },

    /// 環境檢查：這台機器能不能好好跑
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                if cli.verbose {
                    "debug".into()
                } else {
                    "warn".into()
                }
            }),
        )
        .with_target(false)
        .init();

    let config = load_config(cli.config.as_deref())?;
    let data_dir = resolve_data_dir(cli.data_dir.clone())?;

    match cli.command {
        Command::Record { duration } => {
            ops::record::run(&data_dir, config, cli.config.clone(), duration)
        }
        Command::Replay {
            scenario,
            interval_ms,
            dry_run,
            days_ago,
            start,
        } => ops::replay::run(
            &data_dir,
            config,
            &scenario,
            interval_ms,
            dry_run,
            days_ago,
            start,
        ),
        Command::Query { text, limit, json } => ops::query::run(
            &data_dir,
            &text.join(" "),
            limit,
            json,
            config.privacy.query_log,
        ),
        Command::Facts {
            kind,
            search,
            limit,
            json,
        } => ops::facts::run(&data_dir, kind.as_deref(), search.as_deref(), limit, json),
        Command::Queries { limit, empty, json } => ops::queries::run(&data_dir, limit, empty, json),
        Command::Stats { json } => ops::stats::run(&data_dir, json),
        Command::Prune { dry_run } => ops::prune::run(&data_dir, &config, dry_run),
        Command::Export { to, with_frames } => ops::export::run(&data_dir, &to, with_frames),
        Command::Forget { last, yes } => ops::forget::run(&data_dir, &last, yes),
        Command::Pause => ops::pause::run(&data_dir, true),
        Command::Resume => ops::pause::run(&data_dir, false),
        Command::Stop => ops::stop::run(&data_dir),
        Command::Consent {
            grant,
            revoke,
            json,
        } => ops::consent::run(&data_dir, &grant, &revoke, json),
        Command::Doctor => ops::doctor::run(&data_dir, &config, cli.config.clone()),
    }
}

fn load_config(explicit: Option<&std::path::Path>) -> Result<Config> {
    match explicit {
        Some(p) => Config::load(p).with_context(|| format!("load config {}", p.display())),
        None => match Config::default_path() {
            // 沒有設定檔是正常狀態，用預設值——預設值本來就該是安全的
            Some(p) if p.exists() => {
                Config::load(&p).with_context(|| format!("load config {}", p.display()))
            }
            _ => Ok(Config::default()),
        },
    }
}

fn resolve_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    explicit
        .or_else(Config::default_data_dir)
        .context("cannot determine a data directory; pass --data-dir")
}

/// 資料庫檔案的位置。真正的決定在 [`Config::db_path`]——桌面視窗也要開
/// 同一顆檔案，所以那個檔名只能有一個地方說了算。這裡只是轉手。
pub fn db_path(data_dir: &std::path::Path) -> PathBuf {
    Config::db_path(data_dir)
}

/// `--limit 0` 要當場被拒絕。
///
/// 因為它會讓每一個指令都說謊，而且說的是最要命的那種謊：
///
/// ```text
/// $ sister facts --limit 0
/// 這份記憶裡還沒有任何事實——她還沒錄過，或……      ← 其實有 9 筆
/// $ sister query 電話 --limit 0
/// 沒有找到。
/// 不過你自己的排除規則擋掉過東西……                  ← 還幫他想了個錯的理由
/// ```
///
/// 每個空答案畫面都是照著「查得到 vs 查不到」寫的，沒有一個想過「你要的就是
/// 零筆」。與其在三個地方各補一個 `if limit == 0`（同一條規則寫三份，這個專案
/// 已經栽在這上面很多次），不如讓它根本進不來——零筆的請求沒有任何合理用途。
fn at_least_one(s: &str) -> std::result::Result<usize, String> {
    match s.parse::<usize>() {
        Ok(0) => Err("0 問不出東西來。要「全部」的話給一個大一點的數字。".into()),
        Ok(n) => Ok(n),
        Err(e) => Err(format!("{s} 不是一個筆數：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_for_zero_rows_is_refused_not_answered_with_an_empty_screen() {
        let e = at_least_one("0").expect_err("0 要被拒絕");
        assert!(e.contains("全部"), "要告訴他想要什麼該怎麼打：{e}");

        assert_eq!(at_least_one("1"), Ok(1));
        assert_eq!(at_least_one("20"), Ok(20));
        assert!(at_least_one("-1").is_err(), "負數不是筆數");
        assert!(at_least_one("很多").is_err(), "不是數字就不是數字");
    }
}
