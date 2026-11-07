#!/bin/bash
# Throwaway: what real launchd and a hosted Mac actually do.
set -u
uid=$(id -u); domain="gui/$uid"
L=probe.tech.hyperbliss
A="$HOME/Library/LaunchAgents"; mkdir -p "$A"
W="$(mktemp -d)"
section() { printf '\n=== %s ===\n' "$*"; }
run() { printf '$ %s\n' "$*"; "$@"; printf '[exit %s]\n' "$?"; }

section environment
sw_vers; uname -a; id; run launchctl managername; run launchctl print "$domain" | head -30
run csrutil status; run sudo -n true; ls -ld /Applications; df -h / /Applications ~ | head
who; ps -axo pid,ppid,user,comm | grep -E 'WindowServer|loginwindow|Dock|Finder|Runner.Listener|hosted-compute|launchd$' | head -20
system_profiler SPDisplaysDataType 2>/dev/null | head -30
sysctl -n hw.ncpu hw.memsize machdep.cpu.brand_string
python3 -c 'import select,sys;print(sys.version,hasattr(select,"kqueue"))'
which node pwsh brew zstd openssl hdiutil codesign security clang; /usr/bin/python3 --version
xcode-select -p; ls -d /Applications/Xcode* | head; xcrun --show-sdk-version

plist() { # label, seconds?, program...
  local label=$1 extra=$2; shift 2
  { printf '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>Label</key><string>%s</string><key>ProgramArguments</key><array>' "$label"
    for a in "$@"; do printf '<string>%s</string>' "$a"; done
    printf '</array>%s<key>StandardOutPath</key><string>%s/%s.out</string><key>StandardErrorPath</key><string>%s/%s.out</string></dict></plist>\n' "$extra" "$W" "$label" "$W" "$label"
  } > "$A/$label.plist"
}
trim() { grep -E '^\s*(state|runs|pid|last exit code|last exit reason|last terminating signal|active count|immediate reason|program|path) =|^\s*(spawn type|forks|execs|exit timeout|last exit|event triggers|needs LWCR|endpoints|inherited)' ; }

section 'kickstart and exit statuses'
for code in 0 1 73 75 76 78; do
  label="$L.exit$code"
  plist "$label" '' /bin/sh -c "sleep 0.3; exit $code"
  run launchctl bootstrap "$domain" "$A/$label.plist"
  echo "-- before any run:"; launchctl print "$domain/$label" | trim
  echo "-- kickstart -p:"
  t0=$(python3 -c 'import time;print(time.time())')
  out=$(launchctl kickstart -p "$domain/$label" 2>&1); rc=$?
  t1=$(python3 -c 'import time;print(time.time())')
  printf 'stdout+stderr=[%q] rc=%s took=%.3f\n' "$out" "$rc" "$(python3 -c "print($t1-$t0)")"
  sleep 1
  echo "-- print after exit:"; launchctl print "$domain/$label" | trim
  done

section 'full print of an exited job'
launchctl print "$domain/$L.exit75"

section 'signals'
label="$L.sig"
plist "$label" '' /bin/sh -c 'kill -9 $$'
launchctl bootstrap "$domain" "$A/$label.plist"; launchctl kickstart -p "$domain/$label"; sleep 1
launchctl print "$domain/$label" | trim
label="$L.term"
plist "$label" '' /bin/sh -c 'kill -TERM $$'
launchctl bootstrap "$domain" "$A/$label.plist"; launchctl kickstart -p "$domain/$label"; sleep 1
launchctl print "$domain/$label" | trim

section 'kickstart of a running job, then -k, then a kill by launchctl'
label="$L.long"
plist "$label" '' /bin/sleep 30
launchctl bootstrap "$domain" "$A/$label.plist"
run launchctl kickstart -p "$domain/$label"
run launchctl kickstart -p "$domain/$label"
launchctl print "$domain/$label" | trim
run launchctl kickstart -k -p "$domain/$label"
sleep 0.5; launchctl print "$domain/$label" | trim
run launchctl kill SIGTERM "$domain/$label"
sleep 0.5; launchctl print "$domain/$label" | trim

section 'missing program'
label="$L.missing"
plist "$label" '' /nonexistent/program --flag
run launchctl bootstrap "$domain" "$A/$label.plist"
run launchctl kickstart -p "$domain/$label"
sleep 1; launchctl print "$domain/$label" | trim

section 'print and bootout of an unloaded job'
run launchctl print "$domain/$L.never"
run launchctl bootout "$domain/$L.never"
run launchctl kickstart -p "$domain/$L.never"
run launchctl bootout "$domain/$L.exit0"
run launchctl print "$domain/$L.exit0"

