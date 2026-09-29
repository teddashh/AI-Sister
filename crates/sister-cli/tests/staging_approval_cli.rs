#![cfg(feature = "staging-approval")]

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
};

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sister-staging-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn invoke(
    dir: &Path,
    class: &str,
    answer: &str,
    advance_ms: i64,
) -> (String, Vec<serde_json::Value>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sister"))
        .args([
            "--staging-irreversible",
            dir.to_str().unwrap(),
            class,
            "staging://controlled-fixture/one",
            "synthetic local action",
            &advance_ms.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answer.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let log = std::fs::read_to_string(dir.join("staging-approval.jsonl")).unwrap();
    let events = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (stdout, events)
}

fn one_outcome(class: &str, answer: &str, advance_ms: i64, expected: &str, executed: usize) {
    let dir = TempDir::new();
    let (stdout, events) = invoke(dir.path(), class, answer, advance_ms);
    assert!(stdout.contains(expected), "{stdout}");
    assert_eq!(
        events
            .iter()
            .filter(|event: &&serde_json::Value| event["event"] == "executed")
            .count(),
        executed
    );
    let before = std::fs::read(dir.path().join("staging-approval.jsonl")).unwrap();
    let replay = Command::new(env!("CARGO_BIN_EXE_sister"))
        .args(["--staging-replay", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(replay.status.success());
    assert_eq!(
        std::fs::read(dir.path().join("staging-approval.jsonl")).unwrap(),
        before
    );
    let printed = String::from_utf8(replay.stdout).unwrap();
    assert_eq!(printed.lines().count(), events.len());
}

#[test]
fn executable_staging_dispatch_requires_live_approval_for_all_five_classes() {
    for class in ["submit", "publish", "pay", "delete", "open-terminal"] {
        one_outcome(class, "不要\n", 0, "Declined", 0);
        one_outcome(class, "停\n", 0, "Stopped", 0);
        one_outcome(class, "好\n", 30_001, "Expired", 0);
        one_outcome(class, "好\n", 1, "Executed", 1);
    }
}

#[test]
fn executable_staging_dispatch_obeys_stop_and_rejects_nonfixture_target() {
    let dir = TempDir::new();
    sister_hands::kill_switch::pull(dir.path(), 1000).unwrap();
    let (stdout, events) = invoke(dir.path(), "pay", "好\n", 0);
    assert!(stdout.contains("Stopped"), "{stdout}");
    assert!(
        !events
            .iter()
            .any(|event: &serde_json::Value| event["event"] == "executed")
    );
    let bad = Command::new(env!("CARGO_BIN_EXE_sister"))
        .args([
            "--staging-irreversible",
            dir.path().to_str().unwrap(),
            "pay",
            "https://real.example/pay",
            "should refuse",
            "0",
        ])
        .output()
        .unwrap();
    assert!(!bad.status.success());
}
