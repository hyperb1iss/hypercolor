/**
 * WebGLEffect — base class for WebGL2 shader effects.
 *
 * Uses raw WebGL2 (no Three.js). Renders a fullscreen quad with
 * custom fragment shaders. Supports audio-reactive uniforms.
 */

import { AudioData, getAudioData } from '../audio'
import { BaseEffect, EffectConfig } from './base-effect'

export interface WebGLEffectConfig extends EffectConfig {
    fragmentShader: string
    vertexShader?: string
    audioReactive?: boolean
}

/** Uniform value types supported by the WebGL effect. */
export type UniformValue = number | number[] | Float32Array

/** Uniform descriptor with location cache. */
interface UniformEntry {
    value: UniformValue
    location: WebGLUniformLocation | null
}

// Default fullscreen quad vertex shader
const DEFAULT_VERTEX_SHADER = `#version 300 es
precision highp float;
in vec2 aPosition;
void main() {
    gl_Position = vec4(aPosition, 0.0, 1.0);
}
`

export abstract class WebGLEffect<T> extends BaseEffect<T> {
    protected gl: WebGL2RenderingContext | null = null
    protected program: WebGLProgram | null = null
    protected uniforms: Map<string, UniformEntry> = new Map()
    protected fragmentShader: string
    protected vertexShader: string
    protected audioReactive: boolean
    protected currentAudioData: AudioData | null = null

    constructor(config: WebGLEffectConfig) {
        super(config)
        this.fragmentShader = config.fragmentShader
        this.vertexShader = config.vertexShader ?? DEFAULT_VERTEX_SHADER
        this.audioReactive = config.audioReactive ?? false
    }

    protected async initializeRenderer(): Promise<void> {
        if (!this.canvas) throw new Error('Canvas not available')

        this.gl = this.canvas.getContext('webgl2', { preserveDrawingBuffer: true })
        if (!this.gl) throw new Error('WebGL2 not supported')

        // Compile shaders and link program
        this.program = this.createProgram(this.vertexShader, this.fragmentShader)
        this.gl.useProgram(this.program)

        // Create fullscreen quad geometry
        this.createQuad()

        // Register standard uniforms
        this.registerUniform('iTime', 0)
        this.registerUniform('iResolution', [this.canvas.width, this.canvas.height])
        this.registerUniform('iMouse', [0, 0])

        // Register audio uniforms if reactive
        if (this.audioReactive) {
            this.registerAudioUniforms()
        }

        // Let subclass register custom uniforms
        this.createUniforms()

        // Resolve all uniform locations
        this.resolveLocations()

        this.debug('success', 'WebGL2 renderer initialized')
    }

    protected render(time: number): void {
        if (!this.gl || !this.program) return

        this.setUniform('iTime', time)

        if (this.audioReactive) {
            this.currentAudioData = getAudioData()
            this.pushAudioUniforms(this.currentAudioData)
        }

        this.gl.drawArrays(this.gl.TRIANGLE_STRIP, 0, 4)
    }

    protected getAudio(): AudioData | null {
        return this.currentAudioData
    }

    protected updateParameters(controls: T): void {
        if (!this.program) return
        this.updateUniforms(controls)
    }

    // ── Uniform API ─────────────────────────────────────────────────────

    /** Register a uniform. Call in createUniforms(). */
    protected registerUniform(name: string, value: UniformValue): void {
        this.uniforms.set(name, { location: null, value })
    }

    /** Set a uniform value (will be pushed on next draw). */
    protected setUniform(name: string, value: UniformValue): void {
        const entry = this.uniforms.get(name)
        if (entry) {
            entry.value = value
            this.pushUniform(name, entry)
        }
    }

    // ── Internal ────────────────────────────────────────────────────────

    private createProgram(vertSrc: string, fragSrc: string): WebGLProgram {
        const gl = this.gl!
        const vert = this.compileShader(gl.VERTEX_SHADER, vertSrc)
        const frag = this.compileShader(gl.FRAGMENT_SHADER, fragSrc)

        const program = gl.createProgram()!
        gl.attachShader(program, vert)
        gl.attachShader(program, frag)
        gl.linkProgram(program)

        if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
            const info = gl.getProgramInfoLog(program)
            gl.deleteProgram(program)
            throw new Error(`Shader link failed: ${info}`)
        }

