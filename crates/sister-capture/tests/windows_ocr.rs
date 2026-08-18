//! 在**真正的 Windows 上**跑一次 OCR。
//!
//! 這個檔案存在的理由是兩個沒辦法靠讀程式碼回答的問題：
//!
//! 1. **微軟的文件說 `Windows.Media.Ocr` 需要 package identity。**
//!    `OcrEngine` 明列在「WinRT APIs not supported in desktop apps」裡，
//!    也就是說我們這支沒有打包的 `sister.exe` 理論上不該叫得動它。
//!    實務上 ShareX、Text-Grab 都這樣用而且能用——但「別人說可以」不是一個
//!    可以寫進 PRIVACY.md 的保證。所以這裡真的建一次引擎、真的辨識一張圖。
//!
//! 2. **Windows 到底會不會把中文逐字切成一個一個「詞」？**
//!    [`sister_capture::ocr_layout`] 整個模組都建立在「有可能會」這個假設上。
//!    如果會，而我們直接用引擎給的 `OcrLine::Text()`，資料庫裡存的就會是
//!    「本 期 應 繳 金 額」，於是搜尋永遠是空的——而且沒有人會發現。
//!    這裡用一張真的圖驗一次：組回來的字串裡，「本期應繳金額」必須是連著的。
//!
//! 圖是用 Noto Sans CJK TC 產的（`assets/`，和 `doctor` 的自我測試共用
//! 同一張——那張是 `include_bytes!` 進執行檔的），不是螢幕截圖，所以它驗的是
//! 「這條管線通不通」，不是「OCR 準不準」。準確度要等到 Phase 0 的
//! 七天自錄才有真實答案。

#![cfg(windows)]

use sister_capture::ocr_layout;
use sister_capture::traits::{Ocr, RawFrame};
use sister_capture::windows::ocr::{OcrStatus, WindowsOcr};

const PREFERRED: &[&str] = &["zh-Hant-TW", "zh-Hant", "en-US"];

fn preferred() -> Vec<String> {
    PREFERRED.iter().map(|s| s.to_string()).collect()
}

fn load(name: &str) -> RawFrame {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/");
    let img = image::open(format!("{path}{name}"))
        .unwrap_or_else(|e| panic!("讀不到測試圖 {name}: {e}"))
        .to_rgba8();
    let (w, h) = img.dimensions();
    RawFrame::from_rgba(0, 0, w, h, img.into_raw())
}

fn text_of(blocks: &[sister_core::model::OcrBlock]) -> String {
    blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 沒有任何 OCR 語言時要不要讓測試紅。
///
/// 開發機上不該因為沒裝語言包就一直紅；但 CI 上「沒有 OCR」是一個必須被
/// 看見的事實，不能靜靜跳過——這整個檔案就是為了不要有靜靜跳過的東西。
fn skip_without_ocr(status: &OcrStatus) -> bool {
    if status.chosen.is_some() {
        return false;
    }
    let msg = format!(
        "這台機器沒有安裝任何 OCR 語言（已安裝：{:?}）",
        status.available
    );
    assert!(
        std::env::var("CI").is_err(),
        "{msg}——CI 上這代表 OCR 完全沒有被驗證過"
    );
    eprintln!("⚠ 跳過：{msg}");
    true
}

/// 最根本的那一題：這支沒有打包的執行檔叫得動系統的 OCR 嗎。
#[test]
fn ocr_runs_at_all_from_an_unpackaged_exe() {
    let status = OcrStatus::probe(&preferred());
    eprintln!(
        "OCR 語言：選中 {:?}，已安裝 {:?}",
        status.chosen, status.available
    );
    if skip_without_ocr(&status) {
        return;
    }

    let mut ocr = WindowsOcr::new(&preferred());
    assert!(ocr.is_available(), "探測說有 OCR，但引擎建不起來");

    let blocks = ocr
        .recognize(&load("ocr-selftest-en.png"))
        .expect("OCR 失敗");
    let text = text_of(&blocks);
    eprintln!("--- 英文辨識結果 ---\n{text}\n---");

    assert!(!blocks.is_empty(), "一行都沒讀到");
    for needle in ["13,450", "0800"] {
        assert!(
            text.contains(needle),
            "讀不到「{needle}」，實際讀到的是：\n{text}"
        );
    }
}

/// 中文那一題：組回來的字裡，詞不可以被空白切開。
///
/// 這條斷言不管 Windows 有沒有逐字切都成立，而且兩種情況它都擋得住：
/// - 如果引擎逐字切，`assemble_line` 沒做對 → 這裡會紅
/// - 如果引擎本來就給整行 → 這裡照樣該綠
#[test]
fn chinese_comes_back_as_words_not_scattered_characters() {
    let status = OcrStatus::probe(&preferred());
    if skip_without_ocr(&status) {
        return;
    }
    let Some(lang) = status.chosen.as_deref() else {
        return;
    };
    if !lang.starts_with("zh") {
        // 這是 GitHub runner 的常態：只裝了英文。中文讀不出來不是我們的 bug，
        // 但也絕對不能假裝驗過了。
        eprintln!("⚠ 跳過中文驗證：這台機器的 OCR 語言是 {lang}，不是中文");
        return;
    }

    let mut ocr = WindowsOcr::new(&preferred());
    let blocks = ocr
        .recognize(&load("ocr-selftest-zh.png"))
        .expect("OCR 失敗");
    let text = text_of(&blocks);
    eprintln!("--- 中文辨識結果 ---\n{text}\n---");

    assert!(
        text.contains("本期應繳金額"),
        "「本期應繳金額」沒有連在一起——這正是會讓搜尋永遠是空的那個 bug。\
         實際讀到的是：\n{text}"
    );
    assert!(
        text.contains("客服專線"),
        "讀不到「客服專線」，實際讀到的是：\n{text}"
    );

    // 讀得到字還不夠，要能變成事實才算通到底
    let facts = sister_core::facts::extract(&text);
    assert!(
        facts
            .iter()
            .any(|f| f.kind == sister_core::facts::FactKind::Phone),
        "辨識出來的文字抽不出電話，L1 等於沒接上：\n{text}"
    );
}

/// **整個螢幕那麼大的一張圖，讀不讀得到字。**
///
/// 這條是實測逼出來的：CI 上小圖過了、`doctor` 說 OCR ✓、真的錄一分鐘卻
/// 一個字都沒進資料庫。小圖與整張畫面之間差了什麼，只有真的送一張整張
/// 畫面大小的圖進去才知道——`MaxImageDimension`、5MB 的 buffer、
/// 5:3 的長寬比，這些在 900×300 的 fixture 上全都碰不到。
///
/// 尺寸取 **2560×1440**，因為那就是實際送進 OCR 的東西：OCR 吃的是原生
/// 解析度的像素，不是存檔用的縮圖。這個測試原本用 1568×882（`max_long_edge`
/// 的預設值），而那正好是出事的那一版——把 2560 縮成 1568 是 0.61 倍，
/// 12px 的字掉到 7px，於是 `Microsoft Teams` 被讀成 `Micr099ftTeamsTr`。
/// 用縮過的尺寸測試，等於測了一條production 已經不再走的路。
#[test]
fn a_full_screen_sized_frame_still_yields_text() {
    let status = OcrStatus::probe(&preferred());
    if skip_without_ocr(&status) {
        return;
    }

    // 把 fixture 貼在白底的 2560×1440 上，其餘留白。
    // 內容一樣、尺寸變成真實畫面那麼大——變因只有一個。
    let src = image::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/ocr-selftest-en.png"
    ))
    .expect("讀不到測試圖")
    .to_rgba8();
    let (sw, sh) = src.dimensions();
    let (w, h) = (2560u32, 1440u32);
    assert!(sw <= w && sh <= h, "fixture {sw}x{sh} 比目標畫布還大");

    let mut canvas = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
    image::imageops::replace(&mut canvas, &src, 40, 40);

    let mut ocr = WindowsOcr::new(&preferred());
    eprintln!("引擎回報的單邊上限：{}", ocr.max_dimension());
    let blocks = ocr
        .recognize(&RawFrame::from_rgba(0, 0, w, h, canvas.into_raw()))
        .expect("整張畫面大小的圖辨識失敗");
    let text = text_of(&blocks);
    eprintln!(
        "--- {w}x{h} 辨識結果（{} 行）---\n{text}\n---",
        blocks.len()
    );

    assert!(
        text.contains("13,450"),
        "小圖讀得到、{w}×{h} 讀不到——這正是錄製時一個字都沒有的原因。\
         實際讀到的是：\n{text}"
    );
}

