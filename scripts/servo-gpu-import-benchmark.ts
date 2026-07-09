#!/usr/bin/env bun

import { execFileSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync } from "node:fs"
import { cpus } from "node:os"
import { dirname } from "node:path"

type JsonObject = Record<string, unknown>

type Config = {
    daemon: string
    durationMs: number
    intervalMs: number
    warmupMs: number
    label: string
    out?: string
    json: boolean
    hostMetrics: boolean
    daemonPid?: number
}

type StatusSample = {
    receivedAtMs: number
    data: JsonObject
    host?: HostSample
}

type CpuSnapshot = {
    totalJiffies: number
    idleJiffies: number
    busyJiffies: number
    cpuCount: number
    load1: number
    load5: number
    load15: number
}

type MemorySnapshot = {
    totalBytes: number
    availableBytes: number
    usedBytes: number
    usedPercent: number
}

type ProcessSnapshot = {
    pid: number
    cpuJiffies: number
    rssBytes: number
    virtualBytes: number
    threads: number
}

type PressureSnapshot = {
    someAvg10: number
    someAvg60: number
    someAvg300: number
    fullAvg10: number
    fullAvg60: number
    fullAvg300: number
}

type NvidiaSnapshot = {
    gpuUtilPercent: number
    memoryUtilPercent: number
    memoryUsedMb: number
    temperatureCelsius: number
    powerWatts: number
}

type PowercapSnapshot = {
    path: string
    name: string
    energyUj: number
    maxEnergyRangeUj: number
}

type BatterySnapshot = {
    name: string
    status: string
    capacityPercent: number
    powerWatts: number
}

type IntelGpuSnapshot = {
    path: string
    currentFreqMhz: number
    minFreqMhz: number
    maxFreqMhz: number
}

type ThermalSnapshot = {
    path: string
    name: string
    temperatureCelsius: number
}

type HostSample = {
    sampledAtMs: number
    cpu: CpuSnapshot
    memory: MemorySnapshot
    process?: ProcessSnapshot
    pressure: {
        cpu?: PressureSnapshot
        memory?: PressureSnapshot
        io?: PressureSnapshot
    }
    nvidia?: NvidiaSnapshot
    powercap: PowercapSnapshot[]
    battery?: BatterySnapshot
    intelGpu?: IntelGpuSnapshot
    thermals: ThermalSnapshot[]
}

type Check = {
    name: string
    ok: boolean
    actual: number | string | boolean
    limit: number | string | boolean
}

type Report = {
    ok: boolean
    label: string
    daemon: string
    durationMs: number
    warmupMs: number
    sampleCount: number
    startedAt: string
    endedAt: string
    summary: Record<string, number | string | boolean>
    checks: Check[]
    hostSamples: HostSample[]
    first: JsonObject
    last: JsonObject
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
    label: "servo-gpu-import",
    json: false,
    hostMetrics: true,
}

