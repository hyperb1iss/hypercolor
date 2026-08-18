"""Generate Python WebSocket protocol constants from the shared manifest."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

PYTHON_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = PYTHON_ROOT.parent
MANIFEST_PATH = REPO_ROOT / "protocol" / "websocket-v1.json"
OUTPUT_PATH = PYTHON_ROOT / "src" / "hypercolor" / "ws_protocol.py"


def main() -> None:
    args = parse_args()
    generated = render(load_manifest(args.manifest))

    if args.check:
        current = OUTPUT_PATH.read_text(encoding="utf-8") if OUTPUT_PATH.exists() else ""
        if current != generated:
            raise SystemExit("generated WebSocket protocol constants are out of date")
        return

    OUTPUT_PATH.write_text(generated, encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=Path,
        default=MANIFEST_PATH,
        help="Path to the shared WebSocket protocol manifest.",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail if the generated constants differ from the checked-in copy.",
    )
    return parser.parse_args()


def load_manifest(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as manifest_file:
        manifest = json.load(manifest_file)
    if not isinstance(manifest, dict):
        raise TypeError("WebSocket protocol manifest must be a JSON object")
    return manifest


def render(manifest: dict[str, Any]) -> str:
    topics = [str(topic["name"]) for topic in expect_list(manifest["topics"])]
    json_payloads = expect_dict(manifest["json_payloads"])
    json_payload_contracts = render_python_value(json_payloads, indent=0)
    binary_messages = expect_list(manifest["binary_messages"])
    preview_messages = [
        message for message in binary_messages if message.get("layout") == "preview_frame"
    ]
    preview_formats = expect_dict(expect_dict(manifest["preview_frame"])["formats"])

    lines = [
        '"""Generated WebSocket protocol constants."""',
        "",
        "from __future__ import annotations",
        "",
        "from types import MappingProxyType",
        "from typing import Final",
        "",
        f"WS_PROTOCOL_VERSION: Final = {quote(str(manifest['version']))}",
        f"WS_SUBPROTOCOL: Final = {quote(str(manifest['subprotocol']))}",
        *tuple_assignment("DEFAULT_WS_SUBSCRIPTIONS", manifest["default_subscriptions"]),
        "",
        "WS_TOPICS: Final = (",
        *[f"    {quote(topic)}," for topic in topics],
        ")",
        *tuple_assignment("WS_CAPABILITIES", manifest["capabilities"]),
        "",
        f"JSON_PAYLOAD_CONTRACTS: Final = {json_payload_contracts[0]}",
        *json_payload_contracts[1:],
        "",
        "BINARY_MESSAGE_TAGS: Final = MappingProxyType(",
        "    {",
        *[
            f"        {quote(str(message['name']))}: 0x{int(message['tag']):02X},"
            for message in binary_messages
        ],
        "    }",
        ")",
        "PREVIEW_TOPIC_TAGS: Final = MappingProxyType(",
        "    {",
        *[
            f"        0x{int(message['tag']):02X}: {quote(str(message['topic']))},"
            for message in preview_messages
        ],
        "    }",
        ")",
        "CANVAS_FORMAT_TAGS: Final = MappingProxyType(",
        "    {",
        *[
            f"        {int(tag)}: {quote(str(name))},"
            for name, tag in sorted(preview_formats.items(), key=lambda item: int(item[1]))
        ],
        "    }",
        ")",
        "",
    ]
    return "\n".join(lines)


def render_python_value(value: Any, *, indent: int) -> list[str]:
    if isinstance(value, dict):
        return render_python_mapping(value, indent=indent)

    if isinstance(value, list):
        return render_python_list(value, indent=indent)

    if value is None:
        rendered = "None"
    elif isinstance(value, bool):
        rendered = str(value)
    elif isinstance(value, (int, float)):
        rendered = repr(value)
    elif isinstance(value, str):
        rendered = quote(value)
    else:
        raise TypeError("expected JSON value")
    return [rendered]


def render_python_mapping(value: dict[Any, Any], *, indent: int) -> list[str]:
    if not value:
        return ["MappingProxyType({})"]
    lines = ["MappingProxyType(", f"{' ' * (indent + 4)}{{"]
    child_indent = indent + 8
    child_prefix = " " * child_indent
    for key, child in value.items():
        if not isinstance(key, str):
            raise TypeError("expected JSON object key")
        rendered = render_python_value(child, indent=child_indent)
        if len(rendered) == 1:
            lines.append(f"{child_prefix}{quote(key)}: {rendered[0]},")
            continue
        lines.append(f"{child_prefix}{quote(key)}: {rendered[0]}")
        lines.extend(rendered[1:-1])
        lines.append(f"{rendered[-1]},")
    lines.extend((f"{' ' * (indent + 4)}}}", f"{' ' * indent})"))
    return lines


def render_python_list(value: list[Any], *, indent: int) -> list[str]:
    if not value:
        return ["()"]
    if len(value) == 1:
        rendered = render_python_value(value[0], indent=indent)
        if len(rendered) == 1:
            return [f"({rendered[0]},)"]
    lines = ["("]
    child_indent = indent + 4
    child_prefix = " " * child_indent
    for child in value:
        rendered = render_python_value(child, indent=child_indent)
        if len(rendered) == 1:
            lines.append(f"{child_prefix}{rendered[0]},")
            continue
        lines.append(f"{child_prefix}{rendered[0]}")
        lines.extend(rendered[1:-1])
        lines.append(f"{rendered[-1]},")
    lines.append(f"{' ' * indent})")
    return lines


def tuple_assignment(name: str, values: Any) -> list[str]:
    strings = [str(value) for value in expect_list(values)]
    if len(strings) == 1:
        return [f"{name}: Final = ({quote(strings[0])},)"]
    return [f"{name}: Final = (", *[f"    {quote(value)}," for value in strings], ")"]


def quote(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def expect_dict(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise TypeError("expected JSON object")
    return value


def expect_list(value: Any) -> list[Any]:
    if not isinstance(value, list):
        raise TypeError("expected JSON array")
    return value


if __name__ == "__main__":
    main()
