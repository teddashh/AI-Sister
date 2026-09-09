; On a successful mutation path, generated Setup owns the fixed mutex from installer .onInit
; through POSTINSTALL, and direct uninstall owns it from PREUNINSTALL through POSTUNINSTALL.
; Rejections release first; cancellation/process exit lets Windows reclaim handles. The pinned
; template never nests an installed NSIS uninstaller under Setup, so there is no cross-process
; borrow capability. Product processes hold the shared event for their full lifetime and
; double-check the mutex around event creation. After the product event and best-effort current-
; user image-name scan, file-level exclusion opens both installed program files without sharing
; read access. That barrier is cross-version: it catches an executing image even when an old
; binary holds no product event or the process has another name. It cannot retrofit an already-
; shipped old uninstaller or cover a portable copy outside the installation directory.
Var AI_SISTER_INSTALL_LIFECYCLE_HANDLE
Var AI_SISTER_DIAGNOSTIC_AFTER_SCAN_HANDLE
Var AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP
Var AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER
Var AI_SISTER_DIAGNOSTIC_AFTER_EXCLUSION_HANDLE

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

!macro AI_SISTER_RELEASE_PROGRAM_FILES
  ${If} $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP != ""
    System::Call 'kernel32::CloseHandle(p $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP)'
    StrCpy $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP ""
  ${EndIf}
  ${If} $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER != ""
    System::Call 'kernel32::CloseHandle(p $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER)'
    StrCpy $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER ""
  ${EndIf}
!macroend

!macro AI_SISTER_OPEN_PROGRAM_FILE fileName handleVar
  StrCpy $R2 0
  ${Do}
    System::Call 'kernel32::SetLastError(i 0)'
    System::Call 'kernel32::CreateFileW(w "$INSTDIR\${fileName}", i 0x40010000, i 4, p 0, i 3, i 0x80, p 0) p.r0 ?e'
    Pop $R1
    ${If} $0 <> -1
      ${ExitDo}
    ${EndIf}
    ${If} $R1 != 32
    ${AndIf} $R1 != 1224
      ${ExitDo}
    ${EndIf}
    IntOp $R2 $R2 + 1
    ${If} $R2 >= 8
      ${ExitDo}
    ${EndIf}
    Sleep 250
  ${Loop}
  ${If} $0 <> -1
    StrCpy ${handleVar} $0
  ${ElseIf} $R1 = 2
  ${OrIf} $R1 = 3
    StrCpy ${handleVar} ""
  ${ElseIf} $R1 = 32
  ${OrIf} $R1 = 1224
    !insertmacro AI_SISTER_RELEASE_PROGRAM_FILES
    !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterProgramFileInUse
  ${Else}
    !insertmacro AI_SISTER_RELEASE_PROGRAM_FILES
    !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
  ${EndIf}
!macroend

!macro AI_SISTER_RESTORE_PROGRAM_FILE fileName handleVar
  ${If} ${handleVar} != ""
    System::Call 'kernel32::MoveFileExW(w "$INSTDIR\${fileName}.ai-sister-previous", w "$INSTDIR\${fileName}", i 1) i.r0'
  ${EndIf}
!macroend

!macro AI_SISTER_RESTORE_PROGRAM_FILES
  !insertmacro AI_SISTER_RESTORE_PROGRAM_FILE "sister-desktop.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP
  !insertmacro AI_SISTER_RESTORE_PROGRAM_FILE "sister.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER
  !insertmacro AI_SISTER_RELEASE_PROGRAM_FILES
!macroend

!macro AI_SISTER_RENAME_PROGRAM_FILE fileName handleVar
  ${If} ${handleVar} != ""
    System::Call 'kernel32::SetLastError(i 0)'
    System::Call 'kernel32::MoveFileExW(w "$INSTDIR\${fileName}", w "$INSTDIR\${fileName}.ai-sister-previous", i 1) i.r0 ?e'
    Pop $R1
    ${If} $0 = 0
      !if "${fileName}" == "sister.exe"
        !insertmacro AI_SISTER_RESTORE_PROGRAM_FILE "sister-desktop.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP
      !endif
      !insertmacro AI_SISTER_RELEASE_PROGRAM_FILES
      !insertmacro AI_SISTER_FAIL_INSTALL_LIFECYCLE aiSisterInstallLockUnknown
    ${EndIf}
  ${EndIf}
