import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Frequency Cascade', shader, {
    speed:     [1, 10, 5],
    intensity: [0, 100, 75],
    smoothing: [0, 100, 50],
    barWidth:  [0, 100, 58],
    palette:   ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit', 'Sunset'],
    glow:      [0, 100, 28],
    scene:     ['Cascade', 'Prism Skyline', 'Pulse Grid', 'Spectrum Tunnel'],
}, {
    description: 'Spectrum cascade with scene modes and no-audio fallback motion',
    audio: true,
})
