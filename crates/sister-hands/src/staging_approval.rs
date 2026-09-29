//! 受控 staging 的不可逆動作即時核准證據。正式 build 不含此 module。
//! 這裡只能改動記憶體中的 staging fixture，沒有平台 executor 或網路出口。

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

/// UI 必須把具體 class、target 與 details 同時顯示後才回決定。
pub trait LiveApprover {
    fn present_and_ask(&mut self, intent: &IrreversibleIntent) -> LiveDecision;
}

/// 不可序列化的單步票；不能由 standing grant 或舊 action log 重播鑄出。
pub struct LiveIrreversibleApproval(IrreversibleIntent);

pub fn request_live_approval(
    intent: &IrreversibleIntent,
    approver: &mut impl LiveApprover,
) -> Option<LiveIrreversibleApproval> {
    match approver.present_and_ask(intent) {
        LiveDecision::Approve => Some(LiveIrreversibleApproval(intent.clone())),
        LiveDecision::Decline | LiveDecision::Stop => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StagingEvent {
    Proposed {
        intent: IrreversibleIntent,
    },
    Refused {
        intent: IrreversibleIntent,
        why: String,
    },
    Approved {
        intent: IrreversibleIntent,
    },
    Executed {
        intent: IrreversibleIntent,
    },
}

#[derive(Debug, Default)]
pub struct StagingFixture {
    pub events: Vec<StagingEvent>,
    pub completed: Vec<IrreversibleIntent>,
}

impl StagingFixture {
    pub fn propose(&mut self, intent: IrreversibleIntent) {
        self.events.push(StagingEvent::Proposed { intent });
    }

    /// 唯一 staging 執行入口。只記錄到本機 fixture；沒有第三方副作用。
    pub fn complete(
        &mut self,
        intent: IrreversibleIntent,
        approval: Option<LiveIrreversibleApproval>,
    ) -> bool {
        let Some(approval) = approval else {
            self.events.push(StagingEvent::Refused {
                intent,
                why: "needs_live_approval".into(),
            });
            return false;
        };
        if approval.0 != intent {
            self.events.push(StagingEvent::Refused {
                intent,
                why: "approval_was_for_another_step".into(),
            });
            return false;
        }
        self.events.push(StagingEvent::Approved {
            intent: intent.clone(),
        });
        self.events.push(StagingEvent::Executed {
            intent: intent.clone(),
        });
        self.completed.push(intent);
        true
    }

    pub fn replay_jsonl(&self) -> Result<Vec<StagingEvent>, serde_json::Error> {
        self.events
            .iter()
            .map(|event| {
                let line = serde_json::to_string(event)?;
                serde_json::from_str(&line)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StagingApprover {
        decision: LiveDecision,
        shown: Vec<IrreversibleIntent>,
    }
    impl LiveApprover for StagingApprover {
        fn present_and_ask(&mut self, intent: &IrreversibleIntent) -> LiveDecision {
            self.shown.push(intent.clone());
            self.decision
        }
    }

    #[test]
    fn every_irreversible_class_requires_a_fresh_exact_live_approval() {
        assert_eq!(NeverInherited::ALL.len(), 5);
        for class in NeverInherited::ALL {
            let intent = IrreversibleIntent {
                class,
                target: format!("staging://target/{}", class.name()),
                details: "controlled fixture only".into(),
            };
            let mut fixture = StagingFixture::default();
            fixture.propose(intent.clone());
            assert!(!fixture.complete(intent.clone(), None));
            assert!(fixture.completed.is_empty());

            let mut declined = StagingApprover {
                decision: LiveDecision::Decline,
                shown: Vec::new(),
            };
            assert!(request_live_approval(&intent, &mut declined).is_none());
            assert_eq!(declined.shown.as_slice(), std::slice::from_ref(&intent));
            let mut stopped = StagingApprover {
                decision: LiveDecision::Stop,
                shown: Vec::new(),
            };
            assert!(request_live_approval(&intent, &mut stopped).is_none());

            let mut approved = StagingApprover {
                decision: LiveDecision::Approve,
                shown: Vec::new(),
            };
            let ticket = request_live_approval(&intent, &mut approved);
            assert_eq!(approved.shown.as_slice(), std::slice::from_ref(&intent));
            let mut changed = intent.clone();
            changed.target.push_str("/changed");
            assert!(!fixture.complete(changed, ticket));
            assert!(fixture.completed.is_empty());

            let ticket = request_live_approval(&intent, &mut approved);
            assert!(fixture.complete(intent.clone(), ticket));
            assert_eq!(fixture.completed.as_slice(), std::slice::from_ref(&intent));
            assert_eq!(fixture.replay_jsonl().unwrap(), fixture.events);
            assert_eq!(
                fixture
                    .events
                    .iter()
                    .filter(|event| matches!(event, StagingEvent::Executed { .. }))
                    .count(),
                1
            );
        }
    }
}
