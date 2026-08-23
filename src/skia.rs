use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods, UserDataRef, Value};
use skia_safe::{
    AlphaType, Color, ColorType, ImageInfo, Matrix, Paint, RuntimeEffect, Surface,
    gpu::{Budgeted, DirectContext, SurfaceOrigin, SyncCpu, surfaces},
    runtime_effect::RuntimeShaderBuilder,
};

mod gpu;

const BYTES_PER_PIXEL: usize = 4;
const MAX_SURFACE_BYTES: usize = 32 * 1024 * 1024;
static NEXT_SURFACE_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    // skia-safe GPU handle 不能跨线程；Lua userdata 通过 ID 访问当前 effect worker 的资源。
    static GPU_STATE: RefCell<WorkerGpuState> = RefCell::new(WorkerGpuState::default());
}

pub(crate) fn lua_module(lua: &Lua) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    module.set(
        "surface",
        lua.create_function(|lua, (width, height): (i64, i64)| {
            lua.create_userdata(LuaSkiaSurface::new(width, height)?)
        })?,
    )?;
    module.set(
        "runtime_shader",
        lua.create_function(|lua, source: String| {
            let effect = RuntimeEffect::make_for_shader(source, None).map_err(|error| {
                mlua::Error::RuntimeError(format!("编译 SkSL runtime shader 失败：{error}"))
            })?;
            lua.create_userdata(LuaRuntimeShader {
                builder: Mutex::new(RuntimeShaderBuilder::new(effect)),
            })
        })?,
    )?;
    Ok(module)
}

struct LuaSkiaSurface {
    width: usize,
    height: usize,
    id: u64,
}

#[derive(Default)]
struct WorkerGpuState {
    // Surface 必须先于持有原生 device/queue 的 backend 释放。
    surfaces: HashMap<u64, GpuSurfaceState>,
    backend: Option<gpu::Backend>,
}

struct GpuSurfaceState {
    surface: Surface,
    pixels: Vec<u8>,
    pixels_current: bool,
}

impl WorkerGpuState {
    fn create_surface(
        &mut self,
        id: u64,
        image_info: &ImageInfo,
        bytes: usize,
    ) -> mlua::Result<()> {
        if self.backend.is_none() {
            self.backend = Some(gpu::Backend::new().map_err(mlua::Error::RuntimeError)?);
        }
        let surface = surfaces::render_target(
            self.backend
                .as_mut()
                .expect("GPU backend 已初始化")
                .direct_context(),
            Budgeted::Yes,
            image_info,
            None,
            SurfaceOrigin::TopLeft,
            None,
            false,
            false,
        )
        .ok_or_else(|| mlua::Error::RuntimeError("创建 Skia GPU surface 失败".into()))?;
        self.surfaces.insert(
            id,
            GpuSurfaceState {
                surface,
                pixels: vec![0; bytes],
                pixels_current: false,
            },
        );
        Ok(())
    }

    fn with_surface<T>(
        &mut self,
        id: u64,
        operation: impl FnOnce(&mut GpuSurfaceState, &mut DirectContext) -> mlua::Result<T>,
    ) -> mlua::Result<T> {
        let surface = self.surfaces.get_mut(&id).ok_or_else(|| {
            mlua::Error::RuntimeError("Skia surface 必须在创建它的 effect worker 线程使用".into())
        })?;
        let backend = self
            .backend
            .as_mut()
            .expect("存在 GPU surface 时必须存在 backend");
        operation(surface, backend.direct_context())
    }

    fn remove_surface(&mut self, id: u64) {
        self.surfaces.remove(&id);
        if self.surfaces.is_empty() {
            self.backend = None;
        }
    }
}

impl LuaSkiaSurface {
    fn new(width: i64, height: i64) -> mlua::Result<Self> {
        if width <= 0 || height <= 0 || width > i64::from(i32::MAX) || height > i64::from(i32::MAX)
        {
            return Err(mlua::Error::RuntimeError(
                "Skia surface 宽高必须在 1..2147483647".into(),
            ));
        }

        let width = usize::try_from(width).unwrap();
        let height = usize::try_from(height).unwrap();
        let bytes = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
            .filter(|bytes| *bytes <= MAX_SURFACE_BYTES)
            .ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Skia surface RGBA buffer 不能超过 {} MiB",
                    MAX_SURFACE_BYTES / 1024 / 1024
                ))
            })?;

        let id = NEXT_SURFACE_ID.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            return Err(mlua::Error::RuntimeError("Skia surface ID 已耗尽".into()));
        }
        GPU_STATE.with(|state| {
            state
                .try_borrow_mut()
                .map_err(|_| mlua::Error::RuntimeError("Skia surface 不允许重入调用".into()))?
                .create_surface(id, &image_info(width, height), bytes)
        })?;

        Ok(Self { width, height, id })
    }

    fn image_info(&self) -> ImageInfo {
        image_info(self.width, self.height)
    }

    fn row_bytes(&self) -> usize {
        self.width * BYTES_PER_PIXEL
    }

    fn with_state<T>(
        &self,
        operation: impl FnOnce(&mut GpuSurfaceState, &mut DirectContext) -> mlua::Result<T>,
    ) -> mlua::Result<T> {
        GPU_STATE.with(|state| {
            state
                .try_borrow_mut()
                .map_err(|_| mlua::Error::RuntimeError("Skia surface 不允许重入调用".into()))?
                .with_surface(self.id, operation)
        })
    }
}

