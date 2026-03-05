import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Nebula Drift', shader, {
    speed:       [1, 10, 5],
    cloudDensity: [10, 100, 60],
    warpStrength: [0, 100, 50],
    starField:   [0, 100, 40],
    palette:     ['SilkCircuit', 'Cyberpunk', 'Aurora', 'Fire', 'Vaporwave'],
}, {
    description: 'Animated domain-warped fBm nebula clouds with multi-layer star field',
})
