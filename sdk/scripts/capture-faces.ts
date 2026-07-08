#!/usr/bin/env bun
/**
 * Display face screenshot capture tool.
 *
 * Assigns each display-category effect (face) to the Face Dev simulator
 * displays, subscribes the `display_preview` WebSocket channel per device so
 * the display worker encodes simulator frames, then snapshots
 * `/displays/{id}/preview.jpg` into the shared screenshot drafts tree:
 *
 *   effects/screenshots/drafts/<slug>/default/rank-1.png   (round 480x480)
 *   effects/screenshots/drafts/<slug>/strip/rank-1.png     (strip 960x160)
 *
 * Promote with `just promote-screenshots` — the same pipeline as effect
 * captures — to produce curated/<slug>/{default,strip}.webp.
 *
 * Prior face assignments on both simulators are restored on exit.
 */

import { mkdir } from 'node:fs/promises'
import { resolve } from 'node:path'

import sharp from 'sharp'

const SDK_ROOT = resolve(import.meta.dirname, '..')
const DRAFTS_ROOT = resolve(SDK_ROOT, '..', 'effects', 'screenshots', 'drafts')

const DEFAULT_DAEMON = 'http://127.0.0.1:9420'
/** Servo session boot + face entrance animation settle time. */
const DEFAULT_WARMUP_MS = 6500
const SNAP_ATTEMPTS = 6
const SNAP_RETRY_MS = 1200

const ROUND_SIMULATOR = 'Face Dev Round'
const STRIP_SIMULATOR = 'Face Dev Strip'

interface CliOptions {
    daemon: string
    faceFilter: string | null
    warmupMs: number
}

interface Envelope<T> {
    data: T
}

interface EffectSummary {
    id: string
    name: string
    category: string
    source: string | null
    runnable: boolean | null
}

interface DisplaySummary {
    id: string
    name: string
}

interface FaceLayer {
    source: { effect_id?: string; controls?: Record<string, unknown> }
    blend: string
    opacity: number
}

interface FaceState {
    effect: { id: string }
    group: { layers: FaceLayer[] }
}

/** Prior assignment snapshot used to restore a simulator after capture. */
interface PriorFace {
    effectId: string
    controls: Record<string, unknown>
    blendMode: string
    opacity: number
}

function parseArgs(argv: readonly string[]): CliOptions {
    const opts: CliOptions = {
        daemon: DEFAULT_DAEMON,
        faceFilter: null,
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
            case '--face':
                if (!next) throw new Error('--face requires a slug or name')
                opts.faceFilter = next
                index += 1
                break
            case '--warmup':
                if (!next) throw new Error('--warmup requires milliseconds')
                opts.warmupMs = Number.parseInt(next, 10)
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
    return opts
}

function printHelp(): void {
    process.stdout.write(`capture-faces — snapshot every display face on the Face Dev simulators

usage:
  bun sdk/scripts/capture-faces.ts [flags]

flags:
  --daemon <url>       daemon base URL (default ${DEFAULT_DAEMON})
  --face <slug|name>   capture a single face
  --warmup <ms>        Servo boot + entrance settle time (default ${DEFAULT_WARMUP_MS})

Requires a running daemon with the Face Dev simulator displays registered.
Drafts land next to effect captures; run \`just promote-screenshots\` after.
`)
}

export function slugify(value: string): string {
    return value
        .normalize('NFKD')
        .replace(/[\u0300-\u036f]/g, '')
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-+|-+$/g, '')
}