impl Drop for LuaSkiaSurface {
    fn drop(&mut self) {
        GPU_STATE.with(|state| {
            state.borrow_mut().remove_surface(self.id);
        });
    }
}

impl GpuSurfaceState {
    fn pixels(
        &mut self,
        direct_context: &mut DirectContext,
        image_info: &ImageInfo,
        row_bytes: usize,
    ) -> mlua::Result<&[u8]> {
        if !self.pixels_current {
            direct_context.flush_and_submit_surface(&mut self.surface, SyncCpu::Yes);
            if !self
                .surface
                .read_pixels(image_info, &mut self.pixels, row_bytes, (0, 0))
            {
                return Err(mlua::Error::RuntimeError(
                    "从 Skia GPU surface 回读像素失败".into(),
                ));
            }
            self.pixels_current = true;
        }
        Ok(&self.pixels)
    }

    fn changed(&mut self) {
        self.pixels_current = false;
    }
}

fn image_info(width: usize, height: usize) -> ImageInfo {
    ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    )
}

impl UserData for LuaSkiaSurface {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("width", |_, this| Ok(this.width));
        fields.add_field_method_get("height", |_, this| Ok(this.height));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "clear",
            |_, this, (red, green, blue, alpha): (u8, u8, u8, Option<u8>)| {
                this.with_state(|state, _| {
                    state.surface.canvas().clear(Color::from_argb(
                        alpha.unwrap_or(255),
                        red,
                        green,
                        blue,
                    ));
                    state.changed();
                    Ok(())
                })
            },
        );

        methods.add_method(
            "draw_shader",
            |_, this, shader: UserDataRef<LuaRuntimeShader>| {
                let shader = shader.make_shader()?;
                let mut paint = Paint::default();
                paint.set_shader(shader);
                this.with_state(|state, _| {
                    state.surface.canvas().draw_paint(&paint);
                    state.changed();
                    Ok(())
                })
            },
        );

        methods.add_method("pixels_rgba", |lua, this, ()| {
            let rgba = this.with_state(|state, direct_context| {
                let pixels = state.pixels(direct_context, &this.image_info(), this.row_bytes())?;
                let mut rgba = Vec::with_capacity(pixels.len());
                for pixel in pixels.chunks_exact(BYTES_PER_PIXEL) {
                    rgba.extend_from_slice(&[
                        unpremultiply(pixel[0], pixel[3]),
                        unpremultiply(pixel[1], pixel[3]),
                        unpremultiply(pixel[2], pixel[3]),
                        pixel[3],
                    ]);
                }
                Ok(rgba)
            })?;
            lua.create_string(rgba)
        });

        methods.add_method("sample_rgb", |lua, this, points: Table| {
            let coordinates = sample_coordinates(points, this.width, this.height)?;
            let rgb = this.with_state(|state, direct_context| {
                let pixels = state.pixels(direct_context, &this.image_info(), this.row_bytes())?;
                let mut rgb =
                    Vec::with_capacity(coordinates.len().checked_mul(3).ok_or_else(|| {
                        mlua::Error::RuntimeError("Skia sample 点数量过大".into())
                    })?);

                for (x, y) in coordinates {
                    let offset = (y * this.width + x) * BYTES_PER_PIXEL;
                    let alpha = pixels[offset + 3];
                    rgb.extend_from_slice(&[
                        unpremultiply(pixels[offset], alpha),
                        unpremultiply(pixels[offset + 1], alpha),
                        unpremultiply(pixels[offset + 2], alpha),
                    ]);
                }

                Ok(rgb)
            })?;
            lua.create_string(rgb)
        });
    }
}

fn sample_coordinates(
    points: Table,
    width: usize,
    height: usize,
) -> mlua::Result<Vec<(usize, usize)>> {
    let point_count = points.raw_len();
    let mut coordinates = Vec::with_capacity(point_count);
    for index in 1..=point_count {
        let point: Table = points.raw_get(index)?;
        let x: i64 = point.get("x")?;
        let y: i64 = point.get("y")?;
        if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
            return Err(mlua::Error::RuntimeError(format!(
                "Skia sample 第 {index} 个坐标 ({x},{y}) 超出 {width}x{height} surface"
            )));
        }
        coordinates.push((x as usize, y as usize));
    }
    Ok(coordinates)
}

