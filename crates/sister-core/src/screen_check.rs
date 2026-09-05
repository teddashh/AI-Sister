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
