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

interface FrostControls {
    speed: number
    scale: number
    edgeGlow: number
    growth: number
    palette: number
    scene: number
}

const PALETTES = ['SilkCircuit', 'Ice', 'Frost', 'Aurora', 'Cyberpunk']
const SCENES = ['Lattice', 'Shardfield', 'Prism', 'Signal']

@Effect({
    name: 'Frost Crystal',
    description: 'Sharp community-style crystal lattice with crisp geometric edge motifs',
    author: 'Hypercolor',
    audioReactive: false,
})
class FrostCrystal extends WebGLEffect<FrostControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Scale', min: 10, max: 100, default: 56, tooltip: 'Crystal lattice density' })
    scale!: number

    @NumberControl({ label: 'Edge Glow', min: 0, max: 100, default: 74, tooltip: 'Edge brightness and bloom' })
    edgeGlow!: number

    @NumberControl({ label: 'Growth', min: 0, max: 100, default: 68, tooltip: 'Geometric growth pulse amount' })
    growth!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'Ice', tooltip: 'Color palette' })
    palette!: string

    @ComboboxControl({ label: 'Scene', values: SCENES, default: 'Lattice', tooltip: 'Optional motif style' })
    scene!: string

    constructor() {
        super({ id: 'frost-crystal', name: 'Frost Crystal', fragmentShader, audioReactive: false })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.scale = getControlValue('scale', 56)
        this.edgeGlow = getControlValue('edgeGlow', 74)
        this.growth = getControlValue('growth', 68)
        this.palette = getControlValue('palette', 'Ice')
        this.scene = getControlValue('scene', 'Lattice')
    }

    protected getControlValues(): FrostControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            scale: getControlValue('scale', 56),
            edgeGlow: getControlValue('edgeGlow', 74),
            growth: getControlValue('growth', 68),
            palette: comboboxValueToIndex(getControlValue('palette', 'Ice'), PALETTES, 1),
            scene: comboboxValueToIndex(getControlValue('scene', 'Lattice'), SCENES, 0),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', normalizeSpeed(5))
        this.registerUniform('iScale', 56)
        this.registerUniform('iEdgeGlow', 74)
        this.registerUniform('iGrowth', 68)
        this.registerUniform('iPalette', 1)
        this.registerUniform('iScene', 0)
    }

    protected updateUniforms(c: FrostControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iScale', c.scale)
        this.setUniform('iEdgeGlow', c.edgeGlow)
        this.setUniform('iGrowth', c.growth)
        this.setUniform('iPalette', c.palette)
        this.setUniform('iScene', c.scene)
    }
}

const effect = new FrostCrystal()
initializeEffect(() => effect.initialize(), { instance: effect })
