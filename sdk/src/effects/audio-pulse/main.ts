import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Audio Pulse', shader, {
    visualStyle: combo('Style', ['Grid', 'Pulse Field', 'Vortex', 'Waveform'], {
        default: 'Pulse Field',
        tooltip: 'Visualization style',
    }),
    colorScheme: combo('Colors', ['Aurora', 'Cyberpunk', 'Lava', 'Prism', 'Toxic', 'Vaporwave'], {
        default: 'Cyberpunk',
        tooltip: 'Color scheme',
    }),
    sensitivity: num('Sensitivity', [10, 200], 50, {
        normalize: 'none',
        tooltip: 'Audio reactivity',
    }),
    flow: num('Flow', [-100, 100], 30, {
        normalize: 'none',
        tooltip: 'Travel direction and speed',
    }),
    glowIntensity: num('Glow', [10, 200], 100, {
        normalize: 'none',
        tooltip: 'Overall brightness',
    }),
    colorSpeed: num('Color Speed', [0, 200], 50, {
        normalize: 'none',
        tooltip: 'Color cycling speed',
    }),
}, {
    description: 'Cinematic audio visualizer with pulse field, grid, waveform, and vortex modes',
    audio: true,
})
