//! 記住她被放在哪裡。
//!
//! 配方是 TokenMonster 的 `bounds-store.ts`（Ted 自己的 repo，MIT），
//! 但這裡**修掉了那邊的一個 bug**。
//!
//! 原本的作法只在第一次啟動時看一次主螢幕的工作區，用來決定初始位置；之後
//! 還原座標時完全不檢查那個座標現在還落不落在任何一個螢幕上。把視窗拖到副
//! 螢幕、關掉、拔掉副螢幕再開——她就還原到一個看不見的地方。而這個視窗
//! `skipTaskbar` 是開的，所以工作列上也沒有東西可以點回來，唯一的救法是去
//! 刪設定檔。對一個「常駐在角落陪你」的東西來說，那等於她消失了。
//!
//! 所以還原之前一定要再問一次：這個位置現在還看得到嗎？
//!
//! 判斷用的是純粹的矩形運算，跟 Tauri 沒有關係——**這就是它為什麼是一個
//! 獨立的 crate，而不是桌面程式裡的一個模組**。
//!
//! 這段程式本來寫在 `apps/desktop/src-tauri/src/bounds.rs`。放在那裡的話它
//! 一行測試都跑不到：Tauri 的 build script 要一個 Windows 資源編譯器
//! （`llvm-rc`）才能把圖示嵌進執行檔，而這台 Linux 開發機上沒有、也沒有
//! sudo 可以裝，所以整個 crate 在這裡連編都編不起來。搬出來之後這幾條規則
//! 每次 `cargo test` 都會跑。
//!
//! 挑哪一邊搬的判準是：**這件事需要一個真的視窗系統才回答得了嗎？**
//! 「螢幕拔掉之後這個座標還看得到嗎」不需要——那是兩個長方形的問題。
//! 「置頂到底有沒有生效」需要，那條只能留在 Windows 上驗。

use serde::{Deserialize, Serialize};
use std::path::Path;

/// 存下來的東西只有位置與置頂與否。視窗是 `resizable: false`，尺寸由
/// `tauri.conf.json` 說了算，沒有第二個地方可以不同意。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PetState {
    pub x: i32,
    pub y: i32,
    pub pinned: bool,
}

impl Default for PetState {
    fn default() -> Self {
        // 位置沒有有意義的預設值——「螢幕右下角」要等看得到螢幕才算得出來，
        // 那是 `first_run_corner` 的事。
        Self {
            x: 0,
            y: 0,
            pinned: true,
        }
    }
}

/// 一塊長方形。螢幕與視窗共用同一個型別，因為這裡要問的問題兩者都一樣：
/// 它們有沒有重疊、重疊多少。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.w
    }

    fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// 兩塊長方形重疊的面積。
    fn overlap(&self, other: &Rect) -> i64 {
        let w = (self.right().min(other.right()) - self.x.max(other.x)).max(0) as i64;
        let h = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0) as i64;
        w * h
    }
}

/// 視窗至少要露出這麼大一塊，才算「使用者拿得回來」。
///
/// 不是 0：露出一個像素在技術上叫「在螢幕上」，但沒有人抓得到那一個像素。
/// 這個數字大約是拖曳條的高度——看得到那條就抓得回來。
const MIN_GRAB: i32 = 32;

/// 這個位置現在還抓得回來嗎？
///
/// 只看拖曳條那一條（視窗頂端的 `MIN_GRAB` 像素），而且要求它露出來的部分
/// **兩個方向都夠寬**，不是「面積大於零」。
///
/// 第一版寫的正是「面積大於零」，測試當場打臉：視窗放在 x=1919、螢幕寬
/// 1920，於是有 1×32 = 32 的面積露在螢幕上，判定「看得到」——而使用者面前
/// 是一條一個像素寬的線。面積會把「很扁」和「夠大」混為一談。
pub fn is_reachable(win: Rect, monitors: &[Rect]) -> bool {
    let bar_h = MIN_GRAB.min(win.h);
    monitors.iter().any(|m| {
        let w = (win.right().min(m.right()) - win.x.max(m.x)).max(0);
        let h = ((win.y + bar_h).min(m.bottom()) - win.y.max(m.y)).max(0);
        w >= MIN_GRAB && h >= bar_h
    })
}

/// 把視窗搬回看得到的地方。已經看得到就原樣不動。
///
/// 選哪一個螢幕：先選重疊最多的那個（使用者本來就想放那邊，只是超出去了），
/// 完全沒重疊才退回第一個螢幕。
pub fn nudge_onto(win: Rect, monitors: &[Rect]) -> Rect {
    if monitors.is_empty() || is_reachable(win, monitors) {
        return win;
    }

    let target = monitors
        .iter()
        .max_by_key(|m| win.overlap(m))
        .copied()
        .unwrap_or(monitors[0]);

    Rect {
        // 螢幕比視窗小的情況（遠端桌面、很怪的解析度）用 `min` 夾在後面，
        // 讓左上角優先——寧可右下超出去，也不要標題列跑到螢幕上方外面。
        x: win
            .x
            .clamp(target.x, (target.right() - win.w).max(target.x)),
        y: win
            .y
            .clamp(target.y, (target.bottom() - win.h).max(target.y)),
        ..win
    }
}

/// 第一次啟動時放哪裡：主螢幕的右下角，留一點邊。
pub fn first_run_corner(win_w: i32, win_h: i32, primary: Rect) -> (i32, i32) {
    const MARGIN: i32 = 16;
    (
        primary.right() - win_w - MARGIN,
        primary.bottom() - win_h - MARGIN,
    )
}

