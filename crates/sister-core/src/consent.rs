//! 四張同意書。
//!
//! 每一件事各自獨立、各自可以撤回：
//!
//! 1. **本機記錄**：在這台機器的硬碟上記錄螢幕。
//! 2. **上雲解讀**：把螢幕上的文字原文交給使用者設定的本機 CLI。
//! 3. **畫面暫存**：保留變化幀的截圖（相對於「只留 OCR 出來的字」）。
//! 4. **Azure 朗讀**：只在使用者點下朗讀時，把當前答案正文原文交給 Azure。
//!
//! ## 為什麼它會擋住東西
//!
//! 一張不擋任何事情的同意書不是同意書，是免責聲明。所以第一張沒簽，
//! `sister record` 就不會開始錄——不是印個警告然後照錄。
//!
//! 四張的效力刻意**不一樣**，因為四件事的性質不一樣：
//!
//! - 第一張是前提。沒有它就沒有這個產品，所以它是硬擋。
//! - 第三張是程度。沒有它不代表她不能記事，只代表她**只記字不留圖**——
//!   而那正是 SPEC 寫的「0 天 = 只留 OCR 文字」。所以它降級，不擋。
//! - 第二張和第四張是兩扇不同的出境閘門。第二張沒簽，解釋層一次都不會
//!   `spawn` 那支 CLI；第四張沒簽，一次都不會呼叫 Azure。兩者不能互相代替，也
//!   不能靠每個呼叫端自己記得加 `if allows_cloud()`——那一扇門要的是
//!   [`CloudAllowed`]，另一扇要 [`AzureTtsAllowed`]；只有對應的 consent method 鑄得出來。
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
use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::Millis;

/// 目前的條文版本。**改條文就要 +1**，代價是所有人都要重簽一次。
///
/// 2：第二張從「送到我指定的模型商」改成「交給我設定的本機 CLI」。
/// 收件人變了，舊簽名涵蓋不了這件事。
///
/// 3：第二張拿掉「去識別化後的」。送出去的是原文——金額、電話、人名都在。
/// 這是**擴大**了他同意的範圍，舊簽名絕對涵蓋不了，一定要重簽。
/// （為什麼拿掉去敏：記憶長期活在本機資料庫裡，而 `<PERSON_1>` 這種代號是
/// 每次呼叫重編的，跨段對不起來——承諾表和 entities 要的正是「王小明」
/// 這三個字能對得起來。去敏等於先拆掉 L3 的地基。）
///
/// alpha.109 新增的 Azure TTS 是第四張**獨立**條文，沒改舊三張的 wording，
/// 所以不把版本升到 4。舊的 version 3 檔案沒有 `azure_tts`，serde 會讀成
/// `None`：舊三張的簽名繼續如實生效，但絕對不會順便授權新的出境路徑。
pub const VERSION: u32 = 3;

const FILE: &str = "consent.toml";
const WRITE_LOCK: &str = "consent.lock";

/// 四張同意書。
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
    /// 第二張：把螢幕上的文字原文交給我設定的本機 CLI。
    #[serde(default)]
    pub cloud_reading: Option<Millis>,
    /// 第三張：保留變化幀截圖。
    #[serde(default)]
    pub frame_storage: Option<Millis>,
    /// 第四張：把當前答案正文原文交給 Azure TTS。
    #[serde(default)]
    pub azure_tts: Option<Millis>,
}

/// 四張裡的哪一張。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sheet {
    LocalRecording,
    CloudReading,
    FrameStorage,
    AzureTts,
}

impl Sheet {
    pub const ALL: [Sheet; 4] = [
        Sheet::LocalRecording,
        Sheet::CloudReading,
        Sheet::FrameStorage,
        Sheet::AzureTts,
    ];

