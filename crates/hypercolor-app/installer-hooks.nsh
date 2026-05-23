; NSIS installer hooks for Hypercolor's Windows bundle.
;
; Tauri's templated NSIS installer is fine for the file/registry steps,
; but it has no knowledge of our SCM-registered helper services. These
; hooks fill that gap so installs and uninstalls don't leave orphan
; service entries the user has to clean up manually.
;
; Wired in via bundle.windows.nsis.installerHooks in
; tauri.windows.bundle.conf.json.

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop + delete the HypercolorSmBus broker service. NSIS runs the
  ; uninstaller elevated, so sc.exe inherits the necessary rights.
  ; nsExec::ExecToLog silently swallows missing-service failures — we
  ; never want an absent service to block uninstall on retried runs.
  DetailPrint "Stopping HypercolorSmBus service"
  nsExec::ExecToLog 'sc.exe stop HypercolorSmBus'
  Pop $0

  DetailPrint "Removing HypercolorSmBus service registration"
  nsExec::ExecToLog 'sc.exe delete HypercolorSmBus'
  Pop $0

  ; PawnIO is intentionally left installed: it's a shared system
  ; component other software may rely on. Users who really want it
  ; gone can uninstall it separately from Programs & Features.
!macroend
