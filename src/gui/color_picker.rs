use std::f32::consts::{PI, TAU};

use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, overlay, renderer, widget};
use iced::widget::{button, canvas, container, row, space, text, text_input};
use iced::{
    Background, Border, Color, Element, Event as IcedEvent, Font, Length, Padding, Point,
    Rectangle, Renderer, Shadow, Size, Theme, Vector, keyboard, mouse, touch,
};

use crate::types::RgbColor;

const CONTROL_HEIGHT: f32 = 34.0;
const INPUT_TEXT_SIZE: f32 = 13.0;
const INPUT_VERTICAL_PADDING: f32 = 8.0;
const INPUT_HORIZONTAL_PADDING: f32 = 11.0;
const INPUT_LINE_HEIGHT: f32 = CONTROL_HEIGHT - INPUT_VERTICAL_PADDING * 2.0;
const PICKER_SIZE: f32 = 200.0;
const HUE_SEGMENTS: usize = 180;
const TRIANGLE_STRIPS: usize = 80;
const POPOVER_GAP: f32 = 8.0;
const VIEWPORT_MARGIN: f32 = 8.0;

#[derive(Clone, Debug)]
pub(super) enum Event {
    Toggle,
    Dismiss,
    HexChanged(String),
    SaturationValueChanged { saturation: f32, value: f32 },
    HueChanged(f32),
}

pub(super) fn view<'a>(color: RgbColor, hex: &str, hue: f32, open: bool) -> Element<'a, Event> {
    let valid_hex = parse_hex(hex).is_some();
    let swatch = button(space().width(Length::Fill).height(Length::Fill))
        .on_press(Event::Toggle)
        .width(Length::Fixed(CONTROL_HEIGHT))
        .height(Length::Fixed(CONTROL_HEIGHT))
        .padding(0)
        .style(move |theme, status| swatch_style(theme, status, color, open));
    let input = text_input("#RRGGBB", hex)
        .on_input(Event::HexChanged)
        .font(Font::MONOSPACE)
        .size(INPUT_TEXT_SIZE)
        .line_height(text::LineHeight::Absolute(INPUT_LINE_HEIGHT.into()))
        .padding([INPUT_VERTICAL_PADDING, INPUT_HORIZONTAL_PADDING])
        .style(move |theme, status| hex_input_style(theme, status, valid_hex));
    let control = row![swatch, input]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .width(Length::Fill);

    let (_, saturation, value) = rgb_to_hsv(color);
    let popup = container(
        canvas(ColorWheelPicker {
            hue,
            saturation,
            value,
        })
        .width(Length::Fixed(PICKER_SIZE))
        .height(Length::Fixed(PICKER_SIZE)),
    )
    .padding(12)
    .style(popover_style);

    Element::new(Popover {
        content: control.into(),
        popup: popup.into(),
        open,
    })
}

pub(super) fn normalize_hex_input(input: &str) -> Option<String> {
    let digits = input.strip_prefix('#').unwrap_or(input);
    if digits.len() > 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }

    Some(format!("#{}", digits.to_ascii_uppercase()))
}

pub(super) fn parse_hex(input: &str) -> Option<RgbColor> {
    let digits = input.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }

    Some(RgbColor {
        red: u8::from_str_radix(&digits[0..2], 16).ok()?,
        green: u8::from_str_radix(&digits[2..4], 16).ok()?,
        blue: u8::from_str_radix(&digits[4..6], 16).ok()?,
    })
}

pub(super) fn format_hex(color: RgbColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue)
}

