import { color, effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Plasma Engine', shader, {
    theme:   ['Arcade', 'Aurora', 'Custom', 'Cyberpunk', 'Inferno', 'Oceanic', 'Poison', 'Tropical'],
    bgColor: color('Background Color', '#03020c', { uniform: 'iBackgroundColor' }),
    color1:  '#16d1d9',
    color2:  '#ff4fb4',
    color3:  '#7d49ff',
    speed:   [1, 10, 4],
    bloom:   [0, 100, 16],
    spread:  [0, 100, 34],
    density: [10, 100, 32],
}, {
    description: 'Low-frequency demoscene plasma with fluid motion and saturated color drift',
    presets: [
        {
            name: 'Solar Corona Eruption',
            description: 'Superheated hydrogen plasma arcs off the photosphere — million-degree filaments twist in magnetic fury',
            controls: { theme: 'Inferno', bgColor: '#0a0200', color1: '#ff4400', color2: '#ffaa00', color3: '#ff0044', speed: 7, bloom: 85, spread: 72, density: 18 },
        },
        {
            name: 'Borealis Curtain',
            description: 'Charged particles cascade through the magnetosphere — emerald and violet curtains ripple at the edge of space',
            controls: { theme: 'Aurora', bgColor: '#010208', color1: '#33f587', color2: '#3fdcff', color3: '#8c4bff', speed: 3, bloom: 45, spread: 88, density: 24 },
        },
        {
            name: 'Hadal Chemosynthesis',
            description: 'Mineral-laden plasma seeps from black smokers — oceanic chemicals glow with impossible deep-sea energy',
            controls: { theme: 'Oceanic', bgColor: '#020a0f', color1: '#00c8ff', color2: '#0055aa', color3: '#003366', speed: 2, bloom: 28, spread: 55, density: 62 },
        },
        {
            name: 'Nerve Gas Nebula',
            description: 'Toxic interstellar clouds collapse under their own gravity — phosphorescent poison condenses into newborn stars',
            controls: { theme: 'Poison', bgColor: '#030802', color1: '#44ff00', color2: '#00ff88', color3: '#88ff44', speed: 4.5, bloom: 62, spread: 40, density: 45 },
        },
        {
            name: 'Synthwave Reactor Core',
            description: 'The containment field pulses with retro-future energy — neon plasma oscillates between beautiful and catastrophic',
            controls: { theme: 'Cyberpunk', bgColor: '#08020e', color1: '#ff00ff', color2: '#00ffff', color3: '#ff0088', speed: 9, bloom: 100, spread: 95, density: 10 },
        },
    ],
})
