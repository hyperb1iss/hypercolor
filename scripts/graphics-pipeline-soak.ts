#!/usr/bin/env bun

export type JsonObject = Record<string, unknown>

type Config = {
    daemon: string
    durationMs: number
    intervalMs: number
    warmupMs: number
    requireMacosNativeCapture: boolean
    minFpsRatio: number
    maxInputP95Ms: number
    maxBackpressureFrames: number
    maxWriteFailureDelta: number
    maxRetryDelta: number
    maxOutputErrorDelta: number
    maxFullFrameCopyFrames: number
    maxFrameCopyCount: number
    maxSessionFullFrameCopyCountDelta: number
    maxPoolSaturationDelta: number
    maxEffectFallbackDelta: number
    maxProducerGpuReadbackFailureDelta: number
    maxGpuCpuMaterializationBlockDelta: number
    maxGpuReadbackFailedFrames: number
    maxServoStallDelta: number
    maxServoBreakerDelta: number
    maxServoFailureDelta: number
    maxServoQueueWaitMs: number
    maxDisplayFinalizeMissDelta: number
    maxDisplayFinalizeBlockingWaitMs: number
    maxDisplayFinalizeSurfaceReallocDelta: number
    maxDisplayLanePriorityWaitMs: number
    out?: string
    json: boolean
}

export type MetricSample = {
    receivedAtMs: number
    data: JsonObject
}