        gl.deleteShader(vert)
        gl.deleteShader(frag)
        return program
    }

    private compileShader(type: number, source: string): WebGLShader {
        const gl = this.gl!
        const shader = gl.createShader(type)!
        gl.shaderSource(shader, source)
        gl.compileShader(shader)

        if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
            const info = gl.getShaderInfoLog(shader)
            gl.deleteShader(shader)
            throw new Error(`Shader compile failed: ${info}`)
        }

        return shader
    }

    private createQuad(): void {
        const gl = this.gl!
        const vertices = new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1])
        const buffer = gl.createBuffer()
        gl.bindBuffer(gl.ARRAY_BUFFER, buffer)
        gl.bufferData(gl.ARRAY_BUFFER, vertices, gl.STATIC_DRAW)

        const posLoc = gl.getAttribLocation(this.program!, 'aPosition')
        gl.enableVertexAttribArray(posLoc)
        gl.vertexAttribPointer(posLoc, 2, gl.FLOAT, false, 0, 0)
    }

    private resolveLocations(): void {
        const gl = this.gl!
        for (const [name, entry] of this.uniforms) {
            entry.location = gl.getUniformLocation(this.program!, name)
        }
    }

    private pushUniform(_name: string, entry: UniformEntry): void {
        const gl = this.gl
        if (!gl || !entry.location) return

        const val = entry.value
        if (typeof val === 'number') {
            gl.uniform1f(entry.location, val)
        } else if (val.length === 2) {
            gl.uniform2fv(entry.location, val)
        } else if (val.length === 3) {
            gl.uniform3fv(entry.location, val)
        } else if (val.length === 4) {
            gl.uniform4fv(entry.location, val)
        }
    }

    private registerAudioUniforms(): void {
        this.registerUniform('iAudioLevel', 0)
        this.registerUniform('iAudioBass', 0)
        this.registerUniform('iAudioMid', 0)
        this.registerUniform('iAudioTreble', 0)
        this.registerUniform('iAudioBeat', 0)
        this.registerUniform('iAudioBeatPulse', 0)
        this.registerUniform('iAudioBeatPhase', 0)
        this.registerUniform('iAudioBeatConfidence', 0)
        this.registerUniform('iAudioOnset', 0)
        this.registerUniform('iAudioOnsetPulse', 0)
        this.registerUniform('iAudioSpectralFlux', 0)
        this.registerUniform('iAudioHarmonicHue', 0)
        this.registerUniform('iAudioChordMood', 0)
        this.registerUniform('iAudioBrightness', 0.5)
        this.registerUniform('iAudioMomentum', 0)
        this.registerUniform('iAudioSwell', 0)
        this.registerUniform('iAudioTempo', 120)
        this.registerUniform('iAudioFluxBands', [0, 0, 0])
    }

    private pushAudioUniforms(audio: AudioData): void {
        this.setUniform('iAudioLevel', audio.level)
        this.setUniform('iAudioBass', audio.bass)
        this.setUniform('iAudioMid', audio.mid)
        this.setUniform('iAudioTreble', audio.treble)
        this.setUniform('iAudioBeat', audio.beat)
        this.setUniform('iAudioBeatPulse', audio.beatPulse)
        this.setUniform('iAudioBeatPhase', audio.beatPhase)
        this.setUniform('iAudioBeatConfidence', audio.beatConfidence)
        this.setUniform('iAudioOnset', audio.onset)
        this.setUniform('iAudioOnsetPulse', audio.onsetPulse)
        this.setUniform('iAudioSpectralFlux', audio.spectralFlux)
        this.setUniform('iAudioHarmonicHue', audio.harmonicHue)
        this.setUniform('iAudioChordMood', audio.chordMood)
        this.setUniform('iAudioBrightness', audio.brightness)
        this.setUniform('iAudioMomentum', audio.momentum)
        this.setUniform('iAudioSwell', audio.swell)
        this.setUniform('iAudioTempo', audio.tempo)
        this.setUniform('iAudioFluxBands', Array.from(audio.spectralFluxBands))
    }

    /** Register custom uniforms. Called during initialization. */
    protected abstract createUniforms(): void

    /** Update shader uniforms with current control values. */
    protected abstract updateUniforms(controls: T): void
}
