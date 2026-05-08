#!/usr/bin/env bun

type JsonObject = Record<string, unknown>

type Config = {
    daemon: string
    durationMs: number
    intervalMs: number
    warmupMs: number
    minFpsRatio: number
    maxBackpressureFrames: number
    maxWriteFailureDelta: number
    maxRetryDelta: number
    maxOutputErrorDelta: number
    maxFullFrameCopyFrames: number
    maxFrameCopyCount: number
    maxPoolSaturationDelta: number
    maxEffectFallbackDelta: number
    maxServoStallDelta: number
    maxServoBreakerDelta: number
    maxServoFailureDelta: number
    maxServoQueueWaitMs: number
    maxDisplayLanePriorityWaitMs: number
    out?: string
    json: boolean
}

type MetricSample = {
    receivedAtMs: number
    data: JsonObject
}

type BackpressureSample = {
    channel: string
    droppedFrames: number
    suggestedFps: number
}

type Check = {
    name: string
    ok: boolean
    actual: number | string
    limit: number | string
}

type Report = {
    ok: boolean
    daemon: string
    durationMs: number
    sampleCount: number
    backpressure: BackpressureSample[]
    summary: Record<string, number | string>
    checks: Check[]
}

const palette = {
    purple: "\x1b[38;2;225;53;255m",
    cyan: "\x1b[38;2;128;255;234m",
    coral: "\x1b[38;2;255;106;193m",
    yellow: "\x1b[38;2;241;250;140m",
    green: "\x1b[38;2;80;250;123m",
    red: "\x1b[38;2;255;99;99m",
    bold: "\x1b[1m",
    reset: "\x1b[0m",
}

const defaults: Config = {
    daemon: "http://127.0.0.1:9420",
    durationMs: 60_000,
    intervalMs: 1_000,
    warmupMs: 5_000,
    minFpsRatio: 0.75,
    maxBackpressureFrames: 0,
    maxWriteFailureDelta: 0,
    maxRetryDelta: 0,
    maxOutputErrorDelta: 0,
    maxFullFrameCopyFrames: 0,
    maxFrameCopyCount: 0,
    maxPoolSaturationDelta: 0,
    maxEffectFallbackDelta: 0,
    maxServoStallDelta: 0,
    maxServoBreakerDelta: 0,
    maxServoFailureDelta: 0,
    maxServoQueueWaitMs: 100,
    maxDisplayLanePriorityWaitMs: 16.7,
    json: false,
}

function usage(): string {
    return `Hypercolor graphics pipeline soak

Observes an already-running daemon. It does not start or restart services.

Usage:
  bun scripts/graphics-pipeline-soak.ts [options]
  just graphics-soak -- [options]

Options:
  --daemon <url>                       Daemon base URL [${defaults.daemon}]
  --duration-ms <ms>                   Observation window [${defaults.durationMs}]
  --duration <30s|2m|1500ms>           Friendlier duration syntax
  --interval-ms <ms>                   Metrics interval [${defaults.intervalMs}]
  --warmup-ms <ms>                     Exclude initial samples from steady-state checks [${defaults.warmupMs}]
  --min-fps-ratio <ratio>              Median actual FPS must stay above target * ratio [${defaults.minFpsRatio}]
  --max-backpressure-frames <n>        Maximum dropped WS frames [${defaults.maxBackpressureFrames}]
  --max-write-failure-delta <n>        Maximum display write failures [${defaults.maxWriteFailureDelta}]
  --max-retry-delta <n>                Maximum display retry attempts [${defaults.maxRetryDelta}]
  --max-output-error-delta <n>         Maximum render pacing output-error frames [${defaults.maxOutputErrorDelta}]
  --max-full-frame-copy-frames <n>     Maximum pacing full-frame-copy frames [${defaults.maxFullFrameCopyFrames}]
  --max-frame-copy-count <n>           Maximum per-frame full-copy count [${defaults.maxFrameCopyCount}]
  --max-pool-saturation-delta <n>      Maximum render-surface pool saturation reallocs [${defaults.maxPoolSaturationDelta}]
  --max-effect-fallback-delta <n>      Maximum effect fallbacks [${defaults.maxEffectFallbackDelta}]
  --max-servo-stall-delta <n>          Maximum Servo soft stalls [${defaults.maxServoStallDelta}]
  --max-servo-breaker-delta <n>        Maximum Servo breaker opens [${defaults.maxServoBreakerDelta}]
  --max-servo-failure-delta <n>        Maximum total Servo lifecycle failures [${defaults.maxServoFailureDelta}]
  --max-servo-queue-wait-ms <ms>       Maximum Servo render queue wait [${defaults.maxServoQueueWaitMs}]
  --max-display-lane-priority-wait-ms <ms>
                                      Maximum LED-priority display wait [${defaults.maxDisplayLanePriorityWaitMs}]
  --out <path>                         Write JSON report
  --json                               Print JSON only
  --help                               Show this help
`
}

