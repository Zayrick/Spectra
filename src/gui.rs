use std::sync::LazyLock;
use std::time::Duration;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    button, column, container, mouse_area, pane_grid, row, rule, scrollable, svg, text, tooltip,
};
use iced::{Background, Border, Element, Length, Size, Subscription, Task, Theme, time, window};

use crate::engine::Pipeline;
use crate::hid::HidManager;
use crate::plugin::{PluginCatalog, PluginMetadata, RegisteredDevice};

const HOTPLUG_INTERVAL: Duration = Duration::from_secs(1);
const STATUS_INTERVAL: Duration = Duration::from_millis(100);
const WINDOW_SIZE: Size = Size::new(1040.0, 700.0);
const MIN_WINDOW_SIZE: Size = Size::new(820.0, 600.0);
const DEFAULT_SIDEBAR_RATIO: f32 = 200.0 / WINDOW_SIZE.width;
const DEVICE_INFO_HEIGHT: f32 = 180.0;
const EFFECT_DETAIL_WIDTH: f32 = 260.0;
const TITLE_BAR_HEIGHT: f32 = 42.0;
const WINDOW_CONTROL_ICON_SIZE: f32 = 18.0;

static MINIMIZE_ICON: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("assets/svg/window-min.svg")));
static MAXIMIZE_ICON: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("assets/svg/window-max.svg")));
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
    selected_device: Option<DeviceOption>,
    selected_effect: Option<EffectOption>,
    workspace: pane_grid::State<WorkspacePane>,
    pipeline: Option<Pipeline>,
    window_id: Option<window::Id>,
}

#[derive(Clone, Debug)]
enum WorkspacePane {
    Devices,
    Configuration,
}

#[derive(Clone, Debug)]
enum Message {
    DeviceSelected(DeviceOption),
    ToggleEffect(EffectOption),
    WorkspaceResized(pane_grid::ResizeEvent),
    Rescan,
    AutoRescan,
    PipelineStatus,
    WindowOpened(window::Id),
    DragWindow,
    ToggleMaximize,
    MinimizeWindow,
    ShowWindowMenu,
    CloseWindow,
    CloseRequested(window::Id),
}

impl App {
    fn new(catalog: PluginCatalog, hid: HidManager) -> Self {
        let effects: Vec<_> = catalog.effects().cloned().map(EffectOption).collect();
        let selected_effect = effects.first().cloned();

        let devices = match detect_devices(&catalog, &hid) {
            Ok(devices) => devices,
            Err(error) => {
                eprintln!("初次 HID 扫描失败：{error:#}");
                Vec::new()
            }
        };
        let selected_device = devices.first().cloned();
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
            selected_effect,
            workspace,
            pipeline: None,
            window_id: None,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DeviceSelected(device) => self.selected_device = Some(device),
            Message::ToggleEffect(effect) => self.toggle_effect(effect),
            Message::WorkspaceResized(event) => {
                self.workspace.resize(event.split, event.ratio);
            }
            Message::Rescan => self.rescan(true),
            Message::AutoRescan => self.rescan(false),
            Message::PipelineStatus => self.poll_pipeline(),
            Message::WindowOpened(window) => {
                self.window_id = Some(window);
            }
            Message::DragWindow => return self.window_task(window::drag),
            Message::ToggleMaximize => return self.window_task(window::toggle_maximize),
            Message::MinimizeWindow => {
                return self.window_task(|window| window::minimize(window, true));
            }
            Message::ShowWindowMenu => return self.window_task(window::show_system_menu),
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
        if self.pipeline.is_some() {
            subscriptions.push(time::every(STATUS_INTERVAL).map(|_| Message::PipelineStatus));
        }
        Subscription::batch(subscriptions)
    }

    fn window_task(&self, operation: impl FnOnce(window::Id) -> Task<Message>) -> Task<Message> {
        self.window_id.map(operation).unwrap_or_else(Task::none)
    }

    fn close_window(&mut self, window: Option<window::Id>) -> Task<Message> {
        let _ = self.stop_pipeline();
        window.map(window::close).unwrap_or_else(Task::none)
    }

    fn view(&self) -> Element<'_, Message> {
        let workspace = pane_grid::PaneGrid::new(&self.workspace, |_, pane, _| {
            pane_grid::Content::new(match pane {
                WorkspacePane::Devices => self.device_sidebar(),
                WorkspacePane::Configuration => self.configuration_view(),
            })
        })
        .spacing(1)
        .min_size(176)
        .on_resize(8, Message::WorkspaceResized);

