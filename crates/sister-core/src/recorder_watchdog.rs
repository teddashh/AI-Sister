//! Desktop recorder 的純狀態機。
//!
//! 這裡不 spawn 行程、不睡覺，也不讀心跳或控制檔；它只把一個 [`State`] 和
//! 一個 [`Event`] 化成下一個狀態與一個 [`Effect`]。因此 Windows desktop 的
//! 接線可以很薄，而重試、停止與過期 timer 的規則能在 Linux CI 完整跑到。

/// 連續處於 Recording 十分鐘，才把先前的失敗歸零。
pub const HEALTHY_RECORDING_MS: u64 = 10 * 60 * 1_000;

/// 已知 owned child 消失的 wall-clock cutoff。和 heartbeat 時戳分成 newtype，
/// 避免兩個同為 `Option<i64>` 的參數接反仍能編譯。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnedChildGoneAt(Option<crate::Millis>);

impl OwnedChildGoneAt {
    pub const fn new(value: Option<crate::Millis>) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedHeartbeatAt(Option<crate::Millis>);

impl ObservedHeartbeatAt {
    pub const fn new(value: Option<crate::Millis>) -> Self {
        Self(value)
    }
}

/// 沒有 owned child 時，一顆 fresh heartbeat 是否只能屬於外部 recorder。
pub const fn heartbeat_is_external(
    gone_at: OwnedChildGoneAt,
    beat_at: ObservedHeartbeatAt,
) -> bool {
    match (gone_at.0, beat_at.0) {
        // Desktop 這一生未 spawn 過 child；目前 fresh 的那份必然來自外部。
        (None, Some(_)) => true,
        // 已退出 child 不可能在 cutoff 之後再蓋拍。
        (Some(gone), Some(beat)) => beat > gone,
        (None | Some(_), None) => false,
    }
}

/// 某一輪 supervision 的身分。timer 必須把它原樣帶回來。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Generation(u64);

impl Generation {
    pub const INITIAL: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// 呼叫端提供的 monotonic clock；不能拿 wall clock 代替。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonotonicMillis(u64);

impl MonotonicMillis {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    fn after(self, delay: RetryDelay) -> Self {
        Self(self.0.saturating_add(delay.0))
    }

    fn elapsed_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// 下一次嘗試之前要等多久。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryDelay(u64);

impl RetryDelay {
    pub const ONE_SECOND: Self = Self(1_000);
    pub const FIVE_SECONDS: Self = Self(5_000);
    pub const THIRTY_SECONDS: Self = Self(30_000);

    /// Occupancy barrier 要求多久後再看一次。零會造成 busy loop，所以拒絕。
    pub const fn from_millis(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn as_millis(self) -> u64 {
        self.0
    }
}

/// 已累積但尚未被健康 Recording 清掉的失敗數。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureCount(u8);

impl FailureCount {
    pub const ZERO: Self = Self(0);

