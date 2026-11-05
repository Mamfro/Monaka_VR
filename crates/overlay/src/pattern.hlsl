// An abstract home background (pattern.rs), painted into a cube map strip as the captures are: one
// pixel per texel of the room, in display sRGB, which the scene pass then draws, dithers and
// smooths as it does a captured room's. The room is a dome with a floor below the head; each
// texel is the surface where its direction from the middle meets it (pattern.rs: room).

cbuffer Pattern : register(b0) {
    uint4 grid;   // pixels a face, which pattern (0 Drift)
    float4 time;  // seconds (x), kept small so the drift stays smooth
};

static const float PI = 3.14159265;
// The room (pattern.rs: DOME, FLOOR).
static const float DOME = 14.0;
static const float FLOOR_Y = -1.6;

// Each face's direction out, its image's up and its right (scene/files.rs: LOOK, TOP, RIGHT).
static const float3 LOOK[6] = { float3(0, 0, -1), float3(1, 0, 0), float3(0, 0, 1), float3(-1, 0, 0), float3(0, 1, 0), float3(0, -1, 0) };
static const float3 TOP[6] = { float3(0, 1, 0), float3(0, 1, 0), float3(0, 1, 0), float3(0, 1, 0), float3(0, 0, 1), float3(0, 0, -1) };
static const float3 RIGHT[6] = { float3(1, 0, 0), float3(0, 0, 1), float3(-1, 0, 0), float3(0, 0, -1), float3(1, 0, 0), float3(1, 0, 0) };

float4 vs_main(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}

// Where the room's surface is in direction q from the middle: the floor, or the dome.
float3 surface(float3 q) {
    if (q.y < 0) {
        float3 p = q * (FLOOR_Y / q.y);
        if (dot(p.xz, p.xz) < DOME * DOME - FLOOR_Y * FLOOR_Y) {
            return p;
        }
    }
    return q * DOME;
}

bool on_floor(float3 p) {
    return p.y <= FLOOR_Y + 1e-3;
}

// Light added up, eased so it never burns out to white.
float3 soft(float3 c) {
    return 1 - exp(-c);
}

// ---- Drift: soft colours drifting round the dome ----

static const int BLOBS = 9;
static const float3 COLOURS[BLOBS] = {
    float3(0.70, 0.14, 0.20),  // azuki
    float3(0.30, 0.58, 0.26),  // matcha
    float3(0.95, 0.60, 0.24),  // toasted wafer
    float3(0.52, 0.20, 0.66),  // plum
    float3(0.10, 0.50, 0.62),  // sea
    float3(0.20, 0.22, 0.75),  // dusk
    float3(0.90, 0.36, 0.46),  // sakura
    float3(0.62, 0.70, 0.20),  // yuzu
    float3(0.16, 0.36, 0.48),  // deep teal
};
static const float SPEED[BLOBS] = { 0.021, -0.017, 0.013, -0.025, 0.019, -0.011, 0.015, -0.023, 0.009 };
static const float PHASE[BLOBS] = { 0.0, 1.3, 2.6, 3.7, 4.9, 5.8, 0.7, 2.1, 4.2 };
static const float HEIGHT[BLOBS] = { 0.10, 0.45, 0.02, 0.70, 0.28, 1.40, 0.55, 0.15, 0.95 };
static const float SHARP[BLOBS] = { 14.0, 18.0, 10.0, 16.0, 12.0, 9.0, 20.0, 15.0, 11.0 };

float3 drift_sky(float3 q, float t) {
    float3 c = lerp(float3(0.03, 0.025, 0.06), float3(0.07, 0.05, 0.11), saturate(q.y * 0.5 + 0.5));
    [unroll] for (int k = 0; k < BLOBS; k++) {
        float a = t * SPEED[k] + PHASE[k];
        // Each blob wanders round and bobs up and down, and breathes a little as it goes.
        float3 centre = normalize(float3(cos(a), HEIGHT[k] + 0.3 * sin(a * 0.73 + k), sin(a)));
        float sharp = SHARP[k] * (1 + 0.25 * sin(a * 1.9 + k * 2.3));
        c += COLOURS[k] * 1.6 * exp((dot(q, centre) - 1) * sharp);
    }
    return soft(c);
}

float3 drift(float3 q, float3 p, float t) {
    if (!on_floor(p)) {
        return drift_sky(q, t);
    }
    // The floor: a dim mirror of the sky, fading out towards the dome's foot.
    float3 mirrored = normalize(float3(q.x, -q.y, q.z));
    float near = exp(-dot(p.xz, p.xz) / 90);
    return drift_sky(mirrored, t) * (0.3 + 0.3 * near);
}

float4 ps_main(float4 position : SV_Position) : SV_Target {
    uint n = grid.x;
    uint x = (uint)position.x;
    uint y = (uint)position.y;
    uint face = x / n;
    float u = x % n + 0.5;
    float v = y + 0.5;
    float3 q = normalize(LOOK[face] + (2 * u / n - 1) * RIGHT[face] + (1 - 2 * v / n) * TOP[face]);
    float3 p = surface(q);
    float3 c = drift(q, p, time.x);
    return float4(saturate(c), 1);
}
