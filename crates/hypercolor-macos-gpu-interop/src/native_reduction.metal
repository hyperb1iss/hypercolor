#include <metal_stdlib>
using namespace metal;

struct ReductionParameters {
    uint4 content_rect;
    uint4 output_and_format;
    float4 source_rect;
    uint4 source_and_chroma_extent;
    uint4 color;
    uint4 operation;
    float4 source_to_target[3];
    float4 source_luminance_and_exposure;
    float4 curve;
};

struct MaterializationParameters {
    uint4 content_rect;
    uint4 output_extent;
    float4 fill;
};

constant float PQ_M1 = 2610.0 / 16384.0;
constant float PQ_M2 = 2523.0 / 32.0;
constant float PQ_C1 = 3424.0 / 4096.0;
constant float PQ_C2 = 2413.0 / 128.0;
constant float PQ_C3 = 2392.0 / 128.0;

float pq_to_nits(float encoded) {
    float power = pow(clamp(encoded, 0.0, 1.0), 1.0 / PQ_M2);
    float numerator = max(power - PQ_C1, 0.0);
    float denominator = PQ_C2 - PQ_C3 * power;
    return 10000.0 * pow(numerator / denominator, 1.0 / PQ_M1);
}

float nits_to_pq(float nits) {
    float power = pow(max(nits, 0.0) / 10000.0, PQ_M1);
    return pow((PQ_C1 + PQ_C2 * power) / (1.0 + PQ_C3 * power), PQ_M2);
}

float hlg_inverse_oetf(float encoded) {
    constexpr float hlg_a = 0.17883277;
    constexpr float hlg_b = 0.28466892;
    constexpr float hlg_c = 0.55991073;
    return encoded <= 0.5
        ? encoded * encoded / 3.0
        : (exp((encoded - hlg_c) / hlg_a) + hlg_b) / 12.0;
}

float decode_channel(
    float encoded,
    uint transfer,
    float source_reference_nits
) {
    if (transfer == 0) {
        return encoded <= 0.04045
            ? encoded / 12.92
            : pow((encoded + 0.055) / 1.055, 2.4);
    }
    if (transfer == 1) {
        return encoded < 0.081
            ? encoded / 4.5
            : pow((encoded + 0.099) / 1.099, 1.0 / 0.45);
    }
    if (transfer == 2) {
        constexpr float alpha = 1.09929682680944;
        constexpr float beta = 0.018053968510807;
        return encoded < 4.5 * beta
            ? encoded / 4.5
            : pow((encoded + alpha - 1.0) / alpha, 1.0 / 0.45);
    }
    if (transfer == 3) {
        return encoded;
    }
    if (transfer == 4) {
        return pq_to_nits(encoded) / source_reference_nits;
    }
    return hlg_inverse_oetf(max(encoded, 0.0));
}

float map_luminance(float value, constant ReductionParameters& p) {
    float reference_ratio = p.curve.x;
    float source_headroom = p.curve.y;
    float target_peak_nits = p.curve.w;
    if (source_headroom <= 1.0) {
        return min(value, 1.0) * reference_ratio;
    }
    float target_reference_nits = reference_ratio * target_peak_nits;
    float source_peak_nits = target_reference_nits * source_headroom;
    if (source_peak_nits <= target_peak_nits) {
        return clamp(value * reference_ratio, 0.0, 1.0);
    }
    float source_peak_pq = nits_to_pq(source_peak_nits);
    float maximum_luminance = nits_to_pq(target_peak_nits) / source_peak_pq;
    float input_pq = nits_to_pq(value * target_reference_nits) / source_peak_pq;
    float knee_start = 1.5 * maximum_luminance - 0.5;
    if (input_pq < knee_start) {
        return clamp(value * reference_ratio, 0.0, 1.0);
    }
    float t = clamp((input_pq - knee_start) / (1.0 - knee_start), 0.0, 1.0);
    float t2 = t * t;
    float t3 = t2 * t;
    float output_pq = (2.0 * t3 - 3.0 * t2 + 1.0) * knee_start
        + (t3 - 2.0 * t2 + t) * (1.0 - knee_start)
        + (-2.0 * t3 + 3.0 * t2) * maximum_luminance;
    return clamp(pq_to_nits(output_pq * source_peak_pq) / target_peak_nits, 0.0, 1.0);
}

float3 compress_gamut(float3 rgb, float luminance) {
    float neutral = clamp(luminance, 0.0, 1.0);
    float scale = 1.0;
    for (uint index = 0; index < 3; index++) {
        float channel = rgb[index];
        float chroma = channel - neutral;
        if (channel < 0.0) {
            scale = min(scale, neutral / -chroma);
        } else if (channel > 1.0) {
            scale = min(scale, (1.0 - neutral) / chroma);
        }
    }
    return clamp(neutral + (rgb - neutral) * scale, 0.0, 1.0);
}

