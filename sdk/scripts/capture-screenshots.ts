#!/usr/bin/env bun
/**
 * Effect screenshot capture tool.
 *
 * Walks the daemon's effect catalog, applies each effect (and up to 3 presets),
 * pulls frames from the canvas WebSocket channel at native resolution, ranks
 * them by an HSV quality heuristic, and saves the top 3 as PNGs under
 * effects/screenshots/drafts/<slug>/<variant>/rank-{1,2,3}.png.
 *
 * Run `--promote` after curating to re-encode rank-1 PNGs into the curated/
 * tree as WebP at quality 0.92.
 */

import { mkdir, readdir, readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

import sharp from 'sharp'

const SDK_ROOT = resolve(import.meta.dirname, '..')
const SCREENSHOTS_ROOT = resolve(SDK_ROOT, '..', 'effects', 'screenshots')
const DRAFTS_ROOT = resolve(SCREENSHOTS_ROOT, 'drafts')
const CURATED_ROOT = resolve(SCREENSHOTS_ROOT, 'curated')

const DEFAULT_DAEMON = 'http://127.0.0.1:9420'
const DEFAULT_FRAMES = 8
const DEFAULT_WARMUP_MS = 4000
const DEFAULT_CAPTURE_MS = 6000
const DEFAULT_KEEP = 3
const MAX_PRESETS_PER_EFFECT = 3

/** Effect slugs we skip entirely — utility/diagnostic tools, not visual effects. */
const SKIP_SLUGS = new Set([
    'calibration',
    'screen-cast',
    'sensor-grid',
    'solid-color',
    'web-viewport',
])

/** Effect tags that mark utility effects. */
const SKIP_TAGS = new Set(['utility', 'calibration'])

interface CliOptions {
    daemon: string
    effectFilter: string | null
    presetsOnly: boolean
    noPresets: boolean
    promote: boolean
    framesPerVariant: number
    warmupMs: number
    captureMs: number
    keepTopN: number
}

interface EffectSummary {
    id: string
    name: string
    description: string
    category: string
    source: string
    runnable: boolean
    tags: string[]
    version: string
}

interface PresetTemplate {
    name: string
    description?: string
    controls: Record<string, unknown>
}

interface EffectDetail extends EffectSummary {
    controls: unknown[]
    presets: PresetTemplate[]
}

interface Variant {
    key: string
    label: string
    presetName: string | null
}

interface CapturedFrame {
    width: number
    height: number
    rgba: Uint8Array
    score: number
    meanSaturation: number
    meanLuminance: number
    luminanceVariance: number
    capturedAtMs: number
}

interface FrameHeader {
    width: number
    height: number
    format: 'rgb' | 'rgba'
    payload: Uint8Array
}

const CANVAS_HEADER_BYTE = 0x03
const MIN_LUMINANCE = 0.08
const MIN_SATURATION = 0.15

function parseArgs(argv: readonly string[]): CliOptions {
    const opts: CliOptions = {
        daemon: DEFAULT_DAEMON,
        effectFilter: null,
        presetsOnly: false,
        noPresets: false,
        promote: false,
        framesPerVariant: DEFAULT_FRAMES,
        warmupMs: DEFAULT_WARMUP_MS,
        captureMs: DEFAULT_CAPTURE_MS,
        keepTopN: DEFAULT_KEEP,
    }

    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index]
        const next = argv[index + 1]
        switch (arg) {
            case '--daemon':
                if (!next) throw new Error('--daemon requires a URL')
                opts.daemon = next.replace(/\/$/, '')
                index += 1
                break
            case '--effect':
                if (!next) throw new Error('--effect requires a slug or name')
                opts.effectFilter = next
                index += 1
                break
            case '--presets-only':
                opts.presetsOnly = true
                break
            case '--no-presets':
                opts.noPresets = true
                break
            case '--promote':
                opts.promote = true
                break
            case '--frames':
                if (!next) throw new Error('--frames requires a number')
                opts.framesPerVariant = Number.parseInt(next, 10)
                index += 1
                break
            case '--warmup':
                if (!next) throw new Error('--warmup requires milliseconds')
                opts.warmupMs = Number.parseInt(next, 10)
                index += 1
                break
            case '--duration':
                if (!next) throw new Error('--duration requires milliseconds')
                opts.captureMs = Number.parseInt(next, 10)
                index += 1
                break
            case '--keep':
                if (!next) throw new Error('--keep requires a number')
                opts.keepTopN = Number.parseInt(next, 10)
                index += 1
                break
            case '-h':
            case '--help':
                printHelp()
                process.exit(0)
            default:
                throw new Error(`unknown argument: ${arg}`)
        }
    }

    if (opts.presetsOnly && opts.noPresets) {
        throw new Error('--presets-only and --no-presets are mutually exclusive')
    }
    return opts
}