struct LuaRuntimeShader {
    builder: Mutex<RuntimeShaderBuilder>,
}

impl LuaRuntimeShader {
    fn lock_builder(&self) -> mlua::Result<MutexGuard<'_, RuntimeShaderBuilder>> {
        self.builder
            .lock()
            .map_err(|_| mlua::Error::RuntimeError("Skia runtime shader 锁已损坏".into()))
    }

    fn make_shader(&self) -> mlua::Result<skia_safe::Shader> {
        self.lock_builder()?
            .make_shader(&Matrix::new_identity())
            .ok_or_else(|| mlua::Error::RuntimeError("创建 Skia runtime shader 实例失败".into()))
    }
}

impl UserData for LuaRuntimeShader {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "set_uniform_float",
            |_, this, (name, values): (String, Value)| {
                let values = float_values(values)?;
                this.lock_builder()?
                    .set_uniform_float(&name, &values)
                    .map_err(|_| uniform_error(&name, "float"))
            },
        );
        methods.add_method(
            "set_uniform_int",
            |_, this, (name, values): (String, Value)| {
                let values = int_values(values)?;
                this.lock_builder()?
                    .set_uniform_int(&name, &values)
                    .map_err(|_| uniform_error(&name, "int"))
            },
        );
    }
}

fn float_values(value: Value) -> mlua::Result<Vec<f32>> {
    match value {
        Value::Integer(value) => checked_float_values([value as f64]),
        Value::Number(value) => checked_float_values([value]),
        Value::Table(values) => {
            let length = values.raw_len();
            if !(1..=16).contains(&length) {
                return Err(mlua::Error::RuntimeError(
                    "Skia float uniform 必须是 number 或长度为 1..16 的数组".into(),
                ));
            }
            checked_float_values(
                (1..=length)
                    .map(|index| values.raw_get::<f64>(index))
                    .collect::<mlua::Result<Vec<_>>>()?,
            )
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "Skia float uniform 必须是 number 或数组，实际收到 {}",
            other.type_name()
        ))),
    }
}

fn checked_float_values(values: impl IntoIterator<Item = f64>) -> mlua::Result<Vec<f32>> {
    values
        .into_iter()
        .map(|value| {
            let converted = value as f32;
            if converted.is_finite() {
                Ok(converted)
            } else {
                Err(mlua::Error::RuntimeError(
                    "Skia float uniform 必须是有限的 f32 数值".into(),
                ))
            }
        })
        .collect()
}

fn int_values(value: Value) -> mlua::Result<Vec<i32>> {
    match value {
        Value::Integer(value) => Ok(vec![checked_int(value)?]),
        Value::Table(values) => {
            let length = values.raw_len();
            if !(1..=4).contains(&length) {
                return Err(mlua::Error::RuntimeError(
                    "Skia int uniform 必须是 integer 或长度为 1..4 的数组".into(),
                ));
            }
            (1..=length)
                .map(|index| values.raw_get::<i64>(index).and_then(checked_int))
                .collect()
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "Skia int uniform 必须是 integer 或数组，实际收到 {}",
            other.type_name()
        ))),
    }
}

fn checked_int(value: i64) -> mlua::Result<i32> {
    i32::try_from(value)
        .map_err(|_| mlua::Error::RuntimeError("Skia int uniform 必须位于 i32 范围".into()))
}

fn uniform_error(name: &str, kind: &str) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "设置 Skia {kind} uniform {name:?} 失败：名称、类型或值数量不匹配"
    ))
}

fn unpremultiply(channel: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        0
    } else {
        ((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_shader_renders_and_samples_rgb() {
        let lua = Lua::new();
        lua.globals()
            .set("skia", lua_module(&lua).unwrap())
            .unwrap();
        let colors: mlua::LuaString = lua
            .load(
                r#"
local shader = skia.runtime_shader([[
uniform float4 color;
half4 main(float2 position) {
    return half4(color);
}
]])
shader:set_uniform_float("color", { 1, 0, 0, 1 })
local surface = skia.surface(2, 1)
surface:draw_shader(shader)
return surface:sample_rgb({ { x = 0, y = 0 }, { x = 1, y = 0 } })
"#,
            )
            .eval()
            .unwrap();

        assert_eq!(colors.as_bytes().as_ref(), &[255, 0, 0, 255, 0, 0]);
    }

    #[test]
    fn surface_rejects_unbounded_pixel_buffers() {
        let error = LuaSkiaSurface::new(4096, 4096)
            .err()
            .expect("超大 surface 应被拒绝");
        assert!(error.to_string().contains("32 MiB"));
    }
}
