; NSIS hooks of the Windows installer (tauri.conf.json bundle.windows.nsis.installerHooks).

; Before an uninstall, remove the scheduled tasks of group jobs (\claude-sync\<id>, see
; docs/architecture.md section 16). An update runs no uninstaller, and this skips /UPDATE
; anyway, so updates keep the tasks. The acceptances stay in ~/.claude-sync/jobs.json: a
; reinstall registers the accepted jobs again.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    nsExec::Exec '"$INSTDIR\claude-sync.exe" remove-jobs'
    Pop $0
  ${EndIf}
!macroend
