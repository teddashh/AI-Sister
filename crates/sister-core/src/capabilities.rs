//! 上一次開始記錄的時候，這台機器做得到什麼——兩個行程之間的第三條線。
//!
//! [`crate::heartbeat`] 回答「現在有沒有人在錄」，[`crate::pause`] 回答「她有
//! 沒有被叫停」。這裡回答第三個問題：**她做得到的事，和你以為她做得到的事，
//! 是不是同一件**。
//!
//! 為什麼要寫成檔案：能力探測（UIA、輸入 hook）只有 `sister-capture` 的
//! Windows 那半邊做得到，而**設定頁在另一個行程裡**——那個行程沒有、也不該有
//! 那些相依（多一份 UIA、多一次 COM 初始化，只為了畫一行警告）。於是唯一
//! 知道「UIA 中途失效，privacy gate 從那刻起停止讀內容」的人，是把它印進
//! `record.log` 的 recorder，而那個檔案沒有人會開。
//!
//! **存原始能力，不存結論。** 上一場錄製開始的時候使用者可能一條網址規則都
//! 還沒寫——那時候算出來的結論是「沒問題」，而他正是**現在**才在設定頁上打
//! 第一條。結論要拿去和眼前這一刻的規則清單重算，才不會讓那一頁繼續沉默。
//!
//! 不放進資料庫：`prune` 和 `forget` 都會清 `system_events`，而這不是一段
//! 記憶，是一台機器的事實。被 `forget` 帶走的話，設定頁會安靜地變回「看起來
//! 沒問題」——而那正是這整個模組要擋掉的那一種安靜。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::PrivacyConfig;
use crate::model::Millis;

/// 檔名。放在 data dir 裡，跟 `sister.db`、`recording.beat` 同一層。
const FILE: &str = "capabilities.json";

/// 一項能力的三種真實狀態。
///
/// `Unknown` 不是 `Unavailable` 的溫和寫法：前者是這份報告沒量到，
/// 後者是已經量過且確定做不到。開始加 macOS／Linux backend 之後，
/// 建置過、還沒有探測器的平台會大量用到第一種；把它壓成 `false`
/// 會讓 doctor 把「我不知道」畫成一個已驗證的 ✗。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    Unavailable,
    #[default]
    Unknown,
}

impl CapabilityState {
    /// 把一次**真的做過**的布林探測轉成能力狀態。
    ///
    /// 名字裡帶 `measured`，避免呼叫端把 `unwrap_or(false)` 之類的假值
    /// 塞進來後還看起來像一次真探測。
    pub const fn from_measured(available: bool) -> Self {
        if available {
            Self::Available
        } else {
            Self::Unavailable
        }
    }
}

/// 舊報告的正向布林（`true` = 有能力）與新的三態都能讀。
#[derive(Deserialize)]
#[serde(untagged)]
enum CapabilityWire {
    State(CapabilityState),
    LegacyBool(bool),
}

fn deserialize_capability<'de, D>(deserializer: D) -> std::result::Result<CapabilityState, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match CapabilityWire::deserialize(deserializer)? {
        CapabilityWire::State(state) => state,
        CapabilityWire::LegacyBool(available) => CapabilityState::from_measured(available),
    })
}

/// 舊欄位存的是 `input_hook_failed`，極性和新欄位相反。
///
/// `true` 能夠證明當時裝過且失敗，所以是 `Unavailable`。`false` 只能
/// 證明「沒有留下失敗旗標」；舊 schema 沒有存「試過而且成功」的正向
/// 證據，升級時保守讀成 `Unknown`，不替舊檔案補寫一次沒量過的成功。
fn deserialize_input_capability<'de, D>(
    deserializer: D,
) -> std::result::Result<CapabilityState, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match CapabilityWire::deserialize(deserializer)? {
        CapabilityWire::State(state) => state,
        CapabilityWire::LegacyBool(true) => CapabilityState::Unavailable,
        CapabilityWire::LegacyBool(false) => CapabilityState::Unknown,
    })
}

/// UIA 那一路在**這一刻**的狀態。
///
/// 單獨一個型別，因為它和 [`Report`] 其他欄位的時間性不一樣：那些是開機探測
/// 出來的，一整場不會變；這兩個是**錄製途中才會掉**的。UIA 連續卡住三次就
/// 永久投降（見 `sister_capture::windows::uia`）；recorder 會把 privacy context
/// 當 Unknown，在任何內容來源前 fail closed。這個狀態仍要對外說清楚，否則一段
/// 「安全地什麼都沒記」會和正常錄製長得一樣。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlCapture {
    /// 卡住太多次，已經**永久**放棄讀網址。沒有復原。
    #[serde(default)]
    pub gave_up: bool,
    /// 連續問不出焦點是否在敏感欄；只供能力報告，不會關掉 fail-closed gate。
    #[serde(default)]
    pub password_check_broken: bool,
}

