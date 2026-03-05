import 'reflect-metadata'
import {
    ComboboxControl,
    Effect,
    NumberControl,
    WebGLEffect,
    comboboxValueToIndex,
    getControlValue,
    initializeEffect,
    normalizeSpeed,
} from '@hypercolor/sdk'

import fragmentShader from './fragment.glsl'

interface BubbleControls {
    speed: number
    density: number
    size: number
    drift: number
    refraction: number
    glow: number
    scene: number
    palette: number
}

const SCENES = ['Calm', 'Fizz', 'Storm']
const PALETTES = ['Ocean', 'Soda Pop', 'Neon', 'Pastel', 'Twilight']

@Effect({
    name: 'Bubble Garden',
    description: 'Layered bubble field with depth, refraction tint, and scene-based motion',
    author: 'Hypercolor',
    audioReactive: false,
})
class BubbleGarden extends WebGLEffect<BubbleControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Overall motion speed' })
    speed!: number

    @NumberControl({ label: 'Density', min: 0, max: 100, default: 58, tooltip: 'Bubble amount' })
    density!: number

    @NumberControl({ label: 'Size', min: 0, max: 100, default: 48, tooltip: 'Bubble size range' })
    size!: number

    @NumberControl({ label: 'Drift', min: 0, max: 100, default: 42, tooltip: 'Horizontal sway' })
    drift!: number

    @NumberControl({ label: 'Refraction', min: 0, max: 100, default: 60, tooltip: 'Inner bubble distortion' })
    refraction!: number

    @NumberControl({ label: 'Glow', min: 0, max: 100, default: 52, tooltip: 'Bloom and highlights' })
    glow!: number

    @ComboboxControl({ label: 'Scene', values: SCENES, default: 'Fizz', tooltip: 'Motion style' })
    scene!: string

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'Ocean', tooltip: 'Color scene' })
    palette!: string

    constructor() {
        super({ fragmentShader, audioReactive: false })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.density = getControlValue('density', 58)
        this.size = getControlValue('size', 48)
        this.drift = getControlValue('drift', 42)
        this.refraction = getControlValue('refraction', 60)
        this.glow = getControlValue('glow', 52)
        this.scene = getControlValue('scene', 'Fizz')
        this.palette = getControlValue('palette', 'Ocean')
    }

    protected getControlValues(): BubbleControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            density: getControlValue('density', 58),
            size: getControlValue('size', 48),
            drift: getControlValue('drift', 42),
            refraction: getControlValue('refraction', 60),
            glow: getControlValue('glow', 52),
            scene: comboboxValueToIndex(getControlValue('scene', 'Fizz'), SCENES, 1),
            palette: comboboxValueToIndex(getControlValue('palette', 'Ocean'), PALETTES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', normalizeSpeed(5))
        this.registerUniform('iDensity', 58)
        this.registerUniform('iSize', 48)
        this.registerUniform('iDrift', 42)
        this.registerUniform('iRefraction', 60)
        this.registerUniform('iGlow', 52)
        this.registerUniform('iScene', 1)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(c: BubbleControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iDensity', c.density)
        this.setUniform('iSize', c.size)
        this.setUniform('iDrift', c.drift)
        this.setUniform('iRefraction', c.refraction)
        this.setUniform('iGlow', c.glow)
        this.setUniform('iScene', c.scene)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new BubbleGarden()
initializeEffect(() => effect.initialize(), { instance: effect })
