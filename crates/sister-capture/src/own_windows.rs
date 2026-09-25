//! 她自己的視窗在畫面上的那幾塊。
//!
//! 前景是她的時候，那一拍整個不錄（`PrivacyConfig::check` 的 `OwnWindow`）。
//! 這裡管的是另一格：她看得見、但焦點在別的程式——桌寵釘在最上層、時間軸
//! 開在旁邊。那一拍照樣擷取，只是在算 dhash、跑 OCR、存圖之前，先把她那幾塊
//! 塗黑。
//!
//! 平台層只讀原始事實（[`WindowFacts`]），判斷全在這裡，Linux 的 `cargo test`
//! 就驗得到。兩個方向的錯代價不對稱：塗多了，是別的程式那一小塊這一拍少記；
//! 塗少了，是她把自己的答案錄進自己的記憶。所以讀不準的一律往「塗」那邊倒；
//! 唯一反過來的是「別人的視窗擋不擋得住她」——確定擋得住才不塗，讀不準就當
//! 擋不住。

use sister_core::config::OWN_APP_KEYS;

/// 桌面座標（實體像素），半開區間：`left <= x < right`、`top <= y < bottom`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DesktopRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl DesktopRect {
    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    fn intersect(&self, other: &DesktopRect) -> Option<DesktopRect> {
        let both = DesktopRect {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (!both.is_empty()).then_some(both)
    }

    /// `self` 扣掉 `cut` 剩下的部分，切成最多四塊、彼此不重疊。
    fn subtract(&self, cut: &DesktopRect) -> Vec<DesktopRect> {
        let Some(hole) = self.intersect(cut) else {
            return vec![*self];
        };
        let bands = [
            // 洞上面、洞下面：整條寬。
            DesktopRect {
                bottom: hole.top,
                ..*self
            },
            DesktopRect {
                top: hole.bottom,
                ..*self
            },
            // 洞左邊、洞右邊：只到洞的高度。
            DesktopRect {
                top: hole.top,
                right: hole.left,
                bottom: hole.bottom,
                ..*self
            },
            DesktopRect {
                left: hole.right,
                top: hole.top,
                bottom: hole.bottom,
                ..*self
            },
        ];
        bands.into_iter().filter(|band| !band.is_empty()).collect()
    }
}

/// 平台層對一扇頂層視窗讀到的原始事實。讀不到的一律填 `None`，不要猜。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowFacts {
    /// 擁有這扇視窗的程式檔名，只有檔名、不含路徑。
    pub exe_name: Option<String>,
    /// `IsWindowVisible`。
    pub visible: bool,
    /// `IsIconic`。
    pub minimized: bool,
    /// DWM 把它藏起來了（另一個虛擬桌面、暫停中的 UWP）。
    pub cloaked: Option<bool>,
    /// 它可能是半透明的，見 [`layered_is_see_through`]。
    pub see_through: bool,
    /// 它有 `SetWindowRgn` 設的不規則形狀。
    pub shaped: bool,
    /// `GetWindowRect`：Windows 10 起含一圈看不見的縮放邊。
    pub window_rect: Option<DesktopRect>,
    /// `DWMWA_EXTENDED_FRAME_BOUNDS`：畫面上真的看得到的外框。
    pub frame_bounds: Option<DesktopRect>,
}

/// `GetLayeredWindowAttributes` 讀回來的東西。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayeredAttributes {
    /// `LWA_COLORKEY`：某個顏色整片透明。
    pub color_key: bool,
    /// `LWA_ALPHA` 有設才是 `Some`。
    pub alpha: Option<u8>,
}

/// 她的一扇視窗看得見，卻讀不到它在哪裡。這一拍整張不抓。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnWindowUnlocated;

impl std::fmt::Display for OwnWindowUnlocated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("她自己的一扇視窗開著，但讀不到它在畫面上的位置，這一拍沒有抓畫面")
    }
}

impl std::error::Error for OwnWindowUnlocated {}

