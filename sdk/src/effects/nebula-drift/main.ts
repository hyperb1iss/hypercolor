import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Nebula Drift', shader, {
    speed:        [1, 10, 6],
    cloudDensity: [10, 100, 72],
    warpStrength: [0, 100, 78],
    starField:    [0, 100, 40],
    saturation:   [60, 180, 142],
    contrast:     [70, 160, 116],
    palette:      ['SilkCircuit', 'Cyberpunk', 'Aurora', 'Fire', 'Vaporwave'],
}, {
    description: 'Layered nebula ribbons with richer palette grading, visible parallax drift, and twinkling stars',
})
