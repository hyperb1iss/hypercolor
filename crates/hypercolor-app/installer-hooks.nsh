; NSIS installer hooks for Hypercolor's Windows bundle.
;
; Tauri's templated NSIS installer handles the file/registry steps,
; but it has no knowledge of our hardware access stack (PawnIO kernel
; driver + HypercolorSmBus broker service + Windows Firewall exception)
; and no concept of cleaning that stack up on uninstall. These hooks
; fill that gap so a fresh install lands ready to drive RGB hardware
; without the user clicking through additional consent flows, and an
; uninstall doesn't leave orphan service entries or firewall rules.
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
  DetailPrint "Installing PawnIO hardware access (this may take a moment)..."
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\tools\install-bundled-pawnio.ps1" -AssetRoot "$INSTDIR\tools\pawnio" -Silent'
  Pop $0
  DetailPrint "  PawnIO install exit code: $0"

  ; HypercolorSmBus broker service — runs as LocalSystem, owns the
  ; pawnio_open() handle so the daemon (user-mode) can do SMBus / MSR
  ; / SMN reads without each user needing to elevate.
  DetailPrint "Registering Hypercolor SMBus broker service..."
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\tools\install-windows-smbus-service.ps1" -BrokerExe "$INSTDIR\tools\hypercolor-smbus-service.exe" -Reinstall -Start'
  Pop $0
  DetailPrint "  Broker install exit code: $0"

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
