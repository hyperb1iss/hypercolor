import 'reflect-metadata'
import {
    ComboboxControl,
    Effect,
    NumberControl,
    WebGLEffect,
    getControlValue,
    initializeEffect,
    normalizeSpeed,
} from '@hypercolor/sdk'

import fragmentShader from './fragment.glsl'

interface ShockwaveControls {
    speed: number
    intensity: number
    ringCount: number
    decay: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Cyberpunk', 'Fire', 'Aurora', 'Ice']

@Effect({
    name: 'Bass Shockwave',
    description: 'Radial shockwave rings expanding on beat with particle bursts',
    author: 'Hypercolor',
    audioReactive: true,
})
class BassShockwave extends WebGLEffect<ShockwaveControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Intensity', min: 0, max: 100, default: 75, tooltip: 'Brightness' })
    intensity!: number

    @NumberControl({ label: 'Rings', min: 0, max: 100, default: 50, tooltip: 'Ring count' })
    ringCount!: number

    @NumberControl({ label: 'Decay', min: 0, max: 100, default: 50, tooltip: 'Ring fade speed' })
    decay!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'SilkCircuit', tooltip: 'Color palette' })
    palette!: string

    constructor() {
        super({ fragmentShader, audioReactive: true })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.intensity = getControlValue('intensity', 75)
        this.ringCount = getControlValue('ringCount', 50)
        this.decay = getControlValue('decay', 50)
        this.palette = getControlValue('palette', 'SilkCircuit')
    }

    protected getControlValues(): ShockwaveControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            intensity: getControlValue('intensity', 75),
            ringCount: getControlValue('ringCount', 50),
            decay: getControlValue('decay', 50),
            palette: PALETTES.indexOf(getControlValue('palette', 'SilkCircuit')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iIntensity', 75)
        this.registerUniform('iRingCount', 50)
        this.registerUniform('iDecay', 50)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(c: ShockwaveControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iIntensity', c.intensity)
        this.setUniform('iRingCount', c.ringCount)
        this.setUniform('iDecay', c.decay)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new BassShockwave()
initializeEffect(() => effect.initialize(), { instance: effect })