function printHelp(): void {
    process.stdout.write(`capture-screenshots — walk the daemon's effect catalog and grab screenshots

usage:
  bun sdk/scripts/capture-screenshots.ts [flags]
  bun sdk/scripts/capture-screenshots.ts --promote

flags:
  --daemon <url>        daemon base URL (default ${DEFAULT_DAEMON})
  --effect <slug|name>  capture a single effect
  --presets-only        skip the default-controls variant
  --no-presets          capture only the default variant per effect
  --frames <n>          frames sampled per variant (default ${DEFAULT_FRAMES})
  --warmup <ms>         wait this long after apply before collecting (default ${DEFAULT_WARMUP_MS})
  --duration <ms>       sampling window (default ${DEFAULT_CAPTURE_MS})
  --keep <n>            frames kept per variant after ranking (default ${DEFAULT_KEEP})
  --promote             re-encode rank-1 drafts into curated/ as WebP q=0.92
`)
}

// ── slug helpers ──────────────────────────────────────────────────────────

export function slugify(value: string): string {
    return value
        .normalize('NFKD')
        .replace(/[\u0300-\u036f]/g, '')
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-+|-+$/g, '')
}

function isUtility(effect: EffectSummary): boolean {
    const slug = slugify(effect.name)
    if (SKIP_SLUGS.has(slug)) return true
    // "display" category effects are faces that attach to display devices,
    // not the LED pipeline — the apply endpoint rejects them with a 422.
    if (
        effect.category === 'utility' ||
        effect.category === 'calibration' ||
        effect.category === 'display'
    ) {
        return true
    }
    for (const tag of effect.tags) {
        if (SKIP_TAGS.has(tag)) return true
    }
    return false
}

// ── REST client ───────────────────────────────────────────────────────────

async function restGet<T>(daemon: string, path: string): Promise<T> {
    const res = await fetch(`${daemon}${path}`, { headers: { accept: 'application/json' } })
    if (!res.ok) throw new Error(`${path} failed: ${res.status} ${res.statusText}`)
    const body = (await res.json()) as { data: T }
    return body.data
}

