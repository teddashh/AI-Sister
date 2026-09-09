//! Windows 擷取後端。
//!
//! 六個來源各自獨立實作最小的 trait，再用 [`CompositeBackend`] 組起來。
//! 能力是逐項降級的：OCR 還沒接上不代表焦點與剪貼簿不能先跑。
//!
//! 目前**還沒有**的東西，以及它們各自的代價，都在 [`Capabilities`] 裡誠實
//! 列出來，由 `sister doctor` 直接顯示給使用者看。缺功能不可怕，
//! 缺功能而使用者以為有才可怕。

pub mod clipboard;
pub mod focus;
pub mod input;
pub mod ocr;
pub mod screen;
pub mod system;
pub mod uia;

use anyhow::Result;
use sister_core::capabilities::CapabilityState;
use sister_core::config::Config;
use sister_core::db::Db;
use sister_core::now_ms;
use std::path::PathBuf;

use crate::Recorder;
use crate::ocr_regions::ChangedRegionOcr;
use crate::traits::{Backend, CompositeBackend};

/// 只有這個 production Windows 模組與其子模組能造出的證明。
/// permit 與 session provenance 都要求它，避免 replay／測試後端只靠拼字串
/// 冒充已通過 UIA v2 隱私契約的 recorder。
#[derive(Clone, Copy)]
pub(crate) struct WindowsBackendToken(());

const fn backend_token() -> WindowsBackendToken {
    WindowsBackendToken(())
}

/// 這台機器上這個後端實際做得到什麼。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    // `screen` / `focus` / `clipboard` 這三個欄位曾經在這裡，永遠是 `true`，
    // 而且沒有任何一行程式碼讀過它們。那正是這個專案在對付的東西：一個
    // 回報 ✓ 但什麼都沒驗證的能力欄位。真的要知道前景讀不讀得到，
    // `doctor` 現在會當場問一次——見那裡的 `focus_probe`。
    /// UIA／瀏覽器網址能力。沒有它時 privacy context 不可確認，recorder fail closed；
    /// `excluded_urls` 因此無法評估，不會被拿空值當成安全而放行。
    pub url: CapabilityState,
    /// 輸入 hook 的三態。**不能是布林**：「沒去裝」不等於
    /// 「裝失敗」——把兩者壓成 false 會產生一則永遠錯的警告，
    /// 然後整個警告區塊都會被使用者學會忽略。
    pub input: CapabilityState,
    pub ocr: CapabilityState,
    /// OCR 實際挑中的語言，以及這台機器上裝了哪些。
    ///
    /// 「有沒有 OCR」是個布林，但「讀不讀得懂中文」不是——所以兩件事分開存。
    pub ocr_language: Option<String>,
    pub ocr_languages_available: Vec<String>,
}

impl Capabilities {
    pub fn current(config: &Config) -> Self {
        let ocr = ocr::OcrStatus::probe(&config.capture.ocr_languages);
        Self {
            // 「UIA 建得起來」而已。真的讀不讀得到位址列要在有瀏覽器開著
            // 的時候才知道——那件事由 `doctor` 的實測那一段回答，不是這裡。
            url: CapabilityState::from_measured(uia::Uia::probe()),
            input: match input::WindowsInput::state() {
                input::HookState::Active => CapabilityState::Available,
                input::HookState::Failed => CapabilityState::Unavailable,
                input::HookState::NotStarted => CapabilityState::Unknown,
            },
            ocr: CapabilityState::from_measured(ocr.chosen.is_some()),
            ocr_language: ocr.chosen,
            ocr_languages_available: ocr.available,
        }
    }

    /// 寫給另一個行程看的那一份。見 [`sister_core::capabilities`]。
    ///
    /// 只帶原始事實過去，不帶結論——設定頁要拿它和**現在**的規則清單重算，
    /// 不然「上一場錄製開始時他還沒寫那條規則」會讓那一頁永遠沉默。
    ///
    /// 這一份是**開機探測**：`url_capture`、`browser_ticks`、`url_reads` 三個
    /// 都留在預設值，因為這一刻還沒有任何一場錄製發生過。錄製途中由
    /// `record` 迴圈反覆蓋掉（見 `sister_core::capabilities::write`），
    /// 那才是這個檔案不再凍在開機那一刻的地方。
    pub fn report(&self) -> sister_core::capabilities::Report {
        sister_core::capabilities::Report {
            at: now_ms(),
            url: self.url,
            input_hook: self.input,
            ..Default::default()
        }
    }

