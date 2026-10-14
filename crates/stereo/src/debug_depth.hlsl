// The game's depth (any depth format, read through its view) as 32-bit floats.
Texture2D<float> depth_image : register(t0);
RWTexture2D<float> floats : register(u0);

// Threads past the edge read zero and write nowhere: D3D11 drops writes outside a view.
[numthreads(8, 8, 1)]
void main(uint3 at : SV_DispatchThreadID) {
    floats[at.xy] = depth_image.Load(int3(at.xy, 0));
}
