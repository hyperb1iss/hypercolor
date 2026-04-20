import { resolve } from 'node:path'

import type { ArtifactKind, BuildControlDef, ExtractedArtifactMetadata, RegisteredArtifactDef } from './types'

const BUILTIN_UNIFORMS = new Set(['iTime', 'iResolution', 'iMouse'])
const AUDIO_USAGE_PATTERNS = [/\baudio\s*\(/, /\bctx\.audio\b/, /\bgetAudioData\s*\(/, /\bengine\.audio\b/]

function toBuildControls(def: RegisteredArtifactDef): BuildControlDef[] {
    return def.resolvedControls.map((ctrl) => {
        const buildControl: BuildControlDef = {
            default: ctrl.spec.defaultValue,
            group: ctrl.spec.group,
            id: ctrl.key,
            label: ctrl.spec.label,
            tooltip: ctrl.spec.tooltip,
            type: ctrl.spec.__type === 'textfield' ? 'textfield' : ctrl.spec.__type,
        }

        if (ctrl.spec.meta.min != null) buildControl.min = ctrl.spec.meta.min as number
        if (ctrl.spec.meta.max != null) buildControl.max = ctrl.spec.meta.max as number
        if (ctrl.spec.meta.step != null) buildControl.step = ctrl.spec.meta.step as number
        if (ctrl.spec.meta.values) buildControl.values = ctrl.spec.meta.values as string[]
        if (ctrl.spec.meta.aspectLock != null) buildControl.aspectLock = ctrl.spec.meta.aspectLock as number
        if (ctrl.spec.meta.preview) buildControl.preview = ctrl.spec.meta.preview as 'screen' | 'web' | 'canvas'

        return buildControl
    })
}

function extractShaderUniforms(shader: string): Set<string> {
    const uniforms = new Set<string>()

    for (const match of shader.matchAll(/uniform\s+\w+\s+(i\w+)\s*;/g)) {
        const [, uniform] = match
        if (uniform) uniforms.add(uniform)
    }

    return uniforms
}

function validateShaderBindings(entryPath: string, def: RegisteredArtifactDef): void {
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

    if (missing.length > 0) {
        throw new Error(
            `Shader binding validation failed for ${entryPath}: missing control uniforms ${missing.join(', ')}`,
        )
    }

    if (extra.length > 0) {
        console.warn(`Warning: ${entryPath} shader exposes uniforms with no controls: ${extra.join(', ')}`)
    }
}

async function validateExplicitReactivityOptIns(entryPath: string, def: RegisteredArtifactDef): Promise<void> {
    const source = await Bun.file(entryPath).text()

    if (def.audio !== true && AUDIO_USAGE_PATTERNS.some((pattern) => pattern.test(source))) {
        throw new Error(
            `Audio reactivity validation failed for ${entryPath}: effect uses audio helpers but is missing audio: true`,
        )
    }
}

function runtimeGlobals() {
    return globalThis as Record<string, unknown>
}

function kindFromType(type: RegisteredArtifactDef['type']): ArtifactKind {
    return type === 'face' ? 'face' : 'effect'
}

async function loadMetadata(entryPath: string): Promise<ExtractedArtifactMetadata> {
    const entryUrl = Bun.pathToFileURL(resolve(entryPath)).href
    const g = runtimeGlobals()
    const originalWindow = g.window
    const originalDocument = g.document

    g.__HYPERCOLOR_METADATA_ONLY__ = true
    g.window = g

    if (!g.document) {
        g.document = {
            addEventListener: () => {},
            getElementById: () => null,
            readyState: 'complete',
        }
    }

    try {
        delete g.__hypercolorEffectDefs__
        delete g.__hypercolorEffectInstance__

        await import(entryUrl)

        const defs = g.__hypercolorEffectDefs__ as RegisteredArtifactDef[] | undefined
        if (!defs?.length) {
            throw new Error(`Metadata extraction failed for ${entryPath}: no effect definitions were registered`)
        }

        const def = defs.at(-1)
        if (!def) {
            throw new Error(`Metadata extraction failed for ${entryPath}: missing final effect definition`)
        }

        validateShaderBindings(entryPath, def)
        await validateExplicitReactivityOptIns(entryPath, def)

        return {
            audioReactive: def.audio ?? false,
            author: def.author ?? 'Hypercolor',
            builtinId: def.builtinId,
            category: def.category,
            controls: toBuildControls(def),
            description: def.description ?? '',
            kind: kindFromType(def.type),
            name: def.name,
            presets: def.presets ?? [],
            renderer: def.type === 'canvas' ? 'canvas2d' : def.type === 'webgl' ? 'webgl' : undefined,
            screenReactive: def.screen ?? false,
        }
    } finally {
        delete g.__HYPERCOLOR_METADATA_ONLY__
        delete g.__hypercolorEffectDefs__
        delete g.__hypercolorEffectInstance__

        if (originalWindow === undefined) {
            delete g.window
        } else {
            g.window = originalWindow
        }

        if (originalDocument === undefined) {
            delete g.document
        } else {
            g.document = originalDocument
        }
    }
}

self.onmessage = async (event: MessageEvent<{ entryPath: string }>) => {
    try {
        postMessage({ metadata: await loadMetadata(event.data.entryPath) })
    } catch (error) {
        postMessage({
            error: error instanceof Error ? error.message : String(error),
        })
    }
}
