#version 300 es
// Cymatics — Cinematic Audio Visualizer
// Sound made visible. Two modes: Twist (warping cube corridor) and
// Particle Field (luminous 3D cell drift).
// Inspired by "3D Audio Visualizer" by @kishimisu (CC BY-NC-SA 4.0)
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSensitivity;   // 10–200
uniform float iFlow;          // -100–100
uniform float iSpeed;         // 10–200  (applied via iMotionTime in JS)
uniform float iCurvature;     // 0–150
uniform float iThrust;        // 0–150
uniform float iGlowIntensity; // 10–200
uniform float iColorSpeed;    // 0–200
uniform int iVisualStyle;     // 0–1
uniform int iColorScheme;     // 0–5

// Audio (SDK auto-provides)
uniform float iAudioLevel;
uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioBeatPulse;
uniform float iAudioMomentum;
uniform float iAudioSwell;
uniform float iAudioLevelSmooth;
uniform float iAudioBassSmooth;
uniform float iAudioMidSmooth;
uniform float iAudioTrebleSmooth;
uniform float iMotionEnergy;
uniform float iMotionPulse;
uniform vec2 iMotionPan;
uniform float iMotionZoom;
uniform float iMotionTwist;
uniform float iFlowDrift;
uniform float iWarpPhase;
uniform float iMotionTime;    // speed-scaled time accumulator

#define PI 3.14159265359
#define TAU 6.28318530718

// ─── Normalized controls ─────────────────────────────────────

float sens()    { return clamp(iSensitivity / 100.0, 0.1, 2.0); }
float flowN()   { return clamp(iFlow / 100.0, -1.0, 1.0); }
float glowN()   { return clamp(iGlowIntensity / 100.0, 0.1, 2.0); }
float cSpd()    { return clamp(iColorSpeed / 100.0, 0.0, 2.0); }
float curveN()  { return clamp(iCurvature / 100.0, 0.0, 1.5); }
float thrustN() { return clamp(iThrust / 100.0, 0.0, 1.5); }

// ─── Audio helpers ───────────────────────────────────────────

float levelEnv()  { return clamp(iAudioLevelSmooth, 0.0, 1.0); }
float bassEnv()   { return clamp(iAudioBassSmooth, 0.0, 1.0); }
float midEnv()    { return clamp(iAudioMidSmooth, 0.0, 1.0); }
float trebleEnv() { return clamp(iAudioTrebleSmooth, 0.0, 1.0); }
float motionEnergy() { return clamp(iMotionEnergy, 0.0, 1.0); }
float motionPulse() { return clamp(iMotionPulse, 0.0, 1.0); }

// Interpolate bass/mid/treble into a pseudo-spectrum
float getFreq(float idx) {
    idx = clamp(idx, 0.0, 1.0);
    float bW = exp(-idx * idx * 30.0);
    float mW = exp(-(idx - 0.35) * (idx - 0.35) * 12.0);
    float tW = exp(-(idx - 0.75) * (idx - 0.75) * 10.0);
    float total = bW + mW + tW + 0.001;
    return (bassEnv() * bW + midEnv() * mW + trebleEnv() * tW) / total;
}

float pitch(float freq, float scale) {
    return clamp(getFreq(freq) * sens() * scale, 0.0, 1.0);
}

float vol() { return clamp(levelEnv() * sens(), 0.0, 1.2); }

// ─── Utilities ───────────────────────────────────────────────

float sdBox(vec3 p, vec3 b) {
    vec3 q = abs(p) - b;
    return length(max(q, 0.0)) + min(max(q.x, max(q.y, q.z)), 0.0);
}

mat2 rot2(float a) {
    float c = cos(a), s = sin(a);
    return mat2(c, -s, s, c);
}

vec3 satBoost(vec3 c, float b) {
    float l = dot(c, vec3(0.2126, 0.7152, 0.0722));
    return clamp(vec3(l) + (c - vec3(l)) * (1.0 + b), 0.0, 1.15);
}

// ─── Color Schemes ───────────────────────────────────────────
// Three-color cycling for richer palettes. Each scheme has an anchor,
// complement, and accent that cycle at different rates for organic
// color diversity without monotone flatness.
// Combo indices: Aurora(0), Cyberpunk(1), Lava(2), Prism(3), Toxic(4), Vaporwave(5)

