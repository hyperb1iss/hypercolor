import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Frequency Cascade', shader, {
    speed:     [1, 10, 5],
    intensity: [0, 100, 75],
    smoothing: [0, 100, 50],
    barWidth:  [0, 100, 58],
    palette:   ['SilkCircuit', 'Aurora', 'Cyberpunk', 'Fire', 'Sunset', 'Ice'],
    glow:      [0, 100, 28],
    scene:     ['Cascade', 'Pulse Grid', 'Spectrum Tunnel', 'Prism Skyline'],
}, {
    description: 'Community-style spectrum cascade with scene modes and no-audio fallback motion',
    audio: true,
})
