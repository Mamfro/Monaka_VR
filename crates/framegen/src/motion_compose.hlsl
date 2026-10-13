// Motion vectors onto the same eye's previous frame by chaining the game's own (alternate-eye
// stereo: the game's previous frame is the other eye's, two frames back is this eye's). The game's
// vectors point from now to the previous frame (y down); times MvecScale they are render pixels
// (1 for The Beast's, already pixels; the render size for KCD2's FSR ones, in screen units). A
// pixel here was at p1 = p + Motion(p) in the frame before (the other eye's), and there at
// p1 + Previous(p1) in the one before that: this eye's previous frame. The output keeps the
// game's units. Everything that moves is
// carried as the game itself measured it, the weapon and arms that move with the head included
// (a rebase that takes the world as still moves them by the eyes' parallax).

cbuffer Constants : register(b0)
{
    row_major float4x4 Unused0;
    row_major float4x4 Unused1;
    row_major float4x4 Unused2;
    uint2 Size;
    float2 MvecScale;
    float ObjectScale;
    float3 Pad;
};

Texture2D<float4> Previous : register(t0);
Texture2D<float4> Motion : register(t1);
RWTexture2D<float4> Fixed : register(u0);

[numthreads(8, 8, 1)]
void Compose(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= Size.x || id.y >= Size.y)
        return;
    float4 motion = Motion[id.xy];
    float2 before = float2(id.xy) + 0.5 + motion.xy * MvecScale;
    // Off the previous frame's edge: carried on as it moves now.
    float2 previous = motion.xy;
    if (all(before >= 0.0) && all(before < float2(Size)))
        previous = Previous[uint2(before)].xy;
    Fixed[id.xy] = float4(motion.xy + previous, motion.zw);
}
