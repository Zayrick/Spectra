use std::collections::{HashMap, HashSet};
#[cfg(not(target_os = "macos"))]
use std::sync::LazyLock;
use std::time::Duration;
#[cfg(target_os = "macos")]
use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
    rc::Rc,
};

#[cfg(target_os = "macos")]
use block2::RcBlock;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    button, column, container, mouse_area, pane_grid, row, rule, scrollable, slider, text,
};
#[cfg(not(target_os = "macos"))]
use iced::widget::{svg, tooltip};
#[cfg(target_os = "macos")]
use iced::window::raw_window_handle::RawWindowHandle;
use iced::{Element, Length, Padding, Size, Subscription, Task, Theme, time, window};
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::{AnyObject, ProtocolObject};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSView, NSWindow, NSWindowButton, NSWindowDidUpdateNotification,
    NSWindowWillEnterFullScreenNotification, NSWindowWillExitFullScreenNotification,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSPoint,
};

use crate::device::apply_standalone_mode;
use crate::engine::LivePipelineRegistry;
use crate::hid::HidManager;
use crate::plugin::{DeviceKey, PluginCatalog, RegisteredDevice};
use crate::types::{DeviceMode, ModeComponent, ModeControl, ModeValue, RgbColor, SliderRange};

mod color_picker;
mod device_page;
mod model;
mod preview;
mod style;

use device_page::{ControlPage, DevicePageState, control_page_available};
use model::{DeviceOption, EffectOption, detect_devices};

const HOTPLUG_INTERVAL: Duration = Duration::from_secs(1);
const PREVIEW_INTERVAL: Duration = Duration::from_millis(50);
const PIPELINE_STATUS_INTERVAL: Duration = Duration::from_millis(100);
const WINDOW_SIZE: Size = Size::new(1040.0, 700.0);
const MIN_WINDOW_SIZE: Size = Size::new(820.0, 600.0);
const DEFAULT_SIDEBAR_RATIO: f32 = 200.0 / WINDOW_SIZE.width;
const DEVICE_INFO_HEIGHT: f32 = 180.0;
const DETAIL_PANEL_WIDTH: f32 = 280.0;
const TITLE_BAR_HEIGHT: f32 = 42.0;
const TITLE_BAR_LEADING_PADDING: f32 = 14.0;
const MACOS_TITLE_BAR_LEADING_PADDING: f32 = 87.0;
const CONFIGURATION_INSET: f32 = 5.0;
#[cfg(target_os = "macos")]
const MACOS_WINDOW_CONTROL_OFFSET: f64 = 5.0;
#[cfg(not(target_os = "macos"))]
const WINDOW_CONTROL_ICON_SIZE: f32 = 18.0;

#[cfg(target_os = "macos")]
thread_local! {
    static MACOS_WINDOW_OBSERVERS: RefCell<Option<MacOSWindowObservers>> = const { RefCell::new(None) };
}

#[cfg(not(target_os = "macos"))]
static MINIMIZE_ICON: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("assets/svg/window-min.svg")));
#[cfg(not(target_os = "macos"))]
static MAXIMIZE_ICON: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("assets/svg/window-max.svg")));
#[cfg(not(target_os = "macos"))]
static CLOSE_ICON: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("assets/svg/window-close.svg")));

pub fn run(catalog: PluginCatalog, hid: HidManager) -> iced::Result {
    let boot = move || App::new(catalog.clone(), hid.clone());

    iced::application(boot, App::update, App::view)
        .title("Spectra")
        .theme(Theme::TokyoNightStorm)
        .subscription(App::subscription)
        .window(main_window_settings())
        .centered()
        .run()
}

fn main_window_settings() -> window::Settings {
    let settings = window::Settings {
        size: WINDOW_SIZE,
        min_size: Some(MIN_WINDOW_SIZE),
        decorations: false,
        exit_on_close_request: false,
        ..window::Settings::default()
    };

    #[cfg(target_os = "macos")]
    let settings = {
        let mut settings = settings;
        // A transparent system title bar keeps the native window controls while
        // letting the application title bar fill the whole window frame.
        settings.decorations = true;
        settings.platform_specific.title_hidden = true;
        settings.platform_specific.titlebar_transparent = true;
        settings.platform_specific.fullsize_content_view = true;
        settings
    };

    #[cfg(target_os = "windows")]
    let settings = {
        let mut settings = settings;
        settings.platform_specific.undecorated_shadow = true;
        settings
    };

    settings
}

struct App {
    catalog: PluginCatalog,
    hid: HidManager,
    devices: Vec<DeviceOption>,
    effects: Vec<EffectOption>,
    selected_device: Option<DeviceKey>,
    device_pages: HashMap<DeviceKey, DevicePageState>,
    workspace: pane_grid::State<WorkspacePane>,
    live_pipelines: LivePipelineRegistry,
    pending_mode_actions: HashSet<DeviceKey>,
    close_after_mode_action: Option<window::Id>,
    preview_enabled: bool,
    window_id: Option<window::Id>,
}

#[derive(Clone, Debug)]
enum WorkspacePane {
    Devices,
    Configuration,
}

