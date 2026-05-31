#!/usr/bin/env bun

import { execFileSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync } from "node:fs"
import { arch, hostname, platform, release } from "node:os"
import { join } from "node:path"

type JsonObject = Record<string, unknown>

type Config = {
    daemon: string
    durationMs: number
    intervalMs: number
    warmupMs: number
    readyTimeoutMs: number
    settleMs: number
    cooldownMs: number
    repeat: number
    modes: string[]
    outDir: string
    label: string
    profile: string
    features: string
    logLevel: string
    json: boolean
    dryRun: boolean
}

type RunResult = {
    mode: string
    iteration: number
    ok: boolean
    reportPath: string
    logPath: string
    summary: JsonObject
    checks: unknown[]
    output: string
}

type SuiteReport = {
    ok: boolean
    label: string
    startedAt: string
    endedAt: string
    config: Config
    metadata: JsonObject
    runs: RunResult[]
    comparison: JsonObject
}

type StartedDaemon = {
    process: Bun.Subprocess<"ignore", "pipe", "pipe">
    stdout: Promise<string>
    stderr: Promise<string>
    logPath: string
    exitCode?: number
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

function cargoTargetDir(): string {
    return process.env.CARGO_TARGET_DIR ?? "target"
}

const defaults: Config = {
    daemon: "http://127.0.0.1:9420",
    durationMs: 120_000,
    intervalMs: 1_000,
    warmupMs: 5_000,
    readyTimeoutMs: 900_000,
    settleMs: 5_000,
    cooldownMs: 5_000,
    repeat: 1,
    modes: ["off", "auto"],
    outDir: join(cargoTargetDir(), "servo-import-bench", `suite-${timestampSlug(new Date())}`),
    label: "servo-gpu-import-compare",
    profile: "preview",
    features: "servo wgpu servo-gpu-import",
    logLevel: "warn",
    json: false,
    dryRun: false,
}

function usage(): string {
    return `Hypercolor Servo GPU import compare suite

Starts a daemon per mode, runs the observer benchmark, shuts the daemon down,
and writes a portable suite report for comparing laptops/desktops.

Usage:
  bun scripts/servo-gpu-import-compare.ts [options]
  just servo-import-compare -- [options]

Options:
  --modes <off,auto,on>       Mode order [${defaults.modes.join(",")}]
  --repeat <n>                Repeat the mode sequence [${defaults.repeat}]
  --duration <30s|2m|1500ms>  Observation window [${defaults.durationMs}ms]
  --duration-ms <ms>          Observation window in milliseconds
  --interval-ms <ms>          Status polling interval [${defaults.intervalMs}]
  --warmup-ms <ms>            Exclude initial samples from deltas [${defaults.warmupMs}]
  --settle-ms <ms>            Wait after daemon ready before measuring [${defaults.settleMs}]
  --cooldown-ms <ms>          Wait between daemon runs [${defaults.cooldownMs}]
  --ready-timeout-ms <ms>     Daemon startup timeout, including first compile [${defaults.readyTimeoutMs}]
  --out-dir <path>            Output directory [${defaults.outDir}]
  --label <name>              Suite label [${defaults.label}]
  --daemon <url>              Daemon URL [${defaults.daemon}]
  --profile <profile>         Cargo profile [${defaults.profile}]
  --features <features>       Cargo feature string [${defaults.features}]
  --log-level <level>         Daemon log level [${defaults.logLevel}]
  --dry-run                   Print planned commands without starting anything
  --json                      Print suite report JSON only
  --help                      Show this help
`
}

function parseArgs(argv: string[]): Config {
    const config: Config = { ...defaults, modes: [...defaults.modes] }
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
        if (arg === "--dry-run") {
            config.dryRun = true
            continue
        }

        const value = argv[index + 1]
        if (!value || value.startsWith("--")) {
            throw new Error(`${arg} expects a value`)
        }
        index += 1

        switch (arg) {
            case "--modes":
                config.modes = value.split(",").map((mode) => mode.trim()).filter(Boolean)
                break
            case "--repeat":
                config.repeat = parsePositiveInt(arg, value)
                break
            case "--duration":
                config.durationMs = parseDuration(value)
                break
            case "--duration-ms":
                config.durationMs = parsePositiveInt(arg, value)
                break
            case "--interval-ms":
                config.intervalMs = parsePositiveInt(arg, value)
                break
            case "--warmup-ms":
                config.warmupMs = parseNonNegativeInt(arg, value)
                break
            case "--settle-ms":
                config.settleMs = parseNonNegativeInt(arg, value)
                break
            case "--cooldown-ms":
                config.cooldownMs = parseNonNegativeInt(arg, value)
                break
            case "--ready-timeout-ms":
                config.readyTimeoutMs = parsePositiveInt(arg, value)
                break
            case "--out-dir":
                config.outDir = value
                break
            case "--label":
                config.label = value
                break
            case "--daemon":
                config.daemon = value.replace(/\/+$/, "")
                break
            case "--profile":
                config.profile = value
                break
            case "--features":
                config.features = value
                break
            case "--log-level":
                config.logLevel = value
                break
            default:
                throw new Error(`unknown option: ${arg}`)
        }
    }

    for (const mode of config.modes) {
        if (!["off", "auto", "on"].includes(mode)) {
            throw new Error(`unsupported mode '${mode}', expected off, auto, or on`)
        }
    }
    if (config.warmupMs >= config.durationMs) {
        throw new Error("--warmup-ms must be smaller than --duration")
    }
    return config
}