vec3 scheme(vec3 id, float t) {
    vec3 a, b, c;
    if (iColorScheme == 1) {         // Cyberpunk
        a = vec3(0.98, 0.05, 0.78); b = vec3(0.05, 0.82, 1.15); c = vec3(0.92, 0.48, 0.08);
    } else if (iColorScheme == 2) {  // Lava
        a = vec3(1.2, 0.15, 0.03);  b = vec3(1.0, 0.55, 0.04);  c = vec3(0.85, 0.18, 0.65);
    } else if (iColorScheme == 0) {  // Aurora
        a = vec3(0.1, 0.85, 0.6);   b = vec3(0.25, 0.35, 1.2);  c = vec3(0.72, 0.25, 0.92);
    } else if (iColorScheme == 5) {  // Vaporwave
        a = vec3(1.1, 0.35, 0.85);  b = vec3(0.3, 0.85, 1.15);  c = vec3(0.85, 0.72, 0.2);
    } else if (iColorScheme == 4) {  // Toxic
        a = vec3(0.3, 1.15, 0.15);  b = vec3(0.05, 0.6, 0.95);  c = vec3(0.88, 0.95, 0.08);
    } else {                         // Prism (full rainbow)
        return satBoost(0.6 + 0.6 * cos(id * 0.8 + vec3(0.0, 2.0, 4.0) + t), 0.9);
    }
    // Three-phase cycling — a↔b at one rate, blend toward c at another
    float phase1 = 0.5 + 0.5 * sin(t + dot(id, vec3(0.7, 0.3, 0.55)));
    float phase2 = 0.5 + 0.5 * sin(t * 0.73 + dot(id, vec3(0.35, 0.6, 0.25)) + 2.1);
    vec3 ab = mix(a, b, phase1);
    vec3 col = mix(ab, c, phase2 * 0.45);
    col += 0.12 * sin(vec3(0.0, 2.0, 4.0) + t * 0.65 + id.yzx * 0.7);
    return satBoost(col, 0.85);
}

// ─── Style 1: Particle Field ────────────────────────────────
// Ethereal lattice of glowing cubes flowing through 3D space.
// Audio drives cube size and camera movement — not brightness.

