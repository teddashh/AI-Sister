//! Feature-gated parent half of the macOS app-tree probe.
//!
//! LaunchServices starts this executable from `AI-Sister.app`; it then spawns the exact sibling
//! `Contents/MacOS/sister`. The workflow inspects both live processes before allowing one capture.

use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const ARGUMENT: &str = "--macos-ci-app-tree-probe";
const CHILD_TIMEOUT: Duration = Duration::from_secs(60);
const REAP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize)]
struct ProcessRecord {
    schema: u8,
    role: &'static str,
    pid: u32,
    executable: String,
}

#[derive(Serialize)]
struct SpawnRecord {
    schema: u8,
    app_pid: u32,
    child_pid: u32,
}

#[derive(Serialize)]
struct ExitRecord {
    schema: u8,
    success: bool,
    code: Option<i32>,
}

/// Spawn 成功之後，每一條提早離開的路都必須收掉 exact child。
///
/// `std::process::Child` 自己 Drop 不會 kill 或 wait；少這層的話，寫 probe record
/// 失敗或 `try_wait` 出錯都會把 ScreenCaptureKit child 留在 app 後面繼續跑。
struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn id(&self) -> u32 {
        self.child.as_ref().expect("armed child guard").id()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.as_mut().expect("armed child guard").try_wait()
    }

    fn disarm(&mut self) {
        self.child.take();
    }

    fn stop_and_reap(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        let kill_error = child.kill().err();
        let deadline = Instant::now() + REAP_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    return match kill_error {
                        None => Ok(()),
                        Some(error) => Err(format!(
                            "kill returned {error}, but the child subsequently exited and was reaped"
                        )),
                    };
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    return Err(match kill_error {
                        Some(error) => format!(
                            "kill returned {error}, and the child was not waitable within 2 seconds"
                        ),
                        None => "the killed child was not waitable within 2 seconds".to_string(),
                    });
                }
                Err(error) => {
                    return Err(format!("cannot reap the capture child: {error}"));
                }
            }
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Err(error) = self.stop_and_reap() {
            eprintln!("capture child cleanup was incomplete: {error}");
        }
    }
}

pub fn requested_directory() -> Result<Option<PathBuf>, String> {
    let mut found = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument == ARGUMENT {
            if found.is_some() {
                return Err(format!("{ARGUMENT} appeared more than once"));
            }
            let directory = args
                .next()
                .ok_or_else(|| format!("{ARGUMENT} needs a probe directory"))?;
            found = Some(PathBuf::from(directory));
        }
    }
    Ok(found)
}

pub fn run(directory: &Path) -> Result<(), String> {
    let directory = canonical_directory(directory)?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve app executable: {error}"))?
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize app executable: {error}"))?;
    write_new_json(
        &directory.join("app.json"),
        &ProcessRecord {
            schema: 1,
            role: "app",
            pid: std::process::id(),
            executable: path_text(&executable)?,
        },
    )?;

    let child_stdout = create_new_file(&directory.join("child.stdout.log"))?;
    let child_stderr = create_new_file(&directory.join("child.stderr.log"))?;
    let child_executable = super::recorder_supervisor::recorder_path()?;
    let child = Command::new(&child_executable)
        .arg("__macos-ci-capture-probe")
        .arg(&directory)
        .stdin(Stdio::null())
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::from(child_stderr))
        .spawn()
        .map_err(|error| {
            format!(
                "cannot spawn bundled capture child {}: {error}",
                child_executable.display()
            )
        })?;
    let mut child = ChildGuard::new(child);
    write_new_json(
        &directory.join("spawn.json"),
        &SpawnRecord {
            schema: 1,
            app_pid: std::process::id(),
            child_pid: child.id(),
        },
    )?;
    create_marker(&directory.join("app-ready"))?;

    let deadline = Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                return match child.stop_and_reap() {
                    Ok(()) => Err(
                        "capture child exceeded 60 seconds and was stopped and reaped".to_string(),
                    ),
                    Err(error) => Err(format!(
                        "capture child exceeded 60 seconds and cleanup was incomplete: {error}"
                    )),
                };
            }
            Err(error) => return Err(format!("cannot inspect capture child: {error}")),
        }
    };
    // `try_wait` 已回收成功退出的行程；不要讓 Drop 對已回收的 PID 再做一次 kill。
    child.disarm();
    write_new_json(
        &directory.join("app-exit.json"),
        &ExitRecord {
            schema: 1,
            success: status.success(),
            code: status.code(),
        },
    )?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("capture child exited unsuccessfully: {status}"))
    }
}

fn canonical_directory(directory: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "probe path is not a directory: {}",
            directory.display()
        ));
    }
    directory
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize {}: {error}", directory.display()))
}

fn path_text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn create_new_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))
}

fn create_marker(path: &Path) -> Result<(), String> {
    let mut file = create_new_file(path)?;
    file.write_all(b"schema=1\n")
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut file = create_new_file(path)?;
    serde_json::to_writer(&mut file, value)
        .map_err(|error| format!("cannot serialize {}: {error}", path.display()))?;
    file.write_all(b"\n")
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}