type BackpressureSample = {
    droppedFrames: number
    suggestedFps: number
    topic: string
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

export const defaults: Config = {
    daemon: "http://127.0.0.1:9420",
    durationMs: 60_000,
    intervalMs: 1_000,
    warmupMs: 5_000,
    requireMacosNativeCapture: false,
    minFpsRatio: 0.75,
    maxInputP95Ms: 1,
    maxBackpressureFrames: 0,
    maxWriteFailureDelta: 0,
    maxRetryDelta: 0,
    maxOutputErrorDelta: 0,
    maxFullFrameCopyFrames: 0,
    maxFrameCopyCount: 0,
    maxSessionFullFrameCopyCountDelta: 0,
    maxPoolSaturationDelta: 0,
    maxEffectFallbackDelta: 0,
    maxProducerGpuReadbackFailureDelta: 0,
    maxGpuCpuMaterializationBlockDelta: 0,
    maxGpuReadbackFailedFrames: 0,
    maxServoStallDelta: 0,
    maxServoBreakerDelta: 0,
    maxServoFailureDelta: 0,
    maxServoQueueWaitMs: 100,
    maxDisplayFinalizeMissDelta: 0,
    maxDisplayFinalizeBlockingWaitMs: 0,
    maxDisplayFinalizeSurfaceReallocDelta: 0,
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
  --macos-native-capture               Enforce the Spec 76 native screen, audio, and input workload
  --min-fps-ratio <ratio>              Median actual FPS must stay above target * ratio [${defaults.minFpsRatio}]
  --max-input-p95-ms <ms>              Maximum session input-stage p95 [${defaults.maxInputP95Ms}]
  --max-backpressure-frames <n>        Maximum dropped WS frames [${defaults.maxBackpressureFrames}]
  --max-write-failure-delta <n>        Maximum display write failures [${defaults.maxWriteFailureDelta}]
  --max-retry-delta <n>                Maximum display retry attempts [${defaults.maxRetryDelta}]
  --max-output-error-delta <n>         Maximum render pacing output-error frames [${defaults.maxOutputErrorDelta}]
  --max-full-frame-copy-frames <n>     Maximum pacing full-frame-copy frames [${defaults.maxFullFrameCopyFrames}]
  --max-frame-copy-count <n>           Maximum per-frame full-copy count [${defaults.maxFrameCopyCount}]
  --max-session-full-frame-copy-count-delta <n>
                                      Maximum session full-frame-copy count growth
                                      [${defaults.maxSessionFullFrameCopyCountDelta}]
  --max-pool-saturation-delta <n>      Maximum render-surface pool saturation reallocs [${defaults.maxPoolSaturationDelta}]
  --max-effect-fallback-delta <n>      Maximum effect fallbacks [${defaults.maxEffectFallbackDelta}]
  --max-producer-gpu-readback-failure-delta <n>
                                      Maximum GPU producer residency failures [${defaults.maxProducerGpuReadbackFailureDelta}]
  --max-gpu-cpu-materialization-block-delta <n>
                                      Maximum forbidden GPU-to-CPU materialization attempts [${defaults.maxGpuCpuMaterializationBlockDelta}]
  --max-gpu-readback-failed-frames <n>
                                      Maximum rolling GPU residency-failure frames [${defaults.maxGpuReadbackFailedFrames}]
  --max-servo-stall-delta <n>          Maximum Servo soft stalls [${defaults.maxServoStallDelta}]
  --max-servo-breaker-delta <n>        Maximum Servo breaker opens [${defaults.maxServoBreakerDelta}]
  --max-servo-failure-delta <n>        Maximum total Servo lifecycle failures [${defaults.maxServoFailureDelta}]
  --max-servo-queue-wait-ms <ms>       Maximum Servo render queue wait [${defaults.maxServoQueueWaitMs}]
  --max-display-finalize-miss-delta <n>
                                      Maximum GPU display finalizer misses after warmup [${defaults.maxDisplayFinalizeMissDelta}]
  --max-display-finalize-blocking-wait-ms <ms>
                                      Maximum GPU display finalizer blocking wait [${defaults.maxDisplayFinalizeBlockingWaitMs}]
  --max-display-finalize-surface-realloc-delta <n>
                                      Maximum GPU display finalizer surface reallocs after warmup [${defaults.maxDisplayFinalizeSurfaceReallocDelta}]
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
        if (arg === "--") {
            continue
        }
        if (arg === "--help" || arg === "-h") {
            console.log(usage())
            process.exit(0)
        }

        if (arg === "--json") {
            config.json = true
            continue
        }
        if (arg === "--macos-native-capture") {
            config.requireMacosNativeCapture = true
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
            case "--max-input-p95-ms":
                config.maxInputP95Ms = parseNonNegativeNumber(arg, value)
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
            case "--max-session-full-frame-copy-count-delta":
                config.maxSessionFullFrameCopyCountDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-pool-saturation-delta":
                config.maxPoolSaturationDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-effect-fallback-delta":
                config.maxEffectFallbackDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-producer-gpu-readback-failure-delta":
                config.maxProducerGpuReadbackFailureDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-gpu-cpu-materialization-block-delta":
                config.maxGpuCpuMaterializationBlockDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-gpu-readback-failed-frames":
                config.maxGpuReadbackFailedFrames = parseNonNegativeInt(arg, value)
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
            case "--max-display-finalize-miss-delta":
                config.maxDisplayFinalizeMissDelta = parseNonNegativeInt(arg, value)
                break
            case "--max-display-finalize-blocking-wait-ms":
                config.maxDisplayFinalizeBlockingWaitMs = parseNonNegativeNumber(arg, value)
                break
            case "--max-display-finalize-surface-realloc-delta":
                config.maxDisplayFinalizeSurfaceReallocDelta = parseNonNegativeInt(arg, value)
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

async function fetchStatus(config: Config): Promise<JsonObject> {
    const systemUrl = `${apiPrefix(config.daemon)}/system`
    let response: Response
    try {
        response = await fetch(systemUrl)
    } catch (error) {
        throw new Error(`Daemon is not reachable at ${systemUrl}: ${errorMessage(error)}`)
    }
    if (!response.ok) {
        throw new Error(`Daemon status check failed at ${systemUrl}: HTTP ${response.status}`)
    }
    const envelope = (await response.json()) as JsonObject
    const status = objectAt(envelope, ["data", "status"])
    if (!status) {
        throw new Error(`Daemon status at ${systemUrl} omitted data.status`)
    }
    return status
}

async function observe(config: Config): Promise<{
    samples: MetricSample[]
    backpressure: BackpressureSample[]
    statusBefore: JsonObject
    statusBaseline: JsonObject
    acceptanceStartedAtMs: number
    statusAfter: JsonObject
}> {
    const statusBefore = await fetchStatus(config)
    let statusBaseline =
        !config.requireMacosNativeCapture || config.warmupMs === 0 ? statusBefore : undefined
    let acceptanceStartedAtMs = config.warmupMs

    const samples: MetricSample[] = []
    const backpressure: BackpressureSample[] = []
    const endpoint = wsEndpoint(config.daemon)
    const startedAtMs = Date.now()

    const observed = await new Promise<{ samples: MetricSample[]; backpressure: BackpressureSample[] }>((resolve, reject) => {
        const socket = new WebSocket(endpoint)
        let settled = false
        let sawOpen = false

        const cleanup = () => {
            clearTimeout(openTimer)
            clearTimeout(finishTimer)
            if (baselineTimer) {
                clearTimeout(baselineTimer)
            }
            process.off("SIGINT", interrupt)
        }

        const finish = () => {
            if (settled) {
                return
            }
            if (config.requireMacosNativeCapture && !statusBaseline) {
                fail(new Error("Warmup status checkpoint did not complete before the observation window"))
                return
            }
            settled = true
            cleanup()
            socket.close()
            resolve({ samples, backpressure })
        }

        const fail = (error: Error) => {
            if (settled) {
                return
            }
            settled = true
            cleanup()
            socket.close()
            reject(error)
        }

        const interrupt = () => {
            fail(new Error("Graphics soak interrupted before the observation window completed"))
        }

        const openTimer = setTimeout(() => {
            if (!sawOpen) {
                fail(new Error(`Timed out opening ${endpoint}`))
            }
        }, 5_000)

        let finishTimer = setTimeout(finish, config.durationMs)
        const baselineTimer =
            !config.requireMacosNativeCapture || config.warmupMs === 0
                ? undefined
                : setTimeout(() => {
                      void fetchStatus(config)
                          .then((status) => {
                              statusBaseline = status
                              acceptanceStartedAtMs = Date.now() - startedAtMs
                              clearTimeout(finishTimer)
                              finishTimer = setTimeout(
                                  finish,
                                  config.durationMs - config.warmupMs,
                              )
                          })
                          .catch((error) => {
                              fail(new Error(`Warmup status checkpoint failed: ${errorMessage(error)}`))
                          })
                  }, config.warmupMs)

        socket.onopen = () => {
            sawOpen = true
            clearTimeout(openTimer)
            socket.send(
                JSON.stringify({
                    type: "subscribe",
                    topics: [{ topic: "metrics", config: { interval_ms: config.intervalMs } }],
                }),
            )
        }

        socket.onerror = () => {
            fail(new Error(`WebSocket error while observing ${endpoint}`))
        }

        socket.onclose = () => {
            if (!settled) {
                fail(new Error(`WebSocket closed before the observation window completed: ${endpoint}`))
            }
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
                    droppedFrames: numberAt(message, ["dropped_frames"]),
                    suggestedFps: numberAt(message, ["suggested_fps"]),
                    topic: stringAt(message, ["topic"]),
                })
            }
        }

        process.once("SIGINT", interrupt)
    })
    const statusAfter = config.requireMacosNativeCapture ? await fetchStatus(config) : statusBefore
    if (!statusBaseline) {
        throw new Error("Warmup status checkpoint is unavailable")
    }
    return { ...observed, statusBefore, statusBaseline, acceptanceStartedAtMs, statusAfter }
}

