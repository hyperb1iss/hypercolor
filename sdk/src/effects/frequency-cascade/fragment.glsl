#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Audio — engine-provided
uniform float iAudioLevel;
uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioBeatPulse;
uniform float iAudioSwell;
uniform float iAudioBrightness;
uniform float iAudioHarmonicHue;

// Effect-smoothed envelopes (main.ts owns the asymmetric attack/decay)
uniform float iCascadeLevel;
uniform float iCascadeBass;
uniform float iCascadeMid;
uniform float iCascadeTreble;
uniform float iCascadeSwell;
uniform float iCascadePresence;
uniform float iCascadeBeatBloom;
uniform float iCascadeFloor;

// Controls
uniform float iSpeed;
uniform float iIntensity;
uniform float iSmoothing;
uniform float iBarWidth;
uniform float iGlow;
uniform int iPalette;
uniform int iScene;

const float PI = 3.14159265359;
const float TAU = 6.28318530718;

// ── Noise ────────────────────────────────────────────────────────────

float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float valueNoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash12(i),                   hash12(i + vec2(1.0, 0.0)), f.x),
        mix(hash12(i + vec2(0.0, 1.0)),  hash12(i + vec2(1.0, 1.0)), f.x),
        f.y
    );
}

float fbm(vec2 p) {
    float v = 0.0, a = 0.5;
    for (int i = 0; i < 4; i++) {
        v += valueNoise(p) * a;
        p *= 2.03;
        a *= 0.5;
    }
    return v;
}

// ── Tonemap (Narkowicz ACES fit) ─────────────────────────────────────

vec3 ACESFilm(vec3 x) {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

// ── iq cosine palettes ───────────────────────────────────────────────

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(TAU * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.50, 0.28, 0.54), vec3(0.48, 0.45, 0.45), vec3(1.00, 0.85, 0.70), vec3(0.88, 0.18, 0.52));
    if (id == 1) return iqPalette(t, vec3(0.18, 0.50, 0.40), vec3(0.35, 0.40, 0.45), vec3(0.75, 0.70, 0.85), vec3(0.62, 0.30, 0.72));
    if (id == 2) return iqPalette(t, vec3(0.52, 0.18, 0.48), vec3(0.52, 0.45, 0.50), vec3(1.00, 1.00, 1.00), vec3(0.84, 0.10, 0.60));
    if (id == 3) return iqPalette(t, vec3(0.50, 0.22, 0.02), vec3(0.50, 0.40, 0.20), vec3(1.00, 0.72, 0.38), vec3(0.02, 0.16, 0.24));
    if (id == 4) return iqPalette(t, vec3(0.40, 0.16, 0.28), vec3(0.40, 0.26, 0.28), vec3(0.82, 0.68, 0.60), vec3(0.06, 0.24, 0.44));
    if (id == 5) return iqPalette(t, vec3(0.52, 0.60, 0.78), vec3(0.22, 0.30, 0.22), vec3(0.62, 0.82, 1.00), vec3(0.00, 0.10, 0.32));
    return iqPalette(t, vec3(0.50), vec3(0.50), vec3(1.00), vec3(0.00));
}

// ── Spectrum shaping ────────────────────────────────────────────────
// Same five-lobe shape as before — treats bass/mid/treble as spatial
// frequency regions, with beat transients damped toward smoothed level.

float spectralEnvelope(float freq, float smoothAmt) {
    float bassLobe  = exp(-pow((freq - 0.08) * 2.55, 2.0));
    float lowLobe   = exp(-pow((freq - 0.28) * 2.85, 2.0));
    float midLobe   = exp(-pow((freq - 0.52) * 2.95, 2.0));
    float highLobe  = exp(-pow((freq - 0.76) * 2.85, 2.0));
    float airLobe   = exp(-pow((freq - 0.94) * 3.20, 2.0));

    // Lower damping so bass/mid/treble differentiate boldly in 3D — height
    // variation needs to survive perspective foreshortening.
    float bassDamp = mix(0.12, 0.36, smoothAmt);
    float bandDamp = mix(0.08, 0.26, smoothAmt);
    float bass   = mix(iCascadeBass,   iCascadeLevel, bassDamp);
    float mid    = mix(iCascadeMid,    iCascadeLevel, bandDamp);
    float treble = mix(iCascadeTreble, iCascadeLevel, bandDamp);

    return
        bass                     * bassLobe  * 1.15 +
        mix(bass, mid, 0.40)     * lowLobe   * 0.96 +
        mid                      * midLobe   * 1.05 +
        mix(mid, treble, 0.55)   * highLobe  * 1.00 +
        treble                   * airLobe   * 0.88;
}