section 'disable and print-disabled'
run launchctl disable "$domain/$L.exit1"
launchctl print-disabled "$domain" | grep -E "$L|disabled|enabled" | head
run launchctl kickstart -p "$domain/$L.exit1"
run launchctl enable "$domain/$L.exit1"
launchctl print-disabled "$domain" | grep -E "$L" | head

section 'KeepAlive PathState'
label="$L.path"; marker="$W/pending"
plist "$label" "<key>KeepAlive</key><dict><key>PathState</key><dict><key>$marker</key><true/></dict></dict><key>ThrottleInterval</key><integer>4</integer>" /bin/sh -c "date +%T.%N >> $W/path-runs; exit 75"
run launchctl bootstrap "$domain" "$A/$label.plist"
sleep 2; echo "-- absent marker, runs:"; cat "$W/path-runs" 2>/dev/null; launchctl print "$domain/$label" | trim
echo "-- marker up at $(date +%T)"; touch "$marker"
sleep 13; echo "-- runs:"; cat "$W/path-runs"; launchctl print "$domain/$label" | trim
launchctl print "$domain/$label" | sed -n '/event triggers/,/}/p;/spawn type/p'
rm "$marker"; echo "-- marker down at $(date +%T)"; sleep 7; echo "-- runs:"; cat "$W/path-runs"; launchctl print "$domain/$label" | trim
echo "-- a job loaded with the marker present"
run launchctl bootout "$domain/$label"; : > "$W/path-runs"; touch "$marker"
run launchctl bootstrap "$domain" "$A/$label.plist"; sleep 2; cat "$W/path-runs"; rm "$marker"; sleep 1

section 'PathState and a long run: does launchd start a second copy or wait'
label="$L.pathlong"; marker="$W/pending2"
plist "$label" "<key>KeepAlive</key><dict><key>PathState</key><dict><key>$marker</key><true/></dict></dict><key>ThrottleInterval</key><integer>2</integer>" /bin/sh -c "date +%T >> $W/pl; sleep 3; exit 1"
launchctl bootstrap "$domain" "$A/$label.plist"; touch "$marker"; sleep 9; cat "$W/pl"; launchctl print "$domain/$label" | trim; rm "$marker"; sleep 4

section 'test root signing'
cd "$W"
team=TESTTEAM01
cat > root.cnf <<CNF
[req]
distinguished_name=dn
x509_extensions=v3
prompt=no
[dn]
CN=Hypercolor Qualification Root
O=Hypercolor Qualification
[v3]
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
CNF
cat > leaf.cnf <<CNF
[req]
distinguished_name=dn
prompt=no
[dn]
CN=Developer ID Application: Hypercolor Qualification ($team)
OU=$team
O=Hypercolor Qualification
[v3]
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=critical,codeSigning
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid
CNF
openssl version
openssl req -x509 -newkey rsa:2048 -nodes -keyout root.key -out root.pem -days 30 -config root.cnf 2>/dev/null
openssl req -new -newkey rsa:2048 -nodes -keyout leaf.key -out leaf.csr -config leaf.cnf 2>/dev/null
openssl x509 -req -in leaf.csr -CA root.pem -CAkey root.key -CAcreateserial -out leaf.pem -days 30 -extfile leaf.cnf -extensions v3 2>/dev/null
anchor=$(openssl x509 -in root.pem -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')
echo "anchor $anchor"
openssl pkcs12 -export -legacy -inkey leaf.key -in leaf.pem -certfile root.pem -out leaf.p12 -passout pass:qual 2>&1 | head -3 || true
ls -la leaf.p12
kc="$W/qual.keychain-db"
run security create-keychain -p qual "$kc"
run security set-keychain-settings "$kc"
run security unlock-keychain -p qual "$kc"
run security import leaf.p12 -k "$kc" -P qual -T /usr/bin/codesign -T /usr/bin/security
run security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k qual "$kc" >/dev/null
run security list-keychains -d user -s "$kc" login.keychain-db
run security find-identity -p codesigning "$kc"
run security find-identity -v -p codesigning "$kc"
sha=$(openssl x509 -in leaf.pem -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')
cp /bin/ls ./ls.copy 2>/dev/null
cat > hello.c <<'C'
#include <stdio.h>
#include <unistd.h>
int main(void){printf("hello %d\n",getpid());return 0;}
C
clang -o hello hello.c
run codesign --force --sign "$sha" --keychain "$kc" --identifier tech.hyperbliss.hypercolor.cli --options runtime --timestamp=none hello
run codesign -dvvv hello
run codesign -dr - hello
run codesign --verify --verbose=4 hello
run codesign --verify --verbose=4 -R="certificate root = H\"$anchor\" and certificate leaf[subject.OU] = \"$team\" and identifier \"tech.hyperbliss.hypercolor.cli\"" hello
run codesign --verify --verbose=4 -R="certificate root = H\"0000000000000000000000000000000000000000\"" hello
run ./hello
label="$L.signed"
plist "$label" '' "$W/hello"
launchctl bootstrap "$domain" "$A/$label.plist"; run launchctl kickstart -p "$domain/$label"; sleep 1
launchctl print "$domain/$label" | trim; cat "$W/$label.out"

section 'a bundle opened through LaunchServices'
app="$W/Probe.app"; mkdir -p "$app/Contents/MacOS"
cat > "$app/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>probe.app</string><key>CFBundleExecutable</key><string>probe</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleName</key><string>Probe</string><key>CFBundleVersion</key><string>1.0</string><key>CFBundleShortVersionString</key><string>1.0</string></dict></plist>
PL
cat > appkit.swift <<'SW'
import AppKit
let app = NSApplication.shared
FileManager.default.createFile(atPath: CommandLine.arguments.count > 0 ? NSHomeDirectory() + "/probe-app-ran" : "/tmp/x", contents: "\(ProcessInfo.processInfo.processIdentifier)".data(using: .utf8))
DispatchQueue.main.asyncAfter(deadline: .now() + 3) { app.terminate(nil) }
app.setActivationPolicy(.regular)
let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 200, height: 200), styleMask: [.titled], backing: .buffered, defer: false)
w.makeKeyAndOrderFront(nil)
app.run()
SW
swiftc -o "$app/Contents/MacOS/probe" appkit.swift 2>&1 | tail -3
run codesign --force --deep --sign "$sha" --keychain "$kc" --options runtime --timestamp=none "$app"
cp -R "$app" /Applications/Probe.app && ls -la@ /Applications/Probe.app/Contents | head
xattr -lr /Applications/Probe.app | head
run open /Applications/Probe.app
sleep 2; cat ~/probe-app-ran 2>/dev/null; echo; ps -axo pid,ppid,user,comm | grep -i probe | grep -v grep | head; lsappinfo list 2>/dev/null | grep -A3 -i probe | head -8
sleep 3; rm -f ~/probe-app-ran
run spctl --assess --type execute -vv /Applications/Probe.app

