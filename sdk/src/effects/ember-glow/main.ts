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

interface EmberControls {
    speed: number
    intensity: number
    emberDensity: number
    heatWarp: number
    palette: number
}

const PALETTES = ['Ember', 'Lava', 'SilkCircuit', 'Solar', 'Phosphor']

@Effect({
    name: 'Ember Glow',
    description: 'Smoldering embers with thermal convection, blackbody color, and rising sparks',
    author: 'Hypercolor',
})
class EmberGlow extends WebGLEffect<EmberControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 4, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Intensity', min: 0, max: 100, default: 70, tooltip: 'Heat intensity' })
    intensity!: number

    @NumberControl({ label: 'Embers', min: 0, max: 100, default: 55, tooltip: 'Floating ember density' })
    emberDensity!: number

    @NumberControl({ label: 'Heat Warp', min: 0, max: 100, default: 40, tooltip: 'Heat shimmer' })
    heatWarp!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'Ember',
        tooltip: 'Color palette',
    })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 4)
        this.intensity = getControlValue('intensity', 70)
        this.emberDensity = getControlValue('emberDensity', 55)
        this.heatWarp = getControlValue('heatWarp', 40)
        this.palette = getControlValue('palette', 'Ember')
    }

    protected getControlValues(): EmberControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 4)),
            intensity: getControlValue('intensity', 70),
            emberDensity: getControlValue('emberDensity', 55),
            heatWarp: getControlValue('heatWarp', 40),
            palette: PALETTES.indexOf(getControlValue('palette', 'Ember')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iIntensity', 70)
        this.registerUniform('iEmberDensity', 55)
        this.registerUniform('iHeatWarp', 40)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(controls: EmberControls): void {
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iIntensity', controls.intensity)
        this.setUniform('iEmberDensity', controls.emberDensity)
        this.setUniform('iHeatWarp', controls.heatWarp)
        this.setUniform('iPalette', controls.palette)
    }
}

const effect = new EmberGlow()
initializeEffect(() => effect.initialize(), { instance: effect })
