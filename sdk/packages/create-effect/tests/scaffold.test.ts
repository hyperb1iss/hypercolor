import { beforeAll, describe, expect, test } from 'bun:test'
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { main as scaffoldCli } from '../src/cli'

const SDK_ROOT = resolve(import.meta.dirname, '../../..')
const CORE_PACKAGE_DIR = resolve(SDK_ROOT, 'packages/core')
const SDK_PACKAGE_SPEC = `file:${CORE_PACKAGE_DIR}`

async function runCommand(cmd: string[], cwd: string, env: Record<string, string> = {}): Promise<void> {
    const proc = Bun.spawn({
        cmd,
        cwd,
        env: { ...process.env, ...env },
        stderr: 'inherit',
        stdin: 'ignore',
        stdout: 'inherit',
    })

    const exitCode = await proc.exited
    expect(exitCode).toBe(0)
}

beforeAll(async () => {
    await runCommand(['bun', 'run', '--filter', '@hypercolor/sdk', 'build'], SDK_ROOT)
})

describe('@hypercolor/create-effect', () => {
    test('scaffolds a TypeScript workspace and dogfoods add/build/validate/install', async () => {
        const tempRoot = mkdtempSync(join(tmpdir(), 'hypercolor-create-effect-'))
        const workspaceDir = join(tempRoot, 'test-effects')
        const xdgDataHome = join(tempRoot, 'xdg')

        try {
            const exitCode = await scaffoldCli(
                [
                    'test-effects',
                    '--template',
                    'canvas',
                    '--first',
                    'aurora',
                    '--audio',
                    '--no-git',
                    '--no-install',
                    '--sdk-spec',
                    SDK_PACKAGE_SPEC,
                ],
                { cwd: tempRoot, stdout: console },
            )

            expect(exitCode).toBe(0)
            expect(existsSync(join(workspaceDir, 'effects', 'aurora', 'main.ts'))).toBeTrue()

            await runCommand(['bun', 'install'], workspaceDir)
            await runCommand(['bun', 'run', 'add', 'ember', '--template', 'shader'], workspaceDir, { EDITOR: '', VISUAL: '' })
            await runCommand(['bun', 'run', 'add', 'hud', '--template', 'face', '--audio'], workspaceDir, { EDITOR: '', VISUAL: '' })
            await runCommand(['bun', 'run', 'add', 'raw-html', '--template', 'html'], workspaceDir, { EDITOR: '', VISUAL: '' })

            await runCommand(['bun', 'run', 'build'], workspaceDir)
            await runCommand(['bun', 'run', 'validate'], workspaceDir)

            expect(existsSync(join(workspaceDir, 'dist', 'aurora.html'))).toBeTrue()
            expect(existsSync(join(workspaceDir, 'dist', 'ember.html'))).toBeTrue()
            expect(existsSync(join(workspaceDir, 'dist', 'hud.html'))).toBeTrue()
            expect(existsSync(join(workspaceDir, 'effects', 'raw-html.html'))).toBeTrue()

            await runCommand(['./node_modules/.bin/hypercolor', 'validate', 'effects/raw-html.html'], workspaceDir)
            await runCommand(['bun', 'run', 'ship'], workspaceDir, { XDG_DATA_HOME: xdgDataHome })
            await runCommand(
                ['./node_modules/.bin/hypercolor', 'install', 'effects/raw-html.html'],
                workspaceDir,
                { XDG_DATA_HOME: xdgDataHome },
            )

            expect(existsSync(join(xdgDataHome, 'hypercolor', 'effects', 'user', 'aurora.html'))).toBeTrue()
            expect(existsSync(join(xdgDataHome, 'hypercolor', 'effects', 'user', 'ember.html'))).toBeTrue()
            expect(existsSync(join(xdgDataHome, 'hypercolor', 'effects', 'user', 'hud.html'))).toBeTrue()
            expect(existsSync(join(xdgDataHome, 'hypercolor', 'effects', 'user', 'raw-html.html'))).toBeTrue()
        } finally {
            rmSync(tempRoot, { force: true, recursive: true })
        }
    }, 120_000)

    test('scaffolds a plain HTML workspace and validates the starter artifact', async () => {
        const tempRoot = mkdtempSync(join(tmpdir(), 'hypercolor-create-html-'))
        const workspaceDir = join(tempRoot, 'html-effects')
        const xdgDataHome = join(tempRoot, 'xdg')

        try {
            const exitCode = await scaffoldCli(
                [
                    'html-effects',
                    '--template',
                    'html',
                    '--first',
                    'glow-card',
                    '--no-git',
                    '--no-install',
                    '--sdk-spec',
                    SDK_PACKAGE_SPEC,
                ],
                { cwd: tempRoot, stdout: console },
            )

            expect(exitCode).toBe(0)
            expect(readFileSync(join(workspaceDir, 'effects', 'glow-card.html'), 'utf8')).toContain('<title>Glow Card</title>')

            await runCommand(['bun', 'install'], workspaceDir)
            await runCommand(['bun', 'run', 'validate'], workspaceDir)
            await runCommand(['bun', 'run', 'ship'], workspaceDir, { XDG_DATA_HOME: xdgDataHome })

            expect(existsSync(join(xdgDataHome, 'hypercolor', 'effects', 'user', 'glow-card.html'))).toBeTrue()
        } finally {
            rmSync(tempRoot, { force: true, recursive: true })
        }
    }, 120_000)
})