async function restPost<T>(daemon: string, path: string, body: unknown = {}): Promise<T> {
    const res = await fetch(`${daemon}${path}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', accept: 'application/json' },
        body: JSON.stringify(body),
    })
    if (!res.ok && res.status !== 404) {
        throw new Error(`${path} failed: ${res.status} ${res.statusText}`)
    }
    const json = (await res.json().catch(() => ({}))) as { data?: T }
    return (json.data ?? ({} as T)) as T
}

async function listEffects(daemon: string): Promise<EffectSummary[]> {
    const data = await restGet<{ items: EffectSummary[] }>(daemon, '/api/v1/effects')
    return data.items
}

async function getEffectDetail(daemon: string, effectId: string): Promise<EffectDetail> {
    return await restGet<EffectDetail>(daemon, `/api/v1/effects/${encodeURIComponent(effectId)}`)
}

/**
 * Unwrap the daemon's typed `ControlValue` JSON (`{"float": 84}`, `{"color": [...]}`,
 * `{"enum": "Horizontal"}`) into plain JSON — which is what the apply endpoint's
 * `json_to_control_value` expects. Values already in plain shape pass through.
 */
function unwrapControlValues(
    controls: Record<string, unknown>,
): Record<string, unknown> {
    const out: Record<string, unknown> = {}
    for (const [name, value] of Object.entries(controls)) {
        out[name] = unwrapControlValue(value)
    }
    return out
}

function unwrapControlValue(value: unknown): unknown {
    if (value && typeof value === 'object' && !Array.isArray(value)) {
        const entries = Object.entries(value as Record<string, unknown>)
        if (entries.length === 1) {
            const [tag, inner] = entries[0] ?? []
            if (typeof tag === 'string' && isControlValueTag(tag)) return inner
        }
    }
    return value
}

function isControlValueTag(tag: string): boolean {
    return (
        tag === 'float' ||
        tag === 'integer' ||
        tag === 'boolean' ||
        tag === 'color' ||
        tag === 'gradient' ||
        tag === 'enum' ||
        tag === 'text' ||
        tag === 'rect'
    )
}

async function applyEffect(
    daemon: string,
    effectId: string,
    presetControls: Record<string, unknown> | null,
): Promise<void> {
    const body = presetControls ? { controls: unwrapControlValues(presetControls) } : {}
    await restPost(daemon, `/api/v1/effects/${encodeURIComponent(effectId)}/apply`, body)
}

async function stopEffect(daemon: string): Promise<void> {
    await restPost(daemon, '/api/v1/effects/stop')
}

// ── WebSocket frame collection ────────────────────────────────────────────

function parseCanvasFrame(buffer: ArrayBuffer): FrameHeader | null {
    const bytes = new Uint8Array(buffer)
    if (bytes.length < 14) return null
    if (bytes[0] !== CANVAS_HEADER_BYTE) return null
    const view = new DataView(buffer)
    const width = view.getUint16(9, true)
    const height = view.getUint16(11, true)
    const formatByte = view.getUint8(13)
    const format = formatByte === 1 ? 'rgba' : formatByte === 0 ? 'rgb' : null
    if (!format) return null
    if (width === 0 || height === 0) return null
    return { width, height, format, payload: bytes.subarray(14) }
}

function rgbToRgba(payload: Uint8Array, width: number, height: number): Uint8Array {
    const pixelCount = width * height
    const out = new Uint8Array(pixelCount * 4)
    for (let i = 0, j = 0, k = 0; i < pixelCount; i += 1, j += 3, k += 4) {
        out[k] = payload[j] ?? 0
        out[k + 1] = payload[j + 1] ?? 0
        out[k + 2] = payload[j + 2] ?? 0
        out[k + 3] = 255
    }
    return out
}

function collectFrames(
    daemon: string,
    frameCount: number,
    captureMs: number,
): Promise<CapturedFrame[]> {
    const wsUrl = `${daemon.replace(/^http/, 'ws')}/api/v1/ws`
    return new Promise((resolve, reject) => {
        const ws = new WebSocket(wsUrl)
        ws.binaryType = 'arraybuffer'
        const frames: CapturedFrame[] = []
        let captureInterval: ReturnType<typeof setInterval> | null = null
        let startedAt = 0
        let latestFrame: FrameHeader | null = null
        let finished = false

        const finish = (reason: 'ok' | 'timeout' | 'error', err?: Error) => {
            if (finished) return
            finished = true
            if (captureInterval) clearInterval(captureInterval)
            try {
                ws.close()
            } catch {
                /* ignore */
            }
            if (reason === 'error' && err) reject(err)
            else resolve(frames)
        }

        const takeSample = () => {
            if (!latestFrame) return
            const { width, height, format, payload } = latestFrame
            const rgba = format === 'rgba' ? new Uint8Array(payload) : rgbToRgba(payload, width, height)
            frames.push({
                width,
                height,
                rgba,
                score: 0,
                meanSaturation: 0,
                meanLuminance: 0,
                luminanceVariance: 0,
                capturedAtMs: Date.now() - startedAt,
            })
            if (frames.length >= frameCount) finish('ok')
        }

        ws.addEventListener('open', () => {
            ws.send(
                JSON.stringify({
                    type: 'subscribe',
                    channels: ['canvas'],
                    config: { canvas: { fps: 30, format: 'rgba', width: 0, height: 0 } },
                }),
            )
            startedAt = Date.now()
            const interval = Math.max(1, Math.floor(captureMs / frameCount))
            captureInterval = setInterval(takeSample, interval)
            setTimeout(() => finish('timeout'), captureMs + 2000)
        })

        ws.addEventListener('message', (event) => {
            if (!(event.data instanceof ArrayBuffer)) return
            const parsed = parseCanvasFrame(event.data)
            if (parsed) latestFrame = parsed
        })

        ws.addEventListener('error', () => finish('error', new Error('websocket error')))
        ws.addEventListener('close', () => {
            if (!finished) finish('ok')
        })
    })
}

// ── frame scoring ─────────────────────────────────────────────────────────

const DOWNSAMPLE_GRID = 32

function scoreFrame(frame: CapturedFrame): void {
    const { width, height, rgba } = frame
    const gridSize = DOWNSAMPLE_GRID
    const sampleCount = gridSize * gridSize
    const stepX = width / gridSize
    const stepY = height / gridSize

    let satSum = 0
    let lumSum = 0
    const lumValues = new Float32Array(sampleCount)

    for (let gy = 0; gy < gridSize; gy += 1) {
        const py = Math.min(height - 1, Math.floor(gy * stepY))
        for (let gx = 0; gx < gridSize; gx += 1) {
            const px = Math.min(width - 1, Math.floor(gx * stepX))
            const idx = (py * width + px) * 4
            const r = (rgba[idx] ?? 0) / 255
            const g = (rgba[idx + 1] ?? 0) / 255
            const b = (rgba[idx + 2] ?? 0) / 255
            const max = Math.max(r, g, b)
            const min = Math.min(r, g, b)
            const sat = max === 0 ? 0 : (max - min) / max
            const lum = 0.299 * r + 0.587 * g + 0.114 * b
            satSum += sat
            lumSum += lum
            lumValues[gy * gridSize + gx] = lum
        }
    }

    const meanSat = satSum / sampleCount
    const meanLum = lumSum / sampleCount
    let varianceAcc = 0
    for (let i = 0; i < sampleCount; i += 1) {
        const delta = (lumValues[i] ?? 0) - meanLum
        varianceAcc += delta * delta
    }
    const lumVariance = varianceAcc / sampleCount
    // Variance scales roughly 0..0.25 for LED-style frames; normalize to 0..1 with a soft cap.
    const lumVarianceNorm = Math.min(1, lumVariance / 0.08)

    frame.meanSaturation = meanSat
    frame.meanLuminance = meanLum
    frame.luminanceVariance = lumVariance
    // Reject frames that are too dark or too grayscale outright.
    if (meanLum < MIN_LUMINANCE || meanSat < MIN_SATURATION) {
        frame.score = 0
        return
    }
    frame.score = meanSat * 0.6 + lumVarianceNorm * 0.4
}

// ── file IO ───────────────────────────────────────────────────────────────

async function writeDraftFrame(
    slug: string,
    variantKey: string,
    rank: number,
    frame: CapturedFrame,
): Promise<string> {
    const dir = resolve(DRAFTS_ROOT, slug, variantKey)
    await mkdir(dir, { recursive: true })
    const filePath = resolve(dir, `rank-${rank}.png`)
    await sharp(frame.rgba, { raw: { width: frame.width, height: frame.height, channels: 4 } })
        .png({ compressionLevel: 6 })
        .toFile(filePath)
    return filePath
}

async function promoteRank1(): Promise<number> {
    let promoted = 0
    let slugEntries: string[]
    try {
        slugEntries = await readdir(DRAFTS_ROOT)
    } catch {
        process.stderr.write(`no drafts directory at ${DRAFTS_ROOT}\n`)
        return 0
    }

    for (const slug of slugEntries) {
        const slugDir = resolve(DRAFTS_ROOT, slug)
        let variants: string[]
        try {
            variants = await readdir(slugDir)
        } catch {
            continue
        }
        for (const variantKey of variants) {
            const rank1 = resolve(slugDir, variantKey, 'rank-1.png')
            try {
                const bytes = await readFile(rank1)
                const outDir = resolve(CURATED_ROOT, slug)
                await mkdir(outDir, { recursive: true })
                const outPath = resolve(outDir, `${variantKey}.webp`)
                await sharp(bytes).webp({ quality: 92, effort: 4 }).toFile(outPath)
                promoted += 1
                process.stdout.write(`promoted ${slug}/${variantKey}\n`)
            } catch {
                // rank-1 missing for this variant — skip
            }
        }
    }
    return promoted
}

// ── capture orchestration ─────────────────────────────────────────────────

function buildVariants(detail: EffectDetail, opts: CliOptions): Variant[] {
    const variants: Variant[] = []
    if (!opts.presetsOnly) {
        variants.push({ key: 'default', label: 'default', presetName: null })
    }
    if (!opts.noPresets) {
        const presetCount = Math.min(detail.presets.length, MAX_PRESETS_PER_EFFECT)
        for (let index = 0; index < presetCount; index += 1) {
            const preset = detail.presets[index]
            if (!preset) continue
            const key = slugify(preset.name)
            if (!key) continue
            variants.push({ key, label: preset.name, presetName: preset.name })
        }
    }
    return variants
}

async function captureVariant(
    opts: CliOptions,
    effect: EffectDetail,
    variant: Variant,
): Promise<void> {
    const slug = slugify(effect.name)
    const label = `${effect.name} · ${variant.label}`
    process.stdout.write(`\n▸ ${label}\n`)

    const presetControls =
        variant.presetName === null
            ? null
            : (effect.presets.find((p) => p.name === variant.presetName)?.controls ?? null)

    await applyEffect(opts.daemon, effect.id, presetControls)
    await sleep(opts.warmupMs)

    let frames: CapturedFrame[]
    try {
        frames = await collectFrames(opts.daemon, opts.framesPerVariant, opts.captureMs)
    } catch (err) {
        process.stderr.write(`  ✗ capture failed: ${String(err)}\n`)
        return
    }

    if (frames.length === 0) {
        process.stderr.write('  ✗ no frames collected\n')
        return
    }

    for (const frame of frames) scoreFrame(frame)
    const ranked = [...frames].sort((a, b) => b.score - a.score)
    const aboveFloor = ranked.filter((frame) => frame.score > 0)
    // Particle effects (fiberflies, meteor storm, digital rain) are legitimately
    // sparse — the quality floor will reject all their frames on mean-saturation
    // alone. Fall back to the raw top-K so every variant gets drafts to review;
    // keep a flag so the stdout log can flag the fallback.
    const fallbackUsed = aboveFloor.length === 0
    const kept = (fallbackUsed ? ranked : aboveFloor).slice(0, opts.keepTopN)

    if (kept.length === 0) {
        process.stderr.write(`  ✗ no frames captured\n`)
        return
    }
    if (fallbackUsed) {
        process.stdout.write('  ⚠ all frames below quality floor; kept top-K anyway\n')
    }

    for (let rank = 0; rank < kept.length; rank += 1) {
        const frame = kept[rank]
        if (!frame) continue
        const path = await writeDraftFrame(slug, variant.key, rank + 1, frame)
        process.stdout.write(
            `  ✓ rank-${rank + 1}: ${path.replace(SDK_ROOT, 'sdk')}  ` +
                `(sat ${frame.meanSaturation.toFixed(2)}  ` +
                `lum ${frame.meanLuminance.toFixed(2)}  ` +
                `var ${frame.luminanceVariance.toFixed(3)}  ` +
                `score ${frame.score.toFixed(3)})\n`,
        )
    }
}

function sleep(ms: number): Promise<void> {
    return new Promise((res) => setTimeout(res, ms))
}

// ── main ──────────────────────────────────────────────────────────────────

async function main(): Promise<void> {
    const opts = parseArgs(process.argv.slice(2))

    if (opts.promote) {
        const count = await promoteRank1()
        process.stdout.write(`\npromoted ${count} variants to curated/\n`)
        return
    }

    process.stdout.write(`daemon: ${opts.daemon}\n`)
    const effects = await listEffects(opts.daemon)
    const runnable = effects.filter((e) => e.runnable && !isUtility(e))
    const filtered = opts.effectFilter
        ? runnable.filter((e) => slugify(e.name) === slugify(opts.effectFilter ?? ''))
        : runnable

    if (filtered.length === 0) {
        if (opts.effectFilter) throw new Error(`no runnable effect matched ${opts.effectFilter}`)
        throw new Error('no runnable effects available')
    }

    process.stdout.write(`queued ${filtered.length} effect(s)\n`)
    await mkdir(DRAFTS_ROOT, { recursive: true })

    const startedAt = Date.now()
    for (const effect of filtered) {
        let detail: EffectDetail
        try {
            detail = await getEffectDetail(opts.daemon, effect.id)
        } catch (err) {
            process.stderr.write(`✗ ${effect.name}: failed to fetch detail: ${String(err)}\n`)
            continue
        }
        const variants = buildVariants(detail, opts)
        for (const variant of variants) {
            try {
                await captureVariant(opts, detail, variant)
            } catch (err) {
                process.stderr.write(
                    `✗ ${effect.name} · ${variant.label}: ${String(err)}\n`,
                )
            }
        }
    }

    try {
        await stopEffect(opts.daemon)
    } catch {
        // best effort — daemon may already be idle
    }

    const elapsedSec = Math.round((Date.now() - startedAt) / 1000)
    process.stdout.write(`\nfinished in ${elapsedSec}s\n`)
    process.stdout.write(`drafts at ${DRAFTS_ROOT}\n`)
    process.stdout.write(`run again with --promote to copy rank-1 frames into curated/\n`)
}

main().catch((err) => {
    process.stderr.write(`${err instanceof Error ? err.stack ?? err.message : String(err)}\n`)
    process.exit(1)
})
