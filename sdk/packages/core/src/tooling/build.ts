import { existsSync, mkdirSync, readdirSync } from 'node:fs'
import { join, resolve } from 'node:path'

import { HYPERCOLOR_FORMAT_VERSION } from './constants'
import { artifactIdFromEntry, extractArtifactMetadata } from './metadata'
import type { BuildArtifactResult, BuildArtifactsOptions, BuildControlDef, PresetDef } from './types'

function discoverEntryPathsInRoot(root: string): string[] {
    if (!existsSync(root)) return []

    const entries: string[] = []
    for (const dirent of readdirSync(root, { withFileTypes: true })) {
        if (!dirent.isDirectory()) continue
        const mainFile = join(root, dirent.name, 'main.ts')
        if (existsSync(mainFile)) entries.push(mainFile)
    }

    return entries.sort()
}

export function discoverWorkspaceEntries(workspaceRoot: string, roots: string[]): string[] {
    return roots.flatMap((root) => discoverEntryPathsInRoot(resolve(workspaceRoot, root)))
}

function escapeAttr(value: string): string {
    return value.replaceAll('&', '&amp;').replaceAll('"', '&quot;')
}

function stringifyDefaultValue(value: unknown): string {
    if (value && typeof value === 'object' && 'x' in value && 'y' in value && 'width' in value && 'height' in value) {
        const rect = value as { x: number; y: number; width: number; height: number }
        return [rect.x, rect.y, rect.width, rect.height].join(',')
    }

    return String(value)
}

function controlToMeta(control: BuildControlDef): string {
    const attrs: string[] = [`property="${escapeAttr(control.id)}"`]

    if (control.label) attrs.push(`label="${escapeAttr(control.label)}"`)
    attrs.push(`type="${escapeAttr(control.type)}"`)
    if (control.min != null) attrs.push(`min="${control.min}"`)
    if (control.max != null) attrs.push(`max="${control.max}"`)
    if (control.step != null) attrs.push(`step="${control.step}"`)
    if (control.default != null) attrs.push(`default="${escapeAttr(stringifyDefaultValue(control.default))}"`)
    if (control.values?.length) attrs.push(`values="${control.values.map(escapeAttr).join(',')}"`)
    if (control.tooltip) attrs.push(`tooltip="${escapeAttr(control.tooltip)}"`)
    if (control.group) attrs.push(`group="${escapeAttr(control.group)}"`)
    if (control.aspectLock != null) attrs.push(`aspectLock="${control.aspectLock}"`)
    if (control.preview) attrs.push(`preview="${control.preview}"`)

    return `  <meta ${attrs.join(' ')} />`
}

function presetToMeta(preset: PresetDef): string {
    const attrs = [`preset="${escapeAttr(preset.name)}"`]
    if (preset.description) attrs.push(`preset-description="${escapeAttr(preset.description)}"`)
    attrs.push(`preset-controls='${JSON.stringify(preset.controls)}'`)
    return `  <meta ${attrs.join(' ')} />`
}