async function main(): Promise<void> {
    const config = parseArgs(Bun.argv.slice(2))
    const startedAt = new Date()
    const metadata = collectMetadata()
    mkdirSync(config.outDir, { recursive: true })

    if (config.dryRun) {
        printDryRun(config)
        return
    }

    await assertPortFree(config.daemon)

    const runs: RunResult[] = []
    for (let iteration = 1; iteration <= config.repeat; iteration += 1) {
        for (const mode of config.modes) {
            const run = await runMode(config, mode, iteration)
            runs.push(run)
            if (config.cooldownMs > 0) {
                await sleep(config.cooldownMs)
            }
        }
    }

    const report: SuiteReport = {
        ok: runs.every((run) => run.ok),
        label: config.label,
        startedAt: startedAt.toISOString(),
        endedAt: new Date().toISOString(),
        config,
        metadata,
        runs,
        comparison: compareRuns(runs),
    }
    const suitePath = join(config.outDir, "suite.json")
    await Bun.write(suitePath, `${JSON.stringify(report, null, 2)}\n`)

    if (config.json) {
        process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
    } else {
        printReport(report, suitePath)
    }

    process.exit(report.ok ? 0 : 1)
}

async function runMode(config: Config, mode: string, iteration: number): Promise<RunResult> {
    const runName = `run-${String(iteration).padStart(2, "0")}-${mode}`
    const logPath = join(config.outDir, `${runName}.daemon.log`)
    const reportPath = join(config.outDir, `${runName}.json`)
    const label = `${config.label}-${runName}`
    const command = daemonCommand(config, mode)

    console.log(`${palette.purple}${palette.bold}Servo import suite${palette.reset} ${palette.cyan}${runName}${palette.reset}`)
    console.log(`  daemon ${palette.cyan}${command.join(" ")}${palette.reset}`)

    const daemon = startDaemon(command, logPath)
    try {
        await waitForReady(config, daemon)
        if (config.settleMs > 0) {
            await sleep(config.settleMs)
        }
        const benchmark = await runCommandAllowFailure([
            "bun",
            "scripts/servo-gpu-import-benchmark.ts",
            "--daemon",
            config.daemon,
            "--duration-ms",
            String(config.durationMs),
            "--interval-ms",
            String(config.intervalMs),
            "--warmup-ms",
            String(config.warmupMs),
            "--label",
            label,
            "--out",
            reportPath,
        ])
        process.stdout.write(benchmark.stdout)
        process.stderr.write(benchmark.stderr)
        const report = existsSync(reportPath) ? readJson(reportPath) : {}
        return {
            mode,
            iteration,
            ok: benchmark.exitCode === 0 && report.ok === true,
            reportPath,
            logPath,
            summary: objectAt(report, ["summary"]),
            checks: Array.isArray(report.checks) ? report.checks : [],
            output: `${benchmark.stdout}${benchmark.stderr}`,
        }
    } finally {
        await stopDaemon(config, daemon)
    }
}

