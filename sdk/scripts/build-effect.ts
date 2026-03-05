#!/usr/bin/env bun
/**
 * Effect Build Script — compiles SDK TypeScript effects into standalone HTML.
 *
 * Usage:
 *   bun scripts/build-effect.ts src/effects/borealis/main.ts
 *   bun scripts/build-effect.ts --all              # build all effects
 *   bun scripts/build-effect.ts --out effects/evolved/ src/effects/borealis/main.ts
 */

import * as esbuild from 'esbuild'
import { basename, dirname, join, resolve } from 'node:path'
import { existsSync, mkdirSync, readdirSync, readFileSync } from 'node:fs'
import { extractControlsFromClass, extractEffectMetadata } from '@hypercolor/sdk'

const SDK_ROOT = resolve(import.meta.dirname, '..')
const DEFAULT_OUT = resolve(SDK_ROOT, '..', 'effects', 'evolved')

// ── CLI Parsing ────────────────────────────────────────────────────────

function parseArgs() {
    const args = process.argv.slice(2)
    let outDir = DEFAULT_OUT
    let buildAll = false
    const entries: string[] = []

    for (let i = 0; i < args.length; i++) {
        if (args[i] === '--out' && args[i + 1]) {
            outDir = resolve(args[++i])
        } else if (args[i] === '--all') {
            buildAll = true
        } else {
            entries.push(resolve(args[i]))
        }
    }

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

    if (entries.length === 0) {
        console.error('Usage: bun scripts/build-effect.ts [--all | <entry.ts>...]')
        process.exit(1)
    }

    return { outDir, entries }
}

// ── Metadata Extraction ────────────────────────────────────────────────

async function extractMetadata(entryPath: string) {
    // Set metadata-only flag so initializeEffect() skips runtime init
    ;(globalThis as any).__HYPERCOLOR_METADATA_ONLY__ = true
    ;(globalThis as any).window = globalThis

    try {
        // Provide stubs for browser APIs the effect code references
        if (!(globalThis as any).document) {
            ;(globalThis as any).document = {
                getElementById: () => null,
                readyState: 'complete',
                addEventListener: () => {},
            }
        }

        const mod = await import(entryPath)

        // initializeEffect() stores the instance on globalThis in metadata-only mode
        const effectInstance =
            (globalThis as any).__hypercolorEffectInstance__ ??
            (globalThis as any).effectInstance ??
            mod.default

        if (!effectInstance) {
            // Try to find any class instance in the module's exports
            for (const val of Object.values(mod)) {
                if (val && typeof val === 'object' && val.constructor) {
                    const meta = extractEffectMetadata(val.constructor)
                    if (meta) {
                        const controls = extractControlsFromClass(val.constructor)
                        return { effect: meta, controls }
                    }
                }
            }
            console.warn(`  Warning: could not extract metadata from ${entryPath}`)
            return { effect: null, controls: [] }
        }

        const effect = extractEffectMetadata(effectInstance.constructor)
        const controls = extractControlsFromClass(effectInstance.constructor)
        return { effect, controls }
    } catch (err) {
        console.warn(`  Warning: metadata extraction failed: ${err}`)
        return { effect: null, controls: [] }
    } finally {
        delete (globalThis as any).__HYPERCOLOR_METADATA_ONLY__
        delete (globalThis as any).__hypercolorEffectInstance__
    }
}

// ── Meta Tag Generation ────────────────────────────────────────────────

interface ControlDef {
    id: string
    type: string
    label?: string
    tooltip?: string
    default?: any
    min?: number
    max?: number
    values?: string[]
    step?: number
}

function controlToMeta(ctrl: ControlDef): string {
    const attrs: string[] = [`property="${ctrl.id}"`]

    if (ctrl.label) attrs.push(`label="${escapeAttr(ctrl.label)}"`)
    attrs.push(`type="${ctrl.type}"`)

    if (ctrl.min != null) attrs.push(`min="${ctrl.min}"`)
    if (ctrl.max != null) attrs.push(`max="${ctrl.max}"`)
    if (ctrl.step != null) attrs.push(`step="${ctrl.step}"`)
    if (ctrl.default != null) attrs.push(`default="${escapeAttr(String(ctrl.default))}"`)
    if (ctrl.values?.length) attrs.push(`values="${ctrl.values.map(escapeAttr).join(',')}"`)
    if (ctrl.tooltip) attrs.push(`tooltip="${escapeAttr(ctrl.tooltip)}"`)

    return `  <meta ${attrs.join(' ')}/>`
}

function escapeAttr(s: string): string {
    return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;')
}

// ── HTML Template ──────────────────────────────────────────────────────

function generateHTML(
    effectName: string,
    description: string,
    author: string,
    controlMetas: string[],
    jsBundle: string,
): string {
    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${escapeAttr(effectName)}</title>
  <meta description="${escapeAttr(description)}"/>
  <meta publisher="${escapeAttr(author)}"/>
${controlMetas.join('\n')}
</head>
<body style="margin:0;overflow:hidden;background:#000">
  <canvas id="exCanvas" width="320" height="200"></canvas>
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
        entryPoints: [entryPath],
        bundle: true,
        format: 'iife',
        target: 'es2024',
        minify: false, // Servo needs readable JS
        write: false,
        loader: { '.glsl': 'text' },
        // Resolve @hypercolor/sdk to the local package source
        alias: {
            '@hypercolor/sdk': resolve(SDK_ROOT, 'packages', 'core', 'src', 'index.ts'),
        },
        // Help esbuild find workspace packages' deps
        nodePaths: [
            resolve(SDK_ROOT, 'node_modules'),
            resolve(SDK_ROOT, 'packages', 'core', 'node_modules'),
        ],
        external: [],
        logLevel: 'warning',
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

    console.log(`\x1b[38;2;128;255;234m  Building\x1b[0m ${effectId}`)

    // 1. Extract metadata
    const { effect, controls } = await extractMetadata(entryPath)
    const effectName = effect?.name ?? effectId
    const description = effect?.description ?? ''
    const author = effect?.author ?? 'Hypercolor'

    // 2. Generate control meta tags
    const controlMetas = (controls as ControlDef[]).map(controlToMeta)

    // 3. Bundle JS
    const jsBundle = await bundleEffect(entryPath)

    // 4. Generate HTML
    const html = generateHTML(effectName, description, author, controlMetas, jsBundle)

    // 5. Write output
    mkdirSync(outDir, { recursive: true })
    const outPath = join(outDir, `${effectId}.html`)
    Bun.write(outPath, html)

    const sizeKB = (new TextEncoder().encode(html).length / 1024).toFixed(1)
    console.log(`\x1b[38;2;80;250;123m  ✓\x1b[0m ${outPath} (${sizeKB} KB)`)
}

async function main() {
    const { outDir, entries } = parseArgs()

    console.log(`\x1b[38;2;225;53;255m  Hypercolor Effect Builder\x1b[0m`)
    console.log(`  Output: ${outDir}\n`)

    for (const entry of entries) {
        await buildEffect(entry, outDir)
    }

    console.log(`\n\x1b[38;2;80;250;123m  ✓ ${entries.length} effect(s) built\x1b[0m`)
}

main().catch((err) => {
    console.error(`\x1b[38;2;255;99;99m  ✗ Build failed:\x1b[0m`, err)
    process.exit(1)
})