export function analyze(
    config: Config,
    samples: MetricSample[],
    backpressure: BackpressureSample[],
    statusBefore: JsonObject,
    statusAfter: JsonObject,
    statusBaseline: JsonObject = statusBefore,
    acceptanceStartedAtMs: number = config.warmupMs,
): Report {
    const acceptanceBoundaryMs = config.requireMacosNativeCapture
        ? acceptanceStartedAtMs
        : config.warmupMs
    const acceptanceFrameToken = config.requireMacosNativeCapture
        ? requiredInputHistogramFrameToken(statusBaseline)
        : undefined
    const steadySamples = config.requireMacosNativeCapture
        ? samples.filter(
              (sample) => requiredNumberAt(sample.data, ["timeline", "frame_token"]) > acceptanceFrameToken!,
          )
        : samples.filter((sample) => sample.receivedAtMs >= acceptanceBoundaryMs)
    const baseline = config.requireMacosNativeCapture
        ? config.warmupMs === 0
            ? samples[0]
            : samples
                  .filter(
                      (sample) =>
                          requiredNumberAt(sample.data, ["timeline", "frame_token"]) <= acceptanceFrameToken!,
                  )
                  .at(-1)
        : config.warmupMs === 0
          ? samples[0]
          : samples.filter((sample) => sample.receivedAtMs < acceptanceBoundaryMs).at(-1)
    const observed = steadySamples
    const first = baseline
    const last = steadySamples.at(-1)
    const checks: Check[] = []

    if (!first || !last) {
        return {
            ok: false,
            daemon: config.daemon,
            durationMs: config.durationMs,
            sampleCount: samples.length,
            backpressure,
            summary: {},
            checks: [
                {
                    name: "warmup baseline and steady metrics",
                    ok: false,
                    actual: `${baseline ? 1 : 0}/${steadySamples.length}`,
                    limit: "baseline/steady > 0",
                },
            ],
        }
    }

    const steadyWindowMs = config.durationMs - config.warmupMs
    const minimumLastSampleMs = Math.max(
        0,
        acceptanceBoundaryMs + steadyWindowMs - config.intervalMs * 2,
    )
    const expectedSteadySamples = Math.max(
        2,
        Math.floor(steadyWindowMs / config.intervalMs) - 1,
    )

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
    const displayFinalizeMissDelta = delta(first.data, last.data, [
        "effect_health",
        "sparkleflinger_display_finalize_misses_total",
    ])
    const displayFinalizeSurfaceReallocDelta = delta(first.data, last.data, [
        "effect_health",
        "sparkleflinger_display_finalize_surface_reallocs_total",
    ])
    const frameP95BudgetMs = targetFps > 0 ? (1_000 / targetFps) * 1.25 : Number.POSITIVE_INFINITY
    const maxFrameP95Ms = maxAt(observed, ["frame_time", "p95_ms"])
    let maxInputP95Ms = 0
    let inputSampleCountDelta = 0
    let sessionFullFrameCopyCountDelta = 0

    if (config.requireMacosNativeCapture) {
        checks.push(
            checkAtLeast(
                "observation window coverage ms",
                last.receivedAtMs,
                minimumLastSampleMs,
            ),
        )
        checks.push(checkAtLeast("steady metrics samples", steadySamples.length, expectedSteadySamples))
        inputSampleCountDelta = requiredMetricSequenceDelta(
            statusBefore,
            statusBaseline,
            statusAfter,
            ["session_performance", "input_stage", "sample_count"],
        )
        sessionFullFrameCopyCountDelta = requiredMetricSequenceDelta(
            statusBefore,
            statusBaseline,
            statusAfter,
            ["session_performance", "full_frame_cpu_copies", "count"],
        )
        maxInputP95Ms = requiredHistogramDeltaP95Ms(statusBaseline, statusAfter)
        checks.push(daemonContinuityCheck(statusBefore, statusAfter, config.durationMs))
        checks.push(...workloadChecks(statusBaseline, statusAfter))
        checks.push(checkAtMost("input-stage p95 ms", maxInputP95Ms, config.maxInputP95Ms))
        checks.push(checkAtLeast("input-stage sample growth", inputSampleCountDelta, 1))
        checks.push(
            checkAtMost(
                "session full-frame-copy count delta",
                sessionFullFrameCopyCountDelta,
                config.maxSessionFullFrameCopyCountDelta,
            ),
        )
    }

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
            "GPU producer residency failure delta",
            delta(first.data, last.data, ["effect_health", "producer_gpu_readback_failures_total"]),
            config.maxProducerGpuReadbackFailureDelta,
        ),
    )
    checks.push(
        checkAtMost(
            "GPU-to-CPU materialization block delta",
            delta(first.data, last.data, [
                "effect_health",
                "producer_gpu_cpu_materialization_blocked_total",
            ]),
            config.maxGpuCpuMaterializationBlockDelta,
        ),
    )
    checks.push(
        checkAtMost(
            "GPU residency failed frames",
            maxAt(observed, ["pacing", "gpu_readback_failed_frames"]),
            config.maxGpuReadbackFailedFrames,
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
            "Servo pending render age growth ms",
            maxIncreaseAt(observed, ["effect_health", "servo_render_pending_age_max_ms"]),
            config.maxServoQueueWaitMs,
        ),
    )
    checks.push(
        checkAtMost(
            "display finalizer miss delta",
            displayFinalizeMissDelta,
            config.maxDisplayFinalizeMissDelta,
        ),
    )
    checks.push(
        checkAtMost(
            "display finalizer blocking wait ms",
            maxAt(observed, [
                "effect_health",
                "sparkleflinger_display_finalize_blocking_wait_max_ms",
            ]),
            config.maxDisplayFinalizeBlockingWaitMs,
        ),
    )
    checks.push(
        checkAtMost(
            "display finalizer surface realloc delta",
            displayFinalizeSurfaceReallocDelta,
            config.maxDisplayFinalizeSurfaceReallocDelta,
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
        ...(config.requireMacosNativeCapture
            ? {
                  maxInputP95Ms: round(maxInputP95Ms),
                  inputSampleCountDelta,
                  sessionFullFrameCopyCountDelta,
              }
            : {}),
        backpressureFrames,
        writeFailureDelta: delta(first.data, last.data, ["display_output", "write_failures_total"]),
        retryDelta: delta(first.data, last.data, ["display_output", "retry_attempts_total"]),
        outputErrorFrames: delta(first.data, last.data, ["pacing", "output_error_frames"]),
        maxFullFrameCopyFrames: maxAt(observed, ["pacing", "full_frame_copy_frames"]),
        maxProducerFullFrameCopyCount: maxAt(observed, ["copies", "producer_full_frame_count"]),
        maxProducerFullFrameCopyKb: round(maxAt(observed, ["copies", "producer_full_frame_kb"])),
        latestProducerFullFrameCopyReason: stringAt(last.data, ["copies", "producer_reason"]),
        maxPublicationFullFrameCopyCount: maxAt(observed, ["copies", "publication_full_frame_count"]),
        maxPublicationFullFrameCopyKb: round(maxAt(observed, ["copies", "publication_full_frame_kb"])),
        latestPublicationFullFrameCopyReason: stringAt(last.data, ["copies", "publication_reason"]),
        maxFrameCopyCount: maxAt(observed, ["copies", "full_frame_count"]),
        maxPreviewSurfaceFrames: maxAt(observed, ["pacing", "preview_surface"]),
        maxSceneCanvasForcedSurfaceFrames: maxAt(observed, ["pacing", "scene_canvas_forced_surface"]),
        poolSaturationDelta,
        effectFallbackDelta: delta(first.data, last.data, ["effect_health", "fallbacks_applied_total"]),
        producerGpuReadbackFailureDelta: delta(first.data, last.data, [
            "effect_health",
            "producer_gpu_readback_failures_total",
        ]),
        gpuCpuMaterializationBlockDelta: delta(first.data, last.data, [
            "effect_health",
            "producer_gpu_cpu_materialization_blocked_total",
        ]),
        maxGpuReadbackFailedFrames: maxAt(observed, ["pacing", "gpu_readback_failed_frames"]),
        servoFailureDelta,
        servoQueueWaitMaxMs: round(maxAt(observed, ["effect_health", "servo_render_queue_wait_max_ms"])),
        servoSceneQueueWaitMaxMs: round(
            maxAt(observed, ["effect_health", "servo_render_scene_queue_wait_max_ms"]),
        ),
        servoDisplayQueueWaitMaxMs: round(
            maxAt(observed, ["effect_health", "servo_render_display_queue_wait_max_ms"]),
        ),
        servoQueueWaitMaxGrowthMs: round(
            maxIncreaseAt(observed, ["effect_health", "servo_render_queue_wait_max_ms"]),
        ),
        servoPendingAgeMaxMs: round(maxAt(observed, ["effect_health", "servo_render_pending_age_max_ms"])),
        servoPendingAgeMaxGrowthMs: round(
            maxIncreaseAt(observed, ["effect_health", "servo_render_pending_age_max_ms"]),
        ),
        servoRenderQueueDepthMax: maxAt(observed, ["effect_health", "servo_render_queue_depth_max"]),
        servoRenderSupersededDelta: delta(first.data, last.data, [
            "effect_health",
            "servo_render_superseded_total",
        ]),
        servoRendererLoadWaitMaxMs: round(maxAt(observed, ["effect_health", "servo_renderer_load_wait_max_ms"])),
        servoRendererLoadFailuresDelta: delta(first.data, last.data, [
            "effect_health",
            "servo_renderer_load_failures_total",
        ]),
        servoDestroyWaitMaxMs: round(maxAt(observed, ["effect_health", "servo_destroy_wait_max_ms"])),
        displayFinalizeAttemptDelta:
            delta(first.data, last.data, [
                "effect_health",
                "sparkleflinger_display_finalize_rgba_attempts_total",
            ]) +
            delta(first.data, last.data, [
                "effect_health",
                "sparkleflinger_display_finalize_yuv_attempts_total",
            ]),
        displayFinalizeSuccessDelta: delta(first.data, last.data, [
            "effect_health",
            "sparkleflinger_display_finalize_successes_total",
        ]),
        displayFinalizeMissDelta,
        displayFinalizeLatchDelta: delta(first.data, last.data, [
            "effect_health",
            "sparkleflinger_display_finalize_latches_total",
        ]),
        displayFinalizeBlockingWaitMaxMs: round(
            maxAt(observed, [
                "effect_health",
                "sparkleflinger_display_finalize_blocking_wait_max_ms",
            ]),
        ),
        displayFinalizeSurfaceReallocDelta,
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

function requiredMetricSequenceDelta(
    statusBefore: JsonObject,
    statusBaseline: JsonObject,
    statusAfter: JsonObject,
    statusPath: string[],
): number {
    const values = [
        requiredNumberAt(statusBefore, statusPath),
        requiredNumberAt(statusBaseline, statusPath),
        requiredNumberAt(statusAfter, statusPath),
    ]
    for (let index = 1; index < values.length; index += 1) {
        const previous = values[index - 1]
        const current = values[index]
        if (current < previous) {
            throw new Error(`Cumulative metric regressed: ${statusPath.join(".")} (${previous} -> ${current})`)
        }
    }
    return values[values.length - 1] - values[1]
}

type CumulativeHistogram = {
    bucketWidthUs: number
    overflowBucketIndex: number
    snapshotFrameToken: number
    buckets: Map<number, number>
}

function requiredInputHistogramFrameToken(status: JsonObject): number {
    const inputStage = objectAt(status, ["session_performance", "input_stage"])
    const histogram = inputStage ? objectAt(inputStage, ["cumulative_histogram"]) : undefined
    if (!histogram) {
        throw new Error("Missing input-stage cumulative histogram")
    }
    return requiredNonNegativeInteger(histogram, ["snapshot_frame_token"])
}

function requiredHistogramDeltaP95Ms(statusBaseline: JsonObject, statusAfter: JsonObject): number {
    const baseline = cumulativeInputHistogram(statusBaseline)
    const after = cumulativeInputHistogram(statusAfter)
    if (
        baseline.bucketWidthUs !== after.bucketWidthUs ||
        baseline.overflowBucketIndex !== after.overflowBucketIndex
    ) {
        throw new Error("Input latency histogram geometry changed during observation")
    }

    const bucketIndexes = new Set([...baseline.buckets.keys(), ...after.buckets.keys()])
    const deltas = [...bucketIndexes]
        .sort((left, right) => left - right)
        .map((bucketIndex) => {
            const beforeCount = baseline.buckets.get(bucketIndex) ?? 0
            const afterCount = after.buckets.get(bucketIndex) ?? 0
            if (afterCount < beforeCount) {
                throw new Error(
                    `Cumulative input histogram regressed at bucket ${bucketIndex}: ` +
                        `${beforeCount} -> ${afterCount}`,
                )
            }
            return { bucketIndex, count: afterCount - beforeCount }
        })

    const sampleCount = deltas.reduce((total, bucket) => total + bucket.count, 0)
    if (sampleCount === 0) {
        return 0
    }
    const rank = Math.ceil((sampleCount * 95) / 100)
    let observed = 0
    for (const bucket of deltas) {
        observed += bucket.count
        if (observed >= rank) {
            if (bucket.bucketIndex >= after.overflowBucketIndex) {
                return Number.POSITIVE_INFINITY
            }
            return (bucket.bucketIndex * after.bucketWidthUs) / 1_000
        }
    }
    throw new Error("Input latency histogram did not contain its reported sample count")
}

function cumulativeInputHistogram(status: JsonObject): CumulativeHistogram {
    const inputStage = objectAt(status, ["session_performance", "input_stage"])
    const histogram = inputStage ? objectAt(inputStage, ["cumulative_histogram"]) : undefined
    if (!inputStage || !histogram) {
        throw new Error("Missing input-stage cumulative histogram")
    }
    const bucketWidthUs = requiredPositiveInteger(histogram, ["bucket_width_us"])
    const overflowBucketIndex = requiredPositiveInteger(histogram, ["overflow_bucket_index"])
    const snapshotFrameToken = requiredNonNegativeInteger(histogram, ["snapshot_frame_token"])
    const rawBuckets = valueAt(histogram, ["buckets"])
    if (!Array.isArray(rawBuckets)) {
        throw new Error("Missing input-stage cumulative histogram buckets")
    }

    const buckets = new Map<number, number>()
    for (const rawBucket of rawBuckets) {
        if (!rawBucket || typeof rawBucket !== "object" || Array.isArray(rawBucket)) {
            throw new Error("Invalid input-stage cumulative histogram bucket")
        }
        const bucket = rawBucket as JsonObject
        const bucketIndex = requiredNonNegativeInteger(bucket, ["bucket_index"])
        const count = requiredNonNegativeInteger(bucket, ["count"])
        if (bucketIndex > overflowBucketIndex || buckets.has(bucketIndex)) {
            throw new Error(`Invalid input-stage cumulative histogram bucket index: ${bucketIndex}`)
        }
        buckets.set(bucketIndex, count)
    }

    const histogramSamples = [...buckets.values()].reduce((total, count) => total + count, 0)
    const reportedSamples = requiredNonNegativeInteger(inputStage, ["sample_count"])
    if (histogramSamples !== reportedSamples) {
        throw new Error(
            `Input latency histogram sample count mismatch: ${histogramSamples} != ${reportedSamples}`,
        )
    }
    return { bucketWidthUs, overflowBucketIndex, snapshotFrameToken, buckets }
}

function workloadChecks(statusBaseline: JsonObject, statusAfter: JsonObject): Check[] {
    return [
        workloadCheck("native screen active", statusBaseline, statusAfter, nativeScreenActive),
        nativeScreenPublicationCheck(statusBaseline, statusAfter),
        workloadCheck("audio input active", statusBaseline, statusAfter, (status) => sourceActive(status, "audio")),
        workloadCheck("interaction input active", statusBaseline, statusAfter, (status) =>
            sourceActive(status, "interaction"),
        ),
    ]
}

function daemonContinuityCheck(statusBefore: JsonObject, statusAfter: JsonObject, durationMs: number): Check {
    const before = stringAt(statusBefore, ["server", "instance_id"])
    const after = stringAt(statusAfter, ["server", "instance_id"])
    const uptimeBefore = numberAt(statusBefore, ["uptime_seconds"])
    const uptimeAfter = numberAt(statusAfter, ["uptime_seconds"])
    const minimumUptimeGrowth = Math.max(0, Math.floor(durationMs / 1_000) - 1)
    const continuous = Boolean(before) && before === after && uptimeAfter - uptimeBefore >= minimumUptimeGrowth
    return {
        name: "daemon session continuity",
        ok: continuous,
        actual: continuous ? "continuous" : "changed",
        limit: "continuous",
    }
}

function nativeScreenPublicationCheck(statusBefore: JsonObject, statusAfter: JsonObject): Check {
    const before = nativeScreenSource(statusBefore)
    const after = nativeScreenSource(statusAfter)
    const beforeSourceId = before ? stringAt(before, ["source_id"]) : ""
    const afterSourceId = after ? stringAt(after, ["source_id"]) : ""
    const beforeSession = before ? numberAt(before, ["session_generation"]) : 0
    const afterSession = after ? numberAt(after, ["session_generation"]) : 0
    const beforeGraph = before ? numberAt(before, ["source_graph_generation"]) : 0
    const afterGraph = after ? numberAt(after, ["source_graph_generation"]) : 0
    const beforeCapture = before
        ? numberAt(before, ["platform", "telemetry", "capture_session_generation"])
        : 0
    const afterCapture = after
        ? numberAt(after, ["platform", "telemetry", "capture_session_generation"])
        : 0
    const sameSource =
        Boolean(before) &&
        Boolean(after) &&
        Boolean(beforeSourceId) &&
        beforeSourceId === afterSourceId &&
        beforeSession > 0 &&
        beforeSession === afterSession &&
        beforeGraph > 0 &&
        beforeGraph === afterGraph &&
        beforeCapture > 0 &&
        beforeCapture === afterCapture
    const beforeCount = before ? numberAt(before, ["platform", "telemetry", "frames_published"]) : 0
    const afterCount = after ? numberAt(after, ["platform", "telemetry", "frames_published"]) : 0
    const growth = sameSource && afterCount >= beforeCount ? afterCount - beforeCount : -1
    return checkAtLeast("native screen publication growth", growth, 1)
}

function workloadCheck(
    name: string,
    statusBefore: JsonObject,
    statusAfter: JsonObject,
    predicate: (status: JsonObject) => boolean,
): Check {
    const before = predicate(statusBefore)
    const after = predicate(statusAfter)
    return {
        name,
        ok: before && after,
        actual: `${before ? "active" : "inactive"}/${after ? "active" : "inactive"}`,
        limit: "active/active",
    }
}

function nativeScreenActive(status: JsonObject): boolean {
    if (valueAt(status, ["capture_available"]) !== true) {
        return false
    }
    return Boolean(nativeScreenSource(status))
}

function nativeScreenSource(status: JsonObject): JsonObject | undefined {
    return sources(status).find(
        (source) =>
            sourceIsActive(source, "screen") &&
            valueAt(source, ["freshness"]) === "fresh" &&
            valueAt(source, ["platform", "type"]) === "macos_screen" &&
            valueAt(source, ["platform", "telemetry", "publication_path"]) === "native",
    )
}

function sourceActive(status: JsonObject, kind: string): boolean {
    return sources(status).some((source) => sourceIsActive(source, kind))
}

function sourceIsActive(source: JsonObject, kind: string): boolean {
    const freshness = valueAt(source, ["freshness"])
    return (
        valueAt(source, ["kind"]) === kind &&
        valueAt(source, ["demanded"]) === true &&
        numberAt(source, ["active_consumer_count"]) > 0 &&
        valueAt(source, ["state"]) === "live" &&
        (freshness === "fresh" || freshness === "not_applicable")
    )
}

function sources(status: JsonObject): JsonObject[] {
    const value = valueAt(status, ["input", "sources"])
    return Array.isArray(value)
        ? value.filter((source): source is JsonObject => Boolean(source) && typeof source === "object")
        : []
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

function requiredNonNegativeInteger(root: JsonObject, path: string[]): number {
    const value = requiredNumberAt(root, path)
    if (!Number.isSafeInteger(value) || value < 0) {
        throw new Error(`Metric must be a non-negative integer: ${path.join(".")}`)
    }
    return value
}

function requiredPositiveInteger(root: JsonObject, path: string[]): number {
    const value = requiredNonNegativeInteger(root, path)
    if (value === 0) {
        throw new Error(`Metric must be a positive integer: ${path.join(".")}`)
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
    const summary = report.summary
    console.log("")
    console.log(
        `${palette.cyan}copy pressure${palette.reset} producer=${summary.maxProducerFullFrameCopyCount} ` +
            `publication=${summary.maxPublicationFullFrameCopyCount} total=${summary.maxFrameCopyCount}`,
    )
    console.log(
        `${palette.cyan}surface pressure${palette.reset} preview=${summary.maxPreviewSurfaceFrames} ` +
            `scene_canvas=${summary.maxSceneCanvasForcedSurfaceFrames}`,
    )
    console.log(
        `${palette.cyan}servo qos${palette.reset} queue_wait=${summary.servoQueueWaitMaxMs}ms ` +
            `scene=${summary.servoSceneQueueWaitMaxMs}ms display=${summary.servoDisplayQueueWaitMaxMs}ms ` +
            `pending_age=${summary.servoPendingAgeMaxMs}ms depth=${summary.servoRenderQueueDepthMax} ` +
            `superseded=${summary.servoRenderSupersededDelta}`,
    )
    console.log(
        `${palette.cyan}servo lifecycle${palette.reset} load_wait=${summary.servoRendererLoadWaitMaxMs}ms ` +
            `load_failures=${summary.servoRendererLoadFailuresDelta} destroy_wait=${summary.servoDestroyWaitMaxMs}ms`,
    )
    console.log(
        `${palette.cyan}gpu residency${palette.reset} materialization_blocks=${summary.gpuCpuMaterializationBlockDelta} ` +
            `producer_failures=${summary.producerGpuReadbackFailureDelta} failed_frames=${summary.maxGpuReadbackFailedFrames}`,
    )
    console.log(
        `${palette.cyan}display finalizer${palette.reset} attempts=${summary.displayFinalizeAttemptDelta} ` +
            `successes=${summary.displayFinalizeSuccessDelta} misses=${summary.displayFinalizeMissDelta} ` +
            `latches=${summary.displayFinalizeLatchDelta} reallocs=${summary.displayFinalizeSurfaceReallocDelta}`,
    )
}

async function main(): Promise<void> {
    const config = parseArgs(process.argv.slice(2))
    const { samples, backpressure, statusBefore, statusBaseline, acceptanceStartedAtMs, statusAfter } =
        await observe(config)
    const report = analyze(
        config,
        samples,
        backpressure,
        statusBefore,
        statusAfter,
        statusBaseline,
        acceptanceStartedAtMs,
    )
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

if (import.meta.main) {
    main().catch((error) => {
        console.error(`${palette.red}graphics soak failed:${palette.reset} ${errorMessage(error)}`)
        process.exit(1)
    })
}
