use std::ffi::CString;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use mlua::{Lua, LuaString, Table, UserData, UserDataMethods, Value};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HidDeviceInfo {
    pub path: Vec<u8>,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial_number: Option<String>,
    pub release_number: u16,
    pub manufacturer_string: Option<String>,
    pub product_string: Option<String>,
    pub usage_page: u16,
    pub usage: u16,
    pub interface_number: i32,
    pub bus_type: String,
}

impl HidDeviceInfo {
    fn from_hidapi(info: &DeviceInfo) -> Self {
        Self {
            path: info.path().to_bytes().to_vec(),
            vendor_id: info.vendor_id(),
            product_id: info.product_id(),
            serial_number: info.serial_number().map(str::to_owned),
            release_number: info.release_number(),
            manufacturer_string: info.manufacturer_string().map(str::to_owned),
            product_string: info.product_string().map(str::to_owned),
            usage_page: info.usage_page(),
            usage: info.usage(),
            interface_number: info.interface_number(),
            bus_type: format!("{:?}", info.bus_type()).to_ascii_lowercase(),
        }
    }

    pub(crate) fn to_lua(&self, lua: &Lua) -> mlua::Result<Table> {
        let table = lua.create_table()?;
        table.set("path", lua.create_string(&self.path)?)?;
        table.set("vendor_id", self.vendor_id)?;
        table.set("product_id", self.product_id)?;
        table.set("serial_number", self.serial_number.clone())?;
        table.set("release_number", self.release_number)?;
        table.set("manufacturer_string", self.manufacturer_string.clone())?;
        table.set("product_string", self.product_string.clone())?;
        table.set("usage_page", self.usage_page)?;
        table.set("usage", self.usage)?;
        table.set("interface_number", self.interface_number)?;
        table.set("bus_type", self.bus_type.as_str())?;
        Ok(table)
    }
}

#[derive(Clone)]
pub struct HidManager {
    inner: Arc<Mutex<HidApi>>,
}

impl HidManager {
    pub fn new() -> Result<Self> {
        let api = HidApi::new().context("初始化 HIDAPI 失败")?;
        Ok(Self {
            inner: Arc::new(Mutex::new(api)),
        })
    }

    pub fn enumerate(&self) -> Result<Vec<HidDeviceInfo>> {
        let mut api = self.lock_api()?;
        api.refresh_devices().context("刷新 HID 设备列表失败")?;
        let mut devices: Vec<_> = api.device_list().map(HidDeviceInfo::from_hidapi).collect();
        devices.sort_by(|left, right| {
            (left.vendor_id, left.product_id, &left.path).cmp(&(
                right.vendor_id,
                right.product_id,
                &right.path,
            ))
        });
        Ok(devices)
    }

    fn lock_api(&self) -> Result<MutexGuard<'_, HidApi>> {
        self.inner
            .lock()
            .map_err(|_| anyhow::anyhow!("HIDAPI 锁已损坏"))
    }

    fn open_path(&self, path: &[u8]) -> Result<HidDevice> {
        let path = CString::new(path).context("HID path 内含 NUL 字节")?;
        self.lock_api()?
            .open_path(&path)
            .with_context(|| format!("打开 HID path {:?} 失败", path.to_string_lossy()))
    }

    fn open(&self, vendor_id: u16, product_id: u16, serial: Option<&str>) -> Result<HidDevice> {
        let api = self.lock_api()?;
        match serial {
            Some(serial) => api
                .open_serial(vendor_id, product_id, serial)
                .with_context(|| {
                    format!(
                        "打开 HID {:04x}:{:04x} serial={serial:?} 失败",
                        vendor_id, product_id
                    )
                }),
            None => api
                .open(vendor_id, product_id)
                .with_context(|| format!("打开 HID {:04x}:{:04x} 失败", vendor_id, product_id)),
        }
    }

    pub(crate) fn lua_module(&self, lua: &Lua) -> mlua::Result<Table> {
        let module = lua.create_table()?;

        let manager = self.clone();
        module.set(
            "enumerate",
            lua.create_function(
                move |lua, (vendor_id, product_id): (Option<u16>, Option<u16>)| {
                    let devices = manager.enumerate().map_err(lua_error)?;
                    let result = lua.create_table()?;
                    for (index, device) in devices
                        .into_iter()
                        .filter(|device| {
                            vendor_id.is_none_or(|value| value == device.vendor_id)
                                && product_id.is_none_or(|value| value == device.product_id)
                        })
                        .enumerate()
                    {
                        result.raw_set(index + 1, device.to_lua(lua)?)?;
                    }
                    Ok(result)
                },
            )?,
        )?;

        let manager = self.clone();
        module.set(
            "open_path",
            lua.create_function(move |lua, path: LuaString| {
                let device = manager
                    .open_path(path.as_bytes().as_ref())
                    .map_err(lua_error)?;
                lua.create_userdata(LuaHidDevice::new(device))
            })?,
        )?;

        let manager = self.clone();
        module.set(
            "open",
            lua.create_function(
                move |lua, (vendor_id, product_id, serial): (u16, u16, Option<String>)| {
                    let device = manager
                        .open(vendor_id, product_id, serial.as_deref())
                        .map_err(lua_error)?;
                    lua.create_userdata(LuaHidDevice::new(device))
                },
            )?,
        )?;

        Ok(module)
    }
}

