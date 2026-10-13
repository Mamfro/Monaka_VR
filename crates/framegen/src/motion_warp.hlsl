// Motion vectors for an eye warped from the other eye's frame (alternate frame warping with frame
// generation): the real eye's vectors carried into the warped eye's view. Frame generation keeps a
// stream per eye, so the warped eye's previous image is that eye's own real frame, and its vectors
// must point there from the warped image. Per warped pixel, from its depth:
//   camera part toward this eye's previous image = through SameToPrev (its own real frame before)
//   object part = the real eye's vector at the real-eye pixel this one came from (TargetToSource,
//   taken only where the real eye's depth agrees: a filled or stretched gap has no honest motion),
//   minus that pixel's camera part toward the game's previous frame (GameToPrev), scaled by
//   ObjectScale (the frame interval here over the game's).
// The vectors, in the real eye's units, times MvecScale are UV motion from now to before (y down).
// Matrices are row-major with row vectors, as Streamline's.

cbuffer Constants : register(b0)
{
    row_major float4x4 GameToPrev;
    row_major float4x4 SameToPrev;
    row_major float4x4 TargetToSource;
    uint2 Size;
    float2 MvecScale;
    float ObjectScale;
    float3 Pad;
};

Texture2D<float> Depth : register(t0);
Texture2D<float> SourceDepth : register(t1);
Texture2D<float4> Motion : register(t2);
RWTexture2D<float4> Fixed : register(u0);

float2 CameraUv(float4 clip, float2 ndc, row_major float4x4 toPrev)
{
    float4 prev = mul(clip, toPrev);
    float2 prevNdc = prev.xy / prev.w;
    return float2(prevNdc.x - ndc.x, -(prevNdc.y - ndc.y)) * 0.5;
}

// Whether two depth values of one convention show the same surface.
bool Agrees(float a, float b)
{
    return abs(a - b) <= 0.02 * max(abs(a), abs(b)) + 1e-5;
}

[numthreads(8, 8, 1)]
void Warp(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= Size.x || id.y >= Size.y)
        return;
    float depth = Depth[id.xy];
    float2 ndc = float2((id.x + 0.5) / Size.x * 2.0 - 1.0, 1.0 - (id.y + 0.5) / Size.y * 2.0);
    float4 clip = float4(ndc, depth, 1.0);
    float2 sameCamera = CameraUv(clip, ndc, SameToPrev);
    float2 objectUv = float2(0.0, 0.0);
    float2 extra = float2(0.0, 0.0);
    float4 source = mul(clip, TargetToSource);
    if (source.w > 0.0)
    {
        float3 s = source.xyz / source.w;
        float2 pixel = float2((s.x + 1.0) * 0.5 * Size.x, (1.0 - s.y) * 0.5 * Size.y);
        if (all(pixel >= 0.0) && all(pixel < float2(Size)))
        {
            uint2 p = uint2(pixel);
            if (Agrees(SourceDepth[p], s.z))
            {
                float4 motion = Motion[p];
                float4 sourceClip = float4(s.xy, s.z, 1.0);
                objectUv = motion.xy * MvecScale - CameraUv(sourceClip, s.xy, GameToPrev);
                extra = motion.zw;
            }
        }
    }
    float2 fixedUv = sameCamera + ObjectScale * objectUv;
    Fixed[id.xy] = float4(fixedUv / MvecScale, extra);
}
