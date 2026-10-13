// Reprojection: a rendered frame (colour at its own size, depth at the render size, from its own
// camera and projection) redrawn from another camera, the same on D3D11 (`reproject.rs`) and D3D12
// (`reproject12.rs`); each defines IMAGE, the type its images are written as, before this source.
// Each source colour pixel takes its depth at the same place of the depth image (UV scaled) and
// goes through one matrix (target projection * target view * source camera * inverse source
// projection) to its target pixel; the nearest wins a depth test (Splat keys, then Paint the
// winners). Two layers are made this way: the primary (the real frame of this moment, from the
// other eye) and the fill (this eye's own previous real frame). Resolve takes the primary, then the
// fill where the primary left a gap, then the farther horizontal neighbour where both did.
// ResolveDepth does the same with the keys alone, at the output depth's own size, and writes the
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

// The farther (background) horizontal neighbour of target pixel `p` the primary saw, within 64
// pixels: its column, or -1.
int Neighbour(uint2 p) {
    uint best = 0xffffffff; int source = -1;
    [loop] for (int r = 1; r <= 64 && source < 0; ++r) {
        int xs[2] = { int(p.x) - r, int(p.x) + r };
        [unroll] for (int i = 0; i < 2; ++i) {
            if (xs[i] < 0 || xs[i] >= int(W)) continue;
            uint k = PrimaryKeys.Load(int3(xs[i], p.y, 0));
            if (k && k < best) { best = k; source = xs[i]; }
        }
    }
    return source;
}

[numthreads(8, 8, 1)] void Resolve(uint3 id : SV_DispatchThreadID) {
    if (id.x >= W || id.y >= H) return;
    if (PrimaryKeys.Load(int3(id.xy, 0))) { Out[id.xy] = Primary.Load(int3(id.xy, 0)); return; }
    if (FillKeys.Load(int3(id.xy, 0))) { Out[id.xy] = Fill.Load(int3(id.xy, 0)); return; }
    int source = Neighbour(id.xy);
    Out[id.xy] = source >= 0 ? Primary.Load(int3(source, id.y, 0)) : float4(0, 0, 0, 1);
}

[numthreads(8, 8, 1)] void ResolveDepth(uint3 id : SV_DispatchThreadID) {
    if (id.x >= OW || id.y >= OH) return;
    uint2 p = uint2(min((id.x * 2 + 1) * W / (2 * OW), W - 1), min((id.y * 2 + 1) * H / (2 * OH), H - 1));
    uint key = PrimaryKeys.Load(int3(p, 0));
    if (!key) key = FillKeys.Load(int3(p, 0));
    if (!key) {
        int source = Neighbour(p);
        key = source >= 0 ? PrimaryKeys.Load(int3(source, p.y, 0)) : 1;
    }
    float nearness = (key - 1) / KEY_SCALE;
    OutDepth[id.xy] = (nearness - NearB) / NearA;
}
