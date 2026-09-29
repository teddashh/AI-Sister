//! 受控 staging adapter：共用不可逆 dispatch 隘口的唯一 executor。
//! 只更改記憶體 fixture，沒有第三方帳號、金流、發文或網路出口。

use crate::irreversible::{self, IrreversibleSink};
pub use crate::irreversible::{
    IrreversibleIntent, IrreversibleOutcome as StagingOutcome,
    IrreversibleRefusal as StagingRefusal, LIVE_APPROVAL_TTL_MS, LiveApprover, LiveDecision,
};
use fs4::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalInputOrigin {
    #[default]
    ScriptedFixture,
    HumanTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StagingEvent {
    Proposed {
        intent: IrreversibleIntent,
    },
    Refused {
        intent: IrreversibleIntent,
        why: StagingRefusal,
    },
    Approved {
        intent: IrreversibleIntent,
        #[serde(default)]
        input_origin: ApprovalInputOrigin,
    },
    Executed {
        intent: IrreversibleIntent,
    },
}

#[derive(Debug)]
pub struct StagingFixture {
    pub events: Vec<StagingEvent>,
    pub completed: Vec<IrreversibleIntent>,
    input_origin: ApprovalInputOrigin,
}

impl Default for StagingFixture {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            completed: Vec::new(),
            input_origin: ApprovalInputOrigin::ScriptedFixture,
        }
    }
}

impl StagingFixture {
    pub fn with_input_origin(input_origin: ApprovalInputOrigin) -> Self {
        Self {
            input_origin,
            ..Self::default()
        }
    }
    /// 每次都走正式編譯的共用 dispatch；staging sink 只記錄本機結果。
    pub fn dispatch(
        &mut self,
        intent: IrreversibleIntent,
        approver: &mut impl LiveApprover,
        now_ms: impl Fn() -> i64,
        stopped: impl Fn() -> bool,
    ) -> StagingOutcome {
        irreversible::dispatch(self, intent, approver, now_ms, stopped)
    }

    pub fn write_jsonl(&self, path: &Path) -> io::Result<()> {
        fs::write(path, self.jsonl()?)
    }

    /// The executable staging route keeps every run in the same replayable log.
    /// A file lock prevents concurrent staging processes from interleaving rows.
    pub fn append_jsonl(&self, path: &Path) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;
        FileExt::lock(&file)?;
        let result = file
            .write_all(self.jsonl()?.as_bytes())
            .and_then(|_| file.sync_data());
        FileExt::unlock(&file)?;
        result
    }

    /// 回放只是讀證據；它不含核准票，也不呼叫 dispatch 或 executor。
    pub fn replay_jsonl(path: &Path) -> io::Result<Vec<StagingEvent>> {
        let mut file = fs::File::open(path)?;
        FileExt::lock_shared(&file)?;
        let mut raw = String::new();
        let result = file.read_to_string(&mut raw);
        FileExt::unlock(&file)?;
        result?;
        raw.lines()
            .map(|line| serde_json::from_str(line).map_err(io::Error::other))
            .collect()
    }

    fn jsonl(&self) -> io::Result<String> {
        let mut out = String::new();
        for event in &self.events {
            out.push_str(&serde_json::to_string(event).map_err(io::Error::other)?);
            out.push('\n');
        }
        Ok(out)
    }
}

impl IrreversibleSink for StagingFixture {
    fn proposed(&mut self, intent: &IrreversibleIntent) {
        self.events.push(StagingEvent::Proposed {
            intent: intent.clone(),
        });
    }
    fn refused(&mut self, intent: IrreversibleIntent, why: StagingRefusal) {
        self.events.push(StagingEvent::Refused { intent, why });
    }
    fn approved(&mut self, intent: &IrreversibleIntent) {
        self.events.push(StagingEvent::Approved {
            intent: intent.clone(),
            input_origin: self.input_origin,
        });
    }
    fn execute(&mut self, intent: IrreversibleIntent) {
        self.events.push(StagingEvent::Executed {
            intent: intent.clone(),
        });
        self.completed.push(intent);
    }
}

impl crate::irreversible::sealed::Sealed for StagingFixture {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NeverInherited;
    use std::{
        cell::Cell,
        sync::atomic::{AtomicU32, Ordering},
    };

