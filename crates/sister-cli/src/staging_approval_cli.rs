//! Feature-only executable staging entry. Its sole executor changes memory and writes
//! a local receipt; it cannot send, publish, pay, delete, or access an account.

use anyhow::{Context, Result, bail};
use sister_hands::NeverInherited;
use sister_hands::staging_approval::{
    ApprovalInputOrigin, IrreversibleIntent, LiveApprover, LiveDecision, StagingFixture,
};
use std::{
    cell::Cell,
    ffi::OsString,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    time::Instant,
};

struct TerminalApprover<'a> {
    clock_offset_ms: &'a Cell<i64>,
    advance_ms: i64,
}

impl LiveApprover for TerminalApprover<'_> {
    fn present_and_ask(&mut self, intent: &IrreversibleIntent) -> LiveDecision {
        println!(
            "受控 staging 動作：{}；目標：{}；內容：{}",
            intent.class.name(),
            intent.target,
            intent.details
        );
        print!("當場核准？好／不要／停：");
        if std::io::stdout().flush().is_err() {
            return LiveDecision::Stop;
        }
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            return LiveDecision::Stop;
        }
        match answer.trim() {
            "好" => {
                self.clock_offset_ms
                    .set(self.clock_offset_ms.get().saturating_add(self.advance_ms));
                LiveDecision::Approve
            }
            "不要" => LiveDecision::Decline,
            _ => LiveDecision::Stop,
        }
    }
}

fn class(raw: &str) -> Result<NeverInherited> {
    Ok(match raw {
        "submit" => NeverInherited::Submit,
        "publish" => NeverInherited::Publish,
        "pay" => NeverInherited::Pay,
        "delete" => NeverInherited::Delete,
        "open-terminal" => NeverInherited::OpenTerminal,
        _ => bail!("unknown staging class"),
    })
}

fn receipt_path(dir: &Path) -> PathBuf {
    dir.join("staging-approval.jsonl")
}

pub fn run_early(args: &[OsString]) -> Option<Result<()>> {
    let command = args.first()?.to_str()?;
    if command == "--staging-replay" {
        return Some((|| {
            if args.len() != 2 {
                bail!("usage: --staging-replay <data-dir>");
            }
            let dir = PathBuf::from(&args[1]);
            let events = StagingFixture::replay_jsonl(&receipt_path(&dir))?;
            for event in events {
                println!("{}", serde_json::to_string(&event)?);
            }
            Ok(())
        })());
    }
    if command != "--staging-irreversible" {
        return None;
    }
    Some((|| {
        if !(args.len() == 6 || args.len() == 7) {
            bail!(
                "usage: --staging-irreversible <data-dir> <class> <staging://target> <details> <test-clock-advance-ms> [--scripted-test-approval]"
            );
        }
        let scripted = args.len() == 7 && args[6] == "--scripted-test-approval";
        if args.len() == 7 && !scripted {
            bail!("unknown staging argument");
        }
        if !scripted && !std::io::stdin().is_terminal() {
            bail!(
                "live staging approval requires a terminal; scripted fixtures must use --scripted-test-approval"
            );
        }
        let dir = PathBuf::from(&args[1]);
        let target = args[3].to_str().context("staging target must be text")?;
        if !target.starts_with("staging://") || target.len() <= "staging://".len() {
            bail!("staging target must begin with staging:// and name a local fixture");
        }
        let advance_ms: i64 = args[5]
            .to_str()
            .context("clock advance must be text")?
            .parse()?;
        if advance_ms < 0 {
            bail!("clock advance must be nonnegative");
        }
        if !scripted && advance_ms != 0 {
            bail!("a real terminal approval cannot use a test clock advance");
        }
        let intent = IrreversibleIntent {
            class: class(args[2].to_str().context("class must be text")?)?,
            target: target.to_owned(),
            details: args[4].to_str().context("details must be text")?.to_owned(),
        };
        let clock_offset_ms = Cell::new(0_i64);
        let started = Instant::now();
        let mut approver = TerminalApprover {
            clock_offset_ms: &clock_offset_ms,
            advance_ms,
        };
        let mut fixture = StagingFixture::with_input_origin(if scripted {
            ApprovalInputOrigin::ScriptedFixture
        } else {
            ApprovalInputOrigin::HumanTerminal
        });
        let outcome = fixture.dispatch(
            intent,
            &mut approver,
            || {
                i64::try_from(started.elapsed().as_millis())
                    .unwrap_or(i64::MAX)
                    .saturating_add(clock_offset_ms.get())
            },
            || {
                sister_hands::kill_switch::is_pulled(&dir)
                    || sister_hands::master_stop::is_stopped(&dir)
            },
        );
        std::fs::create_dir_all(&dir)?;
        fixture.append_jsonl(&receipt_path(&dir))?;
        println!(
            "staging input: {}",
            if scripted {
                "scripted_fixture"
            } else {
                "human_terminal"
            }
        );
        println!("staging outcome: {outcome:?}");
        Ok(())
    })())
}
