#!/usr/bin/env bun

/**
 * Effect Build Script — compiles SDK TypeScript effects into standalone HTML.
 *
 * Usage:
 *   bun scripts/build-effect.ts src/effects/borealis/main.ts
 *   bun scripts/build-effect.ts --all              # build all effects
 *   bun scripts/build-effect.ts --out effects/hypercolor/ src/effects/borealis/main.ts
 */

import { existsSync, mkdirSync, readdirSync } from 'node:fs'
import { basename, dirname, join, resolve } from 'node:path'
import * as esbuild from 'esbuild'

const SDK_ROOT = resolve(import.meta.dirname, '..')
const DEFAULT_OUT = resolve(SDK_ROOT, '..', 'effects', 'hypercolor')

// ── CLI Parsing ────────────────────────────────────────────────────────

function parseArgs() {
    const args = process.argv.slice(2)
    let outDir = DEFAULT_OUT
    let buildAll = false
    let buildFaces = false
    const entries: string[] = []

    for (let i = 0; i < args.length; i++) {
        if (args[i] === '--out' && args[i + 1]) {
            outDir = resolve(args[++i])
        } else if (args[i] === '--all') {
            buildAll = true
        } else if (args[i] === '--faces') {
            buildFaces = true
        } else {
            entries.push(resolve(args[i]))
        }
    }

    if (buildAll || buildFaces) {
        // Scan effects
        if (buildAll) {
            const effectsDir = resolve(SDK_ROOT, 'src', 'effects')
            if (existsSync(effectsDir)) {
                for (const dir of readdirSync(effectsDir, { withFileTypes: true })) {
                    if (dir.isDirectory()) {
                        const mainFile = join(effectsDir, dir.name, 'main.ts')
                        if (existsSync(mainFile)) entries.push(mainFile)
                    }
                }
            }
        }

        // Scan faces
        if (buildAll || buildFaces) {
            const facesDir = resolve(SDK_ROOT, 'src', 'faces')
            if (existsSync(facesDir)) {
                for (const dir of readdirSync(facesDir, { withFileTypes: true })) {
                    if (dir.isDirectory()) {
                        const mainFile = join(facesDir, dir.name, 'main.ts')
                        if (existsSync(mainFile)) entries.push(mainFile)
                    }
                }
            }
        }
    }

    if (entries.length === 0) {
        console.error('Usage: bun scripts/build-effect.ts [--all | --faces | <entry.ts>...]')
        process.exit(1)
    }

    return { entries, outDir }
}

// ── Metadata Extraction ────────────────────────────────────────────────

interface PresetDef {
    name: string
    description?: string
    controls: Record<string, unknown>
}

interface NewApiDef {
    type?: 'canvas' | 'webgl' | 'face'
    name: string
    shader?: string
    description?: string
    author?: string
    audio?: boolean
    screen?: boolean
    category?: string
    builtinId?: string
    presets?: PresetDef[]
    controls: Record<string, unknown>
    resolvedControls: Array<{
        key: string
        spec: {
            __type: string
            label: string
            defaultValue: unknown
            tooltip?: string
            group?: string
            meta: Record<string, unknown>
        }
        uniformName?: string
    }>
}

/**
 * Convert a new-API effect definition to the legacy ControlDef[] format
 * so the existing meta tag generation works unchanged.
 */
function newApiToControls(def: NewApiDef): ControlDef[] {
    return def.resolvedControls.map((ctrl) => {
        const base: ControlDef = {
            default: ctrl.spec.defaultValue as any,
            group: ctrl.spec.group,
            id: ctrl.key,
            label: ctrl.spec.label,
            tooltip: ctrl.spec.tooltip,
            type: ctrl.spec.__type === 'textfield' ? 'textfield' : ctrl.spec.__type,
        }
        if (ctrl.spec.meta.min != null) base.min = ctrl.spec.meta.min as number
        if (ctrl.spec.meta.max != null) base.max = ctrl.spec.meta.max as number
        if (ctrl.spec.meta.step != null) base.step = ctrl.spec.meta.step as number
        if (ctrl.spec.meta.values) base.values = ctrl.spec.meta.values as string[]
        if (ctrl.spec.meta.aspectLock != null) base.aspectLock = ctrl.spec.meta.aspectLock as number
        if (ctrl.spec.meta.preview) base.preview = ctrl.spec.meta.preview as 'screen' | 'web' | 'canvas'
        return base
    })
}

