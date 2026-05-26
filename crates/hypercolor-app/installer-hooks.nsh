; NSIS installer hooks for Hypercolor's Windows bundle.
;
; Tauri's templated NSIS installer handles the file/registry steps,
; but it has no knowledge of our hardware access stack (PawnIO kernel
; driver + Windows Firewall exception), and no concept of cleaning that
; stack up on uninstall. These hooks fill that gap while keeping
; privileged SMBus broker setup as an explicit administrator action.
;
; Wired in via bundle.windows.nsis.installerHooks in
; tauri.windows.bundle.conf.json. The installer runs elevated
; (installMode = perMachine), so sc.exe / netsh / PawnIO_setup.exe
; all inherit the rights they need.

!macro NSIS_HOOK_POSTINSTALL
  ; PawnIO kernel driver — silent install of the bundled installer.
  ; Skips if PawnIO is already present (the script's Resolve-PawnIoHome
  ; short-circuits the setup.exe run). Modules are copied into PawnIO's
  ; install dir so the broker can find them via pawnio_module_dirs().
  ;
  ; The bundled script propagates Windows installer exit code 3010
  ; ("reboot required") when the kernel driver needs a restart to finish
  ; binding to SCM. We stash that into $R0 so we can prompt the user to
  ; restart at the end of postinstall.
  DetailPrint "Installing PawnIO hardware access (this may take a moment)..."
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\tools\install-bundled-pawnio.ps1" -AssetRoot "$INSTDIR\tools\pawnio" -Silent'
  Pop $R0
  DetailPrint "  PawnIO install exit code: $R0"

  ; HypercolorSmBus broker service is no longer auto-installed by default.
  ; Installing and starting a privileged broker remains an explicit
  ; administrator action outside the main installer.

  ; Windows Firewall — pre-grant the daemon so mDNS discovery and any
  ; future inbound traffic don't trigger the "Allow on public networks?"
  ; popup the first time the user opens Hypercolor. The daemon only
  ; binds 127.0.0.1 for the HTTP API; the inbound exception is for
  ; mDNS multicast responses on UDP 5353.
  DetailPrint "Adding Windows Firewall rules for Hypercolor..."
  nsExec::ExecToLog 'netsh.exe advfirewall firewall delete rule name="Hypercolor Daemon"'
  Pop $0
  nsExec::ExecToLog 'netsh.exe advfirewall firewall add rule name="Hypercolor Daemon" dir=in action=allow program="$INSTDIR\hypercolor-daemon.exe" profile=domain,private,public enable=yes'
  Pop $0
  nsExec::ExecToLog 'netsh.exe advfirewall firewall delete rule name="Hypercolor App"'
  Pop $0
  nsExec::ExecToLog 'netsh.exe advfirewall firewall add rule name="Hypercolor App" dir=in action=allow program="$INSTDIR\hypercolor-app.exe" profile=domain,private,public enable=yes'
  Pop $0

  ; If PawnIO asked for a reboot, surface it. The MUI2 finish page
  ; doesn't natively expose a reboot prompt for installer-driven
  ; restarts, so a simple MessageBox keeps the user informed instead
  ; of letting them launch Hypercolor into a broken hardware-access
  ; state.
  ${If} $R0 = 3010
    MessageBox MB_YESNO|MB_ICONQUESTION "Hypercolor installed successfully, but the PawnIO kernel driver needs a Windows restart before hardware lighting can come online. Restart now?" IDNO no_reboot_now
      Reboot
    no_reboot_now:
  ${EndIf}
!macroend

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

  ; Drop Windows Firewall exceptions so an uninstall doesn't leave
  ; rules pointing at a path that no longer exists.
  DetailPrint "Removing Windows Firewall rules for Hypercolor"
  nsExec::ExecToLog 'netsh.exe advfirewall firewall delete rule name="Hypercolor Daemon"'
  Pop $0
  nsExec::ExecToLog 'netsh.exe advfirewall firewall delete rule name="Hypercolor App"'
  Pop $0

  ; PawnIO is intentionally left installed: it's a shared system
  ; component other software may rely on. Users who really want it
  ; gone can uninstall it separately from Programs & Features.
!macroend