async function api<T>(daemon: string, path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${daemon}/api/v1${path}`, {
        headers: { 'content-type': 'application/json' },
        ...init,
    })
    if (!response.ok) {
        throw new Error(`${init?.method ?? 'GET'} ${path}: ${response.status} ${response.statusText}`)
    }
    return ((await response.json()) as Envelope<T>).data
}

async function getPriorFace(daemon: string, deviceId: string): Promise<PriorFace | null> {
    let state: FaceState
    try {
        state = await api<FaceState>(daemon, `/displays/${deviceId}/face`)
    } catch {
        return null
    }
    const layer = state.group.layers[0]
    return {
        blendMode: layer?.blend ?? 'replace',
        controls: layer?.source.controls ?? {},
        effectId: state.effect.id,
        opacity: layer?.opacity ?? 1.0,
    }
}

async function assignFace(
    daemon: string,
    deviceId: string,
    effectId: string,
    controls: Record<string, unknown>,
    blendMode: string,
    opacity: number,
): Promise<void> {
    await api(daemon, `/displays/${deviceId}/face`, {
        body: JSON.stringify({
            blend_mode: blendMode,
            controls,
            effect_id: effectId,
            opacity,
            scope: 'default',
        }),
        method: 'PUT',
    })
}

async function restoreFace(daemon: string, deviceId: string, prior: PriorFace | null): Promise<void> {
    if (prior) {
        await assignFace(daemon, deviceId, prior.effectId, prior.controls, prior.blendMode, prior.opacity)
    } else {
        await api(daemon, `/displays/${deviceId}/face?scope=default`, { method: 'DELETE' })
    }
}

async function snapPreview(daemon: string, deviceId: string): Promise<Uint8Array | null> {
    for (let attempt = 0; attempt < SNAP_ATTEMPTS; attempt += 1) {
        const response = await fetch(`${daemon}/api/v1/displays/${deviceId}/preview.jpg`)
        if (response.ok) return new Uint8Array(await response.arrayBuffer())
        await sleep(SNAP_RETRY_MS)
    }
    return null
}

async function writeDraft(slug: string, variantKey: string, jpeg: Uint8Array): Promise<string> {
    const dir = resolve(DRAFTS_ROOT, slug, variantKey)
    await mkdir(dir, { recursive: true })
    const filePath = resolve(dir, 'rank-1.png')
    await sharp(jpeg).png({ compressionLevel: 6 }).toFile(filePath)
    return filePath
}

function sleep(ms: number): Promise<void> {
    return new Promise((res) => setTimeout(res, ms))
}

async function main(): Promise<void> {
    const opts = parseArgs(process.argv.slice(2))

    const displays = await api<DisplaySummary[]>(opts.daemon, '/displays')
    const simulators: Array<{ deviceId: string; variantKey: string; label: string }> = []
    for (const [name, variantKey] of [
        [ROUND_SIMULATOR, 'default'],
        [STRIP_SIMULATOR, 'strip'],
    ] as const) {
        const display = displays.find((d) => d.name === name)
        if (!display)
            throw new Error(`simulator display "${name}" not found — is the daemon running with Face Dev simulators?`)
        simulators.push({ deviceId: display.id, label: name, variantKey })
    }

    const effects = await api<{ items: EffectSummary[] }>(opts.daemon, '/effects?limit=500')
    const facesByName = new Map<string, EffectSummary>()
    for (const effect of effects.items) {
        if (effect.category !== 'display' || effect.runnable === false) continue
        // Prefer bundled entries over user-dir duplicates.
        const existing = facesByName.get(effect.name)
        if (!existing || effect.source !== 'user') facesByName.set(effect.name, effect)
    }
    let faces = [...facesByName.values()].sort((a, b) => a.name.localeCompare(b.name))
    if (opts.faceFilter) {
        faces = faces.filter((f) => slugify(f.name) === slugify(opts.faceFilter ?? ''))
        if (faces.length === 0) throw new Error(`no display face matched ${opts.faceFilter}`)
    }

    process.stdout.write(`daemon: ${opts.daemon}\n`)
    process.stdout.write(`queued ${faces.length} face(s) × ${simulators.length} simulator(s)\n`)
    await mkdir(DRAFTS_ROOT, { recursive: true })

    const priors = new Map<string, PriorFace | null>()
    for (const sim of simulators) {
        priors.set(sim.deviceId, await getPriorFace(opts.daemon, sim.deviceId))
    }

    const ws = new WebSocket(`${opts.daemon.replace(/^http/, 'ws')}/api/v1/ws`)
    await new Promise<void>((res, reject) => {
        ws.onopen = () => res()
        ws.onerror = () => reject(new Error('websocket connection failed'))
    })
    const follow = (deviceId: string) =>
        ws.send(
            JSON.stringify({
                channels: ['display_preview'],
                config: { display_preview: { device_id: deviceId, fps: 15 } },
                type: 'subscribe',
            }),
        )

    let failures = 0
    const startedAt = Date.now()
    try {
        for (const face of faces) {
            const slug = slugify(face.name)
            process.stdout.write(`\n▸ ${face.name}\n`)
            for (const sim of simulators) {
                try {
                    await assignFace(opts.daemon, sim.deviceId, face.id, {}, 'replace', 1.0)
                    follow(sim.deviceId)
                    await sleep(opts.warmupMs)
                    const jpeg = await snapPreview(opts.daemon, sim.deviceId)
                    if (!jpeg) {
                        process.stderr.write(`  ✗ ${sim.variantKey}: preview never became available\n`)
                        failures += 1
                        continue
                    }
                    const path = await writeDraft(slug, sim.variantKey, jpeg)
                    process.stdout.write(`  ✓ ${sim.variantKey}: ${path.replace(SDK_ROOT, 'sdk')}\n`)
                } catch (err) {
                    process.stderr.write(`  ✗ ${sim.variantKey}: ${String(err)}\n`)
                    failures += 1
                }
            }
        }
    } finally {
        ws.close()
        for (const sim of simulators) {
            try {
                await restoreFace(opts.daemon, sim.deviceId, priors.get(sim.deviceId) ?? null)
                process.stdout.write(`restored ${sim.label}\n`)
            } catch (err) {
                process.stderr.write(`✗ failed to restore ${sim.label}: ${String(err)}\n`)
                failures += 1
            }
        }
    }

    const elapsedSec = Math.round((Date.now() - startedAt) / 1000)
    process.stdout.write(`\nfinished in ${elapsedSec}s — drafts at ${DRAFTS_ROOT}\n`)
    process.stdout.write('run `just promote-screenshots` to encode curated WebP variants\n')
    if (failures > 0) {
        process.stderr.write(`${failures} failure(s)\n`)
        process.exit(1)
    }
}

main().catch((err) => {
    process.stderr.write(`${err instanceof Error ? (err.stack ?? err.message) : String(err)}\n`)
    process.exit(1)
})
