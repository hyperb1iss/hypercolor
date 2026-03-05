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

interface DeepCurrentControls {
    speed: number
    depth: number
    causticIntensity: number
    currentStrength: number
    palette: number
}

const PALETTES = ['Ocean', 'Deep Sea', 'Aurora', 'SilkCircuit', 'Midnight']

@Effect({
    name: 'Deep Current',
    description: 'Underwater ambient — layered caustics, volumetric currents, and floating particles',
    author: 'Hypercolor',
})
class DeepCurrent extends WebGLEffect<DeepCurrentControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 4, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Depth', min: 0, max: 100, default: 50, tooltip: 'Water depth' })
    depth!: number

    @NumberControl({ label: 'Caustics', min: 0, max: 100, default: 65, tooltip: 'Caustic brightness' })
    causticIntensity!: number

    @NumberControl({ label: 'Current', min: 0, max: 100, default: 45, tooltip: 'Water current strength' })
    currentStrength!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'Ocean',
        tooltip: 'Color palette',
    })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 4)
        this.depth = getControlValue('depth', 50)
        this.causticIntensity = getControlValue('causticIntensity', 65)
        this.currentStrength = getControlValue('currentStrength', 45)
        this.palette = getControlValue('palette', 'Ocean')
    }

    protected getControlValues(): DeepCurrentControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 4)),
            depth: getControlValue('depth', 50),
            causticIntensity: getControlValue('causticIntensity', 65),
            currentStrength: getControlValue('currentStrength', 45),
            palette: PALETTES.indexOf(getControlValue('palette', 'Ocean')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iDepth', 50)
        this.registerUniform('iCausticIntensity', 65)
        this.registerUniform('iCurrentStrength', 45)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(controls: DeepCurrentControls): void {
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iDepth', controls.depth)
        this.setUniform('iCausticIntensity', controls.causticIntensity)
        this.setUniform('iCurrentStrength', controls.currentStrength)
        this.setUniform('iPalette', controls.palette)
    }
}

const effect = new DeepCurrent()
initializeEffect(() => effect.initialize(), { instance: effect })
