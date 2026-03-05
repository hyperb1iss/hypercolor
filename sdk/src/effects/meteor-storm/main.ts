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

interface MeteorControls {
    speed: number
    density: number
    trailLength: number
    glow: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Fire', 'Ice', 'Aurora', 'Cyberpunk']

@Effect({
    name: 'Meteor Storm',
    description: 'Streaking meteors with physics trails and atmospheric glow',
    author: 'Hypercolor',
})
class MeteorStorm extends WebGLEffect<MeteorControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Meteor speed' })
    speed!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 50, tooltip: 'Meteor count' })
    density!: number

    @NumberControl({ label: 'Trail', min: 10, max: 100, default: 60, tooltip: 'Trail length' })
    trailLength!: number

    @NumberControl({ label: 'Glow', min: 10, max: 100, default: 65, tooltip: 'Glow intensity' })
    glow!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'SilkCircuit', tooltip: 'Color palette' })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.density = getControlValue('density', 50)
        this.trailLength = getControlValue('trailLength', 60)
        this.glow = getControlValue('glow', 65)
        this.palette = getControlValue('palette', 'SilkCircuit')
    }

    protected getControlValues(): MeteorControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            density: getControlValue('density', 50),
            trailLength: getControlValue('trailLength', 60),
            glow: getControlValue('glow', 65),
            palette: PALETTES.indexOf(getControlValue('palette', 'SilkCircuit')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iDensity', 50)
        this.registerUniform('iTrailLength', 60)
        this.registerUniform('iGlow', 65)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(c: MeteorControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iDensity', c.density)
        this.setUniform('iTrailLength', c.trailLength)
        this.setUniform('iGlow', c.glow)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new MeteorStorm()
initializeEffect(() => effect.initialize(), { instance: effect })
