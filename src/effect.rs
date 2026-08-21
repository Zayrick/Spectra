use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use mlua::{HookTriggers, LuaString, Table, Value, VmState};

use crate::plugin::{PluginMetadata, PluginType};
use crate::runtime::LuaPluginRuntime;
use crate::types::{ColorFrame, DeviceMatrix};

const HOOK_INSTRUCTIONS: u32 = 10_000;

pub struct ActiveEffect {
    name: String,
    state: Option<Value>,
    matrix: Table,
    color_bytes: usize,
    runtime: LuaPluginRuntime,
}

impl ActiveEffect {
    pub fn start(metadata: &PluginMetadata, matrix: &DeviceMatrix) -> Result<Self> {
        ensure!(
            metadata.plugin_type == PluginType::Effect,
            "{} 不是 effect 插件",
            metadata.name
        );
        let runtime = LuaPluginRuntime::load(metadata, None)?;
        let matrix_table = matrix.to_lua(&runtime.lua)?;
        let context = effect_context(&runtime.lua, matrix_table.clone(), 0.0, 0.0, 0)?;
        let state = match runtime.optional_function("start")? {
            Some(start) => {
                let value: Value = start
                    .call(context)
                    .with_context(|| format!("灯效插件 {} 的 start() 失败", metadata.name))?;
                if matches!(value, Value::Nil) {
                    Value::Table(runtime.lua.create_table()?)
                } else {
                    value
                }
            }
            None => Value::Table(runtime.lua.create_table()?),
        };
        Ok(Self {
            name: metadata.name.clone(),
            state: Some(state),
            matrix: matrix_table,
            color_bytes: matrix.leds.len() * 3,
            runtime,
        })
    }

    pub fn render(
        &self,
        elapsed: f64,
        delta: f64,
        frame_number: u64,
        deadline: Instant,
        stopped: &Arc<AtomicBool>,
    ) -> Result<Option<ColorFrame>> {
        let timed_out = Arc::new(AtomicBool::new(false));
        let hook_timed_out = Arc::clone(&timed_out);
        let hook_stopped = Arc::clone(stopped);
        self.runtime.lua.set_hook(
            HookTriggers::new().every_nth_instruction(HOOK_INSTRUCTIONS),
            move |_, _| {
                if hook_stopped.load(Ordering::Relaxed) {
                    return Err(mlua::Error::RuntimeError("effect worker 已停止".into()));
                }
                if Instant::now() >= deadline {
                    hook_timed_out.store(true, Ordering::Relaxed);
                    return Err(mlua::Error::RuntimeError("effect render() 超时".into()));
                }
                Ok(VmState::Continue)
            },
        )?;

        let result = self.render_frame(elapsed, delta, frame_number);
        self.runtime.lua.remove_hook();
        if stopped.load(Ordering::Relaxed) || timed_out.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match result {
            Ok(_) if Instant::now() >= deadline => Ok(None),
            result => result.map(Some),
        }
    }

    fn render_frame(&self, elapsed: f64, delta: f64, frame_number: u64) -> Result<ColorFrame> {
        let state = self.state.as_ref().context("effect state 已停止")?.clone();
        let context = effect_context(
            &self.runtime.lua,
            self.matrix.clone(),
            elapsed,
            delta,
            frame_number,
        )?;
        let output: LuaString = self
            .runtime
            .required_function("render")?
            .call((state, context))
            .with_context(|| format!("灯效插件 {} 的 render() 失败", self.name))?;
        let colors = output.as_bytes().to_vec();
        ensure!(
            colors.len() == self.color_bytes,
            "effect render() 返回了 {} 字节，当前矩阵需要 {} 字节（每颗 LED 依次为 RGB）",
            colors.len(),
            self.color_bytes
        );
        Ok(colors)
    }

    pub fn stop(&mut self) -> Result<()> {
        let Some(state) = self.state.take() else {
            return Ok(());
        };
        match self.runtime.optional_function("stop")? {
            Some(stop) => stop
                .call::<()>(state)
                .with_context(|| format!("灯效插件 {} 的 stop() 失败", self.name)),
            None => Ok(()),
        }
    }
}

impl Drop for ActiveEffect {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn effect_context(
    lua: &mlua::Lua,
    matrix: Table,
    elapsed: f64,
    delta: f64,
    frame_number: u64,
) -> mlua::Result<Table> {
    let context = lua.create_table_with_capacity(0, 5)?;
    context.set("matrix", matrix)?;
    context.set("elapsed", elapsed)?;
    context.set("delta", delta)?;
    context.set("frame", frame_number)?;
    context.set("target_fps", 60)?;
    Ok(context)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::types::{Led, LedId};

    fn bundled_effect() -> ActiveEffect {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join("effects")
            .join("rainbow.lua");
        let source = fs::read_to_string(&path).unwrap();
        let metadata = PluginMetadata::parse(&path, &source).unwrap();
        let id = LedId::Integer(0);
        let matrix = DeviceMatrix {
            width: 1,
            height: 1,
            cells: vec![vec![Some(id.clone())]],
            leds: vec![Led {
                id,
                name: None,
                x: 0,
                y: 0,
            }],
        };
        ActiveEffect::start(&metadata, &matrix).unwrap()
    }

    #[test]
    fn bundled_rainbow_renders() {
        let effect = bundled_effect();
        let stopped = Arc::new(AtomicBool::new(false));
        let frame = effect
            .render(
                0.0,
                0.0,
                0,
                Instant::now() + std::time::Duration::from_secs(1),
                &stopped,
            )
            .unwrap()
            .unwrap();
        assert_eq!(frame.len(), 3);
    }

    #[test]
    fn slow_render_is_dropped_at_its_deadline() {
        let effect = bundled_effect();
        let render = effect
            .runtime
            .lua
            .load("return function() while true do end end")
            .eval::<mlua::Function>()
            .unwrap();
        effect.runtime.module().set("render", render).unwrap();

        let stopped = Arc::new(AtomicBool::new(false));
        assert!(
            effect
                .render(
                    0.0,
                    0.0,
                    0,
                    Instant::now() + std::time::Duration::from_millis(10),
                    &stopped,
                )
                .unwrap()
                .is_none()
        );

        let render = effect
            .runtime
            .lua
            .load("return function() return string.char(1, 2, 3) end")
            .eval::<mlua::Function>()
            .unwrap();
        effect.runtime.module().set("render", render).unwrap();
        assert_eq!(
            effect
                .render(
                    0.0,
                    0.0,
                    1,
                    Instant::now() + std::time::Duration::from_secs(1),
                    &stopped,
                )
                .unwrap(),
            Some(vec![1, 2, 3])
        );
    }
}
