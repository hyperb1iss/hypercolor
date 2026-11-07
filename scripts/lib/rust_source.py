"""Light Rust source scanning shared by the repository's static checks.

The checks only need three things a regex cannot give them reliably: text
with comments and literal contents blanked out (so a constructor named in a
doc comment or a string never counts), balanced delimiter matching, and the
spans of test-only code. Masking keeps every offset and newline in place,
so positions found in the masked text map straight back to source lines.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

TEST_PATH = re.compile(r"(^|/)(tests?|benches)(/|\.rs$)|_tests\.rs$")
_RAW_STRING = re.compile(r'b?r(#*)"')
_CFG_ATTRIBUTE = re.compile(r"#\s*(!?)\s*\[\s*cfg\s*\(")
_NEGATED_TEST = re.compile(r"not\s*\(\s*test\s*\)")
_TEST_IDENT = re.compile(r"\btest\b")
_OPEN = "([{"
_CLOSE = ")]}"


def _blank(text: str) -> str:
    return "".join("\n" if ch == "\n" else " " for ch in text)


def mask(source: str) -> str:
    """Blank comments and string or char literal contents, keeping offsets."""
    out: list[str] = []
    i = 0
    n = len(source)
    while i < n:
        if source.startswith("//", i):
            end = source.find("\n", i)
            end = n if end < 0 else end
            out.append(_blank(source[i:end]))
            i = end
            continue
        if source.startswith("/*", i):
            depth = 1
            j = i + 2
            while j < n and depth:
                if source.startswith("/*", j):
                    depth += 1
                    j += 2
                elif source.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            out.append(_blank(source[i:j]))
            i = j
            continue
        raw = _RAW_STRING.match(source, i)
        if raw and (i == 0 or not (source[i - 1].isalnum() or source[i - 1] == "_")):
            terminator = '"' + raw.group(1)
            end = source.find(terminator, raw.end())
            end = n if end < 0 else end
            out.append(source[i : raw.end()])
            out.append(_blank(source[raw.end() : end]))
            out.append(source[end : end + len(terminator)])
            i = end + len(terminator)
            continue
        if source[i] == '"' or source.startswith('b"', i):
            start = i + (2 if source[i] == "b" else 1)
            j = start
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            out.append(source[i:start])
            out.append(_blank(source[start:j]))
            out.append('"')
            i = j + 1
            continue
        if source[i] == "'":
            if i + 1 < n and source[i + 1] == "\\":
                # Skip the escaped character itself, which may be a quote.
                end = source.find("'", i + 3)
                if end > 0:
                    out.append("'" + _blank(source[i + 1 : end]) + "'")
                    i = end + 1
                    continue
            elif i + 2 < n and source[i + 2] == "'":
                out.append("' '")
                i += 3
                continue
        out.append(source[i])
        i += 1
    return "".join(out)


def closing(masked: str, open_index: int) -> int:
    """Index of the delimiter that closes the one at `open_index`."""
    depth = 0
    for j in range(open_index, len(masked)):
        ch = masked[j]
        if ch in _OPEN:
            depth += 1
        elif ch in _CLOSE:
            depth -= 1
            if depth == 0:
                return j
    raise ValueError(f"unbalanced delimiter at offset {open_index}")


def is_test_predicate(predicate: str) -> bool:
    """Whether a cfg predicate only holds under `cfg(test)`."""
    return bool(_TEST_IDENT.search(_NEGATED_TEST.sub("", predicate)))


def _item_end(masked: str, start: int) -> int | None:
    """End of the item starting at `start`, or None for `mod name;`."""
    depth = 0
    j = start
    while j < len(masked):
        ch = masked[j]
        if ch == "#" and masked[j + 1 : j + 2] == "[":
            j = closing(masked, j + 1) + 1
            continue
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth -= 1
        elif ch == "{" and depth == 0:
            return closing(masked, j)
        elif ch == ";" and depth == 0:
            return None
        j += 1
    return None


def test_regions(masked: str) -> list[tuple[int, int]]:
    """Spans of items gated on `cfg(test)` (inline test modules and the like)."""
    regions: list[tuple[int, int]] = []
    for match in _CFG_ATTRIBUTE.finditer(masked):
        open_paren = match.end() - 1
        close_paren = closing(masked, open_paren)
        if not is_test_predicate(masked[open_paren + 1 : close_paren]):
            continue
        if match.group(1):
            return [(0, len(masked))]
        attribute_end = masked.find("]", close_paren) + 1
        end = _item_end(masked, attribute_end)
        if end is not None:
            regions.append((match.start(), end))
    return regions


@dataclass(frozen=True)
class RustFile:
    """A Rust source file with its masked text and test spans."""

    path: Path
    relative: str
    source: str
    masked: str
    regions: tuple[tuple[int, int], ...]
    whole_file_is_test: bool

    @classmethod
    def load(cls, root: Path, path: Path) -> RustFile:
        source = path.read_text(encoding="utf-8")
        masked = mask(source)
        relative = path.relative_to(root).as_posix()
        return cls(
            path=path,
            relative=relative,
            source=source,
            masked=masked,
            regions=tuple(test_regions(masked)),
            whole_file_is_test=bool(TEST_PATH.search(relative)),
        )

    def in_test_code(self, offset: int) -> bool:
        return self.whole_file_is_test or any(
            start <= offset <= end for start, end in self.regions
        )

    def line_of(self, offset: int) -> int:
        return self.source.count("\n", 0, offset) + 1

    def line_text(self, offset: int) -> str:
        start = self.source.rfind("\n", 0, offset) + 1
        end = self.source.find("\n", offset)
        return self.source[start : end if end >= 0 else len(self.source)].strip()


def rust_files(root: Path, directories: list[str]) -> list[Path]:
    """Every `.rs` file under the named directories, failing on a missing one."""
    files: list[Path] = []
    for directory in directories:
        base = root / directory
        if not base.is_dir():
            raise FileNotFoundError(f"cannot scan {base}: not a directory")
        files.extend(sorted(base.rglob("*.rs")))
    return files
