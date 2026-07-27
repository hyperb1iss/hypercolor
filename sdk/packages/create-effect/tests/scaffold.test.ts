import { beforeAll, describe, expect, test } from 'bun:test'
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { main as scaffoldCli } from '../src/cli'
import { defaultSdkPackageSpec, workspaceNameFromTarget } from '../src/scaffold'
import { createWorkspaceFiles, renderTemplate } from '../src/templates'

const SDK_ROOT = resolve(import.meta.dirname, '../../..')
const CORE_PACKAGE_DIR = resolve(SDK_ROOT, 'packages/core')
const SDK_PACKAGE_SPEC = `file:${CORE_PACKAGE_DIR}`
const WINDOWS_SDK_PACKAGE_SPEC = String.raw`file:C:\Users\Bliss\dev\hypercolor\sdk\packages\core`
const WINDOWS_WORKSPACE_NAME = String.raw`.\my-effects`
const REPLACEMENT_TOKEN_SDK_PACKAGE_SPEC = String.raw`file:C:\build$&x\__WORKSPACE_NAME__\core`

function expectRenderedWorkspaceManifest(
    template: 'canvas' | 'html',
    sdkPackageSpec: string,
    workspaceName = 'manifest-probe',
): void {
    const tempRoot = mkdtempSync(join(tmpdir(), 'hypercolor-create-manifest-'))

    try {
        createWorkspaceFiles({
            sdkPackageSpec,
            template,
            workspaceDir: tempRoot,
            workspaceName,
        })
        const manifest = JSON.parse(readFileSync(join(tempRoot, 'package.json'), 'utf8')) as {
            name: string
            devDependencies: { hypercolor: string }
        }
        expect(manifest.name).toBe(workspaceName)
        expect(manifest.devDependencies.hypercolor).toBe(sdkPackageSpec)
        expect(readFileSync(join(tempRoot, 'README.md'), 'utf8')).toContain(`# ${workspaceName}`)
    } finally {
        rmSync(tempRoot, { force: true, recursive: true })
    }
}

function expectWindowsTargetWorkspaceManifest(
    template: 'canvas' | 'html',
    target: string,
    expectedWorkspaceName: string,
): void {
    const workspaceName = workspaceNameFromTarget(target)
    expect(workspaceName).toBe(expectedWorkspaceName)
    expectRenderedWorkspaceManifest(template, '^0.3.0', workspaceName)
}

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
    await runCommand(['bun', 'run', '--filter', 'hypercolor', 'build'], SDK_ROOT)
})