#[derive(Clone)]
struct LuaHidDevice {
    inner: Arc<Mutex<Option<HidDevice>>>,
}

impl LuaHidDevice {
    fn new(device: HidDevice) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(device))),
        }
    }

    fn with_device<T>(
        &self,
        operation: impl FnOnce(&HidDevice) -> hidapi::HidResult<T>,
    ) -> mlua::Result<T> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| mlua::Error::RuntimeError("HID device 锁已损坏".into()))?;
        let device = guard
            .as_ref()
            .ok_or_else(|| mlua::Error::RuntimeError("HID device 已关闭".into()))?;
        operation(device).map_err(lua_error)
    }
}

impl UserData for LuaHidDevice {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("write", |_, this, data: Value| {
            let data = lua_bytes(data)?;
            this.with_device(|device| device.write(&data))
        });
        methods.add_method("read", |lua, this, length: usize| {
            let mut buffer = checked_buffer(length)?;
            let read = this.with_device(|device| device.read(&mut buffer))?;
            lua.create_string(&buffer[..read])
        });
        methods.add_method(
            "read_timeout",
            |lua, this, (length, timeout_ms): (usize, i32)| {
                let mut buffer = checked_buffer(length)?;
                let read =
                    this.with_device(|device| device.read_timeout(&mut buffer, timeout_ms))?;
                lua.create_string(&buffer[..read])
            },
        );
        methods.add_method("send_feature_report", |_, this, data: Value| {
            let data = lua_bytes(data)?;
            this.with_device(|device| device.send_feature_report(&data))
        });
        methods.add_method(
            "get_feature_report",
            |lua, this, (report_id, length): (u8, usize)| {
                let mut buffer = checked_buffer(length)?;
                buffer[0] = report_id;
                let read = this.with_device(|device| device.get_feature_report(&mut buffer))?;
                lua.create_string(&buffer[..read])
            },
        );
        methods.add_method("send_output_report", |_, this, data: Value| {
            let data = lua_bytes(data)?;
            this.with_device(|device| device.send_output_report(&data))
        });
        methods.add_method("set_blocking_mode", |_, this, blocking: bool| {
            this.with_device(|device| device.set_blocking_mode(blocking))
        });
        methods.add_method("get_manufacturer_string", |_, this, ()| {
            this.with_device(HidDevice::get_manufacturer_string)
        });
        methods.add_method("get_product_string", |_, this, ()| {
            this.with_device(HidDevice::get_product_string)
        });
        methods.add_method("get_serial_number_string", |_, this, ()| {
            this.with_device(HidDevice::get_serial_number_string)
        });
        methods.add_method("close", |_, this, ()| {
            let mut guard = this
                .inner
                .lock()
                .map_err(|_| mlua::Error::RuntimeError("HID device 锁已损坏".into()))?;
            guard.take();
            Ok(())
        });
        methods.add_method("is_open", |_, this, ()| {
            let guard = this
                .inner
                .lock()
                .map_err(|_| mlua::Error::RuntimeError("HID device 锁已损坏".into()))?;
            Ok(guard.is_some())
        });
    }
}

fn checked_buffer(length: usize) -> mlua::Result<Vec<u8>> {
    if !(1..=1024 * 1024).contains(&length) {
        return Err(mlua::Error::RuntimeError(
            "HID buffer 长度必须在 1..1048576".into(),
        ));
    }
    Ok(vec![0; length])
}

fn lua_bytes(value: Value) -> mlua::Result<Vec<u8>> {
    match value {
        Value::String(value) => Ok(value.as_bytes().to_vec()),
        Value::Table(value) => value
            .sequence_values::<i64>()
            .enumerate()
            .map(|(index, value)| {
                let value = value?;
                u8::try_from(value).map_err(|_| {
                    mlua::Error::RuntimeError(format!(
                        "HID byte table 的第 {} 项 {value} 不在 0..255",
                        index + 1
                    ))
                })
            })
            .collect(),
        other => Err(mlua::Error::RuntimeError(format!(
            "HID 数据必须是 binary string 或 byte table，实际收到 {}",
            other.type_name()
        ))),
    }
}

fn lua_error(error: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::RuntimeError(error.to_string())
}
