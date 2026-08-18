//! 幀去重：dHash + Hamming 距離。
//!
//! 這是整個成本模型的第一道閘門（SPEC §2.2）。畫面沒變就不存、不 OCR、
//! 不進索引——一天 8 小時裡絕大多數的十秒都長得一模一樣，這一層擋掉它們，
//! 後面所有層才有可能便宜。
//!
//! 刻意不依賴 `image_hasher`：演算法只有二十行，自己實作可以直接吃
//! 平台層給的灰階緩衝區，讓 core 完全沒有影像處理相依。

/// 對灰階緩衝區計算 64-bit dHash（difference hash）。
///
/// 作法：box-average 降採樣到 9×8，比較每列相鄰像素的亮度，
/// 左 > 右 記 1。對整體亮度變化免疫，對版面變化敏感——正好是螢幕擷取要的性質。
///
/// `gray` 長度不足 `w*h` 時取可用範圍，不會 panic。
pub fn dhash_gray(gray: &[u8], w: u32, h: u32) -> u64 {
    const DW: u32 = 9;
    const DH: u32 = 8;

    if w == 0 || h == 0 || gray.is_empty() {
        return 0;
    }

    let mut small = [0u8; (DW * DH) as usize];

    for cy in 0..DH {
        // 這一格在原圖對應的 y 範圍
        let y0 = (cy as u64 * h as u64 / DH as u64) as u32;
        let y1 = (((cy + 1) as u64 * h as u64 / DH as u64) as u32)
            .max(y0 + 1)
            .min(h);

        for cx in 0..DW {
            let x0 = (cx as u64 * w as u64 / DW as u64) as u32;
            let x1 = (((cx + 1) as u64 * w as u64 / DW as u64) as u32)
                .max(x0 + 1)
                .min(w);

            let mut sum: u64 = 0;
            let mut count: u64 = 0;
            for y in y0..y1 {
                let row = y as usize * w as usize;
                for x in x0..x1 {
                    if let Some(px) = gray.get(row + x as usize) {
                        sum += *px as u64;
                        count += 1;
                    }
                }
            }
            small[(cy * DW + cx) as usize] = (sum / count.max(1)) as u8;
        }
    }

    let mut hash: u64 = 0;
    let mut bit = 0;
    for cy in 0..DH {
        for cx in 0..(DW - 1) {
            let left = small[(cy * DW + cx) as usize];
            let right = small[(cy * DW + cx + 1) as usize];
            if left > right {
                hash |= 1 << bit;
            }
            bit += 1;
        }
    }
    hash
}

/// 由 RGB/RGBA 緩衝區計算 dHash（先轉灰階）。`stride_bytes` 為每像素位元組數（3 或 4）。
pub fn dhash_rgb(buf: &[u8], w: u32, h: u32, stride_bytes: usize) -> u64 {
    if stride_bytes == 0 {
        return 0;
    }
    let mut gray = Vec::with_capacity((w as usize).saturating_mul(h as usize));
    for px in buf.chunks_exact(stride_bytes) {
        // BT.601 luma，整數運算避免浮點成本
        let y = (px[0] as u32 * 77 + px[1] as u32 * 150 + px[2] as u32 * 29) >> 8;
        gray.push(y as u8);
    }
    dhash_gray(&gray, w, h)
}

/// 兩個 hash 的 Hamming 距離（0 = 完全相同）。
#[inline]
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 螢幕畫面的預設「同一畫面」門檻。
///
/// 研究建議 4–8（一般照片的經典建議 >10 才算不同，但螢幕上一行文字的
/// 差異很小，門檻必須更嚴，否則會漏掉真正的內容變化）。
pub const DEFAULT_THRESHOLD: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameVerdict {
    /// 畫面有變，應保留、OCR、入索引。
    New,
    /// 與前一張保留幀相同；`run` 是已連續重複的張數。
    Duplicate { run: u32 },
}

/// 串流去重器。持有「最後一張被保留的幀」的 hash。
#[derive(Debug, Clone)]
pub struct Deduper {
    threshold: u32,
    last_kept: Option<u64>,
    run: u32,
}

impl Deduper {
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold,
            last_kept: None,
            run: 0,
        }
    }

    pub fn threshold(&self) -> u32 {
        self.threshold
    }

    /// 目前連續重複張數。
    pub fn run(&self) -> u32 {
        self.run
    }

    /// 判斷一張新幀。判定為 `New` 時，內部狀態會更新為以它為基準。
    pub fn check(&mut self, hash: u64) -> FrameVerdict {
        match self.last_kept {
            Some(prev) if hamming(prev, hash) <= self.threshold => {
                self.run += 1;
                FrameVerdict::Duplicate { run: self.run }
            }
            _ => {
                self.last_kept = Some(hash);
                self.run = 0;
                FrameVerdict::New
            }
        }
    }

    /// 強制把下一張視為新幀（例如剛從 pause 恢復、或跨越 idle 心跳）。
    pub fn reset(&mut self) {
        self.last_kept = None;
        self.run = 0;
    }
}

