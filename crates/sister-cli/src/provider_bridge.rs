//! desktop 寫進 `[brain]` 的 hidden provider bridge。
//!
//! 外層 `sister-core::brain::spawn_cli` 仍是唯一持 CloudAllowed／master-stop gate 的
//! 出境口。這一層只把已經從 stdin 收到的同一份文字，換成四支 CLI 各自真正
//! 支援的非互動呼叫方式。

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, ensure};
use sister_core::provider_cli::BrainProvider;

static NEXT_PROMPT_FILE: AtomicU64 = AtomicU64::new(1);
static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(1);

struct PrivateWorkspace {
    path: PathBuf,
}

impl PrivateWorkspace {
    fn create() -> Result<Self> {
        let directory = std::env::temp_dir()
            .join("AI-Sister")
            .join("provider-workspaces");
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("建立 provider 工作目錄 {}", directory.display()))?;
        for _ in 0..32 {
            let serial = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                "run-{}-{}-{}",
                std::process::id(),
                sister_core::now_ms(),
                serial
            ));
            let created = create_private_directory(&path);
            match created {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("建立 provider 工作目錄 {}", path.display()));
                }
            }
        }
        anyhow::bail!("建立 provider 工作目錄時連續撞名")
    }
}

impl Drop for PrivateWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::DirBuilder::new().create(path)
    }
}

struct PrivatePromptFile {
    path: PathBuf,
    // Windows 用 DELETE_ON_CLOSE；handle 要活到 provider 完整讀完。行程被 Job
    // Object 強制收掉時 OS 也會清檔，不必等 Rust Drop。
    _file: File,
}

impl PrivatePromptFile {
    fn create(contents: &str) -> Result<Self> {
        let directory = std::env::temp_dir()
            .join("AI-Sister")
            .join("provider-prompts");
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("建立 provider prompt 目錄 {}", directory.display()))?;

        for _ in 0..32 {
            let serial = NEXT_PROMPT_FILE.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                "prompt-{}-{}-{}.txt",
                std::process::id(),
                sister_core::now_ms(),
                serial
            ));
            let opened = open_private_new(&path);
            let file = match opened {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("建立 provider prompt {}", path.display()));
                }
            };
            let mut prompt = Self { path, _file: file };
            prompt
                ._file
                .write_all(contents.as_bytes())
                .with_context(|| format!("寫入 provider prompt {}", prompt.path.display()))?;
            prompt
                ._file
                .flush()
                .with_context(|| format!("寫入 provider prompt {}", prompt.path.display()))?;
            return Ok(prompt);
        }
        anyhow::bail!("建立 provider prompt 檔時連續撞名")
    }
}

impl Drop for PrivatePromptFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn open_private_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE；Grok 可以打開同一檔，
        // parent 或整個 bridge 被終止後又由 OS 刪除。
        options.share_mode(0x0000_0007);
        options.custom_flags(0x0400_0000); // FILE_FLAG_DELETE_ON_CLOSE
        options.attributes(0x0000_0100); // FILE_ATTRIBUTE_TEMPORARY
    }
    options.open(path)
}

pub fn run(provider: BrainProvider, executable: &Path) -> Result<()> {
    ensure!(
        executable.is_file(),
        "找不到已設定的 {}：{}",
        provider.label(),
        executable.display()
    );

    let mut prompt = String::new();
    std::io::stdin()
        .read_to_string(&mut prompt)
        .context("讀取 provider prompt")?;
    run_prompt(provider, executable, &prompt)
}

fn run_prompt(provider: BrainProvider, executable: &Path, prompt: &str) -> Result<()> {
    ensure!(!prompt.trim().is_empty(), "provider prompt 是空的");

    // CLI 的 cwd 固定是一個空的 private directory，不是 recorder 啟動時碰巧位於
    // 哪個 workspace。送出去的使用者正文仍只有 stdin／Grok prompt file 那一份。
    let workspace = PrivateWorkspace::create()?;
    let prompt_file = if provider == BrainProvider::Grok {
        Some(PrivatePromptFile::create(prompt)?)
    } else {
        None
    };
    let args = provider.runtime_args(prompt_file.as_ref().map(|file| file.path.as_path()));
    let mut command = Command::new(executable);
    command
        .args(&args)
        .current_dir(&workspace.path)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if provider == BrainProvider::Grok {
        command.stdin(Stdio::null());
    } else {
        command.stdin(Stdio::piped());
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("啟動 {} {}", provider.label(), executable.display()))?;
    if provider != BrainProvider::Grok {
        let mut stdin = child.stdin.take().context("provider stdin 沒開成")?;
        stdin
            .write_all(prompt.as_bytes())
            .context("寫入 provider stdin")?;
        drop(stdin);
    }
    let status = child.wait().context("等待 provider 回覆")?;
    ensure!(
        status.success(),
        "{} 結束碼 {}",
        provider.label(),
        status
            .code()
            .map_or_else(|| "unknown".to_owned(), |v| v.to_string())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_prompt_file_is_removed_on_drop() {
        let path = {
            let file = PrivatePromptFile::create("private screen text").unwrap();
            assert_eq!(
                std::fs::read_to_string(&file.path).unwrap(),
                "private screen text"
            );
            file.path.clone()
        };
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_prompt_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let file = PrivatePromptFile::create("private").unwrap();
        let mode = std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn every_provider_receives_the_prompt_in_an_empty_private_workspace() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "sister-provider-bridge-fixture-{}-{}",
            std::process::id(),
            NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let executable = root.join("provider-fixture.sh");
        std::fs::write(
            &executable,
            b"#!/bin/sh\nout=\"$0.out\"\ncwd=\"$0.cwd\"\nprompt_file=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--prompt-file\" ]; then\n    shift\n    prompt_file=$1\n  fi\n  shift\ndone\npwd > \"$cwd\"\nif [ -n \"$prompt_file\" ]; then\n  cp \"$prompt_file\" \"$out\"\nelse\n  cat > \"$out\"\nfi\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = PathBuf::from(format!("{}.out", executable.display()));
        let cwd_output = PathBuf::from(format!("{}.cwd", executable.display()));

        for provider in BrainProvider::ALL {
            let prompt = format!("private screen text for {}", provider.id());
            run_prompt(provider, &executable, &prompt).unwrap();
            assert_eq!(std::fs::read_to_string(&output).unwrap(), prompt);
            let cwd = PathBuf::from(std::fs::read_to_string(&cwd_output).unwrap().trim());
            // macOS 的 `temp_dir()` 會給 `/var/...`，而 child 的 `pwd` 會把同一條
            // 路徑實體化成 `/private/var/...`。先把共同的既存 parent 正規化，
            // 比較的才是同一個目錄，不是兩種字面拼法。
            let expected_parent = std::fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join("AI-Sister");
            assert!(cwd.starts_with(expected_parent));
            assert_ne!(cwd, root);
            assert!(
                !cwd.exists(),
                "private workspace should be removed after exit"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
