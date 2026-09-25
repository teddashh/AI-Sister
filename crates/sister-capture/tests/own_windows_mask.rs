//! 她看得見、但不在前景的視窗：存下來的畫面上，她那一塊要塗掉。
//!
//! 這一份是派工前先寫好的驗收，只碰 `sister_capture::own_windows` 的公開函式。
//! 平台層（Windows 的 EnumWindows 那一半）只負責把讀到的原始事實交進來，判斷
//! 全在這裡，所以 Linux 的 `cargo test` 就驗得到。真的 Windows 桌面那一半在
//! `tests/windows_own_windows.rs`。
//!
//! 兩個方向的錯代價不對稱：塗多了，是別的程式那一小塊這一拍少記；塗少了，是
//! 她把自己的答案錄進自己的記憶。所以讀不準的地方一律往「塗」那邊倒——唯一
//! 的例外是「別人的視窗能不能擋住她」：擋得住才不塗，所以讀不準就當擋不住。

use sister_capture::own_windows::{
    DesktopRect, LayeredAttributes, OwnWindowUnlocated, WindowFacts, blank, is_her_program,
    layered_is_see_through, own_parts,
};
use sister_core::config::OWN_APP_KEYS;

fn r(left: i32, top: i32, right: i32, bottom: i32) -> DesktopRect {
    DesktopRect {
        left,
        top,
        right,
        bottom,
    }
}

/// 她的一扇看得見的視窗，兩種外框都讀得到而且一樣大。
fn hers(bounds: DesktopRect) -> WindowFacts {
    WindowFacts {
        exe_name: Some("sister-desktop.exe".into()),
        visible: true,
        minimized: false,
        cloaked: Some(false),
        see_through: false,
        shaped: false,
        window_rect: Some(bounds),
        frame_bounds: Some(bounds),
    }
}

/// 別人的一扇一般視窗：看得見、沒被藏、不透明、方的。
fn other(bounds: DesktopRect) -> WindowFacts {
    WindowFacts {
        exe_name: Some("notepad.exe".into()),
        ..hers(bounds)
    }
}

const GRID: i32 = 64;

/// `parts` 蓋到的格子，在 [0, w) × [0, h) 上一格一格算。
fn raster(parts: &[DesktopRect], w: i32, h: i32) -> Vec<bool> {
    let mut out = vec![false; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            out[(y * w + x) as usize] = parts
                .iter()
                .any(|p| p.left <= x && x < p.right && p.top <= y && y < p.bottom);
        }
    }
    out
}

fn masked(windows: &[WindowFacts]) -> Vec<bool> {
    let parts = own_parts(windows).expect("這一組每一扇她的視窗都讀得到位置");
    raster(&parts, GRID, GRID)
}

// ─── 誰是她 ────────────────────────────────────────────────────────────

#[test]
fn her_program_is_exactly_her_three_names_whatever_the_case() {
    assert_eq!(OWN_APP_KEYS.len(), 3, "前提：三個平台各一個身分");
    for key in OWN_APP_KEYS {
        assert!(is_her_program(Some(key)), "{key}");
        assert!(
            is_her_program(Some(&key.to_ascii_uppercase())),
            "{key} 大寫也是她：Windows 的檔名不分大小寫"
        );
    }
    for near in [
        "sister-desktop (1).exe",
        "not-sister-desktop.exe",
        "sister-desktop.exe.bak",
        " sister-desktop.exe",
        "sister.exe",
        "msedgewebview2.exe",
        "notepad.exe",
        "",
    ] {
        assert!(!is_her_program(Some(near)), "{near:?} 不是她");
    }
    assert!(!is_her_program(None), "讀不到程式檔名的，不當成她");
}

