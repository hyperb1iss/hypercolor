#!/usr/bin/env bun

type DiagnoseCheck = {
    category: string
    name: string
    status: string
    detail: string
}

type DiagnoseData = {
    checks: DiagnoseCheck[]
    summary: {
        passed: number
        warnings: number
        failed: number
    }
    snapshot: {
        render?: {
            latest_frame?: Record<string, unknown> | null
            recent_window?: Record<string, unknown>
        }
        usb?: Record<string, unknown>
        device_output?: {
            queues: number
            usb_queues: number
            lagging_queues: number
            dropped_frames_total: number
            errors_total: number
            items: DeviceOutputItem[]
        }
    }
}

type DeviceOutputItem = {
    id: string
    backend_id: string
    fps_sent: number
    fps_queued: number
    fps_target: number
    frames_dropped: number
    errors_total: number
    worker_finished: boolean
    avg_queue_wait_ms: number
    avg_write_ms: number
    last_sent_ago_ms?: number | null
    last_error?: string | null
}

type ApiEnvelope = {
    data?: DiagnoseData
}

type Config = {
    api: string
    json: boolean
    system: boolean
    checks?: string[]
}

const palette = {
    purple: "\x1b[38;2;225;53;255m",
    cyan: "\x1b[38;2;128;255;234m",
    yellow: "\x1b[38;2;241;250;140m",
    green: "\x1b[38;2;80;250;123m",
    red: "\x1b[38;2;255;99;99m",
    reset: "\x1b[0m",
}

const useColor = process.stdout.isTTY && !process.env.NO_COLOR

function color(value: string, code: string): string {
    return useColor ? `${code}${value}${palette.reset}` : value
}

function usage(): never {
    console.log(`Hypercolor daemon diagnostics

Usage:
  bun scripts/diagnose-daemon.ts [--api URL] [--checks a,b,c] [--json]

Options:
  --api URL       Daemon base URL. Default: http://127.0.0.1:9420
  --daemon URL    Alias for --api
  --checks LIST   Comma-separated diagnose checks to request
  --json          Print raw JSON response
  --no-system     Omit system uptime check
`)
    process.exit(0)
}

function parseArgs(argv: string[]): Config {
    const config: Config = {
        api: "http://127.0.0.1:9420",
        json: false,
        system: true,
    }

    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index]
        switch (arg) {
            case "--api":
            case "--daemon":
                config.api = requireValue(argv, (index += 1), arg)
                break
            case "--checks":
                config.checks = requireValue(argv, (index += 1), arg)
                    .split(",")
                    .map((value) => value.trim())
                    .filter(Boolean)
                break
            case "--json":
                config.json = true
                break
            case "--no-system":
                config.system = false
                break
            case "-h":
            case "--help":
                usage()
                break
            default:
                if (arg.startsWith("-")) {
                    throw new Error(`Unknown option: ${arg}`)
                }
                config.api = arg
                break
        }
    }

    return config
}

function requireValue(argv: string[], index: number, flag: string): string {
    const value = argv[index]
    if (!value || value.startsWith("-")) {
        throw new Error(`${flag} requires a value`)
    }
    return value
}

async function postDiagnose(config: Config): Promise<ApiEnvelope> {
    const controller = new AbortController()
    const timeout = setTimeout(() => controller.abort(), 3_000)
    try {
        const response = await fetch(`${config.api.replace(/\/$/, "")}/api/v1/diagnose`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({
                system: config.system,
                checks: config.checks,
            }),
            signal: controller.signal,
        })

        if (!response.ok) {
            throw new Error(`diagnose request failed: ${response.status} ${response.statusText}`)
        }

        return (await response.json()) as ApiEnvelope
    } finally {
        clearTimeout(timeout)
    }
}

