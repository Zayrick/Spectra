use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use rgb_core::hid::HidManager;
use rgb_core::plugin::{PluginCatalog, PluginType};

#[derive(Debug, Parser)]
#[command(
    name = "rgb-core",
    version,
    about = "由独立 Lua 5.5 插件驱动的跨平台 RGB 内核"
)]
struct Arguments {
    /// 包含 device/effect Lua 文件的插件目录
    #[arg(long, default_value = "plugins")]
    plugin_dir: PathBuf,

    /// 只解析注释并列出插件，不执行 Lua 或初始化 HID
    #[arg(long)]
    list_plugins: bool,

    /// 运行被 @hid 触发的 device 插件并列出其注册设备
    #[arg(long)]
    list_devices: bool,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let catalog = PluginCatalog::scan(&arguments.plugin_dir)?;

    if arguments.list_plugins {
        print_plugins(&catalog);
    }

    if arguments.list_devices {
        let hid = HidManager::new()?;
        let all_devices = hid.enumerate()?;
        let registered = catalog.discover(&all_devices, &hid)?;
        if registered.is_empty() {
            println!("没有设备插件注册设备。");
        } else {
            for (index, device) in registered.iter().enumerate() {
                println!(
                    "{index}: {} via {} [id={:?}]",
                    device.name,
                    device.plugin.name,
                    device.id_display(),
                );
                if let Some(serial) = &device.serial_number {
                    println!("   serial={serial}");
                }
            }
        }
    }

    if arguments.list_plugins || arguments.list_devices {
        return Ok(());
    }

    let hid = HidManager::new()?;
    rgb_core::gui::run(catalog, hid)?;
    Ok(())
}

fn print_plugins(catalog: &PluginCatalog) {
    for plugin in catalog.plugins() {
        println!(
            "{} ({}) v{} — {}",
            plugin.name, plugin.plugin_type, plugin.version, plugin.description
        );
        println!(
            "   id={} author={} license={} source={}",
            plugin.id, plugin.author, plugin.license, plugin.source
        );
        if plugin.plugin_type == PluginType::Device {
            for declaration in &plugin.hid {
                println!(
                    "   HID {:04x}:{:04x} interface={:?} usage-page={:?} usage={:?}",
                    declaration.vendor_id,
                    declaration.product_id,
                    declaration.interface_number,
                    declaration.usage_page,
                    declaration.usage
                );
            }
        }
    }
}