    /// 命令列與設定檔裡的名字。
    pub fn key(self) -> &'static str {
        match self {
            Sheet::LocalRecording => "local-recording",
            Sheet::CloudReading => "cloud-reading",
            Sheet::FrameStorage => "frame-storage",
            Sheet::AzureTts => "azure-tts",
        }
    }

    /// 給人看的那一句。**是條文本身，不是標題**——他要按的是這句話。
    pub fn wording(self) -> &'static str {
        match self {
            Sheet::LocalRecording => "我同意在我的硬碟上記錄我的螢幕。",
            Sheet::CloudReading => {
                "我同意把螢幕上的文字原文（OCR 抽出來的字，永不含畫面）交給我在設定裡指定的本機 CLI，由那支程式去做解讀。裡面有什麼就送什麼，不會先遮掉。"
            }
            Sheet::FrameStorage => "我同意保留變化幀的截圖，而不是只留上面的字。",
            Sheet::AzureTts => {
                "我同意每次按下 Azure 朗讀時，把當前答案正文原文交給我在設定裡選擇區域的 Microsoft Azure 語音服務。正文可能含姓名、電話與金額，不會先遮罩；不會送出截圖、來源連結、memory id、整份資料庫或其他文字。"
            }
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
                "沒有這一張，sister record 不會開始錄；錄到一半撤回，正在跑的 record 每 5 秒重讀同意書，最多再錄 5 秒加一拍；capture.min_interval_ms 超過 5 秒時，主要會等那一拍。"
            }
            Sheet::CloudReading => {
                "沒有這一張，她一次都不會呼叫那支 CLI；解釋層保持關閉，只累積本機的畫面與文字。正在跑的 sister watch 每看一次就重讀一次同意書，撤回之後它下一次看的時候就停下來，不會再問。"
            }
            Sheet::FrameStorage => "沒有這一張，她只記螢幕上的字，不留截圖。",
            Sheet::AzureTts => "沒有這一張，她一次都不會呼叫 Azure 語音服務；本機朗讀不受影響。",
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
            Sheet::AzureTts => self.azure_tts,
        }
    }

    /// 簽下去。已經簽過的**不會**被蓋掉時戳——「我什麼時候同意的」問的是第一次。
    pub fn grant(&mut self, sheet: Sheet, ts: Millis) {
        let slot = match sheet {
            Sheet::LocalRecording => &mut self.local_recording,
            Sheet::CloudReading => &mut self.cloud_reading,
            Sheet::FrameStorage => &mut self.frame_storage,
            Sheet::AzureTts => &mut self.azure_tts,
        };
        slot.get_or_insert(ts);
        self.version = VERSION;
    }

    pub fn revoke(&mut self, sheet: Sheet) {
        match sheet {
            Sheet::LocalRecording => self.local_recording = None,
            Sheet::CloudReading => self.cloud_reading = None,
            Sheet::FrameStorage => self.frame_storage = None,
            Sheet::AzureTts => self.azure_tts = None,
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

    /// 東西可以離開這台機器嗎。給**顯示**用（doctor、同意書那一頁）。
    ///
    /// 真正送出的那一扇門不讀這個 `bool`。它要 [`CloudAllowed`]，只有
    /// [`Self::cloud_permit`] 鑄得出來。這一格是三張裡唯一一張「猜錯的方向
    /// 不對稱」的：另外兩張猜錯了是少記東西，這張猜錯了是東西送出去了。
    pub fn allows_cloud(&self) -> bool {
        self.current() && self.cloud_reading.is_some()
    }

    /// 交出出境憑證。沒簽、條文改版、檔案讀不出來，都是 `None`。
    ///
    /// [`crate::brain::spawn_cli`] 的第一個參數就是這個型別。沒走過這裡
    /// 就 spawn，編不過。
    pub fn cloud_permit(&self) -> Option<CloudAllowed> {
        self.allows_cloud().then_some(CloudAllowed(()))
    }

    /// 當前條文下，是否已獨立同意把當前答案正文交給 Azure TTS。
    ///
    /// 這個 bool 只給顯示；真正出境邊界要 [`AzureTtsAllowed`]。
    pub fn allows_azure_tts(&self) -> bool {
        self.current() && self.azure_tts.is_some()
    }

    /// 交出 Azure TTS 的獨立出境憑證。第二張 `cloud-reading` 絕不能鑄出它。
    fn azure_tts_permit(&self) -> Option<AzureTtsAllowed> {
        self.allows_azure_tts().then_some(AzureTtsAllowed(()))
    }

    /// 這份設定跑起來，硬碟上**真的**會多出截圖嗎。
    ///
    /// 設定檔說的是「我想要留圖」，同意書說的是「可不可以」，而只有後者算數。
    /// 這條規則本來只寫在 `record` 開錄前那道閘門裡，於是 `sister doctor` 讀的
    /// 是設定檔原值——同一張體檢報告上，同意書那一區印「畫面暫存：未同意 →
    /// 只記螢幕上的字，不留截圖」，隱私那一區印「保留畫面檔：是」。兩行講的是
    /// 同一件事，而其中一行是錯的。
    ///
    /// 這是這個 repo 第四次踩到同一根釘子（前三次是 `question::shape`、
    /// `Config::db_path`、`answer`）：一條規則寫在兩個地方，遲早有一邊忘了改。
    /// 體檢報告尤其不能有第二份——它存在的唯一理由就是講出真的會發生的事。
    ///
    /// `capture.enabled` 也算一票，而且是最外面那一票：它關著的時候每個 tick
    /// 直接回 `Tick::Disabled`，連螢幕都不會碰。少了它，一台
    /// `enabled = false` 的機器上，同意書那一頁會說「接下來跑 sister record
    /// 她才會開始，而且會留截圖」——兩個子句都錯，而它是使用者剛簽完名時
    /// 讀到的最後一句話。
    pub fn keeps_images(&self, config: &crate::config::Config) -> bool {
        config.capture.enabled && config.capture.store_images && self.allows_frames()
    }

    /// 把設定改成[實際會發生的事](Self::keeps_images)，回傳有沒有真的改到。
    ///
    /// 回傳值是給呼叫端「要不要講出來」用的：安靜地少存一半東西，使用者只會
    /// 以為截圖功能壞了。
    pub fn downgrade(&self, config: &mut crate::config::Config) -> bool {
        // 這裡**故意不是** `keeps_images`，雖然兩支長得很像。這一支問的是
        // 「同意書准不准」，而 `capture.enabled` 不是同意書的事：總開關關著
        // 的時候把 `store_images` 也一起改掉，呼叫端就會印出「第三張同意書
        // 沒簽」——而他可能簽得好好的，真正的原因是另一件事，另有一句話講。
        let keep = config.capture.store_images && self.allows_frames();
        let changed = config.capture.store_images != keep;
        config.capture.store_images = keep;
        changed
    }
}

/// 只有 [`Consent::cloud_permit`] 鑄得出來的出境憑證。
///
/// 不是 `bool`，也不是 `struct CloudAllowed { allowed: bool }`：那種還是
/// 能填錯。元組結構體、欄位私有，同一模組之外沒有別的路。
#[derive(Debug, Clone, Copy)]
pub struct CloudAllowed(());

/// 只有 [`begin_azure_tts_admission`] 在 shared consent transaction 裡鑄得出來的
/// Azure TTS 出境 marker；它只能從 [`AzureTtsAdmissionGuard::permit`] 借用，不能
/// 複製到 guard 外重放。
///
/// 它和 [`CloudAllowed`] 是不同型別：同意把 OCR 原文交給本機 CLI，
/// 不等於同意把當前答案送到 Microsoft Azure。
#[derive(Debug)]
pub struct AzureTtsAllowed(());

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// 讀回四張同意書。**任何讀不出來的情況都回「四張都沒簽」。**
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

/// Recorder 開始前對第一張同意書的完整判決。
///
/// 這裡不能回 `Option<bool>`：`Busy` 是另一個 consent writer 正握著 exclusive
/// transaction，`Unknown` 是連鎖檔／同意書都無法安全判讀，兩者都不等於一份
/// 確定無效的同意書。只有 [`Allowed`](Self::Allowed) 會交出必須活到第一拍
/// heartbeat 之後的 shared guard。
#[derive(Debug)]
pub enum RecordingStartConsent {
    /// 在 shared `consent.lock` 裡重讀到這一版有效的第一張同意書。
    Allowed(RecordingStartGuard),
    /// 同意書確定不存在、內容無效，或第一張對目前條文不生效。
    NotAllowed(Consent),
    /// 另一個 consent transaction 正在修改；nonblocking caller 不可以用舊快照開錄。
    Busy,
    /// 鎖或同意書無法安全開啟／驗證／讀取；不確定時不開始錄。
    Unknown(anyhow::Error),
}

/// 活著就持有 `consent.lock` 的 shared lock，並固定這次 start 使用的同意快照。
///
/// Recorder start 的 lock order 只能是：這個 guard → `stop.lock` →
/// `recording.lock`／舊 heartbeat barrier。Explicit clear 與第一拍 Booting heartbeat
/// 都要發生在 guard drop 以前；這樣已拿到 consent exclusive lock 的撤回不可能被
/// 一份較早的 `load()` 結果越過。
#[derive(Debug)]
pub struct RecordingStartGuard {
    _file: File,
    // Allowed 的 consent.toml opened handle 也留到第一拍；Windows 藉由不 share
    // write/delete 固定這份已驗證 inode，Unix 則至少確保讀的是 no-follow handle。
    _consent_file: Option<File>,
    data_dir: PathBuf,
    consent: Consent,
}

impl RecordingStartGuard {
    /// 在 shared transaction 裡讀到的完整快照；第三張的降級也必須用同一份。
    pub fn consent(&self) -> &Consent {
        &self.consent
    }

    /// 防止一份 guard 被接到另一個資料目錄的 stop／lease／heartbeat barrier。
    pub fn belongs_to(&self, data_dir: &Path) -> bool {
        self.data_dir == data_dir
    }
}

/// Azure TTS 建立 outbound admission 前對第四張同意書的完整判決。
///
/// 不能把 `NotAllowed` 和 `Unknown` 壓成同一個 `None`：前者是在 shared transaction
/// 裡確定第四張沒有生效，後者則是鎖檔或 consent handle 根本無法安全判讀。兩種都
/// 不得呼叫 Azure，但給人的錯誤與修復方向不同。只有 [`Allowed`](Self::Allowed)
/// 會交出一份必須活到 caller 完成 request admission 的 shared guard。
#[derive(Debug)]
pub enum AzureTtsAdmissionConsent {
    /// 在 shared `consent.lock` 裡重讀到這一版有效的第四張同意書。
    Allowed(AzureTtsAdmissionGuard),
    /// 同意書確定不存在、內容無效，或第四張對目前條文不生效。
    NotAllowed(Consent),
    /// 鎖或同意書無法安全開啟／驗證／讀取；不確定時不建立 outbound admission。
    Unknown(anyhow::Error),
}

/// 活著就持有 `consent.lock` 的 shared lock，並固定這次 Azure admission 的同意快照。
///
/// Caller 至少要先完成自己那一層的 generation／single-flight admission 才能 drop；
/// desktop 的 Azure transport 會更嚴格地把它保留到 blocking POST 結束。這樣 CLI 或
/// 另一扇 desktop 正在做的 consent transaction 不是完整排在 admission 前，就是等
/// 既有 request 結束才成功，不能讓鎖外的舊 `load()` 越過一份已回覆成功的撤回。
#[derive(Debug)]
pub struct AzureTtsAdmissionGuard {
    _file: File,
    // Windows 藉由不 share write/delete 固定這份已驗證 inode；Unix 則至少固定這次
    // 鎖內讀到的 opened handle，和 Recorder start 使用同一條安全路徑。
    _consent_file: Option<File>,
    data_dir: PathBuf,
    consent: Consent,
    signed_at: Millis,
    permit: AzureTtsAllowed,
}

impl AzureTtsAdmissionGuard {
    /// Shared transaction 內固定下來的完整同意快照。
    pub fn consent(&self) -> &Consent {
        &self.consent
    }

    /// 第四張這次生效的原始簽署時間；不是 admission 當下重新捏出的時間。
    pub const fn signed_at(&self) -> Millis {
        self.signed_at
    }

    /// 這份 guard 在 shared lock 內鑄出的 typed permit。
    ///
    /// 借用的 marker 不能活得比 shared-lock guard 久；真正 outbound helper 會
    /// by-value 吃掉整份 guard，而不是把 marker 複製到鎖外重放。
    pub const fn permit(&self) -> &AzureTtsAllowed {
        &self.permit
    }

    /// 防止一份 guard 被誤接到另一個 data dir 的 request coordinator。
    pub fn belongs_to(&self, data_dir: &Path) -> bool {
        self.data_dir == data_dir
    }
}

/// Blocking 取得 Azure TTS outbound admission 的 shared consent transaction。
///
/// 若 writer 正在 commit，這裡會等它完成後才在同一把鎖內重讀；沒有 `Busy` 分支，
/// 因為 desktop 會在 blocking worker 裡完成這一步，而不是在 UI thread 排隊。
pub fn begin_azure_tts_admission(data_dir: &Path) -> AzureTtsAdmissionConsent {
    let file = match open_consent_lock(data_dir) {
        Ok(file) => file,
        Err(error) => return AzureTtsAdmissionConsent::Unknown(error),
    };
    if let Err(error) = FileExt::lock_shared(&file) {
        return AzureTtsAdmissionConsent::Unknown(anyhow::Error::from(error).context(format!(
            "取得 Azure TTS 同意書 shared admission lock {} 失敗",
            data_dir.join(WRITE_LOCK).display()
        )));
    }

    let (consent, consent_file) = match load_for_shared_consent_admission(data_dir) {
        Ok(loaded) => loaded,
        Err(error) => return AzureTtsAdmissionConsent::Unknown(error),
    };
    let Some(permit) = consent.azure_tts_permit() else {
        return AzureTtsAdmissionConsent::NotAllowed(consent);
    };
    let signed_at = consent
        .azure_tts
        .expect("an Azure TTS permit always has a fourth-sheet timestamp");
    AzureTtsAdmissionConsent::Allowed(AzureTtsAdmissionGuard {
        _file: file,
        _consent_file: consent_file,
        data_dir: data_dir.to_path_buf(),
        consent,
        signed_at,
        permit,
    })
}

/// Blocking 取得 recorder start 的 shared consent transaction。
///
/// 給真人顯式 `sister record` 使用：若 writer 正在 commit，等它完成再在鎖內重讀，
/// 不會拿 writer 之前的快照開錄。此入口正常不回 [`RecordingStartConsent::Busy`]。
pub fn begin_recording_start(data_dir: &Path) -> RecordingStartConsent {
    recording_start_consent(data_dir, StartLockMode::Blocking)
}

/// Nonblocking 取得 recorder start 的 shared consent transaction。
///
/// 給 supervised child／desktop 使用：writer 正忙時回明確的 `Busy`，絕不排隊後在
/// caller 已逾時或取消時才偷偷開始。
pub fn try_begin_recording_start(data_dir: &Path) -> RecordingStartConsent {
    recording_start_consent(data_dir, StartLockMode::Nonblocking)
}

#[derive(Clone, Copy)]
enum StartLockMode {
    Blocking,
    Nonblocking,
}

fn recording_start_consent(data_dir: &Path, mode: StartLockMode) -> RecordingStartConsent {
    let file = match open_consent_lock(data_dir) {
        Ok(file) => file,
        Err(error) => return RecordingStartConsent::Unknown(error),
    };
    match mode {
        StartLockMode::Blocking => {
            if let Err(error) = FileExt::lock_shared(&file) {
                return RecordingStartConsent::Unknown(anyhow::Error::from(error).context(
                    format!(
                        "取得同意書 shared start lock {} 失敗",
                        data_dir.join(WRITE_LOCK).display()
                    ),
                ));
            }
        }
        StartLockMode::Nonblocking => match FileExt::try_lock_shared(&file) {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return RecordingStartConsent::Busy,
            Err(TryLockError::Error(error)) => {
                return RecordingStartConsent::Unknown(anyhow::Error::from(error).context(
                    format!(
                        "取得同意書 shared start lock {} 失敗",
                        data_dir.join(WRITE_LOCK).display()
                    ),
                ));
            }
        },
    }

    let (consent, consent_file) = match load_for_shared_consent_admission(data_dir) {
        Ok(loaded) => loaded,
        Err(error) => return RecordingStartConsent::Unknown(error),
    };
    if !consent.allows_recording() {
        return RecordingStartConsent::NotAllowed(consent);
    }
    RecordingStartConsent::Allowed(RecordingStartGuard {
        _file: file,
        _consent_file: consent_file,
        data_dir: data_dir.to_path_buf(),
        consent,
    })
}

/// 在 shared transaction 裡從 opened handle 讀同意書。Missing／invalid 是確定的
/// `NotAllowed`；路徑跟到 symlink／reparse 或 I/O 失敗則是 `Unknown`。
fn load_for_shared_consent_admission(data_dir: &Path) -> Result<(Consent, Option<File>)> {
    let consent_path = path(data_dir);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
        // 開 reparse point 本身再用 handle metadata 拒絕；不 share write/delete，
        // 讓這份已驗證快照在 guard 活著時不能被 pathname swap。
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode(FILE_SHARE_READ.0);
    }
    let mut file = match options.open(&consent_path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok((Consent::default(), None));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("開啟同意書 {} 失敗", consent_path.display()));
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("驗證同意書 {} 失敗", consent_path.display()))?;
    anyhow::ensure!(
        opened_lock_is_regular(&metadata),
        "同意書不是普通的 non-reparse 檔案：{}",
        consent_path.display()
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("讀取同意書 {} 失敗", consent_path.display()))?;
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok((Consent::default(), Some(file)));
    };
    // 語法壞掉是確定無效的同意，不是可以重試成 Allowed 的 I/O unknown。
    Ok((toml::from_str(text).unwrap_or_default(), Some(file)))
}

