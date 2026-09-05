//! r34 的收貨考題（行為那半）。
//!
//! **在派工出去之前寫的**，我沒看過成品。斷言的都是使用者按下去會發生什麼，
//! 不綁修法：不提正規化函式的名字、不管它是自己補表還是改用 global-hotkey
//! 的解析器、也不讀它的內部狀態。只問 `plan_hotkeys` 這個出口。
//!
//! 每一對字串都是**產品自己生得出來的**：`comboOf` 推 `e.code`（`ArrowUp`、
//! `KeyH`），`config.toml` 是人手打的而 `config.rs:297` 那行 doc 明文把
//! `CommandOrControl+Shift+空白` 列為合法寫法。
//!
//! 現在（r33）的實測：`the_same_physical_key…` 紅、`two_different_keys…` 紅。

use sister_hands::kill_switch::plan_hotkeys;

/// 同一顆實體鍵的每一種寫法，都要認得出是撞號。
///
/// 認不出來的後果是：兩顆都送去註冊 → 後註冊的那顆吃 `AlreadyRegistered`。
/// 對照 `global-hotkey-0.8.0/src/hotkey.rs` 的實際對照表：`:198-220` 是修飾鍵
/// （`OPTION|ALT`、`COMMANDORCONTROL|…` 非 macOS 對映成 CONTROL），
/// `:232` 起是 `parse_key` 的主鍵別名。
#[test]
fn the_same_physical_key_always_collides() {
    for (pause, hands) in [
        ("CommandOrControl+Alt+H", "Ctrl+Alt+H"),
        ("Option+Ctrl+H", "Alt+Ctrl+H"),
        ("Ctrl+Alt+ArrowUp", "Ctrl+Alt+Up"),
        ("Ctrl+Alt+KeyH", "Ctrl+Alt+H"),
        ("Ctrl+Alt+Digit1", "Ctrl+Alt+1"),
    ] {
        let plan = plan_hotkeys(pause, hands);
        assert!(
            plan.collided.is_some(),
            "{pause} 和 {hands} 是同一顆實體鍵（global-hotkey 兩串 parse 成同一個\
             `(modifiers, code)`）。認不出來的話兩顆都送去註冊，而註冊順序決定\
             死掉的是誰——死的那顆一旦是拔手，他按下去什麼都不會發生。"
        );
        assert!(plan.hands.is_some(), "撞號的時候拔手要留下來：它是安全鍵。");
        assert!(
            plan.pause.is_none(),
            "撞號的時候暫停要讓位，它還能從系統匣按。"
        );
    }
}

/// 反方向：不同的鍵不可以被算成撞號。
///
/// 這一族是 r33 **新造**的傷害——舊碼逐字比較，永遠不會把兩顆不同的鍵判成相等。
/// 假撞號的代價是他平白少一顆熱鍵，而畫面上只會說「撞號了」，那是一句假話。
#[test]
fn two_different_keys_never_collide() {
    for (pause, hands) in [
        ("Alt+H", "CommandOrControl+Alt+H"),
        ("Ctrl+H", "Option+Ctrl+H"),
        ("Super+Alt+H", "Ctrl+Alt+H"),
        ("Ctrl+Alt+KeyH", "Ctrl+Alt+KeyJ"),
        ("Ctrl+Shift+KeyH", "Ctrl+Alt+H"),
        ("Ctrl+Alt+ArrowUp", "Ctrl+Alt+ArrowDown"),
    ] {
        let plan = plan_hotkeys(pause, hands);
        assert!(
            plan.collided.is_none(),
            "{pause} 和 {hands} 按下去是兩件事，不是撞號。"
        );
        assert!(plan.pause.is_some(), "沒撞號就不可以拿掉他的暫停鍵。");
        assert!(plan.hands.is_some(), "沒撞號就不可以拿掉拔手鍵。");
    }
}

/// 沒設暫停熱鍵不算撞號——正規化很容易把兩邊都收斂成空字串然後判定相等。
#[test]
fn a_blank_shortcut_is_still_not_a_collision() {
    for blank in ["", "   "] {
        let plan = plan_hotkeys(blank, "Ctrl+Alt+H");
        assert!(plan.collided.is_none(), "沒設暫停（{blank:?}）不算撞號。");
        assert!(plan.hands.is_some(), "沒設暫停的時候拔手照樣要排。");
    }
}
