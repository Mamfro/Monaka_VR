// Reprojection: a rendered frame (colour at its own size, depth at the render size, from its own
// camera and projection) redrawn from another camera, the same on D3D11 (`reproject.rs`) and D3D12
// (`reproject12.rs`); each defines IMAGE, the type its images are written as, before this source.
// Each source colour pixel takes its depth at the same place of the depth image (UV scaled onto the
// part of it that lines up with the image, from its origin, moved by the jitter the depth was
// rendered with): the nearest of the four depth texels around that place, so a colour pixel on the
// blended edge of a near thing (the colour is finer than the depth) moves with that thing and not
// with what is behind it, which would draw its outline where the thing was. It goes through one
// matrix (target projection * target view * source camera * inverse source projection) to its
// target pixel; the nearest wins a depth test (Splat keys, then Paint the winners). A key is the
// nearness and, below it, how close the pixel landed to the target pixel's centre, so of pixels at
// one depth exactly one wins, the same one every frame. Two layers are made this way: the primary (the
// real frame of this moment, from the other eye) and the fill (this eye's own previous real frame).
// Whether a fill pixel may be trusted is asked of the primary's eye itself (Seen): the fill's point,
// taken back into the primary's frame (Back, at the primary's depth area PrimaryDepth), is either
// hidden there (that eye sees something nearer at its place: it could not see the point, so the
// fill alone knows it), seen past (that eye sees farther there: the point is no longer there, a hand
// or what it holds that has moved since the fill's frame), or outside that eye's view.
// Resolve takes the primary, or the fill where the fill is nearer and hidden from the primary's eye
// (what that eye saw beside a surface it could not see slid into the surface's place: the far wall
// seen through a bar where an arrow hid the bar), then the primary's own surface across a crack in
// it (Crack), then the fill where the primary left a gap unless the fill's point was seen past, then
// the surface behind the gap, the farther of its horizontal neighbours (Behind). With FillFirst, a
// pixel where the fill shows the same surface as the primary (their
// nearness agrees) takes the fill: this eye's own last real frame, as the eye rendered it, so a real
// frame and a warped one alternate without a blink; the primary stands where something moved or was
// uncovered. ResolveDepth does the same with the keys alone, at the output depth's own size, and
// writes the target's depth in its own convention (depth = (nearness - NearB) / NearA).
// The output's alpha says where each pixel came from, in thirds (a 2-bit alpha holds it too): 1 the
// primary, 2/3 the fill, 1/3 the primary's own neighbour (across a crack or behind a gap), 0
// nothing. The headset ignores alpha; captures read it.
cbuffer Reproject : register(b0) {
    row_major float4x4 M;
    uint W, H, SW, SH;
    uint DW, DH, OW, OH;  // the source depth's area that lines up with the image, the output depth's
    float NearA, NearB;   // target nearness = NearA * depth + NearB
    float2 Jitter;        // the source depth's jitter, in its pixels (the content moved by it)
    uint2 DepthOrigin;    // where the area starts in the source depth and in the output depth
    uint FillFirst;       // the fill where it shows the primary's surface (see above)
    uint Sources;         // a debug view instead of the image (see Shown)
    row_major float4x4 Back;  // resolves: the target's clip -> the primary's (Seen)
    uint4 PrimaryDepth;       // resolves: the primary's depth area in Depth (origin, size; size 0: none)
};
Texture2D<float4> Color : register(t0);
Texture2D<float> Depth : register(t1);
Texture2D<float4> Primary : register(t2);
Texture2D<float4> Fill : register(t3);
Texture2D<uint> PrimaryKeys : register(t4);
Texture2D<uint> FillKeys : register(t5);
RWTexture2D<uint> Keys : register(u0);
RWTexture2D<IMAGE> Image : register(u1);
RWTexture2D<IMAGE> Out : register(u2);
RWTexture2D<float> OutDepth : register(u3);

