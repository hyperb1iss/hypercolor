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

interface FireControls {
    speed: number
    flameHeight: number
    turbulence: number
    intensity: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Fire', 'Ice', 'Aurora', 'Cyberpunk']

@Effect({
    name: 'Spectral Fire',
    description: 'Audio-reactive flames with frequency-band height mapping and blackbody color',
    author: 'Hypercolor',
    audioReactive: true,
})
class SpectralFire extends WebGLEffect<FireControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Flame animation speed' })
    speed!: number

    @NumberControl({ label: 'Height', min: 10, max: 100, default: 60, tooltip: 'Base flame height' })
    flameHeight!: number

    @NumberControl({ label: 'Turbulence', min: 10, max: 100, default: 50, tooltip: 'Flame turbulence' })
    turbulence!: number

    @NumberControl({ label: 'Intensity', min: 10, max: 100, default: 70, tooltip: 'Brightness' })
    intensity!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'Fire', tooltip: 'Color palette' })
    palette!: string

    constructor() {
        super({ fragmentShader, audioReactive: true })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.flameHeight = getControlValue('flameHeight', 60)
        this.turbulence = getControlValue('turbulence', 50)
        this.intensity = getControlValue('intensity', 70)
        this.palette = getControlValue('palette', 'Fire')
    }

    protected getControlValues(): FireControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            flameHeight: getControlValue('flameHeight', 60),
            turbulence: getControlValue('turbulence', 50),
            intensity: getControlValue('intensity', 70),
            palette: PALETTES.indexOf(getControlValue('palette', 'Fire')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iFlameHeight', 60)
        this.registerUniform('iTurbulence', 50)
        this.registerUniform('iIntensity', 70)
        this.registerUniform('iPalette', 1)
    }

    protected updateUniforms(c: FireControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iFlameHeight', c.flameHeight)
        this.setUniform('iTurbulence', c.turbulence)
        this.setUniform('iIntensity', c.intensity)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new SpectralFire()
initializeEffect(() => effect.initialize(), { instance: effect })