// Each row carries a time-delayed snapshot. Content literally flows from
// near to far: what cz=0 sees at time t is what cz=1 will see at t + Δ.
// Near rows blend in the current spectrum; far rows follow synthesized
// history sampled at (freq, rowTime = time − age/cascadeSpeed).
float rowEnergy(float freq, float rowAge, float time, float smoothAmt) {
    float current = spectralEnvelope(freq, smoothAmt);

    float cascadeSpeed = 0.60 + iSpeed * 0.22;
    float rowTime = time - rowAge / cascadeSpeed;

    // Synthesized past — fbm parameterized by emission time so the pattern
    // at each cell slides continuously from near to far.
    float past = fbm(vec2(freq * 4.5, rowTime * 0.55));
    float pastShape = (past - 0.45) * (0.55 + iCascadePresence * 0.45);

    // Rolling continuous wave — always-on visible cascade, independent of
    // audio transients. Travels outward at cascadeSpeed.
    float rollPhase = rowTime * 0.85 + freq * 0.25;
    float roll = sin(rollPhase * TAU * 0.28);
    roll = pow(max(roll, 0.0), 2.0);
    float rollShape = roll * (0.10 + iCascadeLevel * 0.28);

    // Audio-reactive base — near rows get strong current spectrum,
    // far rows drift into synthesized past.
    float ageDecay = exp(-rowAge * 0.18);
    float currentWithJitter = current + pastShape * 0.22;
    float farShape = iCascadeLevel * 0.28 + pastShape + rollShape * 0.9;
    float energy = mix(farShape, currentWithJitter, ageDecay);

    // Always-on breath so near rows shimmer even with quiet audio
    energy += rollShape * ageDecay * 0.40;

    // Beat pulse — a bright Gaussian that travels outward from cz=0 on
    // every beat. Phase wraps but the traveling wave reads as periodic
    // kicks fanning through the grid.
    float beatPhase = fract(time * (0.55 + iSpeed * 0.08));
    float beatFront = beatPhase * 10.0;
    float beatWave  = exp(-pow((rowAge - beatFront) * 1.7, 2.0));
    energy += iCascadeBeatBloom * beatWave * 0.70 * (1.0 - freq * 0.45);

    float floorV = iCascadeFloor * mix(1.0, 0.40, smoothstep(0.0, 6.0, rowAge));
    energy = max(energy, floorV);
    energy = 1.18 * (1.0 - exp(-energy * 1.55));
    return clamp(energy, 0.0, 1.30);
}

// ── Grid ─────────────────────────────────────────────────────────────
// Cells indexed by (cx, cz): cx ∈ [0, bc-1] along X (frequency),
// cz ∈ [0, ∞) along -Z (time/row depth). Grid centered at X=0.

const float SX = 0.30;
const float SZ = 0.52;

int barCountI() {
    int base = int(floor(mix(72.0, 14.0, clamp(iBarWidth * 0.01, 0.0, 1.0))));
    // Odd count keeps a center bar on the X=0 sightline.
    return ((base & 1) == 0) ? base + 1 : base;
}

float barGapFrac() {
    return mix(0.36, 0.18, clamp(iGlow * 0.01, 0.0, 1.0));
}

bool skipCenterLane(int cx, int bc) {
    if (iScene != 3) return false;
    int c = bc / 2;
    // 5-cell gap — gives the banking/drifting tunnel camera clearance
    return cx >= c - 2 && cx <= c + 2;
}

vec3 cellWorldCenter(int cx, int cz, int bc) {
    float gridW = float(bc - 1) * SX;
    return vec3(float(cx) * SX - gridW * 0.5, 0.0, -float(cz) * SZ);
}

