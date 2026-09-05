//! r33 的收貨考題（行為那半）。
//!
//! **這個檔案是在派工出去之後、成品回來之前寫的**——寫的時候我沒看過 codex 的
//! 成品，只看得到 `8bb3efa`（r32）那份舊碼。所以它證的是「我先寫下的考題過了」，
//! 不是「它的測試通過它自己的修法」。
//!
//! 每一條斷言的都是**使用者看得到的關係**，不綁任何一種修法：不提正規化函式的
//! 名字、不提它認得哪些別名、不讀它的內部狀態。只問 `plan_hotkeys` 這個既有的
//! 出口：兩串產品自己生得出來的字，它認不認得那是同一顆鍵。
//!
//! 舊碼上的預期：`the_two_real_writers…` 和 `the_shapes_the_product…` 紅，
//! 另外兩條綠（那兩條守的是修法**沒有**放寬過頭）。

use sister_core::config::Config;
use sister_hands::kill_switch::plan_hotkeys;

/// 產品自己那兩個寫入端吐出來的兩串字，指的是同一顆實體鍵。
///
/// 設定檔那個寫入端（出廠值）吐 key 形狀 `Ctrl+Alt+H`；設定頁那個寫入端
/// （`settings.js` 的 `comboOf` 推 `e.code`）吐 code 形狀 `Ctrl+Alt+KeyH`。
/// 出廠值**跟產品要**、不手抄；code 形狀那一串是手寫的，所以
/// `scripts/check-hands-hotkey-says.py` 的 ① 在守「`comboOf` 還在推 `e.code`」
/// ——那一邊的慣例一改，這裡的配對就要重新推導（那一條紅了要回去重推，不是
/// 把它改綠）。
#[test]
fn the_two_real_writers_are_recognised_as_the_same_key() {
    let factory = Config::default().shell.hands_stop_shortcut;
    let from_settings_page = "Ctrl+Alt+KeyH";

    let plan = plan_hotkeys(from_settings_page, &factory);

    assert!(
        plan.collided.is_some(),
        "使用者在設定頁把暫停設成和出廠拔手同一顆鍵（{from_settings_page} vs \
         {factory}），撞號要認得出來。認不出來的話兩顆都會送去註冊，第二顆拿到 \
         AlreadyRegistered——而註冊順序讓死掉的那顆是拔手，也就是那顆安全鍵。"
    );
    assert!(
        plan.hands.is_some(),
        "撞號的時候拔手要留下來：它是安全鍵，而暫停還能從系統匣按。"
    );
    assert!(
        plan.pause.is_none(),
        "撞號的時候暫停要讓位。這不是新規則——`plan_hotkeys` 的 doc 從第一版就\
         這樣宣稱，只是以前認不出這一種撞號，所以那句話沒被執行過。"
    );
}

/// 產品自己生得出來的每一種形狀，都要對得上。
///
/// 這一條列的三種都不是假想的：`comboOf` 生 `KeyH`／`Digit1`（`e.code`），
/// 而 `hands_stop_shortcut` 和 `pause_shortcut` 都是手改 `config.toml` 的欄位，
/// 人手打出來的大小寫和修飾鍵順序不受任何東西約束。派工單 §1 把這四種形狀
/// 明列為範圍。
#[test]
fn the_shapes_the_product_can_actually_produce_are_matched() {
    let factory = Config::default().shell.hands_stop_shortcut;

    for from_settings_page in ["Ctrl+Alt+KeyH", "ctrl+alt+keyh", "Alt+Ctrl+KeyH"] {
        assert!(
            plan_hotkeys(from_settings_page, &factory)
                .collided
                .is_some(),
            "{from_settings_page} 和 {factory} 是同一顆實體鍵（大小寫／修飾鍵\
             順序／`Key` 前綴都不改變按下去的是哪一顆）。"
        );
    }

    assert!(
        plan_hotkeys("Ctrl+Alt+Digit1", "Ctrl+Alt+1")
            .collided
            .is_some(),
        "數字鍵那一族同理：`comboOf` 吐 `Digit1`，手改的設定檔寫 `1`。"
    );
}

/// 反方向：不可以為了認出撞號，就把不同的鍵也算成撞號。
///
/// 這一條在舊碼上是**綠**的，它守的是修法沒有放寬過頭。正規化寫得太貪心的話，
/// 使用者會平白少一顆熱鍵，而且畫面上只會說「撞號了」——那是一句假話。
#[test]
fn two_different_keys_still_do_not_collide() {
    let plan = plan_hotkeys("Ctrl+Alt+KeyH", "Ctrl+Alt+KeyJ");
    assert!(plan.collided.is_none(), "H 和 J 是兩顆鍵，不是撞號。");
    assert!(
        plan.pause.is_some(),
        "沒撞號的時候，暫停那一顆不可以被拿掉。"
    );
    assert!(plan.hands.is_some(), "沒撞號的時候，拔手那一顆照樣要排。");

    let plan = plan_hotkeys("Ctrl+Shift+KeyH", "Ctrl+Alt+H");
    assert!(
        plan.collided.is_none(),
        "同一顆字母、不同修飾鍵，按下去是兩件事，不是撞號。"
    );
    assert!(plan.pause.is_some(), "修飾鍵不同就不該讓位。");
}

/// 沒設暫停熱鍵不算撞號——空字串和任何東西都不是同一顆鍵。
///
/// 舊碼上也是綠的。放進來是因為正規化很容易把兩邊都 trim 成空字串然後判定相等，
/// 那會讓「我沒設暫停」變成「你們撞號了」，而且拔手那顆的排程也會被牽動。
#[test]
fn a_blank_shortcut_is_not_a_collision() {
    for blank in ["", "   "] {
        let plan = plan_hotkeys(blank, "Ctrl+Alt+H");
        assert!(
            plan.collided.is_none(),
            "沒設暫停（{blank:?}）不算和拔手撞號。"
        );
        assert!(plan.hands.is_some(), "沒設暫停的時候，拔手那一顆照樣要排。");
    }
}
