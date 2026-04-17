import { color, combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

interface PaletteTriad {
    left: string
    right: string
    bg: string
}

const CUSTOM_PALETTE = 'Custom'

// Each palette is a collision triad: a dominant left current, an opposing
// right current, and a background that reads as void/shadow in the valleys
// between wave crests. All color pairs are tuned to feel like two different
// forces meeting, not just two complementary hues painted side-by-side.
const PALETTES: Record<string, PaletteTriad> = {
    SilkCircuit: { bg: '#0c0020', left: '#e135ff', right: '#ff3ea4' },
    Cyberpunk: { bg: '#06020e', left: '#ff2975', right: '#00f0ff' },
    Aurora: { bg: '#04081a', left: '#00ff9f', right: '#9d00ff' },
    Hydrothermal: { bg: '#0c0208', left: '#ff6a00', right: '#2040a0' },
    'Phantom Reef': { bg: '#060818', left: '#00ffcc', right: '#c03070' },
    'Solar Flare': { bg: '#140400', left: '#fffce0', right: '#ff1a00' },
    Glacial: { bg: '#08101e', left: '#e0f0ff', right: '#40ffd0' },
    Toxic: { bg: '#060c04', left: '#aaff00', right: '#b028d8' },
    'Blood Moon': { bg: '#100006', left: '#d8d0f0', right: '#9a2035' },
    Vaporwave: { bg: '#1a0820', left: '#ff71ce', right: '#01cdfe' },
    Lava: { bg: '#0e0200', left: '#ffba08', right: '#d00000' },
    Tidal: { bg: '#020815', left: '#40ffea', right: '#2070ff' },
    Sunset: { bg: '#1a0206', left: '#ff6b4a', right: '#6040c0' },
    Ember: { bg: '#0c0206', left: '#ffd23f', right: '#b01020' },
}

const PALETTE_NAMES: readonly string[] = [CUSTOM_PALETTE, ...Object.keys(PALETTES)]

function hexToFloats(hex: string): [number, number, number] {
    const h = hex.replace('#', '')
    return [
        Number.parseInt(h.slice(0, 2), 16) / 255,
        Number.parseInt(h.slice(2, 4), 16) / 255,
        Number.parseInt(h.slice(4, 6), 16) / 255,
    ]
}

export default effect(
    'Deep Current',
    shader,
    {
        palette: combo('Palette', PALETTE_NAMES, {
            default: 'SilkCircuit',
            group: 'Color',
            tooltip: 'Curated collision triad. Overrides color pickers unless set to Custom.',
        }),
        leftColor: color('Left Color', '#ff4fb4', { group: 'Color' }),
        rightColor: color('Right Color', '#ff9a3d', { group: 'Color' }),
        bgColor: color('Background', '#050510', { group: 'Color' }),
        blend: num('Collision Width', [0, 100], 20, { group: 'Color' }),
        direction: combo('Flow Direction', ['Diagonal', 'Horizontal', 'Vertical'], { group: 'Scene' }),
        flow: num('Flow Strength', [0, 100], 65, { group: 'Motion' }),
        speed: num('Speed', [1, 10], 4, { group: 'Motion' }),
        turbulence: num('Turbulence', [0, 100], 60, { group: 'Motion' }),
    },
    {
        description:
            'Two opposing fluid currents collide. Domain-warped color fields push against each other across a turbulent, ever-shifting boundary. 14 curated palettes, or bring your own.',
        // Why a frame hook instead of baking palettes into the shader:
        // keeps palette data as editable TS, avoids a bloated const array in
        // GLSL, and lets the Custom path fall through to the color controls
        // without any branching in the fragment shader.
        frame: (ctx) => {
            const idx = ctx.controls.palette as number
            const name = PALETTE_NAMES[idx] ?? CUSTOM_PALETTE
            if (name === CUSTOM_PALETTE) return
            const p = PALETTES[name]
            if (!p) return
            ctx.setUniform('iLeftColor', hexToFloats(p.left))
            ctx.setUniform('iRightColor', hexToFloats(p.right))
            ctx.setUniform('iBgColor', hexToFloats(p.bg))
        },
        presets: [
            {
                controls: {
                    bgColor: '#0c0208',
                    blend: 12,
                    direction: 'Vertical',
                    flow: 85,
                    leftColor: '#ff6a00',
                    palette: 'Hydrothermal',
                    rightColor: '#2040a0',
                    speed: 3,
                    turbulence: 78,
                },
                description:
                    'Superheated amber plumes erupt through deep-ocean indigo. The faintly volcanic background radiates residual heat between the crests.',
                name: 'Hydrothermal Vent',
            },
            {
                controls: {
                    bgColor: '#060818',
                    blend: 60,
                    direction: 'Diagonal',
                    flow: 35,
                    leftColor: '#00ffcc',
                    palette: 'Phantom Reef',
                    rightColor: '#c03070',
                    speed: 1.5,
                    turbulence: 30,
                },
                description:
                    'Ghostly teal drifts through a deep bioluminescent haze. Dark rose pulses from the opposite direction, the two barely touching.',
                name: 'Phantom Reef',
            },
            {
                controls: {
                    bgColor: '#140400',
                    blend: 8,
                    direction: 'Horizontal',
                    flow: 95,
                    leftColor: '#fffce0',
                    palette: 'Solar Flare',
                    rightColor: '#ff1a00',
                    speed: 8,
                    turbulence: 92,
                },
                description:
                    'White-hot plasma slams into coronal red. The dark corona background swallows everything between the razor-thin collision filaments.',
                name: 'Solar Flare',
            },
            {
                controls: {
                    bgColor: '#08101e',
                    blend: 75,
                    direction: 'Horizontal',
                    flow: 25,
                    leftColor: '#e0f0ff',
                    palette: 'Glacial',
                    rightColor: '#40ffd0',
                    speed: 1,
                    turbulence: 18,
                },
                description:
                    'Tectonic ice sheets grind in slow motion. The steel-blue deep fills the gaps with polar cold as glacial white meets pale cyan.',
                name: 'Glacial Crush',
            },
            {
                controls: {
                    bgColor: '#060c04',
                    blend: 35,
                    direction: 'Diagonal',
                    flow: 75,
                    leftColor: '#aaff00',
                    palette: 'Toxic',
                    rightColor: '#b028d8',
                    speed: 5.5,
                    turbulence: 80,
                },
                description:
                    'Acid chartreuse boils against bruised violet. The murky swamp background seeps through as toxic currents churn diagonally.',
                name: "Witch's Cauldron",
            },
            {
                controls: {
                    bgColor: '#0c0020',
                    blend: 20,
                    direction: 'Vertical',
                    flow: 80,
                    leftColor: '#e135ff',
                    palette: 'Custom',
                    rightColor: '#80ffea',
                    speed: 6.5,
                    turbulence: 72,
                },
                description:
                    'A dimensional tear rips open. SilkCircuit violet and neon cyan pour from opposite ends, the void-purple between them crackling with interference.',
                name: 'Rift Gate',
            },
            {
                controls: {
                    bgColor: '#100006',
                    blend: 85,
                    direction: 'Horizontal',
                    flow: 20,
                    leftColor: '#d8d0f0',
                    palette: 'Blood Moon',
                    rightColor: '#9a2035',
                    speed: 1,
                    turbulence: 15,
                },
                description:
                    'Pale moonlight silver meets deep oxblood in a ritual tide. The crimson-black abyss breathes between enormous slow-moving folds.',
                name: 'Blood Moon Tide',
            },
            {
                controls: {
                    bgColor: '#020004',
                    blend: 5,
                    direction: 'Diagonal',
                    flow: 90,
                    leftColor: '#080810',
                    palette: 'Custom',
                    rightColor: '#5500cc',
                    speed: 9,
                    turbulence: 95,
                },
                description:
                    'Near-black mass devours ultraviolet plasma. The boundary barely exists, just violent fractures of purple light tearing through absolute nothing.',
                name: 'Abyssal Maw',
            },
            {
                controls: {
                    bgColor: '#06020e',
                    blend: 8,
                    direction: 'Horizontal',
                    flow: 92,
                    leftColor: '#ff2975',
                    palette: 'Cyberpunk',
                    rightColor: '#00f0ff',
                    speed: 7,
                    turbulence: 88,
                },
                description:
                    'Hot pink runs a gauntlet against electric cyan. Razor-thin interference fractures strobe across the seam like a corrupted data stream.',
                name: 'Neural Clash',
            },
            {
                controls: {
                    bgColor: '#04081a',
                    blend: 72,
                    direction: 'Vertical',
                    flow: 28,
                    leftColor: '#00ff9f',
                    palette: 'Aurora',
                    rightColor: '#9d00ff',
                    speed: 1.5,
                    turbulence: 22,
                },
                description:
                    'Emerald curtains drift skyward through violet haze. The boundary stretches so wide the two currents share every pixel in soft magnetic sheets.',
                name: 'Polar Drift',
            },
            {
                controls: {
                    bgColor: '#0e0200',
                    blend: 30,
                    direction: 'Horizontal',
                    flow: 50,
                    leftColor: '#ffba08',
                    palette: 'Lava',
                    rightColor: '#d00000',
                    speed: 2.5,
                    turbulence: 55,
                },
                description:
                    'Molten gold rolls into deep crimson. Thick viscous crests breathe through a char-black underbed, the collision slow and unstoppable.',
                name: 'Molten Roll',
            },
            {
                controls: {
                    bgColor: '#1a0820',
                    blend: 55,
                    direction: 'Diagonal',
                    flow: 45,
                    leftColor: '#ff71ce',
                    palette: 'Vaporwave',
                    rightColor: '#01cdfe',
                    speed: 3,
                    turbulence: 40,
                },
                description:
                    'Pink and cyan ribbons trade lanes through dusky violet. The whole field breathes in slow, dreamy counterpoint.',
                name: 'Mirror Tide',
            },
        ],
    },
)
