use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use super::LivePipeline;
use crate::hid::HidManager;
use crate::plugin::{DeviceKey, PluginMetadata, RegisteredDevice};
use crate::types::ColorFrame;

pub struct LivePipelineFailure {
    pub device: DeviceKey,
    pub device_name: String,
    pub error: anyhow::Error,
}

#[derive(Default)]
pub struct LivePipelineRegistry {
    pipelines: HashMap<DeviceKey, LivePipeline>,
}

impl LivePipelineRegistry {
    pub fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
    }

    pub fn is_running(&self, device: &DeviceKey) -> bool {
        self.pipelines.contains_key(device)
    }

    pub fn effect_id(&self, device: &DeviceKey) -> Option<&str> {
        self.pipelines.get(device).map(LivePipeline::effect_id)
    }

    pub fn current_frame(&self, device: &DeviceKey) -> Option<ColorFrame> {
        self.pipelines
            .get(device)
            .and_then(LivePipeline::current_frame)
    }

    pub fn start(
        &mut self,
        device: &RegisteredDevice,
        effect: &PluginMetadata,
        hid: &HidManager,
    ) -> Result<()> {
        let key = device.key();
        self.stop(&key)?;
        let pipeline = LivePipeline::start(device, effect, hid)?;
        self.pipelines.insert(key, pipeline);
        Ok(())
    }

    pub fn stop(&mut self, device: &DeviceKey) -> Result<()> {
        match self.pipelines.remove(device) {
            Some(mut pipeline) => pipeline.stop(),
            None => Ok(()),
        }
    }

    pub fn poll(&mut self) -> Vec<LivePipelineFailure> {
        let failed: Vec<_> = self
            .pipelines
            .iter()
            .filter_map(|(key, pipeline)| {
                pipeline
                    .poll()
                    .err()
                    .map(|error| (key.clone(), pipeline.device_name().to_owned(), error))
            })
            .collect();

        failed
            .into_iter()
            .map(|(device, device_name, error)| {
                let error = match self.stop(&device) {
                    Ok(()) => error,
                    Err(stop_error) => {
                        anyhow::anyhow!("{error:#}；关闭管线时又发生错误：{stop_error:#}")
                    }
                };
                LivePipelineFailure {
                    device,
                    device_name,
                    error,
                }
            })
            .collect()
    }

    pub fn retain_devices(
        &mut self,
        connected: impl IntoIterator<Item = DeviceKey>,
    ) -> Vec<LivePipelineFailure> {
        let connected: HashSet<_> = connected.into_iter().collect();
        let disconnected: Vec<_> = self
            .pipelines
            .keys()
            .filter(|key| !connected.contains(*key))
            .cloned()
            .collect();

        disconnected
            .into_iter()
            .map(|device| {
                let device_name = self
                    .pipelines
                    .get(&device)
                    .map(|pipeline| pipeline.device_name().to_owned())
                    .unwrap_or_default();
                let error = self
                    .stop(&device)
                    .err()
                    .unwrap_or_else(|| anyhow::anyhow!("设备已断开连接"));
                LivePipelineFailure {
                    device,
                    device_name,
                    error,
                }
            })
            .collect()
    }

    pub fn stop_all(&mut self) -> Result<()> {
        let keys: Vec<_> = self.pipelines.keys().cloned().collect();
        let mut failures = Vec::new();
        for key in keys {
            if let Err(error) = self.stop(&key) {
                failures.push(format!("{error:#}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            bail!("关闭实时管线失败：{}", failures.join("；"))
        }
    }
}

impl Drop for LivePipelineRegistry {
    fn drop(&mut self) {
        let _ = self.stop_all();
    }
}