function daemonCommand(config: Config, mode: string): string[] {
    return [
        "./scripts/servo-cache-build.sh",
        "cargo",
        "run",
        "-p",
        "hypercolor-daemon",
        "--bin",
        "hypercolor-daemon",
        "--profile",
        config.profile,
        "--features",
        config.features,
        "--",
        "--log-level",
        config.logLevel,
        "--compositor-acceleration-mode",
        "gpu",
        "--servo-gpu-import-mode",
        mode,
    ]
}

function startDaemon(command: string[], logPath: string): StartedDaemon {
    const process = Bun.spawn(command, {
        stdin: "ignore",
        stdout: "pipe",
        stderr: "pipe",
    })
    const started: StartedDaemon = {
        process,
        stdout: collectStream(process.stdout),
        stderr: collectStream(process.stderr),
        logPath,
    }
    process.exited.then((exitCode) => {
        started.exitCode = exitCode
    })
    return started
}

async function stopDaemon(config: Config, daemon: StartedDaemon): Promise<void> {
    const pid = discoverListenerPid(config.daemon)
    if (pid) {
        try {
            globalThis.process.kill(pid, "SIGINT")
        } catch {
            daemon.process.kill("SIGINT")
        }
    } else {
        daemon.process.kill("SIGINT")
    }

    const exited = await waitForProcessExit(daemon.process, 30_000)
    if (!exited) {
        daemon.process.kill("SIGTERM")
        await waitForProcessExit(daemon.process, 10_000)
    }

    const [stdout, stderr] = await Promise.all([daemon.stdout, daemon.stderr])
    await Bun.write(daemon.logPath, `${stdout}${stderr}`)
}

async function waitForReady(config: Config, daemon: StartedDaemon): Promise<void> {
    const startedAt = Date.now()
    while (Date.now() - startedAt < config.readyTimeoutMs) {
        if (daemon.exitCode !== undefined) {
            throw new Error(`daemon exited before ready with status ${daemon.exitCode}`)
        }
        if (await isDaemonReady(config.daemon)) {
            return
        }
        await sleep(1_000)
    }
    throw new Error(`daemon did not become ready within ${config.readyTimeoutMs}ms`)
}

async function assertPortFree(daemon: string): Promise<void> {
    if (await isDaemonReady(daemon)) {
        throw new Error(`${daemon} is already serving Hypercolor; stop it before running the compare suite`)
    }
    const pid = discoverListenerPid(daemon)
    if (pid) {
        throw new Error(`${daemon} is already in use by pid ${pid}; refusing to manage an external process`)
    }
}

async function isDaemonReady(daemon: string): Promise<boolean> {
    try {
        const response = await fetch(`${daemon}/api/v1/status`, { signal: AbortSignal.timeout(1_000) })
        return response.ok
    } catch {
        return false
    }
}

function discoverListenerPid(daemon: string): number | undefined {
    const port = daemonPort(daemon)
    if (!port || !isLocalDaemon(daemon)) {
        return undefined
    }
    const output = runText("ss", ["-H", "-ltnp", `sport = :${port}`])
    const pid = Number(/pid=(\d+)/.exec(output ?? "")?.[1])
    return Number.isInteger(pid) && pid > 0 ? pid : undefined
}

