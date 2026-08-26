use crate::heartbeat::Presence;
use crate::moments::SpeakCategory;

/// 前景視窗是不是全螢幕。**三種，不是兩種。**
///
/// SPEC §8.3.3 要「專注模式（全螢幕 app）自動靜音」。但這一版沒有跨平台的
/// 前景視窗幾何訊號，所以第三種是必要的：把「沒量到」寫成 `false`，等於
/// 宣布「我看過了，不是全螢幕」——一句沒有人查證過的斷言。這個 repo 已經
/// 為這種寫法付過 67 次帳，最近一次就在上一輪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMode {
    Fullscreen,
    Windowed,
    /// 這一版沒有這個訊號。**不是** `Windowed`。
    Unmeasured,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub category: SpeakCategory,
    pub text: String,
    pub evidence: Vec<String>,
    pub impact: f64,
    pub confidence: f64,
    pub timeliness: f64,
    pub evidence_strength: f64,
    /// 只有承諾到期候選有。讓行動接線不必從顯示文字猜是哪張卡。
    pub commitment_id: Option<i64>,
}

impl Candidate {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        category: SpeakCategory,
        text: impl Into<String>,
        evidence: Vec<String>,
        impact: f64,
        confidence: f64,
        timeliness: f64,
        evidence_strength: f64,
    ) -> anyhow::Result<Self> {
        for (name, value) in [
            ("impact", impact),
            ("confidence", confidence),
            ("timeliness", timeliness),
            ("evidence_strength", evidence_strength),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                anyhow::bail!("{name} 必須介於 0.0 到 1.0，實際是 {value}");
            }
        }
        Ok(Self {
            category,
            text: text.into(),
            evidence,
            impact,
            confidence,
            timeliness,
            evidence_strength,
            commitment_id: None,
        })
    }

    pub fn score(&self) -> f64 {
        self.impact * self.confidence * self.timeliness * self.evidence_strength
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Glimmer,
    OneLine,
    Card,
}

