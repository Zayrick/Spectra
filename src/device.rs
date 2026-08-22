use anyhow::{Context, Result, ensure};
use mlua::Table;

use crate::hid::HidManager;
use crate::plugin::{PluginType, RegisteredDevice};
use crate::runtime::LuaPluginRuntime;
use crate::types::{ColorFrame, DeviceCapabilities, DeviceMatrix, ModeSettings};

struct DeviceSession {
    plugin_name: String,
    name: String,
    matrix: DeviceMatrix,
    capabilities: DeviceCapabilities,
    instance: Option<Table>,
    runtime: LuaPluginRuntime,
}

impl DeviceSession {
    fn open(device: &RegisteredDevice, hid: &HidManager) -> Result<Self> {
        let metadata = &device.plugin;
        ensure!(
            metadata.plugin_type == PluginType::Device,
            "{} 不是 device 插件",
            metadata.name
        );

        let runtime = LuaPluginRuntime::load(metadata, Some(hid))?;
        Self::open_with_runtime(device, runtime)
    }

    fn open_with_runtime(device: &RegisteredDevice, runtime: LuaPluginRuntime) -> Result<Self> {
        let metadata = &device.plugin;
        let open = runtime.required_function("open")?;
        let instance: Table = open
            .call(device.to_lua(&runtime.lua)?)
            .with_context(|| format!("设备插件 {} 的 open() 失败", metadata.name))?;

        Ok(Self {
            plugin_name: metadata.name.clone(),
            name: device.name.clone(),
            matrix: device.matrix.clone(),
            capabilities: device.capabilities.clone(),
            instance: Some(instance),
            runtime,
        })
    }

    #[cfg(test)]
    fn from_test_source(device: &RegisteredDevice, source: &str) -> Result<Self> {
        let runtime = LuaPluginRuntime::from_test_source(&device.plugin, source)?;
        Self::open_with_runtime(device, runtime)
    }

    fn enter_live(self) -> Result<LiveDeviceSession> {
        ensure!(
            self.capabilities.live,
            "设备 {} 不支持 live 控制",
            self.name
        );
        let instance = self.instance.as_ref().context("device session 已关闭")?;
        self.runtime
            .required_function("enter_live")?
            .call::<()>(instance.clone())
            .with_context(|| format!("设备插件 {} 的 enter_live() 失败", self.plugin_name))?;
        Ok(LiveDeviceSession { session: self })
    }

    fn render_live(&self, colors: &ColorFrame) -> Result<()> {
        ensure!(
            colors.len() == self.matrix.leds.len() * 3,
            "设备 {} 收到的颜色帧长度无效",
            self.name
        );
        let instance = self.instance.as_ref().context("device session 已关闭")?;
        let frame = self.runtime.lua.create_string(colors)?;

        self.runtime
            .required_function("render")?
            .call::<()>((instance.clone(), frame))
            .with_context(|| format!("设备插件 {} 的 render() 失败", self.plugin_name))
    }

    fn apply_mode(&self, mode_id: &str, settings: &ModeSettings) -> Result<()> {
        let mode = self
            .capabilities
            .mode(mode_id)
            .with_context(|| format!("设备 {} 没有单机模式 {mode_id:?}", self.name))?;
        mode.validate_settings(settings)?;
        let instance = self.instance.as_ref().context("device session 已关闭")?;
        let mode = mode.to_lua(&self.runtime.lua)?;
        let settings = settings.to_lua(&self.runtime.lua)?;
        self.runtime
            .required_function("apply_mode")?
            .call::<()>((instance.clone(), mode, settings))
            .with_context(|| format!("设备插件 {} 的 apply_mode() 失败", self.plugin_name))
    }

    fn close(&mut self) -> Result<()> {
        let Some(instance) = self.instance.take() else {
            return Ok(());
        };
        match self.runtime.optional_function("close")? {
            Some(close) => close
                .call::<()>(instance)
                .with_context(|| format!("设备插件 {} 的 close() 失败", self.plugin_name)),
            None => Ok(()),
        }
    }
}

impl Drop for DeviceSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub(crate) struct LiveDeviceSession {
    session: DeviceSession,
}

impl LiveDeviceSession {
    pub(crate) fn start(device: &RegisteredDevice, hid: &HidManager) -> Result<Self> {
        DeviceSession::open(device, hid)?.enter_live()
    }

    pub(crate) fn render(&self, colors: &ColorFrame) -> Result<()> {
        self.session.render_live(colors)
    }

    pub(crate) fn close(&mut self) -> Result<()> {
        self.session.close()
    }

