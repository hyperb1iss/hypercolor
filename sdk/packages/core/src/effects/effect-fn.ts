/**
 * effect() — declarative shader effect API.
 *
 * Replaces the 5-method decorator pattern with a single function call.
 * Generates a WebGLEffect subclass from a shader + controls map.
 *
 * @example
 * ```typescript
 * import { effect } from '@hypercolor/sdk'
 * import shader from './fragment.glsl'
 *
 * export default effect('Meteor Storm', shader, {
 *     speed:       [1, 10, 5],
 *     density:     [10, 100, 50],
 *     trailLength: [10, 100, 60],
 *     palette:     ['SilkCircuit', 'Fire', 'Ice', 'Aurora', 'Cyberpunk'],
 * })
 * ```
 */

import type { AudioData } from '../audio'
import { getAudioData } from '../audio'
import type { ControlMap } from '../controls/infer'
import { inferControl } from '../controls/infer'
import { deriveLabel, hasMagicTransform, resolveControlNames } from '../controls/names'
import type { ControlSpec } from '../controls/specs'
import { isControlSpec } from '../controls/specs'
import { comboboxValueToIndex, getControlValue, normalizeSpeed } from '../controls/helpers'
import { initializeEffect } from '../init'
import type { UniformValue } from './webgl-effect'
import { WebGLEffect } from './webgl-effect'

// ── Types ────────────────────────────────────────────────────────────────

export interface ShaderContext {
    readonly controls: Record<string, unknown>
    readonly audio: AudioData | null
    readonly gl: WebGL2RenderingContext
    readonly program: WebGLProgram
    readonly width: number
    readonly height: number
    registerUniform(name: string, value: UniformValue): void
    setUniform(name: string, value: UniformValue): void
}

export interface EffectFnOptions {
    description?: string
    author?: string
    audio?: boolean
    vertexShader?: string
    setup?: (ctx: ShaderContext) => void | Promise<void>
    frame?: (ctx: ShaderContext, time: number) => void
}

/** Resolved control entry used at runtime. */
interface ResolvedControl {
    key: string
    spec: ControlSpec
    uniformName: string
    normalize: 'speed' | 'percentage' | 'none'
    isMagicTransform: boolean
    values?: string[]
}

// ── Control Resolution ───────────────────────────────────────────────────

function resolveControls(controls: ControlMap): ResolvedControl[] {
    const resolved: ResolvedControl[] = []

    for (const [key, value] of Object.entries(controls)) {
        const spec = isControlSpec(value)
            ? value
            : inferControl(key, value, deriveLabel(key))

        const names = resolveControlNames(key, spec)
        const values = spec.meta.values as string[] | undefined

        resolved.push({
            key,
            spec,
            uniformName: names.uniformName,
            normalize: names.normalize,
            isMagicTransform: hasMagicTransform(key) && spec.__type === 'combobox',
            values,
        })
    }

    return resolved
}

// ── Generated Effect Class ───────────────────────────────────────────────

class GeneratedWebGLEffect extends WebGLEffect<Record<string, unknown>> {
    private resolvedControls: ResolvedControl[]
    private options: EffectFnOptions
    private shaderCtx: ShaderContext | null = null

    constructor(
        name: string,
        shader: string,
        resolvedControls: ResolvedControl[],
        options: EffectFnOptions,
    ) {
        super({
            id: name.toLowerCase().replace(/\s+/g, '-'),
            name,
            fragmentShader: shader,
            vertexShader: options.vertexShader,
            audioReactive: options.audio ?? false,
        })
        this.resolvedControls = resolvedControls
        this.options = options
    }

    protected initializeControls(): void {
        for (const ctrl of this.resolvedControls) {
            // Read initial value from window
            ;(this as unknown as Record<string, unknown>)[ctrl.key] = getControlValue(
                ctrl.key,
                ctrl.spec.defaultValue,
            )
        }
    }

    protected getControlValues(): Record<string, unknown> {
        const values: Record<string, unknown> = {}
        for (const ctrl of this.resolvedControls) {
            let val = getControlValue(ctrl.key, ctrl.spec.defaultValue)

            // Apply magic normalization
            if (ctrl.normalize === 'speed' && typeof val === 'number') {
                val = normalizeSpeed(val)
            }

            // Apply magic combobox → index transform
            if (ctrl.isMagicTransform && ctrl.values) {
                val = comboboxValueToIndex(
                    val as string | number,
                    ctrl.values,
                    0,
                )
            }

            values[ctrl.key] = val
        }
        return values
    }

