; On a successful mutation path, generated Setup owns the fixed mutex from installer .onInit
; through POSTINSTALL, and direct uninstall owns it from PREUNINSTALL through POSTUNINSTALL.
; Rejections release first; cancellation/process exit lets Windows reclaim handles. The pinned
; template never nests an installed NSIS uninstaller under Setup, so there is no cross-process
; borrow capability. Product processes hold the shared event for their full lifetime and
; double-check the mutex around event creation. Older products still need the best-effort
; image-name scan.
Var AI_SISTER_INSTALL_LIFECYCLE_HANDLE
Var AI_SISTER_DIAGNOSTIC_AFTER_SCAN_HANDLE

!macro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
  ${If} $AI_SISTER_INSTALL_LIFECYCLE_HANDLE != ""
    System::Call 'kernel32::CloseHandle(p $AI_SISTER_INSTALL_LIFECYCLE_HANDLE)'
    StrCpy $AI_SISTER_INSTALL_LIFECYCLE_HANDLE ""
  ${EndIf}
!macroend

!macro AI_SISTER_FAIL_INSTALL_LIFECYCLE message
  !insertmacro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
  ${IfNot} ${Silent}
    MessageBox MB_ICONSTOP|MB_OK "$(${message})"
  ${EndIf}
  SetErrorLevel 32
  Quit
!macroend

!macro AI_SISTER_PROBE_PRODUCT_LIFECYCLE
  ; Missing is the only clear result. Success means an alpha.113-aware product is alive;
  ; every other native error fails closed instead of becoming a fake absence.
  System::Call 'kernel32::SetLastError(i 0)'
  System::Call 'kernel32::OpenEventW(i 0x00100000, i 0, w "Global\com.ted-h.ai-sister-product-lifecycle-v1") p.r0 ?e'
  Pop $R1
  ${If} $0 != 0
    System::Call 'kernel32::CloseHandle(p r0) i.r3'
    ${If} $3 = 0
      !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
    ${EndIf}
    !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterProductLifecycleBusy
  ${EndIf}
  ${If} $R1 != 2
    !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
  ${EndIf}
!macroend

!macro AI_SISTER_ACQUIRE_INSTALL_LIFECYCLE
  System::Call 'kernel32::SetLastError(i 0)'
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Global\com.ted-h.ai-sister-install-lifecycle-v1") p.r0 ?e'
  Pop $R1
  ${If} $0 = 0
    !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
  ${EndIf}
  ${If} $R1 = 183
    System::Call 'kernel32::CloseHandle(p r0)'
    !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallBusy
  ${EndIf}
  StrCpy $AI_SISTER_INSTALL_LIFECYCLE_HANDLE $0

  !insertmacro AI_SISTER_PROBE_PRODUCT_LIFECYCLE
  ReadEnvStr $R0 "AI_SISTER_DIAGNOSTIC_INSTALL_DELAY_MS"
  ${If} $R0 = "15000"
    Sleep 15000
  ${EndIf}
!macroend

!macro AI_SISTER_ENSURE_INSTALL_LIFECYCLE
  ${If} $AI_SISTER_INSTALL_LIFECYCLE_HANDLE = ""
    !insertmacro AI_SISTER_ACQUIRE_INSTALL_LIFECYCLE
  ${EndIf}
!macroend

!macro AI_SISTER_ENSURE_UNINSTALL_LIFECYCLE
  ${If} $AI_SISTER_INSTALL_LIFECYCLE_HANDLE = ""
    !insertmacro AI_SISTER_ACQUIRE_INSTALL_LIFECYCLE
  ${EndIf}
!macroend

!macro AI_SISTER_DIAGNOSTIC_AFTER_PROCESS_SCAN
  ReadEnvStr $R0 "AI_SISTER_DIAGNOSTIC_AFTER_PROCESS_SCAN_MS"
  ${If} $R0 = "5000"
    System::Call 'kernel32::SetLastError(i 0)'
    System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Global\com.ted-h.ai-sister-install-after-process-scan-v1") p.r0 ?e'
    Pop $R1
    ${If} $0 = 0
      !insertmacro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
      SetErrorLevel 32
      Quit
    ${EndIf}
    ${If} $R1 = 183
      System::Call 'kernel32::CloseHandle(p r0)'
      !insertmacro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
      SetErrorLevel 32
      Quit
    ${EndIf}
    StrCpy $AI_SISTER_DIAGNOSTIC_AFTER_SCAN_HANDLE $0
    Sleep 5000
    System::Call 'kernel32::CloseHandle(p $AI_SISTER_DIAGNOSTIC_AFTER_SCAN_HANDLE)'
    StrCpy $AI_SISTER_DIAGNOSTIC_AFTER_SCAN_HANDLE ""
  ${EndIf}
!macroend

; The pinned helper has only found/not-found results; snapshot or SID-query errors are folded
; into not-found. It remains a compatibility scan for old products, not the alpha.113 barrier.
!macro AI_SISTER_REQUIRE_STOPPED executableName
  nsis_tauri_utils::FindProcessCurrentUser "${executableName}"
  Pop $R0
  ${If} $R0 = 0
    !insertmacro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
    ${IfNot} ${Silent}
      MessageBox MB_ICONSTOP|MB_OK "$(aiSisterStillRunning)"
    ${EndIf}
    SetErrorLevel 32
    Quit
  ${EndIf}
!macroend

; utils.nsh is included immediately before this hook. Redefining its complete pinned body puts
; Setup acquisition in installer .onInit, ahead of PageLeaveReinstall/WiX, WebView2 and every
; payload/registry section. Direct uninstallers intentionally wait for PREUNINSTALL so the
; confirmation page does not block products and the saved language has already been restored.
!macroundef SetContext
!macro SetContext
  !ifndef __UNINSTALL__
    !insertmacro AI_SISTER_ENSURE_INSTALL_LIFECYCLE
    !insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"
    !insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"
  !endif

  !if "${INSTALLMODE}" == "currentUser"
    SetShellVarContext current
  !else if "${INSTALLMODE}" == "perMachine"
    SetShellVarContext all
  !endif

  ${If} ${RunningX64}
    !if "${ARCH}" == "x64"
      SetRegView 64
    !else if "${ARCH}" == "arm64"
      SetRegView 64
    !else
      SetRegView 32
    !endif
  ${EndIf}
!macroend

; Replace the stock macro so sections generated by this version never offer or invoke kill.
!macroundef CheckIfAppIsRunning
!macro CheckIfAppIsRunning executableName productName
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro AI_SISTER_ENSURE_INSTALL_LIFECYCLE
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"
  !insertmacro AI_SISTER_DIAGNOSTIC_AFTER_PROCESS_SCAN
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro AI_SISTER_ENSURE_UNINSTALL_LIFECYCLE
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"
  !insertmacro AI_SISTER_DIAGNOSTIC_AFTER_PROCESS_SCAN
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
!macroend
