// The home scene: each pixel of a capture's cube map drawn as a tile at its own distance
// (scene.rs, scene/files.rs), as one eye sees it. The pixels drawn come from `tiles` (none: all
// of them, in order: `grid.w` 1), each one's corners from `corners` (the same order, four 16-bit
// distance codes); each pixel's own distance, for its neighbours, from `distances`. Colours are
// the capture now and the next, faded by `fade.x`.

StructuredBuffer<uint2> corners : register(t0);
StructuredBuffer<uint> tiles : register(t1);
Texture2D now_colour : register(t2);
Texture2D next_colour : register(t3);
Texture2D<float> distances : register(t4);

cbuffer Scene : register(b0) {
    float4 view0;     // room to eye space, rows
    float4 view1;
    float4 view2;
    float4 tangents;  // the image edges: left, right, up, down
    float4 offset;    // the capture point (xyz), and how much further its tiles are pushed (w)
    uint4 grid;       // pixels a face, pixels across the strip, dither steps (0: none), every
                      // pixel drawn in order (w: 1, no list)
    float4 fade;      // how far into the next hour (x), the near plane (y), dither strength (z),
                      // the target sRGB (w: 1, so drawn linear)
};

// Each face's direction out, its image's up and its right (scene/files.rs: LOOK, TOP, RIGHT).
static const float3 LOOK[6] = { float3(0, 0, -1), float3(1, 0, 0), float3(0, 0, 1), float3(-1, 0, 0), float3(0, 1, 0), float3(0, -1, 0) };
static const float3 TOP[6] = { float3(0, 1, 0), float3(0, 1, 0), float3(0, 1, 0), float3(0, 1, 0), float3(0, 0, 1), float3(0, 0, -1) };
static const float3 RIGHT[6] = { float3(1, 0, 0), float3(0, 0, 1), float3(-1, 0, 0), float3(0, 0, -1), float3(1, 0, 0), float3(1, 0, 0) };

// A neighbour at least this fraction of a pixel's distance is on its surface or behind it
// (scene/files.rs: SAME_SURFACE, 8% in logs).
static const float SAME_SURFACE = 0.923;

// A distance as 16 bits (scene/files.rs: distance_code): 0 unknown (the sky), else even steps
// of its log from 1 cm to the sky.
static const float NEAREST_LOG = -4.6051702;  // ln 0.01
static const float LOG_SPAN = 11.5129255;     // ln 1000 - ln 0.01
static const float SKY = 1000.0;

float distance_from(uint code) {
    return code == 0 ? SKY : exp(NEAREST_LOG + (code - 1) / 65534.0 * LOG_SPAN);
}

// Corner `corner` (top left, top right, bottom left, bottom right) of the tile whose codes are
// `packed`.
float corner_distance(uint2 packed, uint corner) {
    uint pair = corner < 2 ? packed.x : packed.y;
    return distance_from((corner & 1) == 0 ? pair & 0xffff : pair >> 16);
}

float mean_distance(uint2 packed) {
    return 0.25 * (corner_distance(packed, 0) + corner_distance(packed, 1) + corner_distance(packed, 2) + corner_distance(packed, 3));
}

// Two triangles a tile; corners top left, top right, bottom left, bottom right.
static const uint ORDER[6] = { 0, 1, 2, 2, 1, 3 };

struct Out {
    float4 position : SV_Position;
    float2 texel : TEXCOORD0;                    // where in the strip, in pixels
    nointerpolation float face_left : TEXCOORD1; // the face's left edge in the strip
    nointerpolation int2 own : TEXCOORD2;        // the pixel this tile is, in the strip
};

Out vs_main(uint vertex : SV_VertexID) {
    uint index = vertex / 6;
    uint pixel = grid.w != 0 ? index : tiles[index];
    uint corner = ORDER[vertex % 6];
    uint n = grid.x;
    uint x = pixel % grid.y;
    uint y = pixel / grid.y;
    uint face = x / n;
    float distance = corner_distance(corners[index], corner);
    float2 at = float2(x % n + (corner & 1), y + (corner >> 1));
    float3 dir = normalize(LOOK[face] + (2 * at.x / n - 1) * RIGHT[face] + (1 - 2 * at.y / n) * TOP[face]);
    float4 p = float4(offset.xyz + dir * distance * offset.w, 1);
    float3 v = float3(dot(view0, p), dot(view1, p), dot(view2, p));
    float w = -v.z;
    Out o;
    o.position = float4((2 * v.x - (tangents.x + tangents.y) * w) / (tangents.y - tangents.x),
                        (2 * v.y - (tangents.z + tangents.w) * w) / (tangents.z - tangents.w),
                        fade.y, w); // depth near / distance ahead: nearer is larger
    o.texel = float2(face * n + at.x, at.y);
    o.face_left = face * n;
    o.own = int2(x, y);
    return o;
}

