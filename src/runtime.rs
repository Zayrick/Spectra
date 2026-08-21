use std::fs;

use anyhow::{Context, Result, ensure};
use mlua::{Function, Lua, Table};

use crate::hid::HidManager;
use crate::plugin::{PluginMetadata, PluginType};

const LUA_MEMORY_LIMIT: usize = 32 * 1024 * 1024;

pub(crate) struct LuaPluginRuntime {
    module: Table,
    pub lua: Lua,
}

impl LuaPluginRuntime {
    pub fn load(metadata: &PluginMetadata, hid: Option<&HidManager>) -> Result<Self> {
        ensure!(
            (metadata.plugin_type == PluginType::Device) == hid.is_some(),
            "只有 device runtime 可以获得 @rgb/hidapi"
        );
        let source = fs::read_to_string(&metadata.path)
            .with_context(|| format!("读取插件 {} 失败", metadata.path.display()))?;
        Self::from_source(metadata, &source, hid)
    }

    fn from_source(
        metadata: &PluginMetadata,
        source: &str,
        hid: Option<&HidManager>,
    ) -> Result<Self> {
        let lua = Lua::new();
        lua.set_memory_limit(LUA_MEMORY_LIMIT)
            .context("设置 Lua runtime 内存上限失败")?;
        install_require(&lua, hid)?;

        let module: Table = lua
            .load(source)
            .set_name(metadata.path.to_string_lossy())
            .eval()
            .with_context(|| format!("执行插件 {} 失败", metadata.path.display()))?;
        Ok(Self { module, lua })
    }

    #[cfg(test)]
    pub fn module(&self) -> Table {
        self.module.clone()
    }

    pub fn required_function(&self, name: &str) -> Result<Function> {
        self.module
            .get::<Function>(name)
            .with_context(|| format!("插件必须导出函数 {name}()"))
    }

    pub fn optional_function(&self, name: &str) -> Result<Option<Function>> {
        self.module
            .get::<Option<Function>>(name)
            .with_context(|| format!("插件字段 {name} 必须是 function 或 nil"))
    }
}

fn install_require(lua: &Lua, hid: Option<&HidManager>) -> Result<()> {
    let hid = hid.map(|hid| hid.lua_module(lua)).transpose()?;
    let require = lua.create_function(move |_, name: String| {
        if name == "@rgb/hidapi" {
            return hid.clone().ok_or_else(|| {
                mlua::Error::RuntimeError("@rgb/hidapi 在这个 runtime 中不可用".into())
            });
        }
        Err(mlua::Error::RuntimeError(format!("未知 module {name:?}")))
    })?;
    lua.globals().set("require", require)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Value;

    #[test]
    fn effect_runtime_cannot_import_hid_capability() {
        let lua = Lua::new();
        install_require(&lua, None).unwrap();
        let error = lua
            .load("return require('@rgb/hidapi')")
            .eval::<Value>()
            .unwrap_err();
        assert!(error.to_string().contains("不可用"));
    }
}