pub(super) fn rgb_to_hsv(color: RgbColor) -> (f32, f32, f32) {
    let red = f32::from(color.red) / 255.0;
    let green = f32::from(color.green) / 255.0;
    let blue = f32::from(color.blue) / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;

    let hue = if delta == 0.0 {
        0.0
    } else if max == red {
        60.0 * ((green - blue) / delta).rem_euclid(6.0)
    } else if max == green {
        60.0 * ((blue - red) / delta + 2.0)
    } else {
        60.0 * ((red - green) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };

    (hue, saturation, max)
}

pub(super) fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> RgbColor {
    let hue = hue.rem_euclid(360.0);
    let saturation = saturation.clamp(0.0, 1.0);
    let value = value.clamp(0.0, 1.0);
    let chroma = value * saturation;
    let hue_sector = hue / 60.0;
    let secondary = chroma * (1.0 - (hue_sector.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match hue_sector as u8 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let match_value = value - chroma;

    RgbColor {
        red: ((red + match_value) * 255.0).round() as u8,
        green: ((green + match_value) * 255.0).round() as u8,
        blue: ((blue + match_value) * 255.0).round() as u8,
    }
}

fn swatch_style(
    theme: &Theme,
    status: button::Status,
    color: RgbColor,
    open: bool,
) -> button::Style {
    let palette = theme.extended_palette();
    let highlighted = open || matches!(status, button::Status::Hovered | button::Status::Pressed);

    button::Style {
        background: Some(Background::Color(Color::from_rgb8(
            color.red,
            color.green,
            color.blue,
        ))),
        border: Border {
            color: if highlighted {
                palette.primary.strong.color
            } else {
                palette.background.base.text.scale_alpha(0.35)
            },
            width: if open { 2.0 } else { 1.0 },
            radius: 7.0.into(),
        },
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(if highlighted { 0.28 } else { 0.18 }),
            offset: Vector::new(
                0.0,
                if status == button::Status::Pressed {
                    0.0
                } else {
                    1.0
                },
            ),
            blur_radius: if highlighted { 6.0 } else { 3.0 },
        },
        ..button::Style::default()
    }
}

fn hex_input_style(theme: &Theme, status: text_input::Status, valid: bool) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    style.border.radius = 7.0.into();
    if !valid {
        style.border.color = theme.extended_palette().danger.base.color;
    }
    style
}

fn popover_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(palette.background.weakest.color)),
        border: Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.38),
            offset: Vector::new(0.0, 8.0),
            blur_radius: 24.0,
        },
        ..container::Style::default()
    }
}

#[derive(Default)]
struct WheelState {
    dragging: Option<PickerRegion>,
}

#[derive(Clone, Copy)]
enum PickerRegion {
    HueRing,
    Triangle,
}

struct ColorWheelPicker {
    hue: f32,
    saturation: f32,
    value: f32,
}

impl canvas::Program<Event> for ColorWheelPicker {
    type State = WheelState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Event>> {
        match event {
            IcedEvent::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let position = cursor.position_in(bounds)?;
                let region = self.region_at(bounds.size(), position)?;
                state.dragging = Some(region);
                Some(self.action(region, bounds.size(), position))
            }
            IcedEvent::Mouse(mouse::Event::CursorMoved { position }) => {
                let region = state.dragging?;
                Some(self.action(
                    region,
                    bounds.size(),
                    *position - Vector::new(bounds.x, bounds.y),
                ))
            }
            IcedEvent::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging.take().map(|_| canvas::Action::capture())
            }
            IcedEvent::Touch(touch::Event::FingerPressed { position, .. })
                if bounds.contains(*position) =>
            {
                let position = *position - Vector::new(bounds.x, bounds.y);
                let region = self.region_at(bounds.size(), position)?;
                state.dragging = Some(region);
                Some(self.action(region, bounds.size(), position))
            }
            IcedEvent::Touch(touch::Event::FingerMoved { position, .. }) => {
                let region = state.dragging?;
                Some(self.action(
                    region,
                    bounds.size(),
                    *position - Vector::new(bounds.x, bounds.y),
                ))
            }
            IcedEvent::Touch(
                touch::Event::FingerLifted { .. } | touch::Event::FingerLost { .. },
            ) => state.dragging.take().map(|_| canvas::Action::capture()),
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let wheel = WheelGeometry::new(bounds.size(), self.hue);
        let outline = theme
            .extended_palette()
            .background
            .base
            .text
            .scale_alpha(0.32);