float3 map_color(float3 encoded, constant ReductionParameters& p) {
    float3 linear;
    for (uint index = 0; index < 3; index++) {
        linear[index] = decode_channel(encoded[index], p.color.w, p.curve.z);
    }
    if (p.color.w == 5) {
        float scene_luminance = max(
            dot(p.source_luminance_and_exposure.xyz, linear),
            0.0
        );
        if (scene_luminance <= FLT_EPSILON) {
            linear = float3(0.0);
        } else {
            float peak_nits = p.curve.z * p.curve.y;
            float system_gamma = 1.2 + 0.42 * log10(peak_nits / 1000.0);
            float ootf_scale = p.curve.y * pow(scene_luminance, system_gamma - 1.0);
            linear *= ootf_scale;
        }
    }
    float exposure = p.source_luminance_and_exposure.w;
    float3 exposed = linear * exposure;
    float source_luminance = max(
        dot(p.source_luminance_and_exposure.xyz, exposed),
        0.0
    );
    float mapped_luminance = map_luminance(source_luminance, p);
    float3 target = float3(
        dot(p.source_to_target[0].xyz, exposed),
        dot(p.source_to_target[1].xyz, exposed),
        dot(p.source_to_target[2].xyz, exposed)
    );
    target = source_luminance > FLT_EPSILON
        ? target * (mapped_luminance / source_luminance)
        : float3(0.0);
    float minimum = min(target.x, min(target.y, target.z));
    float maximum = max(target.x, max(target.y, target.z));
    if (maximum - minimum <= 1.0e-5) {
        target = float3(mapped_luminance);
    }
    return compress_gamut(target, mapped_luminance);
}

float2 read_chroma_bilinear(
    texture2d<float, access::read> chroma,
    float2 coordinate,
    uint2 extent
) {
    float2 bounded = clamp(coordinate, float2(0.0), float2(extent - 1));
    uint2 lower = uint2(floor(bounded));
    uint2 upper = min(lower + 1, extent - 1);
    float2 fraction = fract(bounded);
    float2 top = mix(chroma.read(lower).rg, chroma.read(uint2(upper.x, lower.y)).rg, fraction.x);
    float2 bottom = mix(chroma.read(uint2(lower.x, upper.y)).rg, chroma.read(upper).rg, fraction.x);
    return mix(top, bottom, fraction.y);
}

float3 yuv_to_rgb(float y, float cb, float cr, uint matrix) {
    float kr = matrix == 1 ? 0.299 : (matrix == 2 ? 0.2126 : 0.2627);
    float kb = matrix == 1 ? 0.114 : (matrix == 2 ? 0.0722 : 0.0593);
    float kg = 1.0 - kr - kb;
    return float3(
        y + 2.0 * (1.0 - kr) * cr,
        y - 2.0 * kb * (1.0 - kb) / kg * cb
            - 2.0 * kr * (1.0 - kr) / kg * cr,
        y + 2.0 * (1.0 - kb) * cb
    );
}

float4 load_encoded(
    texture2d<float, access::read> plane0,
    texture2d<float, access::read> plane1,
    uint2 position,
    constant ReductionParameters& p
) {
    uint format = p.output_and_format.z;
    if (format <= 2) {
        return plane0.read(position);
    }
    float y;
    float cb;
    float cr;
    if (format == 3) {
        float2 offset = p.color.z == 1
            ? float2(1.0, 1.0)
            : (p.color.z == 2 ? float2(0.5, 1.0) : float2(0.5, 0.5));
        float2 luma_center = float2(position) + 0.5;
        float2 chroma_coordinate = (luma_center - offset) * 0.5;
        float2 chroma = read_chroma_bilinear(
            plane1,
            chroma_coordinate,
            p.source_and_chroma_extent.zw
        );
        y = plane0.read(position).r;
        if (p.color.x == 0) {
            cb = chroma.r - 128.0 / 255.0;
            cr = chroma.g - 128.0 / 255.0;
        } else {
            y = (y * 255.0 - 16.0) / 219.0;
            cb = (chroma.r * 255.0 - 128.0) / 224.0;
            cr = (chroma.g * 255.0 - 128.0) / 224.0;
        }
    } else {
        float luma_code = round(plane0.read(position).r * 65535.0) / 64.0;
        float2 chroma_code = round(plane1.read(position).rg * 65535.0) / 64.0;
        if (p.color.x == 0) {
            y = luma_code / 1023.0;
            cb = (chroma_code.r - 512.0) / 1023.0;
            cr = (chroma_code.g - 512.0) / 1023.0;
        } else {
            y = (luma_code - 64.0) / 876.0;
            cb = (chroma_code.r - 512.0) / 896.0;
            cr = (chroma_code.g - 512.0) / 896.0;
        }
    }
    return float4(yuv_to_rgb(y, cb, cr, p.color.y), 1.0);
}

float4 load_sample(
    texture2d<float, access::read> plane0,
    texture2d<float, access::read> plane1,
    int2 position,
    constant ReductionParameters& p
) {
    int2 maximum = int2(p.source_and_chroma_extent.xy) - 1;
    uint2 bounded = uint2(clamp(position, int2(0), maximum));
    float4 encoded = load_encoded(plane0, plane1, bounded, p);
    if (p.operation.x == 0) {
        return encoded;
    }
    return float4(map_color(encoded.rgb, p), encoded.a);
}

