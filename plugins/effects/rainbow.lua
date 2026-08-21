---@plugin Spatial Rainbow
---@plugin-type effect
---@author rgb-core contributors
---@version 1.0.0
---@license MIT
---@source https://github.com/yourname/rgb-core
---@description 根据设备矩阵坐标渲染并持续移动的全设备彩虹

local effect = {}

local function hsv_to_rgb(hue, saturation, value)
    local sector = math.floor(hue * 6)
    local fraction = hue * 6 - sector
    local p = value * (1 - saturation)
    local q = value * (1 - fraction * saturation)
    local t = value * (1 - (1 - fraction) * saturation)
    sector = sector % 6
    if sector == 0 then return value, t, p end
    if sector == 1 then return q, value, p end
    if sector == 2 then return p, value, t end
    if sector == 3 then return p, q, value end
    if sector == 4 then return t, p, value end
    return value, p, q
end

local function byte(value)
    return math.floor(value * 255 + 0.5)
end

function effect.start(_context)
    return {
        period_seconds = 8,
        brightness = 1,
        hue_span = 1,
        vertical_phase = 0.08,
    }
end

function effect.render(state, context)
    local matrix = context.matrix
    local x_denominator = math.max(matrix.width - 1, 1)
    local y_denominator = math.max(matrix.height - 1, 1)
    local phase = (context.elapsed / state.period_seconds) % 1
    local colors = {}

    for index, led in ipairs(matrix.leds) do
        local x = led.x / x_denominator
        local y = led.y / y_denominator
        local hue = (phase + x * state.hue_span + y * state.vertical_phase) % 1
        local red, green, blue = hsv_to_rgb(hue, 1, state.brightness)
        local offset = (index - 1) * 3
        colors[offset + 1] = byte(red)
        colors[offset + 2] = byte(green)
        colors[offset + 3] = byte(blue)
    end
    return string.char(table.unpack(colors))
end

return effect
