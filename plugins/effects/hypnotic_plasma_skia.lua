---@plugin Hypnotic Plasma (Skia)
---@plugin-type effect
---@author Contributors
---@version 1.0.0
---@license MIT
---@source bundled
---@description 使用 Skia runtime shader 渲染流动的高饱和度等离子色彩

local skia = require("@Spectra/skia")
local effect = {}

local shader_source = [[
uniform float2 resolution;
uniform float time;
uniform float speed;
uniform float brightness;

float3 hsv_to_rgb(float hue, float saturation, float value) {
    float3 wave = abs(fract(hue + float3(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0);
    return value * mix(float3(1.0), clamp(wave - 1.0, 0.0, 1.0), saturation);
}

half4 main(float2 position) {
    float2 uv = floor(position) / resolution;
    float t1 = time * speed / 500.0;
    float t2 = t1 * 0.7;
    float t3 = t1 * 1.3;

    float wave1 = sin(uv.x * 5.0 + t1);
    float wave2 = sin(
        (uv.x * sin(t2 / 2.0) + uv.y * cos(t3 / 3.0)) * 5.0 + t2
    );
    float radius = distance(uv, float2(0.5));
    float wave3 = sin(radius * 5.0 + t3);
    float intensity = (wave1 + wave2 + wave3 + 3.0) / 6.0;

    float hue = fract((time * 20.0 + intensity * 120.0) / 360.0);
    float value = ((150.0 + 105.0 * intensity) / 255.0) * (brightness / 100.0);
    return half4(hsv_to_rgb(hue, 1.0, value), 1.0);
}
]]

function effect.start(context)
    local matrix = context.matrix
    local shader = skia.runtime_shader(shader_source)
    shader:set_uniform_float("resolution", { matrix.width, matrix.height })
    shader:set_uniform_float("speed", 100)
    shader:set_uniform_float("brightness", 100)
    return {
        shader = shader,
        surface = skia.surface(matrix.width, matrix.height),
    }
end

function effect.render(state, context)
    state.shader:set_uniform_float("time", context.elapsed)
    state.surface:draw_shader(state.shader)
    return state.surface:sample_rgb(context.matrix.leds)
end

return effect
