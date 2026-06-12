import { describe, expect, test } from 'bun:test'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { buildArtifacts, discoverWorkspaceEntries, HYPERCOLOR_FORMAT_VERSION } from '../src/tooling'

const SDK_ROOT = resolve(import.meta.dirname, '../../..')
const SDK_ALIAS = resolve(SDK_ROOT, 'packages/core/src/index.ts')

describe('tooling build', () => {
    test('discovers entries under configured roots', () => {
        const entries = discoverWorkspaceEntries(resolve(SDK_ROOT), ['src/effects', 'src/faces'])
        const normalizedEntries = entries.map((entry) => entry.replaceAll('\\', '/'))

        expect(normalizedEntries.some((entry) => entry.endsWith('src/effects/borealis/main.ts'))).toBeTrue()
        expect(normalizedEntries.some((entry) => entry.endsWith('src/faces/neon-clock/main.ts'))).toBeTrue()
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
            expect(html).toContain('this.gl.flush();')
            expect(html).not.toContain('this.gl.finish();')
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

    test('skips rewriting unchanged html artifacts', async () => {
        const outDir = mkdtempSync(join(tmpdir(), 'hypercolor-stable-build-'))
        try {
            const entryPath = resolve(SDK_ROOT, 'src/effects/borealis/main.ts')
            const [result] = await buildArtifacts({
                entryPaths: [entryPath],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })

            const stableTimestamp = new Date('2024-01-01T00:00:00.000Z')
            utimesSync(result.outputPath, stableTimestamp, stableTimestamp)
            const mtimeBefore = statSync(result.outputPath).mtimeMs

            await buildArtifacts({
                entryPaths: [entryPath],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })

            expect(statSync(result.outputPath).mtimeMs).toBe(mtimeBefore)
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

    test('faces emit the audio-reactive meta from the audio opt-in', async () => {
        const tempRoot = mkdtempSync(join(tmpdir(), 'hypercolor-face-audio-'))
        const outDir = join(tempRoot, 'dist')
        try {
            const writeFace = (slug: string, options: string) => {
                const entryDir = join(tempRoot, slug)
                mkdirSync(entryDir, { recursive: true })
                const entryPath = join(entryDir, 'main.ts')
                writeFileSync(
                    entryPath,
                    `import { face } from ${JSON.stringify(SDK_ALIAS)}

export default face('${slug}', {}, { ${options} }, (ctx) => {
    return (_time, _controls, _sensors, audio) => {
        void audio.data().level
        void ctx.width
    }
})
`,
                )
                return entryPath
            }

            const [withAudio] = await buildArtifacts({
                entryPaths: [writeFace('audio-probe', 'audio: true')],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })
            expect(withAudio.metadata.audioReactive).toBeTrue()
            const audioHtml = readFileSync(withAudio.outputPath, 'utf8')
            expect(audioHtml).toContain('<meta audio-reactive="true" />')

            const [silent] = await buildArtifacts({
                entryPaths: [writeFace('silent-probe', "description: 'control'")],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })
            expect(silent.metadata.audioReactive).toBeFalse()
            const silentHtml = readFileSync(silent.outputPath, 'utf8')
            expect(silentHtml).toContain('<meta audio-reactive="false" />')
        } finally {
            rmSync(tempRoot, { force: true, recursive: true })
        }
    })

    test('faces emit the data-sources meta from media/net/lighting opt-ins', async () => {
        const tempRoot = mkdtempSync(join(tmpdir(), 'hypercolor-face-sources-'))
        const outDir = join(tempRoot, 'dist')
        try {
            const writeFace = (slug: string, options: string) => {
                const entryDir = join(tempRoot, slug)
                mkdirSync(entryDir, { recursive: true })
                const entryPath = join(entryDir, 'main.ts')
                writeFileSync(
                    entryPath,
                    `import { face } from ${JSON.stringify(SDK_ALIAS)}

export default face('${slug}', {}, { ${options} }, (ctx) => {
    return (_time, _controls, _sensors, _audio, data) => {
        void data.media.available()
        void ctx.width
    }
})
`,
                )
                return entryPath
            }

            const [withSources] = await buildArtifacts({
                entryPaths: [writeFace('sources-probe', 'media: true, net: true')],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })
            expect(withSources.metadata.dataSources).toEqual(['media', 'net'])
            const sourcesHtml = readFileSync(withSources.outputPath, 'utf8')
            expect(sourcesHtml).toContain('<meta data-sources="media,net" />')

            const [lightingOnly] = await buildArtifacts({
                entryPaths: [writeFace('lighting-probe', 'lighting: true')],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })
            expect(lightingOnly.metadata.dataSources).toEqual(['lighting'])
            expect(readFileSync(lightingOnly.outputPath, 'utf8')).toContain('<meta data-sources="lighting" />')

            const [bare] = await buildArtifacts({
                entryPaths: [writeFace('bare-probe', "description: 'control'")],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })
            expect(bare.metadata.dataSources).toEqual([])
            expect(readFileSync(bare.outputPath, 'utf8')).not.toContain('data-sources')
        } finally {
            rmSync(tempRoot, { force: true, recursive: true })
        }
    })

    test('emits asset control media kind metadata', async () => {
        const tempRoot = mkdtempSync(join(tmpdir(), 'hypercolor-asset-control-'))
        const entryDir = join(tempRoot, 'media-mask')
        const entryPath = join(entryDir, 'main.ts')
        const outDir = join(tempRoot, 'dist')

        mkdirSync(entryDir, { recursive: true })
        writeFileSync(
            entryPath,
            `
import { asset, effect } from ${JSON.stringify(SDK_ALIAS)}

const shader = \`#version 300 es
precision highp float;
out vec4 fragColor;
void main() {
    fragColor = vec4(1.0);
}
\`

export default effect('Media Mask', shader, {
    mask: asset('Mask', 'image'),
})
`,
        )

        try {
            const [result] = await buildArtifacts({
                entryPaths: [entryPath],
                outDir,
                sdkAliasPath: SDK_ALIAS,
            })

            expect(result.metadata.controls[0]?.type).toBe('asset')
            expect(result.metadata.controls[0]?.mediaKind).toBe('image')
            expect(result.html).toContain('type="asset"')
            expect(result.html).toContain('media-kind="image"')
        } finally {
            rmSync(tempRoot, { force: true, recursive: true })
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