/// 這個程式檔是不是她。和前景那道閘門（`PrivacyConfig::check`）認的是同一份
/// 名單；Windows 的檔名不分大小寫，所以先轉小寫再整串比。
pub fn is_her_program(exe_name: Option<&str>) -> bool {
    exe_name.is_some_and(|name| OWN_APP_KEYS.contains(&name.to_ascii_lowercase().as_str()))
}

/// 一扇別人的 layered 視窗能不能被當成「擋得住她」。
///
/// layered 視窗可以整片半透明、逐像素透明、或某個顏色透明，而那三種底下都
/// 看得到她。只有讀得到屬性、沒有色鍵、alpha 沒設或設成 255 的，才確定是實心。
/// `attributes` 是 `None`＝讀不到（`UpdateLayeredWindow` 的逐像素透明就讀不到）。
pub fn layered_is_see_through(layered: bool, attributes: Option<LayeredAttributes>) -> bool {
    if !layered {
        return false;
    }
    match attributes {
        None => true,
        Some(attributes) => {
            attributes.color_key || attributes.alpha.is_some_and(|alpha| alpha != 255)
        }
    }
}

/// 她看得見的那幾塊，桌面座標。`windows_top_first` 照 z-order 由上往下排
/// （`EnumWindows` 給的順序）。
///
/// - 沒畫出來的（隱藏、縮到最小）不管是誰都跳過。
/// - 她的：`window_rect` 和 `frame_bounds` 兩塊都塗；被 DWM 藏起來、透明、
///   形狀不規則都照塗。兩塊都讀不到就整張不抓——少塗一扇就是把那一扇錄進去。
/// - 別人的：只有確定蓋得住的才擋住她，見 [`covers`]。
/// - 她自己的視窗不互相擋。
pub fn own_parts(
    windows_top_first: &[WindowFacts],
) -> Result<Vec<DesktopRect>, OwnWindowUnlocated> {
    let mut above: Vec<DesktopRect> = Vec::new();
    let mut parts = Vec::new();
    for window in windows_top_first {
        if !window.visible || window.minimized {
            continue;
        }
        if is_her_program(window.exe_name.as_deref()) {
            let bounds: Vec<DesktopRect> = [window.window_rect, window.frame_bounds]
                .into_iter()
                .flatten()
                .collect();
            if bounds.is_empty() {
                return Err(OwnWindowUnlocated);
            }
            for bound in bounds {
                let mut pieces: Vec<DesktopRect> =
                    std::iter::once(bound).filter(|b| !b.is_empty()).collect();
                for cut in &above {
                    pieces = pieces
                        .iter()
                        .flat_map(|piece| piece.subtract(cut))
                        .collect();
                }
                parts.extend(pieces);
            }
        } else if let Some(covers) = covers(window) {
            above.push(covers);
        }
    }
    Ok(parts)
}

/// 別人的一扇視窗確定蓋得住的範圍：沒被藏（讀得到而且是否）、不透明、方的，
/// 而且只算 `frame_bounds`——`window_rect` 多出來的那圈縮放邊底下看得到她。
fn covers(window: &WindowFacts) -> Option<DesktopRect> {
    if window.cloaked != Some(false) || window.see_through || window.shaped {
        return None;
    }
    window.frame_bounds.filter(|bounds| !bounds.is_empty())
}