/// 太小的圖 Windows 會安靜地回傳零行。我們必須擋在前面，
/// 而且不可以 panic、不可以把它報成錯誤。
#[test]
fn tiny_images_are_skipped_not_crashed() {
    let status = OcrStatus::probe(&preferred());
    if skip_without_ocr(&status) {
        return;
    }
    let mut ocr = WindowsOcr::new(&preferred());
    let frame = RawFrame::from_rgba(0, 0, 8, 8, vec![255u8; 8 * 8 * 4]);
    assert_eq!(ocr.recognize(&frame).expect("不該報錯").len(), 0);
}

/// 沒有像素的幀不該讓 OCR panic——但也**不該被當成「這張畫面上沒有字」**。
///
/// 這條測試原本斷言的是 `Ok(空的)`，理由是「不該出事」。那個理由對，
/// 但那個回傳值把一個 bug（擷取端交出一張沒有像素的畫面）翻譯成了一句
/// 平靜的事實陳述，於是 `ocr_failures` 是 0、摘要全綠、資料庫是空的。
///
/// 現在斷言 `Err`：錄製照樣不會停（recorder 接住它、保留畫面），
/// 差別只在於它說得出原因。
#[test]
fn a_frame_without_pixels_is_an_error_not_an_empty_page() {
    let mut ocr = WindowsOcr::new(&preferred());
    let frame = RawFrame {
        ts: 0,
        monitor: 0,
        width: 100,
        height: 100,
        rgba: None,
        dhash: 0,
    };
    let err = ocr.recognize(&frame).expect_err("沒有像素不是「沒有字」");
    // 訊息要說得出是哪一種壞法：使用者貼一行回來就要能定位
    assert!(format!("{err:#}").contains("像素"), "{err:#}");
}

/// 這條在所有平台都成立，放這裡是為了讓上面的中文斷言有個對照組：
/// 如果哪天 `assemble_line` 被改壞，這裡會先紅，而且不需要 Windows。
#[test]
fn the_assembly_rule_itself_still_holds() {
    let words: Vec<_> = "本期應繳金額"
        .chars()
        .enumerate()
        .map(|(i, c)| {
            ocr_layout::Word::new(&c.to_string(), 10.0 + i as f32 * 22.0, 0.0, 20.0, 20.0)
        })
        .collect();
    assert_eq!(
        ocr_layout::assemble_line(&words).unwrap().text,
        "本期應繳金額"
    );
}
