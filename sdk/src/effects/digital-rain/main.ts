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

interface DigitalRainControls {
    speed: number
    density: number
    trailLength: number
    charSize: number
    palette: number
}

const PALETTES = ['Matrix', 'Phosphor', 'SilkCircuit', 'Cyberpunk', 'Ice']

@Effect({
    name: 'Digital Rain',
    description: 'Cascading code rain with depth layers and procedural glyphs',
    author: 'Hypercolor',
})
class DigitalRain extends WebGLEffect<DigitalRainControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Rain speed' })
    speed!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 60, tooltip: 'Column density' })
    density!: number

    @NumberControl({ label: 'Trail', min: 5, max: 100, default: 50, tooltip: 'Trail length' })
    trailLength!: number

    @NumberControl({ label: 'Size', min: 0, max: 100, default: 40, tooltip: 'Character size' })
    charSize!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'Matrix',
        tooltip: 'Color palette',
    })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.density = getControlValue('density', 60)
        this.trailLength = getControlValue('trailLength', 50)
        this.charSize = getControlValue('charSize', 40)
        this.palette = getControlValue('palette', 'Matrix')
    }

    protected getControlValues(): DigitalRainControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            density: getControlValue('density', 60),
            trailLength: getControlValue('trailLength', 50),
            charSize: getControlValue('charSize', 40),
            palette: PALETTES.indexOf(getControlValue('palette', 'Matrix')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iDensity', 60)
        this.registerUniform('iTrailLength', 50)
        this.registerUniform('iCharSize', 40)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(controls: DigitalRainControls): void {
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iDensity', controls.density)
        this.setUniform('iTrailLength', controls.trailLength)
        this.setUniform('iCharSize', controls.charSize)
        this.setUniform('iPalette', controls.palette)
    }
}

const effect = new DigitalRain()
initializeEffect(() => effect.initialize(), { instance: effect })