        column![title_bar(), rule::horizontal(1), workspace]
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
                    text("连接受支持的 HID 设备后会自动出现在这里")
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
            .style(sidebar_style)
            .into()
    }

    fn device_button<'a>(&'a self, device: &'a DeviceOption) -> Element<'a, Message> {
        let selected = self.selected_device.as_ref() == Some(device);
        let running = self
            .pipeline
            .as_ref()
            .is_some_and(|pipeline| pipeline.matches_device(&device.0));
        let content = text(&device.0.name).size(14).width(Length::Fill);

        button(content)
            .on_press(Message::DeviceSelected(device.clone()))
            .width(Length::Fill)
            .padding([11, 12])
            .style(move |theme, status| selectable_button_style(theme, status, selected, running))
            .into()
    }

    fn configuration_view(&self) -> Element<'_, Message> {
        container(
            column![
                self.device_information_panel(),
                rule::horizontal(1),
                self.effect_workspace(),
            ]
            .spacing(0)
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(workspace_style)
        .into()
    }

    fn device_information_panel(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match &self.selected_device {
            Some(DeviceOption(device)) => {
                let serial = device.serial_number.as_deref().unwrap_or("未提供");

                let identity = column![
                    text("设备信息").size(11).style(text::secondary),
                    text(&device.name).size(27),
                    text(format!("S/N  {serial}"))
                        .size(12)
                        .style(text::secondary),
                    text(format!("来源插件  {}", device.plugin.name))
                        .size(12)
                        .style(text::secondary),
                ]
                .spacing(4)
                .width(Length::Fill);

                identity.into()
            }
            None => column![
                text("设备信息").size(11).style(text::secondary),
                container(
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
                .align_y(Vertical::Center),
            ]
            .spacing(12)
            .into(),
        };

        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(DEVICE_INFO_HEIGHT))
            .padding([18, 24])
            .style(device_info_style)
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
            .style(effect_list_style)
            .into()
    }

    fn effect_button<'a>(&'a self, effect: &'a EffectOption) -> Element<'a, Message> {
        let selected = self.selected_effect.as_ref() == Some(effect);
        let running = self.effect_is_running(effect);
        let can_toggle = self.selected_device.is_some();

        let content = text(effect.0.name.as_str()).size(15).width(Length::Fill);

        button(content)
            .on_press_maybe(can_toggle.then_some(Message::ToggleEffect(effect.clone())))
            .width(Length::Fill)
            .padding([13, 12])
            .style(move |theme, status| selectable_button_style(theme, status, selected, running))
            .into()
    }

    fn effect_detail_panel(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match &self.selected_effect {
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
            .width(Length::Fixed(EFFECT_DETAIL_WIDTH))
            .height(Length::Fill)
            .padding([17, 18])
            .style(effect_detail_style)
            .into()
    }

    fn effect_is_running(&self, effect: &EffectOption) -> bool {
        let Some(DeviceOption(device)) = &self.selected_device else {
            return false;
        };
        self.pipeline.as_ref().is_some_and(|pipeline| {
            pipeline.matches_device(device) && pipeline.effect_name() == effect.0.name
        })
    }

    fn toggle_effect(&mut self, effect: EffectOption) {
        let already_running = self.effect_is_running(&effect);
        self.selected_effect = Some(effect);
        if already_running {
            self.stop_selected();
        } else {
            self.start_selected();
        }
    }

    fn start_selected(&mut self) {
        let Some(DeviceOption(device)) = self.selected_device.clone() else {
            eprintln!("没有可启动的受支持 HID 设备");
            return;
        };
        let Some(EffectOption(effect)) = self.selected_effect.clone() else {
            eprintln!("没有可启动的灯效插件");
            return;
        };

        if let Err(error) = self.stop_pipeline() {
            eprintln!("关闭上一条管线失败：{error:#}");
            return;
        }

        match Pipeline::start(&device, &effect, &self.hid) {
            Ok(pipeline) => {
                eprintln!(
                    "已启动 {} + {}，灯效与设备均在独立 worker 中运行",
                    pipeline.device_name(),
                    pipeline.effect_name(),
                );
                self.pipeline = Some(pipeline);
            }
            Err(error) => eprintln!("启动失败：{error:#}"),
        }
    }

    fn stop_selected(&mut self) {
        match self.stop_pipeline() {
            Ok(()) => eprintln!("灯效已停止，设备已关闭并尝试熄灯"),
            Err(error) => eprintln!("停止失败：{error:#}"),
        }
    }

    fn poll_pipeline(&mut self) {
        let result = self.pipeline.as_ref().map(Pipeline::poll);
        if let Some(Err(error)) = result {
            let stop_error = self.stop_pipeline().err();
            match stop_error {
                Some(stop_error) => {
                    eprintln!("渲染管线已停止：{error:#}；关闭时又发生错误：{stop_error:#}")
                }
                None => eprintln!("渲染管线已停止：{error:#}"),
            }
        }
    }

    fn stop_pipeline(&mut self) -> anyhow::Result<()> {
        match self.pipeline.take() {
            Some(mut pipeline) => pipeline.stop(),
            None => Ok(()),
        }
    }

    fn rescan(&mut self, manual: bool) {
        let devices = match detect_devices(&self.catalog, &self.hid) {
            Ok(devices) => devices,
            Err(error) => {
                eprintln!("HID 扫描失败：{error:#}");
                return;
            }
        };
        let changed = self.devices != devices;
        let disconnected = self.pipeline.as_ref().is_some_and(|pipeline| {
            !devices
                .iter()
                .any(|DeviceOption(device)| pipeline.matches_device(device))
        });

        self.replace_devices(devices);

        if disconnected {
            let name = self
                .pipeline
                .as_ref()
                .map(|pipeline| pipeline.device_name().to_owned())
                .unwrap_or_default();
            let stop_error = self.stop_pipeline().err();
            match stop_error {
                Some(error) => {
                    eprintln!("设备 {name} 已拔出；停止管线时发生错误：{error:#}")
                }
                None => eprintln!("设备 {name} 已拔出，灯效管线已停止"),
            }
        } else if manual {
            eprintln!("重新扫描完成：找到 {} 个支持项", self.devices.len());
        } else if changed {
            eprintln!("设备列表已更新：找到 {} 个支持项", self.devices.len());
        }
    }

    fn replace_devices(&mut self, devices: Vec<DeviceOption>) {
        let previous = self.selected_device.take();
        self.selected_device = previous
            .and_then(|selected| devices.iter().find(|device| **device == selected).cloned())
            .or_else(|| devices.first().cloned());
        self.devices = devices;
    }
}

