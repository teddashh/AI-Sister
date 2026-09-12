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
//! 對外只有 [`poll_step`] 一個入口。底下那兩層（`is_solid_at`、
//! `should_pass_through`）刻意不公開：它們都不看視窗可不可見，直接拿來用的話
//! 她收進系統匣時會停在穿透，再叫出來的頭幾十毫秒點不到——那正是 `poll_step`
//! 第一件事就在擋的東西。要問「這一拍該怎麼辦」就問 `poll_step`。
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
fn is_solid_at(solid: &[Rect], x: i32, y: i32) -> bool {
    if solid.is_empty() {
        return true;
    }
    solid.iter().any(|r| contains(*r, x, y))
}

/// 這一刻該不該讓點擊穿過去。
///
/// `window` 是這扇窗自己的矩形，左上角是 `(0, 0)`；`at` 是游標在同一套座標裡的
/// 位置，`None` 代表問不出來。輪詢那條執行緒的整個判斷就是這一句。
///
/// ## 游標在窗外的時候，答案是「維持可點」而不是「穿透」
///
/// 這句反直覺，也是這支函式比 [`is_solid_at`] 多做的唯一一件事。開關是**整扇窗**
/// 的，而游標現在不在窗上——所以這一刻它不影響任何一次點擊。它唯一的作用，是決定
/// **下一個瞬間游標進來時**用哪一邊；而輪詢有間隔，那個瞬間我們看不到。
///
/// 兩種猜錯的代價不對稱：
///
/// * 猜「穿透」、而使用者把游標移到她身上按下去 → 那一下掉到底下的視窗。畫面上
///   明明有她，滑鼠卻穿過去，看起來就是當掉了。
/// * 猜「可點」、而使用者按的是她旁邊的空白 → 那一下被這扇窗吃掉。使用者看得見
///   自己按在哪裡，而且那正是這條線修好之前的行為。
///
/// 所以停在可點的那一邊：把一輪輪詢的延遲挪到便宜的那一格。
fn should_pass_through(window: Rect, solid: &[Rect], at: Option<(i32, i32)>) -> bool {
    let Some((x, y)) = at else {
        return false;
    };
    if !contains(window, x, y) {
        return false;
    }
    !is_solid_at(solid, x, y)
}

/// 游標在窗內（或那一圈裡）的時候多久看一次。
///
/// 她整天掛在螢幕角落，而使用者絕大多數時間在別的視窗工作。兩段間隔的差別就是
/// 那件事：手在她身上的時候要跟得上（40ms 大約比「移過去再按下去」快一個數量
/// 級），手不在的時候沒有人需要這個答案，少醒來幾次就少耗一點電。
pub const POLL_NEAR_MS: u64 = 40;

/// 游標在那一圈外面的時候多久看一次。
///
/// 慢的這一段之所以安全，是因為窗外那一格的答案是「維持可點」（見
/// [`should_pass_through`]）——猜錯的方向是多擋一下，不是她點不到。
pub const POLL_AWAY_MS: u64 = 160;

/// 視窗再往外撐多少才算「附近」。
pub const POLL_NEAR_MARGIN: i32 = 120;

/// 「快」必須真的比「慢」快。
///
/// 兩個常數對調的話每一條測試都還是綠的——它們問的是「這一拍該睡多久」，兩個
/// 值都還在。真正壞掉的是手在她身上時反而用到最慢的那一段，而那是唯一需要跟上
/// 的時候。這一行讓那種對調在**編譯期**就過不去。
const _: () = assert!(POLL_NEAR_MS < POLL_AWAY_MS);

/// 游標在不在「附近」——視窗再往外撐 [`POLL_NEAR_MARGIN`] 那一圈。
///
/// 留一圈是因為間隔是拿**上一次**的位置決定的：貼著邊界切換的話，從外面快速
/// 移進來的那一下會用到慢的那一段。問不出游標在哪就當成不在附近——那一拍本來
/// 就不會動開關（見 [`should_pass_through`]），慢一點沒有代價。
fn near_window(window: Rect, at: Option<(i32, i32)>) -> bool {
    let Some((x, y)) = at else {
        return false;
    };
    contains(
        Rect {
            x: window.x.saturating_sub(POLL_NEAR_MARGIN),
            y: window.y.saturating_sub(POLL_NEAR_MARGIN),
            w: window.w.saturating_add(POLL_NEAR_MARGIN.saturating_mul(2)),
            h: window.h.saturating_add(POLL_NEAR_MARGIN.saturating_mul(2)),
        },
        x,
        y,
    )
}