// A key: 22 bits of nearness over 10 bits of closeness to the target pixel's centre, plus one.
static const float NEARNESS_STEPS = 4194302.0;
static const uint CLOSENESS_BITS = 10;
static const float CLOSENESS_STEPS = 1023.0;

// Of the four depth texels around `at` (in texels of the area at DepthOrigin of DW x DH), the
// nearest: a blended edge pixel goes with the near surface.
float NearestDepth(float2 at) {
    int2 base = int2(floor(at - 0.5));
    float best = 0; float nearest = -1;
    [unroll] for (int j = 0; j < 2; ++j) {
        [unroll] for (int i = 0; i < 2; ++i) {
            int2 texel = clamp(base + int2(i, j), int2(0, 0), int2(DW, DH) - 1) + int2(DepthOrigin);
            float d = Depth.Load(int3(texel, 0));
            float n = NearA * d + NearB;
            if (n > nearest) { nearest = n; best = d; }
        }
    }
    return best;
}

[numthreads(8, 8, 1)] void Clear(uint3 id : SV_DispatchThreadID) {
    if (id.x < W && id.y < H) { Keys[id.xy] = 0; Image[id.xy] = float4(0, 0, 0, 0); }
}

// Where source pixel `p` lands in the target, and its key (larger is nearer; 0 means none).
bool Land(uint2 p, out uint2 target, out uint key) {
    target = uint2(0, 0); key = 0;
    float2 at = (float2(p) + 0.5) * float2(DW, DH) / float2(SW, SH) + Jitter;
    float d = NearestDepth(at);
    float2 ndc = float2((p.x + 0.5) / SW * 2 - 1, 1 - (p.y + 0.5) / SH * 2);
    float4 clip = mul(M, float4(ndc, d, 1));
    if (!(clip.w > 0)) return false;
    float3 t = clip.xyz / clip.w;
    float2 pixel = float2((t.x + 1) * 0.5 * W, (1 - t.y) * 0.5 * H);
    if (!(pixel.x >= 0 && pixel.y >= 0 && pixel.x < W && pixel.y < H)) return false;
    float nearness = saturate(NearA * t.z + NearB);
    target = uint2(pixel);
    float2 off = pixel - (floor(pixel) + 0.5);
    uint closeness = uint((1 - saturate(dot(off, off) * 2)) * CLOSENESS_STEPS);
    key = ((uint(nearness * NEARNESS_STEPS) << CLOSENESS_BITS) | closeness) + 1;
    return true;
}

[numthreads(8, 8, 1)] void Splat(uint3 id : SV_DispatchThreadID) {
    if (id.x >= SW || id.y >= SH) return;
    uint2 target; uint key;
    if (Land(id.xy, target, key)) InterlockedMax(Keys[target], key);
}

[numthreads(8, 8, 1)] void Paint(uint3 id : SV_DispatchThreadID) {
    if (id.x >= SW || id.y >= SH) return;
    uint2 target; uint key;
    if (Land(id.xy, target, key) && Keys[target] == key) Image[target] = float4(Color.Load(int3(id.xy, 0)).rgb, 1);
}

// A key's nearness (0 for none).
uint Nearness(uint key) { return key ? (key - 1) >> CLOSENESS_BITS : 0; }

// Whether two keys show one surface: their nearness within a fiftieth.
bool SameSurface(uint a, uint b) {
    uint na = Nearness(a), nb = Nearness(b);
    uint far = min(na, nb), near = max(na, nb);
    return near - far <= near / 50;
}

// The surface behind a gap at target pixel `p`: of the first pixels the primary saw left and right of
// it within 64 pixels, the farther. Its column (-1: none) and its key.
int Behind(uint2 p, out uint key) {
    key = 0; int column = -1;
    [unroll] for (int side = -1; side <= 1; side += 2) {
        [loop] for (int r = 1; r <= 64; ++r) {
            int x = int(p.x) + side * r;
            if (x < 0 || x >= int(W)) break;
            uint k = PrimaryKeys.Load(int3(x, p.y, 0));
            if (k) { if (column < 0 || Nearness(k) < Nearness(key)) { key = k; column = x; } break; }
        }
    }
    return column;
}