        draw_hue_ring(&mut frame, wheel, outline);
        draw_color_triangle(&mut frame, wheel.triangle, self.hue, outline);

        let hue_marker = point_on_circle(wheel.center, wheel.ring_mid_radius(), self.hue);
        let hue_color = hsv_to_rgb(self.hue, 1.0, 1.0);
        draw_marker(
            &mut frame,
            hue_marker,
            wheel.marker_radius,
            Color::from_rgb8(hue_color.red, hue_color.green, hue_color.blue),
        );

        let color_marker = wheel.triangle.point_for_sv(self.saturation, self.value);
        let selected = hsv_to_rgb(self.hue, self.saturation, self.value);
        draw_marker(
            &mut frame,
            color_marker,
            wheel.marker_radius,
            Color::from_rgb8(selected.red, selected.green, selected.blue),
        );

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let hovered = cursor
            .position_in(bounds)
            .is_some_and(|position| self.region_at(bounds.size(), position).is_some());
        if state.dragging.is_some() || hovered {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::None
        }
    }
}

impl ColorWheelPicker {
    fn region_at(&self, size: Size, position: Point) -> Option<PickerRegion> {
        let wheel = WheelGeometry::new(size, self.hue);
        if wheel.triangle.contains(position) {
            return Some(PickerRegion::Triangle);
        }

        let distance = distance(position, wheel.center);
        let tolerance = wheel.scale * 2.0;
        (distance >= wheel.inner_radius - tolerance && distance <= wheel.outer_radius + tolerance)
            .then_some(PickerRegion::HueRing)
    }

    fn action(&self, region: PickerRegion, size: Size, position: Point) -> canvas::Action<Event> {
        let wheel = WheelGeometry::new(size, self.hue);
        let event = match region {
            PickerRegion::HueRing => {
                let hue = if distance(wheel.center, position) <= wheel.scale {
                    self.hue
                } else {
                    hue_at_point(wheel.center, position)
                };
                Event::HueChanged(hue)
            }
            PickerRegion::Triangle => {
                let (saturation, value) = wheel.triangle.sv_at_point(position);
                Event::SaturationValueChanged { saturation, value }
            }
        };

        canvas::Action::publish(event).and_capture()
    }
}

#[derive(Clone, Copy)]
struct WheelGeometry {
    center: Point,
    outer_radius: f32,
    inner_radius: f32,
    marker_radius: f32,
    scale: f32,
    triangle: ColorTriangle,
}

impl WheelGeometry {
    fn new(size: Size, hue: f32) -> Self {
        let diameter = size.width.min(size.height);
        let scale = diameter / PICKER_SIZE;
        let center = Point::new(size.width / 2.0, size.height / 2.0);
        let outer_radius = diameter / 2.0 - 2.0 * scale;
        let ring_width = 18.0 * scale;
        let inner_radius = outer_radius - ring_width;

        Self {
            center,
            outer_radius,
            inner_radius,
            marker_radius: 6.0 * scale,
            scale,
            triangle: ColorTriangle::new(center, inner_radius, hue),
        }
    }

    fn ring_mid_radius(self) -> f32 {
        (self.outer_radius + self.inner_radius) / 2.0
    }
}

#[derive(Clone, Copy)]
struct ColorTriangle {
    hue: Point,
    white: Point,
    black: Point,
}

impl ColorTriangle {
    fn new(center: Point, radius: f32, hue: f32) -> Self {
        Self {
            hue: point_on_circle(center, radius, hue),
            white: point_on_circle(center, radius, hue + 120.0),
            black: point_on_circle(center, radius, hue + 240.0),
        }
    }

    fn path(self) -> canvas::Path {
        triangle_path(self.hue, self.white, self.black)
    }

