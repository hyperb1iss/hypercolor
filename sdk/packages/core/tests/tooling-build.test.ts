import { describe, expect, test } from 'bun:test'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { buildArtifacts, discoverWorkspaceEntries, HYPERCOLOR_FORMAT_VERSION } from '../src/tooling'

const SDK_ROOT = resolve(import.meta.dirname, '../../..')
const SDK_ALIAS = resolve(SDK_ROOT, 'packages/core/src/index.ts')

describe('tooling build', () => {
    test('discovers entries under configured roots', () => {
        const entries = discoverWorkspaceEntries(resolve(SDK_ROOT), ['src/effects', 'src/faces'])

        expect(entries.some((entry) => entry.endsWith('src/effects/borealis/main.ts'))).toBeTrue()
        expect(entries.some((entry) => entry.endsWith('src/faces/neon-clock/main.ts'))).toBeTrue()
    })

    test('builds an effect html artifact with version metadata', async () => {
        const outDir = mkdtempSync(join(tmpdir(), 'hypercolor-build-'))
        try {
            const [result] = await buildArtifacts({
                entryPaths: [resolve(SDK_ROOT, 'src/effects/borealis/main.ts')],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })

            expect(result.id).toBe('borealis')
            expect(existsSync(result.outputPath)).toBeTrue()
            const html = readFileSync(result.outputPath, 'utf8')
            expect(html).toContain(`<meta name="hypercolor-version" content="${HYPERCOLOR_FORMAT_VERSION}" />`)
            expect(html).toContain('<canvas id="exCanvas"')
            expect(html).toContain('<title>Borealis</title>')
        } finally {
            rmSync(outDir, { force: true, recursive: true })
        }
    })

    test('inlines GLSL source for WebGL effects', async () => {
        const outDir = mkdtempSync(join(tmpdir(), 'hypercolor-webgl-build-'))
        try {
            const [result] = await buildArtifacts({
                entryPaths: [resolve(SDK_ROOT, 'src/effects/arc-storm/main.ts')],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })

            const html = readFileSync(result.outputPath, 'utf8')
            expect(html).toContain('<title>Arc Storm</title>')
            expect(html).toContain('#version 300 es')
            expect(html).not.toContain('var fragment_default = "./fragment-')
        } finally {
            rmSync(outDir, { force: true, recursive: true })
        }
    })

    test('builds a face html artifact with face container markup', async () => {
        const outDir = mkdtempSync(join(tmpdir(), 'hypercolor-face-build-'))
        try {
            const [result] = await buildArtifacts({
                entryPaths: [resolve(SDK_ROOT, 'src/faces/neon-clock/main.ts')],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })

            expect(result.kind).toBe('face')
            const html = readFileSync(result.outputPath, 'utf8')
            expect(html).toContain('id="faceContainer"')
            expect(html).toContain(`<meta name="hypercolor-version" content="${HYPERCOLOR_FORMAT_VERSION}" />`)
        } finally {
            rmSync(outDir, { force: true, recursive: true })
        }
    })

    test('marks shockwave as audio-reactive in built metadata', async () => {
        const outDir = mkdtempSync(join(tmpdir(), 'hypercolor-shockwave-build-'))
        try {
            const [result] = await buildArtifacts({
                entryPaths: [resolve(SDK_ROOT, 'src/effects/shockwave/main.ts')],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })

            expect(result.metadata.audioReactive).toBeTrue()
            const html = readFileSync(result.outputPath, 'utf8')
            expect(html).toContain('<meta audio-reactive="true" />')
        } finally {
            rmSync(outDir, { force: true, recursive: true })
        }
    })

    test('fails fast when an effect uses audio helpers without audio: true', async () => {
        const tempRoot = mkdtempSync(join(tmpdir(), 'hypercolor-audio-optin-'))
        const entryDir = join(tempRoot, 'missing-audio-optin')
        const entryPath = join(entryDir, 'main.ts')
        const outDir = join(tempRoot, 'dist')

        mkdirSync(entryDir, { recursive: true })
        writeFileSync(
            entryPath,
            `
import { audio, canvas } from ${JSON.stringify(SDK_ALIAS)}

export default canvas.stateful('Missing Audio Opt-In', {}, () => {
    return () => {
        const data = audio()
        void data.level
    }
})
`,
        )

        try {
            await expect(
                buildArtifacts({
                    entryPaths: [entryPath],
                    outDir,
                    sdkAliasPath: SDK_ALIAS,
                }),
            ).rejects.toThrow('missing audio: true')
        } finally {
            rmSync(tempRoot, { force: true, recursive: true })
        }
    })
})