section 'RENAME_SWAP in Applications, from an agent and from a step'
mkdir -p /Applications/.probe-update/x && cp -R /Applications/Probe.app /Applications/.probe-update/x/Probe.app
cat > swap.py <<'PY'
import ctypes,sys
libc=ctypes.CDLL(None,use_errno=True)
RENAME_SWAP=2
r=libc.renamex_np(sys.argv[1].encode(),sys.argv[2].encode(),RENAME_SWAP)
print("renamex_np",r,ctypes.get_errno())
PY
run python3 swap.py /Applications/Probe.app /Applications/.probe-update/x/Probe.app
label="$L.swap"
plist "$label" '' /usr/bin/python3 "$W/swap.py" /Applications/Probe.app /Applications/.probe-update/x/Probe.app
launchctl bootstrap "$domain" "$A/$label.plist"; launchctl kickstart -p "$domain/$label"; sleep 2; cat "$W/$label.out"; launchctl print "$domain/$label" | trim
rm -rf /Applications/.probe-update /Applications/Probe.app

section 'trusting a fixture CA'
cat > san.cnf <<CNF
[req]
distinguished_name=dn
x509_extensions=v3
prompt=no
[dn]
CN=127.0.0.1
[v3]
subjectAltName=IP:127.0.0.1
basicConstraints=CA:TRUE
CNF
openssl req -x509 -newkey rsa:2048 -nodes -keyout tls.key -out tls.pem -days 30 -config san.cnf 2>/dev/null
run sudo -n security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain tls.pem
python3 - <<'PY' &
import http.server,ssl
s=http.server.HTTPServer(("127.0.0.1",9555),http.server.SimpleHTTPRequestHandler)
c=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);c.load_cert_chain("tls.pem","tls.key");s.socket=c.wrap_socket(s.socket,server_side=True);s.handle_request()
PY
server=$!
sleep 1; run /usr/bin/curl -sS -o /dev/null -w '%{http_code}\n' https://127.0.0.1:9555/
kill "$server" 2>/dev/null
run sudo -n security delete-certificate -c 127.0.0.1 /Library/Keychains/System.keychain

section 'a DMG'
mkdir dmgsrc && cp -R "$app" dmgsrc/ && ln -s /Applications dmgsrc/Applications
run hdiutil create -quiet -volname Probe -srcfolder dmgsrc -format UDZO -ov probe.dmg
run hdiutil attach -nobrowse -readonly -mountpoint "$W/mnt" probe.dmg
ls -la "$W/mnt"; run hdiutil detach "$W/mnt"

section 'cleanup'
for f in "$A"/probe.*.plist; do label=$(basename "$f" .plist); launchctl bootout "$domain/$label" 2>/dev/null; rm -f "$f"; done
security delete-keychain "$kc"
echo done
