//! 不可逆動作的共用 dispatch 隘口。正式產品目前沒有這類 executor；
//! controlled staging fixture 由 feature module 實作這個私有 sink。

use crate::NeverInherited;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrreversibleIntent {
    pub class: NeverInherited,
    pub target: String,
    pub details: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDecision {
    Approve,
    Decline,
    Stop,
}

pub trait LiveApprover {
    fn present_and_ask(&mut self, intent: &IrreversibleIntent) -> LiveDecision;
}

pub const LIVE_APPROVAL_TTL_MS: i64 = 30_000;

/// 一次性、不可序列化的具體步驟票；舊 action log 不能重建它。
struct LiveApproval {
    intent: IrreversibleIntent,
    issued_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IrreversibleRefusal {
    NeedsLiveApproval,
    Declined,
    Stopped,
    Expired,
    ApprovalWasForAnotherStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrreversibleOutcome {
    Executed,
    Refused(IrreversibleRefusal),
}

/// crate 外不能提供 sink，避免正式執行檔自行增添不可逆執行器。
pub(crate) mod sealed {
    pub trait Sealed {}
}

pub trait IrreversibleSink: sealed::Sealed {
    fn proposed(&mut self, intent: &IrreversibleIntent);
    fn refused(&mut self, intent: IrreversibleIntent, why: IrreversibleRefusal);
    fn approved(&mut self, intent: &IrreversibleIntent);
    fn execute(&mut self, intent: IrreversibleIntent);
}

/// 唯一的不可逆 dispatch：每次當場呈現具體步驟；沒有 grant 或已存票的入口。
/// 即時核准後在真正執行邊界再驗停止與期限，時鐘倒退也拒絕。
pub fn dispatch(
    sink: &mut impl IrreversibleSink,
    intent: IrreversibleIntent,
    approver: &mut impl LiveApprover,
    now_ms: impl Fn() -> i64,
    stopped: impl Fn() -> bool,
) -> IrreversibleOutcome {
    sink.proposed(&intent);
    if stopped() {
        return refuse(sink, intent, IrreversibleRefusal::Stopped);
    }
    let issued_at_ms = now_ms();
    let approval = match approver.present_and_ask(&intent) {
        LiveDecision::Approve => Some(LiveApproval {
            intent: intent.clone(),
            issued_at_ms,
        }),
        LiveDecision::Decline => return refuse(sink, intent, IrreversibleRefusal::Declined),
        LiveDecision::Stop => return refuse(sink, intent, IrreversibleRefusal::Stopped),
    };
    complete(sink, intent, approval, now_ms(), stopped())
}

fn complete(
    sink: &mut impl IrreversibleSink,
    intent: IrreversibleIntent,
    approval: Option<LiveApproval>,
    now_ms: i64,
    stopped: bool,
) -> IrreversibleOutcome {
    if stopped {
        return refuse(sink, intent, IrreversibleRefusal::Stopped);
    }
    let Some(approval) = approval else {
        return refuse(sink, intent, IrreversibleRefusal::NeedsLiveApproval);
    };
    if approval.intent != intent {
        return refuse(sink, intent, IrreversibleRefusal::ApprovalWasForAnotherStep);
    }
    if !now_ms
        .checked_sub(approval.issued_at_ms)
        .is_some_and(|age| (0..=LIVE_APPROVAL_TTL_MS).contains(&age))
    {
        return refuse(sink, intent, IrreversibleRefusal::Expired);
    }
    sink.approved(&intent);
    sink.execute(intent);
    IrreversibleOutcome::Executed
}

fn refuse(
    sink: &mut impl IrreversibleSink,
    intent: IrreversibleIntent,
    why: IrreversibleRefusal,
) -> IrreversibleOutcome {
    sink.refused(intent, why);
    IrreversibleOutcome::Refused(why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Sink {
        executed: Vec<IrreversibleIntent>,
    }
    impl sealed::Sealed for Sink {}
    impl IrreversibleSink for Sink {
        fn proposed(&mut self, _: &IrreversibleIntent) {}
        fn refused(&mut self, _: IrreversibleIntent, _: IrreversibleRefusal) {}
        fn approved(&mut self, _: &IrreversibleIntent) {}
        fn execute(&mut self, intent: IrreversibleIntent) {
            self.executed.push(intent);
        }
    }

    #[test]
    fn missing_or_wrong_step_ticket_never_reaches_executor() {
        let intent = IrreversibleIntent {
            class: NeverInherited::Pay,
            target: "staging://pay".into(),
            details: "test".into(),
        };
        let mut sink = Sink::default();
        assert_eq!(
            complete(&mut sink, intent.clone(), None, 10, false),
            IrreversibleOutcome::Refused(IrreversibleRefusal::NeedsLiveApproval)
        );
        let mut changed = intent.clone();
        changed.target.push_str("/other");
        let ticket = LiveApproval {
            intent,
            issued_at_ms: 10,
        };
        assert_eq!(
            complete(&mut sink, changed, Some(ticket), 10, false),
            IrreversibleOutcome::Refused(IrreversibleRefusal::ApprovalWasForAnotherStep)
        );
        let ticket = LiveApproval {
            intent: IrreversibleIntent {
                class: NeverInherited::Pay,
                target: "staging://pay".into(),
                details: "test".into(),
            },
            issued_at_ms: i64::MIN,
        };
        assert_eq!(
            complete(
                &mut sink,
                ticket.intent.clone(),
                Some(ticket),
                i64::MAX,
                false
            ),
            IrreversibleOutcome::Refused(IrreversibleRefusal::Expired)
        );
        assert!(sink.executed.is_empty());
    }
}
