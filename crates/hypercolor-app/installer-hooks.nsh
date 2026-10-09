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

; This file's directory at compile time, for the scripts the hooks embed.
; NSIS caps every string at 1024 characters, command lines included, so
; logic longer than a one-liner ships as a script file rather than inline.
!define HYPERCOLOR_HOOKS_DIR "${__FILEDIR__}"

; Arguments to the daemon services script, double-quoted so no path (a
; profile named O'Brien, say) can break them.
!define HYPERCOLOR_DAEMON_SERVICES_ARGS `-InstallDir "$INSTDIR" -StateFile "$PLUGINSDIR\hypercolor-daemon-services.txt"`

; Start exactly the daemon services HYPERCOLOR_STOP_DAEMON stopped for the
; file copy. A fresh install never stopped any, so there is no list.
!macro HYPERCOLOR_RESTORE_DAEMON_SERVICES
  ${If} ${FileExists} "$PLUGINSDIR\hypercolor-daemon-services.txt"
    nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\hypercolor-daemon-services.ps1" -Action Restore ${HYPERCOLOR_DAEMON_SERVICES_ARGS}'
    Pop $0
  ${EndIf}
!macroend

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

  !insertmacro HYPERCOLOR_RESTORE_DAEMON_SERVICES

  ; If PawnIO asked for a reboot, surface it. The MUI2 finish page
  ; doesn't natively expose a reboot prompt for installer-driven
  ; restarts, so a simple MessageBox keeps the user informed instead
  ; of letting them launch Hypercolor into a broken hardware-access
  ; state. A silent install (/S, as an automated updater runs it)
  ; answers No: it must never block on a dialog or reboot the machine.
  ; Either way the installer then exits with 3010, the Windows code for
  ; "installed, restart required", so a caller can ask for the restart.
  ${If} $R0 = 3010
    MessageBox MB_YESNO|MB_ICONQUESTION "Hypercolor installed successfully, but the PawnIO kernel driver needs a Windows restart before motherboard lighting and CPU temperature can come online. Restart now?" /SD IDNO IDNO no_reboot_now
      Reboot
    no_reboot_now:
    SetErrorLevel 3010
  ${EndIf}
!macroend

; Stop the HypercolorSmBus broker and wait until it has exited. The
; LocalSystem service runs from $INSTDIR\tools and holds its exe and the
; app-local VC++ runtime beside it open, so files can only be replaced or
; deleted once the process is gone. `sc.exe stop` returns as soon as the
; stop is requested, which races the file operations that follow. The
; wait is bounded and never fails the installer: an absent service is
; the normal state on a first install.
!macro HYPERCOLOR_STOP_BROKER
  DetailPrint "Stopping HypercolorSmBus service"
  nsExec::ExecToLog `powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -Command "$$service = Get-Service -Name HypercolorSmBus -ErrorAction SilentlyContinue; if ($$service -and $$service.Status -ne 'Stopped') { Stop-Service -Name HypercolorSmBus -Force -ErrorAction SilentlyContinue; try { $$service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(20)) } catch { Write-Output 'HypercolorSmBus did not stop within 20 seconds' } }"`
  Pop $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Upgrades write over a running broker's files. POSTINSTALL re-registers
  ; and starts the service with -ReinstallService, so stopping it here
  ; costs nothing on a fresh install and unblocks every upgrade.
  !insertmacro HYPERCOLOR_STOP_BROKER
!macroend

; The app's daemon sidecar lives in the app's kill-on-close job object, so
; it normally exits with the app, but asynchronously. The daemon can also
; run on its own: as a Windows service registered by
; install-windows-service.ps1, or as a process the user started. Any of
; those still running from $INSTDIR would hold hypercolor-daemon.exe open
; and fail the copy. installer-daemon-services.ps1 stops them, bounding
; every wait, and records the services it stopped so
; HYPERCOLOR_RESTORE_DAEMON_SERVICES can start exactly those again.
!macro HYPERCOLOR_STOP_DAEMON
  DetailPrint "Stopping any Hypercolor daemon running from $INSTDIR"
  File "/oname=$PLUGINSDIR\hypercolor-daemon-services.ps1" "${HYPERCOLOR_HOOKS_DIR}\installer-daemon-services.ps1"
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\hypercolor-daemon-services.ps1" -Action Stop ${HYPERCOLOR_DAEMON_SERVICES_ARGS}'
  Pop $0
!macroend

; Upgrades install in place, and copying files only ever overwrites. The UI
; and bundled effects are rebuilt in full by every install, but their file
; names change between releases (trunk names UI assets by content hash, and
; effects come and go), so without this each upgrade left the previous
; release's files behind, the uninstaller never removed them, and dropped
; effects kept appearing in the library. installer.nsi runs this hook once
; the app is closed; the guard only clears folders inside a directory that
; already holds a Hypercolor install.
!macro HYPERCOLOR_HOOK_BEFORE_FILES
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
  ${AndIf} ${FileExists} "$INSTDIR\uninstall.exe"
    !insertmacro HYPERCOLOR_STOP_DAEMON
    DetailPrint "Removing the previous release's UI and bundled effects"
    RMDir /r "$INSTDIR\ui"
    RMDir /r "$INSTDIR\effects\bundled"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop + delete the HypercolorSmBus broker service. NSIS runs the
  ; uninstaller elevated, so sc.exe inherits the necessary rights.
  ; nsExec::ExecToLog silently swallows missing-service failures — we
  ; never want an absent service to block uninstall on retried runs.
  !insertmacro HYPERCOLOR_STOP_BROKER

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
