//! 「該開的東西，真的出現在畫面上了嗎」。
//!
//! 交出一步之後她會等下一張畫面（alpha.81），然後說「做完之後的畫面憑據是
//! frame #12，圖在」——那句話只證明**畫面變了**，不證明變成了他要的樣子。
//! 換上來的可能是錯誤頁、登入牆、完全另一個分頁。這一支就是去比那一列
//! `frames` 上本來就記著的 `url` / `window_title`。
//!
//! 三格：對得上、對不上、**說不準**。第三格是預設情況而不是邊角料，
//! 而且它自己還要再分開講，因為使用者能做的事不一樣：
//!
//! * 那一欄**是空的**（`NothingOnScreen`）。`frames.url` 的唯一產品寫入端是
//!   `sister-capture` 的錄影迴圈，而那條路要同時滿足三件事才會有值：跑在
//!   Windows、前景 app 在 `browsers` 白名單上、UIA 那一次真的讀回來
//!   （`windows/focus.rs:66` 非瀏覽器直接 return，`windows/uia.rs` 連卡三次
//!   之後整組關掉）。所以「空的」是常態，不是故障。
//! * 那一欄**有值、但不是一個看得出網站的網址**（`ScreenUrlUnreadable`）。
//!   實測走得到：UIA 的 `plausible_url` 明文收 `about:` 和 `chrome:`
//!   （`uia.rs:494`），而 `about:blank` 進 `url_host` 之後 host 算出來是
//!   `about`，沒有點，`looks_like_host` 不收 → `None`。開新分頁就是這一格。
//! * **目標那一側**也有同樣的兩格（`NothingInTheAsk` / `AskUrlUnreadable`）。
//!
//! 三格都不可以被歸進「對不上」，那會把一次讀不到 UIA 講成「她開錯了」。
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
                // 目標**有字**（`url` 一路走過授權才會到這裡），只是抽不出
                // 網站名。和「這一步沒有目標」是兩件事，句子也不一樣。
                return TargetOnScreen::CannotTell {
                    why: CannotTell::AskUrlUnreadable,
                };
            };
            // 這裡要分兩格，不能合成一句「沒有記下網址」：
            //   欄位是 `None`  → 她那一刻沒探到（UIA 沒回應、不是瀏覽器）
            //   欄位有值但 `url_host` 回 `None` → 探到了，但那不是網址
            //     （`about:blank`、`file:///C:/x.pdf`、半截字串）
            // r29 早先兩格共用前一句，而後一句的情況下機器**確實記下了**。
            let saw = match screen.url.as_deref() {
                None => {
                    return TargetOnScreen::CannotTell {
                        why: CannotTell::NothingOnScreen {
                            field: ScreenField::Url,
                        },
                    };
                }
                Some(raw) => match crate::segment::url_host(raw) {
                    Some(host) => host,
                    None => {
                        return TargetOnScreen::CannotTell {
                            why: CannotTell::ScreenUrlUnreadable,
                        };
                    }
                },
            };
            compare_hosts(saw, wanted)
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

/// 比兩個網站名。`www.` 在比之前先剝掉，**兩邊都剝**。
///
/// 這不是「順手放寬一點」，是這個產品自己已經寫下來的事實。
/// `config.rs` 在教使用者寫規則的時候就說過：
///
/// > 以 `www.` 開頭：網址列會省略 `www.`，這條規則不會命中
///
/// 也就是：使用者要開的是 `https://www.gov.tw/x`，Chrome 網址列（＝UIA
/// 寫進 `frames.url` 的東西）顯示的是 `gov.tw/x`。少了這一步，一次
/// **完全成功**的動作會被講成「那張畫面的網址在 gov.tw 上，不是你要開的
/// www.gov.tw——這一步有沒有真的做到，她沒有把握」。而 `www.` 開頭正是
/// 一般人手上最常見的那種網址，所以這不是邊角料，是預設會誤報。
///
/// `facts.rs:548` 早就在做同一件正規化了；這裡是第三個知道這件事、
/// 卻沒做的地方，補上去。
///
/// 判準寫成一句話：**兩個網站名相等，或者其中一個是另一個加上一層
/// 開頭的 `www.`**。網址列少印的就是那一層，不多不少。
///
/// 所以不是「把所有 `www.` 都剝掉再比」——那會把
/// `www.www.evil.com` 和 `evil.com` 當成同一個網站，而它們不是。
/// `bwww.example.com` 也一個字都不剝：`www.` 只在開頭才算前綴。
///
/// 回傳的 `saw` / `wanted` 是**沒剝過的原樣**——句子要讓使用者認得出
/// 自己打的那串字，正規化只發生在「相不相等」這個判斷上。
fn compare_hosts(saw: String, wanted: String) -> TargetOnScreen {
    fn bare(host: &str) -> &str {
        host.strip_prefix("www.").unwrap_or(host)
    }
    let field = ScreenField::Url;
    let same = saw == wanted || bare(&saw) == wanted || saw == bare(&wanted);
    if same {
        TargetOnScreen::Matched { field, saw, wanted }
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
            // 對得上那一句要印得出 `wanted`，因為這裡是**子字串**比對：
            // 標題「登入 — 健保存摺」含有「健保存摺」。只印 `saw` 的話
            // 句子會寫成「畫面的標題是 X」，把一次子字串命中講成相等。
            wanted: wanted.to_owned(),
        }
    } else {
        TargetOnScreen::Mismatched {
            field: ScreenField::WindowTitle,
            saw,
            wanted: wanted.to_owned(),
        }
    }
}