/// 在同一把跨行程 exclusive lock 裡重讀、修改並原子替換同意書。
///
/// 呼叫端不可以自己做 `load` → 修改 → `save`：desktop 與 CLI 若同時改不同張，
/// 後寫的人會把先寫的人整份舊快照蓋回去。對第一張而言，那會讓一次真的撤回在
/// 沒有人重簽時復活。closure 一定在取得鎖並重讀磁碟之後才執行；回傳值則是這次
/// 已經成功落地的完整快照。
pub fn mutate(
    data_dir: &Path,
    mutation: impl FnOnce(&mut Consent) -> Result<()>,
) -> Result<Consent> {
    let _lock = ConsentWriteLock::acquire(data_dir)?;
    let mut consent = load(data_dir);
    mutation(&mut consent)?;
    save_atomic_unlocked(data_dir, &consent)?;
    Ok(consent)
}

/// 寫入一份已組好的完整快照。
///
/// 這支仍供匯入與測試 fixture 使用，並和 [`mutate`] 共用同一把 write lock 與
/// atomic replace；任何 read-modify-write 則必須使用 [`mutate`]，否則重讀不在鎖內。
pub fn save(data_dir: &Path, consent: &Consent) -> Result<()> {
    let _lock = ConsentWriteLock::acquire(data_dir)?;
    save_atomic_unlocked(data_dir, consent)
}

