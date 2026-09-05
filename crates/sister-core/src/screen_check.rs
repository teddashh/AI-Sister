//! 「該開的東西，真的出現在畫面上了嗎」。
//!
//! 交出一步之後她會等下一張畫面（alpha.81），然後說「做完之後的畫面憑據是
//! frame #12，圖在」——那句話只證明**畫面變了**，不證明變成了他要的樣子。
//! 換上來的可能是錯誤頁、登入牆、完全另一個分頁。這一支就是去比那一列
//! `frames` 上本來就記著的 `url` / `window_title`。
//!
//! 三格：對得上、對不上、**說不準**。第三格是預設情況而不是邊角料——
//! `frames.url` 只有 Windows 的 UIA 探得到的視窗才有值——所以它一定要說得
//! 像說不準，不可以被歸進「對不上」，那會把一次讀不到 UIA 講成「她開錯了」。
//!
//! 住在 `sister-core` 而不是 `sister-hands`，是因為 `url_host` 在
//! `segment.rs`，而相依方向是 `sister-core` → `sister-hands`（反過來不行）。
//! 在 `sister-hands` 裡再抄一份 host 抽取就會漂。

use sister_hands::ActionSnapshot;
use sister_hands::semi_action::{CannotTell, ScreenAfter, ScreenField, TargetOnScreen};

pub fn target_on_screen(action: &ActionSnapshot, screen: &ScreenAfter) -> TargetOnScreen {
    match action {
        ActionSnapshot::OpenUrl { url } => {
            let Some(wanted) = crate::segment::url_host(url) else {
                return TargetOnScreen::CannotTell {
                    why: CannotTell::NothingInTheAsk,
                };
            };
            let Some(saw) = screen.url.as_deref().and_then(crate::segment::url_host) else {
                return TargetOnScreen::CannotTell {
                    why: CannotTell::NothingOnScreen {
                        field: ScreenField::Url,
                    },
                };
            };
            compare_exact(ScreenField::Url, saw, wanted)
        }
        ActionSnapshot::FocusWindow { title } => check_window_title(title, screen),
        ActionSnapshot::OpenFile { path } => {
            // `file_name()` 之後**還要**自己切一次反斜線。這不是防禦性程式碼，
            // 是量出來的：`\` 在 Linux 上不是路徑分隔符，`Path` 會把整條
            // Windows 路徑當成一個檔名。剛剛拿 rustc 跑的：
            //
            //   PathBuf::from(r"C:\Users\ted\報表.xlsx").file_name()
            //     Linux   → Some("C:\\Users\\ted\\報表.xlsx")   ← 整串
            //     Windows → Some("報表.xlsx")
            //
            // 產品跑在 Windows 上，所以少了這一行**在出貨平台上看不出差別**；
            // 看得出差別的是 Linux 上的測試和 CI，它們會拿整條路徑去比視窗
            // 標題，於是「開檔案」這一格永遠對不上。這正是「平台差異出現在
            // 沒有任何 cfg 的地方」那一族：把它包進 `#[cfg(windows)]` 等於
            // 讓本機再也證明不了這件事。
            //
            // 代價說清楚：Linux 上 `a\b.txt` 是一個合法的**單一**檔名，這裡
            // 會把它切成 `b.txt`，比對因此變寬鬆。在一個不出貨的平台上放寬
            // 一個不存在的檔名，換 Linux 和 Windows 算出同一個答案。
            let wanted = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or_default()
                .to_owned();
            check_window_title(&wanted, screen)
        }
    }
}

fn compare_exact(field: ScreenField, saw: String, wanted: String) -> TargetOnScreen {
    if saw == wanted {
        TargetOnScreen::Matched { field, saw }
    } else {
        TargetOnScreen::Mismatched { field, saw, wanted }
    }
}

fn check_window_title(wanted: &str, screen: &ScreenAfter) -> TargetOnScreen {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return TargetOnScreen::CannotTell {
            why: CannotTell::NothingInTheAsk,
        };
    }
    let Some(saw) = screen
        .window_title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    else {
        return TargetOnScreen::CannotTell {
            why: CannotTell::NothingOnScreen {
                field: ScreenField::WindowTitle,
            },
        };
    };
    let saw = saw.to_owned();
    if saw.to_lowercase().contains(&wanted.to_lowercase()) {
        TargetOnScreen::Matched {
            field: ScreenField::WindowTitle,
            saw,
        }
    } else {
        TargetOnScreen::Mismatched {
            field: ScreenField::WindowTitle,
            saw,
            wanted: wanted.to_owned(),
        }
    }
}