    pub const fn get(self) -> u8 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderPhase {
    Booting,
    Recording,
    /// Child 還活著，但這一拍沒有可驗證的新鮮 Recording heartbeat。
    /// 這包含 missing／stalled／unreadable／thinking；它們不一定是 failure，
    /// 但一定會打斷「連續 Recording 十分鐘」的健康區間。
    NotRecording,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildExit {
    Success,
    Failure,
}

/// retry timer 到點時，production 接線重新探測到的完整結果。
///
/// `Occupied` 是 barrier，不是 recorder 的一次失敗；`Unknown` 絕不能猜成空房。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryReadiness {
    Ready,
    Occupied {
        retry_after: RetryDelay,
    },
    /// 已退出的 owned child 之後出現一顆更新的心跳；那是另一個 recorder，
    /// desktop 只觀察，不接管、kill 或替它安排 retry。
    External,
    StopRequested,
    ConsentRevoked,
    Quitting,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoppedBy {
    NeverStarted,
    SuccessfulExit,
    ExternalExit,
    Requested,
    ConsentRevoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaveUpCause {
    FourFailures,
    ProbeUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped {
        generation: Generation,
        reason: StoppedBy,
    },
    Starting {
        generation: Generation,
        failures: FailureCount,
    },
    Running {
        generation: Generation,
        failures: FailureCount,
        recording_since: Option<MonotonicMillis>,
    },
    /// `try_wait` 剛剛失敗，所以仍保留唯一的 Child handle，且禁止另開一份。
    /// 下一次成功 probe 會回到 Running 或走正常 exit 路徑。
    ChildUncertain {
        generation: Generation,
        failures: FailureCount,
    },
    Backoff {
        generation: Generation,
        failures: FailureCount,
        retry_not_before: MonotonicMillis,
    },
    GaveUp {
        generation: Generation,
        failures: FailureCount,
        cause: GaveUpCause,
    },
    /// 同一個資料目錄有一份不是 desktop 啟動的 recorder。desktop 只顯示它；
    /// 沒有 Child handle，也不替它重試。
    External {
        generation: Generation,
    },
    /// 已對外部 recorder 留下 durable stop，但還沒從 heartbeat 證明它真的離場。
    /// 這時不能說成 Stopped，也不能把它的最後一拍冒充 owned child 的舊心跳。
    ExternalStopping {
        generation: Generation,
        reason: StoppedBy,
    },
    Quitting {
        generation: Generation,
    },
}

impl State {
    pub const fn initial() -> Self {
        Self::Stopped {
            generation: Generation::INITIAL,
            reason: StoppedBy::NeverStarted,
        }
    }

    pub const fn generation(self) -> Generation {
        match self {
            Self::Stopped { generation, .. }
            | Self::Starting { generation, .. }
            | Self::Running { generation, .. }
            | Self::ChildUncertain { generation, .. }
            | Self::Backoff { generation, .. }
            | Self::GaveUp { generation, .. }
            | Self::External { generation }
            | Self::ExternalStopping { generation, .. }
            | Self::Quitting { generation } => generation,
        }
    }

    pub const fn failures(self) -> FailureCount {
        match self {
            Self::Starting { failures, .. }
            | Self::Running { failures, .. }
            | Self::ChildUncertain { failures, .. }
            | Self::Backoff { failures, .. }
            | Self::GaveUp { failures, .. } => failures,
            Self::Stopped { .. }
            | Self::External { .. }
            | Self::ExternalStopping { .. }
            | Self::Quitting { .. } => FailureCount::ZERO,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::initial()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// 使用者明確要求開始；這是唯一會把 GaveUp 與 failure budget 歸零的入口。
    StartRequested,
    SpawnSucceeded {
        generation: Generation,
    },
    SpawnFailed {
        generation: Generation,
        at: MonotonicMillis,
    },
    PhaseObserved {
        generation: Generation,
        phase: RecorderPhase,
        at: MonotonicMillis,
    },
    ChildExited {
        generation: Generation,
        exit: ChildExit,
        at: MonotonicMillis,
    },
    /// `Child::try_wait` 本身失敗：不知道 child 還活著或已退出，不能猜成 crash
    /// 再 spawn 第二個。
    ChildProbeFailed {
        generation: Generation,
    },
    ChildProbeSucceeded {
        generation: Generation,
    },
    RetryDue {
        generation: Generation,
        at: MonotonicMillis,
        readiness: RetryReadiness,
    },
    ExternalObserved,
    ExternalGone,
    StopRequested,
    ConsentRevoked,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    Spawn {
        generation: Generation,
    },
    ScheduleRetry {
        generation: Generation,
        after: RetryDelay,
    },
    CancelRetry,
    GaveUp {
        generation: Generation,
        cause: GaveUpCause,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub state: State,
    pub effect: Effect,
}

impl Transition {
    const fn new(state: State, effect: Effect) -> Self {
        Self { state, effect }
    }

    const fn unchanged(state: State) -> Self {
        Self::new(state, Effect::None)
    }
}

/// 唯一的狀態轉移入口。
pub fn reduce(state: State, event: Event) -> Transition {
    match event {
        Event::StartRequested => start(state),
        Event::SpawnSucceeded { generation } => spawned(state, generation),
        Event::SpawnFailed { generation, at } => failed(state, generation, at),
        Event::PhaseObserved {
            generation,
            phase,
            at,
        } => phase_observed(state, generation, phase, at),
        Event::ChildExited {
            generation,
            exit: ChildExit::Success,
            ..
        } => successful_exit(state, generation),
        Event::ChildExited {
            generation,
            exit: ChildExit::Failure,
            at,
        } => failed(state, generation, at),
        Event::ChildProbeFailed { generation } => child_probe_failed(state, generation),
        Event::ChildProbeSucceeded { generation } => child_probe_succeeded(state, generation),
        Event::RetryDue {
            generation,
            at,
            readiness,
        } => retry_due(state, generation, at, readiness),
        Event::ExternalObserved => external_observed(state),
        Event::ExternalGone => external_gone(state),
        Event::StopRequested => stop(state, StoppedBy::Requested),
        Event::ConsentRevoked => stop(state, StoppedBy::ConsentRevoked),
        Event::Quit => Transition::new(
            State::Quitting {
                generation: state.generation().next(),
            },
            Effect::CancelRetry,
        ),
    }
}

fn child_probe_failed(state: State, event_generation: Generation) -> Transition {
    let generation = state.generation();
    if generation != event_generation
        || !matches!(state, State::Starting { .. } | State::Running { .. })
    {
        return Transition::unchanged(state);
    }
    Transition::new(
        State::ChildUncertain {
            generation,
            failures: state.failures(),
        },
        Effect::CancelRetry,
    )
}

fn child_probe_succeeded(state: State, event_generation: Generation) -> Transition {
    let State::ChildUncertain {
        generation,
        failures,
    } = state
    else {
        return Transition::unchanged(state);
    };
    if generation != event_generation {
        return Transition::unchanged(state);
    }
    Transition::new(
        State::Running {
            generation,
            failures,
            // A failed process probe breaks the proven continuous healthy window.
            recording_since: None,
        },
        Effect::None,
    )
}

fn external_observed(state: State) -> Transition {
    if matches!(
        state,
        State::External { .. } | State::ExternalStopping { .. } | State::Quitting { .. }
    ) {
        return Transition::unchanged(state);
    }
    if !matches!(
        state,
        State::Stopped { .. } | State::Backoff { .. } | State::GaveUp { .. }
    ) {
        return Transition::unchanged(state);
    }
    Transition::new(
        State::External {
            generation: state.generation().next(),
        },
        Effect::CancelRetry,
    )
}

fn external_gone(state: State) -> Transition {
    let (generation, reason) = match state {
        State::External { generation } => (generation, StoppedBy::ExternalExit),
        State::ExternalStopping { generation, reason } => (generation, reason),
        _ => return Transition::unchanged(state),
    };
    Transition::new(
        State::Stopped {
            generation: generation.next(),
            reason,
        },
        Effect::CancelRetry,
    )
}

fn start(state: State) -> Transition {
    match state {
        State::Starting { .. }
        | State::Running { .. }
        | State::ChildUncertain { .. }
        | State::External { .. }
        | State::ExternalStopping { .. }
        | State::Quitting { .. } => Transition::unchanged(state),
        State::Stopped { .. } | State::Backoff { .. } | State::GaveUp { .. } => {
            let generation = state.generation().next();
            Transition::new(
                State::Starting {
                    generation,
                    failures: FailureCount::ZERO,
                },
                Effect::Spawn { generation },
            )
        }
    }
}

fn spawned(state: State, event_generation: Generation) -> Transition {
    let State::Starting {
        generation,
        failures,
    } = state
    else {
        return Transition::unchanged(state);
    };
    if generation != event_generation {
        return Transition::unchanged(state);
    }
    Transition::new(
        State::Running {
            generation,
            failures,
            recording_since: None,
        },
        Effect::None,
    )
}

fn phase_observed(
    state: State,
    event_generation: Generation,
    phase: RecorderPhase,
    at: MonotonicMillis,
) -> Transition {
    let (generation, failures, recording_since) = match state {
        State::Running {
            generation,
            failures,
            recording_since,
        } => (generation, failures, recording_since),
        State::ChildUncertain {
            generation,
            failures,
        } => (generation, failures, None),
        _ => return Transition::unchanged(state),
    };
    if generation != event_generation {
        return Transition::unchanged(state);
    }

    let (failures, recording_since) = match phase {
        // Booting 可以很久；它不是成功錄製，因此既不 timeout 也不清失敗帳。
        RecorderPhase::Booting | RecorderPhase::NotRecording => (failures, None),
        RecorderPhase::Recording => match recording_since {
            Some(since) if at.elapsed_since(since) >= HEALTHY_RECORDING_MS => {
                (FailureCount::ZERO, Some(since))
            }
            Some(since) => (failures, Some(since)),
            None => (failures, Some(at)),
        },
    };
    Transition::new(
        State::Running {
            generation,
            failures,
            recording_since,
        },
        Effect::None,
    )
}

fn successful_exit(state: State, event_generation: Generation) -> Transition {
    let current = state.generation();
    if current != event_generation
        || !matches!(
            state,
            State::Starting { .. } | State::Running { .. } | State::ChildUncertain { .. }
        )
    {
        return Transition::unchanged(state);
    }
    Transition::new(
        State::Stopped {
            generation: current.next(),
            reason: StoppedBy::SuccessfulExit,
        },
        Effect::CancelRetry,
    )
}

fn failed(state: State, event_generation: Generation, at: MonotonicMillis) -> Transition {
    let (generation, failures) = match state {
        State::Starting {
            generation,
            failures,
        } => (generation, failures),
        State::Running {
            generation,
            failures,
            ..
        } => (generation, failures),
        State::ChildUncertain {
            generation,
            failures,
        } => (generation, failures),
        _ => return Transition::unchanged(state),
    };
    if generation != event_generation {
        return Transition::unchanged(state);
    }

    let failures = failures.next();
    let Some(delay) = retry_delay(failures) else {
        let cause = GaveUpCause::FourFailures;
        return Transition::new(
            State::GaveUp {
                generation,
                failures,
                cause,
            },
            Effect::GaveUp { generation, cause },
        );
    };
    Transition::new(
        State::Backoff {
            generation,
            failures,
            retry_not_before: at.after(delay),
        },
        Effect::ScheduleRetry {
            generation,
            after: delay,
        },
    )
}

fn retry_delay(failures: FailureCount) -> Option<RetryDelay> {
    match failures.get() {
        1 => Some(RetryDelay::ONE_SECOND),
        2 => Some(RetryDelay::FIVE_SECONDS),
        3 => Some(RetryDelay::THIRTY_SECONDS),
        _ => None,
    }
}

fn retry_due(
    state: State,
    event_generation: Generation,
    at: MonotonicMillis,
    readiness: RetryReadiness,
) -> Transition {
    let State::Backoff {
        generation,
        failures,
        retry_not_before,
    } = state
    else {
        return Transition::unchanged(state);
    };
    if generation != event_generation {
        return Transition::unchanged(state);
    }

    // 這三個意圖和 Unknown 不必等 timer：知道不能起來，就立刻讓舊 timer 失效。
    match readiness {
        RetryReadiness::StopRequested => return stop(state, StoppedBy::Requested),
        RetryReadiness::ConsentRevoked => return stop(state, StoppedBy::ConsentRevoked),
        RetryReadiness::Quitting => {
            return Transition::new(
                State::Quitting {
                    generation: generation.next(),
                },
                Effect::CancelRetry,
            );
        }
        RetryReadiness::Unknown => {
            let cause = GaveUpCause::ProbeUnknown;
            return Transition::new(
                State::GaveUp {
                    generation,
                    failures,
                    cause,
                },
                Effect::GaveUp { generation, cause },
            );
        }
        RetryReadiness::External => return external_observed(state),
        RetryReadiness::Ready | RetryReadiness::Occupied { .. } => {}
    }

    match readiness {
        RetryReadiness::Ready if at >= retry_not_before => Transition::new(
            State::Starting {
                generation,
                failures,
            },
            Effect::Spawn { generation },
        ),
        RetryReadiness::Ready => {
            let after = RetryDelay(retry_not_before.get() - at.get());
            Transition::new(state, Effect::ScheduleRetry { generation, after })
        }
        RetryReadiness::Occupied { retry_after } => {
            let occupied_until = at.after(retry_after);
            let retry_not_before = retry_not_before.max(occupied_until);
            let after = RetryDelay(retry_not_before.get().saturating_sub(at.get()).max(1));
            Transition::new(
                State::Backoff {
                    generation,
                    failures,
                    retry_not_before,
                },
                Effect::ScheduleRetry { generation, after },
            )
        }
        RetryReadiness::StopRequested
        | RetryReadiness::ConsentRevoked
        | RetryReadiness::Quitting
        | RetryReadiness::External
        | RetryReadiness::Unknown => unreachable!("handled above"),
    }
}

fn stop(state: State, reason: StoppedBy) -> Transition {
    let next = match state {
        // Quit 是更強、且已經對使用者回報的 intent。後來讀到 stop marker 或 child
        // exit 不能把它降回可 Start 的 Stopped。
        State::Quitting { .. } => state,
        State::External { generation } => State::ExternalStopping {
            generation: generation.next(),
            reason,
        },
        // ConsentRevoked 比一般 StopRequested 更強；看到後要升級，但之後重讀一般
        // marker 不得再降級。generation 不變，輪詢不製造假的新 intent。
        State::ExternalStopping {
            generation,
            reason: existing,
        } => State::ExternalStopping {
            generation,
            reason: if existing == StoppedBy::ConsentRevoked || reason == StoppedBy::ConsentRevoked
            {
                StoppedBy::ConsentRevoked
            } else {
                StoppedBy::Requested
            },
        },
        _ => State::Stopped {
            generation: state.generation().next(),
            reason,
        },
    };
    Transition::new(next, Effect::CancelRetry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> MonotonicMillis {
        MonotonicMillis::new(ms)
    }

    #[test]
    fn only_a_heartbeat_after_the_owned_child_cutoff_is_external() {
        let beat = |value| ObservedHeartbeatAt::new(value);
        let gone = |value| OwnedChildGoneAt::new(value);
        assert!(heartbeat_is_external(gone(None), beat(Some(10))));
        assert!(!heartbeat_is_external(gone(Some(10)), beat(Some(10))));
        assert!(!heartbeat_is_external(gone(Some(10)), beat(Some(9))));
        assert!(heartbeat_is_external(gone(Some(10)), beat(Some(11))));
        assert!(!heartbeat_is_external(gone(Some(10)), beat(None)));
    }

    fn started() -> (State, Generation) {
        let transition = reduce(State::initial(), Event::StartRequested);
        let Effect::Spawn { generation } = transition.effect else {
            panic!("explicit start must spawn")
        };
        (transition.state, generation)
    }

    fn retry(state: State, generation: Generation, now: u64) -> State {
        let transition = reduce(
            state,
            Event::RetryDue {
                generation,
                at: at(now),
                readiness: RetryReadiness::Ready,
            },
        );
        assert_eq!(transition.effect, Effect::Spawn { generation });
        transition.state
    }

    #[test]
    fn failures_wait_one_five_thirty_seconds_then_the_fourth_gives_up() {
        let (mut state, generation) = started();
        let expected = [
            RetryDelay::ONE_SECOND,
            RetryDelay::FIVE_SECONDS,
            RetryDelay::THIRTY_SECONDS,
        ];
        let mut now = 10_000;
        for (index, delay) in expected.into_iter().enumerate() {
            let transition = reduce(
                state,
                Event::SpawnFailed {
                    generation,
                    at: at(now),
                },
            );
            assert_eq!(
                transition.effect,
                Effect::ScheduleRetry {
                    generation,
                    after: delay
                }
            );
            assert_eq!(transition.state.failures().get(), index as u8 + 1);
            state = retry(transition.state, generation, now + delay.as_millis());
            now += delay.as_millis();
        }

        let transition = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(now),
            },
        );
        assert_eq!(
            transition.effect,
            Effect::GaveUp {
                generation,
                cause: GaveUpCause::FourFailures
            }
        );
        assert!(matches!(
            transition.state,
            State::GaveUp {
                failures: FailureCount(4),
                cause: GaveUpCause::FourFailures,
                ..
            }
        ));
    }

    #[test]
    fn booting_for_any_length_of_time_neither_times_out_nor_resets_failures() {
        let (state, generation) = started();
        let failed = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        );
        let starting = retry(failed.state, generation, 1_000);
        let running = reduce(starting, Event::SpawnSucceeded { generation }).state;
        let still_booting = reduce(
            running,
            Event::PhaseObserved {
                generation,
                phase: RecorderPhase::Booting,
                at: at(24 * 60 * 60 * 1_000),
            },
        );
        assert_eq!(still_booting.effect, Effect::None);
        assert_eq!(still_booting.state.failures().get(), 1);

        let failed_again = reduce(
            still_booting.state,
            Event::ChildExited {
                generation,
                exit: ChildExit::Failure,
                at: at(24 * 60 * 60 * 1_000 + 1),
            },
        );
        assert_eq!(failed_again.state.failures().get(), 2);
        assert_eq!(
            failed_again.effect,
            Effect::ScheduleRetry {
                generation,
                after: RetryDelay::FIVE_SECONDS
            }
        );
    }

    #[test]
    fn recording_must_reach_ten_minutes_before_it_resets_the_budget() {
        let (state, generation) = started();
        let failed = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        );
        let starting = retry(failed.state, generation, 1_000);
        let running = reduce(starting, Event::SpawnSucceeded { generation }).state;
        let recording = reduce(
            running,
            Event::PhaseObserved {
                generation,
                phase: RecorderPhase::Recording,
                at: at(2_000),
            },
        )
        .state;
        let almost = reduce(
            recording,
            Event::PhaseObserved {
                generation,
                phase: RecorderPhase::Recording,
                at: at(2_000 + HEALTHY_RECORDING_MS - 1),
            },
        )
        .state;
        assert_eq!(almost.failures().get(), 1);

        let healthy = reduce(
            almost,
            Event::PhaseObserved {
                generation,
                phase: RecorderPhase::Recording,
                at: at(2_000 + HEALTHY_RECORDING_MS),
            },
        )
        .state;
        assert_eq!(healthy.failures(), FailureCount::ZERO);
        let next_failure = reduce(
            healthy,
            Event::ChildExited {
                generation,
                exit: ChildExit::Failure,
                at: at(2_000 + HEALTHY_RECORDING_MS + 1),
            },
        );
        assert_eq!(next_failure.state.failures().get(), 1);
        assert_eq!(
            next_failure.effect,
            Effect::ScheduleRetry {
                generation,
                after: RetryDelay::ONE_SECOND
            }
        );
    }

    #[test]
    fn a_heartbeat_gap_breaks_the_continuous_recording_window() {
        let (state, generation) = started();
        let failed = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        );
        let starting = retry(failed.state, generation, 1_000);
        let running = reduce(starting, Event::SpawnSucceeded { generation }).state;
        let recording = reduce(
            running,
            Event::PhaseObserved {
                generation,
                phase: RecorderPhase::Recording,
                at: at(2_000),
            },
        )
        .state;
        let interrupted = reduce(
            recording,
            Event::PhaseObserved {
                generation,
                phase: RecorderPhase::NotRecording,
                at: at(2_000 + HEALTHY_RECORDING_MS - 1),
            },
        )
        .state;
        let resumed = reduce(
            interrupted,
            Event::PhaseObserved {
                generation,
                phase: RecorderPhase::Recording,
                at: at(2_000 + HEALTHY_RECORDING_MS),
            },
        )
        .state;

        assert_eq!(resumed.failures().get(), 1);
        assert!(matches!(
            resumed,
            State::Running {
                recording_since: Some(MonotonicMillis(value)),
                ..
            } if value == 2_000 + HEALTHY_RECORDING_MS
        ));
    }

    #[test]
    fn an_exit_does_not_invent_a_healthy_boundary_that_was_never_observed() {
        let (state, generation) = started();
        let failed = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        );
        let starting = retry(failed.state, generation, 1_000);
        let running = reduce(starting, Event::SpawnSucceeded { generation }).state;
        let recording = reduce(
            running,
            Event::PhaseObserved {
                generation,
                phase: RecorderPhase::Recording,
                at: at(10_000),
            },
        )
        .state;
        let transition = reduce(
            recording,
            Event::ChildExited {
                generation,
                exit: ChildExit::Failure,
                at: at(10_000 + HEALTHY_RECORDING_MS),
            },
        );
        assert_eq!(transition.state.failures().get(), 2);
        assert_eq!(
            transition.effect,
            Effect::ScheduleRetry {
                generation,
                after: RetryDelay::FIVE_SECONDS
            }
        );
    }

    #[test]
    fn exit_zero_never_retries() {
        let (state, generation) = started();
        let state = reduce(state, Event::SpawnSucceeded { generation }).state;
        let transition = reduce(
            state,
            Event::ChildExited {
                generation,
                exit: ChildExit::Success,
                at: at(1),
            },
        );
        assert_eq!(transition.effect, Effect::CancelRetry);
        assert!(matches!(
            transition.state,
            State::Stopped {
                reason: StoppedBy::SuccessfulExit,
                ..
            }
        ));
    }

    #[test]
    fn manual_stop_consent_revoke_and_quit_cancel_instead_of_retrying() {
        let (state, generation) = started();
        let backoff = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        )
        .state;

        let stopped = reduce(backoff, Event::StopRequested);
        assert_eq!(stopped.effect, Effect::CancelRetry);
        assert!(matches!(
            stopped.state,
            State::Stopped {
                reason: StoppedBy::Requested,
                ..
            }
        ));

        let revoked = reduce(backoff, Event::ConsentRevoked);
        assert_eq!(revoked.effect, Effect::CancelRetry);
        assert!(matches!(
            revoked.state,
            State::Stopped {
                reason: StoppedBy::ConsentRevoked,
                ..
            }
        ));

        let quitting = reduce(backoff, Event::Quit);
        assert_eq!(quitting.effect, Effect::CancelRetry);
        assert!(matches!(quitting.state, State::Quitting { .. }));

        for later in [Event::StopRequested, Event::ConsentRevoked] {
            let still_quitting = reduce(quitting.state, later);
            assert_eq!(still_quitting.effect, Effect::CancelRetry);
            assert_eq!(still_quitting.state, quitting.state);
            assert_eq!(
                reduce(still_quitting.state, Event::StartRequested).state,
                quitting.state,
                "a later stop observation must not reopen start after quit"
            );
        }
    }

