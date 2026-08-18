//! 畫面檔的降採樣、編碼與路徑配置。
//!
//! 抽成獨立模組是為了讓「一天佔多少磁碟」這件事能被單獨測量與調整——
//! Phase 0 的退出條件之一就是每天 < 300MB。

use anyhow::{Context, Result};
use image::{ImageEncoder, codecs::png::PngEncoder};
use sister_core::model::Millis;
use std::path::{Path, PathBuf};

/// 降採樣到長邊不超過 `max_long_edge`，再編碼成 PNG。
///
/// 為什麼是 PNG 而不是 WebP：螢幕畫面是大片純色加文字，PNG 的無損壓縮
/// 在這種內容上又小又快，而且文字邊緣不會被有損壓縮糊掉——這些畫面是
/// 給 OCR 與人眼複查用的證據，不是給人欣賞的照片。
pub fn encode_downscaled(
    rgba: &[u8],
    width: u32,
    height: u32,
    max_long_edge: u32,
) -> Result<Vec<u8>> {
    anyhow::ensure!(width > 0 && height > 0, "frame has zero dimension");
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .context("frame dimensions overflow")?;
    anyhow::ensure!(
        rgba.len() >= expected,
        "rgba buffer too small: {} < {expected}",
        rgba.len()
    );

    let img = image::RgbaImage::from_raw(width, height, rgba[..expected].to_vec())
        .context("build image from rgba")?;

    let long_edge = width.max(height);
    let img = if max_long_edge > 0 && long_edge > max_long_edge {
        let scale = max_long_edge as f32 / long_edge as f32;
        let (w, h) = (
            ((width as f32 * scale).round() as u32).max(1),
            ((height as f32 * scale).round() as u32).max(1),
        );
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };

    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("encode png")?;
    Ok(out)
}

/// 畫面檔根目錄。
pub fn frames_root(data_dir: &Path) -> PathBuf {
    data_dir.join("frames")
}

/// 畫面檔的相對路徑：`YYYY/MM/DD/<ts>-<monitor>.png`。
///
/// 按日曆分層而不是用時間戳雜湊：使用者必須能自己打開資料夾、看見
/// 「這天她記了什麼」，並且能直接刪掉某一天。可稽核性優先於整齊。
pub fn relative_path(ts: Millis, monitor: i32) -> String {
    let (y, m, d) = civil_from_epoch_ms(ts);
    format!("{y:04}/{m:02}/{d:02}/{ts}-{monitor}.png")
}

/// Unix 毫秒 → (年, 月, 日) UTC。
///
/// 用 Howard Hinnant 的 civil_from_days 演算法自己算，免去為了三個
/// 整數而背上一個日期函式庫。
fn civil_from_epoch_ms(ms: Millis) -> (i64, u32, u32) {
    let days = ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let c = ((x * 7 + y * 13) % 256) as u8;
                v.extend_from_slice(&[c, c / 2, 255 - c, 255]);
            }
        }
        v
    }

    #[test]
    fn encodes_a_valid_png() {
        let png = encode_downscaled(&rgba(64, 48), 64, 48, 0).expect("encode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "must be a real PNG");
        let decoded = image::load_from_memory(&png).expect("decode");
        assert_eq!((decoded.width(), decoded.height()), (64, 48));
    }

    #[test]
    fn downscales_the_long_edge_and_keeps_aspect_ratio() {
        let png = encode_downscaled(&rgba(160, 100), 160, 100, 80).expect("encode");
        let d = image::load_from_memory(&png).expect("decode");
        assert_eq!(d.width(), 80);
        assert_eq!(d.height(), 50);
    }

    #[test]
    fn small_frames_are_left_alone() {
        let png = encode_downscaled(&rgba(40, 30), 40, 30, 1568).expect("encode");
        let d = image::load_from_memory(&png).expect("decode");
        assert_eq!((d.width(), d.height()), (40, 30));
    }

    #[test]
    fn downscaling_actually_saves_space() {
        // 這條路徑是磁碟預算的來源，不能只是「有跑」而已
        let big = encode_downscaled(&rgba(400, 400), 400, 400, 0).expect("encode");
        let small = encode_downscaled(&rgba(400, 400), 400, 400, 100).expect("encode");
        assert!(
            small.len() * 4 < big.len(),
            "{} vs {}",
            small.len(),
            big.len()
        );
    }

    #[test]
    fn malformed_input_errors_instead_of_panicking() {
        assert!(encode_downscaled(&[], 0, 0, 100).is_err());
        assert!(
            encode_downscaled(&[1, 2, 3], 64, 64, 100).is_err(),
            "buffer too small"
        );
    }

    #[test]
    fn paths_are_calendar_shaped_so_a_day_can_be_deleted() {
        // 2026-08-17T00:00:00Z
        assert_eq!(
            relative_path(1_786_924_800_000, 0),
            "2026/08/17/1786924800000-0.png"
        );
        // epoch
        assert_eq!(relative_path(0, 1), "1970/01/01/0-1.png");
    }

    #[test]
    fn civil_dates_are_correct_across_leap_years_and_epochs() {
        assert_eq!(civil_from_epoch_ms(0), (1970, 1, 1));
        assert_eq!(civil_from_epoch_ms(86_400_000 * 59), (1970, 3, 1));
        // 2000-02-29（世紀閏年）與 2100-03-01（非閏年）
        assert_eq!(civil_from_epoch_ms(951_782_400_000), (2000, 2, 29));
        assert_eq!(civil_from_epoch_ms(4_107_542_400_000), (2100, 3, 1));
        assert_eq!(civil_from_epoch_ms(1_786_924_800_000), (2026, 8, 17));
    }
}
