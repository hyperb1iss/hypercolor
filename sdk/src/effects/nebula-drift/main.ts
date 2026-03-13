import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Nebula Drift', shader, {
    speed:        [1, 10, 6],
    cloudDensity: [10, 100, 72],
    warpStrength: [0, 100, 78],
    starField:    [0, 100, 28],
    saturation:   [60, 160, 120],
    contrast:     [70, 150, 106],
    palette:      ['Aurora', 'Cyberpunk', 'Fire', 'SilkCircuit', 'Vaporwave'],
}, {
    description: 'Layered nebula ribbons with richer palette grading, visible parallax drift, and twinkling stars',
    presets: [
        {
            name: 'Pillars of Creation',
            description: 'Dense stellar nursery columns — massive gas clouds sculpted by newborn stars piercing through the dark',
            controls: {
                speed: 3,
                cloudDensity: 95,
                warpStrength: 55,
                starField: 80,
                saturation: 100,
                contrast: 130,
                palette: 'Aurora',
            },
        },
        {
            name: 'Void Bloom',
            description: 'A lone supernova remnant expanding into the abyss — delicate tendrils of light in total darkness',
            controls: {
                speed: 2,
                cloudDensity: 30,
                warpStrength: 90,
                starField: 15,
                saturation: 75,
                contrast: 145,
                palette: 'Cyberpunk',
            },
        },
        {
            name: 'Astral Reef',
            description: 'Bioluminescent coral translated to cosmic scale — warm pulsing clouds teeming with particle life',
            controls: {
                speed: 5,
                cloudDensity: 78,
                warpStrength: 65,
                starField: 60,
                saturation: 155,
                contrast: 95,
                palette: 'Fire',
            },
        },
        {
            name: 'Quantum Fog',
            description: 'Reality dissolving at the Planck scale — probability clouds shimmering between existence and void',
            controls: {
                speed: 8,
                cloudDensity: 50,
                warpStrength: 100,
                starField: 5,
                saturation: 68,
                contrast: 80,
                palette: 'SilkCircuit',
            },
        },
        {
            name: 'Synthwave Cosmos',
            description: 'The universe as seen through a CRT monitor in 2087 — saturated, scan-lined, impossibly vivid',
            controls: {
                speed: 7,
                cloudDensity: 65,
                warpStrength: 82,
                starField: 45,
                saturation: 160,
                contrast: 140,
                palette: 'Vaporwave',
            },
        },
    ],
})
