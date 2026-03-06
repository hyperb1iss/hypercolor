import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Nebula Drift', shader, {
    speed:        [1, 10, 6],
    cloudDensity: [10, 100, 72],
    warpStrength: [0, 100, 78],
    starField:    [0, 100, 28],
    saturation:   [60, 160, 120],
    contrast:     [70, 150, 106],
    palette:      ['SilkCircuit', 'Cyberpunk', 'Aurora', 'Fire', 'Vaporwave'],
}, {
    description: 'Layered nebula ribbons with richer palette grading, visible parallax drift, and twinkling stars',
})