    #[test]
    fn a_retry_from_an_old_generation_is_inert() {
        let (state, old_generation) = started();
        let backoff = reduce(
            state,
            Event::SpawnFailed {
                generation: old_generation,
                at: at(0),
            },
        )
        .state;
        let stopped = reduce(backoff, Event::StopRequested).state;
        let transition = reduce(
            stopped,
            Event::RetryDue {
                generation: old_generation,
                at: at(1_000),
                readiness: RetryReadiness::Ready,
            },
        );
        assert_eq!(transition.state, stopped);
        assert_eq!(transition.effect, Effect::None);
    }

    #[test]
    fn occupancy_is_a_barrier_not_an_additional_failure() {
        let (state, generation) = started();
        let backoff = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        )
        .state;
        let occupied = reduce(
            backoff,
            Event::RetryDue {
                generation,
                at: at(1_000),
                readiness: RetryReadiness::Occupied {
                    retry_after: RetryDelay::FIVE_SECONDS,
                },
            },
        );
        assert_eq!(occupied.state.failures().get(), 1);
        assert_eq!(
            occupied.effect,
            Effect::ScheduleRetry {
                generation,
                after: RetryDelay::FIVE_SECONDS
            }
        );

        let starting = retry(occupied.state, generation, 6_000);
        let failed = reduce(
            starting,
            Event::SpawnFailed {
                generation,
                at: at(6_001),
            },
        );
        assert_eq!(failed.state.failures().get(), 2);
        assert_eq!(
            failed.effect,
            Effect::ScheduleRetry {
                generation,
                after: RetryDelay::FIVE_SECONDS
            }
        );
    }

