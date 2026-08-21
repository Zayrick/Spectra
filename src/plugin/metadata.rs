use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::hid::HidDeviceInfo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginType {
    Device,
    Effect,
}

impl fmt::Display for PluginType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device => formatter.write_str("device"),
            Self::Effect => formatter.write_str("effect"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HidDeclaration {
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: Option<i32>,
    pub usage_page: Option<u16>,
    pub usage: Option<u16>,
}

impl HidDeclaration {
    pub fn matches(&self, device: &HidDeviceInfo) -> bool {
        self.vendor_id == device.vendor_id
            && self.product_id == device.product_id
            && self
                .interface_number
                .is_none_or(|value| value == device.interface_number)
            && self
                .usage_page
                .is_none_or(|value| value == device.usage_page)
            && self.usage.is_none_or(|value| value == device.usage)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub plugin_type: PluginType,
    pub author: String,
    pub version: String,
    pub license: String,
    pub source: String,
    pub description: String,
    pub hid: Vec<HidDeclaration>,
    pub path: PathBuf,
}

impl PluginMetadata {
    pub fn parse(path: impl AsRef<Path>, source: &str) -> Result<Self> {
        let path = path.as_ref();
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .context("插件文件名必须是有效 UTF-8")?
            .to_owned();

        let mut name = None;
        let mut plugin_type = None;
        let mut author = None;
        let mut version = None;
        let mut license = None;
        let mut origin = None;
        let mut description = None;
        let mut hid = Vec::new();

        for (line_index, line) in source.lines().enumerate() {
            let Some(annotation) = line.trim_start().strip_prefix("---@") else {
                continue;
            };
            let (key, value) = annotation
                .split_once(char::is_whitespace)
                .map(|(key, value)| (key, value.trim()))
                .unwrap_or((annotation, ""));
            let line_number = line_index + 1;
            match key {
                "plugin" => set_once(&mut name, value, key, line_number)?,
                "plugin-type" => {
                    ensure!(plugin_type.is_none(), "第 {line_number} 行重复声明 @{key}");
                    plugin_type = Some(match value {
                        "device" => PluginType::Device,
                        "effect" => PluginType::Effect,
                        _ => bail!("第 {line_number} 行 @plugin-type 必须是 device 或 effect"),
                    });
                }
                "author" => set_once(&mut author, value, key, line_number)?,
                "version" => set_once(&mut version, value, key, line_number)?,
                "license" => set_once(&mut license, value, key, line_number)?,
                "source" => set_once(&mut origin, value, key, line_number)?,
                "description" => set_once(&mut description, value, key, line_number)?,
                "hid" => hid.push(
                    parse_hid(value)
                        .with_context(|| format!("第 {line_number} 行 @hid 声明无效"))?,
                ),
                _ => {}
            }
        }

        let plugin_type = plugin_type.context("缺少 @plugin-type 注释")?;
        if plugin_type == PluginType::Device {
            ensure!(!hid.is_empty(), "device 插件必须至少声明一个 @hid VID:PID");
        } else {
            ensure!(hid.is_empty(), "effect 插件不能声明 @hid");
        }

        Ok(Self {
            id,
            name: required(name, "plugin")?,
            plugin_type,
            author: required(author, "author")?,
            version: required(version, "version")?,
            license: required(license, "license")?,
            source: required(origin, "source")?,
            description: required(description, "description")?,
            hid,
            path: path.to_owned(),
        })
    }
}

fn required(value: Option<String>, name: &str) -> Result<String> {
    value.with_context(|| format!("缺少 @{name} 注释"))
}

fn set_once(target: &mut Option<String>, value: &str, key: &str, line: usize) -> Result<()> {
    ensure!(target.is_none(), "第 {line} 行重复声明 @{key}");
    ensure!(!value.is_empty(), "第 {line} 行 @{key} 不能为空");
    *target = Some(value.to_owned());
    Ok(())
}

fn parse_hid(value: &str) -> Result<HidDeclaration> {
    let mut fields = value.split_whitespace();
    let pair = fields.next().context("格式应为 0xVID:0xPID")?;
    let (vendor, product) = pair.split_once(':').context("格式应为 0xVID:0xPID")?;
    let mut declaration = HidDeclaration {
        vendor_id: parse_hex_u16(vendor).context("VID 无效")?,
        product_id: parse_hex_u16(product).context("PID 无效")?,
        interface_number: None,
        usage_page: None,
        usage: None,
    };

    for field in fields {
        let (key, value) = field
            .split_once('=')
            .with_context(|| format!("选择器 {field:?} 缺少 ="))?;
        match key {
            "interface" => {
                ensure!(declaration.interface_number.is_none(), "interface 重复");
                declaration.interface_number = Some(parse_i32(value)?);
            }
            "usage-page" => {
                ensure!(declaration.usage_page.is_none(), "usage-page 重复");
                declaration.usage_page = Some(parse_hex_u16(value)?);
            }
            "usage" => {
                ensure!(declaration.usage.is_none(), "usage 重复");
                declaration.usage = Some(parse_hex_u16(value)?);
            }
            _ => bail!("未知 HID 选择器 {key:?}"),
        }
    }
    Ok(declaration)
}

fn parse_hex_u16(value: &str) -> Result<u16> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u16::from_str_radix(digits, 16).with_context(|| format!("{value:?} 不是 16 位十六进制数"))
}

fn parse_i32(value: &str) -> Result<i32> {
    if let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        i32::from_str_radix(digits, 16).with_context(|| format!("{value:?} 不是有效整数"))
    } else {
        value
            .parse::<i32>()
            .with_context(|| format!("{value:?} 不是有效整数"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = r#"
---@plugin Demo Device
---@plugin-type device
---@author Test
---@version 1.2.3
---@license MIT
---@source https://example.com/demo
---@description 测试设备
---@hid 0x1234:0xabcd interface=1 usage-page=0xff00 usage=0x0001
return {}
    "#;

    #[test]
    fn parses_device_metadata() {
        let metadata = PluginMetadata::parse("demo.lua", DEVICE).unwrap();
        assert_eq!(metadata.plugin_type, PluginType::Device);
        assert_eq!(metadata.hid[0].interface_number, Some(1));
    }

    #[test]
    fn device_requires_hid_declaration() {
        let source = DEVICE
            .lines()
            .filter(|line| !line.starts_with("---@hid"))
            .collect::<Vec<_>>()
            .join("\n");
        let error = PluginMetadata::parse("demo.lua", &source).unwrap_err();
        assert!(error.to_string().contains("@hid"));
    }
}
