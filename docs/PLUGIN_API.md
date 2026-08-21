# rgb-core Lua 插件 API（v0）

本文描述当前原型 ABI。Lua 版本固定为 5.5；一个 Lua 文件返回一个 module table。

## 共同规则

每个文件必须包含：

```lua
---@plugin MyPlugin
---@plugin-type device
---@author YourName
---@version 1.0.0
---@license MIT
---@source https://github.com/yourname/my-plugin
---@description 一行插件简介
```

索引阶段直接解析注释，插件文件名是插件 ID。每次设备发现和每个被启动的插件都会创建
独立 Lua VM，内存上限为 32 MiB。`require` 加载内核授权的 module，并在当前 VM 内缓存
module table。v0 插件是单个 Lua 文件。

LED ID 可以是 Lua integer 或 UTF-8 string，在一个矩阵中必须唯一。每一帧颜色是一个
长度严格等于 `#matrix.leds * 3` 的 binary string，依次存放每颗 LED 的 R、G、B 字节：

```lua
-- matrix.leds[i] 的颜色
local offset = (i - 1) * 3 + 1
local red, green, blue = string.byte(colors, offset, offset + 2)
```

`matrix.leds` 的顺序确定颜色与 LED 的对应关系；内核在 effect VM 与 device VM 之间
传递连续字节并校验总长度。

## device 插件

### 静态 HID 声明

device 插件至少需要一个：

```lua
---@hid 0x1234:0xabcd
```

完整形式：

```lua
---@hid 0x1234:0xabcd interface=1 usage-page=0xff00 usage=0x0001
```

- VID/PID 按十六进制解析，有无 `0x` 前缀均可；
- `interface` 默认按十进制解析，带 `0x` 时按十六进制解析；
- `usage-page`、`usage` 按十六进制解析；
- 选择器缺省表示不限制该字段。

`@hid` 是插件发现的触发条件。本次 HID 枚举中只要有一项命中任意声明，内核就加载一次
插件并调用 `discover(hids)`；该函数返回的注册项决定可用的逻辑设备及其显示信息。

### module 生命周期

```lua
local hid = require("@rgb/hidapi")
local plugin = {}

function plugin.discover(hids)
    local devices = {}
    for _, info in ipairs(hids) do
        if is_supported(info) then
            devices[#devices + 1] = {
                id = info.path,                          -- 必填，二进制字符串
                name = info.product_string or "My device", -- 必填，UTF-8 字符串
                serial_number = info.serial_number,     -- 可选
                matrix = build_matrix(),                -- 必填
                data = info.path,                       -- 可选，二进制字符串
            }
        end
    end
    return devices
end

function plugin.open(device)
    local handle = hid.open_path(device.data)
    return {
        handle = handle, -- 运行实例的私有字段可任意添加
    }
end

function plugin.render(instance, colors)
    -- colors 是按 matrix.leds 顺序排列的连续 RGB bytes。
    -- 在这里映射硬件槽位、封包并调用 instance.handle:write(...)
end

function plugin.close(instance) -- 可选
    -- 推荐先发送全黑帧，再关闭 handle；必须允许重复清理。
    instance.handle:close()
end

return plugin
```

`discover(hids)` 接收本次枚举到的**全部** HID collection，而不仅是命中 `@hid` 的项；
它必须返回一个连续数组。返回空数组表示插件不注册任何设备。每个注册项包含：

| 字段 | 类型 | 要求 |
|---|---|---|
| `id` | binary string | 必填；同一插件内唯一且在设备仍连接时保持稳定 |
| `name` | UTF-8 string | 必填；显示给用户的设备名称 |
| `serial_number` | UTF-8 string/nil | 可选；显示给用户的序列号 |
| `matrix` | table | 必填；设备 LED 矩阵 |
| `data` | binary string/nil | 可选；内核按不透明数据保存，启动时原样交回脚本；缺省为 `id` |

脚本可以根据 HID 描述字段完成验证，也可以通过 `@rgb/hidapi` 做协议探测；探测期间打开
的临时句柄应在 `discover()` 返回前关闭。某个插件的 `discover()` 抛错会使本次扫描失败。
一个插件可注册零到多个逻辑设备，注册项可按需组合 HID collection。

`open(device)` 只在用户启动一个已注册设备时调用。`device` 是该注册项的重建版本，包含
上述全部字段；`open()` 返回插件私有的运行实例 table。设备显示信息和矩阵由
`discover()` 的注册项确定。

`render` 收到包含全部 LED 的最新输出。协议级写入、ACK、重试和瞬时通信错误恢复都由
设备插件负责：插件可以放弃当前帧并正常返回，让 worker 继续取得最新帧；任何未捕获
Lua error 仍会停止当前管线。device runtime 独占一个 worker 线程；`render` 返回表示
本次驱动调用已经结束。设备忙碌期间被覆盖的中间帧不会发送。