impl Form {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Glimmer => "glimmer",
            Self::OneLine => "one_line",
            Self::Card => "card",
        }
    }
    pub fn cost(self) -> u32 {
        match self {
            Self::Glimmer => 0,
            Self::OneLine => 1,
            Self::Card => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Speak { form: Form, cost: u32 },
    Hold(HoldReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HoldReason {
    /// 這一句的點數放不進今天剩下的額度。
    ///
    /// `needed` 一定要在：`spent=4 / limit=5` 配一張要 2 點的卡片會走到這裡，
    /// 而「用了 4 點、上限 5 點」讀起來像額度**用完了**——沒有。放不下的是
    /// 這一句的大小，不是額度歸零。少了這一格，同一句話要回答兩個問題。
    DailyBudgetExhausted {
        needed: u32,
        spent: u32,
        limit: u32,
    },
    CategoryCoolingDown {
        remaining_minutes: u32,
    },
    QuietHours {
        ends_at: String,
    },
    FullscreenFocus,
    ColdStartCategory {
        category: SpeakCategory,
        days_remaining: u32,
    },
    BelowScore {
        score: f64,
        threshold: f64,
    },
    MissingEvidence,
    FirstUtteranceCategory {
        category: SpeakCategory,
    },
    /// 確定沒在錄。`says` 是 [`Presence`] 六種裡的**哪一種**——「從來沒跑過」
    /// 和「當掉了」對使用者是完全不同的下一步（一個去按開始，一個去看她是不是
    /// 死了）。
    NotRecording {
        says: &'static str,
    },
    /// [`Presence::Unreadable`]：**問不出**現在有沒有在錄。
    ///
    /// 和 `NotRecording` 分開，因為那一句是斷言、這一句是說不準，而說不準的
    /// 時候她本來就不該開口。壓成同一個 variant 的話，一個 I/O 錯誤會讓她
    /// 對使用者宣布「你沒在錄」。
    PresenceUnreadable,
    /// 這一輪它自己過得了關，但同一輪有一句更該講的，而她一次只講一句。
    ///
    /// **這一格非有不可。** 少了它，落選的候選只有兩條路：記成 `spoke`
    /// （於是訓練語料裡有一句她從來沒講過的話，而且照樣扣了點數），或者
    /// 靜靜丟掉（於是那一輪的紀錄少了一半，而 SPEC §8.3.5 要的正是那一半）。
    /// 兩條都是假話，差別只在假在哪裡。
    OutrankedThisRound {
        by_text: String,
    },
}

impl HoldReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DailyBudgetExhausted { .. } => "daily_budget_exhausted",
            Self::CategoryCoolingDown { .. } => "category_cooling_down",
            Self::QuietHours { .. } => "quiet_hours",
            Self::FullscreenFocus => "fullscreen_focus",
            Self::ColdStartCategory { .. } => "cold_start_category",
            Self::BelowScore { .. } => "below_score",
            Self::MissingEvidence => "missing_evidence",
            Self::FirstUtteranceCategory { .. } => "first_utterance_category",
            Self::NotRecording { .. } => "not_recording",
            Self::PresenceUnreadable => "presence_unreadable",
            Self::OutrankedThisRound { .. } => "outranked_this_round",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::DailyBudgetExhausted {
                needed,
                spent,
                limit,
            } => format!("這一句要 {needed} 點，今天已用了 {spent} 點、上限 {limit} 點，放不下。"),
            Self::CategoryCoolingDown { remaining_minutes } => {
                format!("同一類仍在冷卻，還要 {remaining_minutes} 分鐘。")
            }
            Self::QuietHours { ends_at } => format!("現在是安靜時段，{ends_at} 才結束。"),
            Self::FullscreenFocus => "全螢幕專注模式中，先不開口。".into(),
            Self::ColdStartCategory {
                category,
                days_remaining,
            } => format!(
                "安裝後前兩週尚未解鎖 {} 類，還有 {days_remaining} 天。",
                category.as_str()
            ),
            Self::BelowScore { score, threshold } => {
                format!("分數 {score:.3}，未達門檻 {threshold:.3}。")
            }
            Self::MissingEvidence => "沒有證據 ref，不能開口。".into(),
            Self::FirstUtteranceCategory { category } => format!(
                "有史以來第一句只能是 a/b 類，{} 類不能當第一句。",
                category.as_str()
            ),
            Self::NotRecording { says } => {
                format!("現在沒在錄（{says}），不能假裝知道此刻發生什麼事。")
            }
            Self::PresenceUnreadable => {
                "讀不到心跳，問不出她現在有沒有在錄。說不準的時候不開口。".into()
            }
            Self::OutrankedThisRound { by_text } => {
                format!("這一輪有更該講的一句：「{by_text}」。她一次只講一句。")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct GateInput {
    pub candidate: Candidate,
    /// 整個 [`Presence`]，**不是**壓過的 `bool`。壓成 `bool` 的那一版把
    /// 「問不出來」和「確定沒在錄」讀成同一件事，於是一次 I/O 錯誤會讓她對
    /// 使用者宣布「你沒在錄」。
    pub presence: Presence,
    pub quiet_hours_end: Option<String>,
    pub focus_mode: FocusMode,
    pub days_since_first_recording: u32,
    pub cold_start_days: u32,
    pub has_ever_spoken: bool,
    pub cooldown_remaining_minutes: Option<u32>,
    pub points_spent_today: u32,
    pub daily_budget_points: u32,
    pub min_score: f64,
}

impl GateInput {
    #[cfg(test)]
    fn for_test(candidate: Candidate) -> Self {
        Self {
            candidate,
            presence: Presence::Live(crate::heartbeat::Phase::Recording),
            quiet_hours_end: None,
            focus_mode: FocusMode::Windowed,
            days_since_first_recording: 15,
            cold_start_days: 14,
            has_ever_spoken: true,
            cooldown_remaining_minutes: None,
            points_spent_today: 0,
            daily_budget_points: 5,
            min_score: 0.25,
        }
    }
}

fn is_precision_category(category: SpeakCategory) -> bool {
    matches!(
        category,
        SpeakCategory::CommitmentDue | SpeakCategory::UnattendedNotification
    )
}

pub fn decide(input: &GateInput) -> Verdict {
    let c = &input.candidate;
    // 六種 presence，六個不一樣的下一步。沒有 `_`：`heartbeat` 再多一種狀態，
    // 這裡就要編譯錯誤，而不是安靜地被歸進「沒在錄」。
    match input.presence {
        Presence::Live(_) => {}
        Presence::NeverStarted => {
            return Verdict::Hold(HoldReason::NotRecording {
                says: "這個資料目錄從來沒有人跑過錄製",
            });
        }
        Presence::Stopped { .. } => {
            return Verdict::Hold(HoldReason::NotRecording {
                says: "她自己講了收工",
            });
        }
        Presence::Thinking { .. } => {
            return Verdict::Hold(HoldReason::NotRecording {
                says: "錄製迴圈停了，她還在把最後一段想完",
            });
        }
        Presence::Stalled { .. } => {
            return Verdict::Hold(HoldReason::NotRecording {
                says: "心跳停在那裡過期了——當掉、被 kill、或這一拍太慢",
            });
        }
        Presence::Unreadable => return Verdict::Hold(HoldReason::PresenceUnreadable),
    }
    if c.evidence.is_empty() {
        return Verdict::Hold(HoldReason::MissingEvidence);
    }
    // 第一句話規則看的是歷史 utterance，不看安裝天數。
    if !input.has_ever_spoken && !is_precision_category(c.category) {
        return Verdict::Hold(HoldReason::FirstUtteranceCategory {
            category: c.category,
        });
    }
    // 冷啟動規則只看第一次錄製後經過多久，即使已經開過口仍然有效。
    if input.days_since_first_recording < input.cold_start_days
        && !is_precision_category(c.category)
    {
        return Verdict::Hold(HoldReason::ColdStartCategory {
            category: c.category,
            days_remaining: input.cold_start_days - input.days_since_first_recording,
        });
    }
    if let Some(ends_at) = &input.quiet_hours_end {
        return Verdict::Hold(HoldReason::QuietHours {
            ends_at: ends_at.clone(),
        });
    }
    match input.focus_mode {
        FocusMode::Fullscreen => return Verdict::Hold(HoldReason::FullscreenFocus),
        // 量不到就放行。**擋下來才是更糟的選擇**：這一版沒有任何平台送得出
        // 這個訊號，所以「說不準就閉嘴」會把整個守門員永遠關掉，而那不是
        // SPEC §8.3 要的靜音，是功能沒上線。誠實的地方在 `--dry-run` 會印出
        // 「這一版量不到」，不在這裡假裝擋過。
        FocusMode::Windowed | FocusMode::Unmeasured => {}
    }
    if let Some(remaining_minutes) = input.cooldown_remaining_minutes {
        return Verdict::Hold(HoldReason::CategoryCoolingDown { remaining_minutes });
    }
    let score = c.score();
    if score < input.min_score {
        return Verdict::Hold(HoldReason::BelowScore {
            score,
            threshold: input.min_score,
        });
    }
    // 實作選擇，尚未量過：0.75 以上用卡片，0.40 以上用一行，其餘只亮微光。
    let form = if score >= 0.75 {
        Form::Card
    } else if score >= 0.40 {
        Form::OneLine
    } else {
        Form::Glimmer
    };
    let cost = form.cost();
    if input.points_spent_today.saturating_add(cost) > input.daily_budget_points {
        return Verdict::Hold(HoldReason::DailyBudgetExhausted {
            needed: cost,
            spent: input.points_spent_today,
            limit: input.daily_budget_points,
        });
    }
    Verdict::Speak { form, cost }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    Close,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitmentReaction {
    MarkDead { kill_note: String },
    SnoozeAndLowerWeight,
    None,
}

pub fn reaction_effect(category: SpeakCategory, reaction: Reaction) -> CommitmentReaction {
    match (category, reaction) {
        (SpeakCategory::CommitmentDue, Reaction::Close) => CommitmentReaction::MarkDead {
            kill_note: "使用者從主動提醒按下結案".into(),
        },
        (SpeakCategory::CommitmentDue, Reaction::Other) => CommitmentReaction::SnoozeAndLowerWeight,
        (
            SpeakCategory::UnattendedNotification
            | SpeakCategory::Stuck
            | SpeakCategory::SessionEnd
            | SpeakCategory::Leaving,
            Reaction::Close | Reaction::Other,
        ) => CommitmentReaction::None,
    }
}

pub fn react(
    db: &mut crate::db::Db,
    utterance_id: i64,
    reaction: Reaction,
    now: crate::model::Millis,
) -> anyhow::Result<CommitmentReaction> {
    let row = db
        .utterance_by_id(utterance_id)?
        .ok_or_else(|| anyhow::anyhow!("找不到 utterance #{utterance_id}"))?;
    let label = match reaction {
        Reaction::Close => "close",
        Reaction::Other => "other",
    };
    if db.record_reaction(utterance_id, label, now)? == 0 {
        anyhow::bail!("utterance #{utterance_id} 已刪除，不能記反應");
    }
    let effect = reaction_effect(row.category, reaction);
    if row.category == SpeakCategory::CommitmentDue {
        let commitment_id = row
            .evidence
            .iter()
            .find_map(|r| r.strip_prefix("commitment:")?.parse::<i64>().ok())
            .ok_or_else(|| anyhow::anyhow!("a 類 utterance #{utterance_id} 沒有 commitment ref"))?;
        match &effect {
            CommitmentReaction::MarkDead { kill_note } => {
                crate::reviewer::kill_commitment(db, commitment_id, kill_note, now)?;
            }
            CommitmentReaction::SnoozeAndLowerWeight => {
                crate::reviewer::snooze_commitment(db, commitment_id, now)?;
            }
            CommitmentReaction::None => {}
        }
    }
    Ok(effect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_rejects_an_out_of_range_factor_instead_of_clamping_it() {
        let error = Candidate::new(
            SpeakCategory::Stuck,
            "你似乎卡在同一個錯誤。",
            vec!["segment:42".into()],
            1.01,
            0.8,
            0.9,
            1.0,
        )
        .unwrap_err();
        assert!(error.to_string().contains("impact"));
        assert!(error.to_string().contains("1.01"));
        assert!(!error.to_string().contains("clamp"));
    }

    #[test]
    fn first_utterance_and_two_week_cold_start_diverge_on_day_fifteen() {
        let c = Candidate::new(
            SpeakCategory::Stuck,
            "你似乎卡在同一個錯誤。",
            vec!["segment:42".into()],
            1.0,
            1.0,
            1.0,
            1.0,
        )
        .unwrap();
        let mut input = GateInput::for_test(c);
        input.days_since_first_recording = 15;
        input.has_ever_spoken = false;
        assert!(matches!(
            decide(&input),
            Verdict::Hold(HoldReason::FirstUtteranceCategory { .. })
        ));
        input.has_ever_spoken = true;
        assert!(matches!(decide(&input), Verdict::Speak { .. }));
    }

    fn stuck_candidate(strength: f64) -> Candidate {
        Candidate::new(
            SpeakCategory::Stuck,
            "你似乎卡在同一個錯誤。",
            vec!["segment:42".into()],
            1.0,
            1.0,
            1.0,
            strength,
        )
        .unwrap()
    }

    /// 「說不準」不能被說成「你沒在錄」。
    ///
    /// 六種 presence 走到兩個不同的 HoldReason，而 `Unreadable` 那一種是唯一
    /// 不該出現斷言的。壓成 `bool` 的版本會讓一次 I/O 錯誤變成一句斷言。
    #[test]
    fn unreadable_presence_is_not_the_same_answer_as_not_recording() {
        use crate::heartbeat::Phase;
        let mut input = GateInput::for_test(stuck_candidate(1.0));

        input.presence = Presence::Unreadable;
        let unreadable = match decide(&input) {
            Verdict::Hold(r) => r,
            other => panic!("讀不到心跳還開口了：{other:?}"),
        };
        assert_eq!(unreadable.code(), "presence_unreadable");
        assert!(unreadable.message().contains("問不出"));
        assert!(
            !unreadable.message().contains("沒在錄"),
            "問不出來的時候不准斷言她沒在錄：{}",
            unreadable.message()
        );

        input.presence = Presence::Stopped { at: Some(1) };
        let stopped = match decide(&input) {
            Verdict::Hold(r) => r,
            other => panic!("收工了還開口：{other:?}"),
        };
        assert_eq!(stopped.code(), "not_recording");
        assert!(stopped.message().contains("收工"));

        // 而 Live 要走得到底——只留上面那些的話，把 decide 改成永遠 Hold
        // 也會全綠。
        input.presence = Presence::Live(Phase::Recording);
        assert!(matches!(decide(&input), Verdict::Speak { .. }));
    }

    /// 量不到全螢幕，不等於量到了「不是全螢幕」。
    #[test]
    fn unmeasured_focus_is_a_third_state_not_windowed() {
        let mut input = GateInput::for_test(stuck_candidate(1.0));
        input.focus_mode = FocusMode::Fullscreen;
        assert!(matches!(
            decide(&input),
            Verdict::Hold(HoldReason::FullscreenFocus)
        ));
        // 這一版量不到，所以放行——但它是自己一格，不是 `Windowed` 的別名。
        input.focus_mode = FocusMode::Unmeasured;
        assert!(matches!(decide(&input), Verdict::Speak { .. }));
        assert_ne!(FocusMode::Unmeasured, FocusMode::Windowed);
    }

    /// 剩 1 點、這一句要 2 點——額度沒用完，是這一句放不下。
    #[test]
    fn a_card_that_does_not_fit_says_so_without_claiming_the_budget_is_empty() {
        let mut input = GateInput::for_test(stuck_candidate(1.0));
        input.points_spent_today = 4;
        input.daily_budget_points = 5;
        let reason = match decide(&input) {
            Verdict::Hold(r) => r,
            other => panic!("4+2 > 5 還開口了：{other:?}"),
        };
        let said = reason.message();
        assert!(said.contains("這一句要 2 點"), "{said}");
        assert!(said.contains("用了 4 點"), "{said}");
        assert!(said.contains("上限 5 點"), "{said}");
        assert!(!said.contains("用完"), "還剩 1 點，不准講成用完了：{said}");
        // 同樣剩 1 點，一句只要 1 點的話講得出來。
        input.candidate = stuck_candidate(0.5);
        assert!(matches!(
            decide(&input),
            Verdict::Speak {
                form: Form::OneLine,
                cost: 1
            }
        ));
    }

    #[test]
    fn hold_messages_name_both_sides_of_the_reason() {
        let budget = HoldReason::DailyBudgetExhausted {
            needed: 2,
            spent: 5,
            limit: 5,
        }
        .message();
        assert!(budget.contains("用了 5 點"));
        assert!(budget.contains("上限 5 點"));
        assert!(!budget.contains("還要"));
        let cooldown = HoldReason::CategoryCoolingDown {
            remaining_minutes: 37,
        }
        .message();
        assert!(cooldown.contains("還要 37 分鐘"));
        assert!(!cooldown.contains("上限"));
    }

    #[test]
    fn default_quiet_hours_cross_midnight_and_empty_means_disabled() {
        let cfg = crate::config::GatekeeperConfig::default();
        assert_eq!(cfg.quiet_end_at(23 * 60).unwrap().as_deref(), Some("08:00"));
        assert_eq!(
            cfg.quiet_end_at(7 * 60 + 59).unwrap().as_deref(),
            Some("08:00")
        );
        assert_eq!(cfg.quiet_end_at(12 * 60).unwrap(), None);
        let mut off = cfg;
        off.quiet_hours.clear();
        assert_eq!(off.quiet_end_at(23 * 60).unwrap(), None);
    }
}
