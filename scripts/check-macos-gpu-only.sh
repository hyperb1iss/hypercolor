#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
forbidden='CpuReductionExecutor|MacosCpuSourceView|PreparedCpuPublicationFanout|legacy_cpu_capture_frame|native_cpu_capture_frame|publish_macos_cpu_exact|publish_macos_scalar_exact|copy_bgra8_to|with_cpu_source|new_cpu_fixture'

scan_unexpected() {
  local root="$1"
  shift
  python3 - "$root" "$forbidden" "$@" <<'PY'
import itertools
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
forbidden = set(sys.argv[2].split("|"))
fixture_features = {"capture-fixtures", "macos-capture-fixtures"}
excluded = {
    "crates/hypercolor-core/src/input/screen/macos/fixtures.rs",
    "crates/hypercolor-core/src/input/screen/macos/tests.rs",
    "crates/hypercolor-macos-capture/src/cpu.rs",
}


class Token:
    __slots__ = ("text", "start", "end", "kind")

    def __init__(self, text, start, end, kind="punct"):
        self.text = text
        self.start = start
        self.end = end
        self.kind = kind


def tokenize(source):
    tokens = []
    index = 0
    length = len(source)
    while index < length:
        char = source[index]
        if char.isspace():
            index += 1
            continue
        if source.startswith("//", index):
            newline = source.find("\n", index + 2)
            index = length if newline < 0 else newline + 1
            continue
        if source.startswith("/*", index):
            depth = 1
            index += 2
            while index < length and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            continue
        raw = re.match(r'(?:br|r)(?P<hashes>#{0,255})"', source[index:])
        if raw:
            hashes = raw.group("hashes")
            start = index
            index += raw.end()
            terminator = '"' + hashes
            end = source.find(terminator, index)
            index = length if end < 0 else end + len(terminator)
            tokens.append(Token(source[start:index], start, index, "string"))
            continue
        if char == '"' or (char == 'b' and index + 1 < length and source[index + 1] == '"'):
            start = index
            index += 2 if char == 'b' else 1
            while index < length:
                if source[index] == '\\':
                    index += 2
                elif source[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            tokens.append(Token(source[start:index], start, index, "string"))
            continue
        char_start = index + 1 if char == "'" else index + 2
        if char == "'" or (char == 'b' and index + 1 < length and source[index + 1] == "'"):
            start = index
            cursor = char_start
            if cursor < length and source[cursor] == '\\':
                cursor += 2
                if cursor <= length and source[start:cursor].endswith("\\u") and cursor < length and source[cursor] == "{":
                    closing = source.find("}", cursor + 1)
                    cursor = length if closing < 0 else closing + 1
            else:
                cursor += 1
            if cursor < length and source[cursor] == "'":
                index = cursor + 1
                tokens.append(Token(source[start:index], start, index, "char"))
                continue
        raw_identifier = re.match(r'r#(?P<name>[A-Za-z_][A-Za-z0-9_]*)', source[index:])
        if raw_identifier:
            end = index + raw_identifier.end()
            tokens.append(
                Token(raw_identifier.group("name"), index, end, "identifier")
            )
            index = end
            continue
        if char.isalpha() or char == '_':
            match = re.match(r'[A-Za-z_][A-Za-z0-9_]*', source[index:])
            end = index + len(match.group(0))
            tokens.append(Token(source[index:end], index, end, "identifier"))
            index = end
            continue
        tokens.append(Token(char, index, index + 1))
        index += 1
    return tokens


def matching(tokens, start, opening, closing):
    depth = 0
    for index in range(start, len(tokens)):
        if tokens[index].text == opening:
            depth += 1
        elif tokens[index].text == closing:
            depth -= 1
            if depth == 0:
                return index
    return None


def parse_cfg(tokens, guarded_features=fixture_features):
    cursor = 0
    atoms = []

    def expression():
        nonlocal cursor
        if cursor >= len(tokens) or tokens[cursor].kind != "identifier":
            raise ValueError
        name = tokens[cursor].text
        cursor += 1
        if cursor < len(tokens) and tokens[cursor].text == "(":
            cursor += 1
            values = []
            if cursor < len(tokens) and tokens[cursor].text != ")":
                while True:
                    values.append(expression())
                    if cursor >= len(tokens) or tokens[cursor].text != ",":
                        break
                    cursor += 1
            if cursor >= len(tokens) or tokens[cursor].text != ")":
                raise ValueError
            cursor += 1
            if name == "all":
                return lambda feature, state: all(value(feature, state) for value in values)
            if name == "any":
                return lambda feature, state: any(value(feature, state) for value in values)
            if name == "not" and len(values) == 1:
                return lambda feature, state: not values[0](feature, state)
            raise ValueError
        value = None
        if cursor < len(tokens) and tokens[cursor].text == "=":
            cursor += 1
            if cursor >= len(tokens):
                raise ValueError
            value = tokens[cursor].text.strip('"')
            cursor += 1
        if name == "feature" and value in guarded_features:
            return lambda feature, state: feature
        atom = (name, value)
        if atom not in atoms:
            atoms.append(atom)
        return lambda feature, state, atom=atom: state[atom]

    result = expression()
    if cursor != len(tokens):
        raise ValueError
    if len(atoms) > 12:
        return False
    for values in itertools.product((False, True), repeat=len(atoms)):
        if result(False, dict(zip(atoms, values))):
            return False
    return True


def cfg_implies_fixture(tokens, start, end, guarded_features=fixture_features):
    body = tokens[start + 2:end]
    if not body or body[0].text != "cfg" or len(body) < 3 or body[1].text != "(":
        return False
    close = matching(body, 1, "(", ")")
    if close != len(body) - 1:
        return False
    try:
        return parse_cfg(body[2:close], guarded_features)
    except ValueError:
        return False


def attributed_item_end(tokens, start):
    parens = brackets = braces = 0
    saw_brace = False
    for index in range(start, len(tokens)):
        text = tokens[index].text
        if text == "(":
            parens += 1
        elif text == ")":
            parens = max(0, parens - 1)
        elif text == "[":
            brackets += 1
        elif text == "]":
            brackets = max(0, brackets - 1)
        elif text == "{" and parens == 0 and brackets == 0:
            braces += 1
            saw_brace = True
        elif text == "}" and parens == 0 and brackets == 0 and saw_brace:
            braces -= 1
            if braces == 0:
                return tokens[index].end
        elif text in {";", ","} and not any((parens, brackets, braces)):
            return tokens[index].end
    return tokens[-1].end if tokens else 0


def fixture_ranges(tokens, guarded_features=fixture_features):
    ranges = []
    index = 0
    while index + 1 < len(tokens):
        if tokens[index].text != "#" or tokens[index + 1].text != "[":
            index += 1
            continue
        end = matching(tokens, index + 1, "[", "]")
        if end is None or not cfg_implies_fixture(tokens, index, end, guarded_features):
            index += 1
            continue
        item = end + 1
        while item + 1 < len(tokens) and tokens[item].text == "#" and tokens[item + 1].text == "[":
            next_end = matching(tokens, item + 1, "[", "]")
            if next_end is None:
                break
            item = next_end + 1
        if item < len(tokens):
            ranges.append((tokens[item].start, attributed_item_end(tokens, item)))
        index = end + 1
    return ranges


def declaration_is_guarded(path, feature, declaration):
    source = path.read_text(encoding="utf-8")
    tokens = tokenize(source)
    expected = tokenize(declaration)
    guarded = fixture_ranges(tokens, {feature})
    guarded_match = False
    every_match_must_be_guarded = (
        expected[0].text == "mod"
        or [token.text for token in expected[:2]] == ["pub", "use"]
    )
    for index in range(len(tokens) - len(expected) + 1):
        if any(
            actual.text != wanted.text or actual.kind != wanted.kind
            for actual, wanted in zip(tokens[index:], expected)
        ):
            continue
        is_guarded = any(start <= tokens[index].start < end for start, end in guarded)
        guarded_match = guarded_match or is_guarded
        if every_match_must_be_guarded and not is_guarded:
            return False
    return guarded_match


if len(sys.argv) > 3:
    if len(sys.argv) != 7 or sys.argv[3] != "--require":
        raise SystemExit("invalid architecture-fence parser invocation")
    relative = sys.argv[4]
    feature = sys.argv[5]
    declaration = sys.argv[6]
    path = root / relative
    if path.is_file() and declaration_is_guarded(path, feature, declaration):
        raise SystemExit(0)
    print(f"macOS GPU-only fixture boundary is missing in {relative}: {declaration}")
    raise SystemExit(1)


def string_literal_value(token):
    text = token.text
    raw = re.fullmatch(r'r(?P<hashes>#{0,255})"(?P<body>.*)"(?P=hashes)', text, re.DOTALL)
    if raw:
        return raw.group("body")
    if len(text) < 2 or text[0] != '"' or text[-1] != '"':
        return None
    body = text[1:-1]
    decoded = []
    index = 0
    simple = {
        "0": "\0",
        "t": "\t",
        "n": "\n",
        "r": "\r",
        '"': '"',
        "'": "'",
        "\\": "\\",
    }
    while index < len(body):
        if body[index] != "\\":
            decoded.append(body[index])
            index += 1
            continue
        index += 1
        if index >= len(body):
            return None
        escape = body[index]
        if escape in simple:
            decoded.append(simple[escape])
            index += 1
            continue
        if escape == "x":
            value = body[index + 1:index + 3]
            if len(value) != 2 or not re.fullmatch(r"[0-9A-Fa-f]{2}", value):
                return None
            codepoint = int(value, 16)
            if codepoint > 0x7F:
                return None
            decoded.append(chr(codepoint))
            index += 3
            continue
        if escape == "u" and index + 1 < len(body) and body[index + 1] == "{":
            closing = body.find("}", index + 2)
            if closing < 0:
                return None
            value = body[index + 2:closing].replace("_", "")
            if not 1 <= len(value) <= 6 or not re.fullmatch(r"[0-9A-Fa-f]+", value):
                return None
            codepoint = int(value, 16)
            if codepoint > 0x10FFFF or 0xD800 <= codepoint <= 0xDFFF:
                return None
            decoded.append(chr(codepoint))
            index = closing + 1
            continue
        if escape == "\n":
            index += 1
            while index < len(body) and body[index].isspace():
                index += 1
            continue
        return None
    return "".join(decoded)


def char_literal_value(token):
    text = token.text
    if len(text) < 3 or text[0] != "'" or text[-1] != "'":
        return None
    decoded = string_literal_value(Token(f'"{text[1:-1]}"', 0, 0, "string"))
    return decoded if decoded is not None and len(decoded) == 1 else None


def constant_string_expression(tokens):
    if len(tokens) == 1 and tokens[0].kind == "string":
        return string_literal_value(tokens[0])
    if len(tokens) == 1 and tokens[0].kind == "char":
        return char_literal_value(tokens[0])
    if len(tokens) == 1 and tokens[0].text in {"true", "false"}:
        return tokens[0].text
    if (
        len(tokens) >= 5
        and tokens[0].text == "concat"
        and tokens[1].text == "!"
        and tokens[2].text in {"(", "[", "{"}
    ):
        opening = tokens[2].text
        closing = {"(": ")", "[": "]", "{": "}"}[opening]
        close = matching(tokens, 2, opening, closing)
        if close != len(tokens) - 1:
            return None
        parts = []
        start = 3
        depth = 0
        for index in range(3, close + 1):
            text = tokens[index].text if index < close else ","
            if text in {"(", "[", "{"}:
                depth += 1
            elif text in {")",
                "]",
                "}",
            }:
                depth -= 1
            elif text == "," and depth == 0:
                if start == index and index == close and parts:
                    break
                part = constant_string_expression(tokens[start:index])
                if part is None:
                    return None
                parts.append(part)
                start = index + 1
        return "".join(parts)
    if (
        len(tokens) == 5
        and tokens[0].text == "stringify"
        and tokens[1].text == "!"
        and tokens[2].text in {"(", "[", "{"}
        and tokens[4].text == {"(": ")", "[": "]", "{": "}"}[tokens[2].text]
        and tokens[3].kind == "identifier"
    ):
        return tokens[3].text
    return None


def is_concat_expression(tokens):
    return (
        len(tokens) >= 4
        and tokens[0].text == "concat"
        and tokens[1].text == "!"
        and tokens[2].text in {"(", "[", "{"}
    )


def resolves_to_excluded(path, value):
    resolved = (path.parent / value).resolve(strict=False)
    try:
        relative = resolved.relative_to(root.resolve()).as_posix()
    except ValueError:
        return False
    return relative.casefold() in {candidate.casefold() for candidate in excluded}


def is_allowed_dynamic_include(path, tokens, start, end):
    relative = path.relative_to(root).as_posix()
    if relative != "crates/hypercolor-core/src/attachment/embedded.rs":
        return False
    expected = tokenize('include!(concat!(env!("OUT_DIR"), "/embedded_attachments.rs"))')
    actual = tokens[start:end + 1]
    return len(actual) == len(expected) and all(
        left.text == right.text and left.kind == right.kind
        for left, right in zip(actual, expected)
    )


def rejects_alternate_fixture_path(path, tokens, strict_unsupported):
    index = 0
    while index < len(tokens):
        if tokens[index].text == "use":
            end = index + 1
            while end < len(tokens) and tokens[end].text != ";":
                if tokens[end].text == "include":
                    return tokens[index]
                end += 1
            index = end + 1
            continue
        if tokens[index].text == "#" and index + 1 < len(tokens) and tokens[index + 1].text == "[":
            end = matching(tokens, index + 1, "[", "]")
            if end is None:
                return tokens[index]
            body = tokens[index + 2:end]
            for body_index in range(len(body) - 2):
                if body[body_index].text != "path" or body[body_index + 1].text != "=":
                    continue
                value_token = body[body_index + 2]
                value = (
                    string_literal_value(value_token)
                    if value_token.kind == "string"
                    else None
                )
                if (
                    (strict_unsupported and value is None)
                    or (value is not None and resolves_to_excluded(path, value))
                ):
                    return tokens[index]
            index = end + 1
            continue
        if (
            tokens[index].kind == "identifier"
            and index + 2 < len(tokens)
            and tokens[index + 1].text == "!"
            and tokens[index + 2].text in {"(", "[", "{"}
        ):
            opening = tokens[index + 2].text
            closing = {"(": ")", "[": "]", "{": "}"}[opening]
            end = matching(tokens, index + 2, opening, closing)
            if end is None:
                return tokens[index]
            value = constant_string_expression(tokens[index + 3:end])
            concat_expression = is_concat_expression(tokens[index + 3:end])
            allowed_dynamic = (
                value is None
                and is_allowed_dynamic_include(path, tokens, index, end)
            )
            if (
                (
                    (tokens[index].text == "include" or concat_expression)
                    and value is None
                    and not allowed_dynamic
                )
                or (value is not None and resolves_to_excluded(path, value))
            ):
                return tokens[index]
            index = end + 1
            continue
        index += 1
    return None


paths = []
facade = root / "crates/hypercolor-core/src/input/screen/macos.rs"
if facade.is_file():
    paths.append(facade)
for directory in (
    root / "crates/hypercolor-core/src/input/screen/macos",
    root / "crates/hypercolor-macos-capture/src",
):
    if directory.is_dir():
        paths.extend(directory.rglob("*.rs"))

primary_paths = set(paths)
support_paths = set(primary_paths)
for directory in (
    root / "crates/hypercolor-core/src",
    root / "crates/hypercolor-macos-capture/src",
):
    if directory.is_dir():
        support_paths.update(directory.rglob("*.rs"))

failed = False
for path in sorted(support_paths):
    relative = path.relative_to(root).as_posix()
    if relative in excluded:
        continue
    source = path.read_text(encoding="utf-8")
    tokens = tokenize(source)
    alternate = rejects_alternate_fixture_path(path, tokens, path in primary_paths)
    if alternate is not None:
        line = source.count("\n", 0, alternate.start) + 1
        print(f"{relative}:{line}: fixture CPU module has an alternate inclusion path")
        failed = True
    if path not in primary_paths:
        continue
    guarded = fixture_ranges(tokens)
    for token in tokens:
        if token.kind != "identifier" or token.text not in forbidden:
            continue
        if any(start <= token.start < end for start, end in guarded):
            continue
        line = source.count("\n", 0, token.start) + 1
        print(f"{relative}:{line}: fixture CPU symbol is reachable without its feature: {token.text}")
        failed = True
sys.exit(1 if failed else 0)
PY
}

require_guarded() {
  local file="$1"
  local feature="$2"
  local declaration="$3"
  local root="${4:-$repo_root}"
  if ! scan_unexpected "$root" --require "$file" "$feature" "$declaration"; then
    printf 'macOS GPU-only fixture boundary is missing in %s: %s\n' \
      "$file" "$declaration" >&2
    return 1
  fi
}

self_test() {
  local probe_root
  probe_root="$(mktemp -d "${TMPDIR:-/tmp}/hypercolor-macos-gpu-only.XXXXXX")"
  trap 'rm -rf -- "$probe_root"' RETURN
  local path
  for path in \
    crates/hypercolor-core/src/input/screen/macos.rs \
    crates/hypercolor-core/src/input/screen/macos/admission.rs \
    crates/hypercolor-core/src/input/screen/macos/publication.rs \
    crates/hypercolor-core/src/input/screen/macos/status.rs \
    crates/hypercolor-macos-capture/src/frame.rs \
    crates/hypercolor-macos-capture/src/lib.rs; do
    mkdir -p "$probe_root/$(dirname "$path")"
    printf '%s\n' 'fn forbidden() { let _ = CpuReductionExecutor::new; }' \
      > "$probe_root/$path"
    if scan_unexpected "$probe_root" >/dev/null 2>&1; then
      printf 'macOS GPU-only self-test missed an unguarded CPU executor in %s\n' \
        "$path" >&2
      return 1
    fi
    rm -f -- "$probe_root/$path"
  done
  mkdir -p "$probe_root/crates/hypercolor-core/src/input/screen/macos"
  printf '%s\n' \
    '#[cfg(feature = "macos-capture-fixtures")]' \
    'fn fixture_only() { let _ = CpuReductionExecutor::new; }' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos/admission.rs"
  scan_unexpected "$probe_root" >/dev/null
  printf '%s\n' \
    '#[cfg(all(target_os = "macos", feature = "macos-capture-fixtures"))]' \
    'fn fixture_only() { let _ = CpuReductionExecutor::new; }' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos/admission.rs"
  scan_unexpected "$probe_root" >/dev/null
  printf '%s\n' \
    '#[cfg(not(feature = "macos-capture-fixtures"))]' \
    'fn production() { let _ = CpuReductionExecutor::new; }' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos/admission.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted an inverted fixture cfg' >&2
    return 1
  fi
  printf '%s\n' \
    '#[cfg(any(target_os = "macos", feature = "macos-capture-fixtures"))]' \
    'fn production() { let _ = CpuReductionExecutor::new; }' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos/admission.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted an optional fixture cfg' >&2
    return 1
  fi
  rm -f -- "$probe_root/crates/hypercolor-core/src/input/screen/macos/admission.rs"
  printf '%s\n' 'mod fixtures;' 'mod tests;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if require_guarded crates/hypercolor-core/src/input/screen/macos.rs \
    macos-capture-fixtures 'mod fixtures;' "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted an unguarded fixture module' >&2
    return 1
  fi
  if require_guarded crates/hypercolor-core/src/input/screen/macos.rs \
    macos-capture-fixtures 'mod tests;' "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted an unguarded fixture test module' >&2
    return 1
  fi
  printf '%s\n' \
    '#[cfg(feature = "macos-capture-fixtures")]' \
    'mod fixtures;' \
    '#[cfg(all(test, feature = "macos-capture-fixtures"))]' \
    'mod tests;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  require_guarded crates/hypercolor-core/src/input/screen/macos.rs \
    macos-capture-fixtures 'mod fixtures;' "$probe_root" >/dev/null
  require_guarded crates/hypercolor-core/src/input/screen/macos.rs \
    macos-capture-fixtures 'mod tests;' "$probe_root" >/dev/null
  printf '%s\n' \
    '/*' \
    '#[cfg(feature = "macos-capture-fixtures")]' \
    'mod fixtures;' \
    '*/' \
    'mod fixtures;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if require_guarded crates/hypercolor-core/src/input/screen/macos.rs \
    macos-capture-fixtures 'mod fixtures;' "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a fixture cfg inside a comment' >&2
    return 1
  fi
  printf '%s\n' \
    'const SPOOF: &str = "#[cfg(feature = \\"macos-capture-fixtures\\")]\\nmod fixtures;";' \
    'mod fixtures;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if require_guarded crates/hypercolor-core/src/input/screen/macos.rs \
    macos-capture-fixtures 'mod fixtures;' "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a fixture cfg inside a string' >&2
    return 1
  fi
  printf '%s\n' \
    'const SPOOF: &str = r##"#[cfg(feature = "macos-capture-fixtures")]' \
    'mod fixtures;"##;' \
    'mod fixtures;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if require_guarded crates/hypercolor-core/src/input/screen/macos.rs \
    macos-capture-fixtures 'mod fixtures;' "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a fixture cfg inside a raw string' >&2
    return 1
  fi
  printf '%s\n' \
    '#[cfg(feature = "macos-capture-fixtures")]' \
    'mod fixtures;' \
    'mod fixtures;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if require_guarded crates/hypercolor-core/src/input/screen/macos.rs \
    macos-capture-fixtures 'mod fixtures;' "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a second unguarded fixture module' >&2
    return 1
  fi
  printf '%s\n' \
    '#[path = "macos/fixtures.rs"]' \
    'mod alternate;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted an alternate fixture module path' >&2
    return 1
  fi
  printf '%s\n' \
    'include!(r#"macos/fixtures.rs"#);' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted an alternate raw-string include path' >&2
    return 1
  fi
  local escaped_path
  for escaped_path in \
    'macos/fixtures\u{2e}rs' \
    'macos/fixtures\x2ers' \
    'macos\u{2f}fixtures.rs' \
    'macos/Fixtures.rs'; do
    printf '%s\n' \
      "#[path = \"${escaped_path}\"]" \
      'mod alternate;' \
      > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
    if scan_unexpected "$probe_root" >/dev/null 2>&1; then
      printf 'macOS GPU-only self-test accepted escaped fixture path: %s\n' \
        "$escaped_path" >&2
      return 1
    fi
  done
  printf '%s\n' \
    'include!(concat!("macos/fixtures", ".rs"));' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a split fixture include path' >&2
    return 1
  fi
  printf '%s\n' \
    'include! { "macos/fixtures.rs" }' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a brace-delimited fixture include' >&2
    return 1
  fi
  printf '%s\n' \
    'include! [ concat!("macos/", concat!("fixtures", ".rs")) ]' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a bracketed nested fixture include' >&2
    return 1
  fi
  printf '%s\n' \
    '#[cfg_attr(not(feature = "macos-capture-fixtures"), path = "macos/fixtures.rs")]' \
    'mod alternate;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a cfg_attr fixture module path' >&2
    return 1
  fi
  printf '%s\n' \
    '#[r#path = "macos/fixtures.rs"]' \
    'mod alternate;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a raw-identifier path attribute' >&2
    return 1
  fi
  printf '%s\n' \
    'r#include!("macos/fixtures.rs");' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a raw-identifier fixture include' >&2
    return 1
  fi
  printf '%s\n' \
    'fn forbidden() { let _ = r#CpuReductionExecutor::new; }' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a raw forbidden identifier' >&2
    return 1
  fi
  printf '%s\n' \
    'use std::{include as first};' \
    'use first as r#second;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted an imported include alias' >&2
    return 1
  fi
  printf '%s\n' \
    'pub(crate) use std::include as load_cpu;' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/alias.rs"
  printf '%s\n' 'fn safe() {}' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test missed an external include alias origin' >&2
    return 1
  fi
  rm -f -- "$probe_root/crates/hypercolor-core/src/input/screen/alias.rs"
  printf '%s\n' \
    'load_cpu! { concat!["macos/", "fixtures.rs"] }' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted an aliased fixture macro call' >&2
    return 1
  fi
  printf '%s\n' \
    '#[macro_export]' \
    'macro_rules! load_cpu { ($path:expr) => { include!($path); } }' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/helper.rs"
  printf '%s\n' \
    'crate::load_cpu!(env!("FIXTURE_PATH"));' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a dynamic include helper' >&2
    return 1
  fi
  printf '%s\n' \
    'load_cpu!(concat!("macos/", "fixtures.rs",));' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a trailing-comma concat path' >&2
    return 1
  fi
  printf '%s\n' \
    'load_cpu!(concat!('\''m'\'', "acos/fixtures.rs"));' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a char-literal concat path' >&2
    return 1
  fi
  printf '%s\n' \
    'load_cpu!(concat!(stringify!(macos), "/", stringify!(fixtures), ".", stringify!(rs)));' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  if scan_unexpected "$probe_root" >/dev/null 2>&1; then
    echo 'macOS GPU-only self-test accepted a stringify concat path' >&2
    return 1
  fi
  rm -f -- "$probe_root/crates/hypercolor-core/src/input/screen/helper.rs"
  printf '%s\n' 'fn safe() {}' \
    > "$probe_root/crates/hypercolor-core/src/input/screen/macos.rs"
  mkdir -p "$probe_root/crates/hypercolor-core/src/attachment"
  printf '%s\n' \
    'include!(concat!(env!("OUT_DIR"), "/embedded_attachments.rs"));' \
    > "$probe_root/crates/hypercolor-core/src/attachment/embedded.rs"
  scan_unexpected "$probe_root" >/dev/null
}

