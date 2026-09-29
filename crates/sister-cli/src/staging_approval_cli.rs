//! Feature-only executable staging entry. Its sole executor changes memory and writes
//! a local receipt; it cannot send, publish, pay, delete, or access an account.

use anyhow::{Context, Result, bail};
use sister_hands::NeverInherited;
use sister_hands::staging_approval::{
    IrreversibleIntent, LiveApprover, LiveDecision, StagingFixture,
};
use std::{
    cell::Cell,
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

struct TerminalApprover<'a> {
    clock: &'a Cell<i64>,
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
                self.clock
                    .set(self.clock.get().saturating_add(self.advance_ms));
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
        if args.len() != 6 {
            bail!(
                "usage: --staging-irreversible <data-dir> <class> <staging://target> <details> <approval-clock-advance-ms>"
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
        let intent = IrreversibleIntent {
            class: class(args[2].to_str().context("class must be text")?)?,
            target: target.to_owned(),
            details: args[4].to_str().context("details must be text")?.to_owned(),
        };
        let clock = Cell::new(sister_core::now_ms());
        let mut approver = TerminalApprover {
            clock: &clock,
            advance_ms,
        };
        let mut fixture = StagingFixture::default();
        let outcome = fixture.dispatch(
            intent,
            &mut approver,
            || clock.get(),
            || {
                sister_hands::kill_switch::is_pulled(&dir)
                    || sister_hands::master_stop::is_stopped(&dir)
            },
        );
        std::fs::create_dir_all(&dir)?;
        fixture.write_jsonl(&receipt_path(&dir))?;
        println!("staging outcome: {outcome:?}");
        Ok(())
    })())
}
