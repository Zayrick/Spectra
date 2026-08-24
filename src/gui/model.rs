use crate::hid::HidManager;
use crate::plugin::{PluginCatalog, PluginMetadata, RegisteredDevice};

pub(super) fn detect_devices(
    catalog: &PluginCatalog,
    hid: &HidManager,
) -> anyhow::Result<Vec<DeviceOption>> {
    Ok(catalog
        .discover(&hid.enumerate()?, hid)?
        .into_iter()
        .map(DeviceOption)
        .collect())
}

#[derive(Clone, Debug)]
pub(super) struct DeviceOption(pub RegisteredDevice);

impl PartialEq for DeviceOption {
    fn eq(&self, other: &Self) -> bool {
        self.0.key() == other.0.key()
    }
}

impl Eq for DeviceOption {}

#[derive(Clone, Debug)]
pub(super) struct EffectOption(pub PluginMetadata);

impl PartialEq for EffectOption {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for EffectOption {}