function parseArgs(argv: string[]): Config {
    const config = { ...defaults }

    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index]
        if (arg === "--help" || arg === "-h") {
            console.log(usage())
            process.exit(0)
        }

        if (arg === "--json") {
            config.json = true
            continue
        }

        const value = argv[index + 1]
        if (!value || value.startsWith("--")) {
            throw new Error(`${arg} expects a value`)
        }
        index += 1

        switch (arg) {
            case "--daemon":
                config.daemon = value
                break
            case "--duration-ms":
                config.durationMs = parsePositiveInt(arg, value)
                break
            case "--duration":
                config.durationMs = parseDuration(value)
                break
            case "--interval-ms":
                config.intervalMs = parsePositiveInt(arg, value)
                break
            case "--warmup-ms":
                config.warmupMs = parseNonNegativeInt(arg, value)
                break
            case "--min-fps-ratio":
                config.minFpsRatio = parseNonNegativeNumber(arg, value)
                break
            case "--max-backpressure-frames":
                config.maxBackpressureFrames = parseNonNegativeInt(arg, value)
                break
            case "--max-write-failure-delta":
                config.maxWriteFailureDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-retry-delta":
                config.maxRetryDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-output-error-delta":
                config.maxOutputErrorDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-full-frame-copy-frames":
                config.maxFullFrameCopyFrames = parseNonNegativeInt(arg, value)
                break
            case "--max-frame-copy-count":
                config.maxFrameCopyCount = parseNonNegativeInt(arg, value)
                break
            case "--max-pool-saturation-delta":
                config.maxPoolSaturationDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-effect-fallback-delta":
                config.maxEffectFallbackDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-servo-stall-delta":
                config.maxServoStallDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-servo-breaker-delta":
                config.maxServoBreakerDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-servo-failure-delta":
                config.maxServoFailureDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-servo-queue-wait-ms":
                config.maxServoQueueWaitMs = parseNonNegativeNumber(arg, value)
                break
            case "--max-display-lane-priority-wait-ms":
                config.maxDisplayLanePriorityWaitMs = parseNonNegativeNumber(arg, value)
                break
            case "--out":
                config.out = value
                break
            default:
                throw new Error(`Unknown option: ${arg}`)
        }
    }

    if (config.warmupMs >= config.durationMs) {
        throw new Error("--warmup-ms must be smaller than the observation duration")
    }

    return config
}

function parsePositiveInt(name: string, value: string): number {
    const parsed = Number(value)
    if (!Number.isInteger(parsed) || parsed <= 0) {
        throw new Error(`${name} must be a positive integer`)
    }
    return parsed
}

function parseNonNegativeInt(name: string, value: string): number {
    const parsed = Number(value)
    if (!Number.isInteger(parsed) || parsed < 0) {
        throw new Error(`${name} must be a non-negative integer`)
    }
    return parsed
}

function parseNonNegativeNumber(name: string, value: string): number {
    const parsed = Number(value)
    if (!Number.isFinite(parsed) || parsed < 0) {
        throw new Error(`${name} must be a non-negative number`)
    }
    return parsed
}

function parseDuration(value: string): number {
    const match = value.match(/^(\d+(?:\.\d+)?)(ms|s|m)$/)
    if (!match) {
        throw new Error("--duration must look like 1500ms, 30s, or 2m")
    }
    const amount = Number(match[1])
    const unit = match[2]
    const multiplier = unit === "ms" ? 1 : unit === "s" ? 1_000 : 60_000
    const durationMs = Math.round(amount * multiplier)
    if (durationMs <= 0) {
        throw new Error("--duration must be positive")
    }
    return durationMs
}

function apiPrefix(raw: string): string {
    const url = new URL(raw)
    const path = url.pathname.replace(/\/+$/, "")
    const prefix = path.endsWith("/api/v1") ? path : `${path}/api/v1`
    return `${url.origin}${prefix.replace(/^\/+/, "/")}`
}

