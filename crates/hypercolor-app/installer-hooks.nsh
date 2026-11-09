; NSIS installer hooks for Hypercolor's Windows bundle.
;
; Tauri's templated NSIS installer handles the file/registry steps,
; but it has no knowledge of a release unit or of our hardware access stack
; (PawnIO kernel driver + SMBus broker service), and no concept of cleaning
; that stack up on uninstall. These hooks fill that gap.
;
; Wired in via bundle.windows.nsis.installerHooks in
; tauri.windows.bundle.conf.json. The installer runs elevated
; (installMode = perMachine), so sc.exe / netsh / PawnIO_setup.exe
; and the unit's CLI all inherit the rights they need.

; This file's directory at compile time, for the scripts the hooks embed.
; NSIS caps every string at 1024 characters, command lines included, so
; logic longer than a one-liner ships as a script file rather than inline.
!define HYPERCOLOR_HOOKS_DIR "${__FILEDIR__}"

; What scripts/produce-windows-unit.ps1 leaves for this build: the unit tree
; and its digest. A missing digest stops makensis, so no installer is ever
; built around a unit nobody produced.
!define HYPERCOLOR_STAGE_DIR "${HYPERCOLOR_HOOKS_DIR}\..\..\target\bundle-stage"
!define /file HYPERCOLOR_UNIT "${HYPERCOLOR_STAGE_DIR}\windows-unit.digest"

; The desktop app, at the root of every unit and of the flat layout.
!define HYPERCOLOR_APP "hypercolor-app.exe"

; Publish the unit this installer carries and switch the machine to it. Where
; the files are unpacked does not matter: the CLI proves every byte against
; the digest compiled in here as it copies them into the store. A failure
; leaves what ran before in place, and the next installer finishes whatever a
; kill interrupted.
!macro HYPERCOLOR_INSTALL_UNIT
  DetailPrint "Unpacking Hypercolor ${VERSION}"
  SetOutPath "$PLUGINSDIR\unit"
  File /r "${HYPERCOLOR_STAGE_DIR}\windows-unit\*"
  SetOutPath "$PLUGINSDIR"
  DetailPrint "Installing Hypercolor ${VERSION}"
  nsExec::ExecToLog '"$PLUGINSDIR\unit\hypercolor.exe" __install-release --expected-manifest-sha256 ${HYPERCOLOR_UNIT} --install-dir "$INSTDIR"'
  Pop $0
  ${If} $0 != 0
    DetailPrint "  Installing failed (exit code $0). Run this installer again to finish."
    MessageBox MB_OK|MB_ICONSTOP "Hypercolor could not be installed (exit code $0). Close anything running Hypercolor, then run this installer again." /SD IDOK
    Abort "Hypercolor could not be installed."
  ${EndIf}
!macroend

