import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

const PALETTE_MODES = [
    'Rainbow',
    'Neon',
    'Monochrome',
    'Electric',
    'Amethyst',
    'Sunset',
    'Toxic',
    'Vaporwave',
    'Deep Sea',
] as const
const STYLE_MODES = ['Standard', 'Glitch', 'Holo', 'Grain'] as const

export default effect(
    'Kaleido Tunnel',
    shader,
    {
        intensity: num('Intensity', [20, 220], 120, {
            step: 1,
            tooltip: 'Overall brightness and energy in the tunnel.',
            uniform: 'iColorIntensity',
        }),
        palette: combo('Palette', PALETTE_MODES, {
            default: 'Rainbow',
            tooltip: 'Core tunnel palette.',
            uniform: 'iColorMode',
        }),
        pulse: num('Pulse', [0, 100], 50, {
            step: 1,
            tooltip: 'Breathing and shimmer in the tunnel energy.',
            uniform: 'iPulse',
        }),
        segments: num('Segments', [3, 12], 6, {
            step: 1,
            tooltip: 'Number of mirrored kaleidoscope slices.',
            uniform: 'iSegments',
        }),
        speed: num('Speed', [1, 20], 5, {
            normalize: 'none',
            step: 0.5,
            tooltip: 'Controls tunnel motion speed. Values above 10 push into extra overdrive.',
            uniform: 'iSpeed',
        }),
        style: combo('Style', STYLE_MODES, {
            default: 'Standard',
            tooltip: 'Post-processing treatment.',
            uniform: 'iStyle',
        }),
        twist: num('Twist', [0, 100], 40, {
            step: 1,
            tooltip: 'Angular twist through the tunnel depth.',
            uniform: 'iTwist',
        }),
        warp: num('Warp', [0, 100], 30, {
            step: 1,
            tooltip: 'Psychedelic distortion before the kaleidoscope fold.',
            uniform: 'iWarp',
        }),
    },
    {
        author: 'Hypercolor',
        description:
            'Port of the Lightscript Workshop kaleidoscopic tunnel with full symmetry, warp, and palette controls',
    },
)
