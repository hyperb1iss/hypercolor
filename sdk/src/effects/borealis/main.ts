import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Borealis',
    shader,
    {
        banding: num('Banding', [0, 100], 34, { group: 'Color' }),
        contrast: num('Contrast', [70, 140], 104, { group: 'Color' }),
        curtainHeight: num('Curtain Height', [20, 90], 55, { group: 'Atmosphere' }),
        intensity: num('Intensity', [0, 100], 82, { group: 'Atmosphere' }),
        palette: combo(
            'Palette',
            ['Cyberpunk', 'Fire', 'Ice', 'Northern Lights', 'Phosphor', 'SilkCircuit', 'Sunset', 'Vaporwave'],
            { group: 'Color' },
        ),
        saturation: num('Saturation', [60, 150], 118, { group: 'Color' }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        starBrightness: num('Star Brightness', [0, 100], 40, { group: 'Atmosphere' }),
        warpStrength: num('Warp Strength', [0, 100], 62, { group: 'Motion' }),
    },
    {
        description: 'Aurora borealis — layered curtains of light with richer palette grading and tonal control',
        presets: [
            {
                controls: {
                    banding: 18,
                    contrast: 120,
                    curtainHeight: 85,
                    intensity: 90,
                    palette: 'Ice',
                    saturation: 135,
                    speed: 2,
                    starBrightness: 75,
                    warpStrength: 45,
                },
                description:
                    'Midnight sun refracting through polar ice — slow, towering curtains in crystalline blues and greens',
                name: 'Arctic Solstice',
            },
            {
                controls: {
                    banding: 85,
                    contrast: 138,
                    curtainHeight: 70,
                    intensity: 100,
                    palette: 'Cyberpunk',
                    saturation: 145,
                    speed: 9,
                    starBrightness: 20,
                    warpStrength: 95,
                },
                description:
                    'A solar wind eruption tears the sky open — violent ribbons whip and shatter at impossible speed',
                name: 'Magnetic Storm',
            },
            {
                controls: {
                    banding: 50,
                    contrast: 85,
                    curtainHeight: 40,
                    intensity: 65,
                    palette: 'Phosphor',
                    saturation: 80,
                    speed: 3,
                    starBrightness: 55,
                    warpStrength: 30,
                },
                description:
                    'Bioluminescent plankton drifting through the thermosphere — ghostly green veils pulse in slow motion',
                name: 'Phosphor Dreams',
            },
            {
                controls: {
                    banding: 70,
                    contrast: 130,
                    curtainHeight: 60,
                    intensity: 78,
                    palette: 'Sunset',
                    saturation: 125,
                    speed: 4,
                    starBrightness: 10,
                    warpStrength: 80,
                },
                description:
                    'Ancient light ceremony above a volcanic plateau — deep magentas and ambers weave through sacred geometry',
                name: 'Ritual Veil',
            },
            {
                controls: {
                    banding: 42,
                    contrast: 110,
                    curtainHeight: 50,
                    intensity: 88,
                    palette: 'Vaporwave',
                    saturation: 148,
                    speed: 6,
                    starBrightness: 35,
                    warpStrength: 72,
                },
                description: 'A retro-future skyline hums with neon — synthetic aurora over a digital ocean at dusk',
                name: 'Silicon Vaporwave',
            },
        ],
    },
)