    struct Approver<'a> {
        decision: LiveDecision,
        shown: Vec<IrreversibleIntent>,
        clock: &'a Cell<i64>,
        delay_ms: i64,
    }
    impl LiveApprover for Approver<'_> {
        fn present_and_ask(&mut self, intent: &IrreversibleIntent) -> LiveDecision {
            self.shown.push(intent.clone());
            self.clock.set(self.clock.get() + self.delay_ms);
            self.decision
        }
    }

    fn approver<'a>(clock: &'a Cell<i64>, decision: LiveDecision, delay_ms: i64) -> Approver<'a> {
        Approver {
            decision,
            shown: Vec::new(),
            clock,
            delay_ms,
        }
    }

    #[test]
    fn all_five_classes_use_live_dispatch_and_replay_is_read_only() {
        assert_eq!(NeverInherited::ALL.len(), 5);
        for class in NeverInherited::ALL {
            let intent = IrreversibleIntent {
                class,
                target: format!("staging://target/{}", class.name()),
                details: "local controlled fixture".into(),
            };
            let clock = Cell::new(100_000);
            let mut fixture = StagingFixture::default();

            let mut declined = approver(&clock, LiveDecision::Decline, 0);
            assert_eq!(
                fixture.dispatch(intent.clone(), &mut declined, || clock.get(), || false),
                StagingOutcome::Refused(StagingRefusal::Declined)
            );
            assert_eq!(declined.shown.as_slice(), std::slice::from_ref(&intent));
            assert!(fixture.completed.is_empty());

            let mut stopped = approver(&clock, LiveDecision::Stop, 0);
            assert_eq!(
                fixture.dispatch(intent.clone(), &mut stopped, || clock.get(), || false),
                StagingOutcome::Refused(StagingRefusal::Stopped)
            );
            assert!(fixture.completed.is_empty());

            let mut expired = approver(&clock, LiveDecision::Approve, LIVE_APPROVAL_TTL_MS + 1);
            assert_eq!(
                fixture.dispatch(intent.clone(), &mut expired, || clock.get(), || false),
                StagingOutcome::Refused(StagingRefusal::Expired)
            );
            assert!(fixture.completed.is_empty());

            let mut approved = approver(&clock, LiveDecision::Approve, 1);
            assert_eq!(
                fixture.dispatch(intent.clone(), &mut approved, || clock.get(), || false),
                StagingOutcome::Executed
            );
            assert_eq!(approved.shown.as_slice(), std::slice::from_ref(&intent));
            assert_eq!(fixture.completed.as_slice(), std::slice::from_ref(&intent));
            assert_eq!(
                fixture
                    .events
                    .iter()
                    .filter(|event| matches!(event, StagingEvent::Executed { .. }))
                    .count(),
                1
            );

            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "sister-stage-approval-{}-{}.jsonl",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fixture.write_jsonl(&path).unwrap();
            let replayed = StagingFixture::replay_jsonl(&path).unwrap();
            fs::remove_file(&path).unwrap();
            assert_eq!(replayed, fixture.events);
            assert!(StagingFixture::default().completed.is_empty());
        }
    }

    #[test]
    fn stop_and_clock_rollback_refuse_after_approval_before_dispatch() {
        let intent = IrreversibleIntent {
            class: NeverInherited::Pay,
            target: "staging://pay".into(),
            details: "synthetic".into(),
        };
        let clock = Cell::new(10_000);
        let mut fixture = StagingFixture::default();
        let mut approved = approver(&clock, LiveDecision::Approve, -1);
        assert_eq!(
            fixture.dispatch(intent.clone(), &mut approved, || clock.get(), || false),
            StagingOutcome::Refused(StagingRefusal::Expired)
        );
        let mut approved = approver(&clock, LiveDecision::Approve, 0);
        let calls = Cell::new(0);
        assert_eq!(
            fixture.dispatch(
                intent,
                &mut approved,
                || clock.get(),
                || {
                    calls.set(calls.get() + 1);
                    calls.get() > 1
                }
            ),
            StagingOutcome::Refused(StagingRefusal::Stopped)
        );
        assert!(fixture.completed.is_empty());
    }
}
