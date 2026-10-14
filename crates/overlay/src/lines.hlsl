// Debug lines over an eye: each primitive (a segment or a point in tracking space) is one
// instance of six vertices, a quad a few pixels wide built in screen space, so a line keeps its
// width at any distance. Segments are cut at the near plane; points are small squares.

struct Primitive {
    float3 a;
    float3 b;
    uint colour;  // straight RGBA, red in the low byte
    uint kinds;   // category in the low 16 bits, kind in the high (0 segment, 1 point)
};

StructuredBuffer<Primitive> primitives : register(t0);

cbuffer Eye : register(b0) {
    float4 view0;     // tracking to eye space, rows
    float4 view1;
    float4 view2;
    float4 tangents;  // the image edges: left, right, up, down
    float2 size;      // pixels
    float nearZ;
    float halfWidth;  // pixels
};

struct Out {
    float4 position : SV_Position;
    float4 colour : COLOR;
};

static const float2 CORNERS[6] = { float2(0, -1), float2(0, 1), float2(1, 1), float2(0, -1), float2(1, 1), float2(1, -1) };
// Outside the clip volume: the triangle is dropped.
static const float4 NOWHERE = float4(0, 0, -1, 1);

float3 to_eye(float3 p) {
    float4 q = float4(p, 1);
    return float3(dot(view0, q), dot(view1, q), dot(view2, q));
}

// Pixels from the image's middle, y up.
float2 to_pixels(float3 v) {
    float2 t = v.xy / -v.z;
    float2 ndc = float2((2 * t.x - (tangents.x + tangents.y)) / (tangents.y - tangents.x), (2 * t.y - (tangents.z + tangents.w)) / (tangents.z - tangents.w));
    return ndc * size * 0.5;
}

float4 from_pixels(float2 p) {
    return float4(p / (size * 0.5), 0.5, 1);
}

Out vs_main(uint vertex : SV_VertexID, uint instance : SV_InstanceID) {
    Primitive p = primitives[instance];
    float4 c = float4(p.colour & 255, (p.colour >> 8) & 255, (p.colour >> 16) & 255, p.colour >> 24) / 255.0;
    Out o;
    o.colour = float4(c.rgb * c.a, c.a);
    float2 corner = CORNERS[vertex];
    float3 a = to_eye(p.a);
    if ((p.kinds >> 16) == 1) {
        if (a.z > -nearZ) {
            o.position = NOWHERE;
            return o;
        }
        o.position = from_pixels(to_pixels(a) + float2(corner.x * 2 - 1, corner.y) * halfWidth * 2.5);
        return o;
    }
    float3 b = to_eye(p.b);
    bool a_in = a.z <= -nearZ;
    bool b_in = b.z <= -nearZ;
    if (!a_in && !b_in) {
        o.position = NOWHERE;
        return o;
    }
    if (!a_in) a = lerp(a, b, (-nearZ - a.z) / (b.z - a.z));
    if (!b_in) b = lerp(b, a, (-nearZ - b.z) / (a.z - b.z));
    float2 pa = to_pixels(a);
    float2 pb = to_pixels(b);
    float2 d = pb - pa;
    float len = length(d);
    float2 along = len > 1e-4 ? d / len : float2(1, 0);
    float2 across = float2(-along.y, along.x);
    float2 end = corner.x < 0.5 ? pa - along * halfWidth : pb + along * halfWidth;
    o.position = from_pixels(end + across * corner.y * halfWidth);
    return o;
}

float4 ps_main(Out i) : SV_Target {
    return i.colour;
}
