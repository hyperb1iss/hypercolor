#!/usr/bin/env python3
"""Summarize faceless-screen A/B runs.

usage: report.py <runs-dir> <label> [<label> ...]

Reads <runs-dir>/pair-*/<label>/samples.json. Prints per-pair p50/p95/p99
for each label, the across-pair median and range of each percentile, and
the paired differences against the first label.
"""

import json
import statistics
import sys
from pathlib import Path

METRICS = ("total_ms", "composition_ms", "wake_late_ms")
PERCENTILES = (50, 95, 99)


def percentile(sorted_values, q):
    if not sorted_values:
        return float("nan")
    rank = (len(sorted_values) - 1) * q / 100.0
    low = int(rank)
    high = min(low + 1, len(sorted_values) - 1)
    return sorted_values[low] + (sorted_values[high] - sorted_values[low]) * (rank - low)


def load(path):
    data = json.loads(path.read_text())
    fields = data["fields"]
    columns = {field: sorted(row[index] for row in data["frames"]) for index, field in enumerate(fields)}
    return data, columns


def main():
    runs = Path(sys.argv[1])
    labels = sys.argv[2:]
    pairs = sorted(p for p in runs.glob("pair-*") if all((p / label / "samples.json").exists() for label in labels))
    if not pairs:
        raise SystemExit("no complete pairs")

    stats = {label: {m: {q: [] for q in PERCENTILES} for m in METRICS} for label in labels}
    pooled = {label: {m: [] for m in METRICS} for label in labels}
    counts = {label: [] for label in labels}
    for pair in pairs:
        line = [pair.name]
        for label in labels:
            data, columns = load(pair / label / "samples.json")
            counts[label].append((len(data["frames"]), data["rendered_frames"], data["preview_frames"]))
            tier = data["render_loop_after"]["fps_tier"]
            cells = []
            for metric in METRICS:
                values = columns[metric]
                pooled[label][metric].extend(values)
                triple = [percentile(values, q) for q in PERCENTILES]
                for q, value in zip(PERCENTILES, triple):
                    stats[label][metric][q].append(value)
                cells.append("/".join(f"{value:.2f}" for value in triple))
            line.append(f"{label}[{tier.split()[0]} n={len(data['frames'])}] " + "  ".join(cells))
        print(" | ".join(line))

    print()
    print(f"pairs: {len(pairs)}; columns are p50/p95/p99 in ms")
    for label in labels:
        sampled = sum(c[0] for c in counts[label])
        rendered = sum(c[1] for c in counts[label])
        previews = sum(c[2] for c in counts[label])
        print(f"{label}: {sampled} frames sampled of {rendered} rendered, {previews} preview frames")
    print()
    header = f"{'metric':<16}{'label':<8}" + "".join(f"{'p' + str(q) + ' median [min..max]':<30}" for q in PERCENTILES)
    print(header)
    for metric in METRICS:
        for label in labels:
            row = f"{metric:<16}{label:<8}"
            for q in PERCENTILES:
                values = stats[label][metric][q]
                row += f"{statistics.median(values):.2f} [{min(values):.2f}..{max(values):.2f}]".ljust(30)
            print(row)
    print()
    print("pooled over every sampled frame:")
    for metric in METRICS:
        for label in labels:
            values = sorted(pooled[label][metric])
            triple = "/".join(f"{percentile(values, q):.2f}" for q in PERCENTILES)
            mean = statistics.fmean(values)
            print(f"{metric:<16}{label:<8}{triple}  mean {mean:.3f}")
    base = labels[0]
    for other in labels[1:]:
        print()
        print(f"paired difference {other} - {base}, median over pairs [min..max], pairs where {other} is higher:")
        for metric in METRICS:
            row = f"{metric:<16}"
            for q in PERCENTILES:
                diffs = [b - a for a, b in zip(stats[base][metric][q], stats[other][metric][q])]
                higher = sum(1 for d in diffs if d > 0)
                row += f"p{q} {statistics.median(diffs):+.2f} [{min(diffs):+.2f}..{max(diffs):+.2f}] {higher}/{len(diffs)}".ljust(40)
            print(row)


if __name__ == "__main__":
    main()
