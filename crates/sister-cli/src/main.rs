//! `sister` — Phase 0 的全部使用者介面。
//!
//! 這支 CLI 存在的理由不是方便，而是**可稽核**：使用者必須能自己看見
//! 她記了什麼、記了多少、以及每一句話的出處（SPEC §11.4）。
//! 在有 GUI 之前，這裡就是唯一的驗證入口。

#[cfg(any(windows, test))]
mod disk_attribution;
mod fmt;
mod ops;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
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

#[derive(Args)]
#[command(
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    subcommand_precedence_over_arg = true
)]
struct ReplayArgs {
    #[command(subcommand)]
    action: Option<ReplayAction>,

    /// 舊式腳本重播的 JSON 路徑。`replay export/import` 另見子命令。
    #[arg(value_name = "腳本", required = true)]
    scenario: Option<PathBuf>,

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
}

#[derive(Subcommand)]
enum ReplayAction {
    /// 把最近一段真實 L0/L1 記憶打包成去敏的私有 replay 草稿
    Export {
        /// 往回匯出多久：30m、2h、7d；預設最近一天
        #[arg(long, default_value = "24h", value_name = "多久")]
        last: String,
        /// 草稿路徑；省略就放進資料目錄的 replay-drafts/
        #[arg(long, value_name = "檔案")]
        to: Option<PathBuf>,
        /// 同一時間窗的 query log 題目草稿（保留原問句，必須人工標註／審查）
        #[arg(long, value_name = "檔案")]
        questions_to: Option<PathBuf>,
        /// 語料名稱；省略就從時間範圍產生
        #[arg(long)]
        name: Option<String>,
    },
    /// 把 replay 語料匯回本機資料庫，重建搜尋索引與 L1 事實
    Import {
        /// replay corpus JSON
        corpus: PathBuf,
        /// 只在記憶體資料庫驗證，不碰真正資料
        #[arg(long)]
        dry_run: bool,
        /// 把語料第 0 秒對應到「幾天前」。預設 0 = 剛剛結束。
        #[arg(long, default_value_t = 0.0, value_name = "DAYS")]
        days_ago: f64,
        /// 直接指定語料零點的 epoch 毫秒（蓋過 --days-ago）
        #[arg(long, value_name = "EPOCH_MS")]
        start: Option<i64>,
    },
    /// 在同一份 corpus 上比較 text baseline 與 +facts，產生可重現報告
    Evaluate {
        /// replay corpus JSON
        corpus: PathBuf,
        /// 人工標註的 recall QA JSON
        questions: PathBuf,
        /// 每題最多看前幾筆結果
        #[arg(long, default_value_t = 5, value_parser = at_least_one)]
        k: usize,
        /// 暖身後每題實測幾次延遲
        #[arg(long, default_value_t = 3, value_parser = at_least_one)]
        runs: usize,
        /// 把完整 report JSON 印到 stdout
        #[arg(long, conflicts_with = "to")]
        json: bool,
        /// 把完整 report JSON 寫到新檔案（不覆寫）
        #[arg(long, value_name = "檔案", conflicts_with = "json")]
        to: Option<PathBuf>,
        /// 同一份題庫再跑一次 +interpreter+reviewer，輸出並排與差值。沒贏就明講沒贏。
        #[arg(long)]
        ab: bool,
        /// 評測用的 CLI。沒給的話 +brain 那一路會跳過，並寫出原因。
        #[arg(long, value_name = "COMMAND", requires = "ab")]
        brain_command: Option<String>,
        /// 傳給 `--brain-command` 的參數，可重複。
        #[arg(long = "brain-arg", value_name = "ARG", requires = "ab")]
        brain_arg: Vec<String>,
    },
    /// 把 query-log Draft 逐題補成可跑、可審查的 recall 題庫
    Questions {
        #[command(subcommand)]
        action: ReplayQuestionAction,
    },
    /// 承諾集與開口判斷集：該提醒什麼／這一刻該不該講。不會讓產品開口。
    Moments {
        #[command(subcommand)]
        action: ReplayMomentAction,
    },
}