float4 sample_nearest(
    texture2d<float, access::read> plane0,
    texture2d<float, access::read> plane1,
    float2 coordinate,
    constant ReductionParameters& p
) {
    return load_sample(plane0, plane1, int2(floor(coordinate)), p);
}

float4 sample_bilinear(
    texture2d<float, access::read> plane0,
    texture2d<float, access::read> plane1,
    float2 coordinate,
    constant ReductionParameters& p
) {
    float2 centered = coordinate - 0.5;
    int2 lower = int2(floor(centered));
    float2 fraction = fract(centered);
    float4 top = mix(
        load_sample(plane0, plane1, lower, p),
        load_sample(plane0, plane1, lower + int2(1, 0), p),
        fraction.x
    );
    float4 bottom = mix(
        load_sample(plane0, plane1, lower + int2(0, 1), p),
        load_sample(plane0, plane1, lower + int2(1, 1), p),
        fraction.x
    );
    return mix(top, bottom, fraction.y);
}

float4 sample_area(
    texture2d<float, access::read> plane0,
    texture2d<float, access::read> plane1,
    float2 start,
    float2 end,
    constant ReductionParameters& p
) {
    int2 first = int2(floor(start));
    int2 last = int2(ceil(end));
    float4 total = float4(0.0);
    float total_weight = 0.0;
    for (int y = first.y; y < last.y; y++) {
        float height = max(0.0, min(end.y, float(y + 1)) - max(start.y, float(y)));
        for (int x = first.x; x < last.x; x++) {
            float width = max(0.0, min(end.x, float(x + 1)) - max(start.x, float(x)));
            float weight = width * height;
            total += load_sample(plane0, plane1, int2(x, y), p) * weight;
            total_weight += weight;
        }
    }
    return total / max(total_weight, FLT_EPSILON);
}

float encode_srgb(float linear) {
    float bounded = round(clamp(linear, 0.0, 1.0) * 4095.0) / 4095.0;
    return bounded <= 0.0031308
        ? 12.92 * bounded
        : 1.055 * pow(bounded, 1.0 / 2.4) - 0.055;
}

float encode_output(float linear, uint transfer) {
    if (transfer == 0) {
        return encode_srgb(linear);
    }
    if (transfer == 1) {
        return linear;
    }
    if (transfer == 2) {
        return linear < 0.018
            ? 4.5 * linear
            : 1.099 * pow(linear, 0.45) - 0.099;
    }
    constexpr float alpha = 1.0992968;
    constexpr float beta = 0.01805397;
    return linear < beta
        ? 4.5 * linear
        : alpha * pow(linear, 0.45) - (alpha - 1.0);
}

kernel void hypercolor_reduce(
    texture2d<float, access::read> plane0 [[texture(0)]],
    texture2d<float, access::read> plane1 [[texture(1)]],
    texture2d<float, access::write> output [[texture(2)]],
    constant ReductionParameters& p [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (any(gid >= p.output_and_format.xy)) {
        return;
    }
    if (gid.x < p.content_rect.x || gid.y < p.content_rect.y
        || gid.x >= p.content_rect.x + p.content_rect.z
        || gid.y >= p.content_rect.y + p.content_rect.w) {
        output.write(float4(0.0, 0.0, 0.0, 1.0), gid);
        return;
    }
    float2 local = float2(gid - p.content_rect.xy);
    float2 scale = p.source_rect.zw / float2(p.content_rect.zw);
    float2 start = p.source_rect.xy + local * scale;
    float2 end = start + scale;
    float4 sample;
    if (p.output_and_format.w == 0) {
        sample = sample_nearest(plane0, plane1, (start + end) * 0.5, p);
    } else if (p.output_and_format.w == 1) {
        sample = sample_bilinear(plane0, plane1, (start + end) * 0.5, p);
    } else {
        sample = sample_area(plane0, plane1, start, end, p);
    }
    if (p.operation.x != 0) {
        sample.rgb = float3(
            encode_output(sample.r, p.operation.y),
            encode_output(sample.g, p.operation.y),
            encode_output(sample.b, p.operation.y)
        );
    }
    output.write(float4(clamp(sample.rgb, 0.0, 1.0), clamp(sample.a, 0.0, 1.0)), gid);
}

kernel void hypercolor_materialize(
    texture2d<float, access::read> source [[texture(0)]],
    texture2d<float, access::write> output [[texture(1)]],
    constant MaterializationParameters& p [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (any(gid >= p.output_extent.xy)) {
        return;
    }
    if (gid.x < p.content_rect.x || gid.y < p.content_rect.y
        || gid.x >= p.content_rect.x + p.content_rect.z
        || gid.y >= p.content_rect.y + p.content_rect.w) {
        output.write(p.fill, gid);
        return;
    }
    output.write(source.read(gid - p.content_rect.xy), gid);
}
