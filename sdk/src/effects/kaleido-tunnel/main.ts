import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

const PALETTE_MODES = ['Amethyst', 'Deep Sea', 'Electric', 'Monochrome', 'Neon', 'Rainbow', 'Sunset', 'Toxic', 'Vaporwave'] as const
const STYLE_MODES = ['Glitch', 'Grain', 'Holo', 'Standard'] as const

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
        presets: [
            {
                name: 'DMT Breakthrough',
                description: 'Fractal geometry shatters into infinite recursive symmetry — maximum segments, full warp, neon overload',
                controls: {
                    intensity: 210,
                    palette: 'Neon',
                    pulse: 85,
                    segments: 12,
                    speed: 14,
                    style: 'Glitch',
                    twist: 95,
                    warp: 100,
                },
            },
            {
                name: 'Cathedral of Light',
                description: 'Stained glass mandala rotating in slow reverence — deep amethyst hues through a holographic prism',
                controls: {
                    intensity: 160,
                    palette: 'Amethyst',
                    pulse: 30,
                    segments: 8,
                    speed: 2,
                    style: 'Holo',
                    twist: 55,
                    warp: 25,
                },
            },
            {
                name: 'Abyssal Descent',
                description: 'Sinking through bioluminescent depths — toxic greens pulse through a grainy deep-sea corridor',
                controls: {
                    intensity: 80,
                    palette: 'Toxic',
                    pulse: 70,
                    segments: 5,
                    speed: 3.5,
                    style: 'Grain',
                    twist: 40,
                    warp: 65,
                },
            },
            {
                name: 'Sunset Kaleidoscope',
                description: 'Golden hour refracting through a desert crystal — warm vaporwave tones fold into gentle geometry',
                controls: {
                    intensity: 140,
                    palette: 'Sunset',
                    pulse: 45,
                    segments: 6,
                    speed: 4,
                    style: 'Standard',
                    twist: 60,
                    warp: 35,
                },
            },
            {
                name: 'Monochrome Asylum',
                description: 'Stripped of color, pure geometry remains — clinical black and white spinning at the edge of sanity',
                controls: {
                    intensity: 190,
                    palette: 'Monochrome',
                    pulse: 15,
                    segments: 3,
                    speed: 8,
                    style: 'Grain',
                    twist: 80,
                    warp: 50,
                },
            },
        ],
    },
)