function wsEndpoint(raw: string): string {
    const prefix = new URL(apiPrefix(raw))
    prefix.protocol = prefix.protocol === "https:" ? "wss:" : "ws:"
    prefix.pathname = `${prefix.pathname.replace(/\/+$/, "")}/ws`
    return prefix.toString()
}

async function assertDaemonReachable(config: Config): Promise<void> {
    const statusUrl = `${apiPrefix(config.daemon)}/status`
    let response: Response
    try {
        response = await fetch(statusUrl)
    } catch (error) {
        throw new Error(`Daemon is not reachable at ${statusUrl}: ${errorMessage(error)}`)
    }
    if (!response.ok) {
        throw new Error(`Daemon status check failed at ${statusUrl}: HTTP ${response.status}`)
    }
}

async function observe(config: Config): Promise<{ samples: MetricSample[]; backpressure: BackpressureSample[] }> {
    await assertDaemonReachable(config)

    const samples: MetricSample[] = []
    const backpressure: BackpressureSample[] = []
    const endpoint = wsEndpoint(config.daemon)
    const startedAtMs = Date.now()

    return await new Promise((resolve, reject) => {
        const socket = new WebSocket(endpoint)
        let settled = false
        let sawOpen = false

        const finish = () => {
            if (settled) {
                return
            }
            settled = true
            socket.close()
            resolve({ samples, backpressure })
        }

        const fail = (error: Error) => {
            if (settled) {
                return
            }
            settled = true
            socket.close()
            reject(error)
        }

        const openTimer = setTimeout(() => {
            if (!sawOpen) {
                fail(new Error(`Timed out opening ${endpoint}`))
            }
        }, 5_000)

        const finishTimer = setTimeout(finish, config.durationMs)

        socket.onopen = () => {
            sawOpen = true
            clearTimeout(openTimer)
            socket.send(
                JSON.stringify({
                    type: "subscribe",
                    channels: ["metrics"],
                    config: { metrics: { interval_ms: config.intervalMs } },
                }),
            )
        }

        socket.onerror = () => {
            clearTimeout(openTimer)
            clearTimeout(finishTimer)
            fail(new Error(`WebSocket error while observing ${endpoint}`))
        }

        socket.onmessage = (event: MessageEvent) => {
            const text = typeof event.data === "string" ? event.data : ""
            if (!text) {
                return
            }

            let message: JsonObject
            try {
                message = JSON.parse(text)
            } catch {
                return
            }

            const type = stringAt(message, ["type"])
            if (type === "metrics") {
                const data = objectAt(message, ["data"])
                if (data) {
                    samples.push({ receivedAtMs: Date.now() - startedAtMs, data })
                }
                return
            }

            if (type === "backpressure") {
                backpressure.push({
                    channel: stringAt(message, ["channel"]),
                    droppedFrames: numberAt(message, ["dropped_frames"]),
                    suggestedFps: numberAt(message, ["suggested_fps"]),
                })
            }
        }

        process.once("SIGINT", finish)
    })
}

