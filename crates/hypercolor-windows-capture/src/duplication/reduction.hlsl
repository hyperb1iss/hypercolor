Texture2D<float4> Desktop : register(t0);
Texture2D<uint4> Pointer : register(t1);
RWTexture2D<float4> Target : register(u0);

cbuffer ReductionParams : register(b0)
{
    uint SourceWidth;
    uint SourceHeight;
    uint OutputWidth;
    uint OutputHeight;
    uint Stride;
    uint Rotation;
    uint PointerKind;
    uint PointerVisible;
    int PointerX;
    int PointerY;
    uint PointerWidth;
    uint PointerHeight;
    uint RegionX;
    uint RegionY;
    uint RegionWidth;
    uint RegionHeight;
    uint Filter;
    uint ColorPipeline;
    uint CursorPolicy;
    uint Padding;
};

int2 scanout_to_logical(int2 scanout)
{
    if (Rotation == 1) {
        return int2(int(SourceHeight) - 1 - scanout.y, scanout.x);
    }
    if (Rotation == 2) {
        return int2(int(SourceWidth) - 1 - scanout.x,
                    int(SourceHeight) - 1 - scanout.y);
    }
    if (Rotation == 3) {
        return int2(scanout.y, int(SourceWidth) - 1 - scanout.x);
    }
    return scanout;
}

int2 logical_to_scanout(int2 logical)
{
    if (Rotation == 1) {
        return int2(logical.y, int(SourceHeight) - 1 - logical.x);
    }
    if (Rotation == 2) {
        return int2(int(SourceWidth) - 1 - logical.x,
                    int(SourceHeight) - 1 - logical.y);
    }
    if (Rotation == 3) {
        return int2(int(SourceWidth) - 1 - logical.y, logical.x);
    }
    return logical;
}

float4 compose_cursor(uint2 scanout, float4 desktop_sample)
{
    if (CursorPolicy == 0 || PointerVisible == 0) {
        return desktop_sample;
    }

    int2 logical = scanout_to_logical(int2(scanout));
    int2 pointer_position = logical - int2(PointerX, PointerY);
    if (pointer_position.x < 0 || pointer_position.y < 0 ||
        pointer_position.x >= int(PointerWidth) ||
        pointer_position.y >= int(PointerHeight)) {
        return desktop_sample;
    }

    uint4 desktop = uint4(round(saturate(desktop_sample) * 255.0));
    uint4 pointer = Pointer.Load(int3(pointer_position, 0));
    uint3 color;

    if (PointerKind == 0) {
        uint alpha = pointer.a;
        color = (pointer.rgb * alpha + desktop.rgb * (255 - alpha) + 127) / 255;
    } else if (PointerKind == 1) {
        color = pointer.a == 0 ? pointer.rgb : desktop.rgb ^ pointer.rgb;
    } else {
        uint and_mask = pointer.r;
        uint xor_mask = pointer.g;
        color = (desktop.rgb & and_mask) ^ xor_mask;
    }

    return float4(float3(color) / 255.0, 1.0);
}

float3 decode_color(float3 encoded)
{
    if (ColorPipeline == 0) {
        return encoded;
    }
    float3 low = encoded / 12.92;
    float3 high = pow((encoded + 0.055) / 1.055, 2.4);
    return lerp(high, low, encoded <= 0.04045);
}

float3 encode_color(float3 decoded)
{
    if (ColorPipeline == 0) {
        return decoded;
    }
    decoded = saturate(decoded);
    float3 low = decoded * 12.92;
    float3 high = 1.055 * pow(decoded, 1.0 / 2.4) - 0.055;
    return lerp(high, low, decoded <= 0.0031308);
}

float3 sample_logical(uint2 logical)
{
    uint2 scanout = uint2(logical_to_scanout(int2(logical)));
    float4 desktop = Desktop.Load(int3(scanout, 0));
    return decode_color(compose_cursor(scanout, desktop).rgb);
}

float3 sample_nearest_exact(uint2 target)
{
    uint2 centered = target * 2 + 1;
    uint2 numerator = centered * uint2(RegionWidth, RegionHeight);
    uint2 denominator = uint2(OutputWidth, OutputHeight) * 2;
    uint2 logical = uint2(RegionX, RegionY) + numerator / denominator;
    return sample_logical(min(
        logical,
        uint2(RegionX + RegionWidth - 1, RegionY + RegionHeight - 1)
    ));
}