    #[test]
    fn an_unknown_retry_probe_fails_closed_without_spawning() {
        let (state, generation) = started();
        let backoff = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        )
        .state;
        let transition = reduce(
            backoff,
            Event::RetryDue {
                generation,
                at: at(1_000),
                readiness: RetryReadiness::Unknown,
            },
        );
        assert_eq!(
            transition.effect,
            Effect::GaveUp {
                generation,
                cause: GaveUpCause::ProbeUnknown
            }
        );
        assert!(matches!(
            transition.state,
            State::GaveUp {
                cause: GaveUpCause::ProbeUnknown,
                failures: FailureCount(1),
                ..
            }
        ));
    }

    #[test]
    fn a_child_probe_error_keeps_the_owned_child_and_recovers_on_later_evidence() {
        let (starting, generation) = started();
        for state in [
            starting,
            reduce(starting, Event::SpawnSucceeded { generation }).state,
        ] {
            let transition = reduce(state, Event::ChildProbeFailed { generation });
            assert_eq!(transition.effect, Effect::CancelRetry);
            assert!(matches!(transition.state, State::ChildUncertain { .. }));

            let recovered = reduce(transition.state, Event::ChildProbeSucceeded { generation });
            assert!(matches!(recovered.state, State::Running { .. }));
            assert_eq!(recovered.effect, Effect::None);

            let exited = reduce(
                transition.state,
                Event::ChildExited {
                    generation,
                    exit: ChildExit::Failure,
                    at: at(10),
                },
            );
            assert!(matches!(exited.state, State::Backoff { .. }));
            assert!(matches!(exited.effect, Effect::ScheduleRetry { .. }));
        }
    }

