import { color, combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Deep Current',
    shader,
    {
        blend: num('Blend', [0, 100], 26, { group: 'Color' }),
        leftColor: color('Left Color', '#ff4fb4', { group: 'Color' }),
        particleAmount: num('Particle Amount', [0, 100], 56, { group: 'Scene' }),
        rightColor: color('Right Color', '#ff9a3d', { group: 'Color' }),
        rippleIntensity: num('Ripple Intensity', [0, 100], 68, { group: 'Motion' }),
        speed: num('Speed', [1, 10], 4, { group: 'Motion' }),
        splitMode: combo('Split Mode', ['Diagonal', 'Horizontal', 'Vertical'], { group: 'Scene' }),
    },
    {
        description: 'Magenta-amber split-field with crisp ripples and floating particles',
        presets: [
            {
                controls: {
                    blend: 12,
                    leftColor: '#0a1a4f',
                    particleAmount: 92,
                    rightColor: '#ff6a00',
                    rippleIntensity: 85,
                    speed: 3,
                    splitMode: 'Vertical',
                },
                description:
                    'Superheated mineral plumes billow from the ocean floor — sulfurous amber meets abyssal indigo at the thermocline',
                name: 'Hydrothermal Vent',
            },
            {
                controls: {
                    blend: 65,
                    leftColor: '#00ffcc',
                    particleAmount: 78,
                    rightColor: '#0b0040',
                    rippleIntensity: 40,
                    speed: 2,
                    splitMode: 'Diagonal',
                },
                description:
                    'Jellyfish trails of living cyan pulse through midnight water — particles scatter like disturbed plankton',
                name: 'Bioluminescent Drift',
            },
            {
                controls: {
                    blend: 8,
                    leftColor: '#ff2200',
                    particleAmount: 45,
                    rightColor: '#ff8800',
                    rippleIntensity: 100,
                    speed: 8,
                    splitMode: 'Horizontal',
                },
                description:
                    'Tectonic plates grind and split — molten red bleeds through fractured basalt as the seabed tears itself apart',
                name: 'Magma Subduction Zone',
            },
            {
                controls: {
                    blend: 88,
                    leftColor: '#e8f4ff',
                    particleAmount: 35,
                    rightColor: '#1affef',
                    rippleIntensity: 22,
                    speed: 1.5,
                    splitMode: 'Horizontal',
                },
                description:
                    'Glacial turquoise slides beneath polar white — sediment particles suspended in frigid silence',
                name: 'Arctic Meltwater',
            },
            {
                controls: {
                    blend: 48,
                    leftColor: '#33ff66',
                    particleAmount: 100,
                    rightColor: '#8b4513',
                    rippleIntensity: 62,
                    speed: 5,
                    splitMode: 'Diagonal',
                },
                description:
                    'Amino acids crystallize in warm tidal pools — electric green catalysts spark against ferrous mineral haze',
                name: 'Primordial Soup',
            },
        ],
    },
)