self_test
scan_unexpected "$repo_root"

require_guarded crates/hypercolor-macos-capture/src/lib.rs capture-fixtures 'mod cpu;'
require_guarded crates/hypercolor-core/src/input/screen/macos.rs macos-capture-fixtures \
  'mod fixtures;'
require_guarded crates/hypercolor-core/src/input/screen/macos.rs macos-capture-fixtures \
  'mod tests;'
require_guarded crates/hypercolor-macos-capture/src/lib.rs capture-fixtures \
  'pub use cpu::MacosCpuSourceView;'
require_guarded crates/hypercolor-core/src/input/screen/macos.rs macos-capture-fixtures \
  'cpu_executor: Mutex<Option<Arc<CpuReductionExecutor>>>,'
require_guarded crates/hypercolor-core/src/input/screen/macos.rs macos-capture-fixtures \
  'fanout_candidate: Option<PreparedCpuPublicationFanoutCandidate>,'
require_guarded crates/hypercolor-core/src/input/screen/macos.rs macos-capture-fixtures \
  'fanout: Option<PreparedCpuPublicationFanout>,'
require_guarded crates/hypercolor-core/src/input/screen/macos/publication.rs \
  macos-capture-fixtures \
  'pub(super) fn cpu_executor'
require_guarded crates/hypercolor-core/src/input/screen/macos/publication.rs \
  macos-capture-fixtures \
  'pub(super) fn legacy_cpu_capture_frame'