function analyze(config: Config, samples: MetricSample[], backpressure: BackpressureSample[]): Report {
    const steadySamples = samples.filter((sample) => sample.receivedAtMs >= config.warmupMs)
    const observed = steadySamples.length > 0 ? steadySamples : samples
    const first = observed[0]
    const last = observed.at(-1)
    const checks: Check[] = []

    if (!first || !last) {
        return {
            ok: false,
            daemon: config.daemon,
            durationMs: config.durationMs,
            sampleCount: samples.length,
            backpressure,
            summary: {},
            checks: [{ name: "metrics samples", ok: false, actual: 0, limit: "> 0" }],
        }
    }

    const fpsValues = observed.map((sample) => numberAt(sample.data, ["fps", "actual"])).filter((value) => value > 0)
    const targetFps = numberAt(last.data, ["fps", "target"])
    const medianFps = median(fpsValues)
    const minFps = targetFps > 0 ? targetFps * config.minFpsRatio : 0
    const backpressureFrames = backpressure.reduce((total, item) => total + item.droppedFrames, 0)
    const servoFailureDelta =
        delta(first.data, last.data, ["effect_health", "servo_session_create_failures_total"]) +
        delta(first.data, last.data, ["effect_health", "servo_page_load_failures_total"]) +
        delta(first.data, last.data, ["effect_health", "servo_detached_destroy_failures_total"])
    const poolSaturationDelta =
        delta(first.data, last.data, ["render_surfaces", "preview_pool_saturation_reallocs"]) +
        delta(first.data, last.data, ["render_surfaces", "direct_pool_saturation_reallocs"])
    const frameP95BudgetMs = targetFps > 0 ? (1_000 / targetFps) * 1.25 : Number.POSITIVE_INFINITY
    const maxFrameP95Ms = maxAt(observed, ["frame_time", "p95_ms"])

    checks.push(checkAtLeast("median fps", round(medianFps), round(minFps)))
    checks.push(checkAtMost("frame p95 ms", round(maxFrameP95Ms), round(frameP95BudgetMs)))
    checks.push(checkAtMost("backpressure dropped frames", backpressureFrames, config.maxBackpressureFrames))
    checks.push(
        checkAtMost(
            "display write failure delta",
            delta(first.data, last.data, ["display_output", "write_failures_total"]),
            config.maxWriteFailureDelta,
        ),
    )
    checks.push(
        checkAtMost(
            "display retry delta",
            delta(first.data, last.data, ["display_output", "retry_attempts_total"]),
            config.maxRetryDelta,
        ),
    )
    checks.push(
        checkAtMost(
            "pacing output error frames",
            delta(first.data, last.data, ["pacing", "output_error_frames"]),
            config.maxOutputErrorDelta,
        ),
    )
    checks.push(
        checkAtMost(
            "pacing full-frame-copy frames",
            maxAt(observed, ["pacing", "full_frame_copy_frames"]),
            config.maxFullFrameCopyFrames,
        ),
    )
    checks.push(checkAtMost("per-frame full-copy count", maxAt(observed, ["copies", "full_frame_count"]), config.maxFrameCopyCount))
    checks.push(checkAtMost("surface pool saturation reallocs", poolSaturationDelta, config.maxPoolSaturationDelta))
    checks.push(
        checkAtMost(
            "effect fallback delta",
            delta(first.data, last.data, ["effect_health", "fallbacks_applied_total"]),
            config.maxEffectFallbackDelta,
        ),
    )
    checks.push(
        checkAtMost(
            "Servo soft stall delta",
            delta(first.data, last.data, ["effect_health", "servo_soft_stalls_total"]),
            config.maxServoStallDelta,
        ),
    )
    checks.push(
        checkAtMost(
            "Servo breaker delta",
            delta(first.data, last.data, ["effect_health", "servo_breaker_opens_total"]),
            config.maxServoBreakerDelta,
        ),
    )
    checks.push(checkAtMost("Servo lifecycle failure delta", servoFailureDelta, config.maxServoFailureDelta))
    checks.push(
        checkAtMost(
            "Servo render queue wait growth ms",
            maxIncreaseAt(observed, ["effect_health", "servo_render_queue_wait_max_ms"]),
            config.maxServoQueueWaitMs,
        ),
    )
    checks.push(
        checkAtMost(
            "display lane LED-priority wait growth ms",
            requiredMaxIncreaseAt(observed, [
                "display_output",
                "display_lane",
                "display_led_priority_wait_max_ms",
            ]),
            config.maxDisplayLanePriorityWaitMs,
        ),
    )

    const summary = {
        targetFps,
        medianFps: round(medianFps),
        maxFrameP95Ms: round(maxFrameP95Ms),
        backpressureFrames,
        writeFailureDelta: delta(first.data, last.data, ["display_output", "write_failures_total"]),
        retryDelta: delta(first.data, last.data, ["display_output", "retry_attempts_total"]),
        outputErrorFrames: delta(first.data, last.data, ["pacing", "output_error_frames"]),
        maxFullFrameCopyFrames: maxAt(observed, ["pacing", "full_frame_copy_frames"]),
        maxFrameCopyCount: maxAt(observed, ["copies", "full_frame_count"]),
        poolSaturationDelta,
        effectFallbackDelta: delta(first.data, last.data, ["effect_health", "fallbacks_applied_total"]),
        servoFailureDelta,
        servoQueueWaitMaxMs: round(maxAt(observed, ["effect_health", "servo_render_queue_wait_max_ms"])),
        servoQueueWaitMaxGrowthMs: round(
            maxIncreaseAt(observed, ["effect_health", "servo_render_queue_wait_max_ms"]),
        ),
        displayLanePriorityWaitMaxMs: round(
            requiredMaxAt(observed, [
                "display_output",
                "display_lane",
                "display_led_priority_wait_max_ms",
            ]),
        ),
        displayLanePriorityWaitMaxGrowthMs: round(
            requiredMaxIncreaseAt(observed, [
                "display_output",
                "display_lane",
                "display_led_priority_wait_max_ms",
            ]),
        ),
    }

    return {
        ok: checks.every((check) => check.ok),
        daemon: config.daemon,
        durationMs: config.durationMs,
        sampleCount: samples.length,
        backpressure,
        summary,
        checks,
    }
}

