use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PlatformAccessView {
    platform: &'static str,
    screen_recording: Option<bool>,
    accessibility: Option<bool>,
}

#[tauri::command]
pub fn platform_access_read() -> PlatformAccessView {
    #[cfg(target_os = "macos")]
    {
        use objc2_application_services::AXIsProcessTrusted;
        use objc2_core_graphics::CGPreflightScreenCaptureAccess;
        PlatformAccessView {
            platform: "macos",
            screen_recording: Some(CGPreflightScreenCaptureAccess()),
            accessibility: Some(unsafe { AXIsProcessTrusted() }),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PlatformAccessView {
            platform: std::env::consts::OS,
            screen_recording: None,
            accessibility: None,
        }
    }
}

#[tauri::command]
pub fn platform_access_open(kind: String) -> Result<PlatformAccessView, String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_core_graphics::CGRequestScreenCaptureAccess;

        let destination = match kind.as_str() {
            "screen-recording" => {
                // 這條 IPC 只從 trusted click 進來；系統 prompt 若已回答過，
                // macOS 會直接回現況，然後我們開 exact 設定頁供重新授權。
                let _ = CGRequestScreenCaptureAccess();
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            "accessibility" => {
                use objc2_application_services::{
                    AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt,
                };
                use objc2_core_foundation::{CFBoolean, CFDictionary};

                let prompt = CFBoolean::new(true);
                let options =
                    CFDictionary::from_slices(&[unsafe { kAXTrustedCheckOptionPrompt }], &[prompt]);
                let _ = unsafe { AXIsProcessTrustedWithOptions(Some(options.cast_unchecked())) };
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            _ => return Err("不認得這個 macOS 權限項目".to_owned()),
        };
        std::process::Command::new("/usr/bin/open")
            .arg(destination)
            .spawn()
            .map_err(|error| format!("開不起 macOS 權限設定：{error}"))?;
        Ok(platform_access_read())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = kind;
        Err("這個平台不使用 macOS 擷取權限".to_owned())
    }
}
