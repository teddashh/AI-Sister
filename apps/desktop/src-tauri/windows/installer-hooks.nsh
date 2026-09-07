; An in-place update must not tear down a live recorder. Tauri's stock NSIS
; template otherwise offers to kill the main GUI process; this earlier hook
; refuses the operation while either executable is still owned by this user.
!macro AI_SISTER_REQUIRE_STOPPED executableName
  nsis_tauri_utils::FindProcessCurrentUser "${executableName}"
  Pop $R0
  ${If} $R0 = 0
    ${IfNot} ${Silent}
      MessageBox MB_ICONSTOP|MB_OK "$(aiSisterStillRunning)"
    ${EndIf}
    SetErrorLevel 32
    Quit
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"
  !insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"
!macroend
