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

interface FrostControls {
    speed: number
    scale: number
    edgeGlow: number
    growth: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Ice', 'Frost', 'Aurora', 'Cyberpunk']

@Effect({
    name: 'Frost Crystal',
    description: 'Worley-noise crystalline structures with glowing edges and growth animation',
    author: 'Hypercolor',
})
class FrostCrystal extends WebGLEffect<FrostControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 4, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Scale', min: 10, max: 100, default: 50, tooltip: 'Crystal size' })
    scale!: number

    @NumberControl({ label: 'Edge Glow', min: 10, max: 100, default: 70, tooltip: 'Edge brightness' })
    edgeGlow!: number

    @NumberControl({ label: 'Growth', min: 10, max: 100, default: 80, tooltip: 'Growth radius' })
    growth!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'Ice', tooltip: 'Color palette' })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 4)
        this.scale = getControlValue('scale', 50)
        this.edgeGlow = getControlValue('edgeGlow', 70)
        this.growth = getControlValue('growth', 80)
        this.palette = getControlValue('palette', 'Ice')
    }

    protected getControlValues(): FrostControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 4)),
            scale: getControlValue('scale', 50),
            edgeGlow: getControlValue('edgeGlow', 70),
            growth: getControlValue('growth', 80),
            palette: PALETTES.indexOf(getControlValue('palette', 'Ice')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iScale', 50)
        this.registerUniform('iEdgeGlow', 70)
        this.registerUniform('iGrowth', 80)
        this.registerUniform('iPalette', 1)
    }

    protected updateUniforms(c: FrostControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iScale', c.scale)
        this.setUniform('iEdgeGlow', c.edgeGlow)
        this.setUniform('iGrowth', c.growth)
        this.setUniform('iPalette', c.palette)
    }
}

const effect = new FrostCrystal()
initializeEffect(() => effect.initialize(), { instance: effect })