function compareRuns(runs: RunResult[]): JsonObject {
    const off = firstSuccessfulRun(runs, "off")
    const auto = firstSuccessfulRun(runs, "auto")
    if (!off || !auto) {
        return {}
    }

    const offTransferMs = perFrame(num(off.summary, "servoReadbackMsDelta"), num(off.summary, "servoCpuFrameDelta"))
    const autoTransferMs = perFrame(num(auto.summary, "servoImportMsDelta"), num(auto.summary, "servoGpuFrameDelta"))
    return {
        offReportPath: off.reportPath,
        autoReportPath: auto.reportPath,
        transferMsPerFrameOff: round(offTransferMs),
        transferMsPerFrameAuto: round(autoTransferMs),
        transferSpeedup: autoTransferMs > 0 ? round(offTransferMs / autoTransferMs) : 0,
        daemonCpuP95DeltaPercentPoints: round(num(auto.summary, "daemonCpuP95Percent") - num(off.summary, "daemonCpuP95Percent")),
        hostCpuP95DeltaPercentPoints: round(num(auto.summary, "hostCpuP95Percent") - num(off.summary, "hostCpuP95Percent")),
        packagePowerP95DeltaWatts: powerDelta(auto.summary, off.summary, "packagePowerP95Watts"),
        psysPowerP95DeltaWatts: powerDelta(auto.summary, off.summary, "psysPowerP95Watts"),
        intelGpuFreqP95DeltaMhz: round(num(auto.summary, "intelGpuFreqP95Mhz") - num(off.summary, "intelGpuFreqP95Mhz")),
        thermalMaxDeltaCelsius: round(num(auto.summary, "thermalMaxCelsius") - num(off.summary, "thermalMaxCelsius")),
    }
}

function powerDelta(left: JsonObject, right: JsonObject, key: string): number {
    const leftValue = num(left, key)
    const rightValue = num(right, key)
    return leftValue > 0 && rightValue > 0 ? round(leftValue - rightValue) : 0
}

function firstSuccessfulRun(runs: RunResult[], mode: string): RunResult | undefined {
    return runs.find((run) => run.mode === mode && run.ok)
}

function collectMetadata(): JsonObject {
    return {
        capturedAt: new Date().toISOString(),
        hostname: hostname(),
        platform: platform(),
        arch: arch(),
        kernel: release(),
        uname: runText("uname", ["-a"])?.trim() ?? "",
        gitRevision: runText("git", ["rev-parse", "--short", "HEAD"])?.trim() ?? "",
        gitDirty: (runText("git", ["status", "--short"]) ?? "").trim().length > 0,
        commands: {
            nvidiaSmi: commandPath("nvidia-smi"),
            powertop: commandPath("powertop"),
            turbostat: commandPath("turbostat"),
            upower: commandPath("upower"),
            sensors: commandPath("sensors"),
        },
        powercap: powercapMetadata(),
        battery: batteryMetadata(),
    }
}

function powercapMetadata(): JsonObject[] {
    const root = "/sys/class/powercap"
    if (!existsSync(root)) {
        return []
    }
    return safeReadDir(root)
        .map((entry) => `${root}/${entry}`)
        .filter((path) => existsSync(`${path}/energy_uj`))
        .map((path) => ({
            path: safeRealPath(path),
            name: readText(`${path}/name`).trim() || path.split("/").at(-1),
            readable: readOptionalNumberFile(`${path}/energy_uj`) !== undefined,
        }))
}

function batteryMetadata(): JsonObject[] {
    const root = "/sys/class/power_supply"
    if (!existsSync(root)) {
        return []
    }
    return safeReadDir(root)
        .map((entry) => `${root}/${entry}`)
        .filter((path) => readText(`${path}/type`).trim() === "Battery")
        .map((path) => ({
            path: safeRealPath(path),
            name: path.split("/").at(-1),
            status: readText(`${path}/status`).trim(),
            capacityPercent: readOptionalNumberFile(`${path}/capacity`) ?? 0,
            powerReadable: existsSync(`${path}/power_now`) || existsSync(`${path}/current_now`),
        }))
}

