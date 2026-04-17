import { beforeAll, describe, expect, test } from 'bun:test'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { scaffoldWorkspace } from '../../create-effect/src/scaffold'
import { startDevServer } from '../src/dev'

const SDK_ROOT = resolve(import.meta.dirname, '../../..')
const CORE_PACKAGE_DIR = resolve(SDK_ROOT, 'packages/core')
const SDK_PACKAGE_SPEC = `file:${CORE_PACKAGE_DIR}`

async function runCommand(cmd: string[], cwd: string): Promise<void> {
    const proc = Bun.spawn({
        cmd,
        cwd,
        stderr: 'inherit',
        stdin: 'ignore',
        stdout: 'inherit',
    })
    expect(await proc.exited).toBe(0)
}

async function waitFor<T>(factory: () => T | undefined, timeoutMs = 10_000): Promise<T> {
    const started = Date.now()

    while (Date.now() - started < timeoutMs) {
        const value = factory()
        if (value !== undefined) return value
        await Bun.sleep(50)
    }

    throw new Error(`Timed out after ${timeoutMs}ms`)
}

beforeAll(async () => {
    await runCommand(['bun', 'run', '--filter', '@hypercolor/sdk', 'build'], SDK_ROOT)
})

describe('sdk dev server', () => {
    test('serves shell + preview and broadcasts reload state changes', async () => {
        const tempRoot = mkdtempSync(join(tmpdir(), 'hypercolor-dev-server-'))
        const workspaceDir = join(tempRoot, 'dev-workspace')
        const entryFile = join(workspaceDir, 'effects', 'aurora', 'main.ts')
        const messages: Array<{ state: { entries: Array<{ id: string; revision: number }> } }> = []

        try {
            await scaffoldWorkspace({
                audio: false,
                firstEffectId: 'aurora',
                git: false,
                install: false,
                sdkPackageSpec: SDK_PACKAGE_SPEC,
                targetDir: workspaceDir,
                template: 'canvas',
                workspaceName: 'dev-workspace',
            })
            await runCommand(['bun', 'install'], workspaceDir)

            const port = 4300 + Math.floor(Math.random() * 500)
            const handle = await startDevServer({
                cwd: workspaceDir,
                entryRoots: ['effects'],
                port,
                workspaceRoot: workspaceDir,
            })

            try {
                const shellHtml = await fetch(handle.url).then((response) => response.text())
                expect(shellHtml).toContain('Hypercolor Effect Studio')
                expect(shellHtml).toContain('effect-select')
                expect(shellHtml).toContain('Audio Simulation')
                expect(shellHtml).toContain('LED Preview')
                expect(shellHtml).toContain('Trigger Beat')
                expect(shellHtml).toContain('Daemon 640 x 480')

                const state = await fetch(`${handle.url}/api/state`).then((response) => response.json())
                expect(state.entries).toHaveLength(1)
                expect(state.entries[0]?.id).toBe('aurora')
                expect(state.entries[0]?.metadata?.controls?.length).toBeGreaterThan(0)

                const previewHtml = await fetch(`${handle.url}/preview/aurora?width=512&height=288`).then((response) =>
                    response.text(),
                )
                expect(previewHtml).toContain('window.engine')
                expect(previewHtml).toContain('width: 512')
                expect(previewHtml).toContain('<title>Aurora</title>')

                const socket = new WebSocket(`${handle.url.replace('http', 'ws')}/ws`)
                socket.addEventListener('message', (event) => {
                    messages.push(JSON.parse(String(event.data)))
                })

                const initialMessage = await waitFor(() => messages[0])
                const initialRevision = initialMessage.state.entries[0]?.revision ?? 0

                const source = readFileSync(entryFile, 'utf8')
                await Bun.write(entryFile, source.replace("num('Speed', [1, 10], 5", "num('Speed', [1, 10], 6"))

                const updatedMessage = await waitFor(() => {
                    const latest = messages.at(-1)
                    if (!latest) return undefined
                    return latest.state.entries[0]?.revision > initialRevision ? latest : undefined
                })

                expect(updatedMessage.state.entries[0]?.revision).toBeGreaterThan(initialRevision)
                socket.close()
            } finally {
                await handle.close()
            }
        } finally {
            rmSync(tempRoot, { force: true, recursive: true })
        }
    }, 120_000)
})