    fn contains(self, point: Point) -> bool {
        let [hue, white, black] = self.barycentric(point);
        hue >= -0.001 && white >= -0.001 && black >= -0.001
    }

    fn point_for_sv(self, saturation: f32, value: f32) -> Point {
        let saturation = saturation.clamp(0.0, 1.0);
        let value = value.clamp(0.0, 1.0);
        weighted_point(
            self.hue,
            value * saturation,
            self.white,
            value * (1.0 - saturation),
            self.black,
            1.0 - value,
        )
    }

    fn sv_at_point(self, point: Point) -> (f32, f32) {
        let point = if self.contains(point) {
            point
        } else {
            self.closest_point(point)
        };
        let [hue, white, _] = self.barycentric(point).map(|weight| weight.clamp(0.0, 1.0));
        let value = (hue + white).clamp(0.0, 1.0);
        let saturation = if value <= f32::EPSILON {
            0.0
        } else {
            (hue / value).clamp(0.0, 1.0)
        };

        (saturation, value)
    }

    fn barycentric(self, point: Point) -> [f32; 3] {
        let denominator = (self.white.y - self.black.y) * (self.hue.x - self.black.x)
            + (self.black.x - self.white.x) * (self.hue.y - self.black.y);
        let hue = ((self.white.y - self.black.y) * (point.x - self.black.x)
            + (self.black.x - self.white.x) * (point.y - self.black.y))
            / denominator;
        let white = ((self.black.y - self.hue.y) * (point.x - self.black.x)
            + (self.hue.x - self.black.x) * (point.y - self.black.y))
            / denominator;

        [hue, white, 1.0 - hue - white]
    }

    fn closest_point(self, point: Point) -> Point {
        [
            closest_point_on_segment(point, self.hue, self.white),
            closest_point_on_segment(point, self.white, self.black),
            closest_point_on_segment(point, self.black, self.hue),
        ]
        .into_iter()
        .min_by(|left, right| {
            squared_distance(*left, point).total_cmp(&squared_distance(*right, point))
        })
        .unwrap_or(point)
    }
}

fn draw_hue_ring(frame: &mut canvas::Frame<Renderer>, wheel: WheelGeometry, outline: Color) {
    for segment in 0..HUE_SEGMENTS {
        let start_hue = segment as f32 / HUE_SEGMENTS as f32 * 360.0;
        let end_hue = (segment + 1) as f32 / HUE_SEGMENTS as f32 * 360.0;
        let start_angle = hue_angle(start_hue) - 0.001;
        let end_angle = hue_angle(end_hue) + 0.001;
        let color = hsv_to_rgb((start_hue + end_hue) / 2.0, 1.0, 1.0);
        frame.fill(
            &annular_segment(
                wheel.center,
                wheel.inner_radius,
                wheel.outer_radius,
                start_angle,
                end_angle,
            ),
            Color::from_rgb8(color.red, color.green, color.blue),
        );
    }

    for radius in [wheel.inner_radius, wheel.outer_radius] {
        frame.stroke(
            &canvas::Path::circle(wheel.center, radius),
            canvas::Stroke::default()
                .with_color(outline)
                .with_width(wheel.scale),
        );
    }
}