/// 輪詢一拍的兩個決定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// 要交給 `set_ignore_cursor_events` 的值。
    pub pass_through: bool,
    /// 下一拍要睡多久（毫秒）。直接給數字而不是「近不近」，是因為那個對照
    /// 本身也要有人守：把兩個常數對調的話，手在她身上時反而變成最慢的那一段。
    pub next_poll_ms: u64,
}

/// 輪詢一拍：這一刻該不該穿透，還有下一拍該多快回來。
///
/// 整個決定放在這裡而不是桌面程式裡，是因為 `apps/desktop` 是另一個 workspace，
/// 根目錄的 `cargo test --workspace` 走不到它——留在 `main.rs` 的每一行都等於
/// 沒有人守。那邊只剩下真的需要一個視窗系統才做得到的事：問游標在哪、翻開關。
///
/// `visible` 是假的時候不看游標：她收進系統匣了，沒有人會點到她。但開關仍要
/// 翻回可點，**因為再打開的時候要是可點的**——停在穿透的話，她重新出現的頭
/// 幾十毫秒是點不到的，而那正是使用者剛叫出她、最可能馬上去點的那一刻。
pub fn poll_step(window: Rect, solid: &[Rect], visible: bool, at: Option<(i32, i32)>) -> Step {
    if !visible {
        return Step {
            pass_through: false,
            next_poll_ms: POLL_AWAY_MS,
        };
    }
    Step {
        pass_through: should_pass_through(window, solid, at),
        next_poll_ms: if near_window(window, at) {
            POLL_NEAR_MS
        } else {
            POLL_AWAY_MS
        },
    }
}

/// 兩次判斷之間的地板。被叫醒也不會比這個更快。
///
/// [`wait_for_next_poll`] 讓 renderer 可以把輪詢執行緒提早叫起來，而
/// `pointermove` 一秒可以來上千次。地板讓那條執行緒在 renderer 壞掉狂叫的
/// 時候最快也只跑 1000/`POLL_FLOOR_MS` 次；正常情況它只是把「被叫醒」的反應
/// 時間從 0 墊到這個數字，而這個數字比 [`POLL_NEAR_MS`] 小一個量級。
pub const POLL_FLOOR_MS: u64 = 8;
const _: () = assert!(POLL_FLOOR_MS < POLL_NEAR_MS);

/// 「該再看一次了」的閘門：旗標 ＋ condvar。`true` 代表有人叫過而還沒被收走。
pub type PollGate = (std::sync::Mutex<bool>, std::sync::Condvar);

/// 睡到該再看一次為止。回來的時候旗標一定是 `false`。
///
/// 三種回來的理由：睡滿 `nap_ms`、有人在睡之前就叫過了、睡到一半被叫醒。
/// 呼叫端不必分辨是哪一種——它回來之後做的事永遠是同一套 [`poll_step`]，而且
/// 是自己去問游標在哪，不採信叫它的人給的任何座標。**叫的人只能影響那一拍
/// 什麼時候發生，不能影響它算出什麼。**
///
/// 為什麼需要這個：游標離她 [`POLL_NEAR_MARGIN`] 以外的時候，下一拍要
/// [`POLL_AWAY_MS`] 才到。使用者從遠處一口氣把滑鼠甩到她旁邊的空白上按下去，
/// 那一下會被那扇窗吃掉。而那個瞬間 renderer 是收得到 `pointermove` 的——窗
/// 那時候還是可點的——所以讓它喊一聲，比等這條執行緒睡飽快兩個數量級。
///
/// 先無條件睡滿 [`POLL_FLOOR_MS`] 再開始等，那一段誰叫都不動。這段期間來的
/// 呼叫不會漏掉：旗標留在那裡，接下來的等待會立刻看到它。
pub fn wait_for_next_poll(gate: &PollGate, nap_ms: u64) {
    let (flag, cv) = gate;
    std::thread::sleep(std::time::Duration::from_millis(POLL_FLOOR_MS));

    let rest = std::time::Duration::from_millis(nap_ms.saturating_sub(POLL_FLOOR_MS));
    let deadline = std::time::Instant::now() + rest;
    let mut woken = flag.lock().expect("poll gate");
    while !*woken {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        // condvar 會有偽喚醒，所以要看著 deadline 重算剩餘時間再等一次；
        // `wait_timeout` 回一次就當成時間到的話，這支函式會提早回去，而
        // 「提早」在這裡等於整條線變成忙迴圈。
        let (next, _) = cv.wait_timeout(woken, left).expect("poll gate");
        woken = next;
    }
    *woken = false;
}

