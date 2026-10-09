// Reprojection: a rendered frame (colour + depth, from its own camera and projection) redrawn from
// another camera. Each source pixel goes through one matrix (target projection * target view *
// source camera * inverse source projection) to its target pixel; the nearest wins a depth test
// (Splat keys, then Paint the winners). Two layers are made this way: the primary (the real frame
// of this moment, from the other eye) and the fill (this eye's own previous real frame). Resolve
// takes the primary, then the fill where the primary left a gap, then a neighbour where both did.
cbuffer Reproject : register(b0) {
    row_major float4x4 M;
    uint W, H, SW, SH;
    uint DW, DH;
    float NearA, NearB;   // target nearness = NearA * depth + NearB
};
Texture2D<float4> Color : register(t0);
Texture2D<float> Depth : register(t1);
Texture2D<float4> Primary : register(t2);
Texture2D<float4> Fill : register(t3);
Texture2D<uint> PrimaryKeys : register(t4);
Texture2D<uint> FillKeys : register(t5);
RWTexture2D<uint> Keys : register(u0);
RWTexture2D<unorm float4> Out : register(u1);

[numthreads(8, 8, 1)] void Clear(uint3 id : SV_DispatchThreadID) {
    if (id.x < W && id.y < H) { Keys[id.xy] = 0; Out[id.xy] = float4(0, 0, 0, 0); }
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
    if (pixel.x < 0 || pixel.y < 0 || pixel.x >= W || pixel.y >= H) return false;
    float nearness = saturate(NearA * t.z + NearB);
    target = uint2(pixel);
    key = uint(nearness * 4294967040.0) + 1;
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
    if (Land(id.xy, target, key) && Keys[target] == key) Out[target] = float4(Color.Load(int3(id.xy, 0)).rgb, 1);
}

[numthreads(8, 8, 1)] void Resolve(uint3 id : SV_DispatchThreadID) {
    if (id.x >= W || id.y >= H) return;
    if (PrimaryKeys.Load(int3(id.xy, 0))) { Out[id.xy] = Primary.Load(int3(id.xy, 0)); return; }
    if (FillKeys.Load(int3(id.xy, 0))) { Out[id.xy] = Fill.Load(int3(id.xy, 0)); return; }
    // Neither saw it: continue the farther (background) horizontal neighbour of the primary.
    uint best = 0xffffffff; int source = -1;
    [loop] for (int r = 1; r <= 64 && source < 0; ++r) {
        int xs[2] = { int(id.x) - r, int(id.x) + r };
        [unroll] for (int i = 0; i < 2; ++i) {
            if (xs[i] < 0 || xs[i] >= int(W)) continue;
            uint k = PrimaryKeys.Load(int3(xs[i], id.y, 0));
            if (k && k < best) { best = k; source = xs[i]; }
        }
    }
    Out[id.xy] = source >= 0 ? Primary.Load(int3(source, id.y, 0)) : float4(0, 0, 0, 1);
}