#[derive(Clone, Debug)]
enum Message {
    DeviceSelected(DeviceKey),
    ControlPageSelected {
        device: DeviceKey,
        page: ControlPage,
    },
    ToggleEffect {
        device: DeviceKey,
        effect: EffectOption,
    },
    DeviceModeSelected {
        device: DeviceKey,
        mode_id: String,
    },
    ModeColorEvent {
        device: DeviceKey,
        control_id: String,
        event: color_picker::Event,
    },
    ModeSliderChanged {
        device: DeviceKey,
        control_id: String,
        value: i32,
    },
    ApplyDeviceMode(DeviceKey),
    ModeActionFinished {
        device: DeviceKey,
        device_name: String,
        mode_name: String,
        result: Result<(), String>,
    },
    TogglePreview,
    WorkspaceResized(pane_grid::ResizeEvent),
    Rescan,
    AutoRescan,
    LivePipelineTick,
    WindowOpened(window::Id),
    DragWindow,
    ToggleMaximize,
    #[cfg(not(target_os = "macos"))]
    MinimizeWindow,
    ShowWindowMenu,
    #[cfg(not(target_os = "macos"))]
    CloseWindow,
    CloseRequested(window::Id),
}

impl App {
    fn new(catalog: PluginCatalog, hid: HidManager) -> Self {
        let effects: Vec<_> = catalog.effects().cloned().map(EffectOption).collect();
        let default_effect_id = effects.first().map(|effect| effect.0.id.as_str());

        let devices = match detect_devices(&catalog, &hid) {
            Ok(devices) => devices,
            Err(error) => {
                eprintln!("初次设备扫描失败：{error:#}");
                Vec::new()
            }
        };
        let selected_device = devices.first().map(|device| device.0.key());
        let device_pages = devices
            .iter()
            .map(|device| {
                (
                    device.0.key(),
                    DevicePageState::new(&device.0.capabilities, default_effect_id),
                )
            })
            .collect();
        let workspace = pane_grid::State::with_configuration(pane_grid::Configuration::Split {
            axis: pane_grid::Axis::Vertical,
            ratio: DEFAULT_SIDEBAR_RATIO,
            a: Box::new(pane_grid::Configuration::Pane(WorkspacePane::Devices)),
            b: Box::new(pane_grid::Configuration::Pane(WorkspacePane::Configuration)),
        });

        Self {
            catalog,
            hid,
            devices,
            effects,
            selected_device,
            device_pages,
            workspace,
            live_pipelines: LivePipelineRegistry::default(),
            pending_mode_actions: HashSet::new(),
            close_after_mode_action: None,
            preview_enabled: true,
            window_id: None,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DeviceSelected(device) => self.select_device(device),
            Message::ControlPageSelected { device, page } => {
                self.select_control_page(&device, page)
            }
            Message::ToggleEffect { device, effect } => self.toggle_effect(&device, effect),
            Message::DeviceModeSelected { device, mode_id } => {
                self.select_device_mode(&device, mode_id)
            }
            Message::ModeColorEvent {
                device,
                control_id,
                event,
            } => {
                if let Some(page) = self.device_pages.get_mut(&device) {
                    page.update_color(&control_id, event);
                }
            }
            Message::ModeSliderChanged {
                device,
                control_id,
                value,
            } => {
                if let Some(ModeValue::Slider(current)) = self
                    .device_pages
                    .get_mut(&device)
                    .and_then(|page| page.mode_settings.as_mut())
                    .and_then(|settings| settings.get_mut(&control_id))
                {
                    *current = value;
                }
            }
            Message::ApplyDeviceMode(device) => return self.apply_selected_mode(&device),
            Message::ModeActionFinished {
                device,
                device_name,
                mode_name,
                result,
            } => return self.finish_mode_action(&device, &device_name, &mode_name, result),
            Message::TogglePreview => self.preview_enabled = !self.preview_enabled,
            Message::WorkspaceResized(event) => {
                self.workspace.resize(event.split, event.ratio);
            }
            Message::Rescan => self.rescan(true),
            Message::AutoRescan => self.rescan(false),
            Message::LivePipelineTick => self.poll_live_pipeline(),
            Message::WindowOpened(window) => {
                self.window_id = Some(window);
                #[cfg(target_os = "macos")]
                return install_macos_window_observers(window);
            }
            Message::DragWindow => return self.window_task(window::drag),
            Message::ToggleMaximize => return self.window_task(window::toggle_maximize),
            #[cfg(not(target_os = "macos"))]
            Message::MinimizeWindow => {
                return self.window_task(|window| window::minimize(window, true));
            }
            Message::ShowWindowMenu => return self.window_task(window::show_system_menu),
            #[cfg(not(target_os = "macos"))]
            Message::CloseWindow => return self.close_window(self.window_id),
            Message::CloseRequested(window) => return self.close_window(Some(window)),
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            time::every(HOTPLUG_INTERVAL).map(|_| Message::AutoRescan),
            window::open_events().map(Message::WindowOpened),
            window::close_requests().map(Message::CloseRequested),
        ];
        if !self.live_pipelines.is_empty() {
            let interval = if self.preview_enabled {
                PREVIEW_INTERVAL
            } else {
                PIPELINE_STATUS_INTERVAL
            };
            subscriptions.push(time::every(interval).map(|_| Message::LivePipelineTick));
        }
        Subscription::batch(subscriptions)
    }

