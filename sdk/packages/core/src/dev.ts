import { existsSync, watch } from 'node:fs'
import { dirname, resolve } from 'node:path'

import { renderDevShell } from './dev-shell'
import { buildArtifactDocument, discoverWorkspaceEntries } from './tooling'
import { artifactIdFromEntry, extractArtifactMetadata } from './tooling'
import type { ArtifactKind, ExtractedArtifactMetadata } from './tooling/types'

interface DevEntryState {
    dirty: boolean
    entryPath: string
    error?: string
    html?: string
    id: string
    kind: ArtifactKind
    metadata?: ExtractedArtifactMetadata
    name: string
    revision: number
}

export interface DevServerOptions {
    cwd: string
    entryPath?: string
    entryRoots: string[]
    open?: boolean
    port: number
    sdkAliasPath?: string
    stdout?: Pick<Console, 'error' | 'log'>
    workspaceRoot: string
}

export interface DevServerHandle {
    close(): Promise<void>
    port: number
    url: string
}

function output(stdout: DevServerOptions['stdout']) {
    return stdout ?? console
}

function parseDimension(value: string | null, fallback: number): number {
    const parsed = Number(value)
    if (!Number.isFinite(parsed)) return fallback
    return Math.max(100, Math.round(parsed))
}

function injectDevPrelude(html: string, args: { height: number; width: number }): string {
    const prelude = `<script>
(() => {
  const audio = {
    bass: 0,
    bassEnv: 0,
    beat: 0,
    beatConfidence: 0,
    beatPhase: 0,
    beatPulse: 0,
    brightness: 0.5,
    chordMood: 0,
    chromagram: new Float32Array(12),
    density: 0,
    dominantPitch: 0,
    dominantPitchConfidence: 0,
    frequency: new Float32Array(200),
    frequencyRaw: new Int8Array(200),
    frequencyWeighted: new Float32Array(200),
    harmonicHue: 0,
    level: 0,
    levelLong: 0,
    levelRaw: -100,
    levelShort: 0,
    melBands: new Float32Array(24),
    melBandsNormalized: new Float32Array(24),
    mid: 0,
    midEnv: 0,
    momentum: 0,
    onset: 0,
    onsetPulse: 0,
    rolloff: 0.5,
    roughness: 0.2,
    spectralFlux: 0,
    spectralFluxBands: new Float32Array(3),
    spread: 0.3,
    swell: 0,
    tempo: 120,
    treble: 0,
    trebleEnv: 0,
    width: 0.5,
  }

  const sensors = {}

  window.engine = {
    audio,
    getControlValue(id) {
      return window[id]
    },
    getSensorValue(name) {
      return sensors[name] ?? null
    },
    height: ${args.height},
    sensorList: [],
    sensors,
    setSensorValue(name, value, min, max, unit) {
      sensors[name] = { max, min, unit, value }
    },
    width: ${args.width},
    zone: {
      height: 20,
      hue: new Float32Array(560),
      lightness: new Float32Array(560),
      saturation: new Float32Array(560),
      width: 28,
    },
  }
})()
</script>`

    return html.replace('</head>', `${prelude}\n  </head>`)
}

async function tryOpenBrowser(url: string): Promise<void> {
    const cmd =
        process.platform === 'darwin'
            ? ['open', url]
            : process.platform === 'linux'
              ? ['xdg-open', url]
              : undefined

    if (!cmd) return
    Bun.spawn({ cmd, stderr: 'ignore', stdin: 'ignore', stdout: 'ignore' })
}