function usage(): string {
    return `Hypercolor Servo GPU import benchmark

Observes an already-running daemon. It does not start, stop, or restart services.

Usage:
  bun scripts/servo-gpu-import-benchmark.ts [options]
  just servo-import-bench -- [options]

Options:
  --daemon <url>              Daemon base URL [${defaults.daemon}]
  --duration-ms <ms>          Observation window [${defaults.durationMs}]
  --duration <30s|2m|1500ms>  Friendlier duration syntax
  --interval-ms <ms>          Status polling interval [${defaults.intervalMs}]
  --warmup-ms <ms>            Exclude initial samples from deltas [${defaults.warmupMs}]
  --label <name>              Report label [${defaults.label}]
  --out <path>                Write JSON report
  --pid <pid>                 Daemon PID for process CPU/RSS; auto-detected for localhost when omitted
  --no-host-metrics           Skip /proc host resource sampling
  --json                      Print JSON only
  --help                      Show this help
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
        if (arg === "--no-host-metrics") {
            config.hostMetrics = false
            continue
        }

        const value = argv[index + 1]
        if (!value || value.startsWith("--")) {
            throw new Error(`${arg} expects a value`)
        }
        index += 1

        switch (arg) {
            case "--daemon":
                config.daemon = value.replace(/\/+$/, "")
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
            case "--label":
                config.label = value
                break
            case "--out":
                config.out = value
                break
            case "--pid":
                config.daemonPid = parsePositiveInt(arg, value)
                break
            default:
                throw new Error(`unknown option: ${arg}`)
        }
    }

    if (config.warmupMs >= config.durationMs) {
        throw new Error("--warmup-ms must be smaller than --duration")
    }
    return config
}

function parsePositiveInt(name: string, value: string): number {
    const parsed = Number(value)
    if (!Number.isInteger(parsed) || parsed <= 0) {
        throw new Error(`${name} expects a positive integer`)
    }
    return parsed
}

function parseNonNegativeInt(name: string, value: string): number {
    const parsed = Number(value)
    if (!Number.isInteger(parsed) || parsed < 0) {
        throw new Error(`${name} expects a non-negative integer`)
    }
    return parsed
}

function parseDuration(value: string): number {
    const match = /^(?<amount>\d+(?:\.\d+)?)(?<unit>ms|s|m)$/.exec(value)
    if (!match?.groups) {
        throw new Error("--duration must look like 1500ms, 30s, or 2m")
    }
    const amount = Number(match.groups.amount)
    const unit = match.groups.unit
    const multiplier = unit === "ms" ? 1 : unit === "s" ? 1_000 : 60_000
    return Math.round(amount * multiplier)
}

async function fetchStatus(daemon: string): Promise<JsonObject> {
    const response = await fetch(`${daemon}/api/v1/status`)
    if (!response.ok) {
        throw new Error(`status request failed: HTTP ${response.status}`)
    }
    const payload = (await response.json()) as JsonObject
    const data = payload.data
    if (!isObject(data)) {
        throw new Error("status response did not contain an object data payload")
    }
    return data
}

async function observe(config: Config): Promise<StatusSample[]> {
    const startedAtMs = Date.now()
    const samples: StatusSample[] = []
    const daemonPid = config.hostMetrics ? (config.daemonPid ?? discoverDaemonPid(config.daemon)) : undefined

    while (Date.now() - startedAtMs <= config.durationMs) {
        const receivedAtMs = Date.now() - startedAtMs
        samples.push({
            receivedAtMs,
            data: await fetchStatus(config.daemon),
            host: config.hostMetrics ? sampleHost(receivedAtMs, daemonPid) : undefined,
        })
        const nextSampleAtMs = receivedAtMs + config.intervalMs
        const sleepMs = Math.max(0, startedAtMs + nextSampleAtMs - Date.now())
        await sleep(sleepMs)
    }

    return samples
}

function discoverDaemonPid(daemon: string): number | undefined {
    const port = daemonPort(daemon)
    if (!port || !isLocalDaemon(daemon)) {
        return undefined
    }

    const ssOutput = runText("ss", ["-H", "-ltnp", `sport = :${port}`])
    const ssPid = ssOutput ? Number(/pid=(\d+)/.exec(ssOutput)?.[1]) : 0
    if (Number.isInteger(ssPid) && ssPid > 0) {
        return ssPid
    }

    const pgrepOutput = runText("pgrep", ["-af", "hypercolor-daemon"])
    const candidates = (pgrepOutput ?? "")
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => /\/hypercolor-daemon(\s|$)/.test(line))
        .map((line) => Number(line.split(/\s+/, 1)[0]))
        .filter((pid) => Number.isInteger(pid) && pid > 0)
    return candidates.length === 1 ? candidates[0] : undefined
}

function daemonPort(daemon: string): number | undefined {
    try {
        const url = new URL(daemon)
        return Number(url.port || (url.protocol === "https:" ? "443" : "80"))
    } catch {
        return undefined
    }
}

function isLocalDaemon(daemon: string): boolean {
    try {
        const host = new URL(daemon).hostname
        return host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "[::1]"
    } catch {
        return false
    }
}

function sampleHost(sampledAtMs: number, daemonPid: number | undefined): HostSample {
    return {
        sampledAtMs,
        cpu: readCpuSnapshot(),
        memory: readMemorySnapshot(),
        process: daemonPid ? readProcessSnapshot(daemonPid) : undefined,
        pressure: {
            cpu: readPressureSnapshot("/proc/pressure/cpu"),
            memory: readPressureSnapshot("/proc/pressure/memory"),
            io: readPressureSnapshot("/proc/pressure/io"),
        },
        nvidia: readNvidiaSnapshot(),
        powercap: readPowercapSnapshots(),
        battery: readBatterySnapshot(),
        intelGpu: readIntelGpuSnapshot(),
        thermals: readThermalSnapshots(),
    }
}

function readCpuSnapshot(): CpuSnapshot {
    const statLine = readText("/proc/stat").split("\n")[0] ?? ""
    const values = statLine
        .trim()
        .split(/\s+/)
        .slice(1)
        .map((value) => Number(value))
        .filter((value) => Number.isFinite(value))
    const totalJiffies = values.reduce((total, value) => total + value, 0)
    const idleJiffies = (values[3] ?? 0) + (values[4] ?? 0)
    const [load1 = 0, load5 = 0, load15 = 0] = readText("/proc/loadavg")
        .trim()
        .split(/\s+/, 3)
        .map((value) => Number(value))
    return {
        totalJiffies,
        idleJiffies,
        busyJiffies: Math.max(0, totalJiffies - idleJiffies),
        cpuCount: Math.max(1, cpus().length),
        load1: finiteOrZero(load1),
        load5: finiteOrZero(load5),
        load15: finiteOrZero(load15),
    }
}

function readMemorySnapshot(): MemorySnapshot {
    const fields = readKeyedKbFile("/proc/meminfo")
    const totalBytes = (fields.MemTotal ?? 0) * 1024
    const availableBytes = (fields.MemAvailable ?? 0) * 1024
    const usedBytes = Math.max(0, totalBytes - availableBytes)
    return {
        totalBytes,
        availableBytes,
        usedBytes,
        usedPercent: totalBytes > 0 ? (usedBytes / totalBytes) * 100 : 0,
    }
}

function readProcessSnapshot(pid: number): ProcessSnapshot | undefined {
    const stat = readText(`/proc/${pid}/stat`)
    if (!stat) {
        return undefined
    }

    const endOfName = stat.lastIndexOf(")")
    const fields = stat
        .slice(endOfName + 2)
        .trim()
        .split(/\s+/)
    const status = readKeyedKbFile(`/proc/${pid}/status`)
    const utime = Number(fields[11] ?? 0)
    const stime = Number(fields[12] ?? 0)
    const threads = Number(fields[17] ?? 0)
    return {
        pid,
        cpuJiffies: finiteOrZero(utime) + finiteOrZero(stime),
        rssBytes: (status.VmRSS ?? 0) * 1024,
        virtualBytes: (status.VmSize ?? 0) * 1024,
        threads: Number.isFinite(threads) ? threads : 0,
    }
}

function readPressureSnapshot(path: string): PressureSnapshot | undefined {
    if (!existsSync(path)) {
        return undefined
    }
    const lines = readText(path).split("\n")
    const some = parsePressureLine(lines.find((line) => line.startsWith("some ")))
    const full = parsePressureLine(lines.find((line) => line.startsWith("full ")))
    if (!some && !full) {
        return undefined
    }
    return {
        someAvg10: some?.avg10 ?? 0,
        someAvg60: some?.avg60 ?? 0,
        someAvg300: some?.avg300 ?? 0,
        fullAvg10: full?.avg10 ?? 0,
        fullAvg60: full?.avg60 ?? 0,
        fullAvg300: full?.avg300 ?? 0,
    }
}

function parsePressureLine(line: string | undefined): { avg10: number; avg60: number; avg300: number } | undefined {
    if (!line) {
        return undefined
    }
    return {
        avg10: numberFromMatch(line, /avg10=([\d.]+)/),
        avg60: numberFromMatch(line, /avg60=([\d.]+)/),
        avg300: numberFromMatch(line, /avg300=([\d.]+)/),
    }
}

function readNvidiaSnapshot(): NvidiaSnapshot | undefined {
    const output = runText("nvidia-smi", [
        "--query-gpu=utilization.gpu,utilization.memory,memory.used,temperature.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ])
    const line = output?.split("\n").find((candidate) => candidate.trim().length > 0)
    if (!line) {
        return undefined
    }
    const [gpuUtilPercent, memoryUtilPercent, memoryUsedMb, temperatureCelsius, powerWatts] = line
        .split(",")
        .map((value) => Number(value.trim().replace(/[^\d.-]/g, "")))
    if (!Number.isFinite(gpuUtilPercent)) {
        return undefined
    }
    return {
        gpuUtilPercent: finiteOrZero(gpuUtilPercent),
        memoryUtilPercent: finiteOrZero(memoryUtilPercent),
        memoryUsedMb: finiteOrZero(memoryUsedMb),
        temperatureCelsius: finiteOrZero(temperatureCelsius),
        powerWatts: finiteOrZero(powerWatts),
    }
}

function readPowercapSnapshots(): PowercapSnapshot[] {
    const root = "/sys/class/powercap"
    if (!existsSync(root)) {
        return []
    }
    const snapshots: PowercapSnapshot[] = []
    for (const entry of safeReadDir(root)) {
        const path = `${root}/${entry}`
        if (!existsSync(`${path}/energy_uj`)) {
            continue
        }
        const energyUj = readOptionalNumberFile(`${path}/energy_uj`)
        if (energyUj === undefined) {
            continue
        }
        snapshots.push({
            path: safeRealPath(path),
            name: readText(`${path}/name`).trim() || entry,
            energyUj,
            maxEnergyRangeUj: readNumberFile(`${path}/max_energy_range_uj`),
        })
    }
    return snapshots
}

function readBatterySnapshot(): BatterySnapshot | undefined {
    const root = "/sys/class/power_supply"
    if (!existsSync(root)) {
        return undefined
    }
    for (const entry of safeReadDir(root)) {
        const path = `${root}/${entry}`
        if (readText(`${path}/type`).trim() !== "Battery") {
            continue
        }
        const powerNowUw = readNumberFile(`${path}/power_now`)
        const currentNowUa = readNumberFile(`${path}/current_now`)
        const voltageNowUv = readNumberFile(`${path}/voltage_now`)
        const powerWatts = powerNowUw > 0 ? powerNowUw / 1_000_000 : (currentNowUa * voltageNowUv) / 1_000_000_000_000
        return {
            name: entry,
            status: readText(`${path}/status`).trim(),
            capacityPercent: readNumberFile(`${path}/capacity`),
            powerWatts: finiteOrZero(powerWatts),
        }
    }
    return undefined
}

function readIntelGpuSnapshot(): IntelGpuSnapshot | undefined {
    const seen = new Set<string>()
    for (const entry of safeReadDir("/sys/class/drm")) {
        if (!entry.startsWith("card")) {
            continue
        }
        const path = `/sys/class/drm/${entry}/device`
        const realPath = safeRealPath(path)
        if (!existsSync(`${path}/gt_cur_freq_mhz`)) {
            continue
        }
        if (seen.has(realPath)) {
            continue
        }
        seen.add(realPath)
        return {
            path: realPath,
            currentFreqMhz: readNumberFile(`${path}/gt_cur_freq_mhz`),
            minFreqMhz: readNumberFile(`${path}/gt_min_freq_mhz`),
            maxFreqMhz: readNumberFile(`${path}/gt_max_freq_mhz`),
        }
    }
    return undefined
}

function readThermalSnapshots(): ThermalSnapshot[] {
    const root = "/sys/class/hwmon"
    if (!existsSync(root)) {
        return []
    }
    const snapshots: ThermalSnapshot[] = []
    for (const hwmon of safeReadDir(root)) {
        const path = `${root}/${hwmon}`
        const name = readText(`${path}/name`).trim() || hwmon
        for (const entry of safeReadDir(path)) {
            if (!/^temp\d+_input$/.test(entry)) {
                continue
            }
            const temperatureCelsius = readNumberFile(`${path}/${entry}`) / 1000
            if (temperatureCelsius > 0) {
                snapshots.push({
                    path: safeRealPath(`${path}/${entry}`),
                    name,
                    temperatureCelsius,
                })
            }
        }
    }
    return snapshots
}

function readKeyedKbFile(path: string): Record<string, number> {
    const result: Record<string, number> = {}
    for (const line of readText(path).split("\n")) {
        const match = /^([^:]+):\s+(\d+)/.exec(line)
        if (match) {
            result[match[1]] = Number(match[2])
        }
    }
    return result
}

function readText(path: string): string {
    try {
        return readFileSync(path, "utf8")
    } catch {
        return ""
    }
}

function readNumberFile(path: string): number {
    return finiteOrZero(Number(readText(path).trim()))
}

function readOptionalNumberFile(path: string): number | undefined {
    const text = readText(path).trim()
    if (text.length === 0) {
        return undefined
    }
    const value = Number(text)
    return Number.isFinite(value) ? value : undefined
}

function safeReadDir(path: string): string[] {
    try {
        return readdirSync(path)
    } catch {
        return []
    }
}

function safeRealPath(path: string): string {
    try {
        return realpathSync(path)
    } catch {
        return path
    }
}

function runText(command: string, args: string[]): string | undefined {
    try {
        return execFileSync(command, args, {
            encoding: "utf8",
            timeout: 1_000,
            stdio: ["ignore", "pipe", "ignore"],
        })
    } catch {
        return undefined
    }
}

function analyze(config: Config, samples: StatusSample[], startedAt: Date, endedAt: Date): Report {
    const steadySamples = samples.filter((sample) => sample.receivedAtMs >= config.warmupMs)
    const observed = steadySamples.length > 1 ? steadySamples : samples
    const first = observed[0]?.data ?? {}
    const last = observed.at(-1)?.data ?? {}

    const latestFrames = observed.map((sample) => objectAt(sample.data, ["latest_frame"]))
    const frameTotals = latestFrames.map((frame) => numberAt(frame, ["total_ms"])).filter((value) => value > 0)
    const producerTimes = latestFrames.map((frame) => numberAt(frame, ["producer_ms"])).filter((value) => value > 0)
    const compositionTimes = latestFrames.map((frame) => numberAt(frame, ["composition_ms"])).filter((value) => value > 0)
    const sampleTimes = latestFrames.map((frame) => numberAt(frame, ["spatial_sampling_ms"])).filter((value) => value > 0)
    const hostSamples = observed.map((sample) => sample.host).filter((sample): sample is HostSample => Boolean(sample))

    const summary = {
        activeEffect: stringAt(last, ["active_effect"]),
        activeScene: stringAt(last, ["active_scene"]),
        importMode: stringAt(last, ["compositor_acceleration", "servo_gpu_import_mode"]),
        importAttempting: boolAt(last, ["compositor_acceleration", "servo_gpu_import_attempting"]),
        importBackendCompatible: boolAt(last, [
            "compositor_acceleration",
            "gpu_probe",
            "servo_gpu_import_backend_compatible",
        ]),
        compositorBackend: stringAt(last, ["latest_frame", "compositor_backend"]),
        targetFps: numberAt(last, ["render_loop", "target_fps"]),
        actualFps: numberAt(last, ["render_loop", "actual_fps"]),
        servoCpuFrameDelta: delta(first, last, ["effect_health", "servo_render_cpu_frames_total"]),
        servoGpuFrameDelta: delta(first, last, ["effect_health", "servo_render_gpu_frames_total"]),
        servoReadbackMsDelta: round(delta(first, last, ["effect_health", "servo_render_readback_total_ms"])),
        servoImportMsDelta: round(delta(first, last, ["effect_health", "servo_gpu_import_total_ms"])),
        servoImportFailureDelta: delta(first, last, ["effect_health", "servo_gpu_import_failures_total"]),
        servoImportFallbackDelta: delta(first, last, ["effect_health", "servo_gpu_import_fallbacks_total"]),
        producerCpuFrameDelta: delta(first, last, ["effect_health", "producer_cpu_frames_total"]),
        producerGpuFrameDelta: delta(first, last, ["effect_health", "producer_gpu_frames_total"]),
        sourceUploadSkippedDelta: delta(first, last, [
            "effect_health",
            "sparkleflinger_gpu_source_upload_skipped_total",
        ]),
        producerGpuReadbackFailureDelta: delta(first, last, [
            "effect_health",
            "producer_gpu_readback_failures_total",
        ]),
        effectErrorDelta: delta(first, last, ["effect_health", "errors_total"]),
        effectFallbackDelta: delta(first, last, ["effect_health", "fallbacks_applied_total"]),
        latestGpuReadbackFailedSamples: latestFrames.filter((frame) => boolAt(frame, ["gpu_readback_failed"])).length,
        latestGpuSampleCpuFallbackSamples: latestFrames.filter((frame) => boolAt(frame, ["gpu_sample_cpu_fallback"])).length,
        latestGpuSampleWaitBlockedSamples: latestFrames.filter((frame) => boolAt(frame, ["gpu_sample_wait_blocked"])).length,
        latestFullFrameCopySamples: latestFrames.filter((frame) => numberAt(frame, ["full_frame_copy_count"]) > 0).length,
        frameP50Ms: round(percentile(frameTotals, 50)),
        frameP95Ms: round(percentile(frameTotals, 95)),
        frameMaxMs: round(Math.max(0, ...frameTotals)),
        producerP95Ms: round(percentile(producerTimes, 95)),
        compositionP95Ms: round(percentile(compositionTimes, 95)),
        spatialSamplingP95Ms: round(percentile(sampleTimes, 95)),
        ...summarizeHost(hostSamples),
    }

    const importMode = String(summary.importMode)
    const importExpected = importMode === "auto" || importMode === "on"
    const checks = [
        checkAtLeast("status samples", samples.length, 2),
        ...(importExpected
            ? [
                  checkEquals("Servo GPU import attempting", summary.importAttempting, true),
                  checkAtLeast("Servo GPU frame delta", Number(summary.servoGpuFrameDelta), 1),
              ]
            : []),
        checkAtMost("Servo import failure delta", Number(summary.servoImportFailureDelta), 0),
        checkAtMost("Servo import fallback delta", Number(summary.servoImportFallbackDelta), 0),
        checkAtMost("producer GPU readback failure delta", Number(summary.producerGpuReadbackFailureDelta), 0),
        checkAtMost("sampled GPU readback failed frames", Number(summary.latestGpuReadbackFailedSamples), 0),
    ]

    return {
        ok: checks.every((check) => check.ok),
        label: config.label,
        daemon: config.daemon,
        durationMs: config.durationMs,
        warmupMs: config.warmupMs,
        sampleCount: samples.length,
        startedAt: startedAt.toISOString(),
        endedAt: endedAt.toISOString(),
        summary,
        checks,
        hostSamples: samples.map((sample) => sample.host).filter((sample): sample is HostSample => Boolean(sample)),
        first,
        last,
    }
}

function summarizeHost(samples: HostSample[]): Record<string, number | string | boolean> {
    const hostCpuPercents: number[] = []
    const daemonCpuPercents: number[] = []
    for (let index = 1; index < samples.length; index += 1) {
        const previous = samples[index - 1]
        const current = samples[index]
        const totalDelta = current.cpu.totalJiffies - previous.cpu.totalJiffies
        const busyDelta = current.cpu.busyJiffies - previous.cpu.busyJiffies
        if (totalDelta > 0) {
            hostCpuPercents.push((busyDelta / totalDelta) * 100)
        }
        if (previous.process && current.process && previous.process.pid === current.process.pid && totalDelta > 0) {
            const processDelta = current.process.cpuJiffies - previous.process.cpuJiffies
            daemonCpuPercents.push((processDelta / totalDelta) * current.cpu.cpuCount * 100)
        }
    }

    const memoryUsedPercents = samples.map((sample) => sample.memory.usedPercent)
    const memoryAvailableMb = samples.map((sample) => bytesToMb(sample.memory.availableBytes))
    const daemonRssMb = samples
        .map((sample) => (sample.process ? bytesToMb(sample.process.rssBytes) : 0))
        .filter((value) => value > 0)
    const daemonThreads = samples
        .map((sample) => sample.process?.threads ?? 0)
        .filter((value) => value > 0)
    const nvidiaSamples = samples.map((sample) => sample.nvidia).filter((sample): sample is NvidiaSnapshot => Boolean(sample))
    const packageWatts = powercapWatts(samples, "package-0")
    const psysWatts = powercapWatts(samples, "psys")
    const coreWatts = powercapWatts(samples, "core")
    const uncoreWatts = powercapWatts(samples, "uncore")
    const batteryWatts = samples
        .map((sample) => sample.battery?.powerWatts ?? 0)
        .filter((value) => value > 0)
    const intelGpuFreqs = samples
        .map((sample) => sample.intelGpu?.currentFreqMhz ?? 0)
        .filter((value) => value > 0)
    const thermalMaxes = samples
        .map((sample) => maxValue(sample.thermals.map((thermal) => thermal.temperatureCelsius)))
        .filter((value) => value > 0)
    const last = samples.at(-1)

    return {
        hostMetricSamples: samples.length,
        hostCpuCount: last?.cpu.cpuCount ?? 0,
        hostLoad1: round(last?.cpu.load1 ?? 0),
        hostLoad5: round(last?.cpu.load5 ?? 0),
        hostLoad15: round(last?.cpu.load15 ?? 0),
        hostLoad1PerCpu: round(last ? last.cpu.load1 / last.cpu.cpuCount : 0),
        hostCpuP50Percent: round(percentile(hostCpuPercents, 50)),
        hostCpuP95Percent: round(percentile(hostCpuPercents, 95)),
        hostCpuMaxPercent: round(maxValue(hostCpuPercents)),
        hostMemoryUsedP50Percent: round(percentile(memoryUsedPercents, 50)),
        hostMemoryUsedMaxPercent: round(maxValue(memoryUsedPercents)),
        hostMemoryAvailableMinMb: round(minValue(memoryAvailableMb)),
        daemonPid: last?.process?.pid ?? 0,
        daemonCpuP50Percent: round(percentile(daemonCpuPercents, 50)),
        daemonCpuP95Percent: round(percentile(daemonCpuPercents, 95)),
        daemonCpuMaxPercent: round(maxValue(daemonCpuPercents)),
        daemonRssP50Mb: round(percentile(daemonRssMb, 50)),
        daemonRssMaxMb: round(maxValue(daemonRssMb)),
        daemonThreadsMax: round(maxValue(daemonThreads)),
        cpuPressureSomeAvg10Max: round(maxValue(samples.map((sample) => sample.pressure.cpu?.someAvg10 ?? 0))),
        memoryPressureSomeAvg10Max: round(maxValue(samples.map((sample) => sample.pressure.memory?.someAvg10 ?? 0))),
        ioPressureSomeAvg10Max: round(maxValue(samples.map((sample) => sample.pressure.io?.someAvg10 ?? 0))),
        packagePowerP50Watts: round(percentile(packageWatts, 50)),
        packagePowerP95Watts: round(percentile(packageWatts, 95)),
        packagePowerMaxWatts: round(maxValue(packageWatts)),
        psysPowerP50Watts: round(percentile(psysWatts, 50)),
        psysPowerP95Watts: round(percentile(psysWatts, 95)),
        psysPowerMaxWatts: round(maxValue(psysWatts)),
        corePowerP95Watts: round(percentile(coreWatts, 95)),
        uncorePowerP95Watts: round(percentile(uncoreWatts, 95)),
        batteryPowerP50Watts: round(percentile(batteryWatts, 50)),
        batteryPowerMaxWatts: round(maxValue(batteryWatts)),
        intelGpuFreqP50Mhz: round(percentile(intelGpuFreqs, 50)),
        intelGpuFreqP95Mhz: round(percentile(intelGpuFreqs, 95)),
        intelGpuFreqMaxMhz: round(maxValue(intelGpuFreqs)),
        thermalP95Celsius: round(percentile(thermalMaxes, 95)),
        thermalMaxCelsius: round(maxValue(thermalMaxes)),
        nvidiaSampleCount: nvidiaSamples.length,
        nvidiaGpuP50Percent: round(percentile(nvidiaSamples.map((sample) => sample.gpuUtilPercent), 50)),
        nvidiaGpuP95Percent: round(percentile(nvidiaSamples.map((sample) => sample.gpuUtilPercent), 95)),
        nvidiaMemoryUsedMaxMb: round(maxValue(nvidiaSamples.map((sample) => sample.memoryUsedMb))),
        nvidiaPowerMaxWatts: round(maxValue(nvidiaSamples.map((sample) => sample.powerWatts))),
    }
}

function powercapWatts(samples: HostSample[], name: string): number[] {
    const watts: number[] = []
    for (let index = 1; index < samples.length; index += 1) {
        const previous = selectPowercapDomain(samples[index - 1], name)
        const current = selectPowercapDomain(samples[index], name)
        const elapsedSeconds = (samples[index].sampledAtMs - samples[index - 1].sampledAtMs) / 1000
        if (!previous || !current || elapsedSeconds <= 0) {
            continue
        }
        let deltaUj = current.energyUj - previous.energyUj
        if (deltaUj < 0 && current.maxEnergyRangeUj > 0) {
            deltaUj = current.maxEnergyRangeUj - previous.energyUj + current.energyUj
        }
        if (deltaUj >= 0) {
            watts.push(deltaUj / 1_000_000 / elapsedSeconds)
        }
    }
    return watts
}

function selectPowercapDomain(sample: HostSample, name: string): PowercapSnapshot | undefined {
    return (
        sample.powercap.find((domain) => domain.name === name && !domain.path.includes("intel-rapl-mmio")) ??
        sample.powercap.find((domain) => domain.name === name)
    )
}

function checkAtMost(name: string, actual: number, limit: number): Check {
    return { name, ok: actual <= limit, actual: round(actual), limit: round(limit) }
}

function checkAtLeast(name: string, actual: number, limit: number): Check {
    return { name, ok: actual >= limit, actual: round(actual), limit: `>= ${round(limit)}` }
}

function checkEquals(name: string, actual: boolean, limit: boolean): Check {
    return { name, ok: actual === limit, actual, limit }
}

function printReport(report: Report): void {
    const status = report.ok
        ? `${palette.green}PASS${palette.reset}`
        : `${palette.red}FAIL${palette.reset}`
    console.log(`${palette.bold}${palette.purple}Servo GPU import benchmark${palette.reset} ${status}`)
    console.log(`${palette.cyan}${report.daemon}${palette.reset} · ${report.sampleCount} samples · ${report.durationMs}ms`)
    console.log(
        `mode ${palette.coral}${report.summary.importMode}${palette.reset} · attempting ${report.summary.importAttempting} · effect ${palette.cyan}${report.summary.activeEffect}${palette.reset}`,
    )
    console.log(
        `Servo frames cpu/gpu ${palette.coral}${report.summary.servoCpuFrameDelta}${palette.reset}/${palette.coral}${report.summary.servoGpuFrameDelta}${palette.reset} · readback ${palette.coral}${report.summary.servoReadbackMsDelta}ms${palette.reset} · import ${palette.coral}${report.summary.servoImportMsDelta}ms${palette.reset}`,
    )
    console.log(
        `Producer frames cpu/gpu ${palette.coral}${report.summary.producerCpuFrameDelta}${palette.reset}/${palette.coral}${report.summary.producerGpuFrameDelta}${palette.reset} · upload skips ${palette.coral}${report.summary.sourceUploadSkippedDelta}${palette.reset}`,
    )
    console.log(
        `Readback failures producer delta ${palette.coral}${report.summary.producerGpuReadbackFailureDelta}${palette.reset} · sampled latest-frame failures ${palette.coral}${report.summary.latestGpuReadbackFailedSamples}${palette.reset}`,
    )
    console.log(
        `Frame ms p50/p95/max ${palette.coral}${report.summary.frameP50Ms}${palette.reset}/${palette.coral}${report.summary.frameP95Ms}${palette.reset}/${palette.coral}${report.summary.frameMaxMs}${palette.reset}`,
    )
    if (Number(report.summary.hostMetricSamples) > 0) {
        console.log(
            `Host CPU p50/p95/max ${palette.coral}${report.summary.hostCpuP50Percent}%${palette.reset}/${palette.coral}${report.summary.hostCpuP95Percent}%${palette.reset}/${palette.coral}${report.summary.hostCpuMaxPercent}%${palette.reset} · load1 ${palette.coral}${report.summary.hostLoad1}${palette.reset} (${report.summary.hostLoad1PerCpu}/cpu) · mem ${palette.coral}${report.summary.hostMemoryUsedMaxPercent}% max${palette.reset}`,
        )
        if (Number(report.summary.daemonPid) > 0) {
            console.log(
                `Daemon pid ${palette.coral}${report.summary.daemonPid}${palette.reset} · CPU p50/p95/max ${palette.coral}${report.summary.daemonCpuP50Percent}%${palette.reset}/${palette.coral}${report.summary.daemonCpuP95Percent}%${palette.reset}/${palette.coral}${report.summary.daemonCpuMaxPercent}%${palette.reset} · RSS p50/max ${palette.coral}${report.summary.daemonRssP50Mb}MB${palette.reset}/${palette.coral}${report.summary.daemonRssMaxMb}MB${palette.reset}`,
            )
        }
        console.log(
            `Pressure avg10 max cpu/mem/io ${palette.coral}${report.summary.cpuPressureSomeAvg10Max}${palette.reset}/${palette.coral}${report.summary.memoryPressureSomeAvg10Max}${palette.reset}/${palette.coral}${report.summary.ioPressureSomeAvg10Max}${palette.reset}`,
        )
        if (Number(report.summary.packagePowerMaxWatts) > 0 || Number(report.summary.psysPowerMaxWatts) > 0) {
            console.log(
                `Power W package p50/p95/max ${palette.coral}${report.summary.packagePowerP50Watts}${palette.reset}/${palette.coral}${report.summary.packagePowerP95Watts}${palette.reset}/${palette.coral}${report.summary.packagePowerMaxWatts}${palette.reset} · psys p95 ${palette.coral}${report.summary.psysPowerP95Watts}${palette.reset}`,
            )
        }
        if (Number(report.summary.intelGpuFreqMaxMhz) > 0 || Number(report.summary.thermalMaxCelsius) > 0) {
            console.log(
                `Intel GPU freq p50/p95/max ${palette.coral}${report.summary.intelGpuFreqP50Mhz}${palette.reset}/${palette.coral}${report.summary.intelGpuFreqP95Mhz}${palette.reset}/${palette.coral}${report.summary.intelGpuFreqMaxMhz}MHz${palette.reset} · temp max ${palette.coral}${report.summary.thermalMaxCelsius}C${palette.reset}`,
            )
        }
        if (Number(report.summary.batteryPowerMaxWatts) > 0) {
            console.log(
                `Battery draw W p50/max ${palette.coral}${report.summary.batteryPowerP50Watts}${palette.reset}/${palette.coral}${report.summary.batteryPowerMaxWatts}${palette.reset}`,
            )
        }
        if (Number(report.summary.nvidiaSampleCount) > 0) {
            console.log(
                `NVIDIA GPU p50/p95 ${palette.coral}${report.summary.nvidiaGpuP50Percent}%${palette.reset}/${palette.coral}${report.summary.nvidiaGpuP95Percent}%${palette.reset} · VRAM max ${palette.coral}${report.summary.nvidiaMemoryUsedMaxMb}MB${palette.reset} · power max ${palette.coral}${report.summary.nvidiaPowerMaxWatts}W${palette.reset}`,
            )
        }
    }
    for (const check of report.checks) {
        const marker = check.ok ? `${palette.green}ok${palette.reset}` : `${palette.red}fail${palette.reset}`
        console.log(`  ${marker} ${check.name}: ${check.actual} / ${check.limit}`)
    }
}

function delta(first: JsonObject, last: JsonObject, path: string[]): number {
    return Math.max(0, numberAt(last, path) - numberAt(first, path))
}

function numberAt(value: unknown, path: string[]): number {
    const found = valueAt(value, path)
    return typeof found === "number" && Number.isFinite(found) ? found : 0
}

function boolAt(value: unknown, path: string[]): boolean {
    return valueAt(value, path) === true
}

function stringAt(value: unknown, path: string[]): string {
    const found = valueAt(value, path)
    return typeof found === "string" ? found : ""
}

function objectAt(value: unknown, path: string[]): JsonObject {
    const found = valueAt(value, path)
    return isObject(found) ? found : {}
}

function valueAt(value: unknown, path: string[]): unknown {
    let current = value
    for (const key of path) {
        if (!isObject(current)) {
            return undefined
        }
        current = current[key]
    }
    return current
}

function isObject(value: unknown): value is JsonObject {
    return typeof value === "object" && value !== null && !Array.isArray(value)
}

function percentile(values: number[], percentileValue: number): number {
    if (values.length === 0) {
        return 0
    }
    const sorted = [...values].sort((left, right) => left - right)
    const rank = Math.ceil((percentileValue / 100) * sorted.length)
    return sorted[Math.max(0, rank - 1)] ?? 0
}

function maxValue(values: number[]): number {
    return values.length > 0 ? Math.max(...values) : 0
}

function minValue(values: number[]): number {
    return values.length > 0 ? Math.min(...values) : 0
}

function bytesToMb(value: number): number {
    return value / 1024 / 1024
}

function numberFromMatch(value: string, pattern: RegExp): number {
    const parsed = Number(pattern.exec(value)?.[1])
    return finiteOrZero(parsed)
}

function finiteOrZero(value: number): number {
    return Number.isFinite(value) ? value : 0
}

function round(value: number): number {
    return Math.round(value * 100) / 100
}

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
}

async function main(): Promise<void> {
    const config = parseArgs(Bun.argv.slice(2))
    const startedAt = new Date()
    const samples = await observe(config)
    const endedAt = new Date()
    const report = analyze(config, samples, startedAt, endedAt)
    const json = `${JSON.stringify(report, null, 2)}\n`

    if (config.out) {
        mkdirSync(dirname(config.out), { recursive: true })
        await Bun.write(config.out, json)
    }

    if (config.json) {
        process.stdout.write(json)
    } else {
        printReport(report)
        if (config.out) {
            console.log(`${palette.green}wrote${palette.reset} ${palette.cyan}${config.out}${palette.reset}`)
        }
    }

    process.exitCode = report.ok ? 0 : 1
}

main().catch((error: unknown) => {
    const message = error instanceof Error ? error.message : String(error)
    console.error(`${palette.red}servo import benchmark failed:${palette.reset} ${message}`)
    process.exit(1)
})