!macroend

!macro AI_SISTER_UNLINK_PROGRAM_FILE path handleVar
  ${If} ${handleVar} != ""
    System::Call 'kernel32::DeleteFileW(w "${path}") i.r0'
    System::Call 'kernel32::CloseHandle(p ${handleVar})'
    StrCpy ${handleVar} ""
  ${EndIf}
!macroend

!macro AI_SISTER_DIAGNOSTIC_AFTER_FILE_EXCLUSION
  ReadEnvStr $R0 "AI_SISTER_DIAGNOSTIC_AFTER_FILE_EXCLUSION_MS"
  ${If} $R0 = "5000"
    System::Call 'kernel32::SetLastError(i 0)'
    System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Global\com.ted-h.ai-sister-install-after-file-exclusion-v1") p.r0 ?e'
    Pop $R1
    ${If} $0 = 0
      !insertmacro AI_SISTER_RESTORE_PROGRAM_FILES
      !insertmacro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
      SetErrorLevel 32
      Quit
    ${EndIf}
    ${If} $R1 = 183
      System::Call 'kernel32::CloseHandle(p r0)'
      !insertmacro AI_SISTER_RESTORE_PROGRAM_FILES
      !insertmacro AI_SISTER_RELEASE_INSTALL_LIFECYCLE
      SetErrorLevel 32
      Quit
    ${EndIf}
    StrCpy $AI_SISTER_DIAGNOSTIC_AFTER_EXCLUSION_HANDLE $0
    Sleep 5000
    System::Call 'kernel32::CloseHandle(p $AI_SISTER_DIAGNOSTIC_AFTER_EXCLUSION_HANDLE)'
    StrCpy $AI_SISTER_DIAGNOSTIC_AFTER_EXCLUSION_HANDLE ""
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
  !ifndef __UNINSTALL__
    !insertmacro AI_SISTER_OPEN_PROGRAM_FILE "sister-desktop.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP
    !insertmacro AI_SISTER_OPEN_PROGRAM_FILE "sister.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER
    !insertmacro AI_SISTER_RENAME_PROGRAM_FILE "sister-desktop.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP
    !insertmacro AI_SISTER_RENAME_PROGRAM_FILE "sister.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER
    !insertmacro AI_SISTER_DIAGNOSTIC_AFTER_FILE_EXCLUSION
  !else
    !insertmacro AI_SISTER_OPEN_PROGRAM_FILE "sister-desktop.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP
    !insertmacro AI_SISTER_OPEN_PROGRAM_FILE "sister.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER
    !insertmacro AI_SISTER_UNLINK_PROGRAM_FILE "$INSTDIR\sister-desktop.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP
    !insertmacro AI_SISTER_UNLINK_PROGRAM_FILE "$INSTDIR\sister.exe" $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER
    Delete "$INSTDIR\sister-desktop.exe.ai-sister-previous"
    Delete "$INSTDIR\sister.exe.ai-sister-previous"
  !endif
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro AI_SISTER_ENSURE_INSTALL_LIFECYCLE
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"
  !insertmacro AI_SISTER_DIAGNOSTIC_AFTER_PROCESS_SCAN
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro AI_SISTER_UNLINK_PROGRAM_FILE "$INSTDIR\sister-desktop.exe.ai-sister-previous" $AI_SISTER_PROGRAM_FILE_HANDLE_DESKTOP
  !insertmacro AI_SISTER_UNLINK_PROGRAM_FILE "$INSTDIR\sister.exe.ai-sister-previous" $AI_SISTER_PROGRAM_FILE_HANDLE_RECORDER
  Delete "$INSTDIR\sister-desktop.exe.ai-sister-previous"
  Delete "$INSTDIR\sister.exe.ai-sister-previous"
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

; NSIS `File` 失敗、Abort 或使用者中止都不會經過 POSTINSTALL；這裡把已改名的舊檔搬回原路徑
; 再關 handle，讓安裝根目錄回到升級前的樣子。自己的 Quit 路徑不會進來（Quit 不觸發 callback）。
Function .onInstFailed
  !insertmacro AI_SISTER_RESTORE_PROGRAM_FILES
FunctionEnd