    protected createUniforms(): void {
        for (const ctrl of this.resolvedControls) {
            const initial = this.resolveInitialUniformValue(ctrl)
            this.registerUniform(ctrl.uniformName, initial)
        }
    }

    protected async initializeRenderer(): Promise<void> {
        await super.initializeRenderer()

        // Run user's setup hook if provided
        if (this.options.setup && this.gl && this.program) {
            this.shaderCtx = this.createShaderContext()
            await this.options.setup(this.shaderCtx)
        }
    }

    protected updateUniforms(controls: Record<string, unknown>): void {
        for (const ctrl of this.resolvedControls) {
            const val = controls[ctrl.key]
            if (val !== undefined) {
                this.setUniform(ctrl.uniformName, val as UniformValue)
            }
        }
    }

    protected render(time: number): void {
        // Run user's frame hook before the draw call
        if (this.options.frame && this.gl && this.program) {
            if (!this.shaderCtx) this.shaderCtx = this.createShaderContext()
            this.options.frame(this.shaderCtx, time)
        }
        super.render(time)
    }

    private resolveInitialUniformValue(ctrl: ResolvedControl): UniformValue {
        const val = ctrl.spec.defaultValue
        if (ctrl.isMagicTransform && ctrl.values) {
            return comboboxValueToIndex(val as string | number, ctrl.values, 0)
        }
        if (ctrl.normalize === 'speed' && typeof val === 'number') {
            return normalizeSpeed(val)
        }
        if (typeof val === 'boolean') {
            return val ? 1 : 0
        }
        if (typeof val === 'string' && val.startsWith('#')) {
            return hexToFloats(val)
        }
        if (typeof val === 'number') {
            return val
        }
        // Combobox without magic transform: return index 0
        if (ctrl.spec.__type === 'combobox' && ctrl.values) {
            return comboboxValueToIndex(val as string | number, ctrl.values, 0)
        }
        return 0
    }

    private createShaderContext(): ShaderContext {
        const self = this
        return {
            get controls() { return self.getControlValues() },
            get audio() { return getAudioData() },
            gl: self.gl!,
            program: self.program!,
            get width() { return self.canvas?.width ?? 320 },
            get height() { return self.canvas?.height ?? 200 },
            registerUniform: (name, value) => self.registerUniform(name, value),
            setUniform: (name, value) => self.setUniform(name, value),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

function hexToFloats(hex: string): number[] {
    const h = hex.replace('#', '')
    return [
        Number.parseInt(h.slice(0, 2), 16) / 255,
        Number.parseInt(h.slice(2, 4), 16) / 255,
        Number.parseInt(h.slice(4, 6), 16) / 255,
    ]
}

// ── Metadata Extraction Support ──────────────────────────────────────────

interface EffectDef {
    name: string
    controls: ControlMap
    resolvedControls: ResolvedControl[]
    description?: string
    author?: string
    audio?: boolean
}

function storeMetadata(def: EffectDef): void {
    const g = globalThis as Record<string, unknown>
    const defs = (g.__hypercolorEffectDefs__ as EffectDef[]) ?? []
    defs.push(def)
    g.__hypercolorEffectDefs__ = defs
}

// ── Public API ───────────────────────────────────────────────────────────

/**
 * Define a shader effect with a single function call.
 *
 * @param name - Display name of the effect
 * @param shader - Fragment shader GLSL source
 * @param controls - Control map (shorthand or explicit factories)
 * @param options - Optional: description, author, audio, setup/frame hooks
 */
export function effect(
    name: string,
    shader: string,
    controls: ControlMap,
    options?: EffectFnOptions,
): void {
    const resolved = resolveControls(controls)
    const opts = options ?? {}

    // Metadata-only mode: store definition and bail
    if (
        typeof globalThis !== 'undefined' &&
        (globalThis as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__
    ) {
        storeMetadata({
            name,
            controls,
            resolvedControls: resolved,
            description: opts.description,
            author: opts.author,
            audio: opts.audio,
        })
        return
    }

    // Runtime mode: create and initialize the effect
    const fx = new GeneratedWebGLEffect(name, shader, resolved, opts)
    initializeEffect(() => fx.initialize(), { instance: fx })
}
