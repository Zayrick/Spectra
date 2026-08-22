use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::device::LiveDeviceSession;
use crate::effect::ActiveEffect;
use crate::hid::HidManager;
use crate::plugin::{PluginMetadata, RegisteredDevice};
use crate::types::{ColorFrame, DeviceMatrix};

const EFFECT_INTERVAL: Duration = Duration::from_nanos(16_666_667);

pub struct LivePipeline {
    device: LiveDeviceWorker,
    effect: EffectWorker,
}

impl LivePipeline {
    pub fn start(
        device: &RegisteredDevice,
        effect: &PluginMetadata,
        hid: &HidManager,
    ) -> Result<Self> {
        let device = LiveDeviceWorker::start(device, hid)?;
        let effect =
            EffectWorker::start(effect, device.matrix().clone(), Arc::clone(&device.shared))?;
        Ok(Self { device, effect })
    }

    pub fn poll(&self) -> Result<()> {
        self.effect.check()?;
        self.device.check()
    }

    pub fn stop(&mut self) -> Result<()> {
        let effect = self.effect.stop();
        let device = self.device.stop();
        match (effect, device) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(effect), Err(device)) => {
                bail!("停止实时灯效失败：{effect:#}；关闭 HID 会话时又发生错误：{device:#}")
            }
        }
    }

    pub fn matches_device(&self, device: &RegisteredDevice) -> bool {
        self.device.plugin_id == device.plugin.id && self.device.device_id == device.id
    }

    pub fn device_name(&self) -> &str {
        &self.device.name
    }

    pub fn effect_name(&self) -> &str {
        self.effect.name()
    }

    pub fn current_frame(&self) -> Option<ColorFrame> {
        self.device.shared.current_frame()
    }
}

