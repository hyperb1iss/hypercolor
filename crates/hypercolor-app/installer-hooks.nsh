; NSIS installer hooks for Hypercolor's Windows bundle.
;
; Tauri's templated NSIS installer handles the file/registry steps,
; but it has no knowledge of our hardware access stack (PawnIO kernel
; driver + SMBus broker service + Windows Firewall exception), and no
; concept of cleaning that stack up on uninstall. These hooks fill
; that gap.
;
; Wired in via bundle.windows.nsis.installerHooks in
; tauri.windows.bundle.conf.json. The installer runs elevated
; (installMode = perMachine), so sc.exe / netsh / PawnIO_setup.exe
; all inherit the rights they need.

!macro NSIS_HOOK_POSTINSTALL
  ; Hardware access stack: PawnIO kernel driver plus the HypercolorSmBus
  ; broker, installed in one elevated pass. The orchestrator installs
  ; PawnIO (short-circuiting when already present), copies the verified
  ; module blobs into PawnIO's install dir, then registers and starts the
  ; broker.
  ;
  ; Both halves are load-bearing. The broker is what loads PawnIO modules
  ; on behalf of the unelevated daemon, so skipping it leaves CPU package
  ; temperature and motherboard/DRAM SMBus lighting permanently dark with
  ; no error the user can act on.
  ;
  ; Install time is the only moment Hypercolor holds administrator rights.
  ; An unelevated app cannot register a LocalSystem service, so deferring
  ; this means either an out-of-nowhere UAC prompt later or — as shipped in
  ; 0.2.1 — silently broken hardware. Every path handed to the orchestrator
  ; sits under $INSTDIR (Program Files under perMachine), which satisfies
  ; the broker installer's own rejection of user-writable service paths.
  ;
  ; -ReinstallService keeps upgrades idempotent: the broker installer
  ; refuses to clobber an existing registration without it.
  ;
  ; The bundled script propagates Windows installer exit code 3010
  ; ("reboot required") when the kernel driver needs a restart to finish
  ; binding to SCM. We stash that into $R0 so we can prompt the user to
  ; restart at the end of postinstall.
  DetailPrint "Installing Hypercolor hardware access (this may take a moment)..."
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\tools\install-windows-hardware-support.ps1" -AssetRoot "$INSTDIR\tools\pawnio" -BrokerExe "$INSTDIR\tools\hypercolor-smbus-service.exe" -ModuleDestination "$INSTDIR\tools\pawnio\modules" -Silent -ReinstallService'
  Pop $R0
  DetailPrint "  Hardware access exit code: $R0"

  ; A failed hardware-access pass must not fail the install — Hypercolor
  ; still drives every USB and network device without it. Say so plainly
  ; in the details log so a support request has something to quote.
  ${If} $R0 <> 0
  ${AndIf} $R0 <> 3010
    DetailPrint "  Hardware access setup did not complete. USB and network"
    DetailPrint "  lighting still work; motherboard SMBus lighting and CPU"
    DetailPrint "  temperature need Settings > Discovery > Hardware Support."
  ${EndIf}

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
    MessageBox MB_YESNO|MB_ICONQUESTION "Hypercolor installed successfully, but the PawnIO kernel driver needs a Windows restart before motherboard lighting and CPU temperature can come online. Restart now?" IDNO no_reboot_now
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