#[test]
fn only_her_programs_are_masked() {
    let near: Vec<WindowFacts> = ["sister-desktop (1).exe", "msedgewebview2.exe", "sister.exe"]
        .iter()
        .map(|name| WindowFacts {
            exe_name: Some(name.to_string()),
            ..hers(r(0, 0, GRID, GRID))
        })
        .collect();
    assert_eq!(own_parts(&near), Ok(vec![]));
    let unknown = WindowFacts {
        exe_name: None,
        ..hers(r(0, 0, GRID, GRID))
    };
    assert_eq!(own_parts(&[unknown]), Ok(vec![]));
    for key in OWN_APP_KEYS {
        let w = WindowFacts {
            exe_name: Some(key.to_ascii_uppercase()),
            ..hers(r(1, 2, 30, 40))
        };
        assert_eq!(
            masked(&[w]),
            raster(&[r(1, 2, 30, 40)], GRID, GRID),
            "{key}"
        );
    }
}

// ─── 她自己那幾扇 ──────────────────────────────────────────────────────

#[test]
fn nothing_of_hers_on_screen_masks_nothing() {
    assert_eq!(own_parts(&[]), Ok(vec![]));
    assert_eq!(own_parts(&[other(r(0, 0, GRID, GRID))]), Ok(vec![]));
}

#[test]
fn her_window_is_masked_with_both_of_its_bounds() {
    // GetWindowRect 在 Windows 10 起多一圈看不見的縮放邊；DWM 的外框不含那一圈。
    // 塗她的時候寧大勿小：兩個矩形都塗（集合的聯集，不是外接框）。
    let mut w = hers(r(10, 10, 40, 40));
    w.window_rect = Some(r(3, 10, 47, 30));
    w.frame_bounds = Some(r(10, 5, 40, 40));
    let both = raster(&[r(3, 10, 47, 30), r(10, 5, 40, 40)], GRID, GRID);
    assert_eq!(masked(&[w.clone()]), both);

    // 只讀得到其中一個，就用那一個。
    w.window_rect = None;
    assert_eq!(
        masked(&[w.clone()]),
        raster(&[r(10, 5, 40, 40)], GRID, GRID)
    );
    w.window_rect = Some(r(3, 10, 47, 30));
    w.frame_bounds = None;
    assert_eq!(masked(&[w]), raster(&[r(3, 10, 47, 30)], GRID, GRID));
}

#[test]
fn her_window_that_is_not_drawn_is_not_masked() {
    let mut hidden = hers(r(0, 0, 40, 40));
    hidden.visible = false;
    let mut minimized = hers(r(0, 0, 40, 40));
    minimized.minimized = true;
    assert_eq!(own_parts(&[hidden.clone()]), Ok(vec![]));
    assert_eq!(own_parts(&[minimized.clone()]), Ok(vec![]));

    // 沒畫出來的那扇讀不到位置也沒關係：不會因為它整拍不抓。
    for w in [&mut hidden, &mut minimized] {
        w.window_rect = None;
        w.frame_bounds = None;
    }
    assert_eq!(own_parts(&[hidden, minimized]), Ok(vec![]));
}

#[test]
fn her_cloaked_window_is_masked_anyway() {
    // 被 DWM 藏起來的視窗畫面上看不到，但讀錯的代價不對稱：塗多了只是那一塊
    // 少記，塗少了就是把她自己的答案錄進去。讀不到有沒有被藏也一樣。
    for cloaked in [Some(true), None] {
        let mut w = hers(r(5, 5, 30, 30));
        w.cloaked = cloaked;
        assert_eq!(
            masked(&[w]),
            raster(&[r(5, 5, 30, 30)], GRID, GRID),
            "{cloaked:?}"
        );
    }
}

#[test]
fn her_transparent_or_shaped_window_is_masked_in_full() {
    // 桌寵那扇本來就是透明的。透明與形狀管的是「別人的視窗擋不擋得住她」，
    // 不是「她要不要塗」。
    let mut w = hers(r(5, 5, 30, 30));
    w.see_through = true;
    w.shaped = true;
    assert_eq!(masked(&[w]), raster(&[r(5, 5, 30, 30)], GRID, GRID));
}

