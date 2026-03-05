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

interface CityControls {
    speed: number
    density: number
    glow: number
    rainIntensity: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Cyberpunk', 'Synthwave', 'Fire', 'Aurora']

@Effect({
    name: 'Neon City',
    description: 'Parallax cyberpunk cityscape with neon signs, rain, and wet reflections',
    author: 'Hypercolor',
})
class NeonCity extends WebGLEffect<CityControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 4, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 50, tooltip: 'Building density' })
    density!: number

    @NumberControl({ label: 'Glow', min: 10, max: 100, default: 70, tooltip: 'Neon glow intensity' })
    glow!: number

    @NumberControl({ label: 'Rain', min: 0, max: 100, default: 40, tooltip: 'Rain intensity' })
    rainIntensity!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'Cyberpunk', tooltip: 'Color palette' })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 4)
        this.density = getControlValue('density', 50)
        this.glow = getControlValue('glow', 70)
        this.rainIntensity = getControlValue('rainIntensity', 40)
        this.palette = getControlValue('palette', 'Cyberpunk')
    }

    protected getControlValues(): CityControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 4)),
            density: getControlValue('density', 50),
            glow: getControlValue('glow', 70),
            rainIntensity: getControlValue('rainIntensity', 40),
            palette: PALETTES.indexOf(getControlValue('palette', 'Cyberpunk')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iDensity', 50)
        this.registerUniform('iGlow', 70)
        this.registerUniform('iRainIntensity', 40)
        this.registerUniform('iPalette', 1)
    }

    protected updateUniforms(c: CityControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iDensity', c.density)
        this.setUniform('iGlow', c.glow)
        this.setUniform('iRainIntensity', c.rainIntensity)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new NeonCity()
initializeEffect(() => effect.initialize(), { instance: effect })