/// 上一次 `sister record` 起來的時候探測到的東西，加上這一場路上發生的事。
///
/// 欄位刻意都是原始事實（做得到／做不到、發生了幾次），沒有一個是判斷。判斷
/// 在 [`Self::broken_privacy_rules`] 裡，而它每次都拿現在的設定重算。
///
/// 新欄位一律 `#[serde(default)]`：舊版寫下的那份報告要照樣讀得出來。少了這個
/// 標註，一顆升級上來的機器會讓 `read` 回 `None`，於是設定頁從「有話要說」
/// 變成「還不知道」——把一則真的警告換成一句沒有內容的話。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    /// **這份報告描述的是哪一刻**，不是「開機那一刻」。
    ///
    /// recorder 錄製途中會反覆蓋掉這個檔（見 [`write`]），因為底下那幾個欄位
    /// 會在路上變。給人看的：一份三個禮拜前的報告和五分鐘前那份，可信度不同，
    /// 而讀的人有權自己判斷——不要替他決定「夠新了」。
    pub at: Millis,
    /// UIA **建得起來**（開機探測）。
    ///
    /// 注意這只是「COM 物件造得出來」，不是「讀得到位址列」。那兩件事之間
    /// 差著一整台機器，而它們的差別由 [`Self::browser_ticks`] /
    /// [`Self::url_reads`] 回答——`Unavailable` 的時候 `excluded_urls`
    /// 整組規則鐵定不生效，`Available` 的時候只是
    /// **還不確定**。
    #[serde(default, deserialize_with = "deserialize_capability")]
    pub url: CapabilityState,
    /// 輸入 hook 這一場到底裝不裝得上。
    ///
    /// 不再存「有沒有失敗」：那個問法的 `false` 同時包了成功和沒試過。
    /// 新報告存完整三態；升級讀舊 `input_hook_failed` 時，只有 `true`
    /// 能安全地換成 `Unavailable`，舊 `false` 保守留在 `Unknown`。
    #[serde(
        default,
        alias = "input_hook_failed",
        deserialize_with = "deserialize_input_capability"
    )]
    pub input_hook: CapabilityState,
    /// 錄製途中掉掉的那兩樣。見 [`UrlCapture`]。
    #[serde(default)]
    pub url_capture: UrlCapture,
    /// 這一場裡，焦點停在**瀏覽器視窗**上的拍數。
    ///
    /// 它是 [`Self::url_reads`] 的分母，也是一道證據門檻：一個今天還沒開過
    /// 瀏覽器的人，「一個網址都沒讀到」什麼都證明不了。
    #[serde(default)]
    pub browser_ticks: u64,
    /// 其中真的拿到網址的拍數。
    ///
    /// `url = Available` 而這個是 0，是**這一整條線最常見的壞法**：UIA 造得出來、
    /// `doctor` 全綠、設定頁一片乾淨，而位址列從頭到尾沒讀到過一次（瀏覽器
    /// 用系統管理員身分跑、無障礙介面沒開、UIA 樹換了形狀）。那台機器把使用者
    /// 的網銀錄了一整天，而他寫的每一條 `excluded_urls` 一次都沒擋過東西。
    #[serde(default)]
    pub url_reads: u64,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// 蓋一份新的。recorder 開機寫一次，之後**錄製途中每隔一段時間再蓋一次**。
///
/// 「只在開機寫一次」是這個檔案原本最大的問題：UIA 會在半路上永久投降，
/// recorder 從那之後會 fail closed；設定頁若仍拿著開機時的「一切正常」，使用者
/// 只會看到記憶無故停止，卻不知道是哪個安全前提壞了。
///
/// 和 [`crate::heartbeat::beat`] 同一個作法：先寫暫存檔再 rename。讀的人有
/// 機會讀到寫到一半的 JSON——那不會壞掉（解析失敗就當成沒有報告），但會讓
/// 設定頁無緣無故閃一下。
pub fn write(data_dir: &Path, report: &Report) -> Result<()> {
    let path = path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(report).context("serialize capabilities")?;
    std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

/// 上一份報告。`None` = 檔案不在、讀不到、或內容不是我們寫的那個形狀。
///
/// 三種都回 `None` 是刻意的：對讀的人來說它們是同一句話——「還不知道」。而
/// 「還不知道」和「沒問題」在畫面上必須長得不一樣，那是呼叫端的責任。
pub fn read(data_dir: &Path) -> Option<Report> {
    let body = std::fs::read_to_string(path(data_dir)).ok()?;
    serde_json::from_str(&body).ok()
}

/// 「一個網址都沒讀到」要幾拍瀏覽器才算數。
///
/// 沒有這道門檻的話，一個今天還沒開過瀏覽器的人會拿到一則說他的網銀規則全
/// 失效的警告——而那則警告是假的，然後整區警告就被學會忽略了。和
/// `db::signal_audit` 的 `ENOUGH_TO_BE_SURE` 同一條規則：一個檢查一旦從
/// 「整台機器」收窄成「這一場」，就得自己補回那個原本靠量撐著的分母。
///
/// 20 拍：預設節拍下大約是十幾秒到一分多鐘的瀏覽器時間。夠短到「他真的在用
/// 瀏覽器」的那一天第一個小時內就會講，夠長到不會被切過去看一眼就切走觸發。
const ENOUGH_BROWSER_TICKS: u64 = 20;

/// 這一則話該掛在設定頁的**哪一格**底下。
///
/// 一則警告掛錯地方等於沒講。「輸入 hook 裝不上」出現在「排除的網址」那一格
/// 底下的時候，讀起來像是在講他那幾條網址規則出了什麼事——而它講的是另一件
/// 事（節奏訊號這一場會是空的），屬於另一格。
///
/// 為什麼是一個型別而不是一串字：分格的判斷只要沒有跟著句子一起送出去，
/// 設定頁就只能自己猜，而它唯一猜得動的辦法是 `message.includes("hook")`。
/// 那一天句子改一個字，那一則就靜靜地掛回錯的地方，而沒有任何測試會紅。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum About {
    /// 整個錄製的 UIA/privacy context；不是只屬於某一條 URL 規則。
    PrivacyCapture,
    /// 「排除的網址」那一格：`excluded_urls`，以及 UIA privacy context。
    UrlRules,
    /// 「輸入節奏」那一格。
    InputHook,
}

