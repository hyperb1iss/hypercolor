"""Generate the vendored OpenAPI client."""

from __future__ import annotations

import argparse
import json
import os
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
        "EdgeBehavior",
        "EffectSource",
        "LayerSource",
    ):
        schema = schemas.get(name)
        if isinstance(schema, dict):
            schemas[name] = {
                "type": "object",
                "description": schema.get("description", f"{name} payload"),
            }

    normalize_recursive_control_schemas(schemas)

    normalize_binary_response_media_types(spec)

    generator_spec = temp_dir / "openapi-python-client.json"
    generator_spec.write_text(json.dumps(spec, indent=2), encoding="utf-8")
    return generator_spec


def normalize_recursive_control_schemas(schemas: dict[str, object]) -> None:
    for name in ("ControlValue", "DriverControlValue"):
        schema = schemas.get(name)
        if not isinstance(schema, dict):
            continue
        variants = schema.get("oneOf")
        if not isinstance(variants, list):
            continue
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
        schemas[name] = {
            "type": "object",
            "description": schema.get("description", f"{name} payload"),
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
                    if media_type.startswith("image/"):
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
    env = os.environ.copy()
    env.setdefault("CARGO_TARGET_DIR", str(Path.home() / ".cache" / "hypercolor" / "target"))
    return subprocess.run(
        command,
        cwd=cwd,
        env=env,
        stdout=stdout,
        text=True,
        encoding="utf-8",
        check=True,
    )


if __name__ == "__main__":
    main()