bool Inside(int2 p) { return p.x >= 0 && p.y >= 0 && p.x < int(W) && p.y < int(H); }

// What the primary's eye saw of the fill's point at target pixel `p` (key `fill`), taken back into
// the primary's frame (see the top): HIDDEN, something nearer at its place; PAST, something farther
// (the point is not there any more); OUTSIDE, out of that eye's view or no primary depth; SAME, the
// point itself. The primary's depth is in the target's convention (both eyes share depth terms).
static const uint HIDDEN = 0, PAST = 1, OUTSIDE = 2, SAME = 3;
uint Seen(uint2 p, uint fill) {
    if (PrimaryDepth.z == 0 || PrimaryDepth.w == 0) return OUTSIDE;
    float here = Nearness(fill) / NEARNESS_STEPS;
    float2 ndc = float2((p.x + 0.5) / W * 2 - 1, 1 - (p.y + 0.5) / H * 2);
    float4 s = mul(Back, float4(ndc, (here - NearB) / NearA, 1));
    if (!(s.w > 0)) return OUTSIDE;
    s /= s.w;
    if (any(abs(s.xy) >= 1)) return OUTSIDE;
    int2 texel = clamp(int2(float2((s.x + 1) * 0.5, (1 - s.y) * 0.5) * float2(PrimaryDepth.zw)), int2(0, 0), int2(PrimaryDepth.zw) - 1) + int2(PrimaryDepth.xy);
    float seen = NearA * Depth.Load(int3(texel, 0)) + NearB, at = NearA * s.z + NearB;
    if (seen > at + at / 8) return HIDDEN;
    if (seen + at / 8 < at) return PAST;
    return SAME;
}

// A crack: a target pixel no source pixel landed on inside one of the primary's surfaces (a surface
// drawn larger than in its source, nearer or turned toward this eye, leaves such pixels in a grid).
// Told from a gap beside a nearer surface by primary pixels at about the same depth on both sides of
// it within two pixels, along a row, a column or a diagonal; a gap has the nearer surface on one side
// only. The nearest such pixel, or (-1, -1).
int2 Crack(uint2 p) {
    const int2 dirs[4] = { int2(1, 0), int2(0, 1), int2(1, 1), int2(1, -1) };
    uint best_near = 0; int2 best = int2(-1, -1);
    [unroll] for (int d = 0; d < 4; ++d) {
        uint ka = 0, kb = 0; int2 a = int2(-1, -1), b = int2(-1, -1);
        [unroll] for (int r = 1; r <= 2; ++r) {
            int2 q = int2(p) + dirs[d] * r, s = int2(p) - dirs[d] * r;
            if (!ka && Inside(q)) { ka = PrimaryKeys.Load(int3(q, 0)); a = q; }
            if (!kb && Inside(s)) { kb = PrimaryKeys.Load(int3(s, 0)); b = s; }
        }
        uint na = Nearness(ka), nb = Nearness(kb);
        uint nearer = max(na, nb), farther = min(na, nb);
        if (ka && kb && farther >= nearer - nearer / 5 && nearer > best_near) { best_near = nearer; best = na >= nb ? a : b; }
    }
    return best;
}

// What a pixel shows, its colour `color` and key `key` taken from `from` (0 the primary, 1 the fill,
// 2 a neighbour of the primary's, 3 nothing). Sources 1 tints it by where it came from: the primary
// as it is, the fill green, a neighbour solid blue (no game colour matches it exactly), nothing
// magenta. Sources 2 shows its nearness as grey (white near, a square root so far things still
// differ), the colour gone. Sources 3 shows the fill alone and 4 the primary alone (what the other
// left, magenta).
float4 Shown(float4 color, uint key, uint from) {
    float source = (3 - from) / 3.0;  // the alpha (see the top)
    if (Sources == 1) {
        const float3 tints[4] = { float3(0, 0, 0), float3(0, 1, 0), float3(0, 0, 1), float3(1, 0, 1) };
        return float4(from == 0 ? color.rgb : lerp(color.rgb, tints[from], from >= 2 ? 1.0 : 0.6), source);
    }
    if (Sources == 2) return float4(sqrt(key ? ((key - 1) >> CLOSENESS_BITS) / NEARNESS_STEPS : 0).xxx, source);
    return float4(color.rgb, source);
}

