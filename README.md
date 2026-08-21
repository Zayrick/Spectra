# Spectra

`Spectra` 是一个跨平台 RGB 基础设施原型。Rust 内核只负责插件发现、Lua runtime、
HID capability、矩阵/颜色路由、60 Hz 调度和 iced GUI；设备支持与灯效由单文件 Lua 5.5
插件提供。

当前随仓库提供：

- `plugins/effects/rainbow.lua`：根据任意设备矩阵坐标渲染的空间彩虹。
- `plugins/effects/hypnotic_plasma.lua`：以多层正弦波渲染流动的高饱和度等离子色彩。

## 数据流

```text
插件注释 ──VID/PID 触发索引──> HID 扫描 / 热插拔
                                  │ 任一 @hid 命中时启用插件
                                  ▼
设备发现 Lua VM ──discover(hids)──> 已注册的逻辑设备 ──> GUI
                                                        │ 用户启动
                                                        ▼
设备运行 Lua VM <──紧凑 RGB bytes── Rust 内核 <──60 Hz worker── 灯效 Lua VM
      │                                                        （矩阵/时间上下文）
      └── require("@Spectra/hidapi") ──> hidapi ──> HID device
```

每次设备发现以及每个已启动插件都使用独立的 Lua 5.5 VM 和 module cache。设备发现和
运行 runtime 可通过 `require("@Spectra/hidapi")` 获取 HID API；灯效 runtime 接收矩阵和
时间组成的渲染上下文。

## 运行

项目使用 vendored Lua 5.5，不需要单独安装 Lua。Windows 使用原生 `hid.dll` 后端，
Linux 使用 hidraw/basic-udev 后端，macOS 使用 HIDAPI 的系统后端。

```powershell
cargo run --release
```

GUI 操作：

- 在左侧设备列表中选择设备；
- 点击灯效列表中的灯效即可启动，点击正在运行的灯效即可停止管线并关闭设备；
- 点击标题栏的“扫描设备”立即刷新设备列表；后台也会每秒自动扫描热插拔；
- 关闭窗口时，应用会先停止活动管线并关闭设备。

解析并列出插件元数据：

```powershell
cargo run -- --list-plugins
```

运行被 `@hid` 触发的 device 插件，并列出脚本实际注册的逻辑设备：

```powershell
cargo run -- --list-devices
```

自定义插件目录：

```powershell
cargo run -- --plugin-dir D:\my-rgb-plugins
```

Linux 上还需要让当前用户拥有目标 `/dev/hidraw*` 的读写权限；通常通过 udev rule
完成。macOS 首次访问设备时可能需要授予输入设备权限。

## 插件发现

一个插件就是一个 `.lua` 文件。下列七项注释是必需的：

```lua
---@plugin MyPlugin
---@plugin-type device
---@author YourName
---@version 1.0.0
---@license MIT
---@source https://github.com/yourname/my-plugin
---@description 一个示例 Lua 插件
```

`@plugin-type` 只能是 `device` 或 `effect`。文件名（不含 `.lua`）是插件 ID，因此插件
目录中不能有重名文件。

device 插件还必须至少声明一个静态 HID 匹配项：

```lua
---@hid 0x1234:0xabcd interface=1 usage-page=0xff00 usage=0x0001
```

可以写多个 `@hid`。只有 `VID:PID` 必填；`interface`、`usage-page`、`usage` 都是可选
的触发条件。任一 `@hid` 命中后，内核调用 `plugin.discover(hids)`；脚本返回的逻辑设备
注册项进入 GUI。设备名称、序列号、矩阵、稳定 ID 和运行时私有数据均由脚本提供。

完整 ABI 见 [插件 API](docs/PLUGIN_API.md)。

## 调度和路由

灯效和设备各自运行在独立 worker。灯效 `render(state, context)` 以目标 60 Hz 自调度，
返回一个按 `matrix.leds` 排列、每颗灯连续 3 个 RGB bytes 的 binary string。内核只校验
一次字节长度；超时帧和错过的 tick 直接丢弃，不在 UI 线程补跑。设备 worker 使用单槽
latest-frame 邮箱：设备忙时，新灯效帧覆盖尚未发送的旧帧；设备插件的 `render()` 返回
后立即取当时最新的一帧继续发送。协议级 ready、重试和瞬时通信错误恢复由设备 Lua
驱动自行处理，内核负责调度和帧传递。

## 验证

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## 支持范围

- 设备 transport 使用 HID；
- GUI 同时运行一台设备和一个灯效；
- 热插拔每秒刷新一次，拔出活动设备会停止管线；
- Lua 插件在应用进程内分别使用独立 VM 和 worker。