/// 一條失效的隱私規則：給人讀的那一句，加上它該掛在哪一格。
///
/// 終端機沒有「格」，所以 `doctor` 和 `record` 只印 [`Self::message`]
/// （見 `ops.rs`）——`about` 是設定頁的需要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Broken {
    pub about: About,
    pub message: String,
}

impl Report {
    /// 這份報告裡有沒有**只有錄製途中才問得到**的東西。
    ///
    /// 分成兩半是因為這個型別裝著兩種時間性完全不同的事實：
    ///
    /// * `url`、`input_hook` 是**探測**——任何人任何時候重問一次都拿得
    ///   到同一個答案。
    /// * 這裡數的這四個是**歷史**。`gave_up` 記的是「UIA 在上一場的某一刻卡住
    ///   太多次，從那之後 privacy context 不可用、錄製內容 fail closed」；那件事發生在一個已經結束的
    ///   行程裡，用一份全新的 UIA 去問**永遠問不出來**——新的那份是好的。
    ///
    /// 而 `doctor` 手上只有探測那一半。它拿 `Caps::current()` 蓋一次檔，等於
    /// 把上面那四個全部歸零：一則「錄製從昨天下午三點開始因 privacy gate 停住」的
    /// 警告，被換成一份時戳是五分鐘前、全部乾淨的報告——而 [`Self::at`] 那段
    /// 註解說得很清楚，時戳存在的意義就是讓讀的人拿它判斷可信度。愈新的愈可
    /// 信，於是那份假的比真的更有說服力。
    ///
    /// 使用者的動線正好是最壞的那條：覺得怪怪的 → 跑一次 `doctor` → 打開設定
    /// 頁。他親手刪掉了自己要找的那份證據。
    ///
    /// 所以**這份報告屬於錄製的那一場**，`doctor` 只在沒有東西可以弄丟的時候
    /// 才去蓋它。這一支就是「有沒有東西可以弄丟」。
    pub fn has_session_evidence(&self) -> bool {
        self.url_capture != UrlCapture::default() || self.browser_ticks > 0 || self.url_reads > 0
    }

    /// 因為能力缺席而**失效的隱私規則**，拿現在這一份設定重算。
    ///
    /// 和一般的功能缺口分開講：使用者可以接受「還不會 OCR」，但他必須知道
    /// 「你設定的網銀排除規則現在一條都不會生效」。前者是少做了一件事，
    /// 後者是他以為關上的門其實開著。
    ///
    /// **這裡是那幾句話唯一的出處。** 設定頁和 `record` 收工時印的是同一份
    /// ——同一個判斷寫兩個地方，遲早會變成兩句不一樣的話，而使用者會相信
    /// 比較好聽的那一句。
    pub fn broken_privacy_rules(&self, privacy: &PrivacyConfig) -> Vec<Broken> {
        let rules = privacy.excluded_urls.len();
        let mut out = Vec::new();
        let mut push = |about, message: String| out.push(Broken { about, message });
        // 三種能力缺口是一條路上的三個點，所以 `else if`：全部印出來只會稀釋掉
        // 真正要看的那一則。由重到輕——路上死掉最急，因為它有一個「從那之後」。
        if self.url_capture.gave_up {
            let rules_note = if rules == 0 {
                String::new()
            } else {
                format!("；{rules} 條 excluded_urls 也無法評估")
            };
            push(
                About::PrivacyCapture,
                format!(
                    "UIA 在錄製途中卡住太多次已放棄：**從那一刻起 privacy context \
                     無法確認，recorder 會在剪貼簿與螢幕之前停止讀內容**{rules_note}。\
                     這一場不會自行復原，\
                     重開 sister record 才會再試一次"
                ),
            );
        } else if self.url_capture.password_check_broken {
            push(
                About::PrivacyCapture,
                "這一場連續問不出焦點是不是在敏感欄：recorder 仍保持 fail closed，\
                 每拍都在剪貼簿與螢幕之前停下，直到 UIA 再次明確回答"
                    .into(),
            );
        } else if self.url == CapabilityState::Unavailable {
            let rules_note = if rules == 0 {
                String::new()
            } else {
                format!("；{rules} 條 excluded_urls 也會無法評估")
            };
            push(
                About::PrivacyCapture,
                format!(
                    "開機探測拿不到 UIA；錄製端若同樣無法確認 privacy context，\
                     會在剪貼簿與螢幕之前停下，不把 Unknown 當成安全{rules_note}"
                ),
            );
        } else if self.url == CapabilityState::Available
            && rules > 0
            && self.url_reads == 0
            && self.browser_ticks >= ENOUGH_BROWSER_TICKS
        {
            // `url = Available` 只代表 COM 物件造得出來。這一條是那句話和「讀得到
            // 位址列」之間的距離，而它整個是安靜的：doctor 全綠、摘要全綠。
            push(
                About::UrlRules,
                format!(
                    "UIA 起得來，但這一場在瀏覽器視窗上停了 {} 拍、\
                     **一個網址都沒讀到**：你那 {rules} 條 excluded_urls 到現在\
                     一次都沒擋過東西，瀏覽器畫面只靠視窗標題規則過濾",
                    self.browser_ticks
                ),
            );
        }
        if self.input_hook == CapabilityState::Unavailable {
            push(
                About::InputHook,
                "輸入 hook 裝不上：節奏訊號這個 session 會是空的".into(),
            );
        }
        out
    }

