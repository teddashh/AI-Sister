/// 系統匣上的全停項目是固定方向的命令，不是依目前標籤重算的 toggle。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MasterStopAction {
    Engage,
    Release,
}

/// 把 production menu id 收斂成固定方向。未知 id 不屬於全停 callback。
pub(crate) fn master_stop_action_for_menu_id(id: &str) -> Option<MasterStopAction> {
    match id {
        "master-stop" => Some(MasterStopAction::Engage),
        "master-resume" => Some(MasterStopAction::Release),
        _ => None,
    }
}

/// 單一 receiver 是 tray click 順序的線性化點。不能每次 click 各 spawn 一條 thread
/// 再搶 Mutex：Mutex 沒有 FIFO 保證，Resume→Stop 可能反過來跑成最後是 Clear。
pub(crate) fn run_fifo<T>(receiver: std::sync::mpsc::Receiver<T>, mut run: impl FnMut(T)) {
    for job in receiver {
        run(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_have_fixed_directions_and_unknown_ids_are_ignored() {
        assert_eq!(
            master_stop_action_for_menu_id("master-stop"),
            Some(MasterStopAction::Engage)
        );
        assert_eq!(
            master_stop_action_for_menu_id("master-resume"),
            Some(MasterStopAction::Release)
        );
        assert_eq!(master_stop_action_for_menu_id("pause"), None);
        assert_eq!(
            master_stop_action_for_menu_id("master-stop-stale-label"),
            None
        );
    }
}