const BUILTIN_UNIFORMS = new Set(['iTime', 'iResolution', 'iMouse'])

function extractShaderUniforms(shader: string): Set<string> {
    const uniforms = new Set<string>()
    const matches = shader.matchAll(/uniform\s+\w+\s+(i\w+)\s*;/g)
    for (const match of matches) {
        uniforms.add(match[1])
    }
    return uniforms
}

function validateShaderBindings(entryPath: string, def: NewApiDef): void {
    if (!def.shader) return

    const shaderUniforms = extractShaderUniforms(def.shader)
    if (shaderUniforms.size === 0) return

    const controlUniforms = new Set(
        def.resolvedControls.map(
            (ctrl) => ctrl.uniformName ?? `i${ctrl.key.charAt(0).toUpperCase()}${ctrl.key.slice(1)}`,
        ),
    )

    const missing = Array.from(controlUniforms).filter((name) => !shaderUniforms.has(name))
    const extra = Array.from(shaderUniforms).filter(
        (name) => !BUILTIN_UNIFORMS.has(name) && !name.startsWith('iAudio') && !controlUniforms.has(name),
    )

    if (missing.length === 0 && extra.length === 0) return

    const effectId = basename(dirname(entryPath))
    if (missing.length > 0) {
        throw new Error(
            `Shader binding validation failed for ${effectId}: missing control uniforms ${missing.join(', ')}`,
        )
    }
    if (extra.length > 0) {
        console.warn(`  Warning: ${effectId} shader exposes uniforms with no controls: ${extra.join(', ')}`)
    }
}

async function extractMetadata(entryPath: string) {
    // Set metadata-only flag so initializeEffect() skips runtime init
    ;(globalThis as any).__HYPERCOLOR_METADATA_ONLY__ = true
    ;(globalThis as any).window = globalThis

    try {
        // Provide stubs for browser APIs the effect code references
        if (!(globalThis as any).document) {
            ;(globalThis as any).document = {
                addEventListener: () => {},
                getElementById: () => null,
                readyState: 'complete',
            }
        }

        // Clear any previous effect defs
        delete (globalThis as any).__hypercolorEffectDefs__
        delete (globalThis as any).__hypercolorEffectInstance__

        await import(entryPath)

        // ── New API path: check __hypercolorEffectDefs__ first ────────
        const defs = (globalThis as any).__hypercolorEffectDefs__ as NewApiDef[] | undefined
        if (defs && defs.length > 0) {
            const def = defs[defs.length - 1] // use last entry (single-effect files)
            validateShaderBindings(entryPath, def)
            return {
                controls: newApiToControls(def),
                effect: {
                    audioReactive: def.audio ?? false,
                    author: def.author ?? 'Hypercolor',
                    builtinId: def.builtinId,
                    category: def.category,
                    description: def.description ?? '',
                    name: def.name,
                    presets: def.presets ?? [],
                    renderer: def.type === 'canvas' ? 'canvas2d' : def.type === 'face' ? undefined : 'webgl',
                    screenReactive: def.screen ?? false,
                    type: def.type,
                },
            }
        }

        console.warn(`  Warning: could not extract metadata from ${entryPath} (no __hypercolorEffectDefs__)`)
        return { controls: [], effect: null }
    } catch (err) {
        if (err instanceof Error && err.message.startsWith('Shader binding validation failed')) {
            throw err
        }
        console.warn(`  Warning: metadata extraction failed: ${err}`)
        return { controls: [], effect: null }
    } finally {
        delete (globalThis as any).__HYPERCOLOR_METADATA_ONLY__
        delete (globalThis as any).__hypercolorEffectInstance__
        delete (globalThis as any).__hypercolorEffectDefs__
    }
}

// ── Meta Tag Generation ────────────────────────────────────────────────

interface ControlDef {
    id: string
    type: string
    label?: string
    tooltip?: string
    group?: string
    default?: any
    min?: number
    max?: number
    values?: string[]
    step?: number
    aspectLock?: number
    preview?: 'screen' | 'web' | 'canvas'
}

