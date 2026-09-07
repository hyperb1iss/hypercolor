import { describe, expect, test } from "bun:test"

import { analyze, defaults, type JsonObject, type MetricSample } from "./graphics-pipeline-soak"

function metrics(inputP95Ms: number, inputSampleCount: number, sessionFullFrameCount: number): JsonObject {
    return {
        fps: { actual: 60, target: 60 },
        frame_time: { p95_ms: 10 },
        input_latency: { p95_ms: inputP95Ms, sample_count: inputSampleCount },
        copies: {
            full_frame_count: 0,
            session_full_frame_count: sessionFullFrameCount,
        },
        display_output: {
            display_lane: { display_led_priority_wait_max_ms: 0 },
        },
        timeline: { frame_token: inputSampleCount },
    }
}

function samples(first: JsonObject, last: JsonObject): MetricSample[] {
    return [
        { receivedAtMs: 4_000, data: { ...first, timeline: { frame_token: 100 } } },
        { receivedAtMs: 5_500, data: { ...last, timeline: { frame_token: 101 } } },
        { receivedAtMs: 6_000, data: { ...last, timeline: { frame_token: 102 } } },
    ]
}

const specConfig = {
    ...defaults,
    durationMs: 7_000,
    requireMacosNativeCapture: true,
}

function activeStatus(
    inputSampleCount: number,
    sessionFullFrameCount: number,
    inputP95Ms: number,
    framesPublished: number,
    uptimeSeconds: number,
): JsonObject {
    const source = (kind: string, freshness: string, platform?: JsonObject): JsonObject => ({
        source_id: `source-${kind}`,
        kind,
        demanded: true,
        active_consumer_count: 1,
        state: "live",
        freshness,
        source_graph_generation: 9,
        session_generation: 3,
        ...(platform ? { platform } : {}),
    })
    return {
        server: { instance_id: "daemon-1" },
        uptime_seconds: uptimeSeconds,
        capture_available: true,
        session_performance: {
            input_stage: {
                sample_count: inputSampleCount,
                p95_ms: inputP95Ms,
                cumulative_histogram: cumulativeHistogram(
                    inputSampleCount,
                    inputP95Ms,
                    framesPublished,
                ),
            },
            full_frame_cpu_copies: { count: sessionFullFrameCount },
        },
        input: {
            sources: [
                source("screen", "fresh", {
                    type: "macos_screen",
                    telemetry: {
                        publication_path: "native",
                        capture_session_generation: 7,
                        frames_published: framesPublished,
                    },
                }),
                source("audio", "fresh"),
                source("interaction", "not_applicable"),
            ],
        },
    }
}

function cumulativeHistogram(
    sampleCount: number,
    latestP95Ms: number,
    snapshotFrameToken: number,
): JsonObject {
    const historicalCount = Math.min(sampleCount, 10)
    const latestCount = sampleCount - historicalCount
    const counts = new Map<number, number>()
    counts.set(8, historicalCount)
    if (latestCount > 0) {
        const bucketIndex = Math.ceil(latestP95Ms * 10)
        counts.set(bucketIndex, (counts.get(bucketIndex) ?? 0) + latestCount)
    }
    return {
        bucket_width_us: 100,
        overflow_bucket_index: 4096,
        snapshot_frame_token: snapshotFrameToken,
        buckets: [...counts]
            .filter(([, count]) => count > 0)
            .map(([bucket_index, count]) => ({ bucket_index, count })),
    }
}

function setCumulativeHistogram(
    status: JsonObject,
    buckets: Array<[number, number]>,
    snapshotFrameToken: number,
): void {
    const inputStage = valueAt(status, ["session_performance", "input_stage"]) as JsonObject
    inputStage.cumulative_histogram = {
        bucket_width_us: 100,
        overflow_bucket_index: 4096,
        snapshot_frame_token: snapshotFrameToken,
        buckets: buckets.map(([bucket_index, count]) => ({ bucket_index, count })),
    }
}

