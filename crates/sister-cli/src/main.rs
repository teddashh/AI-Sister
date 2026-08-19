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
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
        /// 輸出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 查 L1 事實（金額、電話、日期……）
    Facts {
        /// 只看某一類：money / phone / url / email / file_path / error_code / id_like / datetime
        #[arg(short, long)]
        kind: Option<String>,
        /// 在原文或正規化值裡做子字串比對
        #[arg(short, long)]
        search: Option<String>,
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
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

    /// 叫她閉眼睛。正在跑的 `record` 下一個 tick 就會停下來。
    ///
    /// 暫停**不會自己過期**——她會一直停到有人 `sister resume`（或在字母人
    /// 上按一下）為止。這是刻意的：一個會自己醒來的暫停等於沒有暫停。
    Pause,

    /// 解除暫停。
    Resume,

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
        Command::Query { text, limit, json } => {
            ops::query::run(&data_dir, &text.join(" "), limit, json)
        }
        Command::Facts {
            kind,
            search,
            limit,
            json,
        } => ops::facts::run(&data_dir, kind.as_deref(), search.as_deref(), limit, json),
        Command::Stats { json } => ops::stats::run(&data_dir, json),
        Command::Prune { dry_run } => ops::prune::run(&data_dir, &config, dry_run),
        Command::Pause => ops::pause::run(&data_dir, true),
        Command::Resume => ops::pause::run(&data_dir, false),
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