/// 寫鎖是 consent 檔的 sibling，不跟著每次 atomic replace 換 inode；檔案本身永久
/// 保留，任何 consent／forget／prune 路徑都不刪它。handle drop 由作業系統釋放鎖，
/// 所以 writer crash 不會留下永久占用。
struct ConsentWriteLock {
    _file: File,
}

impl ConsentWriteLock {
    fn acquire(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(WRITE_LOCK);
        let file = open_consent_lock(data_dir)?;
        FileExt::lock(&file).with_context(|| format!("取得同意書寫鎖 {}", path.display()))?;
        Ok(Self { _file: file })
    }
}

/// 所有 consent reader／writer 都經過這個 open，才能確定鎖的是同一個安全 inode。
/// Windows 不 share delete，避免 live transaction 期間 pathname 被換掉；Unix 則在
/// open syscall 本身用 `O_NOFOLLOW` 關掉 check→open swap window。
fn open_consent_lock(data_dir: &Path) -> Result<File> {
    std::fs::create_dir_all(data_dir).with_context(|| format!("建立 {}", data_dir.display()))?;
    let path = data_dir.join(WRITE_LOCK);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("開啟同意書鎖 {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("驗證同意書鎖 {}", path.display()))?;
    anyhow::ensure!(
        opened_lock_is_regular(&metadata),
        "同意書鎖不是普通的 non-reparse 檔案：{}",
        path.display()
    );
    Ok(file)
}

fn opened_lock_is_regular(metadata: &Metadata) -> bool {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return false;
        }
    }
    true
}