fn draw_color_triangle(
    frame: &mut canvas::Frame<Renderer>,
    triangle: ColorTriangle,
    hue: f32,
    outline: Color,
) {
    frame.fill(&triangle.path(), Color::BLACK);

    for strip in 0..TRIANGLE_STRIPS {
        let step = 1.0 / TRIANGLE_STRIPS as f32;
        let start = (strip as f32 * step - step * 0.08).max(0.0);
        let end = ((strip + 1) as f32 * step + step * 0.08).min(1.0);
        let middle = (strip as f32 + 0.5) * step;
        let hue_start = lerp_point(triangle.black, triangle.hue, start);
        let white_start = lerp_point(triangle.black, triangle.white, start);
        let hue_end = lerp_point(triangle.black, triangle.hue, end);
        let white_end = lerp_point(triangle.black, triangle.white, end);
        let hue_middle = lerp_point(triangle.black, triangle.hue, middle);
        let white_middle = lerp_point(triangle.black, triangle.white, middle);
        let hue_color = hsv_to_rgb(hue, 1.0, middle);
        let white_color = hsv_to_rgb(hue, 0.0, middle);

        frame.fill(
            &quad_path(hue_start, hue_end, white_end, white_start),
            canvas::gradient::Linear::new(hue_middle, white_middle)
                .add_stop(
                    0.0,
                    Color::from_rgb8(hue_color.red, hue_color.green, hue_color.blue),
                )
                .add_stop(
                    1.0,
                    Color::from_rgb8(white_color.red, white_color.green, white_color.blue),
                ),
        );
    }

    frame.stroke(
        &triangle.path(),
        canvas::Stroke::default()
            .with_color(outline)
            .with_width(1.0),
    );
}

fn draw_marker(frame: &mut canvas::Frame<Renderer>, center: Point, radius: f32, color: Color) {
    let marker = canvas::Path::circle(center, radius);
    frame.fill(&marker, color);
    frame.stroke(
        &marker,
        canvas::Stroke::default()
            .with_color(Color::BLACK.scale_alpha(0.7))
            .with_width(radius * 0.58),
    );
    frame.stroke(
        &marker,
        canvas::Stroke::default()
            .with_color(Color::WHITE)
            .with_width(radius * 0.32),
    );
}

fn hue_angle(hue: f32) -> f32 {
    hue.to_radians() - PI / 2.0
}

fn hue_at_point(center: Point, point: Point) -> f32 {
    ((point.y - center.y).atan2(point.x - center.x) + PI / 2.0)
        .rem_euclid(TAU)
        .to_degrees()
}

fn point_on_circle(center: Point, radius: f32, hue: f32) -> Point {
    let angle = hue_angle(hue);
    Point::new(
        center.x + radius * angle.cos(),
        center.y + radius * angle.sin(),
    )
}

fn annular_segment(
    center: Point,
    inner_radius: f32,
    outer_radius: f32,
    start_angle: f32,
    end_angle: f32,
) -> canvas::Path {
    let outer_start = Point::new(
        center.x + outer_radius * start_angle.cos(),
        center.y + outer_radius * start_angle.sin(),
    );
    let outer_end = Point::new(
        center.x + outer_radius * end_angle.cos(),
        center.y + outer_radius * end_angle.sin(),
    );
    let inner_end = Point::new(
        center.x + inner_radius * end_angle.cos(),
        center.y + inner_radius * end_angle.sin(),
    );
    let inner_start = Point::new(
        center.x + inner_radius * start_angle.cos(),
        center.y + inner_radius * start_angle.sin(),
    );

    quad_path(outer_start, outer_end, inner_end, inner_start)
}

fn triangle_path(first: Point, second: Point, third: Point) -> canvas::Path {
    canvas::Path::new(|path| {
        path.move_to(first);
        path.line_to(second);
        path.line_to(third);
        path.close();
    })
}

fn quad_path(first: Point, second: Point, third: Point, fourth: Point) -> canvas::Path {
    canvas::Path::new(|path| {
        path.move_to(first);
        path.line_to(second);
        path.line_to(third);
        path.line_to(fourth);
        path.close();
    })
}

fn weighted_point(
    first: Point,
    first_weight: f32,
    second: Point,
    second_weight: f32,
    third: Point,
    third_weight: f32,
) -> Point {
    Point::new(
        first.x * first_weight + second.x * second_weight + third.x * third_weight,
        first.y * first_weight + second.y * second_weight + third.y * third_weight,
    )
}

fn lerp_point(from: Point, to: Point, amount: f32) -> Point {
    Point::new(
        from.x + (to.x - from.x) * amount,
        from.y + (to.y - from.y) * amount,
    )
}

