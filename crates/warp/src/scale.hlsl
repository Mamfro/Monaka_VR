// An image resampled (bilinear) into Rect (x, y, width, height in pixels) of a W x H output, black
// around it.
cbuffer Scale : register(b0) {
    uint W, H, SW, SH;
    float4 Rect;
};
Texture2D<float4> Source : register(t0);
RWTexture2D<unorm float4> Out : register(u0);

float4 At(int2 p) {
    return Source.Load(int3(clamp(p, int2(0, 0), int2(SW - 1, SH - 1)), 0));
}

[numthreads(8, 8, 1)] void Scale(uint3 id : SV_DispatchThreadID) {
    if (id.x >= W || id.y >= H) return;
    float2 p = (float2(id.xy) + 0.5 - Rect.xy) / Rect.zw;
    float3 c = float3(0, 0, 0);
    if (all(p >= 0) && all(p < 1)) {
        float2 s = p * float2(SW, SH) - 0.5;
        int2 i = int2(floor(s));
        float2 f = s - floor(s);
        float4 top = lerp(At(i), At(i + int2(1, 0)), f.x);
        float4 bottom = lerp(At(i + int2(0, 1)), At(i + int2(1, 1)), f.x);
        c = lerp(top, bottom, f.y).rgb;
    }
    Out[id.xy] = float4(c, 1);
}