float cellBarHeight(int cx, int cz, int bc, float time, float smoothAmt) {
    if (cx < 0 || cx >= bc) return 0.0;
    if (cz < 0) return 0.0;
    if (skipCenterLane(cx, bc)) return 0.0;

    float freq = float(cx) / max(float(bc - 1), 1.0);
    float rowAge = float(cz) * SZ;

    float energy = rowEnergy(freq, rowAge, time, smoothAmt);

    // Per-bar independent twinkle — breaks the static "curve" feel
    // without decorrelating from the spectrum. Each bar breathes on
    // its own phase.
    float twinkle = 0.92 + 0.08 * sin(time * 4.5 + float(cx) * 11.7 + float(cz) * 2.3);
    energy *= twinkle;

    float intensity = clamp(iIntensity * 0.01, 0.0, 1.0);
    return 0.06 + energy * mix(0.55, 3.40, intensity);
}

// Ray-box slab intersection. Returns entry t and entry-face normal.
bool rayBox(vec3 ro, vec3 rd, vec3 bMin, vec3 bMax, out float tEnter, out vec3 nEnter) {
    // Guard against near-zero components to avoid 0 * inf NaN in the slab math.
    vec3 safeRd;
    safeRd.x = (abs(rd.x) > 1e-5) ? rd.x : ((rd.x >= 0.0) ? 1e-5 : -1e-5);
    safeRd.y = (abs(rd.y) > 1e-5) ? rd.y : ((rd.y >= 0.0) ? 1e-5 : -1e-5);
    safeRd.z = (abs(rd.z) > 1e-5) ? rd.z : ((rd.z >= 0.0) ? 1e-5 : -1e-5);

    vec3 inv = 1.0 / safeRd;
    vec3 t0 = (bMin - ro) * inv;
    vec3 t1 = (bMax - ro) * inv;
    vec3 tmn = min(t0, t1);
    vec3 tmx = max(t0, t1);
    float te = max(max(tmn.x, tmn.y), tmn.z);
    float tx = min(min(tmx.x, tmx.y), tmx.z);
    if (tx < 0.0 || te > tx) return false;
    tEnter = max(te, 0.0);

    if (tmn.x >= tmn.y && tmn.x >= tmn.z) nEnter = vec3(-sign(safeRd.x), 0.0, 0.0);
    else if (tmn.y >= tmn.z)              nEnter = vec3(0.0, -sign(safeRd.y), 0.0);
    else                                  nEnter = vec3(0.0, 0.0, -sign(safeRd.z));
    return true;
}

struct Hit {
    bool  hit;
    float tHit;
    vec3  pos;
    vec3  normal;
    int   cx;
    int   cz;
    float height;
    float freq;
    float rowAge;
    float side;   // +1 upper bar, -1 mirrored lower bar (Mirror scene only)
};

Hit emptyHit() {
    Hit h;
    h.hit = false; h.tHit = 1e9; h.pos = vec3(0.0); h.normal = vec3(0.0);
    h.cx = 0; h.cz = 0; h.height = 0.0; h.freq = 0.0; h.rowAge = 0.0; h.side = 1.0;
    return h;
}