    pub(crate) fn name(&self) -> &str {
        &self.session.name
    }

    pub(crate) fn matrix(&self) -> &DeviceMatrix {
        &self.session.matrix
    }
}

pub fn apply_standalone_mode(
    device: &RegisteredDevice,
    mode_id: &str,
    settings: &ModeSettings,
    hid: &HidManager,
) -> Result<()> {
    let mode = device
        .capabilities
        .mode(mode_id)
        .with_context(|| format!("设备 {} 没有单机模式 {mode_id:?}", device.name))?;
    mode.validate_settings(settings)?;

    let mut session = DeviceSession::open(device, hid)?;
    let operation = session.apply_mode(mode_id, settings);
    let close = session.close();

    match (operation, close) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(operation), Err(close)) => {
            anyhow::bail!("应用单机模式失败：{operation:#}；关闭 HID 会话时又发生错误：{close:#}")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mlua::Table;

    use super::*;
    use crate::plugin::PluginMetadata;
    use crate::types::{DeviceMode, Led, LedId, ModeComponent, ModeControl, RgbColor, SliderRange};

    const DEVICE_SOURCE: &str = r#"
local calls = {}
local plugin = { calls = calls }

function plugin.open(device)
    calls[#calls + 1] = "open"
    return {}
end

function plugin.enter_live(instance)
    calls[#calls + 1] = "enter_live"
end

function plugin.render(instance, colors)
    calls[#calls + 1] = "render"
end

function plugin.apply_mode(instance, mode, settings)
    calls[#calls + 1] = string.format(
        "apply:%s:%s:%d:%d",
        mode.id,
        mode.data,
        settings.intensity,
        settings.primary_color.red
    )
end

function plugin.close(instance)
    calls[#calls + 1] = "close"
end

return plugin
"#;

    fn registered_device() -> RegisteredDevice {
        let led_id = LedId::Integer(0);
        RegisteredDevice {
            plugin: PluginMetadata {
                id: "test_device".into(),
                name: "Test Device Plugin".into(),
                plugin_type: PluginType::Device,
                author: "Test".into(),
                version: "1.0.0".into(),
                license: "MIT".into(),
                source: "test".into(),
                description: "Test".into(),
                hid: Vec::new(),
                path: PathBuf::from("embedded-test-device"),
            },
            id: b"device".to_vec(),
            name: "Test Device".into(),
            serial_number: None,
            matrix: DeviceMatrix {
                width: 1,
                height: 1,
                cells: vec![vec![Some(led_id.clone())]],
                leds: vec![Led {
                    id: led_id,
                    name: None,
                    x: 0,
                    y: 0,
                }],
            },
            capabilities: DeviceCapabilities {
                live: true,
                modes: vec![DeviceMode {
                    id: "solid".into(),
                    name: "Solid".into(),
                    description: Some("Static device effect".into()),
                    data: b"solid-data".to_vec(),
                    controls: vec![
                        ModeControl {
                            id: "primary_color".into(),
                            name: "Primary color".into(),
                            description: None,
                            component: ModeComponent::Color(RgbColor {
                                red: 1,
                                green: 2,
                                blue: 3,
                            }),
                        },
                        ModeControl {
                            id: "intensity".into(),
                            name: "Intensity".into(),
                            description: None,
                            component: ModeComponent::Slider(SliderRange {
                                min: 0,
                                max: 10,
                                default: 5,
                            }),
                        },
                    ],
                }],
            },
            data: b"path".to_vec(),
        }
    }

    fn calls(session: &DeviceSession) -> Vec<String> {
        session
            .runtime
            .module()
            .get::<Table>("calls")
            .unwrap()
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()
            .unwrap()
    }

    #[test]
    fn live_session_enters_renders_and_closes() {
        let device = registered_device();
        let session = DeviceSession::from_test_source(&device, DEVICE_SOURCE).unwrap();
        let mut live = session.enter_live().unwrap();
        live.render(&vec![0, 0, 0]).unwrap();
        live.close().unwrap();

        assert_eq!(
            calls(&live.session),
            ["open", "enter_live", "render", "close"]
        );
    }

    #[test]
    fn standalone_session_applies_plugin_settings_and_closes() {
        let device = registered_device();
        let mut session = DeviceSession::from_test_source(&device, DEVICE_SOURCE).unwrap();
        let settings = device.capabilities.modes[0].default_settings();
        session.apply_mode("solid", &settings).unwrap();
        session.close().unwrap();

        assert_eq!(
            calls(&session),
            ["open", "apply:solid:solid-data:5:1", "close"]
        );
    }
}