function printReport(data: DiagnoseData, api: string): void {
    console.log(color("Hypercolor Daemon Diagnostics", palette.purple))
    console.log(`API ${color(api, palette.cyan)}`)
    console.log(
        `Summary pass=${color(String(data.summary.passed), palette.green)} warn=${color(
            String(data.summary.warnings),
            palette.yellow,
        )} fail=${color(String(data.summary.failed), palette.red)}`,
    )

    console.log("")
    console.log(color("Checks", palette.purple))
    for (const check of data.checks) {
        const statusColor =
            check.status === "pass"
                ? palette.green
                : check.status === "fail"
                  ? palette.red
                  : palette.yellow
        console.log(
            `  ${color(check.status.padEnd(7), statusColor)} ${color(
                `${check.category}.${check.name}`,
                palette.cyan,
            )} ${check.detail}`,
        )
    }

    const latest = data.snapshot.render?.latest_frame
    if (latest) {
        console.log("")
        console.log(color("Latest LED Frame", palette.purple))
        console.log(
            `  frame=${latest.frame_token} source=${latest.output_frame_source} stale=${latest.gpu_sample_stale} sample_us=${latest.sample_us} push_us=${latest.push_us} devices=${latest.devices_written} leds=${latest.total_leds}`,
        )
        console.log(
            `  gpu deferred=${latest.gpu_sample_deferred} retry=${latest.gpu_sample_retry_hit} queue_saturated=${latest.gpu_sample_queue_saturated} wait_blocked=${latest.gpu_sample_wait_blocked}`,
        )
    }

    const window = data.snapshot.render?.recent_window
    if (window) {
        console.log("")
        console.log(color("Recent Render Window", palette.purple))
        console.log(
            `  frames=${window.frames} current=${window.output_current_frame} published=${window.output_published_frame} routed_reuse=${window.output_routed_reuse} reused_published=${window.output_reused_published_frame}`,
        )
        console.log(
            `  stale=${window.gpu_sample_stale} deferred=${window.gpu_sample_deferred} push_avg_ms=${window.push_avg_ms} push_p95_ms=${window.push_p95_ms}`,
        )
    }

    const usb = data.snapshot.usb
    if (usb) {
        console.log("")
        console.log(color("USB Actor", palette.purple))
        console.log(
            `  display_frames=${usb.display_frames_total} delayed_for_led=${usb.display_frames_delayed_for_led_total} wait_avg_ms=${usb.display_led_priority_wait_avg_ms} wait_max_ms=${usb.display_led_priority_wait_max_ms}`,
        )
    }

    const output = data.snapshot.device_output
    if (output) {
        console.log("")
        console.log(color("Device Output Queues", palette.purple))
        console.log(
            `  queues=${output.queues} usb=${output.usb_queues} lagging=${output.lagging_queues} dropped_total=${output.dropped_frames_total} errors_total=${output.errors_total}`,
        )
        for (const item of output.items) {
            const status = item.worker_finished || item.errors_total > 0 ? "warn" : "ok"
            const statusColor = status === "ok" ? palette.green : palette.yellow
            console.log(
                `  ${color(status.padEnd(4), statusColor)} ${color(
                    `${item.backend_id}:${item.id}`,
                    palette.cyan,
                )} fps=${item.fps_sent.toFixed(1)}/${item.fps_queued.toFixed(
                    1,
                )} target=${item.fps_target} dropped=${item.frames_dropped} queue_wait_ms=${item.avg_queue_wait_ms} write_ms=${item.avg_write_ms} last_sent_ms=${
                    item.last_sent_ago_ms ?? "n/a"
                }`,
            )
            if (item.last_error) {
                console.log(`       last_error=${item.last_error}`)
            }
        }
    }
}

async function main(): Promise<void> {
    const config = parseArgs(process.argv.slice(2))
    const envelope = await postDiagnose(config)
    if (!envelope.data) {
        throw new Error("diagnose response did not include data")
    }

    if (config.json) {
        console.log(JSON.stringify(envelope, null, 2))
        return
    }

    printReport(envelope.data, config.api)
}

main().catch((error: unknown) => {
    const message = error instanceof Error ? error.message : String(error)
    console.error(color(`diagnose failed: ${message}`, palette.red))
    process.exit(1)
})