function analyzeActive(first: JsonObject, last: JsonObject, intermediate: MetricSample[] = []) {
    const observed = samples(first, last)
    return analyze(
        specConfig,
        [observed[0]!, ...intermediate, ...observed.slice(1)],
        [],
        activeStatus(
            metricNumber(first, ["input_latency", "sample_count"]),
            metricNumber(first, ["copies", "session_full_frame_count"]),
            metricNumber(first, ["input_latency", "p95_ms"]),
            100,
            100,
        ),
        activeStatus(
            metricNumber(last, ["input_latency", "sample_count"]),
            metricNumber(last, ["copies", "session_full_frame_count"]),
            metricNumber(last, ["input_latency", "p95_ms"]),
            101,
            107,
        ),
    )
}

describe("Spec 76 graphics acceptance", () => {
    test("accepts bounded input latency with zero full-frame copy growth", () => {
        const report = analyzeActive(metrics(0.8, 10, 4), metrics(0.9, 11, 4))

        expect(report.ok).toBe(true)
        expect(report.summary.maxInputP95Ms).toBe(0.9)
        expect(report.summary.inputSampleCountDelta).toBe(1)
        expect(report.summary.sessionFullFrameCopyCountDelta).toBe(0)
    })

    test("rejects input latency above the one millisecond contract", () => {
        const report = analyzeActive(metrics(0.8, 10, 4), metrics(1.01, 11, 4))

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "input-stage p95 ms",
            ok: false,
            actual: 1.1,
            limit: 1,
        })
    })

    test("rejects session full-frame copy growth", () => {
        const report = analyzeActive(metrics(0.8, 10, 4), metrics(0.9, 11, 5))

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "session full-frame-copy count delta",
            ok: false,
            actual: 1,
            limit: 0,
        })
    })

    test("does not count a warmup copy as steady-state growth", () => {
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 5), metrics(0.9, 11, 5)),
            [],
            activeStatus(10, 4, 0.8, 100, 100),
            activeStatus(11, 5, 0.9, 101, 107),
            activeStatus(10, 5, 0.8, 100, 105),
        )

        expect(report.ok).toBe(true)
        expect(report.summary.sessionFullFrameCopyCountDelta).toBe(0)
    })

    test("counts a first-interval copy when warmup is disabled", () => {
        const noWarmup = {
            ...specConfig,
            durationMs: 3_000,
            warmupMs: 0,
        }
        const first = metrics(0.8, 11, 1)
        const last = metrics(0.9, 12, 1)
        const report = analyze(
            noWarmup,
            [
                { receivedAtMs: 500, data: { ...first, timeline: { frame_token: 101 } } },
                { receivedAtMs: 1_500, data: { ...last, timeline: { frame_token: 102 } } },
                { receivedAtMs: 2_500, data: { ...last, timeline: { frame_token: 103 } } },
            ],
            [],
            activeStatus(10, 0, 0.8, 100, 100),
            activeStatus(12, 1, 0.9, 101, 103),
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "session full-frame-copy count delta",
            ok: false,
            actual: 1,
            limit: 0,
        })
    })

    test("fails closed when required session telemetry is absent", () => {
        const before = activeStatus(10, 4, 0.8, 100, 100)
        const after = activeStatus(11, 4, 0.9, 101, 107)
        const inputStage = valueAt(after, ["session_performance", "input_stage"]) as JsonObject
        delete inputStage.cumulative_histogram
        expect(() =>
            analyze(
                specConfig,
                samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
                [],
                before,
                after,
            ),
        ).toThrow("Missing input-stage cumulative histogram")
    })

    test("rejects a steady window without new input samples", () => {
        const report = analyzeActive(metrics(0.8, 10, 4), metrics(0.9, 10, 4))

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "input-stage sample growth",
            ok: false,
            actual: 0,
            limit: ">= 1",
        })
    })

    test("does not count a warmup input sample as steady growth", () => {
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 11, 4), metrics(0.9, 11, 4)),
            [],
            activeStatus(10, 4, 0.8, 100, 100),
            activeStatus(11, 4, 0.9, 101, 107),
            activeStatus(11, 4, 0.9, 100, 105),
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "input-stage sample growth",
            ok: false,
            actual: 0,
            limit: ">= 1",
        })
    })

    test("rejects an observation without a pre-warmup baseline", () => {
        const report = analyze(
            specConfig,
            [{ receivedAtMs: 6_000, data: metrics(0.9, 11, 4) }],
            [],
            activeStatus(10, 4, 0.8, 100, 100),
            activeStatus(11, 4, 0.9, 101, 107),
        )

        expect(report.ok).toBe(false)
        expect(report.checks[0]?.name).toBe("warmup baseline and steady metrics")
    })

    test("fails closed when a cumulative session counter regresses", () => {
        expect(() => analyzeActive(metrics(0.8, 11, 4), metrics(0.9, 10, 4))).toThrow(
            "Cumulative metric regressed: session_performance.input_stage.sample_count",
        )
    })

    test("fails closed when the full-frame copy counter regresses", () => {
        expect(() => analyzeActive(metrics(0.8, 10, 5), metrics(0.9, 11, 4))).toThrow(
            "Cumulative metric regressed: session_performance.full_frame_cpu_copies.count",
        )
    })

    test("keeps unordered WS counters out of authoritative REST deltas", () => {
        const report = analyzeActive(metrics(0.8, 10, 4), metrics(0.9, 11, 4), [
            { receivedAtMs: 5_250, data: metrics(0.9, 9, 3) },
        ])

        expect(report.ok).toBe(true)
        expect(report.summary.inputSampleCountDelta).toBe(1)
        expect(report.summary.sessionFullFrameCopyCountDelta).toBe(0)
    })

    test("rejects a truncated metrics observation", () => {
        const report = analyze(
            { ...specConfig, durationMs: 60_000 },
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            activeStatus(10, 4, 0.8, 100, 100),
            activeStatus(11, 4, 0.9, 101, 160),
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "observation window coverage ms",
            ok: false,
            actual: 6_000,
            limit: ">= 58000",
        })
    })

    test("does not shorten acceptance when the warmup checkpoint responds late", () => {
        const first = metrics(0.8, 10, 4)
        const last = metrics(0.9, 11, 4)
        const report = analyze(
            { ...specConfig, durationMs: 60_000 },
            [
                { receivedAtMs: 4_000, data: { ...first, timeline: { frame_token: 100 } } },
                { receivedAtMs: 59_000, data: { ...last, timeline: { frame_token: 101 } } },
                { receivedAtMs: 60_000, data: { ...last, timeline: { frame_token: 102 } } },
            ],
            [],
            activeStatus(10, 4, 0.8, 100, 100),
            activeStatus(11, 4, 0.9, 101, 160),
            activeStatus(10, 4, 0.8, 100, 105),
            58_000,
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "observation window coverage ms",
            ok: false,
            actual: 60_000,
            limit: ">= 111000",
        })
        expect(report.checks).toContainEqual({
            name: "steady metrics samples",
            ok: false,
            actual: 2,
            limit: ">= 54",
        })
    })

    test("seals the final copy interval from REST status", () => {
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            activeStatus(10, 4, 0.8, 100, 100),
            activeStatus(12, 5, 0.9, 101, 107),
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "session full-frame-copy count delta",
            ok: false,
            actual: 1,
            limit: 0,
        })
    })

    test("seals the final input latency interval from REST status", () => {
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            activeStatus(10, 4, 0.8, 100, 100),
            activeStatus(12, 4, 1.01, 101, 107),
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "input-stage p95 ms",
            ok: false,
            actual: 1.1,
            limit: 1,
        })
    })

    test("isolates observation latency from lifetime history", () => {
        const before = activeStatus(100_000, 4, 0.1, 100, 100)
        const after = activeStatus(100_100, 4, 0.1, 101, 107)
        setCumulativeHistogram(before, [[1, 100_000]], 100)
        setCumulativeHistogram(
            after,
            [
                [1, 100_000],
                [11, 100],
            ],
            101,
        )
        const report = analyze(
            specConfig,
            samples(metrics(0.1, 100_000, 4), metrics(0.1, 100_100, 4)),
            [],
            before,
            after,
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "input-stage p95 ms",
            ok: false,
            actual: 1.1,
            limit: 1,
        })
    })

    test("rejects a nonnative or inactive workload", () => {
        const before = activeStatus(10, 4, 0.8, 100, 100)
        const after = activeStatus(11, 4, 0.9, 101, 107)
        for (const status of [before, after]) {
            const screen = (valueAt(status, ["input", "sources"]) as JsonObject[])[0]
            if (screen) {
                screen.platform = { type: "macos_screen", telemetry: { publication_path: "cpu" } }
            }
        }
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            before,
            after,
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "native screen active",
            ok: false,
            actual: "inactive/inactive",
            limit: "active/active",
        })
    })

    test("starts workload identity checks after warmup", () => {
        const before = activeStatus(10, 4, 0.8, 90, 100)
        const baseline = activeStatus(10, 4, 0.8, 100, 105)
        const after = activeStatus(11, 4, 0.9, 101, 107)
        const beforeScreen = (valueAt(before, ["input", "sources"]) as JsonObject[])[0]
        if (beforeScreen) {
            beforeScreen.state = "stopped"
            beforeScreen.source_graph_generation = 8
        }
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            before,
            after,
            baseline,
        )

        expect(report.ok).toBe(true)
    })

    test("rejects an inactive host-input source", () => {
        const before = activeStatus(10, 4, 0.8, 100, 100)
        const after = activeStatus(11, 4, 0.9, 101, 107)
        for (const status of [before, after]) {
            const interaction = (valueAt(status, ["input", "sources"]) as JsonObject[])[2]
            if (interaction) {
                interaction.state = "stopped"
            }
        }
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            before,
            after,
        )

        expect(report.ok).toBe(false)
        expect(report.checks).toContainEqual({
            name: "interaction input active",
            ok: false,
            actual: "inactive/inactive",
            limit: "active/active",
        })
    })

    test("rejects a stale native screen source", () => {
        const before = activeStatus(10, 4, 0.8, 100, 100)
        const after = activeStatus(11, 4, 0.9, 101, 107)
        for (const status of [before, after]) {
            const screen = (valueAt(status, ["input", "sources"]) as JsonObject[])[0]
            if (screen) {
                screen.freshness = "stale"
            }
        }
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            before,
            after,
        )

        expect(report.ok).toBe(false)
        expect(report.checks.find((check) => check.name === "native screen active")?.ok).toBe(false)
    })

    test("rejects a native screen without freshness tracking", () => {
        const before = activeStatus(10, 4, 0.8, 100, 100)
        const after = activeStatus(11, 4, 0.9, 101, 107)
        for (const status of [before, after]) {
            const screen = (valueAt(status, ["input", "sources"]) as JsonObject[])[0]
            if (screen) {
                screen.freshness = "not_applicable"
            }
        }
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            before,
            after,
        )

        expect(report.ok).toBe(false)
        expect(report.checks.find((check) => check.name === "native screen active")?.ok).toBe(false)
    })

    test("rejects a source graph replacement during acceptance", () => {
        const before = activeStatus(10, 4, 0.8, 100, 100)
        const after = activeStatus(11, 4, 0.9, 101, 107)
        const screen = (valueAt(after, ["input", "sources"]) as JsonObject[])[0]
        if (screen) {
            screen.source_graph_generation = 10
        }
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            before,
            after,
        )

        expect(report.ok).toBe(false)
        expect(report.checks.find((check) => check.name === "native screen publication growth")?.ok).toBe(false)
    })

    test("rejects a daemon restart during acceptance", () => {
        const report = analyze(
            specConfig,
            samples(metrics(0.8, 10, 4), metrics(0.9, 11, 4)),
            [],
            activeStatus(10, 4, 0.8, 100, 100),
            activeStatus(11, 4, 0.9, 101, 2),
        )

        expect(report.ok).toBe(false)
        expect(report.checks.find((check) => check.name === "daemon session continuity")?.ok).toBe(false)
    })

    test("keeps the generic graphics soak cross-platform", () => {
        const genericConfig = { ...defaults, durationMs: 7_000 }
        const report = analyze(
            genericConfig,
            samples(metrics(0, 0, 0), metrics(0, 0, 0)),
            [],
            {},
            {},
            {},
            0,
        )

        expect(report.ok).toBe(true)
        expect(report.checks.some((check) => check.name === "native screen active")).toBe(false)
        expect(report.summary.maxInputP95Ms).toBeUndefined()
        expect(report.summary.inputSampleCountDelta).toBeUndefined()
        expect(report.summary.sessionFullFrameCopyCountDelta).toBeUndefined()
    })
})

function metricNumber(root: JsonObject, path: string[]): number {
    const value = valueAt(root, path)
    return typeof value === "number" ? value : 0
}

function valueAt(root: JsonObject, path: string[]): unknown {
    let current: unknown = root
    for (const part of path) {
        if (!current || typeof current !== "object" || Array.isArray(current)) {
            return undefined
        }
        current = (current as JsonObject)[part]
    }
    return current
}
