use std::collections::{HashMap, HashSet};
use std::fmt;

use anyhow::{Context, Result, bail, ensure};
use mlua::{Lua, LuaString, Table, Value};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum LedId {
    Integer(i64),
    String(String),
}

impl fmt::Display for LedId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Integer(value) => value.fmt(formatter),
            Self::String(value) => value.fmt(formatter),
        }
    }
}

impl LedId {
    pub(crate) fn from_lua(value: Value) -> Result<Self> {
        match value {
            Value::Integer(value) => Ok(Self::Integer(value)),
            Value::String(value) => Ok(Self::String(
                value
                    .to_str()
                    .context("LED 字符串 ID 必须是有效 UTF-8")?
                    .to_owned(),
            )),
            other => bail!("LED ID 必须是整数或字符串，实际收到 {}", other.type_name()),
        }
    }

    pub(crate) fn to_lua(&self, lua: &Lua) -> mlua::Result<Value> {
        match self {
            Self::Integer(value) => Ok(Value::Integer(*value)),
            Self::String(value) => Ok(Value::String(lua.create_string(value)?)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Led {
    pub id: LedId,
    pub name: Option<String>,
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceMatrix {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Vec<Option<LedId>>>,
    pub leds: Vec<Led>,
}

impl DeviceMatrix {
    pub(crate) fn from_lua(table: Table) -> Result<Self> {
        let width: u16 = table.get("width").context("matrix.width 无效或缺失")?;
        let height: u16 = table.get("height").context("matrix.height 无效或缺失")?;
        ensure!(width > 0 && height > 0, "矩阵宽高必须大于 0");

        let rows: Table = table.get("cells").context("matrix.cells 无效或缺失")?;
        ensure!(
            rows.raw_len() == usize::from(height),
            "matrix.cells 有 {} 行，但 height={height}",
            rows.raw_len()
        );

        let mut cells = Vec::with_capacity(usize::from(height));
        let mut leds = Vec::new();
        let mut ids = HashSet::new();

        for y in 0..height {
            let row: Table = rows
                .raw_get(usize::from(y) + 1)
                .with_context(|| format!("matrix.cells 第 {} 行无效", y + 1))?;
            ensure!(
                row.raw_len() == usize::from(width),
                "matrix.cells 第 {} 行宽度为 {}，预期 {width}；空位置必须写 false",
                y + 1,
                row.raw_len()
            );

            let mut parsed_row = Vec::with_capacity(usize::from(width));
            for x in 0..width {
                let cell: Value = row
                    .raw_get(usize::from(x) + 1)
                    .with_context(|| format!("读取矩阵 ({x},{y}) 失败"))?;
                match cell {
                    Value::Boolean(false) | Value::Nil => parsed_row.push(None),
                    Value::Table(cell) => {
                        let id = LedId::from_lua(
                            cell.get::<Value>("id")
                                .with_context(|| format!("矩阵 ({x},{y}) 缺少 LED id"))?,
                        )?;
                        ensure!(ids.insert(id.clone()), "LED ID {id} 在矩阵中重复");
                        let name: Option<String> = cell
                            .get("name")
                            .with_context(|| format!("LED {id} 的 name 必须是字符串或 nil"))?;
                        parsed_row.push(Some(id.clone()));
                        leds.push(Led { id, name, x, y });
                    }
                    other => bail!(
                        "矩阵 ({x},{y}) 必须是 LED table 或 false，实际收到 {}",
                        other.type_name()
                    ),
                }
            }
            cells.push(parsed_row);
        }

        ensure!(!leds.is_empty(), "设备矩阵至少需要一颗 LED");
        Ok(Self {
            width,
            height,
            cells,
            leds,
        })
    }

    pub(crate) fn to_lua(&self, lua: &Lua) -> mlua::Result<Table> {
        let matrix = lua.create_table_with_capacity(0, 4)?;
        matrix.set("width", self.width)?;
        matrix.set("height", self.height)?;

        let led_by_id: HashMap<&LedId, &Led> = self.leds.iter().map(|led| (&led.id, led)).collect();
        let rows = lua.create_table_with_capacity(self.cells.len(), 0)?;
        for (y, cells) in self.cells.iter().enumerate() {
            let row = lua.create_table_with_capacity(cells.len(), 0)?;
            for (x, id) in cells.iter().enumerate() {
                match id {
                    Some(id) => {
                        let led = led_by_id[id];
                        row.raw_set(x + 1, led_to_lua(lua, led)?)?;
                    }
                    None => row.raw_set(x + 1, false)?,
                }
            }
            rows.raw_set(y + 1, row)?;
        }
        matrix.set("cells", rows)?;

        let leds = lua.create_table_with_capacity(self.leds.len(), 0)?;
        for (index, led) in self.leds.iter().enumerate() {
            leds.raw_set(index + 1, led_to_lua(lua, led)?)?;
        }
        matrix.set("leds", leds)?;
        Ok(matrix)
    }
}

fn led_to_lua(lua: &Lua, led: &Led) -> mlua::Result<Table> {
    let table = lua.create_table_with_capacity(0, 4)?;
    table.set("id", led.id.to_lua(lua)?)?;
    if let Some(name) = &led.name {
        table.set("name", name.as_str())?;
    }
    table.set("x", led.x)?;
    table.set("y", led.y)?;
    Ok(table)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    fn from_lua(table: Table) -> Result<Self> {
        Ok(Self {
            red: table.get("red").context("default.red 无效或缺失")?,
            green: table.get("green").context("default.green 无效或缺失")?,
            blue: table.get("blue").context("default.blue 无效或缺失")?,
        })
    }

    fn to_lua(self, lua: &Lua) -> mlua::Result<Table> {
        let color = lua.create_table_with_capacity(0, 3)?;
        color.set("red", self.red)?;
        color.set("green", self.green)?;
        color.set("blue", self.blue)?;
        Ok(color)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SliderRange {
    pub min: i32,
    pub max: i32,
    pub default: i32,
}

impl SliderRange {
    fn from_lua(table: &Table) -> Result<Self> {
        let range = Self {
            min: table.get("min").context("component.min 无效或缺失")?,
            max: table.get("max").context("component.max 无效或缺失")?,
            default: table
                .get("default")
                .context("component.default 无效或缺失")?,
        };
        ensure!(
            range.min <= range.max,
            "component.min 不能大于 component.max"
        );
        ensure!(
            (range.min..=range.max).contains(&range.default),
            "component.default 必须位于 min..=max"
        );
        Ok(range)
    }

    pub fn contains(self, value: i32) -> bool {
        (self.min..=self.max).contains(&value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModeComponent {
    Slider(SliderRange),
    Color(RgbColor),
}

impl ModeComponent {
    fn from_lua(table: Table) -> Result<Self> {
        let component_type: String = table.get("type").context("component.type 无效或缺失")?;
        match component_type.as_str() {
            "slider" => Ok(Self::Slider(SliderRange::from_lua(&table)?)),
            "color" => {
                let default: Table = table
                    .get("default")
                    .context("color component.default 无效或缺失")?;
                Ok(Self::Color(RgbColor::from_lua(default)?))
            }
            _ => bail!("component.type 必须是 slider 或 color"),
        }
    }

    fn to_lua(&self, lua: &Lua) -> mlua::Result<Table> {
        let component = lua.create_table_with_capacity(0, 4)?;
        match self {
            Self::Slider(range) => {
                component.set("type", "slider")?;
                component.set("min", range.min)?;
                component.set("max", range.max)?;
                component.set("default", range.default)?;
            }
            Self::Color(default) => {
                component.set("type", "color")?;
                component.set("default", default.to_lua(lua)?)?;
            }
        }
        Ok(component)
    }

    fn default_value(&self) -> ModeValue {
        match self {
            Self::Slider(range) => ModeValue::Slider(range.default),
            Self::Color(color) => ModeValue::Color(*color),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModeControl {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub component: ModeComponent,
}

impl ModeControl {
    fn from_lua(table: Table, mode_index: usize, control_index: usize) -> Result<Self> {
        let context = || format!("第 {mode_index} 个单机模式的第 {control_index} 个控件");
        let id: String = table
            .get("id")
            .with_context(|| format!("{}缺少字符串 id", context()))?;
        ensure!(!id.trim().is_empty(), "{}的 id 不能为空", context());
        let name: String = table
            .get("name")
            .with_context(|| format!("{}缺少字符串 name", context()))?;
        ensure!(!name.trim().is_empty(), "{}的 name 不能为空", context());
        let description: Option<String> = table
            .get("description")
            .with_context(|| format!("{}的 description 必须是字符串或 nil", context()))?;
        let component: Table = table
            .get("component")
            .with_context(|| format!("{}缺少 component table", context()))?;

        Ok(Self {
            id,
            name,
            description,
            component: ModeComponent::from_lua(component)
                .with_context(|| format!("{}的 component 无效", context()))?,
        })
    }

    pub(crate) fn to_lua(&self, lua: &Lua) -> mlua::Result<Table> {
        let control = lua.create_table_with_capacity(0, 4)?;
        control.set("id", self.id.as_str())?;
        control.set("name", self.name.as_str())?;
        if let Some(description) = &self.description {
            control.set("description", description.as_str())?;
        }
        control.set("component", self.component.to_lua(lua)?)?;
        Ok(control)
    }

    fn validate_value(&self, value: &ModeValue) -> Result<()> {
        match (&self.component, value) {
            (ModeComponent::Slider(range), ModeValue::Slider(value)) => ensure!(
                range.contains(*value),
                "控件 {} 的值 {value} 超出 {}..={} 范围",
                self.name,
                range.min,
                range.max
            ),
            (ModeComponent::Color(_), ModeValue::Color(_)) => {}
            _ => bail!("控件 {} 的值类型与 component.type 不匹配", self.name),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeValue {
    Slider(i32),
    Color(RgbColor),
}

impl ModeValue {
    fn to_lua(self, lua: &Lua) -> mlua::Result<Value> {
        match self {
            Self::Slider(value) => Ok(Value::Integer(i64::from(value))),
            Self::Color(color) => Ok(Value::Table(color.to_lua(lua)?)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceMode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub data: Vec<u8>,
    pub controls: Vec<ModeControl>,
}

impl DeviceMode {
    fn from_lua(table: Table, index: usize) -> Result<Self> {
        let context = || format!("第 {index} 个单机模式");
        let id: String = table
            .get("id")
            .with_context(|| format!("{}缺少字符串 id", context()))?;
        ensure!(!id.trim().is_empty(), "{}的 id 不能为空", context());
        let name: String = table
            .get("name")
            .with_context(|| format!("{}缺少字符串 name", context()))?;
        ensure!(!name.trim().is_empty(), "{}的 name 不能为空", context());
        let description: Option<String> = table
            .get("description")
            .with_context(|| format!("{}的 description 必须是字符串或 nil", context()))?;
        let data = table
            .get::<Option<LuaString>>("data")
            .with_context(|| format!("{}的 data 必须是二进制字符串或 nil", context()))?
            .map(|value| value.as_bytes().to_vec())
            .unwrap_or_else(|| id.as_bytes().to_vec());
        let controls: Option<Table> = table
            .get("controls")
            .with_context(|| format!("{}的 controls 必须是 table 或 nil", context()))?;
        let mut parsed_controls = Vec::new();
        let mut control_ids = HashSet::new();
        if let Some(controls) = controls {
            parsed_controls.reserve(controls.raw_len());
            for control_index in 1..=controls.raw_len() {
                let control: Table = controls.raw_get(control_index).with_context(|| {
                    format!("{}的第 {control_index} 个控件必须是 table", context())
                })?;
                let control = ModeControl::from_lua(control, index, control_index)?;
                ensure!(
                    control_ids.insert(control.id.clone()),
                    "{}的控件 id {:?} 重复",
                    context(),
                    control.id
                );
                parsed_controls.push(control);
            }
        }

        Ok(Self {
            id,
            name,
            description,
            data,
            controls: parsed_controls,
        })
    }

    pub(crate) fn to_lua(&self, lua: &Lua) -> mlua::Result<Table> {
        let mode = lua.create_table_with_capacity(0, 5)?;
        mode.set("id", self.id.as_str())?;
        mode.set("name", self.name.as_str())?;
        if let Some(description) = &self.description {
            mode.set("description", description.as_str())?;
        }
        mode.set("data", lua.create_string(&self.data)?)?;
        if !self.controls.is_empty() {
            let controls = lua.create_table_with_capacity(self.controls.len(), 0)?;
            for (index, control) in self.controls.iter().enumerate() {
                controls.raw_set(index + 1, control.to_lua(lua)?)?;
            }
            mode.set("controls", controls)?;
        }
        Ok(mode)
    }

    pub fn default_settings(&self) -> ModeSettings {
        ModeSettings {
            values: self
                .controls
                .iter()
                .map(|control| (control.id.clone(), control.component.default_value()))
                .collect(),
        }
    }

    pub fn validate_settings(&self, settings: &ModeSettings) -> Result<()> {
        ensure!(
            self.controls.len() == settings.values.len(),
            "模式 {} 的配置数量与控件声明不匹配",
            self.name,
        );
        for control in &self.controls {
            let value = settings
                .values
                .get(&control.id)
                .with_context(|| format!("模式 {} 缺少控件 {} 的值", self.name, control.name))?;
            control.validate_value(value)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCapabilities {
    pub live: bool,
    pub modes: Vec<DeviceMode>,
}

impl DeviceCapabilities {
    pub(crate) fn from_lua(live: Option<bool>, modes: Option<Table>) -> Result<Self> {
        let live = live.unwrap_or(false);
        let modes = match modes {
            Some(modes) => {
                let mut parsed = Vec::with_capacity(modes.raw_len());
                let mut ids = HashSet::new();
                for index in 1..=modes.raw_len() {
                    let mode: Table = modes
                        .raw_get(index)
                        .with_context(|| format!("第 {index} 个单机模式必须是 table"))?;
                    let mode = DeviceMode::from_lua(mode, index)?;
                    ensure!(
                        ids.insert(mode.id.clone()),
                        "单机模式 id {:?} 重复",
                        mode.id
                    );
                    parsed.push(mode);
                }
                parsed
            }
            None => Vec::new(),
        };
        ensure!(
            live || !modes.is_empty(),
            "设备必须声明 live = true 或至少一个单机模式"
        );
        Ok(Self { live, modes })
    }

    pub(crate) fn modes_to_lua(&self, lua: &Lua) -> mlua::Result<Table> {
        let modes = lua.create_table_with_capacity(self.modes.len(), 0)?;
        for (index, mode) in self.modes.iter().enumerate() {
            modes.raw_set(index + 1, mode.to_lua(lua)?)?;
        }
        Ok(modes)
    }

    pub fn mode(&self, id: &str) -> Option<&DeviceMode> {
        self.modes.iter().find(|mode| mode.id == id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModeSettings {
    values: HashMap<String, ModeValue>,
}

impl ModeSettings {
    pub fn get(&self, id: &str) -> Option<&ModeValue> {
        self.values.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ModeValue> {
        self.values.get_mut(id)
    }

    pub(crate) fn to_lua(&self, lua: &Lua) -> mlua::Result<Table> {
        let settings = lua.create_table_with_capacity(0, self.values.len())?;
        for (id, value) in &self.values {
            settings.set(id.as_str(), value.to_lua(lua)?)?;
        }
        Ok(settings)
    }
}

/// Consecutive R, G, B bytes in `DeviceMatrix::leds` order.
pub type ColorFrame = Vec<u8>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_device_capabilities() {
        let lua = Lua::new();
        let modes: Table = lua
            .load(
                r#"
                return {
                    {
                        id = "solid",
                        name = "Solid",
                        description = "Static device effect",
                        controls = {
                            {
                                id = "primary_color",
                                name = "Primary color",
                                description = "Color sent to every LED",
                                component = {
                                    type = "color",
                                    default = { red = 1, green = 2, blue = 3 },
                                },
                            },
                            {
                                id = "intensity",
                                name = "Intensity",
                                component = {
                                    type = "slider",
                                    min = -10,
                                    max = 10,
                                    default = 2,
                                },
                            },
                        },
                    },
                }
                "#,
            )
            .eval()
            .unwrap();

        let capabilities = DeviceCapabilities::from_lua(None, Some(modes)).unwrap();
        let mode = &capabilities.modes[0];
        assert!(!capabilities.live);
        assert_eq!(mode.id, "solid");
        assert_eq!(mode.data, b"solid");
        assert_eq!(
            mode.default_settings().get("primary_color"),
            Some(&ModeValue::Color(RgbColor {
                red: 1,
                green: 2,
                blue: 3,
            }))
        );
        assert_eq!(
            mode.default_settings().get("intensity"),
            Some(&ModeValue::Slider(2))
        );
    }

    #[test]
    fn requires_registered_device_capability() {
        let error = DeviceCapabilities::from_lua(None, None).unwrap_err();
        assert!(error.to_string().contains("live"));
    }

    #[test]
    fn parses_live_device_capability() {
        let capabilities = DeviceCapabilities::from_lua(Some(true), None).unwrap();
        assert!(capabilities.live);
        assert!(capabilities.modes.is_empty());
    }

    #[test]
    fn validates_settings_against_plugin_declared_controls() {
        let mode = DeviceMode {
            id: "cycle".into(),
            name: "Cycle".into(),
            description: None,
            data: b"cycle".to_vec(),
            controls: vec![ModeControl {
                id: "tempo".into(),
                name: "Tempo".into(),
                description: None,
                component: ModeComponent::Slider(SliderRange {
                    min: 1,
                    max: 5,
                    default: 3,
                }),
            }],
        };
        let mut settings = mode.default_settings();
        *settings.get_mut("tempo").unwrap() = ModeValue::Slider(6);
        assert!(mode.validate_settings(&settings).is_err());
    }
}