    /// 因為能力缺席而無法評估的隱私規則／錄製缺口。
    ///
    /// 和一般的功能缺口分開講：使用者可以接受「還不會 OCR」，但她必須知道
    /// 「UIA 不可用，所以錄製為了安全停在內容來源前」。這種事不能只寫在
    /// release note 裡。
    ///
    /// 判斷本身住在 [`sister_core::capabilities::Report`]：設定頁那個行程
    /// 沒有這裡的相依，卻要講出一模一樣的那句話。同一份判斷兩個地方寫，
    /// 遲早會變成兩句不一樣的話——而使用者會相信比較好聽的那一句。
    pub fn broken_privacy_rules(
        &self,
        privacy: &sister_core::config::PrivacyConfig,
    ) -> Vec<sister_core::capabilities::Broken> {
        self.report().broken_privacy_rules(privacy)
    }

    /// 看起來在運作、實際上不會有結果的地方。
    ///
    /// 和 [`Self::broken_privacy_rules`] 分開：前者講 privacy gate／網址規則的
    /// 可用性，這個講 OCR 等其他「其實什麼都沒記住」的功能缺口。
    pub fn silently_degraded(&self, config: &Config) -> Vec<String> {
        let mut out = Vec::new();
        if config.capture.ocr {
            match self.ocr {
                CapabilityState::Unavailable => out.push(
                    "這台機器沒有任何 OCR 語言：畫面會被記下來，但上面的字\
                     一個都不會進資料庫，搜尋永遠是空的"
                        .into(),
                ),
                CapabilityState::Available => {
                    if let Some(gap) = (ocr::OcrStatus {
                        available: self.ocr_languages_available.clone(),
                        chosen: self.ocr_language.clone(),
                    })
                    .cjk_gap()
                    {
                        out.push(gap);
                    }
                }
                // 沒量到不是「沒有 OCR」的證據。doctor 會把 Unknown 另列出來。
                CapabilityState::Unknown => {}
            }
        }
        out
    }
}

/// 組出 Windows 後端。刻意不公開：第三方不能拿 production composition 包一層
/// 再要求 trusted session。需要錄製只能走 [`recorder`]。
pub(crate) fn backend(config: &Config) -> Result<impl Backend + use<>> {
    enable_dpi_awareness();

    Ok(CompositeBackend {
        // 這只是診斷名稱；trusted provenance 不再放在 Backend 上，因此不能被
        // wrapper 經公開 trait 轉授。真正的票只在 `recorder` 裡建立。
        name: "windows-gdi-uia-focused-url-v2".to_owned(),
        system: system::WindowsSystem::new(),
        screen: screen::WindowsScreen::new(),
        focus: focus::WindowsFocus::new(),
        clipboard: clipboard::WindowsClipboard::new(),
        input: input::WindowsInput::start(now_ms(), config.capture.input_window_secs),
        // bench / doctor 直接用 raw WindowsOcr；只有長時間錄製包 changed-region
        // gate，兩種量測不會在型別相同的情況下不小心接反。
        ocr: ChangedRegionOcr::new(ocr::WindowsOcr::new(&config.capture.ocr_languages)),
    })
}

/// 建立唯一能替 focused-browser URL 背書的 production Windows recorder。
///
/// `Backend::name` 與 `Recorder::new` 都沒有 trusted seam；只有這個模組能造出
/// `WindowsBackendToken`，而 raw backend constructor 也沒有離開 crate。
pub fn recorder(
    config: Config,
    db: Db,
    image_dir: Option<PathBuf>,
    data_dir: PathBuf,
) -> Result<Recorder<impl Backend + use<>>> {
    let backend = backend(&config)?;
    Recorder::new_trusted_windows(
        backend,
        db,
        config,
        image_dir,
        crate::MasterStopSource::Latch(data_dir),
        backend_token(),
    )
}