`discover(hids)` 中每个 HID `info` 的字段：

| 字段 | 类型 | 含义 |
|---|---|---|
| `path` | binary string | HIDAPI 平台路径；应原样传给 `open_path` |
| `vendor_id`, `product_id` | integer | USB/Bluetooth HID VID/PID |
| `serial_number` | string/nil | 序列号 |
| `release_number` | integer | release/bcdDevice |
| `manufacturer_string` | string/nil | 厂商字符串 |
| `product_string` | string/nil | 产品字符串 |
| `usage_page`, `usage` | integer | HID usage |
| `interface_number` | integer | interface；不可用时后端通常给 `-1` |
| `bus_type` | string | `usb`、`bluetooth`、`i2c`、`spi` 或 `unknown` |

### 矩阵

矩阵是固定宽高的二维 table。Lua 数组从 1 开始，但 LED 的 `x/y` 暴露给灯效时从 0
开始。空位置必须显式写 `false`，包括每行尾部，否则 Lua 的数组长度不确定。

```lua
local matrix = {
    width = 4,
    height = 2,
    cells = {
        {
            { id = 0, name = "A" },
            false,
            { id = 1, name = "B" },
            false,
        },
        {
            false,
            { id = "logo" }, -- name 可省略
            false,
            false,
        },
    },
}
```

内核校验宽高、行宽、ID 类型和唯一性，并向 effect runtime 提供两种视图：

- `matrix.cells[y + 1][x + 1]`：LED table 或 `false`；
- `matrix.leds`：按 `cells` 从上到下、从左到右排列的紧凑数组，每项包含 `id`、可选
  `name`、`x`、`y`；effect 返回值和 device 收到的 RGB bytes 都使用这个顺序。

### `@rgb/hidapi`

device runtime 提供此 module。

module 函数：

```lua
local devices = hid.enumerate()                  -- 全部 HID
local devices = hid.enumerate(0x1234, 0xabcd)   -- 可选 VID/PID 过滤
local dev = hid.open_path(info.path)
local dev = hid.open(0x1234, 0xabcd)             -- 打开首个匹配项
local dev = hid.open(0x1234, 0xabcd, "serial")
```

device userdata 方法：

```lua
local written = dev:write(binary_string_or_byte_table)
local data = dev:read(length)
local data = dev:read_timeout(length, timeout_ms) -- timeout 时是空字符串
dev:send_feature_report(data)
local data = dev:get_feature_report(report_id, buffer_length)
dev:send_output_report(data)
dev:set_blocking_mode(true_or_false)
local value = dev:get_manufacturer_string()      -- string/nil
local value = dev:get_product_string()           -- string/nil
local value = dev:get_serial_number_string()     -- string/nil
local open = dev:is_open()
dev:close()
```

`write`、feature/output report 和 byte table 中的每个数字都必须在 `0..255`。遵循 HIDAPI
约定，Output/Feature report 的第一个字节是 Report ID；没有 Report ID 的设备应传 0。
单次读 buffer 当前限制为 1 MiB。

## effect 插件

effect runtime 接收矩阵和时间上下文。`start`/`stop` 可选，`render` 必填：

```lua
local effect = {}

function effect.start(context)
    -- effect 被打开时调用一次；返回值作为私有 state 保存。
    return { speed = 0.2 }
end

function effect.render(state, context)
    local bytes = {}
    for index, led in ipairs(context.matrix.leds) do
        local value = math.floor(((context.elapsed * state.speed + led.x / 10) % 1) * 255)
        local offset = (index - 1) * 3
        bytes[offset + 1] = value
        bytes[offset + 2] = 0
        bytes[offset + 3] = 255 - value
    end
    return string.char(table.unpack(bytes))
end

function effect.stop(state) -- 可选
end

return effect
```

`context` 字段：

| 字段 | 类型 | 含义 |
|---|---|---|
| `matrix` | table | 当前活动设备矩阵；含 `cells` 和 `leds` |
| `elapsed` | number | 灯效启动后的单调时间，秒 |
| `delta` | number | 距上次 effect tick 的秒数 |
| `frame` | integer | 从 0 开始的 effect 帧序号 |
| `target_fps` | integer | 当前固定为 60 |

effect runtime 在独立 worker 中以 `1/60` 秒为目标周期自调度，UI 只轮询 worker 状态。
每次 `render` 的 deadline 是下一帧时刻；超时输出会被丢弃，纯 Lua 长循环也会由 VM
instruction hook 中断。worker 不补跑错过的 tick，`frame` 会直接跨过它们。有效输出只
校验一次字节总长度，再写入 device runtime 的单槽邮箱。device 尚未结束本次驱动调用
时，新帧覆盖旧帧；device `render` 返回后立即处理当时最新的一帧，因此两侧 worker
互不等待。设备插件若将瞬时通信错误捕获并正常返回，worker 会继续运行；若错误逃逸为
Lua error，管线会停止。