function effectHtml(args: {
    author: string
    builtinId?: string
    category?: string
    controlMetas: string[]
    description: string
    jsBundle: string
    name: string
    presets: string[]
    renderer?: string
    audioReactive: boolean
    screenReactive: boolean
}): string {
    const presetBlock = args.presets.length > 0 ? `\n${args.presets.join('\n')}` : ''
    const categoryTag = args.category ? `\n    <meta category="${escapeAttr(args.category)}" />` : ''
    const builtinTag = args.builtinId ? `\n    <meta builtin-id="${escapeAttr(args.builtinId)}" />` : ''
    const rendererTag = args.renderer ? `\n    <meta renderer="${escapeAttr(args.renderer)}" />` : ''

    return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <meta name="hypercolor-version" content="${HYPERCOLOR_FORMAT_VERSION}" />
    <title>${escapeAttr(args.name)}</title>
    <meta description="${escapeAttr(args.description)}" />
    <meta publisher="${escapeAttr(args.author)}" />
    <meta audio-reactive="${args.audioReactive}" />
    <meta screen-reactive="${args.screenReactive}" />${categoryTag}${builtinTag}${rendererTag}
${args.controlMetas.join('\n')}${presetBlock}
  </head>
  <body style="margin:0;overflow:hidden;background:#000;">
    <div id="exStage" style="position:relative;overflow:hidden;background:#000;width:100vw;height:100vh;">
      <canvas id="exCanvas" style="display:block;width:100%;height:100%;"></canvas>
    </div>
    <script>
${args.jsBundle}
    </script>
  </body>
</html>
`
}

function faceHtml(args: {
    author: string
    controlMetas: string[]
    description: string
    jsBundle: string
    name: string
    presets: string[]
}): string {
    const presetBlock = args.presets.length > 0 ? `\n${args.presets.join('\n')}` : ''

    return `<!DOCTYPE html>
<html lang="en" style="width:100%;height:100%;background:transparent;">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <meta name="hypercolor-version" content="${HYPERCOLOR_FORMAT_VERSION}" />
    <title>${escapeAttr(args.name)}</title>
    <meta description="${escapeAttr(args.description)}" />
    <meta publisher="${escapeAttr(args.author)}" />
    <meta category="display" />
${args.controlMetas.join('\n')}${presetBlock}
  </head>
  <body style="margin:0;width:100%;height:100%;overflow:hidden;background:transparent;-webkit-user-select:none;user-select:none;">
    <div id="faceContainer" style="position:relative;overflow:hidden;width:100vw;height:100vh;background:transparent;">
      <canvas id="faceCanvas" style="position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:2;"></canvas>
    </div>
    <script>
${args.jsBundle}
    </script>
  </body>
</html>
`
}

async function bundleEntry(entryPath: string, sdkAliasPath: string | undefined, minify: boolean): Promise<string> {
    const buildConfig = {
        entrypoints: [entryPath],
        format: 'iife',
        loader: {
            '.glsl': 'text',
        },
        minify,
        sourcemap: 'none',
        target: 'browser',
        write: false,
    } as Bun.BuildConfig & {
        alias?: Record<string, string>
    }
    if (sdkAliasPath) {
        buildConfig.alias = { '@hypercolor/sdk': sdkAliasPath }
    }

    const result = await Bun.build(buildConfig)

    if (!result.success) {
        throw new Error(result.logs.map((log) => log.message).join('\n') || `Failed to bundle ${entryPath}`)
    }

    const output = result.outputs.at(0)
    if (!output) {
        throw new Error(`Bun.build produced no output for ${entryPath}`)
    }

    return output.text()
}

export async function buildArtifactDocument(options: {
    entryPath: string
    outDir: string
    minify?: boolean
    sdkAliasPath?: string
}): Promise<BuildArtifactResult> {
    const id = artifactIdFromEntry(options.entryPath)
    const metadata = await extractArtifactMetadata(options.entryPath)
    const jsBundle = await bundleEntry(options.entryPath, options.sdkAliasPath, options.minify ?? false)
    const controlMetas = metadata.controls.map(controlToMeta)
    const presetMetas = metadata.presets.map(presetToMeta)
    const html =
        metadata.kind === 'face'
            ? faceHtml({
                  author: metadata.author,
                  controlMetas,
                  description: metadata.description,
                  jsBundle,
                  name: metadata.name,
                  presets: presetMetas,
              })
            : effectHtml({
                  audioReactive: metadata.audioReactive,
                  author: metadata.author,
                  builtinId: metadata.builtinId,
                  category: metadata.category,
                  controlMetas,
                  description: metadata.description,
                  jsBundle,
                  name: metadata.name,
                  presets: presetMetas,
                  renderer: metadata.renderer,
                  screenReactive: metadata.screenReactive,
              })

    return {
        bytes: new TextEncoder().encode(html).length,
        entryPath: options.entryPath,
        html,
        id,
        kind: metadata.kind,
        metadata,
        outputPath: join(options.outDir, `${id}.html`),
    }
}

export async function buildArtifacts(options: BuildArtifactsOptions): Promise<BuildArtifactResult[]> {
    mkdirSync(options.outDir, { recursive: true })

    const results: BuildArtifactResult[] = []
    for (const entryPath of options.entryPaths) {
        const artifact = await buildArtifactDocument({
            entryPath,
            minify: options.minify,
            outDir: options.outDir,
            sdkAliasPath: options.sdkAliasPath,
        })
        await Bun.write(artifact.outputPath, artifact.html)
        results.push(artifact)
    }

    return results
}
