use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::device::LiveDeviceSession;
use crate::effect::ActiveEffect;
use crate::hid::HidManager;
use crate::plugin::{PluginMetadata, RegisteredDevice};
use crate::types::{ColorFrame, DeviceMatrix};

mod registry;

pub use registry::{LivePipelineFailure, LivePipelineRegistry};

const EFFECT_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const EFFECT_FADE_DURATION: Duration = Duration::from_millis(200);

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

    fn switch(&mut self, effect: &PluginMetadata) -> Result<()> {
        self.effect.switch(effect)
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
                bail!("停止实时灯效失败：{effect:#}；关闭设备会话时又发生错误：{device:#}")
            }
        }
    }

    pub fn device_name(&self) -> &str {
        &self.device.name
    }

    pub fn effect_id(&self) -> &str {
        self.effect.id()
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

enum EffectCommand {
    Switch(Box<PluginMetadata>),
    Stop,
}

struct EffectWorker {
    id: String,
    stopped: Arc<AtomicBool>,
    commands: Sender<EffectCommand>,
    errors: Receiver<String>,
    thread: Option<JoinHandle<()>>,
}

impl EffectWorker {
    fn start(
        metadata: &PluginMetadata,
        matrix: DeviceMatrix,
        device: Arc<FrameMailbox>,
    ) -> Result<Self> {
        let id = metadata.id.clone();
        let metadata = metadata.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let (commands, command_receiver) = mpsc::channel();
        let (errors, error_receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name(format!("rgb-effect-{}", metadata.id))
            .spawn(move || {
                let result =
                    run_effect(metadata, matrix, device, &worker_stopped, command_receiver);
                if let Err(error) = result
                    && !worker_stopped.load(Ordering::Relaxed)
                {
                    let _ = errors.send(format!("实时灯效运行失败：{error:#}"));
                }
            })
            .context("创建灯效线程失败")?;

        Ok(Self {
            id,
            stopped,
            commands,
            errors: error_receiver,
            thread: Some(thread),
        })
    }

    fn switch(&mut self, metadata: &PluginMetadata) -> Result<()> {
        if self.id == metadata.id {
            return Ok(());
        }
        self.check()?;
        self.commands
            .send(EffectCommand::Switch(Box::new(metadata.clone())))
            .map_err(|_| anyhow::anyhow!("灯效线程已退出"))?;
        self.id.clone_from(&metadata.id);
        Ok(())
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
        if self.thread.is_some() {
            let _ = self.commands.send(EffectCommand::Stop);
        }
        let joined = self.thread.take().map(|thread| thread.join());
        self.stopped.store(true, Ordering::Relaxed);
        ensure!(
            !joined.is_some_and(|result| result.is_err()),
            "灯效线程发生 panic"
        );
        match self.errors.try_recv().ok() {
            Some(error) => bail!(error),
            None => Ok(()),
        }
    }

    fn id(&self) -> &str {
        &self.id
    }
}

impl Drop for EffectWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct RunningEffect {
    id: String,
    effect: ActiveEffect,
    started_at: Instant,
    previous: Instant,
    frame: u64,
    latest: Option<ColorFrame>,
}

impl RunningEffect {
    fn start(metadata: &PluginMetadata, matrix: &DeviceMatrix) -> Result<Self> {
        let started_at = Instant::now();
        Ok(Self {
            id: metadata.id.clone(),
            effect: ActiveEffect::start(metadata, matrix)?,
            started_at,
            previous: started_at,
            frame: 0,
            latest: None,
        })
    }

    fn render(
        &mut self,
        scheduled: Instant,
        deadline: Instant,
        stopped: &Arc<AtomicBool>,
    ) -> Result<bool> {
        let rendered = self.effect.render(
            scheduled
                .saturating_duration_since(self.started_at)
                .as_secs_f64(),
            scheduled
                .saturating_duration_since(self.previous)
                .as_secs_f64(),
            self.frame,
            deadline,
            stopped,
        )?;
        self.previous = scheduled;
        self.frame = self.frame.saturating_add(1);
        if let Some(frame) = rendered {
            self.latest = Some(frame);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn skip_frames(&mut self, count: u64) {
        self.frame = self.frame.saturating_add(count);
    }

    fn stop(&mut self) -> Result<()> {
        self.effect.stop()
    }
}

struct EffectTransition {
    incoming: RunningEffect,
    started_at: Option<Instant>,
}

struct TransitionFrame {
    frame: Option<ColorFrame>,
    updated: bool,
    finished: bool,
}

impl EffectTransition {
    fn start(metadata: &PluginMetadata, matrix: &DeviceMatrix) -> Result<Self> {
        Ok(Self {
            incoming: RunningEffect::start(metadata, matrix)?,
            started_at: None,
        })
    }

    fn render(
        &mut self,
        current: &mut RunningEffect,
        scheduled: Instant,
        stopped: &Arc<AtomicBool>,
    ) -> Result<TransitionFrame> {
        let current_updated =
            current.render(scheduled, Instant::now() + EFFECT_INTERVAL, stopped)?;
        let incoming_updated =
            self.incoming
                .render(scheduled, Instant::now() + EFFECT_INTERVAL, stopped)?;
        if self.started_at.is_none() && incoming_updated {
            self.started_at = Some(Instant::now());
        }

        let Some(started_at) = self.started_at else {
            return Ok(TransitionFrame {
                frame: current.latest.clone(),
                updated: current_updated,
                finished: false,
            });
        };

        let progress = fade_progress(started_at, Instant::now());
        let frame = match (&current.latest, &self.incoming.latest) {
            (Some(current), Some(incoming)) => {
                Some(interpolate_frames(current, incoming, progress))
            }
            (None, Some(incoming)) => Some(incoming.clone()),
            _ => None,
        };
        Ok(TransitionFrame {
            updated: frame.is_some(),
            frame,
            finished: progress >= 1.0,
        })
    }
}

enum OutputFade {
    In {
        started_at: Option<Instant>,
    },
    Steady,
    Out {
        started_at: Instant,
        initial_opacity: f64,
    },
}

impl OutputFade {
    fn fade_in() -> Self {
        Self::In { started_at: None }
    }

    fn begin_fade_out(&mut self, now: Instant) {
        if matches!(self, Self::Out { .. }) {
            return;
        }
        let initial_opacity = self.opacity(now);
        *self = Self::Out {
            started_at: now,
            initial_opacity,
        };
    }

    fn apply(&mut self, frame: &[u8], now: Instant) -> ColorFrame {
        if let Self::In { started_at } = self
            && started_at.is_none()
        {
            *started_at = Some(now);
        }

        let opacity = self.opacity(now);
        if matches!(
            self,
            Self::In {
                started_at: Some(started_at)
            } if fade_progress(*started_at, now) >= 1.0
        ) {
            *self = Self::Steady;
        }
        apply_opacity(frame, opacity)
    }

    fn opacity(&self, now: Instant) -> f64 {
        match self {
            Self::In { started_at: None } => 0.0,
            Self::In {
                started_at: Some(started_at),
            } => fade_progress(*started_at, now),
            Self::Steady => 1.0,
            Self::Out {
                started_at,
                initial_opacity,
            } => initial_opacity * (1.0 - fade_progress(*started_at, now)),
        }
    }

    fn is_active(&self) -> bool {
        !matches!(self, Self::Steady)
    }

    fn is_fading_out(&self) -> bool {
        matches!(self, Self::Out { .. })
    }

    fn faded_out(&self, now: Instant) -> bool {
        matches!(
            self,
            Self::Out { started_at, .. } if fade_progress(*started_at, now) >= 1.0
        )
    }
}

fn run_effect(
    metadata: PluginMetadata,
    matrix: DeviceMatrix,
    device: Arc<FrameMailbox>,
    stopped: &Arc<AtomicBool>,
    commands: Receiver<EffectCommand>,
) -> Result<()> {
    let black = vec![0; matrix.leds.len() * 3];
    device.send_and_wait(black.clone())?;
    let mut current = RunningEffect::start(&metadata, &matrix)?;
    let mut transition: Option<EffectTransition> = None;
    let mut pending: Option<Box<PluginMetadata>> = None;
    let mut output_fade = OutputFade::fade_in();
    let mut scheduled = Instant::now();

    loop {
        match commands.recv_timeout(scheduled.saturating_duration_since(Instant::now())) {
            Ok(EffectCommand::Switch(metadata)) => {
                if !output_fade.is_fading_out() {
                    pending = Some(metadata);
                }
                continue;
            }
            Ok(EffectCommand::Stop) => {
                output_fade.begin_fade_out(Instant::now());
                pending = None;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if stopped.load(Ordering::Relaxed) {
            break;
        }

        if pending.is_some()
            && transition
                .as_ref()
                .is_some_and(|transition| transition.started_at.is_none())
            && let Some(mut waiting) = transition.take()
        {
            waiting.incoming.stop()?;
        }
        if transition.is_none()
            && let Some(metadata) = pending.take()
            && metadata.id != current.id
        {
            transition = Some(EffectTransition::start(&metadata, &matrix)?);
        }

        let rendered = match &mut transition {
            Some(transition) => transition.render(&mut current, scheduled, stopped)?,
            None => {
                let updated = current.render(scheduled, scheduled + EFFECT_INTERVAL, stopped)?;
                TransitionFrame {
                    frame: current.latest.clone(),
                    updated,
                    finished: false,
                }
            }
        };

        if rendered.finished {
            let finished = transition.take().expect("灯效过渡状态应存在");
            current.stop()?;
            current = finished.incoming;
        }

        let now = Instant::now();
        let fade_was_active = output_fade.is_active();
        let output = rendered
            .frame
            .as_deref()
            .map(|frame| output_fade.apply(frame, now));
        if output_fade.faded_out(now) {
            device.send_and_wait(black.clone())?;
            break;
        }
        if (rendered.updated || fade_was_active)
            && let Some(output) = output
        {
            device.send(output)?;
        }

        let skipped = advance_schedule(&mut scheduled, Instant::now());
        current.skip_frames(skipped);
        if let Some(transition) = &mut transition {
            transition.incoming.skip_frames(skipped);
        }
    }

    let current_result = current.stop();
    let incoming_result = match &mut transition {
        Some(transition) => transition.incoming.stop(),
        None => Ok(()),
    };
    match (current_result, incoming_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(current), Err(incoming)) => {
            bail!("停止灯效失败：{current:#}；停止切换中的灯效时又发生错误：{incoming:#}")
        }
    }
}

fn advance_schedule(scheduled: &mut Instant, now: Instant) -> u64 {
    *scheduled += EFFECT_INTERVAL;
    if now < *scheduled {
        return 0;
    }

    let skipped = now.duration_since(*scheduled).as_nanos() / EFFECT_INTERVAL.as_nanos() + 1;
    let skipped = u64::try_from(skipped).unwrap_or(u64::MAX);
    let advance = u32::try_from(skipped).unwrap_or(u32::MAX);
    *scheduled += EFFECT_INTERVAL * advance;
    skipped
}

fn fade_progress(started_at: Instant, now: Instant) -> f64 {
    (now.saturating_duration_since(started_at).as_secs_f64() / EFFECT_FADE_DURATION.as_secs_f64())
        .min(1.0)
}

fn interpolate_frames(current: &[u8], incoming: &[u8], progress: f64) -> ColorFrame {
    debug_assert_eq!(current.len(), incoming.len());
    let progress = progress.clamp(0.0, 1.0);
    current
        .iter()
        .zip(incoming)
        .map(|(&current, &incoming)| {
            let current = f64::from(current);
            let incoming = f64::from(incoming);
            (current + (incoming - current) * progress).round() as u8
        })
        .collect()
}

fn apply_opacity(frame: &[u8], opacity: f64) -> ColorFrame {
    let opacity = opacity.clamp(0.0, 1.0);
    frame
        .iter()
        .map(|&channel| (f64::from(channel) * opacity).round() as u8)
        .collect()
}

struct LiveDeviceWorker {
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
                    if let Err(error) = device.render(&frame.colors) {
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
    latest: Option<QueuedFrame>,
    current: Option<ColorFrame>,
    next_sequence: u64,
    rendered_sequence: u64,
    stopped: bool,
    error: Option<String>,
}

struct QueuedFrame {
    sequence: u64,
    colors: ColorFrame,
}

impl FrameMailbox {
    fn send(&self, frame: ColorFrame) -> Result<()> {
        self.enqueue(frame).map(|_| ())
    }

    fn send_and_wait(&self, frame: ColorFrame) -> Result<()> {
        let sequence = self.enqueue(frame)?;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if state.rendered_sequence >= sequence {
                return Ok(());
            }
            if let Some(error) = &state.error {
                bail!(error.clone());
            }
            ensure!(!state.stopped, "设备已停止");
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn enqueue(&self, frame: ColorFrame) -> Result<u64> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(error) = &state.error {
            bail!(error.clone());
        }
        ensure!(!state.stopped, "设备已停止");
        state.next_sequence = state.next_sequence.saturating_add(1);
        let sequence = state.next_sequence;
        state.latest = Some(QueuedFrame {
            sequence,
            colors: frame,
        }); // 覆盖旧帧，只保留最新一帧。
        self.ready.notify_all();
        Ok(sequence)
    }

    fn next(&self) -> Option<QueuedFrame> {
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

    fn mark_rendered(&self, frame: QueuedFrame) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.rendered_sequence = state.rendered_sequence.max(frame.sequence);
        state.current = Some(frame.colors);
        self.ready.notify_all();
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
    fn fade_progress_is_bounded() {
        let started_at = Instant::now();
        assert_eq!(fade_progress(started_at, started_at), 0.0);
        assert_eq!(
            fade_progress(started_at, started_at + EFFECT_FADE_DURATION / 2),
            0.5
        );
        assert_eq!(
            fade_progress(started_at, started_at + EFFECT_FADE_DURATION * 2),
            1.0
        );
    }

    #[test]
    fn frames_are_interpolated() {
        assert_eq!(
            interpolate_frames(&[0, 0, 0], &[255, 128, 64], 0.5),
            vec![128, 64, 32]
        );
    }

    #[test]
    fn output_fades_in_and_out() {
        let started_at = Instant::now();
        let mut fade = OutputFade::fade_in();
        assert_eq!(fade.apply(&[255, 128, 64], started_at), vec![0, 0, 0]);
        assert_eq!(
            fade.apply(&[255, 128, 64], started_at + EFFECT_FADE_DURATION / 2,),
            vec![128, 64, 32]
        );
        let faded_in_at = started_at + EFFECT_FADE_DURATION;
        assert_eq!(fade.apply(&[255, 128, 64], faded_in_at), vec![255, 128, 64]);

        fade.begin_fade_out(faded_in_at);
        assert_eq!(
            fade.apply(&[255, 128, 64], faded_in_at + EFFECT_FADE_DURATION / 2,),
            vec![128, 64, 32]
        );
        let faded_out_at = faded_in_at + EFFECT_FADE_DURATION;
        assert_eq!(fade.apply(&[255, 128, 64], faded_out_at), vec![0, 0, 0]);
        assert!(fade.faded_out(faded_out_at));
    }

    #[test]
    fn only_the_latest_frame_is_kept() {
        let shared = FrameMailbox::default();
        let latest = vec![4, 5, 6];
        shared.send(vec![1, 2, 3]).unwrap();
        shared.send(latest.clone()).unwrap();
        assert_eq!(shared.next().map(|frame| frame.colors), Some(latest));
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

    #[test]
    fn waited_frame_is_rendered_before_returning() {
        let shared = Arc::new(FrameMailbox::default());
        let worker_shared = Arc::clone(&shared);
        let expected = vec![1, 2, 3];
        let worker = thread::spawn(move || {
            let frame = worker_shared.next().unwrap();
            let colors = frame.colors.clone();
            worker_shared.mark_rendered(frame);
            colors
        });

        shared.send_and_wait(expected.clone()).unwrap();

        assert_eq!(worker.join().unwrap(), expected);
    }
}