impl Drop for LivePipeline {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct EffectWorker {
    name: String,
    stopped: Arc<AtomicBool>,
    errors: Receiver<String>,
    thread: Option<JoinHandle<()>>,
}

impl EffectWorker {
    fn start(
        metadata: &PluginMetadata,
        matrix: DeviceMatrix,
        device: Arc<FrameMailbox>,
    ) -> Result<Self> {
        let name = metadata.name.clone();
        let worker_name = name.clone();
        let metadata = metadata.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let (errors, error_receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name(format!("rgb-effect-{}", metadata.id))
            .spawn(move || {
                let result = run_effect(metadata, matrix, device, &worker_stopped);
                if let Err(error) = result
                    && !worker_stopped.load(Ordering::Relaxed)
                {
                    let _ = errors.send(format!("灯效 {worker_name} 运行失败：{error:#}"));
                }
            })
            .context("创建灯效线程失败")?;

        Ok(Self {
            name,
            stopped,
            errors: error_receiver,
            thread: Some(thread),
        })
    }

    fn check(&self) -> Result<()> {
        if let Ok(error) = self.errors.try_recv() {
            bail!(error);
        }
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
        {
            bail!("灯效线程已退出");
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            ensure!(thread.join().is_ok(), "灯效线程发生 panic");
        }
        match self.errors.try_recv().ok() {
            Some(error) => bail!(error),
            None => Ok(()),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for EffectWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn run_effect(
    metadata: PluginMetadata,
    matrix: DeviceMatrix,
    device: Arc<FrameMailbox>,
    stopped: &Arc<AtomicBool>,
) -> Result<()> {
    let effect = ActiveEffect::start(&metadata, &matrix)?;
    let started_at = Instant::now();
    let mut scheduled = started_at;
    let mut previous = started_at;
    let mut frame = 0_u64;

    while !stopped.load(Ordering::Relaxed) {
        while Instant::now() < scheduled && !stopped.load(Ordering::Relaxed) {
            thread::park_timeout(scheduled.saturating_duration_since(Instant::now()));
        }
        if stopped.load(Ordering::Relaxed) {
            break;
        }

        let rendered = effect.render(
            scheduled.duration_since(started_at).as_secs_f64(),
            scheduled.duration_since(previous).as_secs_f64(),
            frame,
            scheduled + EFFECT_INTERVAL,
            stopped,
        )?;
        match rendered {
            Some(colors) => device.send(colors)?,
            None if stopped.load(Ordering::Relaxed) => break,
            None => {}
        }

        previous = scheduled;
        frame += 1;
        scheduled += EFFECT_INTERVAL;
        let now = Instant::now();
        if now >= scheduled {
            let skipped = now.duration_since(scheduled).as_nanos() / EFFECT_INTERVAL.as_nanos() + 1;
            let skipped = u64::try_from(skipped).unwrap_or(u64::MAX);
            frame = frame.saturating_add(skipped);
            let advance = u32::try_from(skipped).unwrap_or(u32::MAX);
            scheduled += EFFECT_INTERVAL * advance;
        }
    }
    Ok(())
}

struct LiveDeviceWorker {
    plugin_id: String,
    device_id: Vec<u8>,
    name: String,
    matrix: DeviceMatrix,
    shared: Arc<FrameMailbox>,
    thread: Option<JoinHandle<()>>,
}

impl LiveDeviceWorker {
    fn start(registered: &RegisteredDevice, hid: &HidManager) -> Result<Self> {
        let mut device = LiveDeviceSession::start(registered, hid)?;
        let name = device.name().to_owned();
        let matrix = device.matrix().clone();
        let shared = Arc::new(FrameMailbox::default());
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name(format!("rgb-live-device-{}", registered.plugin.id))
            .spawn(move || {
                while let Some(frame) = worker_shared.next() {
                    if let Err(error) = device.render(&frame) {
                        worker_shared.fail(format!("设备渲染失败：{error:#}"));
                        break;
                    }
                    worker_shared.mark_rendered(frame);
                }
                if let Err(error) = device.close() {
                    worker_shared.fail(format!("关闭设备失败：{error:#}"));
                }
            })
            .context("创建设备线程失败")?;

        Ok(Self {
            plugin_id: registered.plugin.id.clone(),
            device_id: registered.id.clone(),
            name,
            matrix,
            shared,
            thread: Some(thread),
        })
    }

    fn check(&self) -> Result<()> {
        if let Some(error) = self.shared.error() {
            bail!(error);
        }
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
        {
            bail!("设备线程已退出");
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.shared.stop();
        if let Some(thread) = self.thread.take() {
            ensure!(thread.join().is_ok(), "设备线程发生 panic");
        }
        match self.shared.error() {
            Some(error) => bail!(error),
            None => Ok(()),
        }
    }

    fn matrix(&self) -> &DeviceMatrix {
        &self.matrix
    }
}

impl Drop for LiveDeviceWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Default)]
struct FrameMailbox {
    state: Mutex<FrameMailboxState>,
    ready: Condvar,
}

#[derive(Default)]
struct FrameMailboxState {
    latest: Option<ColorFrame>,
    current: Option<ColorFrame>,
    stopped: bool,
    error: Option<String>,
}

impl FrameMailbox {
    fn send(&self, frame: ColorFrame) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(error) = &state.error {
            bail!(error.clone());
        }
        ensure!(!state.stopped, "设备已停止");
        state.latest = Some(frame); // 覆盖旧帧，只保留最新一帧。
        self.ready.notify_one();
        Ok(())
    }

    fn next(&self) -> Option<ColorFrame> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.latest.is_none() && !state.stopped {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        if state.stopped {
            None
        } else {
            state.latest.take()
        }
    }

    fn mark_rendered(&self, frame: ColorFrame) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current = Some(frame);
    }

    fn current_frame(&self) -> Option<ColorFrame> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current
            .clone()
    }

    fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.stopped = true;
        state.latest = None;
        self.ready.notify_all();
    }

    fn fail(&self, error: String) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.error.get_or_insert(error);
        state.stopped = true;
        state.latest = None;
        self.ready.notify_all();
    }

    fn error(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .error
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_frame_is_kept() {
        let shared = FrameMailbox::default();
        let latest = vec![4, 5, 6];
        shared.send(vec![1, 2, 3]).unwrap();
        shared.send(latest.clone()).unwrap();
        assert_eq!(shared.next(), Some(latest));
    }

    #[test]
    fn rendered_frame_is_kept_while_a_new_frame_waits() {
        let shared = FrameMailbox::default();
        let rendered = vec![1, 2, 3];
        shared.send(rendered.clone()).unwrap();
        shared.mark_rendered(shared.next().unwrap());

        shared.send(vec![4, 5, 6]).unwrap();

        assert_eq!(shared.current_frame(), Some(rendered));
    }
}
