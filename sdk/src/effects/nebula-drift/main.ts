import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Nebula Drift', shader, {
    speed:        [1, 10, 6],
    cloudDensity: [10, 100, 68],
    warpStrength: [0, 100, 72],
    starField:    [0, 100, 34],
    palette:      ['SilkCircuit', 'Cyberpunk', 'Aurora', 'Fire', 'Vaporwave'],
}, {
    description: 'Layered nebula ribbons with visible parallax drift and twinkling stars',
})
