"""Generate the vendored OpenAPI client."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import IO, cast

PYTHON_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = PYTHON_ROOT.parent
GENERATED_ROOT = PYTHON_ROOT / "src" / "hypercolor" / "_generated"


def main() -> None:
    args = parse_args()

    with tempfile.TemporaryDirectory(prefix="hypercolor-openapi-") as temp_dir_raw:
        temp_dir = Path(temp_dir_raw)
        spec_path = Path(args.spec) if args.spec else export_openapi(temp_dir)
        validate_json(spec_path)
        spec_path = prepare_generator_spec(spec_path, temp_dir)
        output_path = temp_dir / "client"
        run(
            [
                "uv",
                "run",
                "--group",
                "generate",
                "openapi-python-client",
                "generate",
                "--path",
                str(spec_path),
                "--meta",
                "none",
                "--output-path",
                str(output_path),
                "--overwrite",
                "--fail-on-warning",
            ],
            cwd=PYTHON_ROOT,
        )
        write_generated_marker(output_path)
        make_enums_tolerant(output_path)
        normalize_generated_files(output_path)
        if args.check:
            ensure_generated_matches(output_path)
        else:
            replace_generated(output_path)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--spec",
        type=Path,
        help="Use an existing OpenAPI JSON document instead of exporting one with Cargo.",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail if generated files differ from the checked-in copy.",
    )
    return parser.parse_args()


def export_openapi(temp_dir: Path) -> Path:
    spec_path = temp_dir / "openapi.json"
    completed = run(
        [
            *cargo_cache_command(),
            "cargo",
            "run",
            "-p",
            "hypercolor-daemon",
            "--bin",
            "hypercolor-openapi",
            "--no-default-features",
            "--quiet",
        ],
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
    )
    spec_path.write_text(openapi_json_from_stdout(completed.stdout), encoding="utf-8")
    return spec_path


def cargo_cache_command() -> list[str]:
    if sys.platform == "win32":
        return [
            "powershell.exe",
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            str(REPO_ROOT / "scripts" / "cargo-cache-build.ps1"),
        ]
    return [str(REPO_ROOT / "scripts" / "cargo-cache-build.sh")]


def openapi_json_from_stdout(stdout: str | None) -> str:
    if stdout is None:
        raise RuntimeError("OpenAPI export produced no stdout")
    start = stdout.find("{")
    if start == -1:
        raise RuntimeError("OpenAPI export stdout did not contain JSON")
    return stdout[start:]


def validate_json(path: Path) -> None:
    with path.open(encoding="utf-8") as spec_file:
        json.load(spec_file)


def prepare_generator_spec(path: Path, temp_dir: Path) -> Path:
    with path.open(encoding="utf-8") as spec_file:
        spec = json.load(spec_file)

    schemas = spec.get("components", {}).get("schemas", {})
    for name in (
        "AssetScanStatus",
        "ControlApplyError",
        "ControlOwner",
        "ControlSurfaceEvent",
        "ControlSurfaceScope",
        "DisplayFaceResponseOptional",
        "EffectSource",
        "LayerSource",
    ):
        schema = schemas.get(name)
        if isinstance(schema, dict):
            schemas[name] = {
                "type": "object",
                "description": schema.get("description", f"{name} payload"),
            }

    normalize_edge_behavior_schema(schemas)
    collapse_string_enum_branches(schemas)
    normalize_recursive_control_schemas(schemas)

    normalize_binary_response_media_types(spec)

    generator_spec = temp_dir / "openapi-python-client.json"
    generator_spec.write_text(json.dumps(spec, indent=2), encoding="utf-8")
    return generator_spec


def normalize_edge_behavior_schema(schemas: dict[str, object]) -> None:
    schema = schemas.get("EdgeBehavior")
    if not isinstance(schema, dict):
        return
    variants = schema.get("oneOf")
    if not isinstance(variants, list):
        return

    string_values = [
        value
        for variant in variants
        if isinstance(variant, dict) and variant.get("type") == "string"
        for enum_values in [variant.get("enum")]
        if isinstance(enum_values, list)
        for value in enum_values
        if isinstance(value, str)
    ]
    fade_wrapper = None
    for variant in variants:
        if not isinstance(variant, dict):
            continue
        properties = variant.get("properties")
        if isinstance(properties, dict) and "fade_to_black" in properties:
            fade_wrapper = variant
            break
    if not string_values or not isinstance(fade_wrapper, dict):
        return
    fade_properties = fade_wrapper.get("properties")
    if not isinstance(fade_properties, dict):
        return
    fade_value = fade_properties.get("fade_to_black")
    if not isinstance(fade_value, dict):
        return

    schemas["EdgeBehavior"] = {
        "description": schema.get("description", "Edge behavior for out-of-bounds LED positions."),
        "oneOf": [
            {"type": "string", "enum": string_values},
            {"$ref": "#/components/schemas/EdgeBehaviorFadeToBlack"},
        ],
    }
    schemas["EdgeBehaviorFadeToBlack"] = {
        "type": "object",
        "description": fade_wrapper.get("description"),
        "required": ["fade_to_black"],
        "properties": {
            "fade_to_black": {"$ref": "#/components/schemas/EdgeBehaviorFadeToBlackValue"}
        },
    }
    schemas["EdgeBehaviorFadeToBlackValue"] = fade_value


def is_plain_string_enum(schema: object) -> bool:
    return (
        isinstance(schema, dict)
        and schema.get("type") == "string"
        and isinstance(schema.get("enum"), list)
        and set(schema) <= {"type", "enum", "description"}
    )


def collapse_string_enum_branches(schemas: dict[str, object]) -> None:
    """Merge a union's string-enum branches into one string enum.

    utoipa renders each documented unit variant as its own one-value string
    enum, so a Rust enum becomes a `oneOf` of single-value branches, often
    beside an object branch for a data-carrying catch-all. The generated
    parser tries the branches in order, and a tolerant enum branch accepts
    any string, so separate branches would swallow each other's values.
    One merged branch keeps every known value on one enum class.
    """
    for name, schema in list(schemas.items()):
        if not isinstance(schema, dict):
            continue
        branches = schema.get("oneOf")
        if not isinstance(branches, list):
            continue
        string_branches = [branch for branch in branches if is_plain_string_enum(branch)]
        if len(string_branches) < 2:
            continue
        values = [
            value
            for branch in string_branches
            for value in cast(dict[str, list[object]], branch)["enum"]
        ]
        merged: dict[str, object] = {"type": "string", "enum": values}
        description = schema.get("description")
        if isinstance(description, str):
            merged["description"] = description
        others = [branch for branch in branches if not is_plain_string_enum(branch)]
        if not others:
            schemas[name] = merged
            continue
        collapsed = {key: value for key, value in schema.items() if key != "oneOf"}
        collapsed["oneOf"] = [merged, *others]
        schemas[name] = collapsed


TOLERANT_ENUM_MODULE = '''"""String enums that keep values a newer daemon sends.

