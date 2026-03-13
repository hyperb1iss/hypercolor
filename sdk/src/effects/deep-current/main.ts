import { color, combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Deep Current', shader, {
    leftColor:       color('Left Color', '#ff4fb4', { group: 'Color' }),
    rightColor:      color('Right Color', '#ff9a3d', { group: 'Color' }),
    speed:           num('Speed', [1, 10], 4, { group: 'Motion' }),
    rippleIntensity: num('Ripple Intensity', [0, 100], 68, { group: 'Motion' }),
    particleAmount:  num('Particle Amount', [0, 100], 56, { group: 'Scene' }),
    blend:           num('Blend', [0, 100], 26, { group: 'Color' }),
    splitMode:       combo('Split Mode', ['Diagonal', 'Horizontal', 'Vertical'], { group: 'Scene' }),
}, {
    description: 'Magenta-amber split-field with crisp ripples and floating particles',
    presets: [
        {
            name: 'Hydrothermal Vent',
            description: 'Superheated mineral plumes billow from the ocean floor — sulfurous amber meets abyssal indigo at the thermocline',
            controls: { leftColor: '#0a1a4f', rightColor: '#ff6a00', speed: 3, rippleIntensity: 85, particleAmount: 92, blend: 12, splitMode: 'Vertical' },
        },
        {
            name: 'Bioluminescent Drift',
            description: 'Jellyfish trails of living cyan pulse through midnight water — particles scatter like disturbed plankton',
            controls: { leftColor: '#00ffcc', rightColor: '#0b0040', speed: 2, rippleIntensity: 40, particleAmount: 78, blend: 65, splitMode: 'Diagonal' },
        },
        {
            name: 'Magma Subduction Zone',
            description: 'Tectonic plates grind and split — molten red bleeds through fractured basalt as the seabed tears itself apart',
            controls: { leftColor: '#ff2200', rightColor: '#ff8800', speed: 8, rippleIntensity: 100, particleAmount: 45, blend: 8, splitMode: 'Horizontal' },
        },
        {
            name: 'Arctic Meltwater',
            description: 'Glacial turquoise slides beneath polar white — sediment particles suspended in frigid silence',
            controls: { leftColor: '#e8f4ff', rightColor: '#1affef', speed: 1.5, rippleIntensity: 22, particleAmount: 35, blend: 88, splitMode: 'Horizontal' },
        },
        {
            name: 'Primordial Soup',
            description: 'Amino acids crystallize in warm tidal pools — electric green catalysts spark against ferrous mineral haze',
            controls: { leftColor: '#33ff66', rightColor: '#8b4513', speed: 5, rippleIntensity: 62, particleAmount: 100, blend: 48, splitMode: 'Diagonal' },
        },
    ],
})