    fn window_task(&self, operation: impl FnOnce(window::Id) -> Task<Message>) -> Task<Message> {
        self.window_id.map(operation).unwrap_or_else(Task::none)
    }

    fn close_window(&mut self, window: Option<window::Id>) -> Task<Message> {
        if !self.pending_mode_actions.is_empty() {
            self.close_after_mode_action = window.or(self.window_id);
            eprintln!("正在完成单机模式操作，HID 会话关闭后退出");
            return Task::none();
        }
        #[cfg(target_os = "macos")]
        remove_macos_window_observers();
        let _ = self.live_pipelines.stop_all();
        window.map(window::close).unwrap_or_else(Task::none)
    }

    fn view(&self) -> Element<'_, Message> {
        let workspace = pane_grid::PaneGrid::new(&self.workspace, |_, pane, _| {
            pane_grid::Content::new(match pane {
                WorkspacePane::Devices => self.device_sidebar(),
                WorkspacePane::Configuration => self.configuration_view(),
            })
        })
        .spacing(0)
        .min_size(176)
        .on_resize(8, Message::WorkspaceResized);

        column![title_bar(), workspace]
            .spacing(0)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn device_sidebar(&self) -> Element<'_, Message> {
        let device_list: Element<'_, Message> = if self.devices.is_empty() {
            container(
                column![
                    text("未发现设备").size(15),
                    text("可用设备会自动出现在这里")
                        .size(12)
                        .style(text::secondary),
                ]
                .spacing(7)
                .align_x(Horizontal::Center),
            )
            .center(Length::Fill)
            .padding(16)
            .into()
        } else {
            let mut items = column![].spacing(7).width(Length::Fill);
            for device in &self.devices {
                items = items.push(self.device_button(device));
            }
            scrollable(items).height(Length::Fill).into()
        };

        container(device_list)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([12, 12])
            .style(style::sidebar)
            .into()
    }

    fn device_button<'a>(&'a self, device: &'a DeviceOption) -> Element<'a, Message> {
        let key = device.0.key();
        let selected = self.selected_device.as_ref() == Some(&key);
        let running = self.live_pipelines.is_running(&key);
        let content = text(&device.0.name).size(14).width(Length::Fill);

        button(content)
            .on_press(Message::DeviceSelected(key))
            .width(Length::Fill)
            .padding([11, 12])
            .style(move |theme, status| style::selectable_button(theme, status, selected, running))
            .into()
    }

    fn configuration_view(&self) -> Element<'_, Message> {
        container(
            container(
                column![
                    self.device_information_panel(),
                    rule::horizontal(1),
                    self.control_workspace(),
                ]
                .spacing(0)
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::configuration_panel),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(
            Padding::ZERO
                .right(CONFIGURATION_INSET)
                .bottom(CONFIGURATION_INSET),
        )
        .style(style::workspace)
        .into()
    }

    fn device_information_panel(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match self.selected_device() {
            Some(device) => {
                let serial = device.serial_number.as_deref().unwrap_or("未提供");

                let identity = container(
                    column![
                        text(&device.name).size(27),
                        text(format!("S/N  {serial}"))
                            .size(12)
                            .style(text::secondary),
                        text(format!("设备来源  {}", device.plugin.name))
                            .size(12)
                            .style(text::secondary),
                    ]
                    .spacing(4),
                )
                .width(Length::FillPortion(5))
                .padding(Padding::ZERO.top(16));

                row![identity, self.lighting_preview(device)]
                    .spacing(28)
                    .height(Length::Fill)
                    .into()
            }
            None => container(
                column![
                    text("尚未选择设备").size(24),
                    text("连接受支持的设备，或点击左侧列表中的设备开始配置")
                        .size(13)
                        .style(text::secondary),
                ]
                .spacing(7)
                .align_x(Horizontal::Center),
            )
            .center_x(Length::Fill)
            .height(Length::Fill)
            .align_y(Vertical::Center)
            .into(),
        };

        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(DEVICE_INFO_HEIGHT))
            .padding([18, 24])
            .style(style::device_info)
            .into()
    }

    fn lighting_preview<'a>(&'a self, device: &'a RegisteredDevice) -> Element<'a, Message> {
        let frame = self
            .preview_enabled
            .then(|| self.live_pipelines.current_frame(&device.key()));

        preview::view(&device.matrix, frame.flatten(), self.preview_enabled)
    }

    fn control_workspace(&self) -> Element<'_, Message> {
        let Some(device) = self.selected_device() else {
            return container(
                column![
                    text("没有可配置的设备").size(17),
                    text("请先从左侧选择一个设备")
                        .size(12)
                        .style(text::secondary),
                ]
                .spacing(7)
                .align_x(Horizontal::Center),
            )
            .center(Length::Fill)
            .into();
        };
        let key = device.key();
        let Some(page) = self.device_pages.get(&key) else {
            return container(text("设备页面状态无效"))
                .center(Length::Fill)
                .into();
        };

        let live_available = device.capabilities.live;
        let standalone_available = !device.capabilities.modes.is_empty();
        let tabs = row![
            button(text("实时").size(13))
                .on_press_maybe(live_available.then_some(Message::ControlPageSelected {
                    device: key.clone(),
                    page: ControlPage::Live,
                }))
                .padding([8, 18])
                .style(|theme, status| {
                    style::selectable_button(
                        theme,
                        status,
                        page.control_page == ControlPage::Live,
                        false,
                    )
                }),
            button(text("单机").size(13))
                .on_press_maybe(
                    standalone_available.then_some(Message::ControlPageSelected {
                        device: key.clone(),
                        page: ControlPage::Standalone,
                    })
                )
                .padding([8, 18])
                .style(|theme, status| {
                    style::selectable_button(
                        theme,
                        status,
                        page.control_page == ControlPage::Standalone,
                        false,
                    )
                }),
        ]
        .spacing(8)
        .padding([10, 20]);

        let content = match page.control_page {
            ControlPage::Live => self.effect_workspace(),
            ControlPage::Standalone => self.standalone_workspace(),
        };

        column![tabs, rule::horizontal(1), content]
            .spacing(0)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn effect_workspace(&self) -> Element<'_, Message> {
        row![
            self.effect_list_panel(),
            rule::vertical(1),
            self.effect_detail_panel(),
        ]
        .spacing(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn effect_list_panel(&self) -> Element<'_, Message> {
        let list: Element<'_, Message> = if self.effects.is_empty() {
            container(
                column![
                    text("没有可用的灯效").size(15),
                    text("向插件目录加入 effect Lua 文件后重启应用")
                        .size(12)
                        .style(text::secondary),
                ]
                .spacing(7)
                .align_x(Horizontal::Center),
            )
            .center(Length::Fill)
            .into()
        } else {
            let mut items = column![].spacing(8).width(Length::Fill);
            for effect in &self.effects {
                items = items.push(self.effect_button(effect));
            }
            scrollable(items).height(Length::Fill).into()
        };

        container(list)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([16, 20])
            .style(style::effect_list)
            .into()
    }

    fn effect_button<'a>(&'a self, effect: &'a EffectOption) -> Element<'a, Message> {
        let Some(device) = self.selected_device.as_ref() else {
            return button(text(effect.0.name.as_str())).into();
        };
        let selected = self
            .device_pages
            .get(device)
            .and_then(|page| page.selected_effect_id.as_deref())
            == Some(effect.0.id.as_str());
        let running = self.effect_is_running(effect);
        let can_toggle = !self.pending_mode_actions.contains(device)
            && self
                .selected_device()
                .is_some_and(|device| device.capabilities.live);

        let content = text(effect.0.name.as_str()).size(15).width(Length::Fill);

        button(content)
            .on_press_maybe(can_toggle.then_some(Message::ToggleEffect {
                device: device.clone(),
                effect: effect.clone(),
            }))
            .width(Length::Fill)
            .padding([13, 12])
            .style(move |theme, status| style::selectable_button(theme, status, selected, running))
            .into()
    }

    fn effect_detail_panel(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match self.selected_effect() {
            Some(effect) => {
                let details = column![
                    text("模式详情").size(11).style(text::secondary),
                    text(effect.0.name.as_str()).size(21),
                    text(effect.0.description.as_str())
                        .size(12)
                        .style(text::secondary),
                    rule::horizontal(1),
                    metadata_row("版本", effect.0.version.as_str()),
                    metadata_row("作者", effect.0.author.as_str()),
                    metadata_row("许可证", effect.0.license.as_str()),
                ]
                .spacing(11)
                .width(Length::Fill);

                scrollable(details).height(Length::Fill).into()
            }
            None => container(
                column![
                    text("模式详情").size(11).style(text::secondary),
                    text("暂无模式").size(20),
                    text("灯效插件会显示在左侧列表中")
                        .size(12)
                        .style(text::secondary),
                ]
                .spacing(10),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        };

        container(content)
            .width(Length::Fixed(DETAIL_PANEL_WIDTH))
            .height(Length::Fill)
            .padding([17, 18])
            .style(style::effect_detail)
            .into()
    }

    fn effect_is_running(&self, effect: &EffectOption) -> bool {
        let Some(device) = self.selected_device.as_ref() else {
            return false;
        };
        self.live_pipelines.effect_id(device) == Some(effect.0.id.as_str())
    }

    fn toggle_effect(&mut self, device: &DeviceKey, effect: EffectOption) {
        let already_running = self.live_pipelines.effect_id(device) == Some(effect.0.id.as_str());
        if let Some(page) = self.device_pages.get_mut(device) {
            page.selected_effect_id = Some(effect.0.id.clone());
        }
        if already_running {
            self.stop_live(device);
        } else {
            self.start_live(device, &effect);
        }
    }

    fn start_live(&mut self, key: &DeviceKey, effect: &EffectOption) {
        let Some(device) = self.device(key).cloned() else {
            eprintln!("没有可启动的设备");
            return;
        };
        if !device.capabilities.live {
            eprintln!("设备 {} 不支持实时控制", device.name);
            return;
        }
        if self.pending_mode_actions.contains(key) {
            eprintln!("单机模式操作尚未完成");
            return;
        }

        match self.live_pipelines.start(&device, &effect.0, &self.hid) {
            Ok(()) => {
                eprintln!(
                    "已启动 {} + {}，灯效与设备均在独立 worker 中运行",
                    device.name, effect.0.name,
                );
            }
            Err(error) => eprintln!("启动失败：{error:#}"),
        }
    }

    fn stop_live(&mut self, device: &DeviceKey) {
        match self.live_pipelines.stop(device) {
            Ok(()) => eprintln!("该设备的实时灯效已停止，设备会话已关闭"),
            Err(error) => eprintln!("停止失败：{error:#}"),
        }
    }

    fn poll_live_pipeline(&mut self) {
        for failure in self.live_pipelines.poll() {
            eprintln!(
                "设备 {} 的渲染管线已停止：{:#}",
                failure.device_name, failure.error
            );
        }
    }

    fn standalone_workspace(&self) -> Element<'_, Message> {
        row![
            self.device_mode_list_panel(),
            rule::vertical(1),
            self.device_mode_detail_panel(),
        ]
        .spacing(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn device_mode_list_panel(&self) -> Element<'_, Message> {
        let list: Element<'_, Message> = match self.selected_device() {
            Some(device) if !device.capabilities.modes.is_empty() => {
                let mut items = column![].spacing(8).width(Length::Fill);
                for mode in &device.capabilities.modes {
                    items = items.push(self.device_mode_button(mode));
                }
                scrollable(items).height(Length::Fill).into()
            }
            _ => container(
                column![
                    text("没有单机模式").size(15),
                    text("该设备插件没有声明设备端灯效")
                        .size(12)
                        .style(text::secondary),
                ]
                .spacing(7)
                .align_x(Horizontal::Center),
            )
            .center(Length::Fill)
            .into(),
        };

        container(list)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([16, 20])
            .style(style::effect_list)
            .into()
    }

    fn device_mode_button<'a>(&'a self, mode: &'a DeviceMode) -> Element<'a, Message> {
        let Some(device) = self.selected_device.as_ref() else {
            return button(text(mode.name.as_str())).into();
        };
        let selected = self
            .device_pages
            .get(device)
            .and_then(|page| page.selected_mode_id.as_deref())
            == Some(mode.id.as_str());
        button(text(mode.name.as_str()).size(15).width(Length::Fill))
            .on_press(Message::DeviceModeSelected {
                device: device.clone(),
                mode_id: mode.id.clone(),
            })
            .width(Length::Fill)
            .padding([13, 12])
            .style(move |theme, status| style::selectable_button(theme, status, selected, false))
            .into()
    }

    fn device_mode_detail_panel(&self) -> Element<'_, Message> {
        let Some(mode) = self.selected_mode() else {
            return container(
                column![
                    text("模式详情").size(11).style(text::secondary),
                    text("暂无单机模式").size(20),
                ]
                .spacing(10),
            )
            .width(Length::Fixed(DETAIL_PANEL_WIDTH))
            .height(Length::Fill)
            .padding([17, 18])
            .style(style::effect_detail)
            .into();
        };
        let Some(page) = self.selected_page() else {
            return container(text("设备页面状态无效")).into();
        };
        let Some(settings) = &page.mode_settings else {
            return container(text("模式参数状态无效"))
                .width(Length::Fixed(DETAIL_PANEL_WIDTH))
                .height(Length::Fill)
                .padding([17, 18])
                .style(style::effect_detail)
                .into();
        };

        let mut details = column![
            text("模式详情").size(11).style(text::secondary),
            text(mode.name.as_str()).size(21),
        ]
        .spacing(12)
        .width(Length::Fill);

        if let Some(description) = &mode.description {
            details = details.push(text(description.as_str()).size(12).style(text::secondary));
        }
        details = details.push(rule::horizontal(1));

        for control in &mode.controls {
            match settings.get(&control.id) {
                Some(value) => details = details.push(self.mode_control(control, value)),
                None => {
                    details = details.push(
                        text(format!("控件 {} 的状态无效", control.name))
                            .size(11)
                            .style(text::danger),
                    );
                }
            }
        }

        let device = self.selected_device.as_ref().expect("已选择设备");
        let available = !self.pending_mode_actions.contains(device);
        let actions = row![
            button(text("应用").size(12))
                .on_press_maybe(available.then_some(Message::ApplyDeviceMode(device.clone())))
                .padding([8, 14])
                .style(button::primary),
        ]
        .spacing(8);
        details = details.push(rule::horizontal(1)).push(actions);
        if !available {
            details = details.push(text("正在写入设备…").size(11).style(text::secondary));
        }

        container(scrollable(details).height(Length::Fill))
            .width(Length::Fixed(DETAIL_PANEL_WIDTH))
            .height(Length::Fill)
            .padding([17, 18])
            .style(style::effect_detail)
            .into()
    }

    fn mode_control<'a>(
        &self,
        control: &'a ModeControl,
        value: &ModeValue,
    ) -> Element<'a, Message> {
        let component: Element<'a, Message> = match (&control.component, value) {
            (ModeComponent::Slider(range), ModeValue::Slider(value)) => {
                self.slider_control(&control.id, *range, *value)
            }
            (ModeComponent::Color(_), ModeValue::Color(color)) => {
                self.color_control(&control.id, *color)
            }
            _ => text("控件值类型无效").size(11).style(text::danger).into(),
        };

        let mut content = column![text(control.name.as_str()).size(12)]
            .spacing(7)
            .width(Length::Fill);
        if let Some(description) = &control.description {
            content = content.push(text(description.as_str()).size(11).style(text::secondary));
        }
        content.push(component).into()
    }

    fn color_control<'a>(&self, control_id: &str, color: RgbColor) -> Element<'a, Message> {
        let Some(device) = self.selected_device.as_ref() else {
            return text("没有选择设备").into();
        };
        let device = device.clone();
        let Some(page) = self.device_pages.get(&device) else {
            return text("设备页面状态无效").into();
        };
        let hex = page
            .color_hex_drafts
            .get(control_id)
            .cloned()
            .unwrap_or_else(|| color_picker::format_hex(color));
        let active = page
            .active_color_picker
            .as_ref()
            .filter(|picker| picker.control_id == control_id);
        let (derived_hue, _, _) = color_picker::rgb_to_hsv(color);
        let hue = active.map_or(derived_hue, |picker| picker.hue);
        let open = active.is_some();
        let control_id = control_id.to_owned();

        color_picker::view(color, &hex, hue, open).map(move |event| Message::ModeColorEvent {
            device: device.clone(),
            control_id: control_id.clone(),
            event,
        })
    }

    fn slider_control<'a>(
        &self,
        control_id: &str,
        range: SliderRange,
        value: i32,
    ) -> Element<'a, Message> {
        let Some(device) = self.selected_device.clone() else {
            return text("没有选择设备").into();
        };
        let control_id = control_id.to_owned();
        column![
            row![
                text(format!("{} – {}", range.min, range.max))
                    .size(11)
                    .style(text::secondary)
                    .width(Length::Fill),
                text(value).size(11),
            ],
            slider(range.min..=range.max, value, move |value| {
                Message::ModeSliderChanged {
                    device: device.clone(),
                    control_id: control_id.clone(),
                    value,
                }
            }),
        ]
        .spacing(6)
        .into()
    }

    fn device(&self, key: &DeviceKey) -> Option<&RegisteredDevice> {
        self.devices
            .iter()
            .find(|device| device.0.key() == *key)
            .map(|device| &device.0)
    }

    fn selected_device(&self) -> Option<&RegisteredDevice> {
        self.device(self.selected_device.as_ref()?)
    }

    fn selected_page(&self) -> Option<&DevicePageState> {
        self.device_pages.get(self.selected_device.as_ref()?)
    }

    fn selected_effect(&self) -> Option<&EffectOption> {
        let effect_id = self.selected_page()?.selected_effect_id.as_deref()?;
        self.effects.iter().find(|effect| effect.0.id == effect_id)
    }

    fn select_device(&mut self, device: DeviceKey) {
        if self.device_pages.contains_key(&device) {
            self.selected_device = Some(device);
        }
    }

    fn select_control_page(&mut self, device: &DeviceKey, page: ControlPage) {
        let available = self
            .device(device)
            .is_some_and(|device| control_page_available(&device.capabilities, page));
        if available && let Some(state) = self.device_pages.get_mut(device) {
            state.control_page = page;
        }
    }

    fn select_device_mode(&mut self, device: &DeviceKey, mode_id: String) {
        let settings = self
            .device(device)
            .and_then(|device| device.capabilities.mode(&mode_id))
            .map(DeviceMode::default_settings);
        if let Some(settings) = settings
            && let Some(page) = self.device_pages.get_mut(device)
        {
            page.selected_mode_id = Some(mode_id);
            page.mode_settings = Some(settings);
            page.reset_color_editor();
        }
    }

    fn selected_mode(&self) -> Option<&DeviceMode> {
        let mode_id = self.selected_page()?.selected_mode_id.as_deref()?;
        self.selected_device()?.capabilities.mode(mode_id)
    }

    fn apply_selected_mode(&mut self, key: &DeviceKey) -> Task<Message> {
        if self.pending_mode_actions.contains(key) {
            return Task::none();
        }
        let Some(device) = self.device(key).cloned() else {
            eprintln!("没有可配置的设备");
            return Task::none();
        };
        let Some(page) = self.device_pages.get(key) else {
            eprintln!("设备页面状态无效");
            return Task::none();
        };
        let Some(mode_id) = page.selected_mode_id.clone() else {
            eprintln!("没有可应用的单机模式");
            return Task::none();
        };
        let Some(settings) = page.mode_settings.clone() else {
            eprintln!("单机模式参数无效");
            return Task::none();
        };
        let Some(mode) = device.capabilities.mode(&mode_id) else {
            eprintln!("设备不再提供所选单机模式");
            return Task::none();
        };
        if let Err(error) = mode.validate_settings(&settings) {
            eprintln!("单机模式参数无效：{error:#}");
            return Task::none();
        }
        let mode_name = mode.name.clone();
        let device_name = device.name.clone();

        if let Err(error) = self.live_pipelines.stop(key) {
            eprintln!("停止该设备的实时管线失败，未应用单机模式：{error:#}");
            return Task::none();
        }

        self.pending_mode_actions.insert(key.clone());
        let task_device = key.clone();
        let hid = self.hid.clone();
        Task::perform(
            async move {
                apply_standalone_mode(&device, &mode_id, &settings, &hid)
                    .map_err(|error| format!("{error:#}"))
            },
            move |result| Message::ModeActionFinished {
                device: task_device.clone(),
                device_name: device_name.clone(),
                mode_name: mode_name.clone(),
                result,
            },
        )
    }

    fn finish_mode_action(
        &mut self,
        device: &DeviceKey,
        device_name: &str,
        mode_name: &str,
        result: Result<(), String>,
    ) -> Task<Message> {
        self.pending_mode_actions.remove(device);
        match result {
            Ok(()) => eprintln!("已向 {device_name} 应用单机模式 {mode_name}"),
            Err(error) => eprintln!("配置 {device_name} 的单机模式失败：{error}"),
        }
        if self.pending_mode_actions.is_empty() {
            match self.close_after_mode_action.take() {
                Some(window) => self.close_window(Some(window)),
                None => Task::none(),
            }
        } else {
            Task::none()
        }
    }

    fn rescan(&mut self, manual: bool) {
        if !self.pending_mode_actions.is_empty() {
            if manual {
                eprintln!("单机模式操作完成后才能重新扫描设备");
            }
            return;
        }
        let devices = match detect_devices(&self.catalog, &self.hid) {
            Ok(devices) => devices,
            Err(error) => {
                eprintln!("设备扫描失败：{error:#}");
                return;
            }
        };
        let changed = self.devices != devices;
        let connected = devices.iter().map(|device| device.0.key());
        let disconnected = self.live_pipelines.retain_devices(connected);

        self.replace_devices(devices);

        for failure in disconnected {
            eprintln!(
                "设备 {} 已拔出，灯效管线已停止：{:#}",
                failure.device_name, failure.error
            );
        }
        if manual {
            eprintln!("重新扫描完成：找到 {} 个支持项", self.devices.len());
        } else if changed {
            eprintln!("设备列表已更新：找到 {} 个支持项", self.devices.len());
        }
    }

    fn replace_devices(&mut self, devices: Vec<DeviceOption>) {
        let default_effect_id = self.effects.first().map(|effect| effect.0.id.as_str());
        let connected: HashSet<_> = devices.iter().map(|device| device.0.key()).collect();
        self.device_pages.retain(|key, _| connected.contains(key));
        for device in &devices {
            self.device_pages
                .entry(device.0.key())
                .and_modify(|page| page.reconcile(&device.0.capabilities, default_effect_id))
                .or_insert_with(|| DevicePageState::new(&device.0.capabilities, default_effect_id));
        }
        if self
            .selected_device
            .as_ref()
            .is_none_or(|key| !connected.contains(key))
        {
            self.selected_device = devices.first().map(|device| device.0.key());
        }
        self.devices = devices;
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct MacOSWindowControlPositions {
    standard: [NSPoint; 3],
    windowed: [NSPoint; 3],
}

#[cfg(target_os = "macos")]
struct MacOSWindowObservers {
    center: Retained<NSNotificationCenter>,
    observers: [Retained<ProtocolObject<dyn NSObjectProtocol>>; 3],
}

#[cfg(target_os = "macos")]
impl Drop for MacOSWindowObservers {
    fn drop(&mut self) {
        for observer in &self.observers {
            // SAFETY: Each value is an observer token issued by this center,
            // and window teardown happens on AppKit's main thread.
            unsafe {
                self.center
                    .removeObserver(AsRef::<AnyObject>::as_ref(&**observer))
            };
        }
    }
}

#[cfg(target_os = "macos")]
fn install_macos_window_observers(window_id: window::Id) -> Task<Message> {
    window::run(window_id, |window| {
        let Ok(window_handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = window_handle.as_raw() else {
            return;
        };

        // SAFETY: The raw handle keeps this NSView valid for the synchronous
        // callback, which iced runs on winit's AppKit event-loop thread.
        let Some(view) = (unsafe { handle.ns_view.as_ptr().cast::<NSView>().as_ref() }) else {
            return;
        };
        let Some(window) = view.window() else {
            return;
        };

        let Some(positions) = macos_window_control_positions(&window) else {
            return;
        };

        remove_macos_window_observers();
        set_macos_window_control_positions(&window, positions.windowed);

        // AppKit owns the title-bar layout while full-screen. During normal
        // window updates, restore the custom offset if AppKit laid it out again.
        let fullscreen = Rc::new(Cell::new(false));
        let center = NSNotificationCenter::defaultCenter();
        let observers = [
            {
                let callback_window = window.clone();
                let fullscreen = Rc::clone(&fullscreen);
                add_macos_window_observer(
                    &center,
                    &window,
                    unsafe { NSWindowDidUpdateNotification },
                    move || {
                        if !fullscreen.get() {
                            set_macos_window_control_positions(
                                &callback_window,
                                positions.windowed,
                            );
                        }
                    },
                )
            },
            {
                let callback_window = window.clone();
                let fullscreen = Rc::clone(&fullscreen);
                add_macos_window_observer(
                    &center,
                    &window,
                    unsafe { NSWindowWillEnterFullScreenNotification },
                    move || {
                        fullscreen.set(true);
                        set_macos_window_control_positions(&callback_window, positions.standard);
                    },
                )
            },
            {
                let callback_window = window.clone();
                add_macos_window_observer(
                    &center,
                    &window,
                    unsafe { NSWindowWillExitFullScreenNotification },
                    move || {
                        fullscreen.set(false);
                        set_macos_window_control_positions(&callback_window, positions.windowed);
                    },
                )
            },
        ];

        MACOS_WINDOW_OBSERVERS.with(|active| {
            *active.borrow_mut() = Some(MacOSWindowObservers { center, observers });
        });
    })
    .discard()
}

#[cfg(target_os = "macos")]
fn add_macos_window_observer(
    center: &NSNotificationCenter,
    window: &NSWindow,
    name: &NSNotificationName,
    handler: impl Fn() + 'static,
) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    let block = RcBlock::new(move |_notification: NonNull<NSNotification>| handler());

    // SAFETY: AppKit posts these window notifications on the main thread. The
    // returned token retains a copy of the block until it is removed on close.
    unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), Some(window), None, &block)
    }
}