function printReport(report: SuiteReport, suitePath: string): void {
    const status = report.ok ? `${palette.green}PASS${palette.reset}` : `${palette.red}FAIL${palette.reset}`
    console.log(`${palette.bold}${palette.purple}Servo GPU import compare${palette.reset} ${status}`)
    console.log(`${palette.cyan}${suitePath}${palette.reset}`)
    for (const run of report.runs) {
        console.log(
            `  ${run.mode} #${run.iteration}: ${run.ok ? palette.green : palette.red}${run.ok ? "PASS" : "FAIL"}${palette.reset} · ${palette.cyan}${run.reportPath}${palette.reset}`,
        )
    }

    if (Object.keys(report.comparison).length > 0) {
        console.log(
            `Transfer ms/frame off→auto ${palette.coral}${report.comparison.transferMsPerFrameOff}${palette.reset} → ${palette.coral}${report.comparison.transferMsPerFrameAuto}${palette.reset} (${palette.coral}${report.comparison.transferSpeedup}x${palette.reset})`,
        )
        console.log(
            `Daemon CPU p95 delta auto-off ${palette.coral}${report.comparison.daemonCpuP95DeltaPercentPoints}pp${palette.reset} · host CPU p95 delta ${palette.coral}${report.comparison.hostCpuP95DeltaPercentPoints}pp${palette.reset}`,
        )
        if (Number(report.comparison.packagePowerP95DeltaWatts) !== 0 || Number(report.comparison.psysPowerP95DeltaWatts) !== 0) {
            console.log(
                `Power p95 delta auto-off package/psys ${palette.coral}${report.comparison.packagePowerP95DeltaWatts}W${palette.reset}/${palette.coral}${report.comparison.psysPowerP95DeltaWatts}W${palette.reset}`,
            )
        } else {
            console.log(`${palette.yellow}Power delta unavailable or zero; check suite metadata for RAPL/battery access.${palette.reset}`)
        }
    }
}

function printDryRun(config: Config): void {
    console.log(`${palette.purple}${palette.bold}Servo GPU import compare dry run${palette.reset}`)
    for (let iteration = 1; iteration <= config.repeat; iteration += 1) {
        for (const mode of config.modes) {
            console.log(`[${iteration}/${mode}] ${daemonCommand(config, mode).join(" ")}`)
        }
    }
}

async function runCommandAllowFailure(command: string[]): Promise<{ stdout: string; stderr: string; exitCode: number }> {
    const child = Bun.spawn(command, {
        stdin: "ignore",
        stdout: "pipe",
        stderr: "pipe",
    })
    const [stdout, stderr, exitCode] = await Promise.all([
        collectStream(child.stdout),
        collectStream(child.stderr),
        child.exited,
    ])
    return { stdout, stderr, exitCode }
}

function collectStream(stream: ReadableStream<Uint8Array> | null): Promise<string> {
    return stream ? new Response(stream).text() : Promise.resolve("")
}

async function waitForProcessExit(process: Bun.Subprocess, timeoutMs: number): Promise<boolean> {
    const result = await Promise.race([process.exited.then(() => true), sleep(timeoutMs).then(() => false)])
    return result
}

function readJson(path: string): JsonObject {
    return JSON.parse(readFileSync(path, "utf8")) as JsonObject
}

function objectAt(value: unknown, path: string[]): JsonObject {
    let current = value
    for (const key of path) {
        if (!isObject(current)) {
            return {}
        }
        current = current[key]
    }
    return isObject(current) ? current : {}
}

function isObject(value: unknown): value is JsonObject {
    return typeof value === "object" && value !== null && !Array.isArray(value)
}

function num(value: JsonObject, key: string): number {
    const found = value[key]
    return typeof found === "number" && Number.isFinite(found) ? found : 0
}

function perFrame(totalMs: number, frames: number): number {
    return frames > 0 ? totalMs / frames : 0
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

function commandPath(command: string): string {
    return runText("bash", ["-lc", `command -v ${shellQuote(command)}`])?.trim() ?? ""
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

function timestampSlug(value: Date): string {
    return value.toISOString().replace(/\.\d{3}Z$/, "Z").replace(/[:]/g, "")
}

function readOptionalNumberFile(path: string): number | undefined {
    const text = readText(path).trim()
    if (text.length === 0) {
        return undefined
    }
    const value = Number(text)
    return Number.isFinite(value) ? value : undefined
}

function readText(path: string): string {
    try {
        return readFileSync(path, "utf8")
    } catch {
        return ""
    }
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
            timeout: 2_000,
            stdio: ["ignore", "pipe", "ignore"],
        })
    } catch {
        return undefined
    }
}

function shellQuote(value: string): string {
    return `'${value.replaceAll("'", "'\\''")}'`
}

function round(value: number): number {
    return Math.round(value * 100) / 100
}

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
}

main().catch((error: unknown) => {
    const message = error instanceof Error ? error.message : String(error)
    console.error(`${palette.red}servo import compare failed:${palette.reset} ${message}`)
    process.exit(1)
})
