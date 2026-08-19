//! 三張同意書。
//!
//! SPEC §11.1 定的三件事，各自獨立、各自可以撤回：
//!
//! 1. **本機記錄**：在這台機器的硬碟上記錄螢幕。
//! 2. **上雲解讀**：把去識別化後的文字送到指定的模型商。
//! 3. **畫面暫存**：保留變化幀的截圖（相對於「只留 OCR 出來的字」）。
//!
//! ## 為什麼它會擋住東西
//!
//! 一張不擋任何事情的同意書不是同意書，是免責聲明。所以第一張沒簽，
//! `sister record` 就不會開始錄——不是印個警告然後照錄。
//!
//! 三張的效力刻意**不一樣**，因為三件事的性質不一樣：
//!
//! - 第一張是前提。沒有它就沒有這個產品，所以它是硬擋。
//! - 第三張是程度。沒有它不代表她不能記事，只代表她**只記字不留圖**——
//!   而那正是 SPEC 寫的「0 天 = 只留 OCR 文字」。所以它降級，不擋。
//! - 第二張目前無事可擋：這份程式裡沒有任何 HTTP client（`check-no-network.sh`
//!   每次 CI 都在證明這件事），所以它現在只是被誠實地記著「沒有簽」。
//!   哪天真的長出雲端路徑，這一格就是那條路的開關。
//!
//! ## 不確定就是沒同意
//!
//! 檔案不見、讀不到、TOML 壞掉、版本對不上——一律當成沒簽。和 [`crate::pause`]
//! 同一條紀律：這種問題只有一個安全的預設答案，而把錯誤丟給呼叫端等於讓每一個
//! 呼叫端各自決定一次，只要有一個人答錯，承諾就破了。
//!
//! ## 條文改了要重問
//!
//! 存下來的 `version` 對不上 [`VERSION`] 時，整份視為沒簽。一份對著舊條文按下
//! 的同意，不能拿來涵蓋後來新加的東西——這件事很不方便，而它的替代方案是
//! 「悄悄地把新條款算他同意了」。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::model::Millis;

/// 目前的條文版本。**改條文就要 +1**，代價是所有人都要重簽一次。
pub const VERSION: u32 = 1;

const FILE: &str = "consent.toml";

/// 三張同意書。
///
/// 每一張存的是**何時**簽的而不是一個 `bool`：`None` 和 `false` 在型別上就分得
/// 開，而且「我什麼時候同意的」是一個他有權利問、而我們現在答得出來的問題。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    /// 按下同意的當下，條文是第幾版。
    #[serde(default)]
    pub version: u32,
    /// 第一張：在我的硬碟上記錄我的螢幕。
    #[serde(default)]
    pub local_recording: Option<Millis>,
    /// 第二張：把去識別化後的文字送到我指定的模型商。
    #[serde(default)]
    pub cloud_reading: Option<Millis>,
    /// 第三張：保留變化幀截圖。
    #[serde(default)]
    pub frame_storage: Option<Millis>,
}

/// 三張裡的哪一張。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sheet {
    LocalRecording,
    CloudReading,
    FrameStorage,
}

impl Sheet {
    pub const ALL: [Sheet; 3] = [
        Sheet::LocalRecording,
        Sheet::CloudReading,
        Sheet::FrameStorage,
    ];