    #[test]
    fn an_external_recorder_cancels_retry_and_is_never_adopted() {
        let (state, generation) = started();
        let backoff = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        )
        .state;
        let external = reduce(
            backoff,
            Event::RetryDue {
                generation,
                at: at(1_000),
                readiness: RetryReadiness::External,
            },
        );
        assert_eq!(external.effect, Effect::CancelRetry);
        assert!(matches!(external.state, State::External { .. }));
        assert_eq!(
            reduce(external.state, Event::StartRequested).state,
            external.state,
            "desktop must not adopt or replace an external recorder"
        );

        let gone = reduce(external.state, Event::ExternalGone);
        assert!(matches!(
            gone.state,
            State::Stopped {
                reason: StoppedBy::ExternalExit,
                ..
            }
        ));
    }

    #[test]
    fn stopping_an_external_recorder_waits_for_proof_that_it_is_gone() {
        let external = reduce(State::initial(), Event::ExternalObserved).state;
        let stopping = reduce(external, Event::StopRequested);
        assert_eq!(stopping.effect, Effect::CancelRetry);
        assert!(matches!(
            stopping.state,
            State::ExternalStopping {
                reason: StoppedBy::Requested,
                ..
            }
        ));
        assert_eq!(
            reduce(stopping.state, Event::ExternalObserved).state,
            stopping.state,
            "a still-fresh heartbeat must remain an external stop in flight"
        );
        assert_eq!(
            reduce(stopping.state, Event::StopRequested).state,
            stopping.state,
            "polling the durable marker must not manufacture generations"
        );
        let revoked = reduce(stopping.state, Event::ConsentRevoked).state;
        assert!(matches!(
            revoked,
            State::ExternalStopping {
                reason: StoppedBy::ConsentRevoked,
                ..
            }
        ));
        assert_eq!(
            reduce(revoked, Event::StopRequested).state,
            revoked,
            "a later generic stop must not downgrade consent revocation"
        );
        assert_eq!(
            reduce(stopping.state, Event::StartRequested).state,
            stopping.state,
            "start stays closed until external vacancy is proven"
        );

        let gone = reduce(stopping.state, Event::ExternalGone);
        assert!(matches!(
            gone.state,
            State::Stopped {
                reason: StoppedBy::Requested,
                ..
            }
        ));
    }