// Whether the fill's pixel at `p` (key `fill`) goes out over the primary's (key `primary`): the
// same surface with FillFirst, or a clearly nearer one the primary's eye could not see (hidden).
bool FillOver(uint2 p, uint fill, uint primary) {
    if (!fill) return false;
    if (FillFirst && SameSurface(fill, primary)) return true;
    uint nf = Nearness(fill), np = Nearness(primary);
    return np + nf / 8 < nf && Seen(p, fill) == HIDDEN;
}

// Whether the fill's pixel at `p` (key `fill`) fills the gap there: anything but a point the
// primary's eye saw past (gone since).
bool FillsGap(uint2 p, uint fill) {
    return fill && Seen(p, fill) != PAST;
}

[numthreads(8, 8, 1)] void Resolve(uint3 id : SV_DispatchThreadID) {
    if (id.x >= W || id.y >= H) return;
    if (Sources == 3) {
        Out[id.xy] = FillKeys.Load(int3(id.xy, 0)) ? Fill.Load(int3(id.xy, 0)) : float4(1, 0, 1, 1);
        return;
    }
    if (Sources == 4) {
        Out[id.xy] = PrimaryKeys.Load(int3(id.xy, 0)) ? Primary.Load(int3(id.xy, 0)) : float4(1, 0, 1, 1);
        return;
    }
    uint primary = PrimaryKeys.Load(int3(id.xy, 0));
    if (primary) {
        uint fill = FillKeys.Load(int3(id.xy, 0));
        Out[id.xy] = FillOver(id.xy, fill, primary) ? Shown(Fill.Load(int3(id.xy, 0)), fill, 1) : Shown(Primary.Load(int3(id.xy, 0)), primary, 0);
        return;
    }
    int2 crack = Crack(id.xy);
    if (crack.x >= 0) { Out[id.xy] = Shown(Primary.Load(int3(crack, 0)), PrimaryKeys.Load(int3(crack, 0)), 2); return; }
    uint fill = FillKeys.Load(int3(id.xy, 0));
    if (FillsGap(id.xy, fill)) { Out[id.xy] = Shown(Fill.Load(int3(id.xy, 0)), fill, 1); return; }
    uint behind; int column = Behind(id.xy, behind);
    Out[id.xy] = column >= 0 ? Shown(Primary.Load(int3(column, id.y, 0)), behind, 2) : Shown(float4(0, 0, 0, 1), 0, 3);
}

[numthreads(8, 8, 1)] void ResolveDepth(uint3 id : SV_DispatchThreadID) {
    if (id.x >= OW || id.y >= OH) return;
    uint2 p = uint2(min((id.x * 2 + 1) * W / (2 * OW), W - 1), min((id.y * 2 + 1) * H / (2 * OH), H - 1));
    uint key = PrimaryKeys.Load(int3(p, 0));
    uint fill = FillKeys.Load(int3(p, 0));
    if (key && FillOver(p, fill, key)) key = fill;
    if (!key) {
        int2 crack = Crack(p);
        if (crack.x >= 0) key = PrimaryKeys.Load(int3(crack, 0));
    }
    if (!key) {
        uint behind; int column = Behind(p, behind);
        key = FillsGap(p, fill) ? fill : column >= 0 ? behind : 1;
    }
    float nearness = ((key - 1) >> CLOSENESS_BITS) / NEARNESS_STEPS;
    OutDepth[id.xy + DepthOrigin] = (nearness - NearB) / NearA;
}
