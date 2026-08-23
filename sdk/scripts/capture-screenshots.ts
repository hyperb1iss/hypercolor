#!/usr/bin/env bun
/**
 * Effect screenshot capture tool.
 *
 * Walks the daemon's effect catalog, applies each effect (and up to 3 presets),
 * pulls bounded frames from the canvas WebSocket channel, ranks
 * them by an HSV quality heuristic, and saves the top 3 as PNGs under
 * effects/screenshots/drafts/<slug>/<variant>/rank-{1,2,3}.png.
 *
 * Run `--promote` after curating to re-encode rank-1 PNGs into the curated/
 * tree as WebP at quality 0.92.
 */

import { readFileSync } from 'node:fs'
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
const CAPTURE_WIDTH = 640
const CAPTURE_HEIGHT = 360
const PREVIEW_TRANSPORT =
    'preview_transport_v2:decoded=536870912,encoded=536936448,connection=1073872896,reassembly=8388608,tombstones=4194304,sender=8388608,cursors=8388608,idle_ms=5000,message=1048576'

/**
 * Effect slugs we skip entirely — utility/diagnostic tools, not visual effects.
 *
 * `ambilight`, `screen-cast` and `web-viewport` render host screen content, so
 * a capture would bake whatever was on the operator's monitor into artwork that
 * ships publicly. Never capture these.
 */
const SKIP_SLUGS = new Set(['ambilight', 'calibration', 'screen-cast', 'sensor-grid', 'solid-color', 'web-viewport'])

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
    // The daemon omits `presets` entirely for effects that ship none.
    presets?: PresetTemplate[]
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

// ── Protocol manifest ─────────────────────────────────────────────────────
//
// protocol/websocket-v1.json is the one definition of the binary frame
// layouts. Reading the offsets from it means a layout change moves this
// decoder with it instead of silently shifting every field past the edit.

const PROTOCOL_MANIFEST_PATH = resolve(SDK_ROOT, '..', 'protocol', 'websocket-v1.json')

const FIXED_FIELD_WIDTHS: Record<string, number> = {
    f32_le: 4,
    u8: 1,
    u16_le: 2,
    u32_le: 4,
    u64_le: 8,
    uuid: 16,
}

interface ManifestFrameLayout {
    prefixLen: number
    offsets: Record<string, number>
    types: Record<string, string>
}

interface ProtocolManifest {
    binary_messages: { name: string; tag: number; layout: string | [string, string][]; topic: string }[]
    preview_frame: { formats: Record<string, number> }
    [key: string]: unknown
}

function readManifest(): ProtocolManifest {
    return JSON.parse(readFileSync(PROTOCOL_MANIFEST_PATH, 'utf8')) as ProtocolManifest
}

function frameLayout(manifest: ProtocolManifest, name: string): ManifestFrameLayout {
    const definition = manifest[name] as { layout: [string, string][] } | undefined
    if (!definition) throw new Error(`protocol manifest has no ${name} layout`)
    const offsets: Record<string, number> = {}
    const types: Record<string, string> = {}
    let offset = 0
    for (const [fieldType, fieldName] of definition.layout) {
        types[fieldName] = fieldType
        offsets[fieldName] = offset
        const width = FIXED_FIELD_WIDTHS[fieldType]
        if (width === undefined) break
        offset += width
    }
    return { offsets, prefixLen: offset, types }
}

function readField(view: DataView, layout: ManifestFrameLayout, name: string): number {
    const offset = layout.offsets[name]
    const fieldType = layout.types[name]
    if (offset === undefined || fieldType === undefined) {
        throw new Error(`protocol layout has no field ${name}`)
    }
    switch (fieldType) {
        case 'u8':
            return view.getUint8(offset)
        case 'u16_le':
            return view.getUint16(offset, true)
        case 'u32_le':
            return view.getUint32(offset, true)
        default:
            throw new Error(`unsupported protocol field type ${fieldType}`)
    }
}

