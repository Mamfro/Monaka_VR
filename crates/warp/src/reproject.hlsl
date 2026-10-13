// Reprojection: a rendered frame (colour at its own size, depth at the render size, from its own
// camera and projection) redrawn from another camera, the same on D3D11 (`reproject.rs`) and D3D12
// (`reproject12.rs`); each defines IMAGE, the type its images are written as, before this source.
// Each source colour pixel takes its depth at the same place of the depth image (UV scaled) and
// goes through one matrix (target projection * target view * source camera * inverse source
// projection) to its target pixel; the nearest wins a depth test (Splat keys, then Paint the
// winners). Two layers are made this way: the primary (the real frame of this moment, from the
// other eye) and the fill (this eye's own previous real frame). Resolve takes the primary, then the
// primary's own surface across a crack in it (Crack), then the fill where the primary left a gap
// (unless the fill is nearer than the surface behind the gap: a near thing that moved since the fill's
// frame, a hand or what it holds, would show where the other eye sees it), then the surface behind
// the gap, the farther of its horizontal neighbours (Behind). ResolveDepth does the same with the keys alone, at the output depth's own size, and writes the
// target's depth in its own convention (depth = (nearness - NearB) / NearA).
cbuffer Reproject : register(b0) {
    row_major float4x4 M;
    uint W, H, SW, SH;
    uint DW, DH, OW, OH;  // source depth size, output depth size
    float NearA, NearB;   // target nearness = NearA * depth + NearB
    uint Pad0, Pad1;
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

static const float KEY_SCALE = 4294967040.0;

[numthreads(8, 8, 1)] void Clear(uint3 id : SV_DispatchThreadID) {
    if (id.x < W && id.y < H) { Keys[id.xy] = 0; Image[id.xy] = float4(0, 0, 0, 0); }
}

// Where source pixel `p` lands in the target, and its key (larger is nearer; 0 means none).
bool Land(uint2 p, out uint2 target, out uint key) {
    target = uint2(0, 0); key = 0;
    float d = Depth.Load(int3(p.x * DW / SW, p.y * DH / SH, 0));
    float2 ndc = float2((p.x + 0.5) / SW * 2 - 1, 1 - (p.y + 0.5) / SH * 2);
    float4 clip = mul(M, float4(ndc, d, 1));
    if (!(clip.w > 0)) return false;
    float3 t = clip.xyz / clip.w;
    float2 pixel = float2((t.x + 1) * 0.5 * W, (1 - t.y) * 0.5 * H);
    if (!(pixel.x >= 0 && pixel.y >= 0 && pixel.x < W && pixel.y < H)) return false;
    float nearness = saturate(NearA * t.z + NearB);
    target = uint2(pixel);
    key = uint(nearness * KEY_SCALE) + 1;
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

// The surface behind a gap at target pixel `p`: of the first pixels the primary saw left and right of
// it within 64 pixels, the farther. Its column (-1: none) and its key.
int Behind(uint2 p, out uint key) {
    key = 0; int column = -1;
    [unroll] for (int side = -1; side <= 1; side += 2) {
        [loop] for (int r = 1; r <= 64; ++r) {
            int x = int(p.x) + side * r;
            if (x < 0 || x >= int(W)) break;
            uint k = PrimaryKeys.Load(int3(x, p.y, 0));
            if (k) { if (column < 0 || k < key) { key = k; column = x; } break; }
        }
    }
    return column;
}

// Whether the fill's pixel of key `fill` shows what is behind a gap whose surface behind is `behind`
// (column -1: none known): no nearer than it, by a quarter.
bool FillsGap(uint fill, int column, uint behind) {
    return fill && (column < 0 || fill <= behind || fill - behind <= behind / 4);
}

bool Inside(int2 p) { return p.x >= 0 && p.y >= 0 && p.x < int(W) && p.y < int(H); }

// A crack: a target pixel no source pixel landed on inside one of the primary's surfaces (a surface
// drawn larger than in its source, nearer or turned toward this eye, leaves such pixels in a grid).
// Told from a gap beside a nearer surface by primary pixels at about the same depth on both sides of
// it within two pixels, along a row, a column or a diagonal; a gap has the nearer surface on one side
// only. The nearest such pixel, or (-1, -1).
int2 Crack(uint2 p) {
    const int2 dirs[4] = { int2(1, 0), int2(0, 1), int2(1, 1), int2(1, -1) };
    uint best_key = 0; int2 best = int2(-1, -1);
    [unroll] for (int d = 0; d < 4; ++d) {
        uint ka = 0, kb = 0; int2 a = int2(-1, -1), b = int2(-1, -1);
        [unroll] for (int r = 1; r <= 2; ++r) {
            int2 q = int2(p) + dirs[d] * r, s = int2(p) - dirs[d] * r;
            if (!ka && Inside(q)) { ka = PrimaryKeys.Load(int3(q, 0)); a = q; }
            if (!kb && Inside(s)) { kb = PrimaryKeys.Load(int3(s, 0)); b = s; }
        }
        uint nearer = max(ka, kb), farther = min(ka, kb);
        if (farther && farther >= nearer - nearer / 5 && nearer > best_key) { best_key = nearer; best = ka >= kb ? a : b; }
    }
    return best;
}

[numthreads(8, 8, 1)] void Resolve(uint3 id : SV_DispatchThreadID) {
    if (id.x >= W || id.y >= H) return;
    if (PrimaryKeys.Load(int3(id.xy, 0))) { Out[id.xy] = Primary.Load(int3(id.xy, 0)); return; }
    int2 crack = Crack(id.xy);
    if (crack.x >= 0) { Out[id.xy] = Primary.Load(int3(crack, 0)); return; }
    uint behind; int column = Behind(id.xy, behind);
    if (FillsGap(FillKeys.Load(int3(id.xy, 0)), column, behind)) { Out[id.xy] = Fill.Load(int3(id.xy, 0)); return; }
    Out[id.xy] = column >= 0 ? Primary.Load(int3(column, id.y, 0)) : float4(0, 0, 0, 1);
}

[numthreads(8, 8, 1)] void ResolveDepth(uint3 id : SV_DispatchThreadID) {
    if (id.x >= OW || id.y >= OH) return;
    uint2 p = uint2(min((id.x * 2 + 1) * W / (2 * OW), W - 1), min((id.y * 2 + 1) * H / (2 * OH), H - 1));
    uint key = PrimaryKeys.Load(int3(p, 0));
    if (!key) {
        int2 crack = Crack(p);
        if (crack.x >= 0) key = PrimaryKeys.Load(int3(crack, 0));
    }
    if (!key) {
        uint behind; int column = Behind(p, behind);
        uint fill = FillKeys.Load(int3(p, 0));
        key = FillsGap(fill, column, behind) ? fill : column >= 0 ? behind : 1;
    }
    float nearness = (key - 1) / KEY_SCALE;
    OutDepth[id.xy] = (nearness - NearB) / NearA;
}