    /// 命令列與設定檔裡的名字。
    pub fn key(self) -> &'static str {
        match self {
            Sheet::LocalRecording => "local-recording",
            Sheet::CloudReading => "cloud-reading",
            Sheet::FrameStorage => "frame-storage",
        }
    }

    /// 給人看的那一句。**是條文本身，不是標題**——他要按的是這句話。
    pub fn wording(self) -> &'static str {
        match self {
            Sheet::LocalRecording => "我同意在我的硬碟上記錄我的螢幕。",
            Sheet::CloudReading => {
                "我同意把去識別化後的文字（永不含畫面）送到我指定的模型商做解讀。"
            }
            Sheet::FrameStorage => "我同意保留變化幀的截圖，而不是只留上面的字。",
        }
    }

    /// 沒簽的話會發生什麼。這一句比條文重要——他要判斷的是後果。
    ///
    /// 這幾句是**同時**被終端機和 onboarding 那一頁印出來的，所以裡面不放
    /// 反引號之類只有其中一邊看得懂的記號——在 GUI 上那就只是兩撇雜訊。
    /// 中文裡夾一段拉丁字母本來就跳得出來，不需要再框一次。
    ///
    /// 改這幾句**不必**動 [`VERSION`]。他按下去同意的是 [`Self::wording`] 那
    /// 一句；這裡是我們對後果的描述，寫得更準確不代表他同意的東西變了。反過來
    /// 要是動了 `wording`，那就是另一句話了，`VERSION` 非加不可。
    pub fn without(self) -> &'static str {
        match self {
            Sheet::LocalRecording => {
                "沒有這一張，sister record 不會開始錄；正在錄的也會在 5 秒內停下來。"
            }
            Sheet::CloudReading => {
                "沒有這一張，她完全在本機跑。（目前這份程式裡沒有任何連外路徑，所以這一張還沒有東西可以開。）"
            }
            Sheet::FrameStorage => "沒有這一張，她只記螢幕上的字，不留截圖。",
        }
    }
}

impl std::str::FromStr for Sheet {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // 底線和連字號都收：`--grant local_recording` 打錯成連字號（或反過來）
        // 而被拒絕，只會讓人以為這個功能壞了。
        let want = s.trim().to_ascii_lowercase().replace('_', "-");
        Sheet::ALL
            .into_iter()
            .find(|s| s.key() == want)
            .ok_or_else(|| {
                format!(
                    "沒有這一張同意書：{s}（可用的是 {}）",
                    Sheet::ALL
                        .into_iter()
                        .map(Sheet::key)
                        .collect::<Vec<_>>()
                        .join("、")
                )
            })
    }
}

impl Consent {
    pub fn get(&self, sheet: Sheet) -> Option<Millis> {
        match sheet {
            Sheet::LocalRecording => self.local_recording,
            Sheet::CloudReading => self.cloud_reading,
            Sheet::FrameStorage => self.frame_storage,
        }
    }

    /// 簽下去。已經簽過的**不會**被蓋掉時戳——「我什麼時候同意的」問的是第一次。
    pub fn grant(&mut self, sheet: Sheet, ts: Millis) {
        let slot = match sheet {
            Sheet::LocalRecording => &mut self.local_recording,
            Sheet::CloudReading => &mut self.cloud_reading,
            Sheet::FrameStorage => &mut self.frame_storage,
        };
        slot.get_or_insert(ts);
        self.version = VERSION;
    }

    pub fn revoke(&mut self, sheet: Sheet) {
        match sheet {
            Sheet::LocalRecording => self.local_recording = None,
            Sheet::CloudReading => self.cloud_reading = None,
            Sheet::FrameStorage => self.frame_storage = None,
        }
    }

    /// 這份同意書是不是對著**現在這一版**條文簽的。
    pub fn current(&self) -> bool {
        self.version == VERSION
    }

    /// 她可以開始錄嗎。
    pub fn allows_recording(&self) -> bool {
        self.current() && self.local_recording.is_some()
    }