; Take the old flat layout off the disk, last of all: until this runs, a
; killed installer leaves a Hypercolor that still starts. Every name here is
; absent on a fresh install, and so is every effect.
!macro HYPERCOLOR_RETIRE_FLAT_LAYOUT
  ; A daemon service names a program that does not outlive this install, and
  ; the app supervises the daemon from now on. It stays registered until here,
  ; so an install that failed leaves it to come back at the next boot.
  ${If} ${FileExists} "$PLUGINSDIR\hypercolor-daemon-services.ps1"
    nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\hypercolor-daemon-services.ps1" -Action Remove -InstallDir "$INSTDIR"'
    Pop $0
  ${EndIf}
  ; The app's autostart value named the flat program, which is about to go.
  ; Whoever elevated this may not be whoever sits at the desktop, so every
  ; signed-in account moves here. An account that is not signed in has no
  ; hive to reach, and the app moves its value when it next starts.
  StrCpy $R8 0
  ${Do}
    EnumRegKey $R9 HKU "" $R8
    ${IfThen} $R9 == "" ${|} ${Break} ${|}
    IntOp $R8 $R8 + 1
    ReadRegStr $0 HKU "$R9\Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
    ${If} $0 != ""
      WriteRegStr HKU "$R9\Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}" '"$INSTDIR\${MAINBINARYNAME}.exe" --minimized'
    ${EndIf}
  ${Loop}
  Delete "$INSTDIR\${HYPERCOLOR_APP}"
  Delete "$INSTDIR\hypercolor-daemon.exe"
  Delete "$INSTDIR\libEGL.dll"
  Delete "$INSTDIR\libGLESv2.dll"
  Delete "$INSTDIR\msvcp140.dll"
  Delete "$INSTDIR\vcruntime140.dll"
  Delete "$INSTDIR\vcruntime140_1.dll"
  RMDir /r "$INSTDIR\ui"
  RMDir /r "$INSTDIR\effects"
  RMDir /r "$INSTDIR\tools"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Hardware access stack: PawnIO kernel driver plus the HypercolorSmBus
  ; broker, installed in one elevated pass. The orchestrator installs
  ; PawnIO (short-circuiting when already present), copies the verified
  ; module blobs into PawnIO's install dir, then registers and starts the
  ; broker. The broker runs from the unit, which already holds the modules
  ; it loads, and nothing may write inside a unit.
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
  ; sits under $INSTDIR, which satisfies the broker installer's own rejection
  ; of user-writable service paths.
  ;
  ; -ReinstallService keeps upgrades idempotent: the broker installer
  ; refuses to clobber an existing registration without it.
  ;
  ; The bundled script propagates Windows installer exit code 3010
  ; ("reboot required") when the kernel driver needs a restart to finish
  ; binding to SCM. We stash that into $R0 so we can prompt the user to
  ; restart at the end of postinstall.
  ;
  ; The installer is a 32-bit process, so a bare powershell.exe resolves
  ; through WOW64 to the 32-bit PowerShell, where $env:ProgramFiles names
  ; "Program Files (x86)" and the 64-bit PawnIO install is invisible.
  ; Turning file system redirection off for these calls runs the native
  ; 64-bit PowerShell instead, the same one the app's Install support
  ; button and a manual elevated run use.
  ; The broker and this setup log to the machine log directory, outside the
  ; installed release, which the messages below point at.
  ReadEnvStr $R2 "ProgramData"

  DetailPrint "Installing Hypercolor hardware access (this may take a moment)..."
  ${DisableX64FSRedirection}
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\units\${HYPERCOLOR_UNIT}\tools\install-windows-hardware-support.ps1" -AssetRoot "$INSTDIR\units\${HYPERCOLOR_UNIT}\tools\pawnio" -BrokerExe "$INSTDIR\units\${HYPERCOLOR_UNIT}\tools\hypercolor-smbus-service.exe" -NoUserModules -Silent -ReinstallService'
  Pop $R0
  DetailPrint "  Hardware access exit code: $R0"

  ; Whatever the exit code says, the broker either runs now or it does
  ; not, and only a running broker lights SMBus devices. Ask the service
  ; manager directly so every failure path (PawnIO setup, registration,
  ; a broker that starts and stops) lands here.
  nsExec::ExecToLog `powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -Command "$$s = Get-Service -Name HypercolorSmBus -ErrorAction SilentlyContinue; if ($$s -and $$s.Status -eq 'Running') { exit 0 }; if ($$s) { Write-Output ('HypercolorSmBus is ' + $$s.Status) } else { Write-Output 'HypercolorSmBus is not registered' }; exit 1"`
  Pop $R1
  ${EnableX64FSRedirection}

  ; A failed hardware-access pass must not fail the install, since
  ; Hypercolor still drives every USB and network device without it. It
  ; must not be silent either: the details pane closes with the wizard, so
  ; tell the user, and point at the setup log that outlives it. A silent
  ; install (/S) answers the dialog with OK and keeps going. The checks
  ; compare strings, so nsExec's "error" (PowerShell never launched) counts
  ; as a failure instead of passing as zero.
  ${If} $R1 != 0
    DetailPrint "  The HypercolorSmBus broker is not running. USB and network"
    DetailPrint "  lighting still work; the setup log is in $R2\Hypercolor\logs."
    MessageBox MB_OK|MB_ICONEXCLAMATION "Hypercolor is installed, but its SMBus hardware broker (HypercolorSmBus) is not running, so motherboard and RAM lighting and CPU temperature are unavailable. USB and network lighting work normally.$\r$\n$\r$\nThe setup log is in $R2\Hypercolor\logs. To try again, open Hypercolor and choose Settings > Device Discovery > Windows SMBus Support > Install support, or run Start-Service HypercolorSmBus in an elevated PowerShell." /SD IDOK
  ${ElseIf} $R0 != 0
  ${AndIf} $R0 != 3010
    DetailPrint "  Hardware access setup did not complete (exit code $R0). USB and"
    DetailPrint "  network lighting still work; the setup log is in $R2\Hypercolor\logs."
    MessageBox MB_OK|MB_ICONEXCLAMATION "Hypercolor is installed, but part of its hardware access setup failed (exit code $R0). The SMBus broker is running, but motherboard and RAM lighting and CPU temperature may not work until PawnIO is set up. USB and network lighting work normally.$\r$\n$\r$\nThe setup log is in $R2\Hypercolor\logs. To try again, open Hypercolor and choose Settings > Device Discovery > Windows SMBus Support > Install support." /SD IDOK
  ${EndIf}

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