export async function startDevServer(options: DevServerOptions): Promise<DevServerHandle> {
    const log = output(options.stdout)
    const entries = new Map<string, DevEntryState>()
    const sockets = new Set<Bun.ServerWebSocket<unknown>>()
    let selectedId: string | undefined
    let refreshTimer: ReturnType<typeof setTimeout> | undefined

    const toSnapshot = () => ({
        entries: Array.from(entries.values()).map((entry) => ({
            error: entry.error,
            id: entry.id,
            kind: entry.kind,
            metadata: entry.metadata,
            name: entry.name,
            revision: entry.revision,
        })),
        initialSelectedId: selectedId ?? Array.from(entries.keys())[0] ?? null,
    })

    const server = Bun.serve({
        async fetch(request, serverRef) {
            const url = new URL(request.url)

            if (url.pathname === '/ws') {
                if (serverRef.upgrade(request)) return
                return new Response('WebSocket upgrade failed', { status: 400 })
            }

            if (url.pathname === '/') {
                return new Response(renderDevShell(), {
                    headers: { 'content-type': 'text/html; charset=utf-8' },
                })
            }

            if (url.pathname === '/api/state') {
                return Response.json(toSnapshot())
            }

            if (url.pathname.startsWith('/preview/')) {
                const entryId = decodeURIComponent(url.pathname.slice('/preview/'.length))
                const entry = entries.get(entryId)
                if (!entry?.metadata) {
                    return new Response(entry?.error ?? `Unknown entry "${entryId}"`, { status: 404 })
                }

                if (entry.dirty || !entry.html) {
                    try {
                        const artifact = await buildArtifactDocument({
                            entryPath: entry.entryPath,
                            outDir: resolve(options.cwd, '.hypercolor-dev'),
                            sdkAliasPath: options.sdkAliasPath,
                        })
                        entry.html = artifact.html
                        entry.kind = artifact.kind
                        entry.metadata = artifact.metadata
                        entry.name = artifact.metadata.name
                        entry.error = undefined
                        entry.dirty = false
                    } catch (error) {
                        const message = error instanceof Error ? error.message : String(error)
                        entry.error = message
                        return new Response(message, { status: 500 })
                    }
                }

                const width = parseDimension(url.searchParams.get('width'), 640)
                const height = parseDimension(url.searchParams.get('height'), 480)
                return new Response(injectDevPrelude(entry.html, { height, width }), {
                    headers: { 'content-type': 'text/html; charset=utf-8' },
                })
            }

            return new Response('Not found', { status: 404 })
        },
        port: options.port,
        websocket: {
            message() {},
            open(ws) {
                sockets.add(ws)
                ws.send(JSON.stringify({ state: toSnapshot(), type: 'state' }))
            },
            close(ws) {
                sockets.delete(ws)
            },
        },
    })

    async function refreshEntries(): Promise<void> {
        const discoveredPaths = options.entryPath ? [resolve(options.cwd, options.entryPath)] : discoverWorkspaceEntries(options.workspaceRoot, options.entryRoots)
        const nextEntries = new Map<string, DevEntryState>()

        for (const entryPath of discoveredPaths) {
            const id = artifactIdFromEntry(entryPath)
            const previous = entries.get(id)

            try {
                const metadata = await extractArtifactMetadata(entryPath)
                nextEntries.set(id, {
                    dirty: true,
                    entryPath,
                    error: undefined,
                    html: previous?.entryPath === entryPath ? previous.html : undefined,
                    id,
                    kind: metadata.kind,
                    metadata,
                    name: metadata.name,
                    revision: (previous?.revision ?? 0) + 1,
                })
            } catch (error) {
                nextEntries.set(id, {
                    dirty: true,
                    entryPath,
                    error: error instanceof Error ? error.message : String(error),
                    html: undefined,
                    id,
                    kind: previous?.kind ?? 'effect',
                    metadata: undefined,
                    name: previous?.name ?? id,
                    revision: (previous?.revision ?? 0) + 1,
                })
            }
        }

        entries.clear()
        for (const [id, entry] of nextEntries) {
            entries.set(id, entry)
        }

        if (!selectedId || !entries.has(selectedId)) {
            selectedId = Array.from(entries.keys())[0]
        }

        const payload = JSON.stringify({ state: toSnapshot(), type: 'state' })
        for (const socket of sockets) {
            socket.send(payload)
        }
    }

    await refreshEntries()

    const watchers = options.entryPath
        ? [dirname(resolve(options.cwd, options.entryPath))]
        : options.entryRoots.map((root) => resolve(options.workspaceRoot, root)).filter(existsSync)

    const fsWatchers = watchers.map((root) =>
        watch(root, { recursive: true }, (_eventType, filename) => {
            const changed = String(filename ?? '')
            if (!changed.endsWith('.ts') && !changed.endsWith('.glsl')) return
            clearTimeout(refreshTimer)
            refreshTimer = setTimeout(() => {
                void refreshEntries()
            }, 150)
        }),
    )

    const handle: DevServerHandle = {
        async close() {
            clearTimeout(refreshTimer)
            for (const watcher of fsWatchers) watcher.close()
            server.stop(true)
        },
        port: server.port ?? options.port,
        url: `http://localhost:${server.port ?? options.port}`,
    }

    log.log(`Hypercolor Effect Studio → ${handle.url}`)
    if (options.open) await tryOpenBrowser(handle.url)

    return handle
}

export async function runDevServer(options: DevServerOptions): Promise<never> {
    await startDevServer(options)
    await new Promise(() => {})
    throw new Error('unreachable')
}