const PROTOCOL = (() => {
    const manifest = readManifest()
    const compact = frameLayout(manifest, 'preview_frame')
    const wide = frameLayout(manifest, 'wide_preview_frame')
    const canvasTag = manifest.binary_messages.find((m) => m.name === 'canvas')?.tag
    const wideTag = manifest.binary_messages.find((m) => m.layout === 'wide_preview_frame')?.tag
    if (canvasTag === undefined) throw new Error('protocol manifest has no canvas message')
    if (wideTag === undefined) throw new Error('protocol manifest has no wide preview message')
    const formats = new Map<number, string>(
        Object.entries(manifest.preview_frame.formats).map(([name, tag]) => [tag, name]),
    )
    const chunkTags = new Set(
        manifest.binary_messages
            .filter((m) => m.layout === 'preview_chunk_frame' || m.layout === 'preview_cancel_frame')
            .map((m) => m.tag),
    )
    return { canvasTag, chunkTags, compact, formats, wide, wideTag }
})()

const MIN_LUMINANCE = 0.08
const MIN_SATURATION = 0.15

function parseArgs(argv: readonly string[]): CliOptions {
    const opts: CliOptions = {
        captureMs: DEFAULT_CAPTURE_MS,
        daemon: DEFAULT_DAEMON,
        effectFilter: null,
        framesPerVariant: DEFAULT_FRAMES,
        keepTopN: DEFAULT_KEEP,
        noPresets: false,
        presetsOnly: false,
        promote: false,
        warmupMs: DEFAULT_WARMUP_MS,
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
                break
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
    if (effect.category === 'utility' || effect.category === 'calibration' || effect.category === 'display') {
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
        body: JSON.stringify(body),
        headers: { accept: 'application/json', 'content-type': 'application/json' },
        method: 'POST',
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

async function applyEffect(
    daemon: string,
    effectId: string,
    presetControls: Record<string, unknown> | null,
): Promise<void> {
    const body = presetControls ? { controls: presetControls } : {}
    await restPost(daemon, `/api/v1/effects/${encodeURIComponent(effectId)}/apply`, body)
}

async function stopEffect(daemon: string): Promise<void> {
    await restPost(daemon, '/api/v1/scene/clear')
}

// ── WebSocket frame collection ────────────────────────────────────────────

let warnedAboutChunkedFrames = false

function parseCanvasFrame(buffer: ArrayBuffer): FrameHeader | null {
    const bytes = new Uint8Array(buffer)
    if (bytes.length === 0) return null
    const tag = bytes[0] as number
    if (PROTOCOL.chunkTags.has(tag)) {
        // Chunked publications need the v2 reassembler, which this script does
        // not carry. Say so once instead of leaving an empty capture run
        // looking like the effect simply rendered nothing.
        if (!warnedAboutChunkedFrames) {
            warnedAboutChunkedFrames = true
            process.stderr.write('  ! canvas frames arrived chunked and were skipped\n')
        }
        return null
    }
    const wide = tag === PROTOCOL.wideTag
    if (!wide && tag !== PROTOCOL.canvasTag) return null
    const layout = wide ? PROTOCOL.wide : PROTOCOL.compact
    if (bytes.length < layout.prefixLen) return null
    const view = new DataView(buffer)
    // The compact form's own tag names the channel; the wide form carries it.
    if (wide && readField(view, layout, 'channel_tag') !== PROTOCOL.canvasTag) return null
    const width = readField(view, layout, 'width')
    const height = readField(view, layout, 'height')
    const format = PROTOCOL.formats.get(readField(view, layout, 'format'))
    if (format !== 'rgb' && format !== 'rgba') return null
    if (width === 0 || height === 0) return null
    const payload = bytes.subarray(layout.prefixLen)
    const bytesPerPixel = format === 'rgba' ? 4 : 3
    if (payload.length !== width * height * bytesPerPixel) return null
    return { format, height, payload, width }
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

function collectFrames(daemon: string, frameCount: number, captureMs: number): Promise<CapturedFrame[]> {
    const wsUrl = `${daemon.replace(/^http/, 'ws')}/api/v1/ws`
    return new Promise((resolve, reject) => {
        const ws = new WebSocket(wsUrl)
        ws.binaryType = 'arraybuffer'
        const frames: CapturedFrame[] = []
        let captureInterval: ReturnType<typeof setInterval> | null = null
        let startedAt = 0
        let latestFrame: FrameHeader | null = null
        let finished = false
        let timeout: ReturnType<typeof setTimeout> | null = null

        const finish = (reason: 'ok' | 'timeout' | 'error', err?: Error) => {
            if (finished) return
            finished = true
            if (captureInterval) clearInterval(captureInterval)
            if (timeout) clearTimeout(timeout)
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
                capturedAtMs: Date.now() - startedAt,
                height,
                luminanceVariance: 0,
                meanLuminance: 0,
                meanSaturation: 0,
                rgba,
                score: 0,
                width,
            })
            if (frames.length >= frameCount) finish('ok')
        }

        ws.addEventListener('open', () => {
            ws.send(
                JSON.stringify({
                    preview_transport: PREVIEW_TRANSPORT,
                    topics: [
                        {
                            config: {
                                format: 'rgba',
                                fps: 30,
                                height: CAPTURE_HEIGHT,
                                width: CAPTURE_WIDTH,
                            },
                            topic: 'canvas',
                        },
                    ],
                    type: 'subscribe',
                }),
            )
            timeout = setTimeout(() => finish('error', new Error('canvas subscription acknowledgment timed out')), 5000)
        })

        ws.addEventListener('message', (event) => {
            if (typeof event.data === 'string') {
                const message = JSON.parse(event.data) as { message?: string; type?: string }
                if (message.type === 'error') {
                    finish('error', new Error(message.message ?? 'canvas subscription rejected'))
                } else if (message.type === 'subscribed' && captureInterval === null) {
                    if (timeout) clearTimeout(timeout)
                    startedAt = Date.now()
                    const interval = Math.max(1, Math.floor(captureMs / frameCount))
                    captureInterval = setInterval(takeSample, interval)
                    timeout = setTimeout(() => finish('timeout'), captureMs + 2000)
                }
                return
            }
            if (!(event.data instanceof ArrayBuffer) || captureInterval === null) return
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

async function writeDraftFrame(slug: string, variantKey: string, rank: number, frame: CapturedFrame): Promise<string> {
    const dir = resolve(DRAFTS_ROOT, slug, variantKey)
    await mkdir(dir, { recursive: true })
    const filePath = resolve(dir, `rank-${rank}.png`)
    await sharp(frame.rgba, { raw: { channels: 4, height: frame.height, width: frame.width } })
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
                await sharp(bytes).webp({ effort: 4, quality: 92 }).toFile(outPath)
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
        const presets = detail.presets ?? []
        const presetCount = Math.min(presets.length, MAX_PRESETS_PER_EFFECT)
        for (let index = 0; index < presetCount; index += 1) {
            const preset = presets[index]
            if (!preset) continue
            const key = slugify(preset.name)
            if (!key) continue
            variants.push({ key, label: preset.name, presetName: preset.name })
        }
    }
    return variants
}

async function captureVariant(opts: CliOptions, effect: EffectDetail, variant: Variant): Promise<void> {
    const slug = slugify(effect.name)
    const label = `${effect.name} · ${variant.label}`
    process.stdout.write(`\n▸ ${label}\n`)

    const presetControls =
        variant.presetName === null
            ? null
            : ((effect.presets ?? []).find((p) => p.name === variant.presetName)?.controls ?? null)

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
        process.stderr.write('  ✗ no frames captured\n')
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
                process.stderr.write(`✗ ${effect.name} · ${variant.label}: ${String(err)}\n`)
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
    process.stdout.write('run again with --promote to copy rank-1 frames into curated/\n')
}

main().catch((err) => {
    process.stderr.write(`${err instanceof Error ? (err.stack ?? err.message) : String(err)}\n`)
    process.exit(1)
})
