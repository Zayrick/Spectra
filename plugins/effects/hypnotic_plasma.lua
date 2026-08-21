---@plugin Hypnotic Plasma
---@plugin-type effect
---@author Contributors
---@version 1.0.0
---@license MIT
---@source bundled
---@description 以多层正弦波生成流动的高饱和度等离子色彩

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
    return math.floor(math.max(0, math.min(1, value)) * 255 + 0.5)
end

function effect.start(_context)
    return {
        speed = 100,
        brightness = 100,
    }
end

function effect.render(state, context)
    local matrix = context.matrix
    local time = context.elapsed
    local t1 = time * state.speed / 500.0
    local t2 = t1 * 0.7
    local t3 = t1 * 1.3
    local colors = {}

    for index, led in ipairs(matrix.leds) do
        local u = led.x / matrix.width
        local v = led.y / matrix.height

        local wave1 = math.sin(u * 5.0 + t1)
        local wave2 = math.sin(
            (u * math.sin(t2 / 2.0) + v * math.cos(t3 / 3.0)) * 5.0 + t2
        )
        local radius = math.sqrt((u - 0.5) ^ 2 + (v - 0.5) ^ 2)
        local wave3 = math.sin(radius * 5.0 + t3)
        local intensity = (wave1 + wave2 + wave3 + 3.0) / 6.0

        local hue = ((time * 20 + intensity * 120) % 360) / 360
        local value = ((150 + 105 * intensity) / 255) * (state.brightness / 100)
        local red, green, blue = hsv_to_rgb(hue, 1, value)
        local offset = (index - 1) * 3
        colors[offset + 1] = byte(red)
        colors[offset + 2] = byte(green)
        colors[offset + 3] = byte(blue)
    end

    return string.char(table.unpack(colors))
end

return effect
