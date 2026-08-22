//! 擷取畫面的等比縮圖規則，以及 Windows OCR 可接受的最小短邊。

/// OCR 擷取路徑允許的長邊上限。
///
/// 這不是畫質旋鈕，是一道防線，所以刻意寫死而不是開成設定：8K 螢幕一張
/// RGBA 是 132MB，而 Windows 的 OCR 引擎本身也只吃到 10000 像素。4096
/// 讓 4K（3840）以下的螢幕全部拿到**原生**像素——也就是絕大多數人——
/// 更大的才縮，而且縮一半仍然遠好過為了它把每個人都降到 1568。
pub const OCR_LONG_EDGE: u32 = 4096;

/// 把 `w×h` 等比縮到長邊不超過 `max`。本來就夠小就原樣返回。
///
/// 零維度也會原樣穿過；實際擷取的呼叫端必須先拒絕無效尺寸。
pub fn fit(w: u32, h: u32, max: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max {
        return (w, h);
    }
    let scale = max as f64 / long as f64;
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

/// 短邊低於這個值，Windows 的 OCR 會回傳零行而且不報錯。
///
/// 官方沒有文件，這個數字來自社群長期的回報（也和 Windows 8.1 前身 API
/// 文件裡寫死的 40～2600 對得上）。
pub const OCR_MIN_SHORT_EDGE: u32 = 40;

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fit_leaves_small_images_alone() {
        assert_eq!(fit(800, 600, 1568), (800, 600));
        assert_eq!(fit(1568, 900, 1568), (1568, 900));
    }

    #[test]
    fn fit_scales_by_the_long_edge_and_keeps_aspect() {
        // 4K 橫向。
        assert_eq!(fit(3840, 2160, 1568), (1568, 882));
        // 直立螢幕：長邊是高度。
        assert_eq!(fit(2160, 3840, 1568), (882, 1568));
        // 正方形。
        assert_eq!(fit(4000, 4000, 1000), (1000, 1000));
    }

    #[test]
    fn fit_keeps_scaled_dimensions_nonzero_but_passes_input_zero_through() {
        // 有效的極端長寬比縮完仍至少是一個像素。
        let (w, h) = fit(10_000, 3, 100);
        assert!(w >= 1 && h >= 1, "got {w}x{h}");
        assert_eq!(fit(0, 50, 100), (0, 50));
        assert_eq!(fit(0, 0, 100), (0, 0));
    }

    #[test]
    fn fit_rounds_to_the_nearest_pixel() {
        assert_eq!(fit(1000, 335, 100), (100, 34));
        assert_eq!(fit(1000, 333, 100), (100, 33));
    }
}