impl Default for Deduper {
    fn default() -> Self {
        Self::new(DEFAULT_THRESHOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, v: u8) -> Vec<u8> {
        vec![v; (w * h) as usize]
    }

    /// 左半黑右半白的圖，是 dHash 的經典可預測輸入。
    fn half_split(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h) as usize);
        for _ in 0..h {
            for x in 0..w {
                v.push(if x < w / 2 { 0 } else { 255 });
            }
        }
        v
    }

    #[test]
    fn solid_image_has_zero_hash() {
        // 全部一樣亮 → 沒有任何 left > right → 全 0
        assert_eq!(dhash_gray(&solid(64, 64, 128), 64, 64), 0);
        assert_eq!(dhash_gray(&solid(64, 64, 0), 64, 64), 0);
    }

    #[test]
    fn brightness_shift_does_not_change_hash() {
        // dHash 對整體亮度免疫：這正是我們要的（螢幕調暗不該被當成新畫面）
        let a = dhash_gray(&half_split(64, 64), 64, 64);
        let dim: Vec<u8> = half_split(64, 64).iter().map(|p| p / 2).collect();
        let b = dhash_gray(&dim, 64, 64);
        assert_eq!(a, b, "brightness change must not alter dhash");
    }

    #[test]
    fn different_layouts_differ_a_lot() {
        let a = dhash_gray(&half_split(64, 64), 64, 64);
        let mut mirrored = Vec::new();
        for _ in 0..64 {
            for x in 0..64 {
                mirrored.push(if x < 32 { 255 } else { 0 });
            }
        }
        let b = dhash_gray(&mirrored, 64, 64);
        assert!(
            hamming(a, b) > 5,
            "mirrored layout should be far: {}",
            hamming(a, b)
        );
    }

    #[test]
    fn empty_and_degenerate_inputs_do_not_panic() {
        assert_eq!(dhash_gray(&[], 0, 0), 0);
        // 宣稱 100x100 卻只給 3 個 byte：落在緩衝區外的取樣格算 0，
        // 不 panic 也不讀到別人的記憶體。只有第一格取得到值，因此結果非零。
        let truncated = dhash_gray(&[1, 2, 3], 100, 100);
        assert_eq!(truncated, 1, "only the first sample cell has data");
        // 比宣告尺寸短的緩衝區不能 panic
        let _ = dhash_gray(&solid(4, 4, 200), 64, 64);
        // 小於 9x8 的圖也要能算
        let _ = dhash_gray(&half_split(3, 2), 3, 2);
    }

    #[test]
    fn rgb_matches_gray_for_neutral_colors() {
        // 灰階色 (v,v,v) 轉出來的亮度應該非常接近 v，hash 要一致
        let gray = half_split(32, 32);
        let mut rgb = Vec::new();
        for px in &gray {
            rgb.extend_from_slice(&[*px, *px, *px, 255]);
        }
        assert_eq!(dhash_gray(&gray, 32, 32), dhash_rgb(&rgb, 32, 32, 4));
    }

    #[test]
    fn hamming_basics() {
        assert_eq!(hamming(0, 0), 0);
        assert_eq!(hamming(0b1011, 0b1000), 2);
        assert_eq!(hamming(u64::MAX, 0), 64);
    }

    #[test]
    fn deduper_collapses_runs_and_counts_them() {
        let mut d = Deduper::new(5);
        assert_eq!(d.check(0xFFFF_0000_FFFF_0000), FrameVerdict::New);
        assert_eq!(
            d.check(0xFFFF_0000_FFFF_0000),
            FrameVerdict::Duplicate { run: 1 }
        );
        assert_eq!(
            d.check(0xFFFF_0000_FFFF_0000),
            FrameVerdict::Duplicate { run: 2 }
        );
        assert_eq!(d.run(), 2);

        // 差 1 bit，在門檻內 → 仍算同一畫面
        assert_eq!(
            d.check(0xFFFF_0000_FFFF_0001),
            FrameVerdict::Duplicate { run: 3 }
        );

        // 差很多 → 新畫面，run 歸零
        assert_eq!(d.check(0x0000_FFFF_0000_FFFF), FrameVerdict::New);
        assert_eq!(d.run(), 0);
    }

    #[test]
    fn deduper_threshold_is_respected() {
        // 門檻 0 = 只有完全相同才算重複
        let mut strict = Deduper::new(0);
        assert_eq!(strict.check(0b0), FrameVerdict::New);
        assert_eq!(strict.check(0b1), FrameVerdict::New);

        let mut loose = Deduper::new(64);
        assert_eq!(loose.check(0), FrameVerdict::New);
        assert_eq!(loose.check(u64::MAX), FrameVerdict::Duplicate { run: 1 });
    }

    #[test]
    fn reset_forces_next_frame_to_be_new() {
        let mut d = Deduper::default();
        assert_eq!(d.check(42), FrameVerdict::New);
        assert_eq!(d.check(42), FrameVerdict::Duplicate { run: 1 });
        d.reset();
        assert_eq!(
            d.check(42),
            FrameVerdict::New,
            "after reset the same frame must count as new"
        );
        assert_eq!(d.run(), 0);
    }
}
