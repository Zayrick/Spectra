use iced::widget::{canvas, mouse_area};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};

use crate::types::{ColorFrame, DeviceMatrix};

use super::Message;

pub(super) fn view(
    matrix: &DeviceMatrix,
    frame: Option<ColorFrame>,
    enabled: bool,
) -> Element<'_, Message> {
    mouse_area(
        canvas(LightingPreview {
            matrix,
            frame,
            enabled,
        })
        .width(Length::FillPortion(4))
        .height(Length::Fill),
    )
    .on_press(Message::TogglePreview)
    .into()
}

struct LightingPreview<'a> {
    matrix: &'a DeviceMatrix,
    frame: Option<ColorFrame>,
    enabled: bool,
}

impl canvas::Program<Message> for LightingPreview<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        if !self.enabled {
            return Vec::new();
        }

        let palette = theme.extended_palette();
        let mut drawing = canvas::Frame::new(renderer, bounds.size());
        let cell_size = (bounds.width / f32::from(self.matrix.width))
            .min(bounds.height / f32::from(self.matrix.height));
        if cell_size <= 0.0 {
            return vec![drawing.into_geometry()];
        }

        let grid_width = cell_size * f32::from(self.matrix.width);
        let grid_height = cell_size * f32::from(self.matrix.height);
        let origin = Point::new(
            (bounds.width - grid_width) / 2.0,
            (bounds.height - grid_height) / 2.0,
        );
        let gap = (cell_size * 0.18).clamp(1.0, 3.5);
        let led_size = Size::new((cell_size - gap).max(1.0), (cell_size - gap).max(1.0));
        let radius = (led_size.width * 0.24).clamp(1.0, 4.0);
        let inactive = palette.background.strong.color;
        let outline = palette.background.base.text.scale_alpha(0.18);
        let mut colors = self.frame.as_deref().unwrap_or_default().chunks_exact(3);

        for led in &self.matrix.leds {
            let color = colors
                .next()
                .map(|rgb| Color::from_rgb8(rgb[0], rgb[1], rgb[2]))
                .unwrap_or(inactive);
            let position = Point::new(
                origin.x + f32::from(led.x) * cell_size + gap / 2.0,
                origin.y + f32::from(led.y) * cell_size + gap / 2.0,
            );
            let light = canvas::Path::rounded_rectangle(position, led_size, radius.into());
            drawing.fill(&light, color);
            drawing.stroke(
                &light,
                canvas::Stroke::default()
                    .with_color(outline)
                    .with_width(0.75),
            );
        }

        vec![drawing.into_geometry()]
    }
}