/// 把畫面檔變成 `data:` URL 用的 base64。
///
/// 自己寫而不是拉一個 crate：只用在這一個地方，而且是標準裡最短的那種編碼。
/// 但它放在**這個** crate 而不是桌面程式裡，理由跟 [`is_reachable`] 一樣——
/// 桌面那邊的測試在 Linux 上一行都跑不到，而一個沒有測試的手寫 base64 是
/// 那種「看起來對、在某個長度上錯掉」的東西。補位那段就有分支：長度除以 3
/// 餘 1 補兩個 `=`、餘 2 補一個。
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from_be_bytes([
            0,
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 0x3F] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// 讀。**任何一種壞掉都回 `None`**——壞掉的設定檔不該讓她開不起來，
/// 大不了回到右下角重新開始。
pub fn load(path: &Path) -> Option<PetState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 寫。寫不進去只留一行 log：位置記不住是小事，為了它讓她關不掉是大事。
pub fn save(path: &Path, state: &PetState) {
    let Ok(text) = serde_json::to_string(state) else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("建不出設定目錄，位置記不住：{err}");
        return;
    }
    if let Err(err) = std::fs::write(path, text) {
        tracing::warn!("寫不進視窗位置：{err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PET: (i32, i32) = (340, 560);

    fn win(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            w: PET.0,
            h: PET.1,
        }
    }

    fn monitor(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            w: 1920,
            h: 1080,
        }
    }

    #[test]
    fn a_window_on_the_only_screen_is_left_alone() {
        let screens = [monitor(0, 0)];
        let w = win(1400, 400);
        assert!(is_reachable(w, &screens));
        assert_eq!(nudge_onto(w, &screens), w);
    }

    /// 這就是 TokenMonster 那個 bug 的劇本：她被放在右邊那台螢幕上，
    /// 那台螢幕不見了，於是座標指向一個不存在的地方。
    #[test]
    fn unplugging_the_screen_she_lived_on_brings_her_back() {
        let both = [monitor(0, 0), monitor(1920, 0)];
        let w = win(2400, 300);
        assert!(is_reachable(w, &both));

        let alone = [monitor(0, 0)];
        assert!(
            !is_reachable(w, &alone),
            "副螢幕拔掉之後這個座標應該要被判定成看不到"
        );

        let moved = nudge_onto(w, &alone);
        assert!(
            is_reachable(moved, &alone),
            "搬完之後還是看不到，那就白搬了：{moved:?}"
        );
        // 尺寸不該被動到——她是 resizable: false。
        assert_eq!((moved.w, moved.h), PET);
    }

    /// 只露出一個像素不算拿得回來。這條測試釘的是 `MIN_GRAB` 不能被改成 0——
    /// 改成 0 的話上面那條測試仍然會過，而使用者仍然抓不到她。
    #[test]
    fn a_sliver_of_window_is_not_good_enough() {
        let screens = [monitor(0, 0)];
        assert!(
            !is_reachable(win(1919, 500), &screens),
            "只有一個像素在螢幕上，沒有人抓得到那一條"
        );
        assert!(is_reachable(win(1800, 500), &screens));
    }

    /// 拖曳條在上面，所以「露出下半身」不算數——抓得到的只有那條。
    #[test]
    fn showing_only_her_feet_does_not_count() {
        let screens = [monitor(0, 0)];
        // 視窗頂端在螢幕上緣之外，只有身體下半段露在螢幕裡。
        let w = win(600, -80);
        assert!(
            !is_reachable(w, &screens),
            "拖曳條已經在螢幕外面了，露出來的部分抓不動她"
        );
        assert!(is_reachable(nudge_onto(w, &screens), &screens));
    }

    #[test]
    fn she_goes_back_to_the_screen_she_was_mostly_on() {
        let screens = [monitor(0, 0), monitor(1920, 0)];
        // 大部分身體在右邊那台，但整個掉到下方外面去了。
        let w = win(2200, 1500);
        let moved = nudge_onto(w, &screens);
        assert!(
            moved.x >= 1920,
            "她本來就在右邊那台，不該被搬回左邊：{moved:?}"
        );
        assert!(is_reachable(moved, &screens));
    }

    #[test]
    fn no_screens_at_all_is_not_a_crash() {
        let w = win(100, 100);
        assert_eq!(nudge_onto(w, &[]), w);
    }

    /// RFC 4648 的向量。三種餘數各一條——補位是這段唯一有分支的地方，
    /// 而一個只在「長度剛好整除 3」時正確的 base64 會讓某些畫面開不起來，
    /// 其他的都好好的。那種 bug 最難從症狀反推。
    #[test]
    fn base64_matches_the_standard_vectors() {
        for (raw, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(raw.as_bytes()), want, "輸入 {raw:?}");
        }
    }

    /// 畫面檔是二進位，不是文字。WebP 的檔頭就有 0x00 與高位元組。
    #[test]
    fn base64_survives_bytes_that_are_not_text() {
        assert_eq!(base64(&[0x00, 0xFF, 0x80]), "AP+A");
        assert_eq!(base64(&[0xFF; 4]), "/////w==");
        // 長度是 4/3 進位到 4 的倍數，任何長度都成立。
        for len in 0..64usize {
            let encoded = base64(&vec![0xABu8; len]);
            assert_eq!(encoded.len(), len.div_ceil(3) * 4, "長度 {len}");
        }
    }

    #[test]
    fn a_corrupt_state_file_is_a_shrug_not_a_failure() {
        let dir = std::env::temp_dir().join("sister-bounds-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("pet.json");

        std::fs::write(&path, "{ 這不是 json").expect("write");
        assert_eq!(load(&path), None);

        let state = PetState {
            x: 12,
            y: 34,
            pinned: false,
        };
        save(&path, &state);
        assert_eq!(load(&path), Some(state));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
