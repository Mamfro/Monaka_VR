// Motion vectors for temporal upscaling, from depth: each pixel goes through one matrix (previous
// projection * previous view * this camera * inverse of this projection, all of one eye) to where
// it was in that eye's previous frame. A far depth of 0 (reversed infinite) unprojects to a
// direction, so the sky moves with the rotation only. A game's own object motion (what moved in
// the world, the camera left out) is added scaled to UV units. Out: UV-space vectors from this
// frame to the previous one (y down), and the depth as R32 for the upscaler.
cbuffer Motion : register(b0) {
    row_major float4x4 M;
    uint W, H;                // render size (the vectors' and depth output's)
    uint DW, DH;              // the depth texture's size
    float2 ObjectScale;       // object motion to UV units (0: none)
    uint HasObject;
    uint Pad;
};
Texture2D<float> Depth : register(t0);
Texture2D<float2> Object : register(t1);
RWTexture2D<float2> Motion : register(u0);
RWTexture2D<float> DepthOut : register(u1);

[numthreads(8, 8, 1)] void Main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= W || id.y >= H) return;
    float d = Depth.Load(int3(id.x * DW / W, id.y * DH / H, 0));
    float2 ndc = float2((id.x + 0.5) / W * 2 - 1, 1 - (id.y + 0.5) / H * 2);
    float4 previous = mul(M, float4(ndc, d, 1));
    float2 mv = float2(0, 0);
    if (previous.w > 1e-6) {
        float2 p = previous.xy / previous.w;
        mv = float2(p.x - ndc.x, ndc.y - p.y) * 0.5;
    }
    if (HasObject) mv += Object.Load(int3(id.xy, 0)) * ObjectScale;
    Motion[id.xy] = mv;
    DepthOut[id.xy] = d;
}