#[derive(Subcommand)]
enum ReplayQuestionAction {
    /// 只看已標／未標實數，不印題目原話
    Status {
        /// replay corpus JSON
        corpus: PathBuf,
        /// recall question-set JSON
        questions: PathBuf,
    },
    /// 在終端逐題標註；候選只是提示，不會自動當成正解
    Annotate {
        /// replay corpus JSON
        corpus: PathBuf,
        /// 尚未審查的 recall question-set JSON
        questions: PathBuf,
        /// 寫入另一個新檔案；不會改寫來源
        #[arg(long, value_name = "檔案")]
        to: PathBuf,
        /// 每題顯示幾筆產品檢索候選
        #[arg(long, default_value_t = 5, value_parser = at_least_one)]
        k: usize,
        /// 連已標過的題目也重新走一遍
        #[arg(long)]
        all: bool,
    },
    /// 全部標註有效後，把另一份新檔標成 Reviewed
    Review {
        /// replay corpus JSON
        corpus: PathBuf,
        /// 已經逐題標完的 Draft question set
        questions: PathBuf,
        /// Reviewed 輸出；不會改寫 Draft
        #[arg(long, value_name = "檔案")]
        to: PathBuf,
        /// 確認題目原話與答案都已由人檢查，知道它們沒有自動去敏
        #[arg(long)]
        confirm_private_text_reviewed: bool,
    },
}

#[derive(Subcommand)]
enum ReplayMomentAction {
    /// 掃 corpus，把值得人看一眼的時刻提出來；全部未標
    Draft {
        /// replay corpus JSON
        corpus: PathBuf,
        /// 寫入新檔案；不會覆寫已存在的檔
        #[arg(long, value_name = "檔案")]
        to: PathBuf,
    },
    /// 只看已標／未標實數，不印原話
    Status {
        /// replay corpus JSON
        corpus: PathBuf,
        /// moment-set JSON
        moments: PathBuf,
        /// 把計數與 fingerprint 印成 JSON；不含提醒原文或 why
        #[arg(long)]
        json: bool,
    },
    /// 在終端逐個標註；候選只是提示，不會自動當成正解
    Annotate {
        /// replay corpus JSON
        corpus: PathBuf,
        /// 尚未審查的 moment-set JSON
        moments: PathBuf,
        /// 寫入另一個新檔案；不會改寫來源
        #[arg(long, value_name = "檔案")]
        to: PathBuf,
        /// 連已標過的時刻也重新走一遍
        #[arg(long)]
        all: bool,
    },
    /// 全部標註有效後，把另一份新檔標成 Reviewed
    Review {
        /// replay corpus JSON
        corpus: PathBuf,
        /// 已經逐個標完的 Draft moment set
        moments: PathBuf,
        /// Reviewed 輸出；不會改寫 Draft
        #[arg(long, value_name = "檔案")]
        to: PathBuf,
        /// 確認提醒原文與 why 都已由人檢查，知道它們沒有自動去敏
        #[arg(long)]
        confirm_private_text_reviewed: bool,
    },
}