#[test]
fn her_drawn_window_without_a_position_refuses_the_whole_frame() {
    let mut lost = hers(r(0, 0, 10, 10));
    lost.window_rect = None;
    lost.frame_bounds = None;
    assert_eq!(own_parts(&[lost.clone()]), Err(OwnWindowUnlocated));
    // 另一扇找得到也一樣：少塗一扇，就是把那一扇錄進去。前後都要擋。
    assert_eq!(
        own_parts(&[hers(r(20, 20, 40, 40)), lost.clone()]),
        Err(OwnWindowUnlocated)
    );
    assert_eq!(
        own_parts(&[lost.clone(), hers(r(20, 20, 40, 40))]),
        Err(OwnWindowUnlocated)
    );
    // 被別人整扇蓋住也一樣：她在哪裡都不知道，就不知道有沒有被蓋住。
    assert_eq!(
        own_parts(&[other(r(-100, -100, 100, 100)), lost]),
        Err(OwnWindowUnlocated)
    );
}

#[test]
fn her_windows_do_not_hide_each_other() {
    let parts = masked(&[hers(r(0, 0, 30, 30)), hers(r(10, 10, 50, 50))]);
    assert_eq!(
        parts,
        raster(&[r(0, 0, 30, 30), r(10, 10, 50, 50)], GRID, GRID)
    );
}

// ─── 別人的視窗擋在她上面 ──────────────────────────────────────────────

#[test]
fn an_opaque_window_above_her_hides_its_part_of_her() {
    // 由上往下排：記事本在上面，蓋住她的右半邊，那一半畫面上是記事本。
    let parts = masked(&[other(r(20, 0, GRID, GRID)), hers(r(0, 0, 40, 40))]);
    assert_eq!(parts, raster(&[r(0, 0, 20, 40)], GRID, GRID));
}

#[test]
fn a_window_that_covers_all_of_her_leaves_nothing_to_mask() {
    // 她的時間軸開在記事本後面：記事本整頁都要留著。
    let parts =
        own_parts(&[other(r(0, 0, GRID, GRID)), hers(r(10, 10, 40, 40))]).expect("讀得到位置");
    assert_eq!(
        raster(&parts, GRID, GRID),
        vec![false; (GRID * GRID) as usize]
    );
}

#[test]
fn a_window_below_her_hides_nothing() {
    let parts = masked(&[hers(r(0, 0, 40, 40)), other(r(20, 0, GRID, GRID))]);
    assert_eq!(parts, raster(&[r(0, 0, 40, 40)], GRID, GRID));
}

#[test]
fn a_window_above_her_hides_her_only_under_its_visible_frame() {
    // GetWindowRect 左右下各多一圈看不見的縮放邊，那一圈底下看得到她。
    let mut above = other(r(20, 0, GRID, GRID));
    above.window_rect = Some(r(13, 0, GRID, GRID));
    let parts = masked(&[above, hers(r(0, 0, 40, 40))]);
    assert_eq!(parts, raster(&[r(0, 0, 20, 40)], GRID, GRID));
}

