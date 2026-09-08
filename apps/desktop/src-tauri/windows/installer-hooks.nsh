; On a successful mutation path, generated Setup owns the fixed mutex from installer .onInit
; through POSTINSTALL, and direct uninstall owns it from PREUNINSTALL through POSTUNINSTALL.
; Rejections release first; cancellation/process exit lets Windows reclaim handles. An uninstaller
; spawned by PageLeaveReinstall borrows its parent's marker through an inherited capability.
; Product processes hold the shared event for their full lifetime and double-check the mutex
; around event creation. Older products still need the best-effort image-name scan.
Var AI_SISTER_INSTALL_LIFECYCLE_HANDLE
Var AI_SISTER_INSTALL_CAPABILITY_HANDLE
Var AI_SISTER_INSTALL_LIFECYCLE_BORROWED
Var AI_SISTER_DIAGNOSTIC_AFTER_SCAN_HANDLE

!macro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
  ${If} $AI_SISTER_INSTALL_LIFECYCLE_BORROWED = "1"
    System::Call 'kernel32::SetEnvironmentVariableW(w "AI_SISTER_INSTALL_LIFECYCLE_CAPABILITY", p 0)'
    ${If} $AI_SISTER_INSTALL_LIFECYCLE_HANDLE != ""
      ; This is the child's own OpenMutex handle, not the parent's handle. Retaining it through
      ; POSTUNINSTALL keeps the fixed object alive even if the waiting parent exits unexpectedly.
      System::Call 'kernel32::CloseHandle(p $AI_SISTER_INSTALL_LIFECYCLE_HANDLE)'
      StrCpy $AI_SISTER_INSTALL_LIFECYCLE_HANDLE ""
    ${EndIf}
    StrCpy $AI_SISTER_INSTALL_LIFECYCLE_BORROWED ""
  ${ElseIf} $AI_SISTER_INSTALL_LIFECYCLE_HANDLE != ""
    System::Call 'kernel32::SetEnvironmentVariableW(w "AI_SISTER_INSTALL_LIFECYCLE_CAPABILITY", p 0)'
    ${If} $AI_SISTER_INSTALL_CAPABILITY_HANDLE != ""
      System::Call 'kernel32::CloseHandle(p $AI_SISTER_INSTALL_CAPABILITY_HANDLE)'
      StrCpy $AI_SISTER_INSTALL_CAPABILITY_HANDLE ""
    ${EndIf}
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

!macro AI_SISTER_ACQUIRE_INSTALL_LIFECYCLE publishCapability
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

  !if "${publishCapability}" == "1"
    ; PageLeaveReinstall may ExecWait an installed alpha.113+ uninstaller. A dynamic event name
    ; passed through this Setup's process environment lets a later child validate that it may
    ; borrow, but never close, this parent's marker.
    System::Call 'kernel32::GetCurrentProcessId() i.r2'
    StrCpy $2 "Local\com.ted-h.ai-sister-install-parent-$2"
    System::Call 'kernel32::SetLastError(i 0)'
    System::Call 'kernel32::CreateEventW(p 0, i 1, i 0, w r2) p.r0 ?e'
    Pop $R1
    ${If} $0 = 0
      !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
    ${EndIf}
    ${If} $R1 = 183
      System::Call 'kernel32::CloseHandle(p r0)'
      !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
    ${EndIf}
    StrCpy $AI_SISTER_INSTALL_CAPABILITY_HANDLE $0
    System::Call 'kernel32::SetEnvironmentVariableW(w "AI_SISTER_INSTALL_LIFECYCLE_CAPABILITY", w r2) i.r0'
    ${If} $0 = 0
      !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
    ${EndIf}
  !endif

  !insertmacro AI_SISTER_PROBE_PRODUCT_LIFECYCLE
  ReadEnvStr $R0 "AI_SISTER_DIAGNOSTIC_INSTALL_DELAY_MS"
  ${If} $R0 = "15000"
    Sleep 15000
  ${EndIf}
!macroend

!macro AI_SISTER_ENSURE_INSTALL_LIFECYCLE
  ${If} $AI_SISTER_INSTALL_LIFECYCLE_HANDLE = ""
    ; Setup never trusts an ambient borrow capability. Only a child uninstaller may borrow.
    !insertmacro AI_SISTER_ACQUIRE_INSTALL_LIFECYCLE 1
  ${EndIf}
!macroend

!macro AI_SISTER_ENSURE_UNINSTALL_LIFECYCLE
  ${If} $AI_SISTER_INSTALL_LIFECYCLE_HANDLE = ""
  ${AndIf} $AI_SISTER_INSTALL_LIFECYCLE_BORROWED != "1"
    ReadEnvStr $2 "AI_SISTER_INSTALL_LIFECYCLE_CAPABILITY"
    ${If} $2 = ""
      !insertmacro AI_SISTER_ACQUIRE_INSTALL_LIFECYCLE 0
    ${Else}
      System::Call 'kernel32::SetLastError(i 0)'
      System::Call 'kernel32::OpenMutexW(i 0x00100000, i 0, w "Global\com.ted-h.ai-sister-install-lifecycle-v1") p.r0 ?e'
      Pop $R1
      ${If} $0 = 0
        !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
      ${EndIf}
      System::Call 'kernel32::SetLastError(i 0)'
      System::Call 'kernel32::OpenEventW(i 0x00100000, i 0, w r2) p.r3 ?e'
      Pop $R1
      ${If} $3 = 0
        System::Call 'kernel32::CloseHandle(p r0)'
        !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
      ${EndIf}
      System::Call 'kernel32::CloseHandle(p r3) i.r1'
      ${If} $1 = 0
        System::Call 'kernel32::CloseHandle(p r0)'
        !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
      ${EndIf}
      ; Keep this child's own fixed-object handle for the full uninstall section. Parent and child
      ; handles are independent; closing this one at POST never closes a handle in the parent.
      StrCpy $AI_SISTER_INSTALL_LIFECYCLE_HANDLE $0
      StrCpy $AI_SISTER_INSTALL_LIFECYCLE_BORROWED "1"
    ${EndIf}
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