fn title_bar<'a>() -> Element<'a, Message> {
    let title = text("Spectra").size(13);

    let drag_area = mouse_area(
        container(title)
            .width(Length::Fill)
            .height(Length::Fixed(TITLE_BAR_HEIGHT))
            .padding([0, 14])
            .align_y(Vertical::Center),
    )
    .on_press(Message::DragWindow)
    .on_double_click(Message::ToggleMaximize)
    .on_right_press(Message::ShowWindowMenu);

    let controls = row![
        window_control_button(
            &MINIMIZE_ICON,
            "最小化",
            Message::MinimizeWindow,
            title_bar_button_style,
        ),
        window_control_button(
            &MAXIMIZE_ICON,
            "最大化或还原",
            Message::ToggleMaximize,
            title_bar_button_style,
        ),
        window_control_button(
            &CLOSE_ICON,
            "关闭",
            Message::CloseWindow,
            close_window_button_style,
        ),
    ]
    .height(Length::Fixed(TITLE_BAR_HEIGHT));

    let scan = container(
        button(text("扫描设备").size(11))
            .on_press(Message::Rescan)
            .padding([6, 10])
            .style(button::secondary),
    )
    .height(Length::Fill)
    .padding([0, 8])
    .align_y(Vertical::Center);

    container(
        row![drag_area, scan, controls]
            .width(Length::Fill)
            .height(Length::Fixed(TITLE_BAR_HEIGHT))
            .align_y(Vertical::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(TITLE_BAR_HEIGHT))
    .style(title_bar_style)
    .into()
}

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
            .style(window_control_icon_style),
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

fn window_control_icon_style(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(theme.extended_palette().background.base.text),
    }
}

fn title_bar_button_style(theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => button::background(theme, status),
        button::Status::Active | button::Status::Disabled => button::text(theme, status),
    }
}

fn close_window_button_style(theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => button::danger(theme, status),
        button::Status::Active | button::Status::Disabled => button::text(theme, status),
    }
}

fn selectable_button_style(
    theme: &Theme,
    status: button::Status,
    selected: bool,
    running: bool,
) -> button::Style {
    let palette = theme.extended_palette();
    let base = if running {
        palette.success.weak
    } else if selected {
        palette.primary.weak
    } else {
        palette.background.weakest
    };
    let (background, text_color) = match status {
        button::Status::Hovered => {
            let pair = if running {
                palette.success.base
            } else if selected {
                palette.primary.base
            } else {
                palette.background.weak
            };
            (pair.color, pair.text)
        }
        button::Status::Pressed => (
            palette.background.strong.color,
            palette.background.strong.text,
        ),
        button::Status::Disabled => (base.color.scale_alpha(0.45), base.text.scale_alpha(0.55)),
        button::Status::Active => (base.color, base.text),
    };
    let border_color = if running {
        palette.success.base.color
    } else if selected {
        palette.primary.base.color
    } else {
        palette.background.weak.color
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color,
        border: Border {
            color: border_color,
            width: if selected || running { 1.0 } else { 0.0 },
            radius: 9.0.into(),
        },
        ..button::Style::default()
    }
}

fn title_bar_style(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.weakest.color)
}

fn sidebar_style(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.weakest.color)
}

fn workspace_style(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.base.color)
}

fn device_info_style(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.base.color)
}

fn effect_list_style(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.base.color)
}

fn effect_detail_style(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.weakest.color)
}

fn detect_devices(catalog: &PluginCatalog, hid: &HidManager) -> anyhow::Result<Vec<DeviceOption>> {
    Ok(catalog
        .discover(&hid.enumerate()?, hid)?
        .into_iter()
        .map(DeviceOption)
        .collect())
}

#[derive(Clone, Debug)]
struct DeviceOption(RegisteredDevice);

impl PartialEq for DeviceOption {
    fn eq(&self, other: &Self) -> bool {
        self.0.key() == other.0.key()
    }
}

impl Eq for DeviceOption {}

#[derive(Clone, Debug)]
struct EffectOption(PluginMetadata);

impl PartialEq for EffectOption {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for EffectOption {}