function checkAtMost(name: string, actual: number, limit: number): Check {
    return { name, ok: actual <= limit, actual: round(actual), limit: round(limit) }
}

function checkAtLeast(name: string, actual: number, limit: number): Check {
    return { name, ok: actual >= limit, actual: round(actual), limit: `>= ${round(limit)}` }
}

function delta(first: JsonObject, last: JsonObject, path: string[]): number {
    return Math.max(0, numberAt(last, path) - numberAt(first, path))
}

function maxAt(samples: MetricSample[], path: string[]): number {
    return samples.reduce((max, sample) => Math.max(max, numberAt(sample.data, path)), 0)
}

function requiredMaxAt(samples: MetricSample[], path: string[]): number {
    return samples.reduce((max, sample) => Math.max(max, requiredNumberAt(sample.data, path)), 0)
}

function maxIncreaseAt(samples: MetricSample[], path: string[]): number {
    const first = samples[0]
    if (!first) {
        return 0
    }
    return Math.max(0, maxAt(samples, path) - numberAt(first.data, path))
}

function requiredMaxIncreaseAt(samples: MetricSample[], path: string[]): number {
    const first = samples[0]
    if (!first) {
        return 0
    }
    return Math.max(0, requiredMaxAt(samples, path) - requiredNumberAt(first.data, path))
}

function median(values: number[]): number {
    if (values.length === 0) {
        return 0
    }
    const sorted = [...values].sort((left, right) => left - right)
    const middle = Math.floor(sorted.length / 2)
    if (sorted.length % 2 === 1) {
        return sorted[middle]
    }
    return (sorted[middle - 1] + sorted[middle]) / 2
}

function objectAt(root: JsonObject, path: string[]): JsonObject | undefined {
    const value = valueAt(root, path)
    return value && typeof value === "object" && !Array.isArray(value) ? (value as JsonObject) : undefined
}

function numberAt(root: JsonObject, path: string[]): number {
    const value = valueAt(root, path)
    return typeof value === "number" && Number.isFinite(value) ? value : 0
}

function requiredNumberAt(root: JsonObject, path: string[]): number {
    const value = valueAt(root, path)
    if (typeof value !== "number" || !Number.isFinite(value)) {
        throw new Error(`Missing numeric metric: ${path.join(".")}`)
    }
    return value
}

function stringAt(root: JsonObject, path: string[]): string {
    const value = valueAt(root, path)
    return typeof value === "string" ? value : ""
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

function round(value: number): number {
    return Math.round(value * 100) / 100
}

function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error)
}

function printReport(report: Report): void {
    const status = report.ok ? `${palette.green}PASS${palette.reset}` : `${palette.red}FAIL${palette.reset}`
    console.log(`${palette.bold}${palette.purple}Hypercolor graphics soak${palette.reset} ${status}`)
    console.log(`${palette.cyan}${report.daemon}${palette.reset} · ${report.sampleCount} samples · ${report.durationMs}ms`)
    console.log("")
    for (const check of report.checks) {
        const marker = check.ok ? `${palette.green}✓${palette.reset}` : `${palette.red}✗${palette.reset}`
        const actual = check.ok ? `${check.actual}` : `${palette.coral}${check.actual}${palette.reset}`
        console.log(`${marker} ${check.name}: ${actual} / ${check.limit}`)
    }
}

async function main(): Promise<void> {
    const config = parseArgs(process.argv.slice(2))
    const { samples, backpressure } = await observe(config)
    const report = analyze(config, samples, backpressure)
    const json = `${JSON.stringify(report, null, 2)}\n`

    if (config.out) {
        await Bun.write(config.out, json)
    }

    if (config.json) {
        process.stdout.write(json)
    } else {
        printReport(report)
    }

    process.exit(report.ok ? 0 : 1)
}

main().catch((error) => {
    console.error(`${palette.red}graphics soak failed:${palette.reset} ${errorMessage(error)}`)
    process.exit(1)
})
