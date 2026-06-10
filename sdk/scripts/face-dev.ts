/**
 * face-dev — hardware-free authoring loop for display faces.
 *
 * `just face-dev NAME` builds the face, installs it into the running
 * daemon, ensures the two canonical simulator displays exist (480x480
 * round + 960x160 strip), assigns the face to both, opens the Displays
 * page, then rebuilds and reinstalls on every save. Target: save to
 * preview refresh in under five seconds.
 */

import { watch } from 'node:fs'
import { resolve } from 'node:path'

import { installArtifactsViaDaemon } from '../packages/core/src/tooling'

const SDK_ROOT = resolve(import.meta.dir, '..')
const DAEMON_URL = process.env.HYPERCOLOR_URL ?? 'http://127.0.0.1:9420'
const SIMULATORS = [
    { circular: true, height: 480, name: 'Face Dev Round', width: 480 },
    { circular: false, height: 160, name: 'Face Dev Strip', width: 960 },
]
const REBUILD_DEBOUNCE_MS = 150

const faceName = process.argv[2]
if (!faceName) {
    console.error('usage: bun scripts/face-dev.ts <face-name>')
    process.exit(1)
}
const entryPath = `src/faces/${faceName}/main.ts`
if (!(await Bun.file(resolve(SDK_ROOT, entryPath)).exists())) {
    console.error(`no face entry at sdk/${entryPath}`)
    process.exit(1)
}

interface Envelope<T> {
    data: T
}

async function api<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${DAEMON_URL}/api/v1${path}`, {
        headers: { 'content-type': 'application/json' },
        ...init,
    })
    if (!response.ok) {
        throw new Error(`${init?.method ?? 'GET'} ${path} failed: ${response.status} ${await response.text()}`)
    }
    return ((await response.json()) as Envelope<T>).data
}

async function ensureDaemon(): Promise<void> {
    try {
        await api('/effects')
    } catch {
        console.error(`no daemon reachable at ${DAEMON_URL} — start one with: just daemon`)
        process.exit(1)
    }
}

interface SimulatorConfig {
    id: string
    name: string
    width: number
    height: number
    circular: boolean
}

async function ensureSimulators(): Promise<SimulatorConfig[]> {
    const existing = await api<SimulatorConfig[]>('/simulators/displays')
    const ready: SimulatorConfig[] = []
    for (const wanted of SIMULATORS) {
        const found = existing.find(
            (simulator) =>
                simulator.name === wanted.name ||
                (simulator.width === wanted.width &&
                    simulator.height === wanted.height &&
                    simulator.circular === wanted.circular),
        )
        if (found) {
            ready.push(found)
            continue
        }
        const created = await api<SimulatorConfig>('/simulators/displays', {
            body: JSON.stringify({ ...wanted, enabled: true }),
            method: 'POST',
        })
        console.log(`created simulator ${created.name} (${created.width}x${created.height})`)
        ready.push(created)
    }
    return ready
}

async function runCli(args: string[]): Promise<boolean> {
    const proc = Bun.spawn(['bun', 'packages/core/src/cli.ts', ...args], {
        cwd: SDK_ROOT,
        stderr: 'inherit',
        stdout: 'inherit',
    })
    return (await proc.exited) === 0
}

/** Build the face and install it into the daemon; returns the
 *  registered effect name on success. */
async function buildAndInstall(): Promise<string | null> {
    const built = await runCli([
        'build',
        entryPath,
        '--out',
        '../effects/hypercolor',
        '--sdk-alias-path',
        'packages/core/src/index.ts',
    ])
    if (!built) return null

    const result = await installArtifactsViaDaemon({
        cwd: SDK_ROOT,
        daemonUrl: DAEMON_URL,
        filePatterns: [`../effects/hypercolor/${faceName}.html`],
    })
    for (const failure of result.failures) {
        console.error(`install failed: ${failure.errors.join('; ')}`)
    }
    return result.successes[0]?.installedName ?? null
}

interface EffectSummary {
    id: string
    name: string
}

async function resolveEffectId(installedName: string): Promise<string | undefined> {
    const page = await api<{ items: EffectSummary[] }>('/effects?limit=500')
    return page.items.find((effect) => effect.name === installedName)?.id
}

async function assignFace(simulators: SimulatorConfig[], effectId: string): Promise<void> {
    for (const simulator of simulators) {
        await api(`/displays/${simulator.id}/face`, {
            body: JSON.stringify({ effect_id: effectId }),
            method: 'PUT',
        })
        console.log(`assigned to ${simulator.name}`)
    }
}

function openDisplaysPage(): void {
    const url = process.env.HYPERCOLOR_UI_URL ?? `${DAEMON_URL}/displays`
    Bun.spawn(['xdg-open', url], { stderr: 'ignore', stdout: 'ignore' }).exited.catch(() => {})
    console.log(`displays page: ${url}`)
}

async function cycle(simulators: SimulatorConfig[], reason: string): Promise<void> {
    const started = performance.now()
    console.log(`\n— ${reason}`)
    const installedName = await buildAndInstall()
    if (!installedName) {
        console.error('build/install failed; waiting for the next save')
        return
    }
    const effectId = await resolveEffectId(installedName)
    if (!effectId) {
        console.error(`installed, but no effect named '${installedName}' is registered`)
        return
    }
    await assignFace(simulators, effectId)
    console.log(`ready in ${((performance.now() - started) / 1000).toFixed(1)}s`)
}

await ensureDaemon()
const simulators = await ensureSimulators()
await cycle(simulators, `initial build of ${faceName}`)
openDisplaysPage()

const watchRoots = [
    resolve(SDK_ROOT, `src/faces/${faceName}`),
    resolve(SDK_ROOT, 'src/faces/shared'),
    resolve(SDK_ROOT, 'packages/core/src'),
]
let pending: ReturnType<typeof setTimeout> | null = null
let building = false
for (const root of watchRoots) {
    watch(root, { recursive: true }, (_event, filename) => {
        if (pending) clearTimeout(pending)
        pending = setTimeout(() => {
            pending = null
            if (building) return
            building = true
            cycle(simulators, `change in ${filename ?? 'sources'}`).finally(() => {
                building = false
            })
        }, REBUILD_DEBOUNCE_MS)
    })
}
console.log(`\nwatching ${faceName} + shared + sdk core — save to rebuild, ctrl-c to stop`)