function controlToMeta(ctrl: ControlDef): string {
    const attrs: string[] = [`property="${ctrl.id}"`]

    if (ctrl.label) attrs.push(`label="${escapeAttr(ctrl.label)}"`)
    attrs.push(`type="${ctrl.type}"`)

    if (ctrl.min != null) attrs.push(`min="${ctrl.min}"`)
    if (ctrl.max != null) attrs.push(`max="${ctrl.max}"`)
    if (ctrl.step != null) attrs.push(`step="${ctrl.step}"`)
    if (ctrl.default != null) {
        const defaultValue =
            ctrl.type === 'rect' && typeof ctrl.default === 'object'
                ? rectDefaultAttr(ctrl.default as { x: number; y: number; width: number; height: number })
                : String(ctrl.default)
        attrs.push(`default="${escapeAttr(defaultValue)}"`)
    }
    if (ctrl.values?.length) attrs.push(`values="${ctrl.values.map(escapeAttr).join(',')}"`)
    if (ctrl.tooltip) attrs.push(`tooltip="${escapeAttr(ctrl.tooltip)}"`)
    if (ctrl.group) attrs.push(`group="${escapeAttr(ctrl.group)}"`)
    if (ctrl.aspectLock != null) attrs.push(`aspectLock="${ctrl.aspectLock}"`)
    if (ctrl.preview) attrs.push(`preview="${ctrl.preview}"`)

    return `  <meta ${attrs.join(' ')}/>`
}

function presetToMeta(preset: PresetDef): string {
    const attrs: string[] = [`preset="${escapeAttr(preset.name)}"`]
    if (preset.description) attrs.push(`preset-description="${escapeAttr(preset.description)}"`)
    attrs.push(`preset-controls='${JSON.stringify(preset.controls)}'`)
    return `  <meta ${attrs.join(' ')}/>`
}

function escapeAttr(s: string): string {
    return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;')
}

function rectDefaultAttr(rect: { x: number; y: number; width: number; height: number }): string {
    return [rect.x, rect.y, rect.width, rect.height].join(',')
}

// ── Face HTML Template ─────────────────────────────────────────────────

function generateFaceHTML(
    faceName: string,
    description: string,
    author: string,
    controlMetas: string[],
    presetMetas: string[],
    jsBundle: string,
): string {
    const presetBlock = presetMetas.length > 0 ? `\n${presetMetas.join('\n')}` : ''
    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${escapeAttr(faceName)}</title>
  <meta description="${escapeAttr(description)}"/>
  <meta publisher="${escapeAttr(author)}"/>
  <meta category="display"/>
${controlMetas.join('\n')}${presetBlock}
</head>
<body style="margin:0;overflow:hidden;background:transparent;-webkit-user-select:none;user-select:none">
  <div id="faceContainer" style="position:relative;overflow:hidden;width:100vw;height:100vh;background:transparent">
    <canvas id="faceCanvas" style="position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:2"></canvas>
  </div>
  <script>
${jsBundle}
  </script>
</body>
</html>
`
}

// ── Effect HTML Template ──────────────────────────────────────────────

function generateHTML(
    effectName: string,
    description: string,
    author: string,
    audioReactive: boolean,
    screenReactive: boolean,
    category: string | undefined,
    builtinId: string | undefined,
    renderer: string | undefined,
    controlMetas: string[],
    presetMetas: string[],
    jsBundle: string,
): string {
    const audioTag = `\n  <meta audio-reactive="${audioReactive}"/>`
    const screenTag = `\n  <meta screen-reactive="${screenReactive}"/>`
    const categoryTag = category ? `\n  <meta category="${escapeAttr(category)}"/>` : ''
    const builtinTag = builtinId ? `\n  <meta builtin-id="${escapeAttr(builtinId)}"/>` : ''
    const rendererTag = renderer ? `\n  <meta renderer="${escapeAttr(renderer)}"/>` : ''
    const presetBlock = presetMetas.length > 0 ? `\n${presetMetas.join('\n')}` : ''
    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${escapeAttr(effectName)}</title>
  <meta description="${escapeAttr(description)}"/>
  <meta publisher="${escapeAttr(author)}"/>${audioTag}${screenTag}${categoryTag}${builtinTag}${rendererTag}
${controlMetas.join('\n')}${presetBlock}
</head>
<body style="margin:0;overflow:hidden;background:#000">
  <div id="exStage" style="position:relative;overflow:hidden;background:#000;width:100vw;height:100vh">
    <canvas id="exCanvas" style="display:block;width:100%;height:100%"></canvas>
  </div>
  <script>
${jsBundle}
  </script>
</body>
</html>
`
}

// ── Bundle ──────────────────────────────────────────────────────────────