#[cfg(target_os = "macos")]
fn remove_macos_window_observers() {
    MACOS_WINDOW_OBSERVERS.with(|active| {
        active.borrow_mut().take();
    });
}

#[cfg(target_os = "macos")]
fn macos_window_control_positions(window: &NSWindow) -> Option<MacOSWindowControlPositions> {
    let buttons = [
        window.standardWindowButton(NSWindowButton::CloseButton)?,
        window.standardWindowButton(NSWindowButton::MiniaturizeButton)?,
        window.standardWindowButton(NSWindowButton::ZoomButton)?,
    ];
    let standard = buttons.each_ref().map(|button| button.frame().origin);
    let windowed = buttons.each_ref().map(|button| {
        let mut origin = button.frame().origin;
        origin.x += MACOS_WINDOW_CONTROL_OFFSET;

        // AppKit title-bar container views can use either coordinate direction.
        let is_flipped =
            unsafe { button.superview() }.is_some_and(|superview| superview.isFlipped());
        origin.y += if is_flipped {
            MACOS_WINDOW_CONTROL_OFFSET
        } else {
            -MACOS_WINDOW_CONTROL_OFFSET
        };
        origin
    });

    Some(MacOSWindowControlPositions { standard, windowed })
}

#[cfg(target_os = "macos")]
fn set_macos_window_control_positions(window: &NSWindow, positions: [NSPoint; 3]) {
    for (kind, position) in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .into_iter()
    .zip(positions)
    {
        let Some(button) = window.standardWindowButton(kind) else {
            continue;
        };
        if button.frame().origin == position {
            continue;
        }

        button.setFrameOrigin(position);
    }
}