; Stop a service and wait until it has exited. A LocalSystem service runs from
; the install directory and holds its exe and whatever loaded beside it open,
; so files can only be replaced or deleted once the process is gone.
; `sc.exe stop` returns as soon as the stop is requested, which races the file
; operations that follow, and Stop-Service waits without a bound while the
; service is stopping, so the stop goes through the controller with a bounded
; wait. The wait never fails the installer: an absent service is the normal
; state on a first install.
!macro HYPERCOLOR_STOP_SERVICE NAME
  DetailPrint "Stopping ${NAME} service"
  nsExec::ExecToLog `powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -Command "$$service = Get-Service -Name ${NAME} -ErrorAction SilentlyContinue; if ($$service -and $$service.Status -ne 'Stopped') { try { if ($$service.Status -ne 'StopPending') { $$service.Stop() }; $$service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(20)) } catch { Write-Output ('${NAME} did not stop: ' + $$_.Exception.GetBaseException().Message) } }"`
  Pop $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; The broker runs from the release this install replaces, and POSTINSTALL
  ; registers and starts it from the new one with -ReinstallService, so
  ; stopping it here costs nothing on a fresh install. The transaction
  ; refuses to leave a running broker behind it.
  !insertmacro HYPERCOLOR_STOP_SERVICE HypercolorSmBus
!macroend

; The app's daemon sidecar lives in the app's kill-on-close job object, so
; it normally exits with the app, but asynchronously. The daemon can also
; run on its own: as a Windows service registered by
; install-windows-service.ps1, or as a process the user started. Any of
; those still running from $INSTDIR would keep the transaction from starting.
; installer-daemon-services.ps1 stops them, bounding every wait.
!macro HYPERCOLOR_STOP_DAEMON
  DetailPrint "Stopping any Hypercolor daemon running from $INSTDIR"
  File "/oname=$PLUGINSDIR\hypercolor-daemon-services.ps1" "${HYPERCOLOR_HOOKS_DIR}\installer-daemon-services.ps1"
  nsExec::ExecToLog 'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\hypercolor-daemon-services.ps1" -Action Stop -InstallDir "$INSTDIR"'
  Pop $0
!macroend

; Runs once the app is closed. Only a directory that already holds a
; Hypercolor install can hold a daemon to stop.
!macro HYPERCOLOR_HOOK_BEFORE_FILES
  ${If} ${FileExists} "$INSTDIR\uninstall.exe"
    !insertmacro HYPERCOLOR_STOP_DAEMON
  ${EndIf}
!macroend

; Have the broker of any installed unit remove the machine directory. Every
; user may create folders under ProgramData, so this elevated uninstaller
; never deletes there by path: the broker, stopped by now and still on disk,
; removes it through handles that refuse links, and leaves alone one a
; standard user could have prepared. A name starting with a dot is an
; interrupted copy, which may hold no whole program.
!macro HYPERCOLOR_REMOVE_MACHINE_DIRECTORY
  DetailPrint "Removing Hypercolor's logs and release records"
  ClearErrors
  FindFirst $R8 $R9 "$INSTDIR\units\*"
  ${DoUntil} ${Errors}
    StrCpy $R7 $R9 1
    ${If} $R7 != "."
    ${AndIf} ${FileExists} "$INSTDIR\units\$R9\tools\hypercolor-smbus-service.exe"
      nsExec::ExecToLog '"$INSTDIR\units\$R9\tools\hypercolor-smbus-service.exe" --remove-machine-directory'
      Pop $0
      ${Break}
    ${EndIf}
    ClearErrors
    FindNext $R8 $R9
  ${Loop}
  FindClose $R8
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop + delete both services. NSIS runs the uninstaller elevated, so
  ; sc.exe inherits the necessary rights. nsExec::ExecToLog silently
  ; swallows missing-service failures — we never want an absent service to
  ; block uninstall on retried runs, and an open-source build has no update
  ; service at all.
  !insertmacro HYPERCOLOR_STOP_SERVICE HypercolorUpdate
  DetailPrint "Removing HypercolorUpdate service registration"
  nsExec::ExecToLog 'sc.exe delete HypercolorUpdate'
  Pop $0
  !insertmacro HYPERCOLOR_STOP_SERVICE HypercolorSmBus
  DetailPrint "Removing HypercolorSmBus service registration"
  nsExec::ExecToLog 'sc.exe delete HypercolorSmBus'
  Pop $0

  !insertmacro HYPERCOLOR_REMOVE_MACHINE_DIRECTORY

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
