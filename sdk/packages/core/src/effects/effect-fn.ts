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
import { comboboxValueToIndex, getControlValue, normalizePercentage, normalizeSpeed } from '../controls/helpers'
import type { ControlMap } from '../controls/infer'
import { inferControl } from '../controls/infer'
import { deriveLabel, hasMagicTransform, resolveControlNames } from '../controls/names'
import type { ControlSpec } from '../controls/specs'
import { isControlSpec } from '../controls/specs'
import { initializeEffect } from '../init'
import { DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH } from './base-effect'
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

/** A named preset with control overrides, defined by the effect author. */
export interface PresetDef {
    name: string
    description?: string
    controls: Record<string, unknown>
}

export interface EffectFnOptions {
    description?: string
    author?: string
    audio?: boolean
    /** Effect-defined presets — named control snapshots bundled with the effect. */
    presets?: PresetDef[]
    vertexShader?: string
    preserveDrawingBuffer?: boolean
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
        const spec = isControlSpec(value) ? value : inferControl(key, value, deriveLabel(key))

        const names = resolveControlNames(key, spec)
        const values = spec.meta.values as string[] | undefined

        resolved.push({
            isMagicTransform: hasMagicTransform(key) && spec.__type === 'combobox',
            key,
            normalize: names.normalize,
            spec,
            uniformName: names.uniformName,
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

    constructor(name: string, shader: string, resolvedControls: ResolvedControl[], options: EffectFnOptions) {
        super({
            audioReactive: options.audio ?? false,
            fragmentShader: shader,
            id: name.toLowerCase().replace(/\s+/g, '-'),
            name,
            preserveDrawingBuffer: options.preserveDrawingBuffer,
            vertexShader: options.vertexShader,
        })
        this.resolvedControls = resolvedControls
        this.options = options
    }

    protected initializeControls(): void {
        for (const ctrl of this.resolvedControls) {
            // Read initial value from window
            ;(this as unknown as Record<string, unknown>)[ctrl.key] = getControlValue(ctrl.key, ctrl.spec.defaultValue)
        }
    }

    protected getControlValues(): Record<string, unknown> {
        const values: Record<string, unknown> = {}
        for (const ctrl of this.resolvedControls) {
            let val = getControlValue(ctrl.key, ctrl.spec.defaultValue)

            // Apply magic normalization
            if (typeof val === 'number') {
                if (ctrl.normalize === 'speed') {
                    val = normalizeSpeed(val)
                } else if (ctrl.normalize === 'percentage') {
                    val = normalizePercentage(val)
                }
            }

            // Apply magic combobox → index transform
            if (ctrl.isMagicTransform && ctrl.values) {
                val = comboboxValueToIndex(val as string | number, ctrl.values, 0)
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

            // Resolve locations for any uniforms registered in setup()
            // (super already resolved its own; re-resolving is idempotent)
            this.resolveNewUniforms()
        }
    }

    protected updateUniforms(controls: Record<string, unknown>): void {
        for (const ctrl of this.resolvedControls) {
            const raw = controls[ctrl.key]
            if (raw !== undefined) {
                this.setUniform(ctrl.uniformName, this.toUniformValue(ctrl, raw, true))
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

    /** Re-resolve uniform locations for any uniforms registered after init. */
    private resolveNewUniforms(): void {
        this.resolveLocations()
        this.pushAllUniforms()
    }

    /** Convert a raw control value to a GPU-compatible uniform value. */
    private toUniformValue(ctrl: ResolvedControl, raw: unknown, skipNormalization = false): UniformValue {
        // Combobox → integer index (palette magic or any combobox)
        if (ctrl.spec.__type === 'combobox' && ctrl.values) {
            return comboboxValueToIndex(raw as string | number, ctrl.values, 0)
        }
        // Speed normalization
        if (!skipNormalization && typeof raw === 'number') {
            if (ctrl.normalize === 'speed') return normalizeSpeed(raw)
            if (ctrl.normalize === 'percentage') return normalizePercentage(raw)
        }
        // Boolean → 0/1
        if (typeof raw === 'boolean') {
            return raw ? 1 : 0
        }
        // Color hex → vec3 floats
        if (typeof raw === 'string' && raw.startsWith('#')) {
            return hexToFloats(raw)
        }
        if (typeof raw === 'number') {
            return raw
        }
        return 0
    }

    private resolveInitialUniformValue(ctrl: ResolvedControl): UniformValue {
        return this.toUniformValue(ctrl, ctrl.spec.defaultValue)
    }

    private createShaderContext(): ShaderContext {
        const gl = this.gl
        const program = this.program
        if (!gl || !program) throw new Error('GL context not initialized')
        const self = this
        return {
            get audio() {
                return getAudioData()
            },
            get controls() {
                return self.getControlValues()
            },
            gl,
            get height() {
                return self.canvas?.height ?? DEFAULT_CANVAS_HEIGHT
            },
            program,
            registerUniform: (name, value) => self.registerUniform(name, value),
            setUniform: (name, value) => self.setUniform(name, value),
            get width() {
                return self.canvas?.width ?? DEFAULT_CANVAS_WIDTH
            },
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
    shader: string
    controls: ControlMap
    resolvedControls: ResolvedControl[]
    description?: string
    author?: string
    audio?: boolean
    presets?: PresetDef[]
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
export function effect(name: string, shader: string, controls: ControlMap, options?: EffectFnOptions): void {
    const resolved = resolveControls(controls)
    const opts = options ?? {}

    // Metadata-only mode: store definition and bail
    if (typeof globalThis !== 'undefined' && (globalThis as Record<string, unknown>).__HYPERCOLOR_METADATA_ONLY__) {
        storeMetadata({
            audio: opts.audio,
            author: opts.author,
            controls,
            description: opts.description,
            name,
            presets: opts.presets,
            resolvedControls: resolved,
            shader,
        })
        return
    }

    // Runtime mode: create and initialize the effect
    const fx = new GeneratedWebGLEffect(name, shader, resolved, opts)
    initializeEffect(() => fx.initialize(), { instance: fx })
}