fn closest_point_on_segment(point: Point, start: Point, end: Point) -> Point {
    let segment = end - start;
    let length_squared = segment.x * segment.x + segment.y * segment.y;
    if length_squared <= f32::EPSILON {
        return start;
    }
    let offset = point - start;
    let amount = ((offset.x * segment.x + offset.y * segment.y) / length_squared).clamp(0.0, 1.0);
    lerp_point(start, end, amount)
}

fn squared_distance(first: Point, second: Point) -> f32 {
    let x = first.x - second.x;
    let y = first.y - second.y;
    x * x + y * y
}

fn distance(first: Point, second: Point) -> f32 {
    squared_distance(first, second).sqrt()
}

struct Popover<'a> {
    content: Element<'a, Event>,
    popup: Element<'a, Event>,
    open: bool,
}

impl Widget<Event, Theme, Renderer> for Popover<'_> {
    fn children(&self) -> Vec<widget::Tree> {
        vec![
            widget::Tree::new(&self.content),
            widget::Tree::new(&self.popup),
        ]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[self.content.as_widget(), self.popup.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &IcedEvent,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Event>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        inherited_style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            inherited_style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Event, Theme, Renderer>> {
        let (content_tree, popup_tree) = tree.children.split_at_mut(1);
        let content_overlay = self.content.as_widget_mut().overlay(
            &mut content_tree[0],
            layout,
            renderer,
            viewport,
            translation,
        );
        let popup_overlay = self.open.then(|| {
            overlay::Element::new(Box::new(PickerOverlay {
                anchor: layout.bounds() + translation,
                popup: &mut self.popup,
                tree: &mut popup_tree[0],
            }))
        });

        if content_overlay.is_some() || popup_overlay.is_some() {
            Some(
                overlay::Group::with_children(
                    content_overlay.into_iter().chain(popup_overlay).collect(),
                )
                .overlay(),
            )
        } else {
            None
        }
    }
}

struct PickerOverlay<'a, 'b> {
    anchor: Rectangle,
    popup: &'b mut Element<'a, Event>,
    tree: &'b mut widget::Tree,
}

impl overlay::Overlay<Event, Theme, Renderer> for PickerOverlay<'_, '_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let popup = self.popup.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, bounds).shrink(Padding::new(VIEWPORT_MARGIN)),
        );
        let size = popup.size();
        let max_x = (bounds.width - size.width - VIEWPORT_MARGIN).max(VIEWPORT_MARGIN);
        let x = self.anchor.x.clamp(VIEWPORT_MARGIN, max_x);
        let below = self.anchor.y + self.anchor.height + POPOVER_GAP;
        let above = self.anchor.y - size.height - POPOVER_GAP;
        let max_y = (bounds.height - size.height - VIEWPORT_MARGIN).max(VIEWPORT_MARGIN);
        let y = if below + size.height <= bounds.height - VIEWPORT_MARGIN {
            below
        } else {
            above
        }
        .clamp(VIEWPORT_MARGIN, max_y);

        popup.translate(Vector::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        inherited_style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.popup.as_widget().draw(
            self.tree,
            renderer,
            theme,
            inherited_style,
            layout,
            cursor,
            &Rectangle::INFINITE,
        );
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.popup
            .as_widget_mut()
            .operate(self.tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        event: &IcedEvent,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Event>,
    ) {
        let pressed_position = match event {
            IcedEvent::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => cursor.position(),
            IcedEvent::Touch(touch::Event::FingerPressed { position, .. }) => Some(*position),
            _ => None,
        };
        let escape_pressed = matches!(
            event,
            IcedEvent::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            })
        );

        if escape_pressed || pressed_position.is_some_and(|point| !layout.bounds().contains(point))
        {
            shell.publish(Event::Dismiss);
            shell.capture_event();
            return;
        }

        self.popup.as_widget_mut().update(
            self.tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &Rectangle::INFINITE,
        );

        if pressed_position.is_some() && !shell.is_event_captured() {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.popup.as_widget().mouse_interaction(
            self.tree,
            layout,
            cursor,
            &Rectangle::INFINITE,
            renderer,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_six_digit_hex_colors() {
        let color = RgbColor {
            red: 0x1A,
            green: 0xB2,
            blue: 0x03,
        };

        assert_eq!(format_hex(color), "#1AB203");
        assert_eq!(parse_hex("#1ab203"), Some(color));
        assert_eq!(parse_hex("1AB203"), None);
        assert_eq!(parse_hex("#12345"), None);
    }

    #[test]
    fn normalizes_editable_hex_input() {
        assert_eq!(normalize_hex_input(""), Some("#".into()));
        assert_eq!(normalize_hex_input("a0bf"), Some("#A0BF".into()));
        assert_eq!(normalize_hex_input("#00ff11"), Some("#00FF11".into()));
        assert_eq!(normalize_hex_input("#1234567"), None);
        assert_eq!(normalize_hex_input("#12xz"), None);
    }

    #[test]
    fn converts_primary_colors_between_rgb_and_hsv() {
        for (color, expected_hue) in [
            (
                RgbColor {
                    red: 255,
                    green: 0,
                    blue: 0,
                },
                0.0,
            ),
            (
                RgbColor {
                    red: 0,
                    green: 255,
                    blue: 0,
                },
                120.0,
            ),
            (
                RgbColor {
                    red: 0,
                    green: 0,
                    blue: 255,
                },
                240.0,
            ),
        ] {
            let (hue, saturation, value) = rgb_to_hsv(color);
            assert!((hue - expected_hue).abs() < f32::EPSILON);
            assert!((saturation - 1.0).abs() < f32::EPSILON);
            assert!((value - 1.0).abs() < f32::EPSILON);
            assert_eq!(hsv_to_rgb(hue, saturation, value), color);
        }
    }

    #[test]
    fn preserves_rgb_values_through_hsv_round_trip() {
        for color in [
            RgbColor {
                red: 23,
                green: 117,
                blue: 204,
            },
            RgbColor {
                red: 19,
                green: 19,
                blue: 19,
            },
            RgbColor {
                red: 255,
                green: 128,
                blue: 37,
            },
        ] {
            let (hue, saturation, value) = rgb_to_hsv(color);
            assert_eq!(hsv_to_rgb(hue, saturation, value), color);
        }
    }

    #[test]
    fn hue_ring_coordinates_round_trip() {
        let center = Point::new(100.0, 100.0);
        for hue in [0.0, 37.0, 90.0, 180.0, 245.0, 359.0] {
            let point = point_on_circle(center, 80.0, hue);
            assert!((hue_at_point(center, point) - hue).abs() < 0.001);
        }
    }

    #[test]
    fn color_triangle_is_inscribed_in_the_hue_ring() {
        let wheel = WheelGeometry::new(Size::new(PICKER_SIZE, PICKER_SIZE), 37.0);
        for vertex in [
            wheel.triangle.hue,
            wheel.triangle.white,
            wheel.triangle.black,
        ] {
            assert!((distance(vertex, wheel.center) - wheel.inner_radius).abs() < 0.001);
        }
    }

    #[test]
    fn triangle_coordinates_round_trip_saturation_and_value() {
        for hue in [0.0, 37.0, 245.0] {
            let triangle = ColorTriangle::new(Point::new(100.0, 100.0), 80.0, hue);
            for (saturation, value) in
                [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.35, 0.72), (0.9, 0.2)]
            {
                let point = triangle.point_for_sv(saturation, value);
                let (actual_saturation, actual_value) = triangle.sv_at_point(point);
                let expected_saturation = if value == 0.0 { 0.0 } else { saturation };

                assert!((actual_saturation - expected_saturation).abs() < 0.001);
                assert!((actual_value - value).abs() < 0.001);
            }
        }
    }
}