#[test]
fn windows_that_cannot_be_trusted_to_hide_her_do_not() {
    let whole = r(0, 0, GRID, GRID);
    let cases: Vec<(&str, WindowFacts)> = vec![
        (
            "被 DWM 藏起來",
            WindowFacts {
                cloaked: Some(true),
                ..other(whole)
            },
        ),
        (
            "讀不到有沒有被藏",
            WindowFacts {
                cloaked: None,
                ..other(whole)
            },
        ),
        (
            "可能半透明",
            WindowFacts {
                see_through: true,
                ..other(whole)
            },
        ),
        (
            "不規則形狀",
            WindowFacts {
                shaped: true,
                ..other(whole)
            },
        ),
        (
            "讀不到 DWM 外框（不能退回 GetWindowRect）",
            WindowFacts {
                frame_bounds: None,
                ..other(whole)
            },
        ),
        (
            "沒畫出來",
            WindowFacts {
                visible: false,
                ..other(whole)
            },
        ),
        (
            "縮到最小",
            WindowFacts {
                minimized: true,
                ..other(whole)
            },
        ),
    ];
    for (why, above) in cases {
        let parts = masked(&[above, hers(r(0, 0, 40, 40))]);
        assert_eq!(
            parts,
            raster(&[r(0, 0, 40, 40)], GRID, GRID),
            "{why}：擋不住她，她整扇都要塗"
        );
    }
}

#[test]
fn a_window_between_her_two_windows_hides_only_the_lower_one() {
    let parts = masked(&[
        hers(r(0, 0, 20, 20)),
        other(r(0, 0, GRID, GRID)),
        hers(r(30, 30, 60, 60)),
    ]);
    assert_eq!(parts, raster(&[r(0, 0, 20, 20)], GRID, GRID));
}

#[test]
fn z_order_is_read_top_first() {
    // 同一組視窗，順序反過來結果就不同——順序是 EnumWindows 給的，由上往下。
    let top_first = [other(r(0, 0, GRID, GRID)), hers(r(10, 10, 40, 40))];
    let reversed = [hers(r(10, 10, 40, 40)), other(r(0, 0, GRID, GRID))];
    assert_eq!(masked(&top_first), vec![false; (GRID * GRID) as usize]);
    assert_eq!(masked(&reversed), raster(&[r(10, 10, 40, 40)], GRID, GRID));
}

// ─── 隨機對照：一格一格照規則算 ────────────────────────────────────────

/// 固定種子的 xorshift，免得為了一支測試拉一個 crate。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    /// 可能是空的、反的、超出格子的、負座標的。
    fn rect(&mut self, span: i32) -> DesktopRect {
        let left = self.below(span as u64 + 8) as i32 - 4;
        let top = self.below(span as u64 + 8) as i32 - 4;
        let right = left + self.below(34) as i32 - 3;
        let bottom = top + self.below(34) as i32 - 3;
        r(left, top, right, bottom)
    }
}

fn inside(rect: &Option<DesktopRect>, x: i32, y: i32) -> bool {
    rect.is_some_and(|p| p.left <= x && x < p.right && p.top <= y && y < p.bottom)
}

fn drawn(f: &WindowFacts) -> bool {
    f.visible && !f.minimized
}

fn is_hers(f: &WindowFacts) -> bool {
    f.exe_name
        .as_deref()
        .is_some_and(|name| OWN_APP_KEYS.contains(&name.to_ascii_lowercase().as_str()))
}

fn hides(f: &WindowFacts, x: i32, y: i32) -> bool {
    drawn(f)
        && !is_hers(f)
        && f.cloaked == Some(false)
        && !f.see_through
        && !f.shaped
        && inside(&f.frame_bounds, x, y)
}

/// 規則本身，一格一格算：`None` 是「這一拍整張不抓」。
fn oracle(windows: &[WindowFacts], span: i32) -> Option<Vec<bool>> {
    if windows
        .iter()
        .any(|f| drawn(f) && is_hers(f) && f.window_rect.is_none() && f.frame_bounds.is_none())
    {
        return None;
    }
    let mut out = vec![false; (span * span) as usize];
    for y in 0..span {
        for x in 0..span {
            out[(y * span + x) as usize] = windows.iter().enumerate().any(|(i, f)| {
                drawn(f)
                    && is_hers(f)
                    && (inside(&f.window_rect, x, y) || inside(&f.frame_bounds, x, y))
                    && !windows[..i].iter().any(|above| hides(above, x, y))
            });
        }
    }
    Some(out)
}

