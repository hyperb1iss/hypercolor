import { color, combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Plasma Engine',
    shader,
    {
        bgColor: color('Background Color', '#03020c', { group: 'Scene', uniform: 'iBackgroundColor' }),
        bloom: num('Bloom', [0, 100], 16, { group: 'Scene' }),
        color1: color('Color 1', '#16d1d9', { group: 'Color' }),
        color2: color('Color 2', '#ff4fb4', { group: 'Color' }),
        color3: color('Color 3', '#7d49ff', { group: 'Color' }),
        density: num('Density', [10, 100], 32, { group: 'Scene' }),
        speed: num('Speed', [1, 10], 4, { group: 'Motion' }),
        spread: num('Spread', [0, 100], 34, { group: 'Scene' }),
        theme: combo('Theme', ['Arcade', 'Aurora', 'Custom', 'Cyberpunk', 'Inferno', 'Oceanic', 'Poison', 'Tropical'], {
            group: 'Color',
        }),
    },
    {
        description: 'Low-frequency demoscene plasma with fluid motion and saturated color drift',
        presets: [
            {
                controls: {
                    bgColor: '#0a0200',
                    bloom: 85,
                    color1: '#ff4400',
                    color2: '#ffaa00',
                    color3: '#ff0044',
                    density: 18,
                    speed: 7,
                    spread: 72,
                    theme: 'Inferno',
                },
                description:
                    'Superheated hydrogen plasma arcs off the photosphere — million-degree filaments twist in magnetic fury',
                name: 'Solar Corona Eruption',
            },
            {
                controls: {
                    bgColor: '#010208',
                    bloom: 45,
                    color1: '#33f587',
                    color2: '#3fdcff',
                    color3: '#8c4bff',
                    density: 24,
                    speed: 3,
                    spread: 88,
                    theme: 'Aurora',
                },
                description:
                    'Charged particles cascade through the magnetosphere — emerald and violet curtains ripple at the edge of space',
                name: 'Borealis Curtain',
            },
            {
                controls: {
                    bgColor: '#020a0f',
                    bloom: 28,
                    color1: '#00c8ff',
                    color2: '#0055aa',
                    color3: '#003366',
                    density: 62,
                    speed: 2,
                    spread: 55,
                    theme: 'Oceanic',
                },
                description:
                    'Mineral-laden plasma seeps from black smokers — oceanic chemicals glow with impossible deep-sea energy',
                name: 'Hadal Chemosynthesis',
            },
            {
                controls: {
                    bgColor: '#030802',
                    bloom: 62,
                    color1: '#44ff00',
                    color2: '#00ff88',
                    color3: '#88ff44',
                    density: 45,
                    speed: 4.5,
                    spread: 40,
                    theme: 'Poison',
                },
                description:
                    'Toxic interstellar clouds collapse under their own gravity — phosphorescent poison condenses into newborn stars',
                name: 'Nerve Gas Nebula',
            },
            {
                controls: {
                    bgColor: '#08020e',
                    bloom: 100,
                    color1: '#ff00ff',
                    color2: '#00ffff',
                    color3: '#ff0088',
                    density: 10,
                    speed: 9,
                    spread: 95,
                    theme: 'Cyberpunk',
                },
                description:
                    'The containment field pulses with retro-future energy — neon plasma oscillates between beautiful and catastrophic',
                name: 'Synthwave Reactor Core',
            },
        ],
    },
)
