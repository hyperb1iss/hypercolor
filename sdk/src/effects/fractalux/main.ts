import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Fractalux',
    shader,
    {
        palette: combo(
            'Theme',
            ['Aurora', 'Crystal', 'Ember', 'Frozen', 'Midnight', 'Neon', 'Psychedelic', 'SilkCircuit'],
            {
                default: 'SilkCircuit',
                tooltip: 'Color palette — each transforms the light differently.',
                group: 'Color',
            },
        ),
        brightness: num('Intensity', [0, 100], 70, {
            step: 1,
            tooltip: 'Overall luminance — crank it to burn bright.',
            group: 'Color',
        }),
        saturation: num('Saturation', [0, 100], 75, {
            step: 1,
            tooltip: 'Color vibrancy — higher values push toward neon.',
            group: 'Color',
        }),
        motion: combo('Motion', ['Rotate', 'Breathe', 'Spiral', 'Drift', 'Hyperdrive'], {
            default: 'Rotate',
            tooltip: 'Movement style — Hyperdrive combines everything.',
            group: 'Motion',
        }),
        speed: num('Speed', [1, 10], 5, {
            step: 0.5,
            tooltip: 'How fast the fractal evolves and rotates.',
            group: 'Motion',
        }),
        pulse: num('Pulse', [0, 100], 40, {
            step: 1,
            tooltip: 'Breathing intensity — rhythmic zoom and brightness waves.',
            group: 'Motion',
        }),
        segments: num('Symmetry', [3, 12], 8, {
            step: 1,
            tooltip: 'Kaleidoscope fold count — more segments, more fractal mirrors.',
            group: 'Geometry',
        }),
        zoom: num('Scale', [0, 100], 50, {
            step: 1,
            tooltip: 'Pattern magnification — low zooms out, high zooms in.',
            group: 'Geometry',
        }),
        complexity: num('Refraction', [0, 100], 72, {
            step: 1,
            tooltip: 'Fractal detail depth — higher adds crystalline layers.',
            group: 'Geometry',
        }),
        warp: num('Distortion', [0, 100], 55, {
            step: 1,
            tooltip: 'Domain warp intensity — bends space itself.',
            group: 'Geometry',
        }),
    },
    {
        description:
            'Psychedelic kaleidoscopic fractal light engine with multiple motion modes, chromatic dispersion, and deep color control',
        presets: [
            {
                name: 'Hypnotic Vortex',
                description:
                    'Reality folding inward — a spiral of pure spectrum light pulling you into infinite fractal depth, every color the eye can perceive cascading through impossible geometry',
                controls: {
                    brightness: 80,
                    complexity: 85,
                    motion: 'Spiral',
                    palette: 'Psychedelic',
                    pulse: 50,
                    saturation: 85,
                    segments: 8,
                    speed: 6,
                    warp: 70,
                    zoom: 35,
                },
            },
            {
                name: 'Crystal Cathedral',
                description:
                    'Light breathing through stained glass at molecular scale — crystalline refractions expanding and contracting in slow luminous waves',
                controls: {
                    brightness: 80,
                    complexity: 70,
                    motion: 'Breathe',
                    palette: 'Crystal',
                    pulse: 60,
                    saturation: 65,
                    segments: 6,
                    speed: 3,
                    warp: 45,
                    zoom: 55,
                },
            },
            {
                name: 'Neon Dimension',
                description:
                    'Every motion mode firing simultaneously — maximum fractal chaos drenched in electric neon, spiraling and breathing and drifting through a dimension of pure light',
                controls: {
                    brightness: 85,
                    complexity: 95,
                    motion: 'Hyperdrive',
                    palette: 'Neon',
                    pulse: 55,
                    saturation: 90,
                    segments: 10,
                    speed: 8,
                    warp: 80,
                    zoom: 20,
                },
            },
            {
                name: 'Aurora Dreams',
                description:
                    'Northern lights captured in a crystal ball — gentle drifting greens and violets flowing through organic symmetry, peaceful and otherworldly',
                controls: {
                    brightness: 75,
                    complexity: 55,
                    motion: 'Drift',
                    palette: 'Aurora',
                    pulse: 45,
                    saturation: 70,
                    segments: 5,
                    speed: 3,
                    warp: 40,
                    zoom: 60,
                },
            },
            {
                name: 'SilkCircuit Surge',
                description:
                    'Our signature palette cranked to eleven — electric purple and neon cyan refracting through deep geometric symmetry, the visual identity made kinetic',
                controls: {
                    brightness: 75,
                    complexity: 80,
                    motion: 'Rotate',
                    palette: 'SilkCircuit',
                    pulse: 35,
                    saturation: 80,
                    segments: 8,
                    speed: 5,
                    warp: 60,
                    zoom: 40,
                },
            },
            {
                name: 'Deep Meditation',
                description:
                    'Slow breathing mandala in deep indigo space — the kind of visual that dissolves time, four-fold symmetry pulsing like a cosmic heartbeat',
                controls: {
                    brightness: 65,
                    complexity: 60,
                    motion: 'Breathe',
                    palette: 'Midnight',
                    pulse: 70,
                    saturation: 60,
                    segments: 4,
                    speed: 2,
                    warp: 35,
                    zoom: 65,
                },
            },
            {
                name: 'Ember Vortex',
                description:
                    'A fire tornado through a kaleidoscope — 12-fold crimson symmetry spiraling at high speed, molten amber light refracting through every fold',
                controls: {
                    brightness: 80,
                    complexity: 90,
                    motion: 'Spiral',
                    palette: 'Ember',
                    pulse: 40,
                    saturation: 85,
                    segments: 12,
                    speed: 7,
                    warp: 75,
                    zoom: 25,
                },
            },
        ],
    },
)
