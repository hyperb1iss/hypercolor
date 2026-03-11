import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Audio Pulse', shader, {
    visualStyle: combo('Style', ['Grid', 'Pulse Field', 'Vortex', 'Waveform'], {
        default: 'Pulse Field',
        tooltip: 'Visualization style',
    }),
    colorScheme: combo('Colors', ['Aurora', 'Cyberpunk', 'Lava', 'Prism', 'Toxic', 'Vaporwave'], {
        default: 'Cyberpunk',
        tooltip: 'Color scheme preset',
    }),
    sensitivity: num('Sensitivity', [10, 200], 50, {
        normalize: 'none',
        tooltip: 'Audio sensitivity - lower for loud sources',
    }),
    smoothing: num('Smoothing', [0, 95], 70, {
        normalize: 'none',
        tooltip: 'Motion smoothing (higher = less jitter)',
    }),
    bassBoost: num('Bass Boost', [0, 200], 80, {
        normalize: 'none',
        tooltip: 'Bass frequency emphasis',
    }),
    colorSpeed: num('Color Speed', [0, 200], 30, {
        normalize: 'none',
        tooltip: 'Color cycling speed',
    }),
    ringCount: num('Segments', [4, 16], 8, {
        step: 1,
        normalize: 'none',
        tooltip: 'Pattern complexity / bar count',
    }),
    glowIntensity: num('Glow', [0, 200], 80, {
        normalize: 'none',
        tooltip: 'Bloom and glow intensity',
    }),
    flow: num('Flow', [-100, 100], 30, {
        normalize: 'none',
        tooltip: 'Negative = inward pull, positive = outward burst',
    }),
    direction: num('Direction', [-360, 360], 0, {
        normalize: 'none',
        tooltip: 'Pulse Field camera yaw offset',
    }),
    bend: num('Bend', [-200, 200], 0, {
        normalize: 'none',
        tooltip: 'Pulse Field lattice bend strength',
    }),
}, {
    description: 'Cinematic audio visualizer with pulse field, grid, waveform, and vortex modes',
    audio: true,
})
