use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use mlua::{Lua, LuaString, Table};

use super::{PluginMetadata, PluginType};
use crate::hid::{HidDeviceInfo, HidManager};
use crate::runtime::LuaPluginRuntime;
use crate::types::DeviceMatrix;

#[derive(Clone, Debug)]
pub struct RegisteredDevice {
    pub plugin: PluginMetadata,
    pub id: Vec<u8>,
    pub name: String,
    pub serial_number: Option<String>,
    pub matrix: DeviceMatrix,
    pub data: Vec<u8>,
}

impl RegisteredDevice {
    pub fn key(&self) -> (&str, &[u8]) {
        (&self.plugin.id, &self.id)
    }

    pub fn id_display(&self) -> String {
        String::from_utf8_lossy(&self.id).into_owned()
    }

    pub(crate) fn to_lua(&self, lua: &Lua) -> mlua::Result<Table> {
        let registration = lua.create_table_with_capacity(0, 5)?;
        registration.set("id", lua.create_string(&self.id)?)?;
        registration.set("name", self.name.as_str())?;
        registration.set("serial_number", self.serial_number.clone())?;
        registration.set("matrix", self.matrix.to_lua(lua)?)?;
        registration.set("data", lua.create_string(&self.data)?)?;
        Ok(registration)
    }
}

#[derive(Clone, Debug)]
pub struct PluginCatalog {
    plugins: Vec<PluginMetadata>,
    devices_by_vid_pid: HashMap<(u16, u16), Vec<usize>>,
    effect_indices: Vec<usize>,
}

impl PluginCatalog {
    pub fn scan(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        ensure!(root.is_dir(), "插件目录 {} 不存在", root.display());

        let mut paths = Vec::new();
        collect_lua_files(root, &mut paths)?;
        paths.sort();

        let mut plugins = Vec::with_capacity(paths.len());
        let mut ids = HashSet::new();
        for path in paths {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("读取插件 {} 失败", path.display()))?;
            let metadata = PluginMetadata::parse(&path, &source)
                .with_context(|| format!("解析插件 {} 的注释失败", path.display()))?;
            ensure!(
                ids.insert(metadata.id.clone()),
                "插件 ID {:?}（Lua 文件名）重复",
                metadata.id
            );
            plugins.push(metadata);
        }
        ensure!(!plugins.is_empty(), "插件目录中没有 .lua 插件");

        let mut devices_by_vid_pid: HashMap<(u16, u16), Vec<usize>> = HashMap::new();
        let mut effect_indices = Vec::new();
        for (index, plugin) in plugins.iter().enumerate() {
            match plugin.plugin_type {
                PluginType::Device => {
                    for hid in &plugin.hid {
                        devices_by_vid_pid
                            .entry((hid.vendor_id, hid.product_id))
                            .or_default()
                            .push(index);
                    }
                }
                PluginType::Effect => effect_indices.push(index),
            }
        }

        Ok(Self {
            plugins,
            devices_by_vid_pid,
            effect_indices,
        })
    }

    pub fn plugins(&self) -> &[PluginMetadata] {
        &self.plugins
    }

    pub fn effects(&self) -> impl ExactSizeIterator<Item = &PluginMetadata> {
        self.effect_indices
            .iter()
            .map(|index| &self.plugins[*index])
    }

    pub fn discover(
        &self,
        devices: &[HidDeviceInfo],
        hid: &HidManager,
    ) -> Result<Vec<RegisteredDevice>> {
        let mut triggered = HashSet::new();
        for device in devices {
            let Some(candidate_indices) = self
                .devices_by_vid_pid
                .get(&(device.vendor_id, device.product_id))
            else {
                continue;
            };

            for index in candidate_indices {
                let plugin = &self.plugins[*index];
                if plugin.hid.iter().any(|filter| filter.matches(device)) {
                    triggered.insert(*index);
                }
            }
        }

        let mut triggered: Vec<_> = triggered.into_iter().collect();
        triggered.sort_unstable();

        let mut registered = Vec::new();
        let mut keys = HashSet::new();
        for index in triggered {
            let plugin = &self.plugins[index];
            for device in discover_plugin(plugin, devices, hid)? {
                ensure!(
                    keys.insert((plugin.id.clone(), device.id.clone())),
                    "设备插件 {} 重复注册 id {:?}",
                    plugin.name,
                    device.id_display()
                );
                registered.push(device);
            }
        }

        registered.sort_by(|left, right| {
            (&left.name, &left.plugin.name, &left.id).cmp(&(
                &right.name,
                &right.plugin.name,
                &right.id,
            ))
        });
        Ok(registered)
    }
}

fn discover_plugin(
    plugin: &PluginMetadata,
    devices: &[HidDeviceInfo],
    hid: &HidManager,
) -> Result<Vec<RegisteredDevice>> {
    let runtime = LuaPluginRuntime::load(plugin, Some(hid))?;
    let hid_list = runtime
        .lua
        .create_table_with_capacity(devices.len(), 0)
        .context("创建设备插件的 HID 枚举列表失败")?;
    for (index, device) in devices.iter().enumerate() {
        hid_list
            .raw_set(index + 1, device.to_lua(&runtime.lua)?)
            .with_context(|| format!("复制第 {} 个 HID 信息到 Lua 失败", index + 1))?;
    }

    let discover = runtime.required_function("discover")?;
    let registrations: Table = discover
        .call(hid_list)
        .with_context(|| format!("设备插件 {} 的 discover() 失败", plugin.name))?;
    let mut devices = Vec::with_capacity(registrations.raw_len());
    for index in 1..=registrations.raw_len() {
        let registration: Table = registrations
            .raw_get(index)
            .with_context(|| format!("设备插件 {} 的第 {index} 个注册项无效", plugin.name))?;
        devices.push(parse_registration(plugin, registration, index)?);
    }
    Ok(devices)
}

fn parse_registration(
    plugin: &PluginMetadata,
    registration: Table,
    index: usize,
) -> Result<RegisteredDevice> {
    let context = || format!("设备插件 {} 的第 {index} 个注册项", plugin.name);
    let id: LuaString = registration
        .get("id")
        .with_context(|| format!("{}缺少二进制字符串 id", context()))?;
    let id = id.as_bytes().to_vec();
    ensure!(!id.is_empty(), "{}的 id 不能为空", context());

    let name: String = registration
        .get("name")
        .with_context(|| format!("{}缺少 UTF-8 字符串 name", context()))?;
    ensure!(!name.trim().is_empty(), "{}的 name 不能为空", context());

    let serial_number: Option<String> = registration
        .get("serial_number")
        .with_context(|| format!("{}的 serial_number 必须是字符串或 nil", context()))?;
    let matrix: Table = registration
        .get("matrix")
        .with_context(|| format!("{}缺少 matrix", context()))?;
    let matrix = DeviceMatrix::from_lua(matrix)
        .with_context(|| format!("{}提供的 matrix 无效", context()))?;
    let data = registration
        .get::<Option<LuaString>>("data")
        .with_context(|| format!("{}的 data 必须是二进制字符串或 nil", context()))?
        .map(|value| value.as_bytes().to_vec())
        .unwrap_or_else(|| id.clone());

    Ok(RegisteredDevice {
        plugin: plugin.clone(),
        id,
        name,
        serial_number,
        matrix,
        data,
    })
}

fn collect_lua_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("读取插件目录 {} 失败", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_lua_files(&path, output)?;
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("lua"))
        {
            output.push(path);
        }
    }
    Ok(())
}
