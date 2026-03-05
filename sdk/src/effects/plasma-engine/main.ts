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

interface PlasmaControls {
    speed: number
    complexity: number
    distortion: number
    zoom: number
    palette: number
}

const PALETTES = ['SilkCircuit', 'Rainbow', 'Cyberpunk', 'Fire', 'Aurora', 'Vaporwave']

@Effect({
    name: 'Plasma Engine',
    description: 'Classic multi-wave plasma interference with IQ palette mapping',
    author: 'Hypercolor',
})
class PlasmaEngine extends WebGLEffect<PlasmaControls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5, tooltip: 'Animation speed' })
    speed!: number

    @NumberControl({ label: 'Complexity', min: 0, max: 100, default: 50, tooltip: 'Wave complexity' })
    complexity!: number

    @NumberControl({ label: 'Distortion', min: 0, max: 100, default: 40, tooltip: 'Field distortion' })
    distortion!: number

    @NumberControl({ label: 'Zoom', min: 0, max: 100, default: 30, tooltip: 'Zoom level' })
    zoom!: number

    @ComboboxControl({
        label: 'Palette',
        values: PALETTES,
        default: 'SilkCircuit',
        tooltip: 'Color palette',
    })
    palette!: string

    constructor() {
        super({ fragmentShader })
    }

    protected initializeControls(): void {
        this.speed = getControlValue('speed', 5)
        this.complexity = getControlValue('complexity', 50)
        this.distortion = getControlValue('distortion', 40)
        this.zoom = getControlValue('zoom', 30)
        this.palette = getControlValue('palette', 'SilkCircuit')
    }

    protected getControlValues(): PlasmaControls {
        return {
            speed: normalizeSpeed(getControlValue('speed', 5)),
            complexity: getControlValue('complexity', 50),
            distortion: getControlValue('distortion', 40),
            zoom: getControlValue('zoom', 30),
            palette: PALETTES.indexOf(getControlValue('palette', 'SilkCircuit')),
        }
    }

    protected createUniforms(): void {
        this.registerUniform('iSpeed', 1.0)
        this.registerUniform('iComplexity', 50)
        this.registerUniform('iDistortion', 40)
        this.registerUniform('iZoom', 30)
        this.registerUniform('iPalette', 0)
    }

    protected updateUniforms(controls: PlasmaControls): void {
        this.setUniform('iSpeed', controls.speed)
        this.setUniform('iComplexity', controls.complexity)
        this.setUniform('iDistortion', controls.distortion)
        this.setUniform('iZoom', controls.zoom)
        this.setUniform('iPalette', controls.palette)
    }
}

const effect = new PlasmaEngine()
initializeEffect(() => effect.initialize(), { instance: effect })
