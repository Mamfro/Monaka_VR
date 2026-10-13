// Motion vectors rebased onto the same eye's previous frame (alternate-eye stereo: the game's
// previous frame is the other eye's). In The Beast the game's vectors, in pixels, times
// mvecScale are the UV motion from now to the previous frame (y down), and for a still scene they
// equal the camera's motion through clipToPrevClip exactly. So per pixel:
//   camera part toward the game's previous  = through GameToPrev (the game's clipToPrevClip)
//   camera part toward this eye's previous  = through SameToPrev (two frames back)
//   object part = the game's vector minus its camera part, doubled (two frames instead of one)
// Matrices are row-major with row vectors, as Streamline's.

cbuffer Constants : register(b0)
{
    row_major float4x4 GameToPrev;
    row_major float4x4 SameToPrev;
    row_major float4x4 Unused2;
    uint2 Size;
    float2 MvecScale;
    float ObjectScale;
    float3 Pad;
};

Texture2D<float> Depth : register(t0);
Texture2D<float4> Motion : register(t1);
RWTexture2D<float4> Fixed : register(u0);

float2 CameraUv(float4 clip, float2 ndc, row_major float4x4 toPrev)
{
    float4 prev = mul(clip, toPrev);
    float2 prevNdc = prev.xy / prev.w;
    return float2(prevNdc.x - ndc.x, -(prevNdc.y - ndc.y)) * 0.5;
}

[numthreads(8, 8, 1)]
void Fix(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= Size.x || id.y >= Size.y)
        return;
    float depth = Depth[id.xy];
    float4 motion = Motion[id.xy];
    float2 ndc = float2((id.x + 0.5) / Size.x * 2.0 - 1.0, 1.0 - (id.y + 0.5) / Size.y * 2.0);
    float4 clip = float4(ndc, depth, 1.0);
    float2 gameCamera = CameraUv(clip, ndc, GameToPrev);
    float2 sameCamera = CameraUv(clip, ndc, SameToPrev);
    float2 objectUv = motion.xy * MvecScale - gameCamera;
    float2 fixedUv = sameCamera + ObjectScale * objectUv;
    Fixed[id.xy] = float4(fixedUv / MvecScale, motion.zw);
}