#[test]
fn own_parts_matches_the_rule_cell_by_cell_on_random_screens() {
    const SPAN: i32 = 40;
    let names = [
        Some("sister-desktop.exe"),
        Some("SISTER-DESKTOP"),
        Some("com.ted-h.ai-sister"),
        Some("notepad.exe"),
        Some("sister-desktop (1).exe"),
        None,
    ];
    let mut rng = Rng(0xA158_0002_5EED_0001);
    let (mut refused, mut some_masked, mut some_hidden) = (0, 0, 0);
    for round in 0..1500 {
        let count = 1 + rng.below(7) as usize;
        let windows: Vec<WindowFacts> = (0..count)
            .map(|_| WindowFacts {
                exe_name: names[rng.below(names.len() as u64) as usize].map(str::to_owned),
                visible: rng.chance(85),
                minimized: rng.chance(10),
                cloaked: match rng.below(20) {
                    0..3 => Some(true),
                    3..6 => None,
                    _ => Some(false),
                },
                see_through: rng.chance(12),
                shaped: rng.chance(6),
                window_rect: rng.chance(80).then(|| rng.rect(SPAN)),
                frame_bounds: rng.chance(80).then(|| rng.rect(SPAN)),
            })
            .collect();
        let want = oracle(&windows, SPAN);
        let got = own_parts(&windows);
        match (&want, &got) {
            (None, Err(OwnWindowUnlocated)) => refused += 1,
            (Some(want), Ok(parts)) => {
                assert_eq!(
                    &raster(parts, SPAN, SPAN),
                    want,
                    "第 {round} 輪\n{windows:#?}"
                );
                if want.iter().any(|&m| m) {
                    some_masked += 1;
                }
                // 同一組視窗、把每一扇別人的都拿掉之後塗得比較多＝真的有東西被擋住。
                let alone: Vec<WindowFacts> =
                    windows.iter().filter(|f| is_hers(f)).cloned().collect();
                let unhidden = oracle(&alone, SPAN).expect("她的那幾扇沒變");
                if unhidden != *want {
                    some_hidden += 1;
                }
            }
            _ => panic!("第 {round} 輪：規則說 {want:?}，得到 {got:?}\n{windows:#?}"),
        }
    }
    // 前提：三種情境都真的抽到過，不然上面那一圈可能一直在比兩個空集合。
    assert!(refused >= 20, "整張不抓的情境只抽到 {refused} 次");
    assert!(
        some_masked >= 150,
        "有塗到東西的情境只抽到 {some_masked} 次"
    );
    assert!(
        some_hidden >= 50,
        "她被別人擋住一部分的情境只抽到 {some_hidden} 次"
    );
}

// ─── 塗像素 ────────────────────────────────────────────────────────────

/// 每個像素都不一樣的底圖。R、G 從 1 起跳、alpha 故意不是 255，
/// 塗過的 (0,0,0,255) 和原本的像素不可能長得一樣。
fn pattern(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| [(i % 251) as u8 + 1, (i / 251 % 251) as u8 + 1, 77, 7])
        .collect()
}

/// `want` 裡為真的像素要是 (0,0,0,255)，其餘每個位元組都不准動。
fn expect_blanked(before: &[u8], after: &[u8], want: &[bool]) {
    assert_eq!(before.len(), after.len());
    assert_eq!(want.len() * 4, after.len());
    for (i, &blank) in want.iter().enumerate() {
        let px = &after[i * 4..i * 4 + 4];
        if blank {
            assert_eq!(px, [0, 0, 0, 255], "第 {i} 個像素該塗掉");
        } else {
            assert_eq!(px, &before[i * 4..i * 4 + 4], "第 {i} 個像素不該被動到");
        }
    }
}

#[test]
fn no_parts_change_nothing() {
    let before = pattern(64, 48);
    let mut px = before.clone();
    assert_eq!(blank(&mut px, 64, 48, r(0, 0, 64, 48), &[]), Some(0));
    assert_eq!(px, before);
}