// DDA over the 2D cell grid (X, -Z). Each visited cell: test ray-box
// against its bar (and mirrored bar below for Mirror scene). Return
// nearest hit.
Hit traceGrid(vec3 ro, vec3 rd, float time, float smoothAmt) {
    Hit h = emptyHit();

    int bc = barCountI();
    float gridW = float(bc - 1) * SX;
    float halfW = SX * 0.5 * (1.0 - barGapFrac());
    float halfD = SZ * 0.5 * (1.0 - barGapFrac());

    // Map to DDA-space (axis 0 = X, axis 1 = -Z)
    vec2 ro2 = vec2(ro.x, -ro.z);
    vec2 rd2 = vec2(rd.x, -rd.z);

    int cx = int(floor((ro2.x + gridW * 0.5) / SX + 0.5));
    int cz = int(floor(ro2.y / SZ + 0.5));

    vec2 step_ = sign(rd2);
    vec2 delta = vec2(SX, SZ) / max(abs(rd2), vec2(1e-5));

    vec2 cellCenter2 = vec2(float(cx) * SX - gridW * 0.5, float(cz) * SZ);
    vec2 rel = ro2 - cellCenter2;
    vec2 sideDist;
    sideDist.x = (step_.x > 0.0) ? (( SX * 0.5 - rel.x) / rd2.x)
               : (step_.x < 0.0) ? ((-SX * 0.5 - rel.x) / rd2.x) : 1e9;
    sideDist.y = (step_.y > 0.0) ? (( SZ * 0.5 - rel.y) / rd2.y)
               : (step_.y < 0.0) ? ((-SZ * 0.5 - rel.y) / rd2.y) : 1e9;
    sideDist = max(sideDist, vec2(1e-5));

    for (int i = 0; i < 200; i++) {
        float bh = cellBarHeight(cx, cz, bc, time, smoothAmt);
        if (bh > 0.015) {
            vec3 c = cellWorldCenter(cx, cz, bc);
            vec3 bMin = vec3(c.x - halfW, 0.0, c.z - halfD);
            vec3 bMax = vec3(c.x + halfW, bh,  c.z + halfD);
            float tHit;
            vec3 n;
            if (rayBox(ro, rd, bMin, bMax, tHit, n)) {
                h.hit = true;
                h.tHit = tHit;
                h.pos = ro + rd * tHit;
                h.normal = n;
                h.cx = cx; h.cz = cz;
                h.height = bh;
                h.freq = float(cx) / max(float(bc - 1), 1.0);
                h.rowAge = float(cz) * SZ;
                h.side = 1.0;
                return h;
            }

            // Mirror scene: also test the bar's mirror twin below Y=0.
            if (iScene == 1) {
                vec3 bMinM = vec3(c.x - halfW, -bh, c.z - halfD);
                vec3 bMaxM = vec3(c.x + halfW,  0.0, c.z + halfD);
                if (rayBox(ro, rd, bMinM, bMaxM, tHit, n)) {
                    h.hit = true;
                    h.tHit = tHit;
                    h.pos = ro + rd * tHit;
                    h.normal = n;
                    h.cx = cx; h.cz = cz;
                    h.height = bh;
                    h.freq = float(cx) / max(float(bc - 1), 1.0);
                    h.rowAge = float(cz) * SZ;
                    h.side = -1.0;
                    return h;
                }
            }
        }

        float tAdvance;
        if (sideDist.x < sideDist.y) {
            tAdvance = sideDist.x;
            sideDist.x += delta.x;
            cx += int(step_.x);
        } else {
            tAdvance = sideDist.y;
            sideDist.y += delta.y;
            cz += int(step_.y);
        }

        if (tAdvance > 90.0) break;
        if (cz > 2400) break;
        if (cx < -3 || cx > bc + 2) break;
    }

    return h;
}

// ── Background & ground ──────────────────────────────────────────────

// Volumetric backdrop: sky gradient + FBM nebula + sparse starfield.
// Fills the frame with colored detail instead of near-black.
vec3 background(vec3 rd, float time) {
    float up = rd.y;

    // Deep sky gradient — quiet, not glowing
    vec3 top     = paletteColor(0.55, iPalette) * 0.08;
    vec3 horizon = paletteColor(0.18, iPalette) * 0.14;
    vec3 below   = paletteColor(0.02, iPalette) * 0.035;
    vec3 sky;
    if (up > 0.0) sky = mix(horizon, top,   smoothstep(0.0, 0.55, up));
    else          sky = mix(horizon, below, smoothstep(0.0, 0.50, -up));

    // Dual-scale FBM nebula — slow drift, LOW contribution so it
    // reads as depth atmosphere rather than electric fog
    vec2 skyUV = rd.xy * 2.2;
    float nebA = fbm(skyUV * 1.2 + vec2(time * 0.025, -time * 0.014));
    float nebB = fbm(skyUV * 3.1 + vec2(-time * 0.018, time * 0.022));
    vec3 nebula = paletteColor(0.32 + time * 0.002, iPalette) * nebA * 0.08
                + paletteColor(0.72, iPalette)               * nebB * 0.05;

    // Sparse starfield — hashed cells, only bright cells emit
    vec2 starCell = floor(rd.xy * 42.0);
    float starSeed = hash12(starCell);
    vec2 starLocal = fract(rd.xy * 42.0) - 0.5;
    float starDist = length(starLocal);
    float starMask = step(0.985, starSeed);
    float twinkle  = 0.55 + 0.45 * sin(time * 2.6 + starSeed * 47.0);
    vec3 stars = vec3(1.0, 0.92, 1.0) * exp(-starDist * 90.0)
               * starMask * twinkle * (starSeed - 0.985) * 40.0;

    return sky + nebula + stars;
}

// Floor plane at Y=0. Returns t, or -1 if no intersection.
float groundT(vec3 ro, vec3 rd) {
    if (rd.y >= -1e-4) return -1.0;
    float t = -ro.y / rd.y;
    return (t > 0.0) ? t : -1.0;
}

