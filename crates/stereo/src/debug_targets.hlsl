// One of the game's render targets (read through a view of its stored values) as 16-bit floats.
Texture2D<float4> target_image : register(t0);
RWTexture2D<float4> floats : register(u0);

// Threads past the edge read zero and write nowhere: D3D11 drops writes outside a view.
[numthreads(8, 8, 1)]
void main(uint3 at : SV_DispatchThreadID) {
    floats[at.xy] = target_image.Load(int3(at.xy, 0));
}
