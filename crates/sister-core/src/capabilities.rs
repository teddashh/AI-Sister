//! 上一次開始記錄的時候，這台機器做得到什麼——兩個行程之間的第三條線。
//!
//! [`crate::heartbeat`] 回答「現在有沒有人在錄」，[`crate::pause`] 回答「她有
//! 沒有被叫停」。這裡回答第三個問題：**她做得到的事，和你以為她做得到的事，
//! 是不是同一件**。
//!
//! 為什麼要寫成檔案：能力探測（UIA、輸入 hook）只有 `sister-capture` 的
//! Windows 那半邊做得到，而**設定頁在另一個行程裡**——那個行程沒有、也不該有
//! 那些相依（多一份 UIA、多一次 COM 初始化，只為了畫一行警告）。於是唯一
//! 知道「你那 12 條 excluded_urls 一條都不會生效」的人，是把它印進 `record.log`
//! 的 recorder，而那個檔案沒有人會開。
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

/// UIA 那一路在**這一刻**的狀態。
///
/// 單獨一個型別，因為它和 [`Report`] 其他欄位的時間性不一樣：那些是開機探測
/// 出來的，一整場不會變；這兩個是**錄製途中才會掉**的，而且掉了不會有錯誤、
/// 不會有例外——UIA 連續卡住三次就永久投降（見
/// `sister_capture::windows::uia`），從那一刻起 `excluded_urls` 一條都不生效，
/// 網銀跟登入頁開始被錄進去。以前唯一問過這件事的人是收工時的那一行
/// `println!`，印進 `record.log`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlCapture {
    /// 卡住太多次，已經**永久**放棄讀網址。沒有復原。
    #[serde(default)]
    pub gave_up: bool,
    /// 已經停止用「焦點在密碼欄上」擋畫面（連續問不出來）。
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
    /// [`Self::url_reads`] 回答——`false` 的時候 `excluded_urls` 整組規則
    /// 鐵定不生效，`true` 的時候只是**還不確定**。
    pub url: bool,
    /// 輸入 hook **試過而且失敗**。
    ///
    /// 是 `試過失敗` 而不是 `裝好了`：「沒去裝」和「裝失敗」不是同一件事，
    /// 壓成一個布林會產生一則永遠為真的警告，然後整區警告都會被學會忽略。
    /// 這一份是 recorder 寫的，它一定試過——但欄位的語意要留給以後也對。
    pub input_hook_failed: bool,
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
    /// `url = true` 而這個是 0，是**這一整條線最常見的壞法**：UIA 造得出來、
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
/// 「只在開機寫一次」是這個檔案原本最大的問題：UIA 會在半路上永久投降，而
/// 那之後 `excluded_urls` 一條都不生效——設定頁卻拿著一份開機時的「一切正常」
/// 一句話都不說。使用者正是在那一頁上打那些規則的。
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
    /// 「排除的網址」那一格：`excluded_urls`，以及瀏覽器裡的密碼欄遮蔽。
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
    /// * `url`、`input_hook_failed` 是**探測**——任何人任何時候重問一次都拿得
    ///   到同一個答案。
    /// * 這裡數的這四個是**歷史**。`gave_up` 記的是「UIA 在上一場的某一刻卡住
    ///   太多次，從那之後位址列一個字都讀不到」；那件事發生在一個已經結束的
    ///   行程裡，用一份全新的 UIA 去問**永遠問不出來**——新的那份是好的。
    ///
    /// 而 `doctor` 手上只有探測那一半。它拿 `Caps::current()` 蓋一次檔，等於
    /// 把上面那四個全部歸零：一則「你的網銀從昨天下午三點開始被錄進去了」的
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
        // 三種壞法，一條路上的三個點，所以 `else if`：全部印出來只會稀釋掉
        // 真正要看的那一則。由重到輕——路上死掉最急，因為它有一個「從那之後」。
        if self.url_capture.gave_up {
            // 密碼欄那道保護跟著一起沒了，而且不會反映在
            // `password_check_broken` 上——那個旗標數的是「問了問不出來」，
            // 投降之後根本不會再問。所以那件事得由這一句帶著講，不能等
            // 底下那則。
            push(
                About::UrlRules,
                format!(
                    "UIA 在錄製途中卡住太多次已放棄：**從那一刻起讀不到網址**，\
                     {rules} 條 excluded_urls 規則不再生效（網銀、登入頁可能被錄了進去），\
                     瀏覽器裡的密碼欄也不再擋畫面。\
                     這是永久的，重開 sister record 才會再試一次"
                ),
            );
        } else if !self.url && rules > 0 {
            push(
                About::UrlRules,
                format!(
                    "沒有 UIA 網址擷取：{rules} 條 excluded_urls 規則（網銀、登入頁）\
                     目前不會生效，瀏覽器畫面只靠視窗標題規則過濾"
                ),
            );
        } else if rules > 0 && self.url_reads == 0 && self.browser_ticks >= ENOUGH_BROWSER_TICKS {
            // `url = true` 只代表 COM 物件造得出來。這一條是那句話和「讀得到
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
        // `!gave_up`：投降之後這道保護早就沒了，而上面那一則已經講過。
        // 兩則講同一個原因只會稀釋掉真正要看的那一則。
        if self.url_capture.password_check_broken && !self.url_capture.gave_up {
            push(
                // 密碼欄遮蔽和 `excluded_urls` 擋的不是同一件事，但它們在設定頁
                // 上是同一格（都在講「瀏覽器裡的東西怎麼被擋掉」）。
                About::UrlRules,
                "問不出焦點是不是在密碼欄上（連續失敗），已停止用它擋畫面：\
                 瀏覽器裡的密碼欄現在只靠圓點遮蔽保護"
                    .into(),
            );
        }
        if self.input_hook_failed {
            push(
                About::InputHook,
                "輸入 hook 裝不上：節奏訊號這個 session 會是空的".into(),
            );
        }
        out
    }
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
            url: true,
            input_hook_failed: false,
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

    #[test]
    fn a_rule_written_after_the_last_recording_still_gets_judged() {
        // 這一條是整個模組存在的理由。上一場錄製開始的時候他一條網址規則都
        // 沒有，所以那時候算出來的結論是「沒問題」——而他正是**現在**才在設定
        // 頁上打第一條。存結論的話，那一頁會拿著一份三禮拜前的「沒問題」，
        // 對著一條剛剛才寫下、而且不會生效的規則，什麼都不說。
        let blind = Report {
            url: false,
            ..able()
        };
        assert!(
            blind.broken_privacy_rules(&with_rules(0)).is_empty(),
            "沒有規則就沒有失效的規則"
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

    /// `url: true` 只代表 COM 物件造得出來，不代表讀得到位址列。
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
    /// 不會有錯誤也不會有例外，只是從那一刻起網銀跟登入頁開始被錄進去。
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
        // 密碼欄那道保護跟著一起沒了，而 `password_check_broken` 那個旗標
        // 數的是「問了問不出來」——投降之後根本不會再問，所以它是 false。
        // 這句話不由它帶的話，那件事就沒有人會講。
        assert!(!died.url_capture.password_check_broken);
        assert!(said[0].message.contains("密碼欄"), "{}", said[0].message);

        // 三種壞法在同一條路上，所以只講一則。全部印出來只會稀釋掉真正
        // 要看的那一則——而這裡最該看的是「有一個從那之後」。
        let died_blind_and_unprobed = Report {
            url: false,
            browser_ticks: 400,
            url_reads: 0,
            ..died
        };
        assert_eq!(
            died_blind_and_unprobed.broken_privacy_rules(&with_rules(16)),
            said
        );
    }

    /// 密碼欄那道保護關掉了要單獨講：它和網址規則擋的不是同一件事。
    #[test]
    fn the_password_shield_switching_itself_off_gets_its_own_line() {
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
        assert!(said[0].message.contains("密碼欄"), "{}", said[0].message);
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
            input_hook_failed: true,
            ..able()
        };
        let said = no_hook.broken_privacy_rules(&with_rules(16));
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(said[0].about, About::InputHook, "{said:?}");

        // 兩件事同時壞的時候要分成兩則、掛兩格——不是併成一段話塞在其中一格。
        let both = Report {
            url: false,
            ..no_hook
        };
        let said = both.broken_privacy_rules(&with_rules(16));
        assert_eq!(
            said.iter().map(|b| b.about).collect::<Vec<_>>(),
            vec![About::UrlRules, About::InputHook],
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
        assert!(back.url);
        assert_eq!(back.browser_ticks, 0, "舊檔案本來就沒有這個證據");
        assert!(!back.url_capture.gave_up);
        // 而「沒有證據」必須是沉默，不是警告。
        assert!(back.broken_privacy_rules(&with_rules(16)).is_empty());
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
            url: false,
            input_hook_failed: true,
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
        assert!(!back.url);
        assert!(back.input_hook_failed);
        assert_eq!(back.url_capture, written.url_capture);
        assert_eq!((back.browser_ticks, back.url_reads), (300, 7));
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
                url: true,
                input_hook_failed: true,
                ..Default::default()
            }
            .has_session_evidence()
        );
    }

    /// 而這四個只有那一場問得到，蓋掉就沒了。
    #[test]
    fn the_four_things_only_that_session_could_have_seen_are_evidence() {
        // 最重的那一則：從投降那一刻起，excluded_urls 一條都不生效。用一份
        // 全新的 UIA 去問永遠問不出來——新的那份是好的。
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