/// 叫醒在 [`wait_for_next_poll`] 裡等著的那條執行緒。
///
/// 和 `wait_for_next_poll` 是同一個協定的兩半，所以擺在一起：分開寫的話，
/// 叫的那一端很容易少掉 `notify_one()`——旗標立起來了、可是沒人被戳醒，於是
/// 那條執行緒照舊睡滿整覺，而**看起來完全正常**（它終究會醒，只是晚了）。
/// 那種失敗沒有任何症狀可以指認。
pub fn nudge(gate: &PollGate) {
    let (flag, cv) = gate;
    *flag.lock().expect("poll gate") = true;
    cv.notify_one();
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
    #[test]
    fn a_cursor_outside_the_window_leaves_it_clickable() {
        // 這是 `should_pass_through` 唯一比 `is_solid_at` 多做的事，也是最容易
        // 寫反的一格：同一個點，底層說「不是實心」，而正確答案是不要穿透。
        let win = r(0, 0, 340, 560);
        let her = r(45, 206, 300, 300);
        assert!(
            !is_solid_at(&[her], -125, 300),
            "底層看到的是：那裡沒畫東西"
        );
        assert!(
            !should_pass_through(win, &[her], Some((-125, 300))),
            "但它在窗外——下一瞬間游標進來時要是可點的"
        );
        assert!(!should_pass_through(win, &[her], Some((170, -3))), "上方");
        assert!(
            !should_pass_through(win, &[her], Some((340, 300))),
            "右邊界外"
        );
        assert!(
            !should_pass_through(win, &[her], Some((170, 560))),
            "下邊界外"
        );
    }

    #[test]
    fn an_unknown_cursor_leaves_the_window_clickable() {
        let win = r(0, 0, 340, 560);
        assert!(!should_pass_through(win, &[r(45, 206, 300, 300)], None));
    }

    #[test]
    fn an_empty_report_never_passes_anything_through() {
        // 和 `empty_means_unknown_so_the_window_stays_clickable` 同一條規則，
        // 但走的是真正接到輪詢上的那個出口。
        let win = r(0, 0, 340, 560);
        assert!(!should_pass_through(win, &[], Some((170, 120))));
        assert!(!should_pass_through(win, &[], Some((170, 300))));
    }

    #[test]
    fn inside_the_window_it_still_follows_her_silhouette() {
        let win = r(0, 0, 340, 560);
        let pill = r(80, 7, 250, 34);
        let her = r(45, 206, 300, 300);
        let painted = [pill, her];
        assert!(
            should_pass_through(win, &painted, Some((170, 120))),
            "頭頂上方那塊要穿透"
        );
        assert!(
            !should_pass_through(win, &painted, Some((170, 20))),
            "控制列"
        );
        assert!(
            !should_pass_through(win, &painted, Some((170, 300))),
            "她自己"
        );
    }
    /// 這四條守的是 `main.rs` 那條輪詢執行緒的**整個**決定。在搬進來以前它們
    /// 一條都沒有人跑：桌面那棵樹是另一個 workspace。
    fn pet() -> Rect {
        r(0, 0, 340, 560)
    }

    #[test]
    fn hidden_forces_it_clickable_so_she_is_usable_the_moment_she_comes_back() {
        // 游標正落在一塊沒畫東西的地方——可見的時候這一格會是「穿透」。
        let her = r(45, 206, 300, 300);
        assert!(should_pass_through(pet(), &[her], Some((170, 120))));
        // 但她收起來了，開關要停在可點：她重新出現的頭幾十毫秒不能是點不到的。
        let step = poll_step(pet(), &[her], false, Some((170, 120)));
        assert!(!step.pass_through, "藏起來的時候要翻回可點");
        assert_eq!(
            step.next_poll_ms, POLL_AWAY_MS,
            "沒有人需要快速輪詢一扇看不見的窗"
        );
    }

    #[test]
    fn the_near_band_reaches_past_the_window_edge() {
        // 間隔是拿上一次的位置決定的，所以窗外那一圈也要算「附近」。
        let solid = [r(45, 206, 300, 300)];
        let near = |x, y| poll_step(pet(), &solid, true, Some((x, y))).next_poll_ms == POLL_NEAR_MS;
        assert!(near(-119, 300), "窗外 119px 還在圈內");
        assert!(!near(-121, 300), "窗外 121px 已經出圈");
        assert!(near(459, 300), "右邊那一圈");
        assert!(!near(460, 300), "右邊界外一格就出圈");
        assert!(near(170, 300), "窗內當然算");
        assert!(!near(170, 680), "下面出圈");
    }

    #[test]
    fn an_unknown_cursor_is_neither_near_nor_passed_through() {
        let step = poll_step(pet(), &[r(45, 206, 300, 300)], true, None);
        assert!(!step.pass_through, "問不出游標在哪就維持可點");
        assert_eq!(step.next_poll_ms, POLL_AWAY_MS);
    }

    #[test]
    fn a_visible_window_still_follows_her_silhouette() {
        let painted = [r(80, 7, 250, 34), r(45, 206, 300, 300)];
        let pass = |x, y| poll_step(pet(), &painted, true, Some((x, y))).pass_through;
        assert!(pass(170, 120), "頭頂上方那塊要穿透");
        assert!(!pass(170, 20), "控制列");
        assert!(!pass(170, 300), "她自己");
        assert!(!pass(-125, 300), "窗外維持可點");
    }
    #[test]
    fn the_hand_on_her_gets_the_fast_cadence_not_the_slow_one() {
        // 兩個常數對調的話，每一條既有測試都還是綠的——它們只問「近不近」。
        // 這一條問的是那個對照本身。
        let solid = [r(45, 206, 300, 300)];
        let ms = |x, y| poll_step(pet(), &solid, true, Some((x, y))).next_poll_ms;
        assert_eq!(ms(170, 300), POLL_NEAR_MS, "手在她身上要用快的那一段");
        assert_eq!(ms(-1000, 300), POLL_AWAY_MS, "手在遠處用慢的那一段");
        // 「快的真的比慢的快」不在這裡斷言——那是編譯期的 `const _` 在守。
    }

    /* ---------- 被叫醒的那一拍 ---------- */

    /// 一個乾淨的閘門；`raised` 代表「有人在開始等之前就叫過了」，而那一聲
    /// 同樣走產品的 [`nudge`]。
    fn gate(raised: bool) -> PollGate {
        let g = (std::sync::Mutex::new(false), std::sync::Condvar::new());
        if raised {
            nudge(&g);
        }
        g
    }

    fn raised(gate: &PollGate) -> bool {
        *gate.0.lock().expect("poll gate")
    }

    /// 時間斷言一律留大餘裕：CI runner 比開發機慢兩倍，而這幾條要問的是
    /// 「有沒有等滿那一覺」這種量級的事，不是精確的毫秒數。
    const WAY_LESS_THAN_THE_NAP: std::time::Duration = std::time::Duration::from_secs(1);
    const A_NAP_NOBODY_WOULD_WAIT_OUT: u64 = 10_000;

    #[test]
    fn a_nudge_that_arrived_first_is_not_lost() {
        let g = gate(true);
        let started = std::time::Instant::now();
        wait_for_next_poll(&g, A_NAP_NOBODY_WOULD_WAIT_OUT);
        assert!(
            started.elapsed() < WAY_LESS_THAN_THE_NAP,
            "旗標在開始等之前就立起來了，卻還是睡滿了整覺——地板那段 sleep 期間\
             來的呼叫被吃掉了，而那正是最容易發生的時機"
        );
    }

    #[test]
    fn waking_up_clears_the_flag() {
        let g = gate(true);
        wait_for_next_poll(&g, A_NAP_NOBODY_WOULD_WAIT_OUT);
        assert!(
            !raised(&g),
            "叫過一次之後旗標沒收回去。留著的話下一圈會立刻又回來，整條執行緒\
             變成以地板為週期的忙迴圈"
        );
    }

    #[test]
    fn nobody_nudging_means_it_sleeps() {
        let g = gate(false);
        let started = std::time::Instant::now();
        wait_for_next_poll(&g, POLL_FLOOR_MS + 40);
        let slept = started.elapsed();
        assert!(
            slept >= std::time::Duration::from_millis(POLL_FLOOR_MS),
            "沒人叫，卻連地板都沒睡滿（只睡了 {slept:?}）"
        );
    }

    #[test]
    fn a_nudge_from_another_thread_cuts_the_nap_short() {
        let g = std::sync::Arc::new(gate(false));
        let caller = std::sync::Arc::clone(&g);
        let nudger = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            // 用產品自己那一半，不要在測試裡手抄一份 condvar 協定：抄的那份
            // 少一句 `notify_one()` 也照樣會綠，而產品少那一句就是這條線失效。
            nudge(&caller);
        });
        let started = std::time::Instant::now();
        wait_for_next_poll(&g, A_NAP_NOBODY_WOULD_WAIT_OUT);
        let waited = started.elapsed();
        nudger.join().expect("nudger");
        assert!(
            waited < WAY_LESS_THAN_THE_NAP,
            "睡到一半被叫，卻等了 {waited:?} 才回來——`notify_one` 沒有把它從\
             `wait_timeout` 裡撈出來，那條執行緒只是照舊睡滿"
        );
    }

    #[test]
    fn the_floor_is_not_skippable() {
        let g = gate(true);
        let started = std::time::Instant::now();
        wait_for_next_poll(&g, 0);
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(POLL_FLOOR_MS),
            "旗標立著、`nap_ms` 是 0，就整個不睡了。renderer 每秒叫一千次的時候\
             這條執行緒會跟著跑一千圈"
        );
    }
}
