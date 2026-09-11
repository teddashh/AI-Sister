//! 游標底下到底有沒有東西。
//!
//! 這扇窗是 340×560 而且整片透明，畫了東西的只有控制列、輸入列、她本人，
//! 還有偶爾冒出來的泡泡。可是作業系統是照**整個矩形**做命中判定的，所以她
//! 頭頂上方那塊一百多像素高、什麼都看不到的帶狀區域仍然會把點擊吃掉——底下
//! 的視窗收不到。對一個永遠置頂、整天掛在角落的東西來說，那是一塊會跟著她
//! 移動的隱形擋板。
//!
//! 解法是 `set_ignore_cursor_events`，但它是**整扇窗**的開關，不是逐像素的，
//! 所以必須有人一直回答「現在游標底下算不算實心」。這個模組就是那個回答。
//!
//! 為什麼在這裡而不在桌面程式裡：和 [`crate::nudge_onto`] 同一個判準——
//! 這件事需要一個真的視窗系統才回答得了嗎？不需要，它是點和一堆長方形的
//! 問題。搬進來就每次 `cargo test` 都會跑。
//!
//! ## 為什麼是一堆長方形，而不是一張遮罩
//!
//! 她是去背的透明立繪，用她的外框當命中範圍會把一大片空白也算成她（那個框
//! 300×300，佔整扇窗的 47%）。所以 renderer 會把她的剪影切成一列一列的橫條
//! 再送過來。條數大概一兩百條，每次輪詢線性掃一遍就好；換成 bitset 要多一套
//! 編碼、解碼和除法，而這裡沒有任何一格需要那個複雜度。

use crate::Rect;

/// 點在不在這塊長方形裡。右下邊界不算，長方形之間才不會重疊到。
fn contains(r: Rect, x: i32, y: i32) -> bool {
    x >= r.x && y >= r.y && x < r.x.saturating_add(r.w) && y < r.y.saturating_add(r.h)
}

/// 視窗座標 `(x, y)` 底下算不算畫了東西。
///
/// **空的清單是「不知道」，不是「整扇窗都可以穿透」。** renderer 還沒回報、
/// JS 壞掉、persona 關掉的那一瞬間，這裡都會拿到空的；那種時候回 `true`，
/// 讓整扇窗維持可點。寧可多擋住一點桌面，也不要讓她整個人點不到——後者沒有
/// 別的救法，使用者連系統匣都得自己想到。
///
/// 寬或高不是正數的長方形不會讓任何一點變成實心——那是 renderer 算壞了，
/// 不是一塊「負的」實心區域。這件事不需要另外擋：`contains` 的右下邊界是
/// 開的，所以 `x < r.x + 0` 對任何 `x` 都是假的。曾經多寫過一道
/// `r.w > 0 && r.h > 0`，突變證明它一條測試都改不動，是死碼。
pub fn is_solid_at(solid: &[Rect], x: i32, y: i32) -> bool {
    if solid.is_empty() {
        return true;
    }
    solid.iter().any(|r| contains(*r, x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn empty_means_unknown_so_the_window_stays_clickable() {
        // 這條是整個模組唯一會造成「她點不到」的那一格的守衛。
        assert!(is_solid_at(&[], 0, 0));
        assert!(is_solid_at(&[], 170, 280));
        assert!(is_solid_at(&[], -5, 9999));
    }

    #[test]
    fn a_point_inside_a_panel_is_solid() {
        let pill = r(80, 7, 250, 34);
        assert!(is_solid_at(&[pill], 100, 20));
        assert!(is_solid_at(&[pill], 80, 7), "左上角含在內");
    }

    #[test]
    fn the_far_edge_belongs_to_the_next_rect_not_this_one() {
        // 半開區間。她的剪影是一列一列的橫條，相鄰兩條共用邊界；如果兩邊
        // 都含著那條線，重疊本身不會出錯，但「有沒有洞」就驗不準了。
        let box_ = r(10, 10, 5, 5);
        assert!(is_solid_at(&[box_], 14, 14));
        assert!(!is_solid_at(&[box_], 15, 14), "右邊界不算");
        assert!(!is_solid_at(&[box_], 14, 15), "下邊界不算");
    }

    #[test]
    fn the_empty_band_above_her_head_is_not_solid() {
        // 這就是整條線要修的那塊：控制列在上、她在下，中間那段沒有人畫東西。
        let pill = r(80, 7, 250, 34);
        let her = r(45, 206, 300, 300);
        assert!(!is_solid_at(&[pill, her], 170, 120), "頭頂上方那塊要穿透");
        assert!(is_solid_at(&[pill, her], 170, 20), "控制列還是實心");
        assert!(is_solid_at(&[pill, her], 170, 300), "她自己還是實心");
    }

    #[test]
    fn her_silhouette_leaves_the_gap_under_her_arm_transparent() {
        // 她是被切成橫條送過來的，所以同一列可以有兩段中間夾著空隙——手臂
        // 和身體之間那種。用外框當命中範圍的話這一格會是 true。
        let left_arm = r(60, 300, 30, 4);
        let torso = r(120, 300, 60, 4);
        let rows = [left_arm, torso];
        assert!(is_solid_at(&rows, 70, 301));
        assert!(is_solid_at(&rows, 150, 301));
        assert!(!is_solid_at(&rows, 100, 301), "腋下那塊空隙要穿透");
    }

    #[test]
    fn a_degenerate_rect_never_makes_anything_solid() {
        // renderer 算壞了會送出寬或高是 0／負的長方形。那不是一塊實心區域。
        assert!(!is_solid_at(&[r(10, 10, 0, 50)], 10, 20));
        assert!(!is_solid_at(&[r(10, 10, 50, 0)], 20, 10));
        assert!(!is_solid_at(&[r(10, 10, -50, -50)], 5, 5));
        // 但壞的那一塊不會把好的那一塊一起拖下水。
        assert!(is_solid_at(&[r(10, 10, 0, 50), r(0, 0, 20, 20)], 5, 5));
    }

    #[test]
    fn a_rect_at_the_far_edge_does_not_overflow() {
        // x + w 會溢位的話，`contains` 裡那個加法必須是 saturating——否則
        // debug build 直接 panic，而 panic 在輪詢執行緒裡是整條線靜悄悄死掉。
        assert!(is_solid_at(&[r(i32::MAX - 2, 0, 10, 10)], i32::MAX - 1, 5));
        assert!(!is_solid_at(&[r(0, 0, 10, 10)], i32::MAX, 5));
    }
}
