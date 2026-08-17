# WebSocket binary wire-format golden fixtures

Every binary tag the daemon can put on `/api/v1/ws` is frozen here. Each `.hex`
file holds the exact bytes a fixed input produces when it runs through the
encoder production uses, and `tests/ws_golden_tests.rs` asserts that equality on
every run. A change that shifts a field, widens an integer, reorders a header,
or renumbers a tag fails the suite before any deployed client sees it.

## Changing a fixture is changing the wire

These bytes are a contract with every shipped web UI, TUI, and third-party
client. Rewriting a fixture to make a test pass is the failure mode this
directory exists to prevent.

A deliberate wire change goes through the lockstep doctrine in
`docs/specs/76-internal-api-unification.md` §0: the new form replaces the old
one outright, every in-repo client moves in the same PR, and no dual-accept or
version-gated fallback ships. So a fixture for a changed layout is **replaced in
place**, deliberately, in the same commit that changes the layout — the fixture
is an intentionality fence, not a compatibility record. A layout that is genuinely
new, rather than a changed one, gets its own fixture.

The distinction that matters is who rewrites a fixture and why. Rewriting one to
make a red test go green is the failure this directory exists to prevent.
Rewriting one because you changed the wire on purpose, and reviewing the byte
diff as the change, is the workflow.

## File format

Plain text. Everything after `#` on a line is a comment; the remaining tokens
are two-digit lowercase hex bytes, in wire order. The header block carries the
fixture name, the tag byte and the constant it comes from, the encoder path, a
description of the input, and two load-bearing counts:

- `total-bytes` — length of the complete encoder output, checked against the
  encoder on every run.
- `stored-bytes` — how many of those bytes this file carries, checked against
  the bytes actually parsed out of it.

The two counts are equal for every fixture except the four wide-layout preview
frames (`0b`, `0c`, `0d`, `12`). A wide frame needs an axis above 65535, so its
smallest legal RGB payload is 196 608 bytes. Those fixtures commit the full
header plus the first 16 payload bytes; the remainder is a deterministic fill
that would add hundreds of kilobytes of hex without pinning any layout the
header does not already pin. The test still asserts the full encoded length
against `total-bytes`, and `wide_frame_payloads_are_written_verbatim` asserts
that the encoder copies those payloads through unmodified.

## Regenerating one fixture

Only when the wire change is intended and has been through the process above:

```bash
HYPERCOLOR_WS_GOLDEN_BLESS=09-screen-zones \
  ./scripts/cargo-cache-build.sh cargo test -p hypercolor-daemon --test ws_golden_tests
```

The variable takes a fixture name (the file stem), a comma-separated list of
them, or `all`. An unknown name fails the test rather than silently doing
nothing. Named fixtures are rewritten from the current encoder output and then
asserted, so a blessing run that passes means the new bytes are on disk. Review
the resulting diff byte by byte — that diff is the wire-format change.

Writing a fixture that did not exist before takes two runs: the round-trip test
reads from disk and runs alongside the one that writes, so run the suite once
more without the variable to assert the freshly written files.

## Coverage

Two completeness gates run on every build, from two different authorities.

`golden_fixtures_cover_the_whole_known_tag_space` derives the expected tag set
from `protocol/websocket-v1.json` and asserts it matches the fixture table
exactly, in both directions. A fixture cannot claim a tag the manifest never
declared, and a manifest tag cannot ship without one. Tag `0x04` stays
deliberately unassigned.

`golden_fixtures_cover_every_tag_leptos_ext_declares` walks every `.rs` file
under `hypercolor-leptos-ext/src/ws/`, including subdirectories and files that
did not exist when this gate was written, and scans them for tag constants. It
asserts the declaration count, per-tag fixture coverage, and that no two
differently named constants claim the same tag byte, so a wire collision fails
with both names rather than being silently merged. Moving the codecs out of that
directory makes the walk find nothing and fail loudly rather than pass
vacuously.

The scan reads source text, so it recognizes the shape the codecs actually use:
`pub const NAME_TAG: u8 = 0x..` and hex discriminants on the
`PreviewFrameChannel` enum. A tag written as a decimal literal, aliased through
another constant, or produced by a macro is outside what it can see. That
boundary is deliberate: the gate exists to catch a tag someone forgot to freeze,
not to defend against someone deliberately hiding one. Spec 76 §5 replaces it
with a real registry, where `define_ws_topics!` can enforce unique tag ownership
at compile time.

Decode coverage mirrors the encoders: every fixture whose frame has a
`hypercolor-leptos-ext` decoder is decoded back from the on-disk bytes and
compared to the input struct. The `frames` tag (`0x01`) has no in-repo decoder,
so only its encoder is frozen.

Two fixtures share tag `0x01` because the daemon has two encode paths for it:
`01-frames-all` takes the unfiltered path and `01-frames-selected` takes the
zone-filtered one, which writes its zone count after the loop rather than
before.
