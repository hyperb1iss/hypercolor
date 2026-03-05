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

interface NebulaControls {
    speed: number
    density: number
    warp: number
    brightness: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Cosmic', 'Fire', 'Aurora', 'Ice']

@Effect({
    name: 'Nebula Drift',
    description: 'Domain-warped nebula clouds with layered fBm and twinkling starfield',
    author: 'Hypercolor',
})
class NebulaDrift extends WebGLEffect<NebulaControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 4, tooltip: 'Drift speed' })
    speed!: number

    @NumberControl({ label: 'Density', min: 10, max: 100, default: 50, tooltip: 'Cloud density' })
    density!: number

    @NumberControl({ label: 'Warp', min: 0, max: 100, default: 60, tooltip: 'Domain warp intensity' })
    warp!: number

    @NumberControl({ label: 'Brightness', min: 10, max: 100, default: 65, tooltip: 'Nebula brightness' })
    brightness!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'SilkCircuit', tooltip: 'Color palette' })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 4)
        this.density = getControlValue('density', 50)
        this.warp = getControlValue('warp', 60)
        this.brightness = getControlValue('brightness', 65)
        this.palette = getControlValue('palette', 'SilkCircuit')
    }

    protected getControlValues(): NebulaControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 4)),
            density: getControlValue('density', 50),
            warp: getControlValue('warp', 60),
            brightness: getControlValue('brightness', 65),
            palette: PALETTES.indexOf(getControlValue('palette', 'SilkCircuit')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iDensity', 50)
        this.registerUniform('iWarp', 60)
        this.registerUniform('iBrightness', 65)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(c: NebulaControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iDensity', c.density)
        this.setUniform('iWarp', c.warp)
        this.setUniform('iBrightness', c.brightness)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new NebulaDrift()
initializeEffect(() => effect.initialize(), { instance: effect })