require_guarded crates/hypercolor-core/src/input/screen/macos/publication.rs \
  macos-capture-fixtures \
  'pub(super) fn native_cpu_capture_frame'
require_guarded crates/hypercolor-core/src/input/screen/macos/publication.rs \
  macos-capture-fixtures \
  'pub(super) fn publish_macos_cpu_exact'
require_guarded crates/hypercolor-core/src/input/screen/macos/publication.rs \
  macos-capture-fixtures \
  'pub(super) fn publish_macos_scalar_exact'
require_guarded crates/hypercolor-core/src/input/screen/macos/admission.rs \
  macos-capture-fixtures \
  'pub(super) fn prepare_macos_exact_runtime'
require_guarded crates/hypercolor-core/src/input/screen/macos/status.rs \
  macos-capture-fixtures \
  'pub(super) fn set_cpu_fallback'

if ! rg -U -q \
  '#\[cfg\(not\(feature = "macos-capture-fixtures"\)\)\][[:space:]]*pub\(super\) fn resolve_macos_publication_branch_with_telemetry[\s\S]*ScreenPublicationExecutorRequest::Cpu => return Ok\(None\)' \
  "$repo_root/crates/hypercolor-core/src/input/screen/macos/publication.rs"; then
  echo 'production macOS publication does not reject CPU execution requests' >&2
  exit 1
fi

echo 'macOS GPU-only architecture fence: PASS'