/// 宣告自己認得 per-monitor DPI。
///
/// 不做這件事的話，在高 DPI 螢幕上 Windows 會餵給我們一張被系統放大過的
/// 模糊點陣圖，而不是原生像素。那張圖 OCR 幾乎讀不出字——縮放後的字緣
/// 全是插值出來的灰階。這一行直接決定文字品質。
fn enable_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    // 已經設過（或 manifest 裡設過）會失敗，那是正常的
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_data_dir(label: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sister-windows-recorder-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        dir
    }

    #[test]
    fn only_the_windows_module_token_mints_v2_url_provenance() {
        let identity = crate::backend_identity::BackendIdentity::trusted_windows(backend_token());
        assert_eq!(
            identity.session_platform(),
            sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM
        );
        assert!(identity.session_platform().ends_with("focused-url-v2"));
        assert_ne!(
            crate::CapturePermit::backend_local(7),
            crate::CapturePermit::windows(backend_token(), 7, 7, 7),
            "third-party backend-local permits must not cross the Windows permit boundary"
        );
    }

    #[test]
    #[ignore = "Windows CI runs process-global production composition in an isolated process"]
    fn production_composition_writes_v2_url_provenance() {
        let _input_lock = input::test_exclusive();
        let data_dir = temp_data_dir("provenance");
        let recorder = recorder(
            Config::default(),
            Db::open_in_memory().expect("db"),
            None,
            data_dir.clone(),
        )
        .expect("windows recorder");
        let platform: String = recorder
            .db()
            .conn()
            .query_row("SELECT platform FROM sessions WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("platform");
        assert_eq!(platform, sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    #[ignore = "Windows CI runs process-global production composition in an isolated process"]
    fn production_composition_blocks_on_the_real_data_dir_latch_before_capture() {
        let _input_lock = input::test_exclusive();
        let data_dir = temp_data_dir("master-stop");
        sister_hands::master_stop::engage(&data_dir, 1_000).expect("engage master stop");
        let mut recorder = recorder(
            Config::default(),
            Db::open_in_memory().expect("db"),
            None,
            data_dir.clone(),
        )
        .expect("windows recorder");

        assert_eq!(
            recorder.tick(2_000).expect("gated tick"),
            crate::Tick::MasterStopped
        );
        assert_eq!(recorder.stats().master_blocked_ticks, 1);
        assert_eq!(recorder.stats().working_ticks, 0);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    #[ignore = "Windows CI runs process-global production composition in an isolated process"]
    fn production_recorder_drop_drains_callbacks_before_stop_can_complete() {
        use std::sync::mpsc;
        use std::time::Duration;

        let _input_lock = input::test_exclusive();
        let data_dir = temp_data_dir("drop-drain");
        let mut recorder = recorder(
            Config::default(),
            Db::open_in_memory().expect("db"),
            None,
            data_dir.clone(),
        )
        .expect("windows recorder");
        assert!(
            !input::callback_gate_open_for_test(),
            "production construction must cold-start the callback gate closed"
        );

        // Use the exact admission helper called by a production tick, but do
        // not poll unrelated UIA/GDI/clipboard sources merely to build this
        // drop-order fixture. The first alpha.118 main run access-violated while
        // this new test and another mutex waiter were the only unfinished unit
        // tests; its full live-source tick was unrelated to the asserted fact.
        assert!(
            recorder.admit_master_input_for_test(),
            "production recorder did not retain its between-tick activity lease"
        );
        input::open_callback_gate_for_test();
        assert!(
            input::callback_gate_open_for_test(),
            "a clear admitted recorder must open callbacks"
        );

        let stop_dir = data_dir.clone();
        let (tx, rx) = mpsc::channel();
        let stopper = std::thread::spawn(move || {
            tx.send(sister_hands::master_stop::engage(&stop_dir, 1_000))
                .unwrap();
        });
        for _ in 0..500 {
            if data_dir.join("master.stop.pending").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(data_dir.join("master.stop.pending").exists());
        assert!(
            rx.recv_timeout(Duration::from_millis(40)).is_err(),
            "stop completed while the between-tick callback source was live"
        );

        // This is the same destruction path used by `windows_record` when a
        // maintenance `?` exits before the explicit finalize block.
        drop(recorder);
        rx.recv_timeout(Duration::from_secs(5))
            .expect("stop did not finish after recorder drop")
            .expect("engage after recorder drop");
        stopper.join().unwrap();
        assert!(
            !input::callback_gate_open_for_test(),
            "stop success must not precede callback suspension"
        );
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    /// 一台什麼都做得到的機器。測試從這裡出發，只改要測的那一項，
    /// 這樣斷言就不會受跑測試的那台機器裝了什麼影響。
    fn fully_capable() -> Capabilities {
        Capabilities {
            url: CapabilityState::Available,
            input: CapabilityState::Available,
            ocr: CapabilityState::Available,
            ocr_language: Some("zh-Hant-TW".into()),
            ocr_languages_available: vec!["zh-Hant-TW".into()],
        }
    }

    /// 缺 URL 擷取必須被說成隱私問題，不能只算功能缺口——
    /// 使用者設了網銀排除規則，她有權知道那些規則現在是空的。
    #[test]
    fn missing_url_capture_is_reported_as_a_privacy_gap() {
        let caps = Capabilities {
            url: CapabilityState::Unavailable,
            ..fully_capable()
        };
        let broken = caps.broken_privacy_rules(&Config::default().privacy);
        assert!(
            broken.iter().any(|w| w.message.contains("excluded_urls")),
            "沒有把失效的網址規則講出來：{broken:?}"
        );
    }

    #[test]
    fn a_fully_capable_backend_reports_nothing_broken() {
        let config = Config::default();
        assert!(
            fully_capable()
                .broken_privacy_rules(&config.privacy)
                .is_empty()
        );
        assert!(fully_capable().silently_degraded(&config).is_empty());
    }

    /// 沒有 OCR 的話，錄製看起來一切正常但搜尋永遠是空的。
    /// 這件事必須有人講出來。
    #[test]
    fn missing_ocr_is_reported_as_a_silent_failure() {
        let caps = Capabilities {
            ocr: CapabilityState::Unavailable,
            ocr_language: None,
            ocr_languages_available: vec![],
            ..fully_capable()
        };
        let warnings = caps.silently_degraded(&Config::default());
        assert!(
            warnings.iter().any(|w| w.contains("搜尋")),
            "沒有講出「搜尋會是空的」這個實際後果：{warnings:?}"
        );
    }

    /// 三態最重要的反面：探測沒有結果時，不能把三個 `Unknown`
    /// 壓成三個已經驗證的失敗。真 Windows 報告與 core 判決都要保留它。
    #[test]
    fn unmeasured_windows_capabilities_are_not_reported_as_missing() {
        let caps = Capabilities {
            url: CapabilityState::Unknown,
            input: CapabilityState::Unknown,
            ocr: CapabilityState::Unknown,
            ocr_language: None,
            ocr_languages_available: vec![],
        };
        let report = caps.report();
        assert_eq!(report.url, CapabilityState::Unknown);
        assert_eq!(report.input_hook, CapabilityState::Unknown);
        assert!(
            caps.broken_privacy_rules(&Config::default().privacy)
                .is_empty()
        );
        assert!(caps.silently_degraded(&Config::default()).is_empty());
    }

    /// 有 OCR 但只讀得懂英文，比完全沒有 OCR 更陰險：
    /// 英文介面讀得到，中文內容讀不到，看起來就只是「她沒記到那件事」。
    #[test]
    fn an_english_only_ocr_engine_is_reported_as_a_gap() {
        let caps = Capabilities {
            ocr: CapabilityState::Available,
            ocr_language: Some("en-US".into()),
            ocr_languages_available: vec!["en-US".into()],
            ..fully_capable()
        };
        let warnings = caps.silently_degraded(&Config::default());
        assert!(
            warnings.iter().any(|w| w.contains("中文")),
            "英文引擎讀不懂中文這件事沒有被講出來：{warnings:?}"
        );
    }

    /// 使用者自己把 OCR 關掉時，不該再嘮叨語言的事。
    #[test]
    fn disabling_ocr_on_purpose_is_not_a_warning() {
        let mut config = Config::default();
        config.capture.ocr = false;
        let caps = Capabilities {
            ocr: CapabilityState::Unavailable,
            ocr_language: None,
            ocr_languages_available: vec![],
            ..fully_capable()
        };
        assert!(caps.silently_degraded(&config).is_empty());
    }
}