#[test]
fn a_same_size_frame_blanks_exactly_her_pixels() {
    let before = pattern(64, 48);
    let mut px = before.clone();
    let n = blank(&mut px, 64, 48, r(0, 0, 64, 48), &[r(10, 5, 30, 20)]);
    assert_eq!(n, Some(20 * 15));
    expect_blanked(&before, &px, &raster(&[r(10, 5, 30, 20)], 64, 48));
}

#[test]
fn parts_are_clipped_to_the_monitor_that_was_captured() {
    // 左上角露出 5×5；右邊那一扇在隔壁螢幕上（半開區間，right == 64 不算）。
    let before = pattern(64, 48);
    let mut px = before.clone();
    let n = blank(
        &mut px,
        64,
        48,
        r(0, 0, 64, 48),
        &[r(-10, -10, 5, 5), r(64, 0, 100, 48)],
    );
    assert_eq!(n, Some(25));
    expect_blanked(&before, &px, &raster(&[r(0, 0, 5, 5)], 64, 48));
}

#[test]
fn a_monitor_left_of_the_primary_has_negative_coordinates() {
    let before = pattern(64, 48);
    let mut px = before.clone();
    let n = blank(&mut px, 64, 48, r(-64, 0, 0, 48), &[r(-40, 10, -20, 30)]);
    assert_eq!(n, Some(20 * 20));
    expect_blanked(&before, &px, &raster(&[r(24, 10, 44, 30)], 64, 48));
}

#[test]
fn a_monitor_away_from_the_origin() {
    let before = pattern(64, 48);
    let mut px = before.clone();
    let n = blank(
        &mut px,
        64,
        48,
        r(1920, 200, 1984, 248),
        &[r(1930, 210, 1940, 220)],
    );
    assert_eq!(n, Some(100));
    expect_blanked(&before, &px, &raster(&[r(10, 10, 20, 20)], 64, 48));
}

#[test]
fn overlapping_parts_count_each_pixel_once() {
    let before = pattern(64, 48);
    let mut px = before.clone();
    let parts = [r(0, 0, 20, 20), r(10, 10, 30, 30)];
    let n = blank(&mut px, 64, 48, r(0, 0, 64, 48), &parts);
    assert_eq!(n, Some(400 + 400 - 100));
    expect_blanked(&before, &px, &raster(&parts, 64, 48));
}

#[test]
fn a_downscaled_frame_rounds_outward_and_takes_one_more_pixel() {
    // 128×96 縮成 64×48。來源 x 11..21 → 5.5..10.5 → 往外取整 5..11，
    // 縮過的畫面再往外多一格 4..12（縮圖會把鄰近的來源像素混進來）。
    let before = pattern(64, 48);
    let mut px = before.clone();
    let n = blank(&mut px, 64, 48, r(0, 0, 128, 96), &[r(11, 11, 21, 21)]);
    assert_eq!(n, Some(8 * 8));
    expect_blanked(&before, &px, &raster(&[r(4, 4, 12, 12)], 64, 48));
}

#[test]
fn the_extra_pixel_stops_at_the_frame_edge() {
    let before = pattern(64, 48);
    let mut px = before.clone();
    // 0..4 → 0..2 → 多一格是 -1..3，切到畫面裡剩 0..3。
    let n = blank(&mut px, 64, 48, r(0, 0, 128, 96), &[r(0, 0, 4, 4)]);
    assert_eq!(n, Some(9));
    expect_blanked(&before, &px, &raster(&[r(0, 0, 3, 3)], 64, 48));
}

