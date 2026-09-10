//! 已安裝 CLI 大腦的共同接線表。
//!
//! desktop 用這張表做偵測／登入，`sister` 的 hidden bridge 用同一張表做
//! 非互動呼叫。兩邊不能各自手抄參數：登入測通一套、recorder 實際跑另一套，
//! 畫面就會把一個不能用的選擇寫成「使用中」。

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const BRIDGE_COMMAND: &str = "brain-cli-bridge";
pub const READY_TOKEN: &str = "AI_SISTER_READY_7219";
pub const READY_PROMPT: &str =
    "This is a connection test. Reply with exactly AI_SISTER_READY_7219 and nothing else.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrainProvider {
    Claude,
    Codex,
    Gemini,
    Grok,
}

impl BrainProvider {
    pub const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::Gemini, Self::Grok];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Gemini => "Gemini CLI",
            Self::Grok => "Grok CLI",
        }
    }

    pub const fn command_name(self) -> &'static str {
        self.id()
    }

    pub const fn login_args(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["auth", "login"],
            Self::Codex => &["login", "--device-auth"],
            Self::Gemini => &[],
            Self::Grok => &["login", "--device-auth"],
        }
    }

    pub const fn version_args(self) -> &'static [&'static str] {
        &["--version"]
    }

    /// prompt 仍從 bridge 的 stdin 進來；這裡只列不含正文的固定參數。
    pub fn runtime_args(self, grok_prompt: Option<&Path>) -> Vec<String> {
        match self {
            Self::Claude => [
                "-p",
                "--tools",
                "",
                "--disable-slash-commands",
                "--no-session-persistence",
                "--safe-mode",
                "--strict-mcp-config",
                "--permission-prompts",
                "none",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            Self::Codex => [
                "exec",
                "--skip-git-repo-check",
                "--ephemeral",
                "--color",
                "never",
                "--sandbox",
                "read-only",
                "--ignore-rules",
                "-",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            Self::Gemini => ["--output-format", "text", "--approval-mode", "plan"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Self::Grok => {
                let prompt = grok_prompt.expect("Grok runtime 必須先建立 prompt file");
                vec![
                    "--prompt-file".to_owned(),
                    prompt.to_string_lossy().into_owned(),
                    "--output-format".to_owned(),
                    "plain".to_owned(),
                    "--disable-web-search".to_owned(),
                    "--no-subagents".to_owned(),
                    "--permission-mode".to_owned(),
                    "plan".to_owned(),
                    "--sandbox".to_owned(),
                    "read-only".to_owned(),
                    "--max-turns".to_owned(),
                    "1".to_owned(),
                    "--verbatim".to_owned(),
                ]
            }
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.id() == value)
    }
}

impl fmt::Display for BrainProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// 寫進 `[brain]` 的完整 argv。正文不在裡面，只會由 recorder 寫到 bridge stdin。
pub fn bridge_args(provider: BrainProvider, executable: &Path) -> Vec<String> {
    vec![
        BRIDGE_COMMAND.to_owned(),
        provider.id().to_owned(),
        executable.to_string_lossy().into_owned(),
    ]
}

/// 只接受這一版自己寫出的 exact shape，不用「args 裡剛好有 grok」猜。
pub fn parse_bridge_args(args: &[String]) -> Option<(BrainProvider, PathBuf)> {
    let [bridge, provider, executable] = args else {
        return None;
    };
    if bridge != BRIDGE_COMMAND || executable.trim().is_empty() {
        return None;
    }
    Some((BrainProvider::from_id(provider)?, PathBuf::from(executable)))
}

pub fn output_has_ready_token(stdout: &str) -> bool {
    stdout.lines().any(|line| line.trim() == READY_TOKEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_provider_ids_and_login_routes_are_exact() {
        assert_eq!(
            BrainProvider::ALL.map(BrainProvider::id),
            ["claude", "codex", "gemini", "grok"]
        );
        assert_eq!(BrainProvider::Claude.login_args(), ["auth", "login"]);
        assert_eq!(
            BrainProvider::Codex.login_args(),
            ["login", "--device-auth"]
        );
        assert!(BrainProvider::Gemini.login_args().is_empty());
        assert_eq!(BrainProvider::Grok.login_args(), ["login", "--device-auth"]);
    }

    #[test]
    fn bridge_round_trip_never_puts_prompt_in_argv() {
        for provider in BrainProvider::ALL {
            let executable = Path::new("C:\\Tools\\provider.exe");
            let args = bridge_args(provider, executable);
            assert!(!args.iter().any(|arg| arg.contains(READY_PROMPT)));
            assert_eq!(
                parse_bridge_args(&args),
                Some((provider, executable.to_path_buf()))
            );
        }
    }

    #[test]
    fn malformed_bridge_shapes_are_not_recognized() {
        assert!(parse_bridge_args(&[]).is_none());
        assert!(parse_bridge_args(&[BRIDGE_COMMAND.into(), "claude".into(), "".into()]).is_none());
        assert!(
            parse_bridge_args(&[
                BRIDGE_COMMAND.into(),
                "claude".into(),
                "provider.exe".into(),
                "extra".into(),
            ])
            .is_none()
        );
    }

    #[test]
    fn readiness_requires_an_exact_output_line() {
        assert!(output_has_ready_token(READY_TOKEN));
        assert!(output_has_ready_token(&format!("noise\n{READY_TOKEN}\n")));
        assert!(!output_has_ready_token("AI_SISTER_READY"));
        assert!(!output_has_ready_token(&format!("prefix {READY_TOKEN}")));
    }

    #[test]
    fn runtime_profiles_are_noninteractive_and_prompt_free() {
        for provider in BrainProvider::ALL {
            let prompt_file = Path::new("C:\\Temp\\prompt.txt");
            let args = provider.runtime_args(Some(prompt_file));
            assert!(!args.iter().any(|arg| arg.contains(READY_PROMPT)));
            match provider {
                BrainProvider::Claude => assert!(args.iter().any(|arg| arg == "-p")),
                BrainProvider::Codex => assert_eq!(args.last().map(String::as_str), Some("-")),
                BrainProvider::Gemini => {
                    assert!(args.windows(2).any(|w| w == ["--approval-mode", "plan"]))
                }
                BrainProvider::Grok => assert!(
                    args.windows(2)
                        .any(|w| w == ["--prompt-file", prompt_file.to_str().unwrap()])
                ),
            }
        }
    }
}