    /// 那幾條網址規則**驗過了沒有**——和 [`Self::broken_privacy_rules`] 是不同
    /// 的問題，所以是不同的回傳值。
    ///
    /// 為什麼要分開：上面那條 `else if` 鏈的最後一格要求
    /// `browser_ticks >= ENOUGH_BROWSER_TICKS`，門檻沒到就**什麼都不 push**。
    /// 那道門檻是對的（一個今天還沒開過瀏覽器的人，「一個網址都沒讀到」什麼都
    /// 證明不了），錯的是「門檻沒到」被印成**沒有判決**，而設定頁把沒有判決畫
    /// 成一片乾淨——而那一頁自己寫著「空白在這一格就是『都生效』」。
    ///
    /// 於是三台完全不同的機器印同一片空白：真的讀得到網址的、UIA 起得來但一次
    /// 都沒讀到過的（[`Self::url_reads`] 那段註解叫它「這一整條線最常見的壞
    /// 法」）、以及報告是十一個月前寫的。第二台那個人正在那一頁上打
    /// `*.bank.com.tw*`，然後去網銀。
    ///
    /// 和 `capture_off` 不塞進 `broken` 同一個理由：那個清單講的是「你以為關上
    /// 的門其實開著」，而這裡講的是「我還不知道那扇門關了沒」——方向是第三種，
    /// 混進去只會讓那幾句話輕重不分。
    pub fn url_rules_verdict(&self, privacy: &PrivacyConfig) -> UrlRules {
        let rules = privacy.excluded_urls.len();
        if rules == 0 {
            return UrlRules::None;
        }
        // 上面那三格任何一格有話講，這裡就閉嘴：同一件事講兩次，讀的人會以為
        // 是兩件事。
        if self.url_capture.gave_up || self.url == CapabilityState::Unavailable {
            return UrlRules::Broken;
        }
        if self.url == CapabilityState::Unknown {
            return UrlRules::Unknown;
        }
        if self.url_reads > 0 {
            return UrlRules::Working {
                reads: self.url_reads,
            };
        }
        if self.browser_ticks >= ENOUGH_BROWSER_TICKS {
            // 這一格 `broken_privacy_rules` 已經講了（「停了 N 拍、一個網址都
            // 沒讀到」）。
            return UrlRules::Broken;
        }
        UrlRules::Unproven {
            ticks: self.browser_ticks,
            need: ENOUGH_BROWSER_TICKS,
        }
    }
}