Generated by `python/scripts/generate_openapi_client.py`. Do not edit by hand.
"""

from enum import StrEnum
from typing import Self


class TolerantStrEnum(StrEnum):
    """A string enum that admits values newer than this client.

    A minor daemon release may add enum values. A value this client does not
    list decodes as an unlisted member that keeps the wire string, so one new
    value never fails the whole response. It equals its string and is never
    one of the declared members.
    """

    @classmethod
    def _missing_(cls, value: object) -> Self | None:
        if not isinstance(value, str):
            return None
        member = str.__new__(cls, value)
        member._name_ = value
        member._value_ = value
        return member
'''

STR_ENUM_CLASS = re.compile(
    r"^class (\w+)\(StrEnum\):\n((?:    [A-Z0-9_]+ = .*\n)+)", re.MULTILINE
)
UNION_PARSER = re.compile(r"^(\s+)def _parse_\w+\(.*?(?=^\1\S|\Z)", re.MULTILINE | re.DOTALL)


def make_enums_tolerant(root: Path) -> None:
    """Rebase every generated vocabulary enum on `TolerantStrEnum`.

    An enum with one member is the tag of an internally tagged union
    variant (every one in the document is), and the generated parser tells
    variants apart by trying each tag strictly, so tags stay `StrEnum`.
    Every enum with two or more members is vocabulary a newer daemon may
    extend, so it learns to keep unknown values.
    """
    (root / "tolerant_enum.py").write_text(TOLERANT_ENUM_MODULE, encoding="utf-8", newline="\n")
    tolerant: set[str] = set()
    for path in sorted((root / "models").glob("*.py")):
        text = path.read_text(encoding="utf-8")
        match = STR_ENUM_CLASS.search(text)
        if match is None or len(match.group(2).splitlines()) < 2:
            continue
        if "from enum import StrEnum\n" not in text:
            raise RuntimeError(f"{path.name} declares a StrEnum without the expected import")
        text = text.replace(
            "from enum import StrEnum\n", "from ..tolerant_enum import TolerantStrEnum\n"
        ).replace(f"class {match.group(1)}(StrEnum):", f"class {match.group(1)}(TolerantStrEnum):")
        path.write_text(text, encoding="utf-8", newline="\n")
        tolerant.add(match.group(1))
    if not tolerant:
        raise RuntimeError("no generated enum was rebased on TolerantStrEnum")
    ensure_unions_stay_decidable(root, tolerant)


def ensure_unions_stay_decidable(root: Path, tolerant: set[str]) -> None:
    """Refuse a union parser that tries two tolerant enums.

    A tolerant enum accepts any string, so the first one tried would claim
    every value meant for the second. `collapse_string_enum_branches` merges
    such branches in the spec; this check proves nothing slipped past it.
    """
    constructor = re.compile(r"\b(" + "|".join(sorted(tolerant)) + r")\(data\)")
    for path in sorted((root / "models").glob("*.py")):
        for parser in UNION_PARSER.finditer(path.read_text(encoding="utf-8")):
            enums = set(constructor.findall(parser.group(0)))
            if len(enums) > 1:
                raise RuntimeError(f"{path.name} parses one value as any of {sorted(enums)}")


def normalize_recursive_control_schemas(schemas: dict[str, object]) -> None:
    schema = schemas.get("ControlValue")
    if not isinstance(schema, dict):
        return
    variants = schema.get("oneOf")
    if not isinstance(variants, list):
        return
    kinds = [
        kind
        for variant in variants
        if isinstance(variant, dict)
        for properties in [variant.get("properties")]
        if isinstance(properties, dict)
        for kind_schema in [properties.get("kind")]
        if isinstance(kind_schema, dict)
        for enum_values in [kind_schema.get("enum")]
        if isinstance(enum_values, list)
        for kind in enum_values
        if isinstance(kind, str)
    ]
    schemas["ControlValue"] = {
        "type": "object",
        "description": schema.get("description", "ControlValue payload"),
        "required": ["kind"],
        "properties": {
            "kind": {"type": "string", "enum": kinds},
            "value": {},
        },
        "additionalProperties": False,
    }


def normalize_binary_response_media_types(spec: dict[str, object]) -> None:
    paths = spec.get("paths")
    if not isinstance(paths, dict):
        return
    for path_item in cast(dict[str, object], paths).values():
        if not isinstance(path_item, dict):
            continue
        for operation in cast(dict[str, object], path_item).values():
            if not isinstance(operation, dict):
                continue
            responses = cast(dict[str, object], operation).get("responses")
            if not isinstance(responses, dict):
                continue
            for response in cast(dict[str, object], responses).values():
                if not isinstance(response, dict):
                    continue
                content = cast(dict[str, object], response).get("content")
                if not isinstance(content, dict):
                    continue
                content_by_media_type = cast(dict[str, object], content)
                for media_type in list(content_by_media_type):
                    if media_type.startswith("image/") or media_type == "application/x-tar":
                        content_by_media_type["application/octet-stream"] = (
                            content_by_media_type.pop(media_type)
                        )


def replace_generated(source: Path) -> None:
    if GENERATED_ROOT.exists():
        shutil.rmtree(GENERATED_ROOT)
    ignore = shutil.ignore_patterns(".ruff_cache", "__pycache__", "*.pyc")
    shutil.copytree(source, GENERATED_ROOT, ignore=ignore)


def normalize_generated_files(root: Path) -> None:
    for path in root.rglob("*"):
        if path.suffix == ".py" or path.name == "GENERATED.md":
            text = path.read_text(encoding="utf-8")
            path.write_text(text, encoding="utf-8", newline="\n")


def write_generated_marker(root: Path) -> None:
    (root / "GENERATED.md").write_text(
        "\n".join(
            [
                "# Generated Hypercolor OpenAPI Client",
                "",
                "This private package is generated by `python/scripts/generate_openapi_client.py`.",
                "Do not edit these files by hand.",
                "",
            ]
        ),
        encoding="utf-8",
        newline="\n",
    )


def ensure_generated_matches(source: Path) -> None:
    expected_files = relative_files(source)
    actual_files = relative_files(GENERATED_ROOT)
    missing = expected_files - actual_files
    extra = actual_files - expected_files
    changed = {
        path
        for path in expected_files & actual_files
        if (source / path).read_bytes() != (GENERATED_ROOT / path).read_bytes()
    }
    if missing or extra or changed:
        for label, paths in (("missing", missing), ("extra", extra), ("changed", changed)):
            for path in sorted(paths)[:20]:
                print(f"{label}: {path}", file=sys.stderr)
        raise SystemExit("generated OpenAPI client is out of date")


def relative_files(root: Path) -> set[Path]:
    if not root.exists():
        return set()
    return {
        path.relative_to(root)
        for path in root.rglob("*")
        if path.is_file()
        and ".ruff_cache" not in path.parts
        and "__pycache__" not in path.parts
        and path.suffix != ".pyc"
    }


def run(
    command: list[str],
    *,
    cwd: Path,
    stdout: int | IO[str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        stdout=stdout,
        text=True,
        encoding="utf-8",
        check=True,
    )


if __name__ == "__main__":
    main()
