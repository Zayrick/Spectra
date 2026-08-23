#[cfg(target_os = "macos")]
mod platform {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice};
    use skia_safe::gpu::{self, DirectContext, mtl};

    pub(crate) struct Backend {
        direct: DirectContext,
        _native: mtl::BackendContext,
    }

    impl Backend {
        pub(crate) fn new() -> Result<Self, String> {
            autoreleasepool(|_| {
                let device = MTLCreateSystemDefaultDevice()
                    .ok_or_else(|| "没有可用的 Metal GPU device".to_owned())?;
                let queue = device
                    .newCommandQueue()
                    .ok_or_else(|| "创建 Metal command queue 失败".to_owned())?;
                let native = unsafe {
                    mtl::BackendContext::new(
                        Retained::as_ptr(&device) as mtl::Handle,
                        Retained::as_ptr(&queue) as mtl::Handle,
                    )
                };
                let direct = gpu::direct_contexts::make_metal(&native, None)
                    .ok_or_else(|| "创建 Skia Metal DirectContext 失败".to_owned())?;
                Ok(Self {
                    direct,
                    _native: native,
                })
            })
        }

        pub(crate) fn direct_context(&mut self) -> &mut DirectContext {
            &mut self.direct
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use skia_safe::gpu::{self, DirectContext, Protected, d3d};
    use windows::Win32::Graphics::{
        Direct3D::D3D_FEATURE_LEVEL_11_0,
        Direct3D12::{D3D12CreateDevice, ID3D12Device},
        Dxgi::{
            CreateDXGIFactory1, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_NONE,
            DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory4,
        },
    };

    pub(crate) struct Backend {
        direct: DirectContext,
        _native: d3d::BackendContext,
    }

    impl Backend {
        pub(crate) fn new() -> Result<Self, String> {
            let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory1() }
                .map_err(|error| format!("创建 DXGI factory 失败：{error}"))?;
            let (adapter, device) = hardware_device(&factory)?;
            let queue = unsafe { device.CreateCommandQueue(&Default::default()) }
                .map_err(|error| format!("创建 D3D12 command queue 失败：{error}"))?;
            let native = d3d::BackendContext {
                adapter,
                device,
                queue,
                memory_allocator: None,
                protected_context: Protected::No,
            };
            let direct = unsafe { gpu::direct_contexts::make_d3d(&native, None) }
                .ok_or_else(|| "创建 Skia D3D12 DirectContext 失败".to_owned())?;
            Ok(Self {
                direct,
                _native: native,
            })
        }

        pub(crate) fn direct_context(&mut self) -> &mut DirectContext {
            &mut self.direct
        }
    }

    fn hardware_device(factory: &IDXGIFactory4) -> Result<(IDXGIAdapter1, ID3D12Device), String> {
        for index in 0.. {
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
                break;
            };
            let description = unsafe { adapter.GetDesc1() }
                .map_err(|error| format!("读取 DXGI adapter 信息失败：{error}"))?;
            if (DXGI_ADAPTER_FLAG(description.Flags as _) & DXGI_ADAPTER_FLAG_SOFTWARE)
                != DXGI_ADAPTER_FLAG_NONE
            {
                continue;
            }

            let mut device = None;
            if unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }.is_ok() {
                return Ok((
                    adapter,
                    device.expect("D3D12CreateDevice 成功但没有返回 device"),
                ));
            }
        }
        Err("没有可用的硬件 D3D12 adapter".to_owned())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::c_void;
    use std::ptr;

    use ash::vk::{self, Handle};
    use skia_safe::gpu::{self, DirectContext, vk as skia_vk};

    pub(crate) struct Backend {
        direct: Option<DirectContext>,
        device: ash::Device,
        instance: ash::Instance,
        entry: ash::Entry,
    }

    impl Backend {
        pub(crate) fn new() -> Result<Self, String> {
            let entry = unsafe { ash::Entry::load() }
                .map_err(|error| format!("加载 Vulkan loader 失败：{error}"))?;
            let loader_version = unsafe { entry.try_enumerate_instance_version() }
                .map_err(|error| format!("查询 Vulkan 版本失败：{error}"))?
                .unwrap_or(vk::API_VERSION_1_0);
            if loader_version < vk::API_VERSION_1_1 {
                return Err("Skia Vulkan backend 需要 Vulkan 1.1 或更高版本".to_owned());
            }
            let api_version = vk::API_VERSION_1_1;
            let application = vk::ApplicationInfo::default()
                .application_name(c"Spectra")
                .api_version(api_version);
            let instance_info = vk::InstanceCreateInfo::default().application_info(&application);
            let instance = unsafe { entry.create_instance(&instance_info, None) }
                .map_err(|error| format!("创建 Vulkan instance 失败：{error}"))?;

            let selection = select_device(&instance);
            let (physical_device, queue_family) = match selection {
                Ok(Some(selection)) => selection,
                Ok(None) => {
                    unsafe { instance.destroy_instance(None) };
                    return Err("没有可用的硬件 Vulkan graphics queue".to_owned());
                }
                Err(message) => {
                    unsafe { instance.destroy_instance(None) };
                    return Err(message);
                }
            };

            let priorities = [1.0_f32];
            let queue_info = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family)
                .queue_priorities(&priorities)];
            let device_info = vk::DeviceCreateInfo::default().queue_create_infos(&queue_info);
            let device =
                match unsafe { instance.create_device(physical_device, &device_info, None) } {
                    Ok(device) => device,
                    Err(create_error) => {
                        unsafe { instance.destroy_instance(None) };
                        return Err(format!("创建 Vulkan device 失败：{create_error}"));
                    }
                };
            let queue = unsafe { device.get_device_queue(queue_family, 0) };
            let mut backend = Self {
                direct: None,
                device,
                instance,
                entry,
            };

            let direct = {
                let get_proc = |request: skia_vk::GetProcOf| unsafe {
                    match request {
                        skia_vk::GetProcOf::Instance(instance, name) => backend
                            .entry
                            .get_instance_proc_addr(vk::Instance::from_raw(instance as _), name),
                        skia_vk::GetProcOf::Device(device, name) => backend
                            .instance
                            .get_device_proc_addr(vk::Device::from_raw(device as _), name),
                    }
                    .map(|function| function as *const c_void)
                    .unwrap_or(ptr::null())
                };
                let native = unsafe {
                    skia_vk::BackendContext::new_builder(
                        backend.instance.handle().as_raw() as _,
                        physical_device.as_raw() as _,
                        backend.device.handle().as_raw() as _,
                        (queue.as_raw() as _, queue_family as usize),
                        &get_proc,
                        Some(api_version.into()),
                    )
                    .build()
                };
                gpu::direct_contexts::make_vulkan(&native, None)
                    .ok_or_else(|| "创建 Skia Vulkan DirectContext 失败".to_owned())?
            };
            backend.direct = Some(direct);
            Ok(backend)
        }

        pub(crate) fn direct_context(&mut self) -> &mut DirectContext {
            self.direct
                .as_mut()
                .expect("Vulkan backend 缺少 DirectContext")
        }
    }

    impl Drop for Backend {
        fn drop(&mut self) {
            if let Some(mut direct) = self.direct.take() {
                direct.flush_submit_and_sync_cpu();
                drop(direct);
            }
            unsafe {
                let _ = self.device.device_wait_idle();
                self.device.destroy_device(None);
                self.instance.destroy_instance(None);
            }
        }
    }

    fn select_device(
        instance: &ash::Instance,
    ) -> Result<Option<(vk::PhysicalDevice, u32)>, String> {
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|error| format!("枚举 Vulkan physical device 失败：{error}"))?;
        let mut selected = None;
        for physical_device in physical_devices {
            let properties = unsafe { instance.get_physical_device_properties(physical_device) };
            if properties.device_type == vk::PhysicalDeviceType::CPU
                || properties.api_version < vk::API_VERSION_1_1
            {
                continue;
            }
            let queue_family =
                unsafe { instance.get_physical_device_queue_family_properties(physical_device) }
                    .iter()
                    .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS));
            let Some(queue_family) = queue_family else {
                continue;
            };
            let score = match properties.device_type {
                vk::PhysicalDeviceType::DISCRETE_GPU => 3,
                vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
                _ => 1,
            };
            if selected
                .as_ref()
                .is_none_or(|(_, _, selected_score)| score > *selected_score)
            {
                selected = Some((physical_device, queue_family as u32, score));
            }
        }
        Ok(selected.map(|(device, queue, _)| (device, queue)))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("@Spectra/skia GPU backend 仅支持 Linux、macOS 和 Windows");

pub(crate) use platform::Backend;
