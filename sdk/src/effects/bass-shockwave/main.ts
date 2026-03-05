import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Bass Shockwave', shader, {
    speed:     [1, 10, 6],
    intensity: [0, 100, 78],
    ringCount: [0, 100, 58],
    decay:     [0, 100, 52],
    palette:   ['SilkCircuit', 'Cyberpunk', 'Fire', 'Aurora', 'Ice'],
    scene:     ['Core Burst', 'Twin Burst', 'Prism Grid'],
}, {
    description: 'Crisp burst-driven shockwave rings with scene-selectable compositions',
    audio: true,
})