async function bundleEffect(entryPath: string): Promise<string> {
    const result = await esbuild.build({
        // Resolve @hypercolor/sdk to the local package source
        alias: {
            '@hypercolor/sdk': resolve(SDK_ROOT, 'packages', 'core', 'src', 'index.ts'),
        },
        bundle: true,
        entryPoints: [entryPath],
        external: [],
        format: 'iife',
        loader: { '.glsl': 'text' },
        logLevel: 'warning',
        minify: false, // Servo needs readable JS
        // Help esbuild find workspace packages' deps
        nodePaths: [resolve(SDK_ROOT, 'node_modules'), resolve(SDK_ROOT, 'packages', 'core', 'node_modules')],
        target: 'es2024',
        write: false,
    })

    if (result.outputFiles?.length) {
        return result.outputFiles[0].text
    }
    throw new Error('esbuild produced no output')
}

// ── Main ───────────────────────────────────────────────────────────────

async function buildEffect(entryPath: string, outDir: string) {
    const effectDir = dirname(entryPath)
    const effectId = basename(effectDir)
    const isFaceSource = entryPath.includes(`${join('src', 'faces')}`)

    const label = isFaceSource ? 'face' : 'effect'
    console.log(`\x1b[38;2;128;255;234m  Building ${label}\x1b[0m ${effectId}`)

    // 1. Extract metadata
    const { effect, controls } = await extractMetadata(entryPath)
    const effectName = effect?.name ?? effectId
    const description = effect?.description ?? ''
    const author = effect?.author ?? 'Hypercolor'

    // Detect face type from metadata (face() stores type: 'face')
    const metadataType = (effect as any)?.type as string | undefined
    const isFace = isFaceSource || metadataType === 'face'

    // Cross-directory validation — catch misplaced sources early
    if (isFaceSource && metadataType && metadataType !== 'face') {
        throw new Error(`${effectId}: effect() found in src/faces/ — move to src/effects/`)
    }
    if (!isFaceSource && metadataType === 'face') {
        console.warn(`  \x1b[38;2;241;250;140mWarning:\x1b[0m ${effectId} uses face() but lives outside src/faces/ — building as face anyway`)
    }

    // 2. Generate control meta tags
    const controlMetas = (controls as ControlDef[]).map(controlToMeta)

    // 3. Generate preset meta tags
    const presetMetas = (effect as any)?.presets ? ((effect as any).presets as PresetDef[]).map(presetToMeta) : []

    // 4. Bundle JS
    const jsBundle = await bundleEffect(entryPath)

    // 5. Generate HTML — face or effect template
    const html = isFace
        ? generateFaceHTML(effectName, description, author, controlMetas, presetMetas, jsBundle)
        : generateHTML(
              effectName,
              description,
              author,
              (effect as any)?.audioReactive ?? false,
              (effect as any)?.screenReactive ?? false,
              (effect as any)?.category,
              (effect as any)?.builtinId,
              (effect as any)?.renderer,
              controlMetas,
              presetMetas,
              jsBundle,
          )

    // 6. Write output
    mkdirSync(outDir, { recursive: true })
    const outPath = join(outDir, `${effectId}.html`)
    await Bun.write(outPath, html)

    const sizeKB = (new TextEncoder().encode(html).length / 1024).toFixed(1)
    const icon = isFace ? '💎' : '✓'
    console.log(`\x1b[38;2;80;250;123m  ${icon}\x1b[0m ${outPath} (${sizeKB} KB)`)
}

async function main() {
    const { outDir, entries } = parseArgs()

    console.log('\x1b[38;2;225;53;255m  Hypercolor Effect Builder\x1b[0m')
    console.log(`  Output: ${outDir}\n`)

    for (const entry of entries) {
        await buildEffect(entry, outDir)
    }

    const faceCount = entries.filter((e) => e.includes(join('src', 'faces'))).length
    const effectCount = entries.length - faceCount
    const parts = []
    if (effectCount > 0) parts.push(`${effectCount} effect(s)`)
    if (faceCount > 0) parts.push(`${faceCount} face(s)`)
    console.log(`\n\x1b[38;2;80;250;123m  ✓ ${parts.join(' + ')} built\x1b[0m`)
}

main().catch((err) => {
    console.error('\x1b[38;2;255;99;99m  ✗ Build failed:\x1b[0m', err)
    process.exit(1)
})
