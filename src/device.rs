use anyhow::{Context, Result, ensure};
use mlua::Table;

use crate::hid::HidManager;
use crate::plugin::{PluginType, RegisteredDevice};
use crate::runtime::LuaPluginRuntime;
use crate::types::{ColorFrame, DeviceMatrix};

pub struct ActiveDevice {
    plugin_name: String,
    name: String,
    matrix: DeviceMatrix,
    instance: Option<Table>,
    runtime: LuaPluginRuntime,
}

impl ActiveDevice {
    pub fn open(device: &RegisteredDevice, hid: &HidManager) -> Result<Self> {
        let metadata = &device.plugin;
        ensure!(
            metadata.plugin_type == PluginType::Device,
            "{} 不是 device 插件",
            metadata.name
        );

        let runtime = LuaPluginRuntime::load(metadata, Some(hid))?;
        let open = runtime.required_function("open")?;
        let instance: Table = open
            .call(device.to_lua(&runtime.lua)?)
            .with_context(|| format!("设备插件 {} 的 open() 失败", metadata.name))?;

        Ok(Self {
            plugin_name: metadata.name.clone(),
            name: device.name.clone(),
            matrix: device.matrix.clone(),
            instance: Some(instance),
            runtime,
        })
    }

    pub fn render(&self, colors: &ColorFrame) -> Result<()> {
        let instance = self.instance.as_ref().context("device instance 已关闭")?;
        let frame = self.runtime.lua.create_string(colors)?;

        self.runtime
            .required_function("render")?
            .call::<()>((instance.clone(), frame))
            .with_context(|| format!("设备插件 {} 的 render() 失败", self.plugin_name))
    }

    pub fn close(&mut self) -> Result<()> {
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

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn matrix(&self) -> &DeviceMatrix {
        &self.matrix
    }
}

impl Drop for ActiveDevice {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