fn save_atomic_unlocked(data_dir: &Path, consent: &Consent) -> Result<()> {
    std::fs::create_dir_all(data_dir).with_context(|| format!("建立 {}", data_dir.display()))?;
    let destination = path(data_dir);
    let body = toml::to_string_pretty(consent)?;
    atomic_write(data_dir, &destination, body.as_bytes())
}

fn atomic_write(data_dir: &Path, destination: &Path, body: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut opened = None;
    let mut temp_path = None;
    for _ in 0..128 {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = data_dir.join(format!(".consent-tmp-{}-{serial}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                opened = Some(file);
                temp_path = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("建立同意書暫存檔於 {} 失敗", data_dir.display()));
            }
        }
    }
    let mut file = opened.context("找不到可用的同意書暫存檔名")?;
    let temp = temp_path.expect("temp path accompanies opened file");
    let result = (|| -> Result<()> {
        file.write_all(body)
            .with_context(|| format!("寫入 {} 失敗", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("同步 {} 失敗", temp.display()))?;
        drop(file);
        replace_file_atomically(&temp, destination)?;
        #[cfg(unix)]
        File::open(data_dir)
            .and_then(|dir| dir.sync_all())
            .with_context(|| format!("同步 {} 目錄失敗", data_dir.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(source, destination).with_context(|| {
        format!(
            "原子替換 {} → {} 失敗",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .with_context(|| {
        format!(
            "原子替換 {} → {} 失敗",
            source.display(),
            destination.display()
        )
    })
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

    #[test]
    fn unsigned_cloud_sheet_cannot_mint_a_permit() {
        assert!(Consent::default().cloud_permit().is_none());
        let mut c = Consent::default();
        c.grant(Sheet::CloudReading, 1);
        assert!(c.cloud_permit().is_some());
        c.version = VERSION + 1;
        assert!(c.cloud_permit().is_none());
    }

    #[test]
    fn azure_tts_has_its_own_fail_closed_permit() {
        let mut cli_only = Consent::default();
        cli_only.grant(Sheet::CloudReading, 1);
        assert!(cli_only.cloud_permit().is_some());
        assert!(cli_only.azure_tts_permit().is_none());
        assert!(!cli_only.allows_azure_tts());

        let mut azure_only = Consent::default();
        azure_only.grant(Sheet::AzureTts, 2);
        assert!(azure_only.azure_tts_permit().is_some());
        assert!(azure_only.allows_azure_tts());
        assert!(azure_only.cloud_permit().is_none());
        assert!(!azure_only.allows_cloud());

        azure_only.version = VERSION + 1;
        assert!(azure_only.azure_tts_permit().is_none());
        assert!(!azure_only.allows_azure_tts());
    }

    #[test]
    fn a_version_three_consent_from_before_azure_keeps_old_grants_but_not_azure() {
        let old: Consent = toml::from_str(
            "version = 3\nlocal_recording = 11\ncloud_reading = 12\nframe_storage = 13\n",
        )
        .expect("pre-Azure version 3 consent");

        assert_eq!(old.version, VERSION);
        assert_eq!(old.local_recording, Some(11));
        assert_eq!(old.cloud_reading, Some(12));
        assert_eq!(old.frame_storage, Some(13));
        assert_eq!(old.azure_tts, None);
        assert!(old.allows_recording());
        assert!(old.allows_cloud());
        assert!(old.allows_frames());
        assert!(old.azure_tts_permit().is_none());
    }

    #[test]
    fn azure_wording_names_exactly_what_leaves_and_what_does_not() {
        let wording = Sheet::AzureTts.wording();
        for sent in ["當前答案正文原文", "姓名", "電話", "金額", "不會先遮罩"] {
            assert!(wording.contains(sent), "missing {sent}: {wording}");
        }
        for not_sent in ["截圖", "來源連結", "memory id", "整份資料庫", "其他文字"] {
            assert!(wording.contains(not_sent), "missing {not_sent}: {wording}");
        }

        let consequence = Sheet::AzureTts.without();
        assert!(consequence.contains("一次都不會呼叫 Azure"));
        assert!(consequence.contains("本機朗讀不受影響"));
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
        c.grant(Sheet::AzureTts, 44);
        save(&tmp.0, &c).expect("save");
        assert_eq!(load(&tmp.0), c);
        assert!(load(&tmp.0).azure_tts_permit().is_some());
    }

    #[test]
    fn recording_start_has_four_non_conflated_outcomes() {
        let tmp = Tmp::new("recording-start-outcomes");
        assert!(matches!(
            try_begin_recording_start(&tmp.0),
            RecordingStartConsent::NotAllowed(_)
        ));

        let mut consent = Consent::default();
        consent.grant(Sheet::LocalRecording, 42);
        save(&tmp.0, &consent).expect("signed consent");
        let allowed = match try_begin_recording_start(&tmp.0) {
            RecordingStartConsent::Allowed(guard) => guard,
            other => panic!("signed consent should be allowed, got {other:?}"),
        };
        assert_eq!(allowed.consent(), &consent);
        assert!(allowed.belongs_to(&tmp.0));
        drop(allowed);

        let writer = ConsentWriteLock::acquire(&tmp.0).expect("exclusive writer");
        assert!(matches!(
            try_begin_recording_start(&tmp.0),
            RecordingStartConsent::Busy
        ));
        drop(writer);

        std::fs::remove_file(tmp.0.join(WRITE_LOCK)).expect("remove unlocked lock file");
        std::fs::create_dir(tmp.0.join(WRITE_LOCK)).expect("directory-shaped lock");
        assert!(matches!(
            try_begin_recording_start(&tmp.0),
            RecordingStartConsent::Unknown(_)
        ));
    }

    #[test]
    fn azure_tts_admission_has_three_non_conflated_outcomes_and_a_bound_permit() {
        let tmp = Tmp::new("azure-admission-outcomes");
        match begin_azure_tts_admission(&tmp.0) {
            AzureTtsAdmissionConsent::NotAllowed(snapshot) => {
                assert_eq!(snapshot, Consent::default());
                assert!(!snapshot.allows_azure_tts());
            }
            other => panic!("missing fourth sheet must be NotAllowed, got {other:?}"),
        }

        let mut signed = Consent::default();
        signed.grant(Sheet::CloudReading, 41);
        signed.grant(Sheet::AzureTts, 42);
        save(&tmp.0, &signed).expect("signed Azure consent");
        let allowed = match begin_azure_tts_admission(&tmp.0) {
            AzureTtsAdmissionConsent::Allowed(guard) => guard,
            other => panic!("effective fourth sheet should admit Azure TTS, got {other:?}"),
        };
        assert_eq!(allowed.consent(), &signed);
        assert_eq!(allowed.signed_at(), 42);
        assert!(allowed.belongs_to(&tmp.0));
        let _typed_permit: &AzureTtsAllowed = allowed.permit();
        drop(allowed);

        std::fs::remove_file(tmp.0.join(WRITE_LOCK)).expect("remove unlocked lock file");
        std::fs::create_dir(tmp.0.join(WRITE_LOCK)).expect("directory-shaped lock");
        assert!(matches!(
            begin_azure_tts_admission(&tmp.0),
            AzureTtsAdmissionConsent::Unknown(_)
        ));
    }

    #[test]
    fn invalid_or_stale_azure_consent_is_not_unknown_and_never_mints_a_guard() {
        let tmp = Tmp::new("invalid-azure-admission");
        std::fs::write(path(&tmp.0), b"not = [valid toml").expect("invalid consent fixture");
        match begin_azure_tts_admission(&tmp.0) {
            AzureTtsAdmissionConsent::NotAllowed(snapshot) => {
                assert_eq!(snapshot, Consent::default());
            }
            other => panic!("invalid consent must fail closed as NotAllowed, got {other:?}"),
        }

        let mut stale = Consent::default();
        stale.grant(Sheet::AzureTts, 77);
        stale.version = VERSION + 1;
        save(&tmp.0, &stale).expect("stale fourth-sheet fixture");
        match begin_azure_tts_admission(&tmp.0) {
            AzureTtsAdmissionConsent::NotAllowed(snapshot) => {
                assert_eq!(
                    snapshot.azure_tts,
                    Some(77),
                    "signed history remains visible"
                );
                assert!(
                    !snapshot.allows_azure_tts(),
                    "stale wording cannot authorize"
                );
            }
            other => panic!("stale fourth sheet must be NotAllowed, got {other:?}"),
        }
    }

    #[test]
    fn azure_admission_guard_holds_the_cross_process_shared_lock_until_drop() {
        let tmp = Tmp::new("azure-admission-lock");
        let mut signed = Consent::default();
        signed.grant(Sheet::AzureTts, 91);
        save(&tmp.0, &signed).expect("signed Azure consent");
        let guard = match begin_azure_tts_admission(&tmp.0) {
            AzureTtsAdmissionConsent::Allowed(guard) => guard,
            other => panic!("signed fourth sheet should produce a guard, got {other:?}"),
        };

        let writer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(tmp.0.join(WRITE_LOCK))
            .expect("independent writer handle");
        assert!(
            matches!(FileExt::try_lock(&writer), Err(TryLockError::WouldBlock)),
            "a consent writer must not cross a live Azure admission guard"
        );

        drop(guard);
        FileExt::try_lock(&writer).expect("dropping the admission guard releases the writer");
        drop(writer);

        mutate(&tmp.0, |consent| {
            consent.revoke(Sheet::AzureTts);
            Ok(())
        })
        .expect("revoke after admission releases its guard");
        assert!(matches!(
            begin_azure_tts_admission(&tmp.0),
            AzureTtsAdmissionConsent::NotAllowed(_)
        ));
    }

    #[test]
    fn invalid_consent_is_not_unknown_and_never_mints_a_start_guard() {
        let tmp = Tmp::new("invalid-start-consent");
        std::fs::write(path(&tmp.0), b"not = [valid toml").expect("invalid consent fixture");
        match try_begin_recording_start(&tmp.0) {
            RecordingStartConsent::NotAllowed(snapshot) => {
                assert_eq!(snapshot, Consent::default());
                assert!(!snapshot.allows_recording());
            }
            other => panic!("invalid consent must fail closed as NotAllowed, got {other:?}"),
        }
    }

    #[test]
    fn locked_mutation_reloads_the_previous_commit_instead_of_restoring_a_stale_sheet() {
        let tmp = Tmp::new("locked-rmw");
        let mut initial = Consent::default();
        initial.grant(Sheet::LocalRecording, 1);
        save(&tmp.0, &initial).expect("initial local consent");

        // 模擬第一個 writer 已拿鎖、撤回第一張但尚未 commit。第二個獨立 handle
        // 此刻必須真的撞鎖；光讓 save 本身加鎖、把 reload 留在鎖外，擋不住
        // desktop/CLI 各自拿舊快照覆寫。
        let first = ConsentWriteLock::acquire(&tmp.0).expect("first writer lock");
        let mut revoked = load(&tmp.0);
        revoked.revoke(Sheet::LocalRecording);
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(tmp.0.join(WRITE_LOCK))
            .expect("second lock handle");
        assert!(
            matches!(
                fs4::FileExt::try_lock(&contender),
                Err(fs4::TryLockError::WouldBlock)
            ),
            "另一個 consent writer 不可以穿過 live transaction"
        );
        save_atomic_unlocked(&tmp.0, &revoked).expect("commit revoke");
        drop(first);

        // 後來只簽第二張；它必須在鎖內重讀到第一張已撤回，不能把 initial 那份
        // local signature 帶回來。
        let committed = mutate(&tmp.0, |current| {
            current.grant(Sheet::CloudReading, 2);
            Ok(())
        })
        .expect("unrelated grant");
        assert!(!committed.allows_recording(), "沒有重簽第一張，不可以復活");
        assert!(committed.allows_cloud(), "後來明確簽的第二張仍要生效");
        assert_eq!(load(&tmp.0), committed);
    }

    #[test]
    fn atomic_save_replaces_an_existing_consent_without_leaving_a_temp_file() {
        let tmp = Tmp::new("atomic-replace");
        let mut first = Consent::default();
        first.grant(Sheet::LocalRecording, 1);
        save(&tmp.0, &first).expect("first snapshot");

        let mut second = Consent::default();
        second.grant(Sheet::CloudReading, 2);
        save(&tmp.0, &second).expect("replace snapshot");
        assert_eq!(load(&tmp.0), second);
        assert!(
            std::fs::read_dir(&tmp.0)
                .expect("list data dir")
                .all(|entry| !entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".consent-tmp-")),
            "成功 commit 後不該留下暫存檔"
        );
    }

    #[test]
    fn a_non_regular_write_lock_fails_closed() {
        let tmp = Tmp::new("lock-directory");
        std::fs::create_dir(tmp.0.join(WRITE_LOCK)).expect("directory-shaped lock");
        let error = mutate(&tmp.0, |_| Ok(())).expect_err("directory is not a lock file");
        assert!(format!("{error:#}").contains(WRITE_LOCK), "{error:#}");
        assert_eq!(load(&tmp.0), Consent::default());
    }

    #[cfg(unix)]
    #[test]
    fn a_shared_start_guard_never_follows_a_symlinked_lock_or_consent() {
        use std::os::unix::fs::symlink;

        let lock_tmp = Tmp::new("shared-lock-symlink");
        let lock_target = lock_tmp.0.join("not-the-shared-lock");
        std::fs::write(&lock_target, b"must remain an ordinary unlocked file").expect("target");
        symlink(&lock_target, lock_tmp.0.join(WRITE_LOCK)).expect("lock symlink");
        assert!(matches!(
            try_begin_recording_start(&lock_tmp.0),
            RecordingStartConsent::Unknown(_)
        ));
        let target_handle = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_target)
            .expect("target handle");
        FileExt::try_lock(&target_handle).expect("symlink target was never locked");

        let consent_tmp = Tmp::new("shared-consent-symlink");
        let mut signed = Consent::default();
        signed.grant(Sheet::LocalRecording, 1);
        let signed_target = consent_tmp.0.join("not-consent.toml");
        std::fs::write(&signed_target, toml::to_string(&signed).expect("serialize"))
            .expect("signed target");
        symlink(&signed_target, path(&consent_tmp.0)).expect("consent symlink");
        assert!(matches!(
            try_begin_recording_start(&consent_tmp.0),
            RecordingStartConsent::Unknown(_)
        ));
        assert!(matches!(
            begin_azure_tts_admission(&consent_tmp.0),
            AzureTtsAdmissionConsent::Unknown(_)
        ));
        assert_eq!(
            std::fs::read_to_string(&signed_target).expect("target survives"),
            toml::to_string(&signed).expect("serialize again")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_live_windows_write_lock_cannot_be_unlinked_and_replaced() {
        let tmp = Tmp::new("lock-deny-delete");
        let lock = ConsentWriteLock::acquire(&tmp.0).expect("acquire consent write lock");
        let path = tmp.0.join(WRITE_LOCK);

        std::fs::remove_file(&path)
            .expect_err("consent lock must deny delete sharing while a transaction owns it");
        assert!(
            path.is_file(),
            "failed deletion must leave the lock pathname intact"
        );

        drop(lock);
        std::fs::remove_file(&path).expect("dropping transaction releases delete sharing");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_write_lock_is_rejected_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let tmp = Tmp::new("lock-symlink");
        let target = tmp.0.join("not-the-consent-lock");
        std::fs::write(&target, b"do not lock or rewrite me").expect("target fixture");
        symlink(&target, tmp.0.join(WRITE_LOCK)).expect("symlink lock fixture");

        let error = mutate(&tmp.0, |consent| {
            consent.grant(Sheet::LocalRecording, 1);
            Ok(())
        })
        .expect_err("write lock must not follow a symlink");
        assert!(format!("{error:#}").contains(WRITE_LOCK), "{error:#}");
        assert_eq!(
            std::fs::read(&target).expect("target survives"),
            b"do not lock or rewrite me"
        );
        assert!(!load(&tmp.0).allows_recording());
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
        assert_eq!(
            Sheet::from_str("azure_tts").expect("Azure underscore"),
            Sheet::AzureTts
        );
        assert!(Sheet::from_str("everything").is_err());
    }

    #[test]
    fn revoking_local_recording_names_both_the_consent_clock_and_tick() {
        let consequence = Sheet::LocalRecording.without();
        assert!(consequence.contains("每 5 秒重讀同意書"));
        assert!(consequence.contains("最多再錄 5 秒加一拍"));
        assert!(consequence.contains("min_interval_ms 超過 5 秒時，主要會等那一拍"));
        assert!(!consequence.contains("下一個 tick 停下來"));
        assert!(!consequence.contains("5 秒內停下來"));
    }

    /// 設定檔說要留圖、第三張沒簽——`record` 會降級成只記字，而體檢報告以前
    /// 讀的是設定檔原值，於是同一張報告上一區說「不留截圖」、另一區說「是」。
    /// 兩個問句現在只有一個答案。
    #[test]
    fn the_config_can_ask_for_frames_but_the_sheet_decides() {
        let mut config = crate::config::Config::default();
        assert!(config.capture.store_images, "預設是想留圖的");

        let unsigned = Consent::default();
        assert!(!unsigned.keeps_images(&config), "沒簽就是不會留");
        assert!(unsigned.downgrade(&mut config), "而且要講出來");
        assert!(!config.capture.store_images);

        // 已經降級過的設定不會再回報一次——講第二次就變成雜訊。
        assert!(!unsigned.downgrade(&mut config));

        let mut signed = Consent::default();
        signed.grant(Sheet::FrameStorage, 1);
        let mut wants = crate::config::Config::default();
        assert!(signed.keeps_images(&wants));
        assert!(!signed.downgrade(&mut wants), "簽了就不該被動到");
        assert!(wants.capture.store_images);

        // 反過來也一樣：簽了同意書不代表使用者想留圖。同意是允許，不是要求。
        let mut text_only = crate::config::Config::default();
        text_only.capture.store_images = false;
        assert!(!signed.keeps_images(&text_only));
        assert!(!signed.downgrade(&mut text_only));
    }

    /// 總開關關著的時候不會有截圖，而**那不是同意書的鍋**。
    ///
    /// `keeps_images` 問的是「硬碟上真的會多出截圖嗎」，所以它要算
    /// `capture.enabled` 那一票——關著的話每個 tick 直接回 `Tick::Disabled`，
    /// 連螢幕都不會碰。少了它，同意書那一頁會在一台 `enabled = false` 的機器
    /// 上說「接下來跑 sister record 她才會開始，而且會留截圖」，兩個子句都錯。
    ///
    /// `downgrade` 問的是另一題（同意書准不准），所以它**不算**那一票。兩支
    /// 用同一個算式的話，總開關關著就會印出「第三張同意書沒簽」——而他可能
    /// 簽得好好的，真正的原因另有一句話在講。
    #[test]
    fn the_master_switch_counts_for_what_lands_on_disk_but_not_for_whose_fault_it_is() {
        let mut signed = Consent::default();
        signed.grant(Sheet::FrameStorage, 1);

        let mut off = crate::config::Config::default();
        off.capture.enabled = false;
        assert!(off.capture.store_images, "他要的是留圖");
        assert!(
            !signed.keeps_images(&off),
            "總開關關著，硬碟上不會多出任何一張截圖"
        );
        assert!(
            !signed.downgrade(&mut off),
            "但這不是同意書擋的，不准借同意書的嘴講出來"
        );
        assert!(off.capture.store_images, "他的偏好也不該被改掉");
    }

    /// 條文改版之後舊簽名失效，留圖也要跟著停——`allows_frames` 已經管了版本，
    /// 這裡是釘住「降級這條路真的有走過那個檢查」。
    #[test]
    fn an_outdated_signature_stops_the_screenshots_too() {
        let mut stale = Consent::default();
        stale.grant(Sheet::FrameStorage, 1);
        stale.version = VERSION + 1;

        let mut config = crate::config::Config::default();
        assert!(!stale.keeps_images(&config));
        assert!(stale.downgrade(&mut config));
    }
}
