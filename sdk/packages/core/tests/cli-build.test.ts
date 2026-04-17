import { describe, expect, test } from 'bun:test'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { main } from '../src/cli'

const SDK_ROOT = resolve(import.meta.dirname, '../../..')

describe('sdk cli build + validate', () => {
    test('build command writes an artifact for an explicit entry', async () => {
        const outDir = mkdtempSync(join(tmpdir(), 'hypercolor-cli-build-'))
        const output: string[] = []

        try {
            const exitCode = await main(
                [
                    'build',
                    'src/effects/borealis/main.ts',
                    '--workspace-root',
                    SDK_ROOT,
                    '--out',
                    outDir,
                    '--sdk-alias-path',
                    'packages/core/src/index.ts',
                ],
                {
                    cwd: SDK_ROOT,
                    stdout: {
                        error: (message: string) => output.push(message),
                        log: (message: string) => output.push(message),
                    },
                },
            )

            expect(exitCode).toBe(0)
            expect(readFileSync(join(outDir, 'borealis.html'), 'utf8')).toContain('<title>Borealis</title>')
            expect(output.some((line) => line.includes('borealis'))).toBeTrue()
        } finally {
            rmSync(outDir, { force: true, recursive: true })
        }
    })

    test('validate command returns zero for a generated artifact', async () => {
        const outDir = mkdtempSync(join(tmpdir(), 'hypercolor-cli-validate-'))

        try {
            const buildExit = await main(
                [
                    'build',
                    'src/effects/borealis/main.ts',
                    '--workspace-root',
                    SDK_ROOT,
                    '--out',
                    outDir,
                    '--sdk-alias-path',
                    'packages/core/src/index.ts',
                ],
                { cwd: SDK_ROOT, stdout: console },
            )
            expect(buildExit).toBe(0)

            const validateExit = await main(['validate', join(outDir, 'borealis.html')], { cwd: SDK_ROOT, stdout: console })
            expect(validateExit).toBe(0)
        } finally {
            rmSync(outDir, { force: true, recursive: true })
        }
    })
})
