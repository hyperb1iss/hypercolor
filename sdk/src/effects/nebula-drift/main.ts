import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Nebula Drift',
    shader,
    {
        cloudDensity: num('Cloud Density', [10, 100], 72, { group: 'Atmosphere' }),
        contrast: num('Contrast', [70, 150], 106, { group: 'Color' }),
        palette: combo('Palette', ['Aurora', 'Cyberpunk', 'Fire', 'SilkCircuit', 'Vaporwave'], { group: 'Color' }),
        saturation: num('Saturation', [60, 160], 120, { group: 'Color' }),
        speed: num('Speed', [1, 10], 6, { group: 'Motion' }),
        starField: num('Star Field', [0, 100], 28, { group: 'Atmosphere' }),
        warpStrength: num('Warp Strength', [0, 100], 78, { group: 'Motion' }),
    },
    {
        description:
            'Drift through layered nebula ribbons in slow parallax — twinkling stars pierce luminous veils of cosmic gas and dust',
        presets: [
            {
                controls: {
                    cloudDensity: 95,
                    contrast: 130,
                    palette: 'Aurora',
                    saturation: 100,
                    speed: 3,
                    starField: 80,
                    warpStrength: 55,
                },
                description:
                    'Dense stellar nursery columns — massive gas clouds sculpted by newborn stars piercing through the dark',
                name: 'Pillars of Creation',
            },
            {
                controls: {
                    cloudDensity: 30,
                    contrast: 145,
                    palette: 'Cyberpunk',
                    saturation: 75,
                    speed: 2,
                    starField: 15,
                    warpStrength: 90,
                },
                description:
                    'A lone supernova remnant expanding into the abyss — delicate tendrils of light in total darkness',
                name: 'Void Bloom',
            },
            {
                controls: {
                    cloudDensity: 78,
                    contrast: 95,
                    palette: 'Fire',
                    saturation: 155,
                    speed: 5,
                    starField: 60,
                    warpStrength: 65,
                },
                description:
                    'Bioluminescent coral translated to cosmic scale — warm pulsing clouds teeming with particle life',
                name: 'Astral Reef',
            },
            {
                controls: {
                    cloudDensity: 50,
                    contrast: 80,
                    palette: 'SilkCircuit',
                    saturation: 68,
                    speed: 8,
                    starField: 5,
                    warpStrength: 100,
                },
                description:
                    'Reality dissolving at the Planck scale — probability clouds shimmering between existence and void',
                name: 'Quantum Fog',
            },
            {
                controls: {
                    cloudDensity: 65,
                    contrast: 140,
                    palette: 'Vaporwave',
                    saturation: 160,
                    speed: 7,
                    starField: 45,
                    warpStrength: 82,
                },
                description:
                    'The universe as seen through a CRT monitor in 2087 — saturated, scan-lined, impossibly vivid',
                name: 'Synthwave Cosmos',
            },
            {
                controls: {
                    cloudDensity: 15,
                    contrast: 150,
                    palette: 'Cyberpunk',
                    saturation: 60,
                    speed: 1,
                    starField: 100,
                    warpStrength: 10,
                },
                description:
                    'Ten thousand frozen stars suspended in crystal-clear vacuum — the void between galaxies, silent and absolute',
                name: 'Intergalactic Corridor',
            },
            {
                controls: {
                    cloudDensity: 100,
                    contrast: 70,
                    palette: 'Aurora',
                    saturation: 145,
                    speed: 10,
                    starField: 0,
                    warpStrength: 100,
                },
                description:
                    "Ionized plasma cascades through a gas giant's magnetosphere — emerald and violet storm bands tearing across a world with no surface",
                name: 'Jovian Storm Dive',
            },
        ],
    },
)