    /// 她可以把截圖寫到硬碟上嗎。
    ///
    /// 注意這裡**不是**「可不可以錄」：沒有這一張她照樣記字，只是不留圖。
    pub fn allows_frames(&self) -> bool {
        self.current() && self.frame_storage.is_some()
    }
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// 讀回三張同意書。**任何讀不出來的情況都回「三張都沒簽」。**
///
/// 和 [`crate::pause::is_paused`] 一樣不回 `Result`：見模組開頭。
pub fn load(data_dir: &Path) -> Consent {
    let Ok(text) = std::fs::read_to_string(path(data_dir)) else {
        return Consent::default();
    };
    // 壞掉的 TOML 不可以退回「預設值」再往下走——這裡的預設值剛好就是
    // 「沒簽」，所以這一行看起來很無聊；它無聊是因為型別選對了。
    toml::from_str(&text).unwrap_or_default()
}

pub fn save(data_dir: &Path, consent: &Consent) -> Result<()> {
    std::fs::create_dir_all(data_dir).with_context(|| format!("建立 {}", data_dir.display()))?;
    let p = path(data_dir);
    std::fs::write(&p, toml::to_string_pretty(consent)?)
        .with_context(|| format!("寫入 {}", p.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 自己搭暫存目錄，和 `retention.rs`／`tests/privacy.rs` 同一套。
    /// 不引 `tempfile`：相依樹是被 `check-no-network.sh` 盯著的資產。
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-consent-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 全新的機器上，她不准開始錄。
    ///
    /// 這是整個模組的理由。反過來那個版本——「還沒問過，那就先錄著」——
    /// 正是這份專案在文件裡指著別人罵的那件事。
    #[test]
    fn a_machine_that_was_never_asked_does_not_get_recorded() {
        let tmp = Tmp::new("fresh");
        let c = load(&tmp.0);
        assert_eq!(c, Consent::default());
        assert!(!c.allows_recording(), "沒問過就是沒同意");
        assert!(!c.allows_frames());
    }

    /// 壞掉的同意書等於沒有同意書。
    #[test]
    fn a_consent_file_we_cannot_parse_is_no_consent_at_all() {
        let tmp = Tmp::new("broken");
        std::fs::write(path(&tmp.0), "這不是 TOML {{{").expect("write");
        assert!(!load(&tmp.0).allows_recording());
    }

    /// 條文改版之後，舊的簽名不算數。
    ///
    /// 很不方便，而且是故意的：替代方案是把他沒讀過的新條款算他同意了。
    #[test]
    fn a_signature_on_last_years_wording_does_not_cover_this_years() {
        let tmp = Tmp::new("stale");
        let mut c = Consent::default();
        c.grant(Sheet::LocalRecording, 1_700_000_000_000);
        c.version = VERSION + 1; // 假裝這是一份未來版本的檔案
        save(&tmp.0, &c).expect("save");

        let back = load(&tmp.0);
        assert_eq!(
            back.local_recording,
            Some(1_700_000_000_000),
            "簽過的事實要留著，不然使用者會以為自己從來沒按過"
        );
        assert!(!back.allows_recording(), "但它不能拿來蓋現在這一版");
    }

    /// 第三張沒簽不是「不能錄」，是「只記字不留圖」。
    #[test]
    fn refusing_the_screenshots_still_leaves_her_able_to_remember_words() {
        let mut c = Consent::default();
        c.grant(Sheet::LocalRecording, 1);
        assert!(c.allows_recording(), "第一張簽了就能錄");
        assert!(!c.allows_frames(), "但第三張沒簽就不留圖");
    }

    /// 再按一次同意，不會把「我什麼時候同意的」改成今天。
    #[test]
    fn signing_twice_does_not_rewrite_the_date_on_the_first_signature() {
        let mut c = Consent::default();
        c.grant(Sheet::LocalRecording, 111);
        c.grant(Sheet::LocalRecording, 999);
        assert_eq!(c.local_recording, Some(111));
    }

    /// 撤回之後再簽，時戳才會是新的——因為那真的是一次新的同意。
    #[test]
    fn revoking_and_signing_again_records_the_new_date() {
        let mut c = Consent::default();
        c.grant(Sheet::FrameStorage, 111);
        c.revoke(Sheet::FrameStorage);
        assert!(!c.allows_frames());
        c.grant(Sheet::FrameStorage, 999);
        assert_eq!(c.frame_storage, Some(999));
    }

    #[test]
    fn round_trips_through_disk() {
        let tmp = Tmp::new("roundtrip");
        let mut c = Consent::default();
        c.grant(Sheet::LocalRecording, 42);
        c.grant(Sheet::FrameStorage, 43);
        save(&tmp.0, &c).expect("save");
        assert_eq!(load(&tmp.0), c);
    }

    #[test]
    fn sheet_names_survive_the_usual_typos() {
        use std::str::FromStr;
        assert_eq!(
            Sheet::from_str("local_recording").expect("underscore"),
            Sheet::LocalRecording
        );
        assert_eq!(
            Sheet::from_str("  Frame-Storage ").expect("case and space"),
            Sheet::FrameStorage
        );
        assert!(Sheet::from_str("everything").is_err());
    }
}