vec3 pulseFieldStyle(vec2 uv, float time) {
    float v = vol();
    float energy = motionEnergy();
    float pulse = motionPulse();
    float swell = clamp(iAudioSwell * 0.65 + pulse * 0.25, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum * 0.8, -1.0, 1.0);
    float bass = clamp(bassEnv() * sens(), 0.0, 1.5);
    float mid = clamp(midEnv() * sens(), 0.0, 1.3);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();
    float zoom = max(iMotionZoom, 0.82);

    // Thrust cycle — slow periodic window, gated by audio swell, so flight
    // episodically collapses the lateral wander into forward push. Prevents
    // constant lateral noise while still giving occasional straight-through runs.
    float thrustCycle = 0.5 + 0.5 * sin(iWarpPhase * 0.08 + time * 0.09);
    float thrust = smoothstep(0.5, 0.88, thrustCycle) *
                   (0.35 + swell * 0.45 + energy * 0.35 + pulse * 0.25) * thrustN();
    float swayScale = 1.0 - thrust * 0.7;

    // Flow direction with audio momentum
    float totalFlow = clamp(fl + momentum * 0.42 + pulse * 0.12 * sign(fl + momentum + 0.001), -1.8, 1.8);
    float travelDir = totalFlow >= 0.0 ? -1.0 : 1.0;

    // Camera — audio drives position and speed. Thrust mode amplifies forward
    // advance and pulls xy sway toward zero for dramatic straight flight.
    float drift = time * totalFlow * (1.2 + swell * 0.95 + v * 0.35 + energy * 0.55 + pulse * 0.3 + thrust * 0.8)
                + iFlowDrift * 3.4;
    vec3 ro = vec3(
        sin(time * 0.20 + iWarpPhase * 0.22) * (0.82 + swell * 0.32 + v * 0.18 + energy * 0.26 + pulse * 0.18) * swayScale
            + iMotionPan.x * (3.1 - thrust * 1.9),
        cos(time * 0.17 - iWarpPhase * 0.18) * (0.64 + mid * 0.18 + energy * 0.18) * swayScale
            + iMotionPan.y * (2.6 - thrust * 1.6),
        drift + travelDir * (0.58 + bass * 0.92 + pulse * 0.42 + energy * 0.28 + thrust * 1.3)
    );

    vec2 view = (uv + iMotionPan * 0.22) * ((0.78 - energy * 0.06) / zoom);
    vec3 rd = normalize(vec3(view, 0.9 + bass * 0.18 + v * 0.1 + pulse * 0.08 + thrust * 0.22));
    rd.xy = rot2(time * 0.06 + iMotionTwist * 0.72 + momentum * 0.16 + energy * 0.08) * rd.xy;

    // Curve cycle — separate slow oscillator for path curvature. Combined
    // with thrust, this gives runs that arc through space rather than sway.
    float curveCycle = 0.5 + 0.5 * sin(iWarpPhase * 0.11 + time * 0.063);
    float curveAmp = (0.12 + energy * 0.06 + pulse * 0.05) * (0.45 + curveCycle * 1.35) * curveN();

    vec3 col = vec3(0.0);
    float travel = 0.0;

    for (int i = 0; i < 70; i++) {
        vec3 p = ro + rd * travel * travelDir;

        // Depth-dependent arc — positions bend as we march deeper, creating
        // curved flight paths through the cell grid.
        float depth = travel * travelDir;
        p.xy += vec2(sin(depth * 0.17 + time * 0.19 + iWarpPhase * 0.14),
                     cos(depth * 0.13 - time * 0.15 + iWarpPhase * 0.11)) * curveAmp;

        // Subtle flow field distortion
        float sw = time * 0.15 + iWarpPhase * 0.5 + dot(p, vec3(0.05, 0.07, 0.09));
        p += vec3(sin(sw), cos(sw + iWarpPhase * 0.35), 0.0) * (0.1 + abs(totalFlow) * 0.12 + energy * 0.06 + pulse * 0.08) * (1.0 - thrust * 0.4);
        p.z += bass * 0.14 + sin(iWarpPhase + p.x * 0.4) * (0.05 + energy * 0.03 + pulse * 0.02);

        vec3 cell = floor(p);
        vec3 local = fract(p) - 0.5;

        // Per-cell audio: drives SIZE
        float freqIdx = fract(dot(cell, vec3(0.31, 0.21, 0.13)) * 0.25);
        float amp = pitch(freqIdx, 1.0 + v * 0.3);
        float size = 0.18 + amp * 0.22 + mid * 0.08 + pulse * 0.04 + energy * 0.03;

        float d = sdBox(local, vec3(size));
        float dist = max(d, 0.0);

        // Glow from SDF distance — brightness is geometric, not audio-driven.
        // Tighter inner falloff keeps colored cores from blending into white.
        float sparkle = exp(-dist * 22.0) / (0.68 + dist * 32.0);
        // Color cycling uses iTime (not motion time) so color speed is
        // independent of the Speed control — colorSpeed already handles it.
        vec3 sc = scheme(cell + vec3(0.0, 0.0, drift * 0.25), iTime * cs * 0.5 + freqIdx * 2.0);
        vec3 contrib = sc * sparkle * 0.082 * g * (0.5 + energy * 0.06);
        // Soft front-to-back: attenuate by luminance already accumulated so
        // dense regions stay saturated instead of summing to gray.
        col += contrib * (1.0 - 0.35 * clamp(dot(col, vec3(0.333)), 0.0, 1.0));

        travel += max(abs(d), 0.038) / (1.0 + energy * 0.12 + pulse * 0.08);
        if (travel > 34.0) break;
    }

    return col;
}

// ─── Style 0: Twist ──────────────────────────────────────────
// Warping corridor of reactive cubes. Depth-dependent curves bend the
// flight path through the grid; audio drives box size and flight speed.

