use std::path::Path;

/// 一個路徑作為目錄使用時的狀態。
///
/// 抽成共用 IO 邊界，是因為 Windows 對「祖先是檔案」的子路徑 `try_exists`
/// 會回 `Ok(false)`，Linux 卻回 `Err(NotADirectory)`。各呼叫端仍要自己決定
/// 哪個方向才安全；這裡只陳述檔案系統真的回答了什麼。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DirState {
    Dir,
    NotADir,
    Absent,
    Unreadable,
}

pub(crate) fn dir_state(path: &Path) -> DirState {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => DirState::Dir,
        Ok(_) => DirState::NotADir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DirState::Absent,
        Err(_) => DirState::Unreadable,
    }
}