describe('create-hypercolor', () => {
    test('renders adjacent and mixed-case template placeholders independently', () => {
        expect(
            renderTemplate('__WORKSPACE_NAME____sdk_package_spec__', {
                sdk_package_spec: '^0.3.0',
                WORKSPACE_NAME: 'my-effects',
            }),
        ).toBe('my-effects^0.3.0')
    })

    test('defaults the SDK spec to the published caret range', () => {
        const previousSpec = process.env.HYPERCOLOR_SDK_PACKAGE_SPEC
        delete process.env.HYPERCOLOR_SDK_PACKAGE_SPEC

        try {
            const manifest = JSON.parse(readFileSync(resolve(import.meta.dirname, '../package.json'), 'utf8')) as {
                version: string
            }
            expect(defaultSdkPackageSpec()).toBe(`^${manifest.version}`)
        } finally {
            if (previousSpec === undefined) {
                delete process.env.HYPERCOLOR_SDK_PACKAGE_SPEC
            } else {
                process.env.HYPERCOLOR_SDK_PACKAGE_SPEC = previousSpec
            }
        }
    })

    test('honors the HYPERCOLOR_SDK_PACKAGE_SPEC override', () => {
        const previousSpec = process.env.HYPERCOLOR_SDK_PACKAGE_SPEC
        process.env.HYPERCOLOR_SDK_PACKAGE_SPEC = 'file:/tmp/local-sdk'

        try {
            expect(defaultSdkPackageSpec()).toBe('file:/tmp/local-sdk')
        } finally {
            if (previousSpec === undefined) {
                delete process.env.HYPERCOLOR_SDK_PACKAGE_SPEC
            } else {
                process.env.HYPERCOLOR_SDK_PACKAGE_SPEC = previousSpec
            }
        }
    })

    test('escapes Windows SDK file specs in TypeScript workspace manifests', () => {
        expectRenderedWorkspaceManifest('canvas', WINDOWS_SDK_PACKAGE_SPEC)
    })

    test('escapes Windows SDK file specs in HTML workspace manifests', () => {
        expectRenderedWorkspaceManifest('html', WINDOWS_SDK_PACKAGE_SPEC)
    })

    test('preserves POSIX SDK file specs in workspace manifests', () => {
        expectRenderedWorkspaceManifest('canvas', 'file:/home/bliss/dev/hypercolor/sdk/packages/core')
    })

    test('escapes Windows workspace names in TypeScript workspace manifests', () => {
        expectRenderedWorkspaceManifest('canvas', '^0.3.0', WINDOWS_WORKSPACE_NAME)
    })

    test('escapes Windows workspace names in HTML workspace manifests', () => {
        expectRenderedWorkspaceManifest('html', '^0.3.0', WINDOWS_WORKSPACE_NAME)
    })

    test('derives TypeScript workspace names from Windows relative targets', () => {
        expectWindowsTargetWorkspaceManifest('canvas', `${String.raw`.\my-effects`}\\`, 'my-effects')
    })

    test('derives HTML workspace names from Windows absolute targets', () => {
        expectWindowsTargetWorkspaceManifest('html', `${String.raw`C:\Users\Bliss\dev\html-effects`}\\`, 'html-effects')
    })

    test('preserves replacement tokens in SDK file specs', () => {
        expectRenderedWorkspaceManifest('canvas', REPLACEMENT_TOKEN_SDK_PACKAGE_SPEC)
        expectRenderedWorkspaceManifest('html', REPLACEMENT_TOKEN_SDK_PACKAGE_SPEC)
    })

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
            await runCommand(['bun', 'run', 'add', 'ember', '--template', 'shader'], workspaceDir, {
                EDITOR: '',
                VISUAL: '',
            })
            await runCommand(['bun', 'run', 'add', 'hud', '--template', 'face', '--audio'], workspaceDir, {
                EDITOR: '',
                VISUAL: '',
            })
            await runCommand(['bun', 'run', 'add', 'raw-html', '--template', 'html'], workspaceDir, {
                EDITOR: '',
                VISUAL: '',
            })

            await runCommand(['bun', 'run', 'build'], workspaceDir)
            await runCommand(['bun', 'run', 'validate'], workspaceDir)

            expect(existsSync(join(workspaceDir, 'dist', 'aurora.html'))).toBeTrue()
            expect(existsSync(join(workspaceDir, 'dist', 'ember.html'))).toBeTrue()
            expect(existsSync(join(workspaceDir, 'dist', 'hud.html'))).toBeTrue()
            expect(existsSync(join(workspaceDir, 'effects', 'raw-html.html'))).toBeTrue()

            await runCommand(['./node_modules/.bin/hypercolor', 'validate', 'effects/raw-html.html'], workspaceDir)
            await runCommand(['bun', 'run', 'ship'], workspaceDir, { XDG_DATA_HOME: xdgDataHome })
            await runCommand(['./node_modules/.bin/hypercolor', 'install', 'effects/raw-html.html'], workspaceDir, {
                XDG_DATA_HOME: xdgDataHome,
            })

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
            expect(readFileSync(join(workspaceDir, 'effects', 'glow-card.html'), 'utf8')).toContain(
                '<title>Glow Card</title>',
            )

            await runCommand(['bun', 'install'], workspaceDir)
            await runCommand(['bun', 'run', 'validate'], workspaceDir)
            await runCommand(['bun', 'run', 'ship'], workspaceDir, { XDG_DATA_HOME: xdgDataHome })

            expect(existsSync(join(xdgDataHome, 'hypercolor', 'effects', 'user', 'glow-card.html'))).toBeTrue()
        } finally {
            rmSync(tempRoot, { force: true, recursive: true })
        }
    }, 120_000)
})