float2 bilinear_axis(uint target, uint source_length, uint output_length)
{
    float position = ((float(target) + 0.5) * float(source_length) /
                      float(output_length)) - 0.5;
    float clamped = clamp(position, 0.0, float(source_length - 1));
    float lower = floor(clamped);
    return float2(lower, clamped - lower);
}

float3 sample_bilinear_exact(uint2 target)
{
    float2 x = bilinear_axis(target.x, RegionWidth, OutputWidth);
    float2 y = bilinear_axis(target.y, RegionHeight, OutputHeight);
    uint x0 = uint(x.x);
    uint y0 = uint(y.x);
    uint x1 = min(x0 + 1, RegionWidth - 1);
    uint y1 = min(y0 + 1, RegionHeight - 1);
    float3 top = lerp(
        sample_logical(uint2(RegionX + x0, RegionY + y0)),
        sample_logical(uint2(RegionX + x1, RegionY + y0)),
        x.y
    );
    float3 bottom = lerp(
        sample_logical(uint2(RegionX + x0, RegionY + y1)),
        sample_logical(uint2(RegionX + x1, RegionY + y1)),
        x.y
    );
    return lerp(top, bottom, y.y);
}

float3 sample_area_exact(uint2 target)
{
    float2 region_size = float2(RegionWidth, RegionHeight);
    float2 output_size = float2(OutputWidth, OutputHeight);
    float2 left = float2(target) * region_size / output_size;
    float2 right = float2(target + 1) * region_size / output_size;
    uint2 begin = uint2(floor(left));
    uint2 end = uint2(ceil(right));
    float3 sum = 0.0;
    float total_weight = 0.0;
    for (uint y = begin.y; y < end.y; ++y) {
        float y_weight = min(right.y, float(y + 1)) - max(left.y, float(y));
        for (uint x = begin.x; x < end.x; ++x) {
            float x_weight = min(right.x, float(x + 1)) - max(left.x, float(x));
            float weight = x_weight * y_weight;
            sum += sample_logical(uint2(RegionX + x, RegionY + y)) * weight;
            total_weight += weight;
        }
    }
    return sum / max(total_weight, 1.0e-20);
}

[numthreads(8, 8, 1)]
void reduce_desktop_exact(uint3 thread_id : SV_DispatchThreadID)
{
    if (thread_id.x >= OutputWidth || thread_id.y >= OutputHeight) {
        return;
    }
    float3 sample;
    if (Filter == 0) {
        sample = sample_nearest_exact(thread_id.xy);
    } else if (Filter == 1) {
        sample = sample_bilinear_exact(thread_id.xy);
    } else {
        sample = sample_area_exact(thread_id.xy);
    }
    Target[thread_id.xy] = float4(encode_color(sample), 1.0);
}

[numthreads(8, 8, 1)]
void reduce_desktop(uint3 thread_id : SV_DispatchThreadID)
{
    if (thread_id.x >= OutputWidth || thread_id.y >= OutputHeight) {
        return;
    }

    uint2 region_begin = uint2(RegionX, RegionY);
    uint2 region_end = region_begin + uint2(RegionWidth, RegionHeight);
    uint2 begin = region_begin + thread_id.xy * Stride;
    uint2 end = min(begin + Stride, region_end);
    float3 sum = 0.0;
    uint samples = 0;
    for (uint y = begin.y; y < end.y; ++y) {
        for (uint x = begin.x; x < end.x; ++x) {
            float4 desktop = Desktop.Load(int3(x, y, 0));
            sum += compose_cursor(uint2(x, y), desktop).rgb;
            ++samples;
        }
    }

    Target[thread_id.xy] = float4(sum / max(samples, 1), 1.0);
}

[numthreads(8, 8, 1)]
void publish_surface_exact(uint3 thread_id : SV_DispatchThreadID)
{
    if (thread_id.x >= OutputWidth || thread_id.y >= OutputHeight) {
        return;
    }

    uint2 centered = thread_id.xy * 2 + 1;
    uint2 numerator = centered * uint2(RegionWidth, RegionHeight);
    uint2 denominator = uint2(OutputWidth, OutputHeight) * 2;
    uint2 logical = uint2(RegionX, RegionY) + numerator / denominator;
    logical = min(
        logical,
        uint2(RegionX + RegionWidth - 1, RegionY + RegionHeight - 1)
    );
    uint2 source = uint2(logical_to_scanout(int2(logical)));

    float4 desktop = Desktop.Load(int3(source, 0));
    Target[thread_id.xy] = float4(compose_cursor(source, desktop).rgb, 1.0);
}