#[test]
fn a_part_just_off_the_captured_monitor_does_not_bleed_in_when_scaled() {
    // 她在隔壁那台螢幕、貼著這台的左邊。縮圖只讀這台的像素，多一格那條規則
    // 不能把她從隔壁拉一條黑邊進來。
    let before = pattern(64, 48);
    let mut px = before.clone();
    let n = blank(
        &mut px,
        64,
        48,
        r(0, 0, 128, 96),
        &[r(-40, 0, 0, 96), r(0, 96, 128, 140)],
    );
    assert_eq!(n, Some(0));
    assert_eq!(px, before);
}

#[test]
fn a_wrong_sized_buffer_or_capture_is_refused_untouched() {
    let before = pattern(64, 48);
    let everything = [r(-1000, -1000, 1000, 1000)];

    let mut short = before[..before.len() - 4].to_vec();
    let untouched = short.clone();
    assert_eq!(
        blank(&mut short, 64, 48, r(0, 0, 64, 48), &everything),
        None
    );
    assert_eq!(short, untouched, "太短");

    let mut long = before.clone();
    long.extend_from_slice(&[9, 9, 9, 9]);
    let untouched = long.clone();
    assert_eq!(blank(&mut long, 64, 48, r(0, 0, 64, 48), &everything), None);
    assert_eq!(long, untouched, "太長");

    let mut px = before.clone();
    assert_eq!(
        blank(&mut px, 64, 48, r(0, 0, 0, 48), &everything),
        None,
        "擷取範圍是空的"
    );
    assert_eq!(
        blank(&mut px, 64, 48, r(10, 0, 5, 48), &everything),
        None,
        "擷取範圍是反的"
    );
    assert_eq!(px, before);
    assert_eq!(
        blank(&mut Vec::new(), 0, 0, r(0, 0, 64, 48), &everything),
        None,
        "零大小的畫面"
    );
}

/// 一個畫面像素讀進來的來源範圍，碰到 [a, b) 沒有。都是相對擷取範圍的座標。
fn footprint_hits(f: i64, frame: i64, src: i64, a: i64, b: i64) -> bool {
    a < b && f * src < b * frame && (f + 1) * src > a * frame
}

#[test]
fn blank_matches_the_footprint_rule_on_random_frames() {
    let mut rng = Rng(0xA158_0002_B1A4_0001);
    let (mut same_size, mut scaled, mut widened) = (0, 0, 0);
    for round in 0..800 {
        let sw = 8 + rng.below(90) as i64;
        let sh = 8 + rng.below(90) as i64;
        let (fw, fh) = if rng.chance(40) {
            (sw, sh)
        } else {
            (
                1 + rng.below(sw as u64) as i64,
                1 + rng.below(sh as u64) as i64,
            )
        };
        let ox = rng.below(400) as i32 - 200;
        let oy = rng.below(400) as i32 - 200;
        let captured = r(ox, oy, ox + sw as i32, oy + sh as i32);
        let parts: Vec<DesktopRect> = (0..rng.below(4))
            .map(|_| {
                let left = ox + rng.below(sw as u64 + 40) as i32 - 20;
                let top = oy + rng.below(sh as u64 + 40) as i32 - 20;
                let right = left + rng.below(40) as i32 - 4;
                let bottom = top + rng.below(40) as i32 - 4;
                r(left, top, right, bottom)
            })
            .collect();

        // 先切到擷取範圍裡，再看每個畫面像素的來源碰不碰得到。
        let hit = |fx: i64, fy: i64| {
            parts.iter().any(|p| {
                let a = i64::from(p.left.max(captured.left) - ox);
                let b = i64::from(p.right.min(captured.right) - ox);
                let c = i64::from(p.top.max(captured.top) - oy);
                let d = i64::from(p.bottom.min(captured.bottom) - oy);
                footprint_hits(fx, fw, sw, a, b) && footprint_hits(fy, fh, sh, c, d)
            })
        };
        let is_scaled = fw != sw || fh != sh;
        let mut want = vec![false; (fw * fh) as usize];
        let mut grew = false;
        for fy in 0..fh {
            for fx in 0..fw {
                let direct = hit(fx, fy);
                let near = is_scaled
                    && (-1..=1).any(|dy| {
                        (-1..=1).any(|dx| {
                            let (gx, gy) = (fx + dx, fy + dy);
                            (0..fw).contains(&gx) && (0..fh).contains(&gy) && hit(gx, gy)
                        })
                    });
                want[(fy * fw + fx) as usize] = direct || near;
                grew |= near && !direct;
            }
        }

        let before = pattern(fw as u32, fh as u32);
        let mut px = before.clone();
        let n = blank(&mut px, fw as u32, fh as u32, captured, &parts);
        let count = want.iter().filter(|&&b| b).count() as u64;
        assert_eq!(
            n,
            Some(count),
            "第 {round} 輪：{sw}×{sh} → {fw}×{fh}，{captured:?}，{parts:?}"
        );
        expect_blanked(&before, &px, &want);
        if count > 0 {
            if is_scaled {
                scaled += 1;
            } else {
                same_size += 1;
            }
        }
        if grew {
            widened += 1;
        }
    }
    assert!(same_size >= 60, "同尺寸而且有塗到的只抽到 {same_size} 次");
    assert!(scaled >= 100, "縮圖而且有塗到的只抽到 {scaled} 次");
    assert!(
        widened >= 50,
        "多一格那條規則真的多塗了的只抽到 {widened} 次"
    );
}