/// 「你那幾條 excluded_urls 到底有沒有在擋東西」的答案。
///
/// 核心事實仍是三態：已驗證有效、已驗證失效、還不知道。
/// `None` 是「沒有規則，所以沒有問題要回答」；`Broken` 是「失效原因已由
/// [`Report::broken_privacy_rules`] 講了」的顯示控制態。它們不能被拿來合併
/// 核心三態，尤其**「驗過了，有效」和「還沒驗過」不可以長得一樣**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UrlRules {
    /// 一條規則都沒有——這一格沒有問題要回答。
    None,
    /// 這份報告沒有量到 URL 能力。不是已經確定失效。
    Unknown,
    /// 這一場真的讀到過網址，所以那幾條規則有機會生效。
    Working { reads: u64 },
    /// **還沒有證據**：瀏覽器用得還不夠多，問不出來。不是「沒問題」。
    Unproven { ticks: u64, need: u64 },
    /// [`Report::broken_privacy_rules`] 那邊已經有話講了，這裡不要再講一次。
    Broken,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("sister-caps-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("mkdir");
            Self(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn able() -> Report {
        Report {
            at: 1_000,
            url: CapabilityState::Available,
            input_hook: CapabilityState::Available,
            url_capture: UrlCapture::default(),
            // 讀得到，而且證明過：從這裡出發，只改要測的那一項。
            browser_ticks: 500,
            url_reads: 480,
        }
    }

    fn with_rules(n: usize) -> PrivacyConfig {
        PrivacyConfig {
            excluded_urls: (0..n).map(|i| format!("*bank{i}*")).collect(),
            ..Default::default()
        }
    }

    /// 這一條是這個 enum 存在的全部理由：**三台機器，三種答案。**
    ///
    /// 以前 `broken_privacy_rules` 對第二台什麼都不 push，而設定頁自己寫著
    /// 「空白在這一格就是『都生效』」——於是「驗過了，有效」和「還沒驗過」
    /// 在那一頁上逐像素相同，而第二台正是 `url_reads` 那段註解叫做「這一整條
    /// 線最常見的壞法」的那一台。
    #[test]
    fn proven_and_never_proven_do_not_look_the_same() {
        let rules = with_rules(3);
        let proven = able();
        let never = Report {
            url_reads: 0,
            browser_ticks: 2,
            ..able()
        };
        let blind = Report {
            url: CapabilityState::Unavailable,
            ..able()
        };

        assert_eq!(
            proven.url_rules_verdict(&rules),
            UrlRules::Working { reads: 480 }
        );
        assert_eq!(
            never.url_rules_verdict(&rules),
            UrlRules::Unproven {
                ticks: 2,
                need: ENOUGH_BROWSER_TICKS
            }
        );
        assert_eq!(blind.url_rules_verdict(&rules), UrlRules::Broken);

        // 三個都不一樣才算數——少了這一句，一支「永遠回 Broken」的實作也會過
        // 上面那三條裡的一條。
        assert_ne!(
            proven.url_rules_verdict(&rules),
            never.url_rules_verdict(&rules)
        );
        assert_ne!(
            never.url_rules_verdict(&rules),
            blind.url_rules_verdict(&rules)
        );

        // 而「還沒驗過」那一台，`broken` 仍然是空的：那個清單講的是「你以為
        // 關上的門其實開著」，這件事是第三種方向。兩邊要各自成立。
        assert!(never.broken_privacy_rules(&rules).is_empty());
    }

    /// 「沒量到」不是一種可用，也不是一種不可用。這是三態模型
    /// 最容易在消費端被壓回 bool 的那一面，所以同時釘住判決與警告。
    #[test]
    fn an_unmeasured_capability_is_neither_working_nor_broken() {
        let rules = with_rules(3);
        let unknown = Report {
            url: CapabilityState::Unknown,
            input_hook: CapabilityState::Unknown,
            // 即使舊報告留下了夠多瀏覽器拍數，沒量到 URL 能力就
            // 不能用那個分母反過來證明它已失效。
            browser_ticks: ENOUGH_BROWSER_TICKS + 100,
            url_reads: 0,
            ..able()
        };

        assert_eq!(unknown.url_rules_verdict(&rules), UrlRules::Unknown);
        assert!(
            unknown.broken_privacy_rules(&rules).is_empty(),
            "沒量到的 URL 或 hook 不能被描述成已確定失敗"
        );
        assert_ne!(unknown.url_rules_verdict(&rules), UrlRules::Broken);
        assert_ne!(
            unknown.url_rules_verdict(&rules),
            UrlRules::Working { reads: 1 }
        );
    }

    /// 門檻兩側各一次。差一拍就從「還不知道」翻成一則指控，所以那一刀要落在
    /// 寫下來的那個數字上，不是它附近。
    #[test]
    fn the_threshold_is_where_it_says_it_is() {
        let rules = with_rules(1);
        let at = |ticks| {
            Report {
                url_reads: 0,
                browser_ticks: ticks,
                ..able()
            }
            .url_rules_verdict(&rules)
        };
        assert_eq!(
            at(ENOUGH_BROWSER_TICKS - 1),
            UrlRules::Unproven {
                ticks: ENOUGH_BROWSER_TICKS - 1,
                need: ENOUGH_BROWSER_TICKS
            }
        );
        // 到了門檻，話由 `broken_privacy_rules` 那一則講（「停了 N 拍、一個網址
        // 都沒讀到」），這裡就閉嘴——同一件事講兩次，讀的人會以為是兩件事。
        assert_eq!(at(ENOUGH_BROWSER_TICKS), UrlRules::Broken);
        assert!(
            !Report {
                url_reads: 0,
                browser_ticks: ENOUGH_BROWSER_TICKS,
                ..able()
            }
            .broken_privacy_rules(&rules)
            .is_empty(),
            "門檻到了就一定要有人講話，不能兩邊都閉嘴"
        );
    }

    /// 一條規則都沒有的時候不要講話。這一格是「你那幾條規則有沒有在擋東西」，
    /// 沒有規則就沒有那個問題——而一則答非所問的訊息會讓整區被學會忽略。
    #[test]
    fn with_no_rules_there_is_no_question_to_answer() {
        // `with_rules(0)` 而不是 `PrivacyConfig::default()`——預設值本來就帶著
        // 幾條規則（網銀、登入頁），拿它當「沒有規則」會測到別的東西。
        let none = with_rules(0);
        for r in [
            able(),
            Report {
                url_reads: 0,
                browser_ticks: 1,
                ..able()
            },
            Report {
                url: CapabilityState::Unavailable,
                ..able()
            },
        ] {
            assert_eq!(r.url_rules_verdict(&none), UrlRules::None);
        }
    }

    /// 半路投降的那一台歸 `Broken`，不歸「還沒驗過」——它的 `url_reads` 可能是
    /// 0（投降得早），而那個 0 的意思和「瀏覽器用得不夠多」完全相反：一個是
    /// 問不出來，一個是**已經確定壞了**。
    #[test]
    fn giving_up_midway_is_not_the_same_as_not_knowing_yet() {
        let rules = with_rules(2);
        let gave_up = Report {
            url_reads: 0,
            browser_ticks: 1,
            url_capture: UrlCapture {
                gave_up: true,
                ..UrlCapture::default()
            },
            ..able()
        };
        assert_eq!(gave_up.url_rules_verdict(&rules), UrlRules::Broken);
    }

    #[test]
    fn a_rule_written_after_the_last_recording_still_gets_judged() {
        // 這一條是整個模組存在的理由。上一場錄製開始的時候他一條網址規則都
        // 沒有，所以那時候算出來的結論是「沒問題」——而他正是**現在**才在設定
        // 頁上打第一條。存結論的話，那一頁會拿著一份三禮拜前的「沒問題」，
        // 對著一條剛剛才寫下、而且不會生效的規則，什麼都不說。
        let blind = Report {
            url: CapabilityState::Unavailable,
            ..able()
        };
        let without_rules = blind.broken_privacy_rules(&with_rules(0));
        assert_eq!(
            without_rules.len(),
            1,
            "開機 UIA probe 缺席仍是整體 privacy capture 警告，不依賴 URL 規則"
        );
        assert_eq!(without_rules[0].about, About::PrivacyCapture);
        assert!(
            !without_rules[0].message.contains("excluded_urls"),
            "沒有規則時不能假稱有 URL 規則失效：{}",
            without_rules[0].message
        );
        let said = blind.broken_privacy_rules(&with_rules(1));
        assert_eq!(said.len(), 1, "剛打的這一條要被判出來：{said:?}");
        assert!(
            said[0].message.contains("1 條"),
            "要數得出幾條：{}",
            said[0].message
        );
    }

    #[test]
    fn a_machine_that_can_read_urls_says_nothing() {
        assert!(able().broken_privacy_rules(&with_rules(1)).is_empty());
    }

    /// `url: Available` 只代表 COM 物件造得出來，不代表讀得到位址列。
    ///
    /// 這是這一整條線最常見的壞法，而且從頭到尾是安靜的：UIA 建得起來，
    /// `doctor` 全綠，設定頁一片乾淨，而位址列一次都沒讀到（瀏覽器用系統
    /// 管理員身分跑、無障礙介面沒開、UIA 樹換了形狀）。那台機器把使用者的
    /// 網銀錄了一整天，而他寫的每一條規則一次都沒擋過東西。
    ///
    /// 和 OCR 那一條（「讀字那一段是斷的」）是同一個形狀：引擎起得來，
    /// 一個字都沒讀出來，而所有畫面都說一切正常。
    #[test]
    fn a_uia_that_builds_but_never_reads_a_url_is_not_a_working_url_rule() {
        let blind = Report {
            browser_ticks: 400,
            url_reads: 0,
            ..able()
        };
        let said = blind.broken_privacy_rules(&with_rules(16));
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].message.contains("一個網址都沒讀到"),
            "{}",
            said[0].message
        );
        assert!(
            said[0].message.contains("16 條"),
            "要數得出幾條：{}",
            said[0].message
        );
    }

    /// 但「他今天還沒開過瀏覽器」不算證據。
    ///
    /// 這道門檻和 `db::signal_audit` 的 `ENOUGH_TO_BE_SURE` 是同一條規則：
    /// 一個檢查一旦從「整台機器」收窄成「這一場」，就得自己補回那個原本
    /// 靠量撐著的分母。沒有它，每一場錄製的頭幾秒都會噴一則假警告，
    /// 然後整區警告就被學會忽略了。
    #[test]
    fn not_having_opened_a_browser_yet_is_not_evidence_of_anything() {
        for ticks in [0, ENOUGH_BROWSER_TICKS - 1] {
            let early = Report {
                browser_ticks: ticks,
                url_reads: 0,
                ..able()
            };
            assert!(
                early.broken_privacy_rules(&with_rules(16)).is_empty(),
                "{ticks} 拍還不夠下判斷"
            );
        }
    }

    /// UIA 半路投降，是這個檔案存在的第二個理由。
    ///
    /// 它是**永久的**（`uia::note_abandoned_thread` 沒有復原路徑），發生時
    /// 不會有錯誤也不會有例外；recorder 會從那一刻起安全停讀內容，但若不報告，
    /// 使用者只會得到一段沒有原因的記憶空洞。
    /// 以前唯一問過這件事的是收工時的一行 `println!`，印進沒有人會開的
    /// `record.log`——而那要等到這一場結束，可能是好幾天以後。
    #[test]
    fn uia_dying_mid_session_outranks_the_boot_time_probe() {
        let died = Report {
            url_capture: UrlCapture {
                gave_up: true,
                ..UrlCapture::default()
            },
            ..able()
        };
        let said = died.broken_privacy_rules(&with_rules(16));
        assert_eq!(said.len(), 1, "只講最急的那一則：{said:?}");
        assert!(
            said[0].message.contains("從那一刻起"),
            "{}",
            said[0].message
        );
        assert!(said[0].message.contains("16 條"), "{}", said[0].message);
        assert!(!died.url_capture.password_check_broken);
        assert!(
            said[0].message.contains("停止讀內容"),
            "{}",
            said[0].message
        );
        assert!(!said[0].message.contains("可能被錄"), "{}", said[0].message);

        // 三種壞法在同一條路上，所以只講一則。全部印出來只會稀釋掉真正
        // 要看的那一則——而這裡最該看的是「有一個從那之後」。
        let died_blind_and_unprobed = Report {
            url: CapabilityState::Unavailable,
            browser_ticks: 400,
            url_reads: 0,
            ..died
        };
        assert_eq!(
            died_blind_and_unprobed.broken_privacy_rules(&with_rules(16)),
            said
        );
    }

    /// 敏感欄狀態持續未知要單獨講，但不能把警告寫成保護已關掉。
    #[test]
    fn persistent_sensitive_field_failure_reports_that_fail_closed_remains_active() {
        let no_shield = Report {
            url_capture: UrlCapture {
                password_check_broken: true,
                ..UrlCapture::default()
            },
            ..able()
        };
        // 一條網址規則都沒寫也要講——這道保護不是他設定出來的，
        // 是產品本來就答應他的。
        let said = no_shield.broken_privacy_rules(&with_rules(0));
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].message.contains("敏感欄"), "{}", said[0].message);
        assert!(
            said[0].message.contains("fail closed"),
            "{}",
            said[0].message
        );
        assert!(
            !said[0].message.contains("停止用它擋"),
            "{}",
            said[0].message
        );
    }

    /// 輸入 hook 那一句不准被歸到「排除的網址」底下。
    ///
    /// 設定頁把這幾則話分兩格掛（`settings.js` 按 `about` 分流）。而分格的
    /// 判斷只要沒有跟著句子一起送出去，那一頁就只剩下猜——`includes("hook")`
    /// 之類的，然後句子改一個字就靜靜地掛回錯的地方。掛錯的那一則讀起來像
    /// 是在說他那幾條網址規則出了事，而它講的是另一件事。
    #[test]
    fn the_input_hook_line_does_not_get_filed_under_the_url_rules() {
        let no_hook = Report {
            input_hook: CapabilityState::Unavailable,
            ..able()
        };
        let said = no_hook.broken_privacy_rules(&with_rules(16));
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(said[0].about, About::InputHook, "{said:?}");

        // 兩件事同時壞的時候要分成兩則、掛兩格——不是併成一段話塞在其中一格。
        let both = Report {
            url: CapabilityState::Unavailable,
            ..no_hook
        };
        let said = both.broken_privacy_rules(&with_rules(16));
        assert_eq!(
            said.iter().map(|b| b.about).collect::<Vec<_>>(),
            vec![About::PrivacyCapture, About::InputHook],
            "{said:?}"
        );
    }

    /// 舊版寫下的報告要照樣讀得出來。
    ///
    /// 少了 `#[serde(default)]`，一顆升級上來的機器會讓 `read` 回 `None`，
    /// 於是設定頁從「有話要說」變成「還不知道」——把一則真的警告換成一句
    /// 沒有內容的話，而使用者什麼都不會察覺。
    #[test]
    fn a_report_written_by_the_previous_version_still_parses() {
        let dir = Tmp::new("old-shape");
        std::fs::write(
            path(&dir.0),
            r#"{"at":1755000000000,"url":true,"input_hook_failed":false}"#,
        )
        .expect("write");
        let back = read(&dir.0).expect("舊格式要讀得出來");
        assert_eq!(back.url, CapabilityState::Available);
        assert_eq!(
            back.input_hook,
            CapabilityState::Unknown,
            "舊 false 沒有記下成功證據，不能自動補成可用"
        );
        assert_eq!(back.browser_ticks, 0, "舊檔案本來就沒有這個證據");
        assert!(!back.url_capture.gave_up);
        // 而「沒有證據」必須是沉默，不是警告。
        assert!(back.broken_privacy_rules(&with_rules(16)).is_empty());
    }

    #[test]
    fn a_legacy_hook_failure_remains_a_known_failure() {
        let dir = Tmp::new("old-hook-failure");
        std::fs::write(
            path(&dir.0),
            r#"{"at":1755000000000,"url":false,"input_hook_failed":true}"#,
        )
        .expect("write");
        let back = read(&dir.0).expect("舊格式要讀得出來");
        assert_eq!(back.url, CapabilityState::Unavailable);
        assert_eq!(back.input_hook, CapabilityState::Unavailable);
        let said = back.broken_privacy_rules(&with_rules(1));
        assert_eq!(
            said.iter().map(|line| line.about).collect::<Vec<_>>(),
            vec![About::PrivacyCapture, About::InputHook]
        );
    }

    #[test]
    fn fields_absent_from_an_old_report_are_unknown_not_false() {
        let dir = Tmp::new("old-missing-capabilities");
        std::fs::write(path(&dir.0), r#"{"at":1755000000000}"#).expect("write");
        let back = read(&dir.0).expect("舊格式要讀得出來");
        assert_eq!(back.url, CapabilityState::Unknown);
        assert_eq!(back.input_hook, CapabilityState::Unknown);
        assert_eq!(back.url_rules_verdict(&with_rules(1)), UrlRules::Unknown);
        assert!(back.broken_privacy_rules(&with_rules(1)).is_empty());
    }

    #[test]
    fn a_report_that_was_never_written_is_not_a_clean_bill_of_health() {
        // `read` 回 `None` 的三種原因（沒錄過、檔案壞了、被誰刪了）在這裡是
        // 同一句話：還不知道。畫面上它必須和「沒問題」長得不一樣——這裡先
        // 釘住 `None` 本身，不要哪天有人「順手」讓它回一份全 true 的預設值。
        let dir = Tmp::new("missing");
        assert!(read(&dir.0).is_none());
        std::fs::write(path(&dir.0), "{ 不是 JSON").expect("write");
        assert!(read(&dir.0).is_none(), "壞掉的檔案也是「還不知道」");
    }

    #[test]
    fn what_the_recorder_wrote_is_what_the_settings_page_reads() {
        let dir = Tmp::new("roundtrip");
        let written = Report {
            at: 1_755_000_000_000,
            url: CapabilityState::Unavailable,
            input_hook: CapabilityState::Unavailable,
            url_capture: UrlCapture {
                gave_up: true,
                password_check_broken: true,
            },
            browser_ticks: 300,
            url_reads: 7,
        };
        write(&dir.0, &written).expect("write");
        let back = read(&dir.0).expect("read back");
        assert_eq!(back.at, written.at);
        assert_eq!(back.url, CapabilityState::Unavailable);
        assert_eq!(back.input_hook, CapabilityState::Unavailable);
        assert_eq!(back.url_capture, written.url_capture);
        assert_eq!((back.browser_ticks, back.url_reads), (300, 7));
        let body = std::fs::read_to_string(path(&dir.0)).expect("read JSON");
        assert!(body.contains(r#""url": "unavailable""#), "{body}");
        assert!(body.contains(r#""input_hook": "unavailable""#), "{body}");
        assert!(
            !body.contains("input_hook_failed"),
            "新報告不能繼續寫模糊的舊欄位：{body}"
        );
        // 暫存檔要收乾淨，不然 data dir 裡會慢慢長出一堆 .json.tmp。
        assert!(!path(&dir.0).with_extension("json.tmp").exists());
    }

    /// 一份只有開機探測的報告，`doctor` 蓋掉它不會弄丟任何東西。
    ///
    /// 這一半要成立，`doctor` 才還能在一台剛裝好的機器上把探測結果餵給設定頁
    /// ——README 的 quickstart 第一句就是「跑一次 doctor」。
    #[test]
    fn a_report_with_only_boot_probes_has_nothing_doctor_cannot_redo() {
        assert!(!Report::default().has_session_evidence());
        // 這兩個是探測，重問一次就有——不算證據。
        assert!(
            !Report {
                at: 1,
                url: CapabilityState::Available,
                input_hook: CapabilityState::Unavailable,
                ..Default::default()
            }
            .has_session_evidence()
        );
    }

    /// 而這四個只有那一場問得到，蓋掉就沒了。
    #[test]
    fn the_four_things_only_that_session_could_have_seen_are_evidence() {
        // 最重的那一則：從投降那一刻起，整個 privacy context fail closed。
        // 用一份全新的 UIA 去問永遠問不出上一場何時停住——新的那份是好的。
        let gave_up = Report {
            url_capture: UrlCapture {
                gave_up: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(gave_up.has_session_evidence());
        assert!(
            !gave_up
                .broken_privacy_rules(&PrivacyConfig::default())
                .is_empty(),
            "投降那一則本身就是 doctor 會蓋掉的東西——它得先講得出來"
        );

        assert!(
            Report {
                url_capture: UrlCapture {
                    password_check_broken: true,
                    ..Default::default()
                },
                ..Default::default()
            }
            .has_session_evidence()
        );
        // 分母和分子都算數。`browser_ticks` 撐著「一個網址都沒讀到」那一則的
        // 證據門檻，歸零之後那一則就再也講不出來了。
        assert!(
            Report {
                browser_ticks: 1,
                ..Default::default()
            }
            .has_session_evidence()
        );
        assert!(
            Report {
                url_reads: 1,
                ..Default::default()
            }
            .has_session_evidence()
        );
    }
}