vec3 shadeGround(vec3 ro, vec3 rd, float t, float time) {
    vec3 p = ro + rd * t;
    vec3 baseLow = paletteColor(0.08, iPalette) * 0.04;
    vec3 baseHi  = paletteColor(0.42, iPalette) * 0.09;
    float dist   = length(p.xz) * 0.07;
    float near   = exp(-dist);
    vec3 col = mix(baseLow, baseHi, near);

    // Faint bar-aligned grid lines underfoot — extends the scene in X/Z
    float gx = abs(fract(p.x / SX + 0.5) - 0.5);
    float gz = abs(fract(p.z / SZ + 0.5) - 0.5);
    float lineX = smoothstep(0.020, 0.002, gx);
    float lineZ = smoothstep(0.015, 0.002, gz);
    col += paletteColor(0.36, iPalette) * (lineX * 0.22 + lineZ * 0.16) * near;

    // Depth fog to horizon sky
    float fog = exp(-t * 0.045);
    return mix(background(rd, time), col, fog);
}

// ── Bar shading ──────────────────────────────────────────────────────

vec3 shadeBar(Hit h, vec3 ro, vec3 rd, float time, float smoothAmt) {
    float paletteT = h.freq * 0.75
                   + h.rowAge * 0.03
                   + iAudioHarmonicHue * 0.0008
                   + (iAudioBrightness - 0.5) * 0.10;
    vec3 baseColor   = paletteColor(paletteT,        iPalette);
    vec3 accentColor = paletteColor(paletteT + 0.22, iPalette);
    vec3 peakColor   = paletteColor(paletteT + 0.42, iPalette);

    float energy = rowEnergy(h.freq, h.rowAge, time, smoothAmt);

    // Mirror flips local Y: use |pos.y| for yNorm so both sides read
    // "base → top" the same way.
    float yNorm = clamp(abs(h.pos.y) / max(h.height, 0.001), 0.0, 1.0);
    bool isCap = abs(h.normal.y) > 0.5;

    vec3 color;
    if (isCap) {
        // Top/bottom cap — emissive peak, bright but not clipping
        color = peakColor * (0.75 + energy * 1.45);
        float topN = fbm(h.pos.xz * 14.0 + time * 0.25);
        color *= 0.90 + 0.25 * topN;
    } else {
        // Side face — vivid but not electric. Keeps gradient readable.
        vec3 lowC = mix(baseColor, accentColor, 0.35) * 0.55;
        vec3 hiC  = mix(accentColor, peakColor, 0.70) * 0.90;
        color = mix(lowC, hiC, yNorm);

        // LED row segmentation (horizontal groutlines — crisp, not blurry)
        float rows = mix(26.0, 12.0, smoothAmt);
        float rowCell = fract(yNorm * rows);
        float grout = smoothstep(0.03, 0.0, rowCell)
                    * smoothstep(0.03, 0.0, 1.0 - rowCell);
        color *= mix(1.0, 0.48, grout);

        // Faint vertical channel lines (one or two seams per side)
        vec2 seamCoord = vec2(h.freq * 80.0, h.cz);
        float seam = step(0.85, fbm(seamCoord)) * 0.10;
        color *= 1.0 - seam;

        // Face lighting: +Z normal = facing camera (brightest)
        float front = max( h.normal.z, 0.0);
        float back  = max(-h.normal.z, 0.0);
        float xSide = abs(h.normal.x);
        float lighting = 0.50 + front * 0.50 + xSide * 0.28 + back * 0.10;
        color *= lighting;

        color *= 0.55 + energy * 1.15;
    }

    // Mirror bars: dim and cool-tint to read as a reflection, not a real bar.
    if (h.side < 0.0) {
        color *= 0.42;
        color = mix(color, color * vec3(0.78, 0.92, 1.12), 0.30);
    }

    // SHARP top-edge rim — thin and clean, no haze halo.
    float topY = (h.side > 0.0) ? h.height : -h.height;
    float rimDist = abs(h.pos.y - topY);
    float rim = smoothstep(0.020, 0.0, rimDist);
    color += peakColor * rim
           * (0.40 + energy * 0.72)
           * (0.28 + iGlow * 0.006)
           * ((h.side > 0.0) ? 1.0 : 0.55);

    // Beat kiss on caps only — keeps it crisp
    if (isCap) color += accentColor * iCascadeBeatBloom * 0.28 * ((h.side > 0.0) ? 1.0 : 0.5);

    // Depth fog — attenuate to background, no additive glow
    float fog = exp(-h.tHit * 0.045);
    color = mix(background(rd, time), color, fog);

    return color;
}

