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

use crate::config::Config;
use crate::model::Millis;

/// 檔名。放在 data dir 裡，跟 `sister.db`、`recording.beat` 同一層。
const FILE: &str = "capabilities.json";

/// 上一次 `sister record` 起來的時候探測到的東西。
///
/// 欄位刻意都是原始事實（做得到／做不到），沒有一個是判斷。判斷在
/// [`Self::broken_privacy_rules`] 裡，而它每次都拿現在的設定重算。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// 探測的時間。給人看的：一份三個禮拜前的報告和今天早上那份，可信度不同，
    /// 而讀的人有權自己判斷——不要替他決定「夠新了」。
    pub at: Millis,
    /// 讀得到瀏覽器網址（UIA）。**沒有它，`excluded_urls` 整組規則不生效。**
    pub url: bool,
    /// 輸入 hook **試過而且失敗**。
    ///
    /// 是 `試過失敗` 而不是 `裝好了`：「沒去裝」和「裝失敗」不是同一件事，
    /// 壓成一個布林會產生一則永遠為真的警告，然後整區警告都會被學會忽略。
    /// 這一份是 recorder 寫的，它一定試過——但欄位的語意要留給以後也對。
    pub input_hook_failed: bool,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// 蓋一份新的。recorder 每次開機寫一次。
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

impl Report {
    /// 因為能力缺席而**失效的隱私規則**，拿現在這一份設定重算。
    ///
    /// 和一般的功能缺口分開講：使用者可以接受「還不會 OCR」，但他必須知道
    /// 「你設定的網銀排除規則現在一條都不會生效」。前者是少做了一件事，
    /// 後者是他以為關上的門其實開著。
    pub fn broken_privacy_rules(&self, config: &Config) -> Vec<String> {
        let mut out = Vec::new();
        if !self.url && !config.privacy.excluded_urls.is_empty() {
            out.push(format!(
                "沒有 UIA 網址擷取：{} 條 excluded_urls 規則（網銀、登入頁）\
                 目前不會生效，瀏覽器畫面只靠視窗標題規則過濾",
                config.privacy.excluded_urls.len()
            ));
        }
        if self.input_hook_failed {
            out.push("輸入 hook 裝不上：節奏訊號這個 session 會是空的".into());
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
        let mut config = Config::default();
        config.privacy.excluded_urls.clear();
        assert!(
            blind.broken_privacy_rules(&config).is_empty(),
            "沒有規則就沒有失效的規則"
        );
        config.privacy.excluded_urls = vec!["*bank*".into()];
        let said = blind.broken_privacy_rules(&config);
        assert_eq!(said.len(), 1, "剛打的這一條要被判出來：{said:?}");
        assert!(said[0].contains("1 條"), "要數得出幾條：{}", said[0]);
    }

    #[test]
    fn a_machine_that_can_read_urls_says_nothing() {
        let mut config = Config::default();
        config.privacy.excluded_urls = vec!["*bank*".into()];
        assert!(able().broken_privacy_rules(&config).is_empty());
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
        };
        write(&dir.0, &written).expect("write");
        let back = read(&dir.0).expect("read back");
        assert_eq!(back.at, written.at);
        assert!(!back.url);
        assert!(back.input_hook_failed);
        // 暫存檔要收乾淨，不然 data dir 裡會慢慢長出一堆 .json.tmp。
        assert!(!path(&dir.0).with_extension("json.tmp").exists());
    }
}