// A skirt (scene/files.rs: skirts): the edge between a pixel and the one right of it (or below,
// bit 31 set), as a quad from the edge on one tile to the same edge on the other; coloured as the
// farther of the two.
Out vs_skirt(uint vertex : SV_VertexID) {
    uint entry = tiles[vertex / 6];
    uint corner = ORDER[vertex % 6];
    bool below = (entry >> 31) != 0;
    uint first = entry & 0x7fffffff;
    uint second = below ? first + grid.y : first + 1;
    uint n = grid.x;
    uint x = first % grid.y;
    uint y = first / grid.y;
    uint face = x / n;
    // The edge's two ends (corners 0 and 1 of the quad on the first tile, 2 and 3 on the second),
    // and which corner of each tile they are.
    uint end = corner & 1;
    bool on_second = corner >= 2;
    float2 at = below ? float2(x % n + end, y + 1) : float2(x % n + 1, y + end);
    uint of_first = below ? 2 + end : 1 + 2 * end;  // bottom left/right, or top/bottom right
    uint of_second = below ? end : 2 * end;         // top left/right, or top/bottom left
    uint pixel = on_second ? second : first;
    // The middle draws every pixel in order: a pixel's corners are its own entry.
    float distance = corner_distance(corners[pixel], on_second ? of_second : of_first);
    float3 dir = normalize(LOOK[face] + (2 * at.x / n - 1) * RIGHT[face] + (1 - 2 * at.y / n) * TOP[face]);
    float4 p = float4(offset.xyz + dir * distance * offset.w, 1);
    float3 v = float3(dot(view0, p), dot(view1, p), dot(view2, p));
    float w = -v.z;
    uint farther = mean_distance(corners[second]) > mean_distance(corners[first]) ? second : first;
    Out o;
    o.position = float4((2 * v.x - (tangents.x + tangents.y) * w) / (tangents.y - tangents.x),
                        (2 * v.y - (tangents.z + tangents.w) * w) / (tangents.z - tangents.w),
                        fade.y, w);
    o.texel = float2(farther % grid.y + 0.5, farther / grid.y + 0.5);
    o.face_left = face * n;
    o.own = int2(farther % grid.y, farther / grid.y);
    return o;
}

// An 8x8 Bayer matrix, values 0..63.
static const float BAYER8[64] = {
     0, 32,  8, 40,  2, 34, 10, 42,
    48, 16, 56, 24, 50, 18, 58, 26,
    12, 44,  4, 36, 14, 46,  6, 38,
    60, 28, 52, 20, 62, 30, 54, 22,
     3, 35, 11, 43,  1, 33,  9, 41,
    51, 19, 59, 27, 49, 17, 57, 25,
    15, 47,  7, 39, 13, 45,  5, 37,
    63, 31, 55, 23, 61, 29, 53, 21 };

// One capture pixel as shown: the hour now faded into the next, then dithered as the game's
// stylized display does it: each channel to `grid.z` steps (none: 0), offset by the Bayer
// threshold of the pixel's place in its face, `fade.z` strong. In display sRGB, as captured.
float3 shown(int2 at, int face_left) {
    float3 c = lerp(now_colour.Load(int3(at, 0)).rgb, next_colour.Load(int3(at, 0)).rgb, fade.x);
    if (grid.z == 0) {
        return c;
    }
    int2 in_face = int2(at.x - face_left, at.y);
    float threshold = (BAYER8[(in_face.y & 7) * 8 + (in_face.x & 7)] + 0.5) / 64.0 - 0.5;
    float steps = grid.z;
    return floor(c * steps + 0.5 + threshold * fade.z) / steps;
}

// How far away a capture pixel is (the mean of its tile's corners, as `distances` holds it).
float distance_of(int2 at) {
    return distance_from((uint)round(distances.Load(int3(at, 0)) * 65535.0));
}

// A neighbour's colour, if it may show at this tile's edge: one on the same surface, or behind
// it. A nearer one's may not: from anywhere but the capture point the near thing has moved off
// this tile's edge, and its colour there would be left on (a frame's outline on the sky).
float3 beside(int2 at, int face_left, float own_distance, float3 own) {
    return distance_of(at) >= own_distance * SAME_SURFACE ? shown(at, face_left) : own;
}

float4 ps_main(Out i) : SV_Target {
    int n = grid.x;
    int face_left = (int)i.face_left;
    // Hard pixels whose edges are smoothed over one pixel of the eye's image, so they do not
    // shimmer as the head moves: inside a pixel the sample sits on its middle; across an edge it
    // slides to the next pixel's middle within the width of one eye pixel. Each pixel is shown
    // (and dithered) on its own, then the (up to) four round the sample are blended.
    float2 seam = floor(i.texel + 0.5);
    float2 across = max(fwidth(i.texel), 1e-4);
    float2 s = seam + clamp((i.texel - seam) / across, -0.5, 0.5);
    float2 base = floor(s - 0.5);
    float2 w = s - 0.5 - base;
    int2 lo = int2(face_left, 0);
    int2 hi = int2(face_left + n - 1, n - 1);
    int2 a = clamp(int2(base), lo, hi);
    int2 b = clamp(int2(base) + 1, lo, hi);
    float own_distance = distance_of(i.own);
    float3 own = shown(i.own, face_left);
    float3 top = lerp(beside(a, face_left, own_distance, own), beside(int2(b.x, a.y), face_left, own_distance, own), w.x);
    float3 bottom = lerp(beside(int2(a.x, b.y), face_left, own_distance, own), beside(b, face_left, own_distance, own), w.x);
    float3 c = lerp(top, bottom, w.y);
    // To the target: linear for an sRGB one (`fade.w` 1), else as it is.
    if (fade.w != 0) {
        c = c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
    }
    return float4(c, 1);
}
