import { color, combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Deep Current',
    shader,
    {
        bgColor: color('Background', '#050510', { group: 'Color' }),
        blend: num('Collision Width', [0, 100], 20, { group: 'Color' }),
        direction: combo('Flow Direction', ['Diagonal', 'Horizontal', 'Vertical'], { group: 'Scene' }),
        flow: num('Flow Strength', [0, 100], 65, { group: 'Motion' }),
        leftColor: color('Left Color', '#ff4fb4', { group: 'Color' }),
        rightColor: color('Right Color', '#ff9a3d', { group: 'Color' }),
        speed: num('Speed', [1, 10], 4, { group: 'Motion' }),
        turbulence: num('Turbulence', [0, 100], 60, { group: 'Motion' }),
    },
    {
        description:
            'Two opposing fluid currents collide — domain-warped color fields push against each other across a turbulent, ever-shifting boundary',
        presets: [
            {
                controls: {
                    bgColor: '#030318',
                    blend: 15,
                    direction: 'Vertical',
                    flow: 80,
                    leftColor: '#0a1a4f',
                    rightColor: '#ff6a00',
                    speed: 3,
                    turbulence: 75,
                },
                description:
                    'Superheated amber plumes push against abyssal indigo — the thermocline buckles and folds as the two masses collide',
                name: 'Hydrothermal Vent',
            },
            {
                controls: {
                    bgColor: '#020210',
                    blend: 55,
                    direction: 'Diagonal',
                    flow: 45,
                    leftColor: '#00ffcc',
                    rightColor: '#0b0040',
                    speed: 2,
                    turbulence: 40,
                },
                description:
                    'Living cyan flows through midnight water — the boundary dissolves into soft turbulent tendrils of bioluminescence',
                name: 'Bioluminescent Drift',
            },
            {
                controls: {
                    bgColor: '#0a0200',
                    blend: 10,
                    direction: 'Horizontal',
                    flow: 90,
                    leftColor: '#ff2200',
                    rightColor: '#ff8800',
                    speed: 7,
                    turbulence: 85,
                },
                description:
                    'Molten red and orange magma streams slam into each other — the collision zone erupts with turbulent cross-current fingers',
                name: 'Magma Subduction Zone',
            },
            {
                controls: {
                    bgColor: '#060a12',
                    blend: 70,
                    direction: 'Horizontal',
                    flow: 30,
                    leftColor: '#e8f4ff',
                    rightColor: '#1affef',
                    speed: 1.5,
                    turbulence: 25,
                },
                description:
                    'Glacial white slides against polar turquoise — barely turbulent, the two bodies merge in long slow folds',
                name: 'Arctic Meltwater',
            },
            {
                controls: {
                    bgColor: '#020a04',
                    blend: 40,
                    direction: 'Diagonal',
                    flow: 70,
                    leftColor: '#33ff66',
                    rightColor: '#8b4513',
                    speed: 5,
                    turbulence: 65,
                },
                description:
                    'Electric green catalysts crash against ferrous mineral flow — the diagonal collision spawns organic tendrils in both directions',
                name: 'Primordial Soup',
            },
            {
                controls: {
                    bgColor: '#08001a',
                    blend: 25,
                    direction: 'Vertical',
                    flow: 75,
                    leftColor: '#e135ff',
                    rightColor: '#80ffea',
                    speed: 6,
                    turbulence: 70,
                },
                description:
                    'Electric violet and neon cyan flow against each other — the collision boundary warps and folds in a permanent shockwave',
                name: 'Rift Gate',
            },
            {
                controls: {
                    bgColor: '#0a0600',
                    blend: 80,
                    direction: 'Horizontal',
                    flow: 25,
                    leftColor: '#ffd700',
                    rightColor: '#ff4500',
                    speed: 1,
                    turbulence: 20,
                },
                description:
                    'Liquid gold pours into burnt sienna — the wide collision zone is almost peaceful, slow folds of amber merging at the horizon',
                name: 'Saharan Goldmelt',
            },
            {
                controls: {
                    bgColor: '#020002',
                    blend: 8,
                    direction: 'Diagonal',
                    flow: 85,
                    leftColor: '#0d0d0d',
                    rightColor: '#4a0080',
                    speed: 8,
                    turbulence: 90,
                },
                description:
                    'Void-black mass collides with ultraviolet plasma at high velocity — the razor-thin boundary fractures into violent cross-current tendrils',
                name: 'Event Horizon',
            },
        ],
    },
)