#[derive(Subcommand)]
enum BrainAction {
    /// 外送紀錄：什麼時候、送給哪支命令、送了哪個 segment、送了多少字、拿回什麼。不含原文。
    Log {
        #[arg(short, long, default_value_t = 20, value_parser = at_least_one)]
        limit: usize,
    },
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
    Replay(ReplayArgs),

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
        /// 只看你標記過「我本來已經忘了」的那些
        #[arg(long)]
        marked: bool,
        #[arg(long)]
        json: bool,
    },

    /// 「這一題我本來已經忘了」——標記一次她真的幫上忙的時刻
    ///
    /// PHASES.md Phase 1 的第一條退場條件是「自用 7 天內 ≥ 3 次答對我自己都忘
    /// 掉的東西」，而那件事**只有你知道**：題庫記得住你問了什麼、她給了幾筆、
    /// 你點開了哪一個出處，記不住你當時知不知道那個答案。點開出處也不是它——
    /// 那件事最常發生在她答錯、或你在查核的時候。
    ///
    /// 一個禮拜之後回頭補不了：那是你看到答案那一刻腦袋裡的狀態。所以它是一個
    /// 當下按的按鈕，不是一份事後的問卷。
    ///
    /// 不帶參數就是標記你**剛剛問的那一題**。標錯了就 `--undo`。
    ///
    /// 不管走哪一條，它都會把**它到底標了哪一題**連題目原話一起印出來。那不是
    /// 客套，是要你看的：題號會被重複使用。`#N` 是 SQLite 的 rowid，最大號那
    /// 一列被刪掉之後（`forget` 或保留期），下一題就會拿到同一個號碼。於是一
    /// 個從舊畫面上抄下來的號碼，可能指到一題完全不同的問題——實測過。
    /// 印出來的那句話是唯一分得出來的地方，所以它一定要印，而你一定要看。
    Mark {
        /// 標哪一題（`sister queries` 每一列開頭那個 `#N`）。不給就是最近那一題
        ///
        /// 這個號碼**不是永久的**：題目被刪掉之後號碼會被下一題撿走。隔了一段
        /// 時間才要標的話，先 `sister queries` 重看一次，別用舊的。
        #[arg(long, value_name = "題號")]
        id: Option<i64>,
        /// 收回這個標記
        #[arg(long)]
        undo: bool,
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

    /// 逐步顯示承諾卡的下一步；每一步都要你親手核准才會交給作業系統。
    Do {
        /// 這次授權的任務描述。核准綁的是「具體動作 + 具體目標」，不是「幫我處理這件事」。
        #[arg(long)]
        task: String,
        /// 允許碰哪些 app（可重複）。步驟宣告的 app 不在裡面就拒絕。
        #[arg(long = "app")]
        apps: Vec<String>,
        /// 允許哪幾類動作（可重複）：open-url / open-file / focus-window。預設 open-url。
        #[arg(long = "allow")]
        allow: Vec<String>,
        /// 授權多久後失效（分鐘）。
        #[arg(long, default_value_t = 5)]
        minutes: u64,
        /// 步數上限。0 會被拒絕（`StepLimit` 鑄不出來）。
        #[arg(long, default_value_t = 3)]
        steps: u32,
        /// 只印出這張授權書會涵蓋哪些步驟，不問、不做。
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

    /// 請正在跑的 `record` 收工。
    ///
    /// 和 `pause` 不一樣：暫停是「先別看，但留在這裡」，這個是「今天到此為止」
    /// ——那個行程會結束。乾淨地收尾（寫完 session、收掉心跳），所以下一個
    /// tick 才會停，不是立刻。
    Stop,

    /// 把最近關閉的段落交給設定的 CLI，收回一張 L2 假設卡片。
    ///
    /// `--dry-run` 印出**這一刻真的會送出去的那段字**（原文，沒遮），一個字都不送。
    /// 沒簽第二張同意書、沒設定 [brain] command、預算用完，三種原因印三種話。
    Interpret {
        /// 印出會送出的全文，一個字都不送
        #[arg(long)]
        dry_run: bool,
        /// 往回看多久：`30m`、`2h`、`7d`。預設最近一天
        #[arg(long, default_value = "24h", value_name = "多久")]
        last: String,
        /// 最多處理幾段
        #[arg(long, default_value_t = 4, value_parser = at_least_one)]
        limit: usize,
        /// 指定某一段的 core_started_at（epoch 毫秒）。有的話跳過「值不值得」那一關
        #[arg(long, value_name = "EPOCH_MS")]
        at: Option<i64>,
    },

    /// 解釋層的外送紀錄與降級紀錄
    Brain {
        #[command(subcommand)]
        action: BrainAction,
    },

    /// 審閱層：讀 L2、回查 L0 原件、必要時雙 pass，寫入 L3。
    ///
    /// 活躍時最短 15 分鐘一輪；`--eod` 是日終盤點。不是每秒輪詢。
    /// 沒簽同意書 2、沒設定 CLI、預算用完、還沒到間隔，四種原因印四種話。
    Review {
        #[arg(long)]
        dry_run: bool,
        /// 往回看多久：`30m`、`2h`、`7d`。預設最近一天
        #[arg(long, default_value = "24h", value_name = "多久")]
        last: String,
        /// 日終盤點（寫日摘要、把到期未互動的轉封存）
        #[arg(long)]
        eod: bool,
        /// 不理 15 分鐘間隔
        #[arg(long)]
        force: bool,
    },

    /// 承諾表。只有兩個動作：結案、其他一切（snooze + 降權）。
    Commitments {
        /// 結案（= dead）。帶 `--note` 寫進 kill_note。
        #[arg(long, value_name = "ID")]
        kill: Option<i64>,
        /// 其他一切（= snooze + 降權）
        #[arg(long, value_name = "ID")]
        other: Option<i64>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// 主動開口守門員：預演現在的候選，或查看判決紀錄。
    Speak {
        #[arg(long, conflicts_with = "log")]
        dry_run: bool,
        #[arg(long, conflicts_with = "dry_run")]
        log: bool,
        #[arg(long, requires = "log", value_name = "YYYY-MM-DD")]
        day: Option<String>,
    },

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

    /// 量 GDI 抓圖三段，以及 OCR 縮圖的速度與準確度
    ///
    /// `doctor` 問的是「能不能」，這個問的是「多貴」。實測一次擷取 127 ms
    /// ——除以 3.7M 像素是 34 ns/像素，而同樣 14.7 MB 的 memcpy 只要 1.5 ms。
    /// 那不是搬運，是有人在逐像素做事。這張表把一次擷取拆成建立 GDI 物件、
    /// BitBlt、GetDIBits 三段，一次只換一個變因，好知道要往哪裡改。
    ///
    /// 它不寫資料庫，也不留任何畫面。會印兩張表；OCR 表另花約一分鐘，
    /// 期間請不要動畫面，因為它會拿螢幕上的字當比較基準。
    Bench {
        /// 每種抓法量幾次（另有一輪熱身；省略時兩張表各用自己的預設）
        #[arg(long)]
        rounds: Option<u32>,
    },
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

    let data_dir = resolve_data_dir(cli.data_dir.clone())?;

    // **在需要的地方才載，不是在門口。**
    //
    // 這裡本來是 `let config = load_config(..)?;`，於是一個 TOML 語法錯會讓
    // **每一個**子命令停在門口，包括那兩個你正是因為出事了才會打開的：
    // `doctor` 和 `bench`。而它們兩個一個只需要看這台機器的能力、一個根本
    // 不讀設定檔。用設定檔把設定檔的診斷工具擋掉，是在最需要它的那一刻把它
    // 關掉。
    //
    // 其餘子命令照舊「壞掉就停」，而且理由沒有變：它們會照著那份設定去動
    // 使用者的資料（保留期、排除規則、要不要存圖），拿預設值去做那些事比
    // 停下來危險得多。
    let config = || load_config(cli.config.as_deref());

    match cli.command {
        Command::Record { duration } => {
            ops::record::run(&data_dir, config()?, cli.config.clone(), duration)
        }
        Command::Replay(args) => match args.action {
            Some(ReplayAction::Export {
                last,
                to,
                questions_to,
                name,
            }) => ops::replay::export_corpus(
                &data_dir,
                &last,
                to.as_deref(),
                questions_to.as_deref(),
                name.as_deref(),
            ),
            Some(ReplayAction::Import {
                corpus,
                dry_run,
                days_ago,
                start,
            }) => ops::replay::import_corpus(&data_dir, &corpus, dry_run, days_ago, start),
            Some(ReplayAction::Evaluate {
                corpus,
                questions,
                k,
                runs,
                json,
                to,
                ab,
                brain_command,
                brain_arg,
            }) => ops::replay::evaluate_corpus(
                &corpus,
                &questions,
                ops::replay::EvaluateOpts {
                    k,
                    runs,
                    json,
                    output: to.as_deref(),
                    ab,
                    brain: brain_command.map(|command| sister_core::eval::BrainEval {
                        command,
                        args: brain_arg,
                    }),
                },
            ),
            Some(ReplayAction::Questions { action }) => match action {
                ReplayQuestionAction::Status { corpus, questions } => {
                    ops::replay::question_status(&corpus, &questions)
                }
                ReplayQuestionAction::Annotate {
                    corpus,
                    questions,
                    to,
                    k,
                    all,
                } => ops::replay::annotate_questions(&corpus, &questions, &to, k, all),
                ReplayQuestionAction::Review {
                    corpus,
                    questions,
                    to,
                    confirm_private_text_reviewed,
                } => ops::replay::review_questions(
                    &corpus,
                    &questions,
                    &to,
                    confirm_private_text_reviewed,
                ),
            },
            Some(ReplayAction::Moments { action }) => match action {
                ReplayMomentAction::Draft { corpus, to } => {
                    ops::replay::draft_moments(&corpus, &to)
                }
                ReplayMomentAction::Status {
                    corpus,
                    moments,
                    json,
                } => ops::replay::moment_status(&corpus, &moments, json),
                ReplayMomentAction::Annotate {
                    corpus,
                    moments,
                    to,
                    all,
                } => ops::replay::annotate_moments(&corpus, &moments, &to, all),
                ReplayMomentAction::Review {
                    corpus,
                    moments,
                    to,
                    confirm_private_text_reviewed,
                } => ops::replay::review_moments(
                    &corpus,
                    &moments,
                    &to,
                    if confirm_private_text_reviewed {
                        sister_core::moments::ConfirmPrivateTextReviewed::CONFIRMED
                    } else {
                        sister_core::moments::ConfirmPrivateTextReviewed::NOT_CONFIRMED
                    },
                ),
            },
            None => ops::replay::run(
                &data_dir,
                config()?,
                args.scenario.as_deref().context("缺少 replay 腳本")?,
                args.interval_ms,
                args.dry_run,
                args.days_ago,
                args.start,
            ),
        },
        Command::Query { text, limit, json } => ops::query::run(
            &data_dir,
            &text.join(" "),
            limit,
            json,
            config()?.privacy.query_log,
        ),
        Command::Facts {
            kind,
            search,
            limit,
            json,
        } => ops::facts::run(&data_dir, kind.as_deref(), search.as_deref(), limit, json),
        Command::Queries {
            limit,
            empty,
            marked,
            json,
        } => ops::queries::run(&data_dir, limit, empty, marked, json),
        Command::Mark { id, undo } => {
            ops::mark::run(&data_dir, id, !undo, config()?.privacy.query_log)
        }
        Command::Stats { json } => ops::stats::run(&data_dir, &config()?, json),
        Command::Prune { dry_run } => ops::prune::run(&data_dir, &config()?, dry_run),
        Command::Export { to, with_frames } => ops::export::run(&data_dir, &to, with_frames),
        Command::Forget { last, yes } => ops::forget::run(&data_dir, &last, yes),
        Command::Do {
            task,
            apps,
            allow,
            minutes,
            steps,
            dry_run,
        } => ops::act::run(
            &data_dir,
            &ops::act::Options {
                task,
                apps,
                allow,
                minutes,
                steps,
                dry_run,
            },
        ),
        Command::Pause => ops::pause::run(&data_dir, true),
        Command::Resume => ops::pause::run(&data_dir, false),
        Command::Stop => ops::stop::run(&data_dir),
        Command::Interpret {
            dry_run,
            last,
            limit,
            at,
        } => ops::interpret::run(&data_dir, &config()?, dry_run, &last, limit, at),
        Command::Brain { action } => match action {
            BrainAction::Log { limit } => ops::brain::log(&data_dir, limit),
        },
        Command::Review {
            dry_run,
            last,
            eod,
            force,
        } => ops::review::run(&data_dir, &config()?, dry_run, &last, eod, force),
        Command::Commitments {
            kill,
            other,
            note,
            json,
        } => ops::commitments::run(&data_dir, kill, other, note.as_deref(), json),
        Command::Speak { dry_run, log, day } => {
            if !dry_run && !log {
                anyhow::bail!("sister speak 需要 --dry-run 或 --log");
            }
            ops::speak::run(&data_dir, &config()?, dry_run, day.as_deref())
        }
        Command::Consent {
            grant,
            revoke,
            json,
        } => ops::consent::run(&data_dir, &config()?, &grant, &revoke, json),
        Command::Doctor => ops::doctor::run(&data_dir, config(), cli.config.clone()),
        Command::Bench { rounds } => ops::bench::run(rounds),
    }
}

fn load_config(explicit: Option<&std::path::Path>) -> Result<Config> {
    match explicit {
        // 他親手打了這個路徑，那它不存在就是打錯了，不是「請用預設值」。
        //
        // `Config::load` 對不存在的檔案回傳預設值——那對**預設位置**是對的
        // （沒有設定檔本來就跑預設）。但照搬到這裡，`sister --config
        // ~/sister.toml doctor` 會安安靜靜地印出一整頁預設值，而他正是打開
        // doctor 來確認那份設定有沒有生效的。最糟的是排除規則那三行：他看到
        // 「排除的 app 9 條規則」就走了，那 9 條是內建的，他自己寫的那 30 條
        // 一條都沒載進來。
        //
        // 錄製迴圈那邊早就防過同一件事（設定檔中途不見了不算請用預設值），
        // 這裡是它的開機版本。
        Some(p) if !p.exists() => {
            anyhow::bail!("找不到設定檔：{}", p.display())
        }
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

    #[test]
    fn replay_keeps_the_old_script_syntax_and_adds_real_export_import_subcommands() {
        let old = Cli::try_parse_from(["sister", "replay", "scenarios/bill-lookup.json"])
            .expect("舊 quickstart 不能壞");
        let Command::Replay(old) = old.command else {
            panic!("parsed the wrong command")
        };
        assert!(old.action.is_none());
        assert_eq!(
            old.scenario.as_deref(),
            Some(std::path::Path::new("scenarios/bill-lookup.json"))
        );

        let export = Cli::try_parse_from([
            "sister",
            "replay",
            "export",
            "--last",
            "2d",
            "--to",
            "day.sister-replay-draft.json",
            "--questions-to",
            "day.sister-questions-draft.json",
        ])
        .expect("export subcommand");
        let Command::Replay(export) = export.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            export.action,
            Some(ReplayAction::Export {
                ref last,
                ref questions_to,
                ..
            }) if last == "2d"
                && questions_to.as_deref()
                    == Some(std::path::Path::new("day.sister-questions-draft.json"))
        ));

        let import = Cli::try_parse_from([
            "sister",
            "replay",
            "import",
            "day.sister-replay-draft.json",
            "--dry-run",
        ])
        .expect("import subcommand");
        let Command::Replay(import) = import.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            import.action,
            Some(ReplayAction::Import { dry_run: true, .. })
        ));

        let evaluate = Cli::try_parse_from([
            "sister",
            "replay",
            "evaluate",
            "day.corpus.json",
            "recall.questions.json",
            "--k",
            "10",
            "--runs",
            "2",
        ])
        .expect("evaluate subcommand");
        let Command::Replay(evaluate) = evaluate.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            evaluate.action,
            Some(ReplayAction::Evaluate { k: 10, runs: 2, .. })
        ));

        let annotate = Cli::try_parse_from([
            "sister",
            "replay",
            "questions",
            "annotate",
            "day.corpus.json",
            "draft.questions.json",
            "--to",
            "labeled.questions.json",
            "--all",
        ])
        .expect("interactive annotation subcommand");
        let Command::Replay(annotate) = annotate.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            annotate.action,
            Some(ReplayAction::Questions {
                action: ReplayQuestionAction::Annotate {
                    k: 5,
                    all: true,
                    ..
                }
            })
        ));

        let review = Cli::try_parse_from([
            "sister",
            "replay",
            "questions",
            "review",
            "day.corpus.json",
            "labeled.questions.json",
            "--to",
            "reviewed.questions.json",
            "--confirm-private-text-reviewed",
        ])
        .expect("review subcommand");
        let Command::Replay(review) = review.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            review.action,
            Some(ReplayAction::Questions {
                action: ReplayQuestionAction::Review {
                    confirm_private_text_reviewed: true,
                    ..
                }
            })
        ));

        assert!(
            Cli::try_parse_from([
                "sister",
                "replay",
                "questions",
                "annotate",
                "day.corpus.json",
                "draft.questions.json",
                "--to",
                "out.json",
                "--k",
                "0",
            ])
            .is_err()
        );

        let moments_draft = Cli::try_parse_from([
            "sister",
            "replay",
            "moments",
            "draft",
            "day.corpus.json",
            "--to",
            "draft.moments.json",
        ])
        .expect("moments draft subcommand");
        let Command::Replay(moments_draft) = moments_draft.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            moments_draft.action,
            Some(ReplayAction::Moments {
                action: ReplayMomentAction::Draft { .. }
            })
        ));

        let moments_status = Cli::try_parse_from([
            "sister",
            "replay",
            "moments",
            "status",
            "day.corpus.json",
            "draft.moments.json",
        ])
        .expect("moments status subcommand");
        let Command::Replay(moments_status) = moments_status.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            moments_status.action,
            Some(ReplayAction::Moments {
                action: ReplayMomentAction::Status { json: false, .. }
            })
        ));

        let moments_status_json = Cli::try_parse_from([
            "sister",
            "replay",
            "moments",
            "status",
            "day.corpus.json",
            "draft.moments.json",
            "--json",
        ])
        .expect("moments status --json");
        let Command::Replay(moments_status_json) = moments_status_json.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            moments_status_json.action,
            Some(ReplayAction::Moments {
                action: ReplayMomentAction::Status { json: true, .. }
            })
        ));

        let moments_annotate = Cli::try_parse_from([
            "sister",
            "replay",
            "moments",
            "annotate",
            "day.corpus.json",
            "draft.moments.json",
            "--to",
            "labeled.moments.json",
            "--all",
        ])
        .expect("moments annotate subcommand");
        let Command::Replay(moments_annotate) = moments_annotate.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            moments_annotate.action,
            Some(ReplayAction::Moments {
                action: ReplayMomentAction::Annotate { all: true, .. }
            })
        ));

        let moments_review = Cli::try_parse_from([
            "sister",
            "replay",
            "moments",
            "review",
            "day.corpus.json",
            "labeled.moments.json",
            "--to",
            "reviewed.moments.json",
            "--confirm-private-text-reviewed",
        ])
        .expect("moments review subcommand");
        let Command::Replay(moments_review) = moments_review.command else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            moments_review.action,
            Some(ReplayAction::Moments {
                action: ReplayMomentAction::Review {
                    confirm_private_text_reviewed: true,
                    ..
                }
            })
        ));

        let moments_review_without_flag = Cli::try_parse_from([
            "sister",
            "replay",
            "moments",
            "review",
            "day.corpus.json",
            "labeled.moments.json",
            "--to",
            "reviewed.moments.json",
        ])
        .expect("flag is opt-in; refusal happens later");
        let Command::Replay(moments_review_without_flag) = moments_review_without_flag.command
        else {
            panic!("parsed the wrong command")
        };
        assert!(matches!(
            moments_review_without_flag.action,
            Some(ReplayAction::Moments {
                action: ReplayMomentAction::Review {
                    confirm_private_text_reviewed: false,
                    ..
                }
            })
        ));
    }
}