// ─── 別人的 layered 視窗 ───────────────────────────────────────────────

#[test]
fn only_a_layered_window_that_proves_it_is_opaque_can_hide_her() {
    let attrs = |color_key, alpha| Some(LayeredAttributes { color_key, alpha });
    assert!(
        !layered_is_see_through(false, None),
        "不是 layered 的視窗照一般視窗算"
    );
    assert!(
        !layered_is_see_through(false, attrs(true, Some(0))),
        "不是 layered 的視窗，那組屬性不算數"
    );
    assert!(
        layered_is_see_through(true, None),
        "讀不到 layered 屬性（UpdateLayeredWindow 逐像素透明，或還沒設過）"
    );
    assert!(
        layered_is_see_through(true, attrs(true, None)),
        "有透明色鍵"
    );
    assert!(
        layered_is_see_through(true, attrs(true, Some(255))),
        "有透明色鍵"
    );
    for alpha in [0u8, 1, 128, 254] {
        assert!(
            layered_is_see_through(true, attrs(false, Some(alpha))),
            "alpha {alpha}"
        );
    }
    assert!(
        !layered_is_see_through(true, attrs(false, Some(255))),
        "整扇不透明"
    );
    assert!(
        !layered_is_see_through(true, attrs(false, None)),
        "沒設 alpha 也沒色鍵"
    );
}

// ─── 兩半接起來：平常那一格 ────────────────────────────────────────────

#[test]
fn the_pinned_pet_over_notepad_on_a_1080p_monitor() {
    // 由上往下：工作列（最上層）、桌寵（最上層、340×560、壓到工作列上緣）、
    // 記事本（一般視窗，焦點在它上面）。
    let windows = [
        WindowFacts {
            exe_name: Some("explorer.exe".into()),
            ..other(r(0, 1040, 1920, 1080))
        },
        hers(r(1500, 500, 1840, 1060)),
        other(r(0, 0, 1920, 1040)),
    ];
    let parts = own_parts(&windows).expect("讀得到位置");
    let (w, h) = (1920u32, 1080u32);
    let before = pattern(w, h);
    let mut px = before.clone();
    let n = blank(&mut px, w, h, r(0, 0, 1920, 1080), &parts);
    // 工作列擋住桌寵最下面 20 列，那 20 列畫面上是工作列。
    assert_eq!(n, Some(340 * 540));
    let want = raster(&[r(1500, 500, 1840, 1040)], w as i32, h as i32);
    expect_blanked(&before, &px, &want);
}