    #[test]
    fn an_external_recorder_can_appear_after_the_watchdog_gave_up() {
        let gave_up = State::GaveUp {
            generation: Generation::INITIAL,
            failures: FailureCount(4),
            cause: GaveUpCause::FourFailures,
        };
        let transition = reduce(gave_up, Event::ExternalObserved);
        assert_eq!(transition.effect, Effect::CancelRetry);
        assert!(matches!(transition.state, State::External { .. }));
    }

    #[test]
    fn retry_rechecks_stop_consent_and_quit_before_it_spawns() {
        let (state, generation) = started();
        let backoff = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(0),
            },
        )
        .state;
        for (readiness, expected) in [
            (RetryReadiness::StopRequested, StoppedBy::Requested),
            (RetryReadiness::ConsentRevoked, StoppedBy::ConsentRevoked),
        ] {
            let transition = reduce(
                backoff,
                Event::RetryDue {
                    generation,
                    at: at(1_000),
                    readiness,
                },
            );
            assert_eq!(transition.effect, Effect::CancelRetry);
            assert!(matches!(
                transition.state,
                State::Stopped { reason, .. } if reason == expected
            ));
        }
        let quitting = reduce(
            backoff,
            Event::RetryDue {
                generation,
                at: at(1_000),
                readiness: RetryReadiness::Quitting,
            },
        );
        assert_eq!(quitting.effect, Effect::CancelRetry);
        assert!(matches!(quitting.state, State::Quitting { .. }));
    }

    #[test]
    fn an_early_timer_is_rescheduled_for_only_the_remaining_delay() {
        let (state, generation) = started();
        let backoff = reduce(
            state,
            Event::SpawnFailed {
                generation,
                at: at(10_000),
            },
        )
        .state;
        let transition = reduce(
            backoff,
            Event::RetryDue {
                generation,
                at: at(10_250),
                readiness: RetryReadiness::Ready,
            },
        );
        assert_eq!(
            transition.effect,
            Effect::ScheduleRetry {
                generation,
                after: RetryDelay(750)
            }
        );
        assert_eq!(transition.state.failures().get(), 1);
    }

    #[test]
    fn explicit_start_after_give_up_has_a_new_generation_and_clean_budget() {
        let old_generation = Generation(9);
        let gave_up = State::GaveUp {
            generation: old_generation,
            failures: FailureCount(4),
            cause: GaveUpCause::FourFailures,
        };
        let transition = reduce(gave_up, Event::StartRequested);
        let new_generation = transition.state.generation();
        assert_ne!(new_generation, old_generation);
        assert_eq!(transition.state.failures(), FailureCount::ZERO);
        assert_eq!(
            transition.effect,
            Effect::Spawn {
                generation: new_generation
            }
        );
    }
}
