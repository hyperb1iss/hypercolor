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

interface VoronoiControls {
    speed: number
    scale: number
    edgeGlow: number
    colorShift: number
    palette: number
    distanceMode: number
}

const PALETTES = ['SilkCircuit', 'Aurora', 'Cyberpunk', 'Sunset', 'Fire', 'Vaporwave']
const DISTANCE_MODES = ['Euclidean', 'Manhattan', 'Chebyshev']

@Effect({
    name: 'Voronoi Glass',
    description: 'Stained glass Voronoi cells with F2-F1 edge glow and caustic shimmer',
    author: 'Hypercolor',
})
class VoronoiGlass extends WebGLEffect<VoronoiControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 4, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Scale', min: 0, max: 100, default: 50, tooltip: 'Cell density' })
    scale!: number

    @NumberControl({ label: 'Edge Glow', min: 0, max: 100, default: 60, tooltip: 'Edge brightness' })
    edgeGlow!: number

    @NumberControl({ label: 'Color Shift', min: 0, max: 100, default: 30, tooltip: 'Color animation speed' })
    colorShift!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'SilkCircuit',
        tooltip: 'Color palette',
    })
    palette!: string

    @ComboboxControl({
        label: 'Distance',
        values: DISTANCE_MODES,
        default: 'Euclidean',
        tooltip: 'Voronoi distance formula',
    })
    distanceMode!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 4)
        this.scale = getControlValue('scale', 50)
        this.edgeGlow = getControlValue('edgeGlow', 60)
        this.colorShift = getControlValue('colorShift', 30)
        this.palette = getControlValue('palette', 'SilkCircuit')
        this.distanceMode = getControlValue('distanceMode', 'Euclidean')
    }

    protected getControlValues(): VoronoiControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 4)),
            scale: getControlValue('scale', 50),
            edgeGlow: getControlValue('edgeGlow', 60),
            colorShift: getControlValue('colorShift', 30),
            palette: PALETTES.indexOf(getControlValue('palette', 'SilkCircuit')),
            distanceMode: DISTANCE_MODES.indexOf(getControlValue('distanceMode', 'Euclidean')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iScale', 50)
        this.registerUniform('iEdgeGlow', 60)
        this.registerUniform('iColorShift', 30)
        this.registerUniform('iPalette', 0)
        this.registerUniform('iDistanceMode', 0)
    }

    protected updateUniforms(controls: VoronoiControls): void {
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iScale', controls.scale)
        this.setUniform('iEdgeGlow', controls.edgeGlow)
        this.setUniform('iColorShift', controls.colorShift)
        this.setUniform('iPalette', controls.palette)
        this.setUniform('iDistanceMode', controls.distanceMode)
    }
}

const effect = new VoronoiGlass()
initializeEffect(() => effect.initialize(), { instance: effect })
