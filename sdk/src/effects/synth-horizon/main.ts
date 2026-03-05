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

interface SynthControls {
    scene: number
    speed: number
    gridDensity: number
    glow: number
    palette: number
    colorMode: number
    cycleSpeed: number
}

const SCENES = ['Roller Grid', 'Arcade Carpet', 'Laser Lanes']
const PALETTES = ['SilkCircuit', 'Rink Pop', 'Arcade Heat', 'Ice Neon', 'Midnight']
const COLOR_MODES = ['Static', 'Color Cycle', 'Mono Neon']

@Effect({
    name: 'Synth Horizon',
    description: 'Crisp retro roller-rink geometry with arcade carpet motifs and neon horizon scenes',
    author: 'Hypercolor',
    audioReactive: false,
})
class SynthHorizon extends WebGLEffect<SynthControls> {
    @ComboboxControl({ label: 'Scene', values: SCENES, default: 'Roller Grid', tooltip: 'Retro composition style' })
    scene!: string

    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Grid', min: 10, max: 100, default: 62, tooltip: 'Grid and motif density' })
    gridDensity!: number

    @NumberControl({ label: 'Glow', min: 10, max: 100, default: 72, tooltip: 'Neon bloom intensity' })
    glow!: number

    @ComboboxControl({ label: 'Palette', values: PALETTES, default: 'Rink Pop', tooltip: 'Retro color palette' })
    palette!: string

    @ComboboxControl({
        label: 'Color Mode',
        values: COLOR_MODES,
        default: 'Color Cycle',
        tooltip: 'Static, cycling, or monochrome tinting',
    })
    colorMode!: string

    @NumberControl({ label: 'Cycle Speed', min: 0, max: 100, default: 44, tooltip: 'Color cycle speed' })
    cycleSpeed!: number

    constructor() {
        super({ fragmentShader, audioReactive: false })
    }

    protected initializeControls(): void {
        this.scene = getControlValue('scene', 'Roller Grid')
        this.speed = getControlValue('speed', 5)
        this.gridDensity = getControlValue('gridDensity', 62)
        this.glow = getControlValue('glow', 72)
        this.palette = getControlValue('palette', 'Rink Pop')
        this.colorMode = getControlValue('colorMode', 'Color Cycle')
        this.cycleSpeed = getControlValue('cycleSpeed', 44)
    }

    protected getControlValues(): SynthControls {
        return {
            scene: comboboxValueToIndex(getControlValue('scene', 'Roller Grid'), SCENES, 0),
            speed: normalizeSpeed(getControlValue('speed', 5)),
            gridDensity: getControlValue('gridDensity', 62),
            glow: getControlValue('glow', 72),
            palette: comboboxValueToIndex(getControlValue('palette', 'Rink Pop'), PALETTES, 1),
            colorMode: comboboxValueToIndex(getControlValue('colorMode', 'Color Cycle'), COLOR_MODES, 1),
            cycleSpeed: getControlValue('cycleSpeed', 44),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iGridDensity', 62)
        this.registerUniform('iGlow', 72)
        this.registerUniform('iScene', 0)
        this.registerUniform('iPalette', 1)
        this.registerUniform('iColorMode', 1)
        this.registerUniform('iCycleSpeed', 44)
    }

    protected updateUniforms(c: SynthControls): void {
        this.setUniform('iSpeed', c.speed)
        this.setUniform('iGridDensity', c.gridDensity)
        this.setUniform('iGlow', c.glow)
        this.setUniform('iScene', c.scene)
        this.setUniform('iPalette', c.palette)
        this.setUniform('iColorMode', c.colorMode)
        this.setUniform('iCycleSpeed', c.cycleSpeed)
    }
}

const effect = new SynthHorizon()
initializeEffect(() => effect.initialize(), { instance: effect })
