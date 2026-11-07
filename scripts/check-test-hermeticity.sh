#!/usr/bin/env bash
# Keep test code off the host: no unscoped mDNS daemon, no LAN scanner with
# discovery left on, and no host USB, SMBus, or HID enumeration.
#
# A daemon cannot boot without choosing its HostPorts. This check covers
# what the type system cannot: test code choosing the native ports, and
# code that builds scanners and mDNS daemons directly, in integration test
# files and inline cfg(test) items in every crate. Each construction is
# judged on its own statement, so a loopback scope elsewhere in the file
# never excuses an unscoped one.
set -euo pipefail

repo_root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec python3 -B - "$repo_root" "$script_dir/lib" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
sys.path.insert(0, sys.argv[2])
from rust_source import RustFile, closing, rust_files  # noqa: E402

# Constructions with no hermetic form: tests use the named seam instead.
FORBIDDEN = {
    "HostPorts::native": "composes the real host; use HostPorts::isolated(&claim)",
    "WledScanner::new": "browses mDNS; use with_known_ips(.., None, ..)",
    "WledScanner::with_timeout": "browses mDNS; use with_known_ips(.., None, ..)",
    "WledScanner::default": "browses mDNS; use with_known_ips(.., None, ..)",
    "UsbScanner::new": "enumerates the host USB bus",
    "SmBusScanner::new": "opens host SMBus adapters; use SmBusScanner::with_dev_root",
    "GoveeLanScanner::new": "multicasts a Govee scan onto the LAN",
    "HidUsageIndex::enumerate": "enumerates the host HID stack; use HidUsageIndex::from_enumeration",
    "enumerate_usb_hid_collections": "enumerates the host HID stack",
    "HidApi::new": "opens the host HID stack",
    "MdnsBrowser::new": "browses every interface; use MdnsBrowser::with_scope(MdnsScope::Loopback)",
    "HueScanner::new": "browses mDNS and asks the Hue cloud; use with_options(.., false, ..) and with_nupnp_url",
    "NanoleafScanner::new": "browses mDNS; use NanoleafScanner::with_options(.., false)",
    "NanoleafScanner::with_known_devices": "browses mDNS; use NanoleafScanner::with_options(.., false)",
    "NanoleafScanner::with_known_ips": "browses mDNS; use NanoleafScanner::with_options(.., false)",
}
# Constructions whose mDNS argument, by index, must keep the browse off the
# LAN: one of the listed values.
MDNS_ARGUMENT = {
    "WledScanner::with_known_ips": (1, {"None", "Some(MdnsScope::Loopback)"}),
    "WledScanner::with_known_targets": (1, {"None", "Some(MdnsScope::Loopback)"}),
    "NanoleafScanner::with_options": (3, {"false"}),
    "HueScanner::with_options": (3, {"false"}),
}
NAMES = sorted(
    set(FORBIDDEN) | set(MDNS_ARGUMENT)
    | {"ServiceDaemon::new", "ServiceDaemon::new_with_port", "MdnsBrowser::with_scope"},
    key=len,
    reverse=True,
)
CONSTRUCTION = re.compile(r"(?<!\w)(" + "|".join(re.escape(n) for n in NAMES) + r")\s*\(")


def statement(masked, start):
    """The expression statement a construction starts, through its chain."""
    depth = 0
    for j in range(start, len(masked)):
        ch = masked[j]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
            if depth < 0:
                return masked[start:j]
        elif ch in ";," and depth == 0:
            return masked[start:j]
    return masked[start:]


def arguments(masked, open_paren):
    end = closing(masked, open_paren)
    args, depth, current = [], 0, []
    for ch in masked[open_paren + 1 : end]:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            args.append("".join(current).strip())
            current = []
        else:
            current.append(ch)
    args.append("".join(current).strip())
    return args


def compact(text):
    return re.sub(r"\s+", "", text)


def binding_before(masked, start):
    line_start = masked.rfind(";", 0, start)
    line_start = max(line_start, masked.rfind("{", 0, start)) + 1
    found = re.search(r"\blet\s+(?:mut\s+)?(\w+)\s*(?::[^=]+)?=\s*$", masked[line_start:start])
    return found.group(1) if found else None


def enclosing_block_end(masked, offset):
    depth = 0
    for j in range(offset - 1, -1, -1):
        if masked[j] == "}":
            depth += 1
        elif masked[j] == "{":
            if depth == 0:
                return closing(masked, j)
            depth -= 1
    return len(masked)


def problem(rust, match):
    name = match.group(1)
    masked = rust.masked
    open_paren = match.end() - 1
    chained = compact(statement(masked, match.start()))
    if name in FORBIDDEN:
        return FORBIDDEN[name]
    if name in MDNS_ARGUMENT:
        index, allowed = MDNS_ARGUMENT[name]
        args = arguments(masked, open_paren)
        if len(args) <= index or compact(args[index]) not in allowed:
            return "browses mDNS; pass " + " or ".join(sorted(allowed))
        if name == "HueScanner::with_options" and ".with_nupnp_url(" not in chained:
            return "asks the Hue cloud when no bridge answers; chain .with_nupnp_url(<loopback>)"
        return None
    if name == "MdnsBrowser::with_scope":
        args = arguments(masked, open_paren)
        return None if compact(args[0]) == "MdnsScope::Loopback" else "browses beyond loopback"
    # ServiceDaemon: restricting this binding must be the first thing that
    # happens to it, in the block that owns it.
    binding = binding_before(masked, match.start())
    if binding is None:
        return "bind the daemon and call MdnsScope::Loopback.restrict(&daemon)"
    start = match.start() + len(statement(masked, match.start()))
    block = masked[start : enclosing_block_end(masked, match.start())]
    restriction = re.search(
        rf"MdnsScope::Loopback\s*\.\s*restrict\s*\(\s*&\s*({binding})\b", block
    )
    if restriction is None:
        return f"never calls MdnsScope::Loopback.restrict(&{binding})"
    first_use = re.search(rf"\b{binding}\b", block)
    if first_use.start() != restriction.start(1):
        return f"uses {binding} before MdnsScope::Loopback.restrict(&{binding})"
    return None


failures = []
try:
    files = rust_files(root, ["crates"])
except FileNotFoundError as error:
    sys.exit(f"test hermeticity check failed: {error}")
for path in files:
    if "crates/hypercolor-ui/" in path.as_posix():
        continue
    try:
        # Most files name no construction at all; skip masking them.
        if not any(name in path.read_text(encoding="utf-8") for name in NAMES):
            continue
        rust = RustFile.load(root, path)
    except (OSError, UnicodeDecodeError, ValueError) as error:
        sys.exit(f"test hermeticity check failed: cannot scan {path}: {error}")
    for match in CONSTRUCTION.finditer(rust.masked):
        if not rust.in_test_code(match.start()):
            continue
        reason = problem(rust, match)
        if reason:
            failures.append(
                f"{rust.relative}:{rust.line_of(match.start())}: {match.group(1)} {reason}\n"
                f"    {rust.line_text(match.start())}"
            )

if failures:
    print("test code reaches the host:", file=sys.stderr)
    for failure in failures:
        print(f"  {failure}", file=sys.stderr)
    sys.exit(1)
print("test hermeticity: no test code reaches host hardware or the LAN")
PY
