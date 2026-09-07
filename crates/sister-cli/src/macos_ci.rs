//! Feature-gated child half of the macOS app-tree capture probe.
//!
//! This is deliberately parsed before the public CLI and never exists in a shipping build.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ARGUMENT: &str = "__macos-ci-capture-probe";
const GO_TIMEOUT: Duration = Duration::from_secs(30);
const CAPTURE_PROCESS_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Serialize)]
struct ProcessRecord {
    schema: u8,
    role: &'static str,
    pid: u32,
    executable: String,
}

/// Find the private feature-only argument without letting normal clap parsing see it.
pub fn requested_directory() -> Result<Option<PathBuf>> {
    let mut found = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument == ARGUMENT {
            if found.is_some() {
                bail!("{ARGUMENT} appeared more than once");
            }
            let directory = args
                .next()
                .with_context(|| format!("{ARGUMENT} needs a probe directory"))?;
            found = Some(PathBuf::from(directory));
        }
    }
    Ok(found)
}

pub fn run(directory: &Path) -> Result<()> {
    let directory = canonical_directory(directory)?;
    let executable = std::env::current_exe()
        .context("cannot resolve the capture child executable")?
        .canonicalize()
        .context("cannot canonicalize the capture child executable")?;
    write_new_json(
        &directory.join("child.json"),
        &ProcessRecord {
            schema: 1,
            role: "capture_child",
            pid: std::process::id(),
            executable: path_text(&executable)?,
        },
    )?;
    create_marker(&directory.join("child-ready"))?;

    let go = directory.join("go");
    let deadline = Instant::now() + GO_TIMEOUT;
    while !go.try_exists().context("cannot inspect the go marker")? {
        if Instant::now() >= deadline {
            bail!("the app-tree verifier did not send go within 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // ScreenCaptureKit 的同步 screenshot API 沒有 cancellation handle。逾時不能只
    // 放掉 worker 再宣稱「停止」；這個 private CLI 本身就是隔離邊界，所以 watchdog
    // 到點直接結束整個 child process。外層 app 另外握著 Child handle 作第二層回收。
    let _watchdog = std::thread::Builder::new()
        .name("macos-capture-process-watchdog".to_string())
        .spawn(|| {
            std::thread::sleep(CAPTURE_PROCESS_TIMEOUT);
            eprintln!("macOS capture diagnostic exceeded 20 seconds; exiting child process");
            std::process::exit(124);
        })
        .context("cannot start the capture process watchdog")?;
    let report = sister_capture::macos::diagnostic_probe();
    write_new_json(&directory.join("capture.json"), &report)
}

fn canonical_directory(directory: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(directory)
        .with_context(|| format!("cannot inspect probe directory {}", directory.display()))?;
    if !metadata.file_type().is_dir() {
        bail!("probe path is not a directory: {}", directory.display());
    }
    directory.canonicalize().with_context(|| {
        format!(
            "cannot canonicalize probe directory {}",
            directory.display()
        )
    })
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .with_context(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn create_marker(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("cannot create marker {}", path.display()))?;
    file.write_all(b"schema=1\n")?;
    file.sync_all()?;
    Ok(())
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    serde_json::to_writer(&mut file, value)
        .with_context(|| format!("cannot serialize {}", path.display()))?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