/// 把 `parts` 在擷取畫面上塗成 (0, 0, 0, 255)，回傳塗到的像素數（重疊只算一次）。
///
/// `rgba` 是 `frame_w × frame_h` 的 RGBA8、由上往下；`captured` 是這張畫面在
/// 桌面上的範圍。畫面比擷取範圍小（縮過圖）時，每一塊先切到擷取範圍裡、往外
/// 取整，再各邊多塗一格：縮圖把鄰近的來源像素混在一起，邊上那一格可能帶著她
/// 的顏色。切掉之後是空的就不塗——她在隔壁螢幕上，不會多出一條黑邊。
///
/// 輸入對不起來（長度不對、擷取範圍或畫面是空的）回 `None`，一個位元組都不動。
pub fn blank(
    rgba: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    captured: DesktopRect,
    parts: &[DesktopRect],
) -> Option<u64> {
    if frame_w == 0 || frame_h == 0 || captured.is_empty() {
        return None;
    }
    let pixels = (frame_w as usize).checked_mul(frame_h as usize)?;
    if rgba.len() != pixels.checked_mul(4)? {
        return None;
    }
    let (fw, fh) = (i64::from(frame_w), i64::from(frame_h));
    let src_w = i64::from(captured.right) - i64::from(captured.left);
    let src_h = i64::from(captured.bottom) - i64::from(captured.top);
    let grow = i64::from(fw != src_w || fh != src_h);

    // 每一塊換成畫面座標的 [x0, x1) × [y0, y1)。
    let mapped: Vec<(i64, i64, i64, i64)> = parts
        .iter()
        .filter_map(|part| part.intersect(&captured))
        .map(|clip| {
            let a = i64::from(clip.left) - i64::from(captured.left);
            let b = i64::from(clip.right) - i64::from(captured.left);
            let c = i64::from(clip.top) - i64::from(captured.top);
            let d = i64::from(clip.bottom) - i64::from(captured.top);
            // a..d 都 >= 0、分母 > 0：一般的整數除法就是 floor。
            let x0 = a * fw / src_w - grow;
            let x1 = (b * fw + src_w - 1) / src_w + grow;
            let y0 = c * fh / src_h - grow;
            let y1 = (d * fh + src_h - 1) / src_h + grow;
            (x0.max(0), y0.max(0), x1.min(fw), y1.min(fh))
        })
        .filter(|(x0, y0, x1, y1)| x0 < x1 && y0 < y1)
        .collect();

    let mut blanked = 0u64;
    let mut spans: Vec<(i64, i64)> = Vec::new();
    for y in 0..fh {
        spans.clear();
        spans.extend(
            mapped
                .iter()
                .filter(|(_, y0, _, y1)| *y0 <= y && y < *y1)
                .map(|(x0, _, x1, _)| (*x0, *x1)),
        );
        if spans.is_empty() {
            continue;
        }
        spans.sort_unstable();
        let mut end = i64::MIN;
        for &(start, stop) in &spans {
            // 已經塗過的那一段不重算。
            let from = start.max(end);
            if from < stop {
                let row = (y * fw) as usize;
                for x in from..stop {
                    let at = (row + x as usize) * 4;
                    rgba[at..at + 4].copy_from_slice(&[0, 0, 0, 255]);
                }
                blanked += (stop - from) as u64;
            }
            end = end.max(stop);
        }
    }
    Some(blanked)
}

/// 抓一張不含她的畫面。抓之前、抓之後各問一次她在哪裡（`her_parts`），兩次
/// 問到的都塗掉：抓圖要幾十毫秒，她的視窗可能在這中間出現、消失或被拖走，
/// 只問一次就蓋不到另一頭。任何一次問不出來、或畫面大小對不上，整張不要。
///
/// 平台層只交出「問她在哪」和「抓」兩件事；先後順序和塗法都在這裡。
pub fn grab_without_her(
    mut her_parts: impl FnMut() -> anyhow::Result<Vec<DesktopRect>>,
    grab: impl FnOnce() -> anyhow::Result<Vec<u8>>,
    frame_w: u32,
    frame_h: u32,
    captured: DesktopRect,
) -> anyhow::Result<Vec<u8>> {
    let before = her_parts()?;
    let mut rgba = grab()?;
    let after = her_parts()?;
    let parts: Vec<DesktopRect> = before.into_iter().chain(after).collect();
    blank(&mut rgba, frame_w, frame_h, captured, &parts)
        .ok_or_else(|| anyhow::anyhow!("擷取的畫面大小對不上，這一拍沒有抓畫面"))?;
    Ok(rgba)
}