fn title_bar<'a>() -> Element<'a, Message> {
    let title = text("Spectra").size(13);
    let leading_padding = if cfg!(target_os = "macos") {
        MACOS_TITLE_BAR_LEADING_PADDING
    } else {
        TITLE_BAR_LEADING_PADDING
    };

    let drag_area = mouse_area(
        container(title)
            .width(Length::Fill)
            .height(Length::Fixed(TITLE_BAR_HEIGHT))
            .padding(Padding::ZERO.left(leading_padding).right(14))
            .align_y(Vertical::Center),
    )
    .on_press(Message::DragWindow)
    .on_double_click(Message::ToggleMaximize)
    .on_right_press(Message::ShowWindowMenu);

    let scan = container(
        button(text("扫描设备").size(12))
            .on_press(Message::Rescan)
            .padding([5.35, 8.0])
            .style(style::scan_button),
    )
    .height(Length::Fill)
    .padding([0, 8])
    .align_y(Vertical::Center);

    let contents = row![drag_area, scan];
    #[cfg(not(target_os = "macos"))]
    let contents = contents.push(
        row![
            window_control_button(
                &MINIMIZE_ICON,
                "最小化",
                Message::MinimizeWindow,
                style::title_bar_button,
            ),
            window_control_button(
                &MAXIMIZE_ICON,
                "最大化或还原",
                Message::ToggleMaximize,
                style::title_bar_button,
            ),
            window_control_button(
                &CLOSE_ICON,
                "关闭",
                Message::CloseWindow,
                style::close_window_button,
            ),
        ]
        .height(Length::Fixed(TITLE_BAR_HEIGHT)),
    );

    container(
        contents
            .width(Length::Fill)
            .height(Length::Fixed(TITLE_BAR_HEIGHT))
            .align_y(Vertical::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(TITLE_BAR_HEIGHT))
    .style(style::title_bar)
    .into()
}

#[cfg(not(target_os = "macos"))]
fn window_control_button<'a>(
    icon: &svg::Handle,
    label: &'a str,
    message: Message,
    style: fn(&Theme, button::Status) -> button::Style,
) -> Element<'a, Message> {
    let icon = container(
        svg(icon.clone())
            .width(Length::Fixed(WINDOW_CONTROL_ICON_SIZE))
            .height(Length::Fixed(WINDOW_CONTROL_ICON_SIZE))
            .style(style::window_control_icon),
    )
    .center(Length::Fill);
    let control = button(icon)
        .on_press(message)
        .width(Length::Fixed(46.0))
        .height(Length::Fill)
        .padding(0)
        .style(style);

    tooltip(
        control,
        container(text(label).size(12))
            .padding([6, 9])
            .style(container::rounded_box),
        tooltip::Position::Bottom,
    )
    .gap(6)
    .into()
}

fn metadata_row<'a>(label: &'a str, value: &'a str) -> Element<'a, Message> {
    row![
        text(label)
            .size(10)
            .style(text::secondary)
            .width(Length::Fill),
        text(value).size(11),
    ]
    .spacing(10)
    .align_y(Vertical::Center)
    .into()
}
