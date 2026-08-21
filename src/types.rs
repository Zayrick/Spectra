use std::collections::{HashMap, HashSet};
use std::fmt;

use anyhow::{Context, Result, bail, ensure};
use mlua::{Lua, Table, Value};

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

/// Consecutive R, G, B bytes in `DeviceMatrix::leds` order.
pub type ColorFrame = Vec<u8>;
