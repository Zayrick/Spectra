#[cfg(not(target_os = "macos"))]
use iced::widget::svg;
use iced::widget::{button, container};
use iced::{Background, Border, Theme};

#[cfg(not(target_os = "macos"))]
pub(super) fn window_control_icon(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(theme.extended_palette().background.base.text),
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn title_bar_button(theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => button::background(theme, status),
        button::Status::Active | button::Status::Disabled => button::text(theme, status),
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn close_window_button(theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => button::danger(theme, status),
        button::Status::Active | button::Status::Disabled => button::text(theme, status),
    }
}

pub(super) fn scan_button(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::secondary(theme, status);
    style.border.radius = 8.0.into();
    style
}

pub(super) fn selectable_button(
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

pub(super) fn title_bar(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.weakest.color)
}

pub(super) fn sidebar(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.weakest.color)
}

pub(super) fn workspace(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.base.color)
}

pub(super) fn device_info(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.base.color)
}

pub(super) fn effect_list(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.base.color)
}

pub(super) fn effect_detail(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.weakest.color)
}