// ── Main ─────────────────────────────────────────────────────────────

void main() {
    vec2 p = (gl_FragCoord.xy - 0.5 * iResolution) / iResolution.y;
    float aspect = iResolution.x / max(iResolution.y, 1.0);

    float time = iTime;
    float smoothAmt = clamp(iSmoothing * 0.01, 0.0, 1.0);

    // ── Cinematic cameras ───────────────────────────────────────────
    // Continuous forward glide: camera z decreases monotonically with
    // time, so we actually travel OVER the grid instead of oscillating
    // in place. Music drives speed, rise, and beat-punch kicks.
    float musicDrive = clamp(iCascadeLevel * 1.2 + iCascadePresence * 0.75, 0.0, 1.4);
    float beatKick   = iCascadeBeatBloom;
    float bassBump   = iCascadeBass;
    float swellLift  = iCascadeSwell;
    float trebleHiss = iCascadeTreble;

    vec3 ro, ta;
    vec3 camUp = vec3(0.0, 1.0, 0.0);
    float fov = 1.45;

    // Shared: target always LEADS the camera forward by a lookahead
    // that grows with music presence. Creates anticipatory framing.
    float look = 7.5 + iCascadePresence * 2.4;

    if (iScene == 0) {
        // Cascade — classic flyover. Camera X drifts small; target
        // yaw sweeps BIG so head-turn reads as rotation. Monotonic
        // forward creates real travel.
        float cruise = 0.38 + iSpeed * 0.06 + musicDrive * 0.65;
        float forward = -time * cruise - beatKick * 1.6;
        float T = time * 0.18;
        float riseRaw = 0.5 + 0.5 * sin(T * 0.33);
        float rise = smoothstep(0.0, 1.0, riseRaw) * 1.30 + swellLift * 0.60;
        float camX = sin(T * 0.27) * 0.45 + sin(T * 0.53 + 1.1) * 0.18;
        float bassShudder = bassBump * 0.32 * sin(time * 5.5);
        ro = vec3(camX + trebleHiss * 0.12 * sin(time * 9.0),
                  1.85 + rise + bassShudder,
                  3.0 + forward);
        // Target yaw: big lateral swing, separate phase from camera
        float yaw = sin(T * 0.42) * 2.40
                  + sin(T * 0.77 + 0.6) * 0.85
                  + sin(T * 1.35) * 0.25;
        ta = vec3(yaw,
                  0.60 + sin(T * 0.28) * 0.60 + bassBump * 0.45,
                  forward - look);
    } else if (iScene == 1) {
        // Mirror — slow skim. Target yaw moderate so the reflection's
        // angle is always reframing.
        float cruise = 0.24 + iSpeed * 0.045 + musicDrive * 0.42;
        float forward = -time * cruise - beatKick * 1.0;
        float T = time * 0.14;
        float liftRaw = 0.5 + 0.5 * sin(T * 0.35);
        float lift = 2.20 + smoothstep(0.0, 1.0, liftRaw) * 1.45 + swellLift * 0.50;
        float camX = sin(T * 0.31) * 0.55;
        ro = vec3(camX,
                  lift + bassBump * 0.25,
                  3.4 + forward);
        float yaw = sin(T * 0.45) * 2.05
                  + sin(T * 0.79 + 0.8) * 0.65
                  + sin(T * 1.42) * 0.18;
        ta = vec3(yaw,
                  0.85 + sin(T * 0.27) * 0.70,
                  forward - look - 0.5);
    } else if (iScene == 2) {
        // Horizon — stately forward drift. Big yaw looks across
        // the skyline horizon-to-horizon.
        float cruise = 0.18 + iSpeed * 0.035 + musicDrive * 0.32;
        float forward = -time * cruise - beatKick * 1.2;
        float T = time * 0.10;
        float rideRaw = 0.5 + 0.5 * sin(T * 0.28);
        float ride = 0.55 + smoothstep(0.0, 1.0, rideRaw) * 2.95 + swellLift * 0.70;
        float camX = sin(T * 0.26) * 0.35;
        ro = vec3(camX,
                  ride + bassBump * 0.30,
                  3.0 + forward);
        float yaw = sin(T * 0.38) * 2.10
                  + cos(T * 0.67) * 0.80
                  + sin(T * 1.3) * 0.22;
        ta = vec3(yaw,
                  1.05 + sin(T * 0.29 + 0.4) * 0.45,
                  forward - look - 3.0);
        fov = 1.25;
    } else {
        // Tunnel — banking corridor flight. Camera drift stays in
        // gap; target yaw constrained to ±0.7 so we keep looking
        // down the corridor, not into walls. Roll banks with yaw
        // direction for aircraft feel.
        float cruise = 0.60 + iSpeed * 0.10 + musicDrive * 0.95;
        float forward = -time * cruise - beatKick * 2.2;
        float T = time * 0.22;
        float drift = sin(T * 0.55) * 0.28
                    + sin(T * 0.83) * 0.10;
        float vBob  = sin(T * 0.45) * 0.20 + bassBump * 0.22 * sin(time * 6.0);
        ro = vec3(drift + sin(time * 0.85) * 0.04
                        + trebleHiss * 0.06 * sin(time * 11.0),
                  1.15 + vBob + swellLift * 0.30,
                  2.0 + forward);
        float yaw = sin(T * 0.48) * 0.55 + sin(T * 0.91) * 0.20;
        float bank  = yaw * -0.55
                    + sin(T * 1.3) * 0.06;
        ta = vec3(yaw,
                  1.05 + sin(T * 0.61 + 0.3) * 0.22,
                  forward - look - 4.0);
        camUp = vec3(sin(bank), cos(bank), 0.0);
        fov = 1.32;
    }

    vec3 ww = normalize(ta - ro);
    vec3 uu = normalize(cross(ww, camUp));
    vec3 vv = cross(uu, ww);
    vec3 rd = normalize(p.x * uu + p.y * vv + fov * ww);

    Hit h = traceGrid(ro, rd, time, smoothAmt);
    float tG = groundT(ro, rd);

    vec3 color;
    bool gridFirst  = h.hit && (tG <= 0.0 || h.tHit < tG);
    bool groundOnly = (!h.hit || (tG > 0.0 && tG < h.tHit)) && tG > 0.0;
    if (gridFirst) {
        color = shadeBar(h, ro, rd, time, smoothAmt);
    } else if (groundOnly) {
        color = shadeGround(ro, rd, tG, time);
    } else {
        color = background(rd, time);
    }

    // ── Scene accents ───────────────────────────────────────────────
    if (iScene == 1) {
        // Mirror: thin horizon seam at the waterline
        float seam = exp(-abs(rd.y + 0.015) * 34.0);
        color += mix(paletteColor(0.40, iPalette), paletteColor(0.60, iPalette), 0.5)
               * seam * (0.08 + iCascadePresence * 0.14);
    } else if (iScene == 2) {
        // Horizon: a bright sunline
        float horiz = exp(-abs(rd.y - 0.04) * 42.0);
        color += mix(paletteColor(0.35, iPalette), paletteColor(0.68, iPalette), 0.5)
               * horiz * (0.14 + iCascadePresence * 0.22);
    } else if (iScene == 3) {
        // Tunnel: soft center-vanishing core (the vanishing point of the corridor)
        float centerDist = length(p * vec2(1.0 / max(aspect, 0.001), 1.0));
        float core = exp(-centerDist * 4.2);
        color += paletteColor(0.30, iPalette)
               * core * (0.14 + iCascadePresence * 0.20 + iCascadeBeatBloom * 0.10);
    }

    // Vignette (soft — don't pinch edges, fluid motion needs space)
    float vig = 1.0 - smoothstep(0.70, 1.45, length(p * vec2(aspect, 1.0)));
    color *= mix(0.88, 1.0, vig);

    // Gentle per-channel Reinhard — preserves saturation in mid-tones
    // (ACES desaturates too aggressively for synthwave vibe). Small
    // saturation lift after compression keeps bars electric.
    color = color / (1.0 + color * 0.36);
    vec3 luma = vec3(dot(color, vec3(0.2126, 0.7152, 0.0722)));
    color = mix(luma, color, 1.18);
    color = pow(max(color, 0.0), vec3(0.93));

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