vec3 twistStyle(vec2 uv, float time) {
    float v = vol();
    float energy = motionEnergy();
    float pulse = motionPulse();
    float swell = clamp(iAudioSwell * 0.65 + pulse * 0.22, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum * 0.8, -1.0, 1.0);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();
    float zoom = max(iMotionZoom, 0.82);

    // Thrust — episodic forward push, gated by warp-phase cycle and audio.
    float thrustCycle = 0.5 + 0.5 * sin(iWarpPhase * 0.09 + time * 0.075);
    float thrust = smoothstep(0.48, 0.9, thrustCycle) *
                   (0.3 + swell * 0.4 + energy * 0.32 + pulse * 0.22) * thrustN();
    float swayScale = 1.0 - thrust * 0.65;

    float flow = clamp(fl + momentum * 0.42 + pulse * 0.18 * sign(fl + 0.001), -1.8, 1.8);
    float flowDir = flow >= 0.0 ? -1.0 : 1.0;
    float speed = (1.35 + abs(flow) * 1.6 + swell * 0.82 + v * 0.5 + energy * 0.8 + pulse * 0.45)
                * (1.0 + thrust * 0.55);
    float drive = 0.95 + pulse * 0.24 + bassEnv() * 0.14 + energy * 0.18;

    vec2 view = (uv + iMotionPan * 0.28) * ((0.7 + v * 0.1 + energy * 0.06) / zoom);
    vec3 rd = normalize(vec3(view, flowDir * (1.02 + swell * 0.18 + thrust * 0.3)));
    rd.xy = rot2(time * 0.08 + iMotionTwist * 0.56 + momentum * 0.14 + pulse * 0.08) * rd.xy;

    vec3 ro = vec3(iMotionPan.x * (2.6 - thrust * 1.5), iMotionPan.y * (2.2 - thrust * 1.3),
                   time * speed * flowDir + iFlowDrift * 2.8);
    ro.xy += vec2(sin(time * 0.21 + iWarpPhase * 0.25), cos(time * 0.16 - iWarpPhase * 0.2))
           * (0.28 + v * 0.18 + energy * 0.12) * swayScale;
    ro.xy += rot2(time * 0.06 + pulse * 0.25) * vec2(momentum * 0.52 + pulse * 0.18, swell * 0.34 + energy * 0.12) * swayScale;

    // Curve cycle — slow oscillator, independent of thrust, so the path bends
    // through space in long arcs even when thrust is off.
    float curveCycle = 0.5 + 0.5 * sin(iWarpPhase * 0.13 + time * 0.068);
    float curveAmp = (0.18 + energy * 0.08 + pulse * 0.05) * (0.4 + curveCycle * 1.4) * curveN();

    vec3 col = vec3(0.0);

    for (float i = 0.0, t = 0.0; i < 60.0; i++) {
        vec3 p = ro + t * rd;
        // Signature positional twist — rotate xy by a depth-phased angle.
        p.xy = rot2(sin(time * 0.05 + p.z * 0.32 + pulse * 0.6) * (0.26 + energy * 0.08)) * p.xy;
        // Curved flight path — depth-dependent arc layered on top of the twist.
        p.xy += vec2(sin(p.z * 0.22 + time * 0.19 + iWarpPhase * 0.15),
                     cos(p.z * 0.17 - time * 0.14 + iWarpPhase * 0.12)) * curveAmp;

        vec3 id = floor(p);
        vec3 q = fract(p) - 0.5;

        float freqIdx = mod(abs(id.x) + abs(id.y) * 2.0 + abs(id.z) * 0.5, 32.0) / 32.0;
        float amp = pitch(freqIdx, 1.0 + v * 0.3);

        // Audio drives box SIZE
        float boxSize = 0.2 + amp * 0.22 + pulse * 0.05 + energy * 0.03;
        float d = sdBox(q, vec3(boxSize));
        float fade = exp(-t * 0.06);

        // Geometric glow — tighter falloff so adjacent cells of different hues
        // don't stack additively into gray. Each ray should hit ~1-2 bright cells.
        float crisp = exp(-max(d, 0.0) * 28.0);
        float edge = smoothstep(0.09, 0.0, abs(d)) * 0.22;
        float glow = (crisp * 0.55 + edge * 0.25) * (0.42 + energy * 0.06);

        // iTime (not motion time) so Color Speed is independent of motion Speed.
        vec3 sc = scheme(id, iTime * cs * 0.5 + freqIdx * 3.0);
        // Front-to-back style alpha: sample contribution is attenuated by what's
        // already been accumulated. Prevents the raymarch from summing into white.
        vec3 contrib = sc * glow * fade * g * 0.42;
        col += contrib * (1.0 - 0.45 * clamp(dot(col, vec3(0.333)), 0.0, 1.0));

        t += max(abs(d), 0.042) / (drive * (1.0 + abs(flow) * 0.28 + energy * 0.12 + pulse * 0.08));
        if (t > 35.0) break;
    }

    return col;
}

// ─── Main ────────────────────────────────────────────────────

void mainImage(out vec4 fragOut, vec2 fragCoord) {
    vec2 uv = (2.0 * fragCoord - iResolution.xy) / iResolution.y;

    // Pass iMotionTime (speed-scaled) so the Speed control scales all motion
    // consistently. iTime is referenced directly for color cycling below.
    vec3 color = (iVisualStyle == 0)
        ? twistStyle(uv, iMotionTime)
        : pulseFieldStyle(uv, iMotionTime);

    // Saturation-preserving tonemap — compress on the max channel so color
    // ratios survive. Per-channel Reinhard flattens hues toward white when
    // additive raymarching produces elevated values across all three channels;
    // operating on the max channel keeps the R:G:B ratio intact.
    float peak = max(max(color.r, color.g), max(color.b, 1e-4));
    float scaled = peak / (1.0 + peak * 0.42);
    color *= scaled / peak;

    // Chroma recovery — tonemap gently desaturates, boost it back up so
    // vivid hues stay vivid on screen and on physical LEDs.
    color = satBoost(color, 0.28);

    fragOut = vec4(clamp(color, 0.0, 1.0), 1.0);
}

void main() {
    mainImage(fragColor, gl_FragCoord.xy);
}
