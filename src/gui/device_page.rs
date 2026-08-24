use std::collections::HashMap;

use crate::types::{DeviceCapabilities, ModeSettings, ModeValue, RgbColor};

use super::color_picker;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlPage {
    Live,
    Standalone,
}

pub(super) struct ActiveColorPicker {
    pub control_id: String,
    pub hue: f32,
}

pub(super) struct DevicePageState {
    pub control_page: ControlPage,
    pub selected_effect_id: Option<String>,
    pub selected_mode_id: Option<String>,
    pub mode_settings: Option<ModeSettings>,
    pub color_hex_drafts: HashMap<String, String>,
    pub active_color_picker: Option<ActiveColorPicker>,
}

impl DevicePageState {
    pub fn new(capabilities: &DeviceCapabilities, default_effect_id: Option<&str>) -> Self {
        let mode = capabilities.modes.first();
        Self {
            control_page: preferred_control_page(capabilities),
            selected_effect_id: default_effect_id.map(str::to_owned),
            selected_mode_id: mode.map(|mode| mode.id.clone()),
            mode_settings: mode.map(|mode| mode.default_settings()),
            color_hex_drafts: HashMap::new(),
            active_color_picker: None,
        }
    }

    pub fn reconcile(
        &mut self,
        capabilities: &DeviceCapabilities,
        default_effect_id: Option<&str>,
    ) {
        if !control_page_available(capabilities, self.control_page) {
            self.control_page = preferred_control_page(capabilities);
        }
        if self.selected_effect_id.is_none() {
            self.selected_effect_id = default_effect_id.map(str::to_owned);
        }

        let selected_mode = self
            .selected_mode_id
            .as_deref()
            .and_then(|id| capabilities.mode(id));
        match (selected_mode, self.mode_settings.as_ref()) {
            (Some(mode), Some(settings)) if mode.validate_settings(settings).is_ok() => {}
            (Some(mode), _) => {
                self.mode_settings = Some(mode.default_settings());
                self.reset_color_editor();
            }
            (None, _) => {
                let mode = capabilities.modes.first();
                self.selected_mode_id = mode.map(|mode| mode.id.clone());
                self.mode_settings = mode.map(|mode| mode.default_settings());
                self.reset_color_editor();
            }
        }
    }

    pub fn reset_color_editor(&mut self) {
        self.active_color_picker = None;
        self.color_hex_drafts.clear();
    }

    pub fn mode_color(&self, control_id: &str) -> Option<RgbColor> {
        match self.mode_settings.as_ref()?.get(control_id)? {
            ModeValue::Color(color) => Some(*color),
            ModeValue::Slider(_) => None,
        }
    }

    pub fn set_mode_color(&mut self, control_id: &str, color: RgbColor) {
        if let Some(ModeValue::Color(current)) = self
            .mode_settings
            .as_mut()
            .and_then(|settings| settings.get_mut(control_id))
        {
            *current = color;
            self.color_hex_drafts.remove(control_id);
        }
    }

    pub fn update_color(&mut self, control_id: &str, event: color_picker::Event) {
        match event {
            color_picker::Event::Toggle => {
                if self
                    .active_color_picker
                    .as_ref()
                    .is_some_and(|picker| picker.control_id == control_id)
                {
                    self.active_color_picker = None;
                } else if let Some(color) = self.mode_color(control_id) {
                    let (hue, _, _) = color_picker::rgb_to_hsv(color);
                    self.active_color_picker = Some(ActiveColorPicker {
                        control_id: control_id.to_owned(),
                        hue,
                    });
                }
            }
            color_picker::Event::Dismiss => {
                if self
                    .active_color_picker
                    .as_ref()
                    .is_some_and(|picker| picker.control_id == control_id)
                {
                    self.active_color_picker = None;
                }
            }
            color_picker::Event::HexChanged(input) => {
                let Some(input) = color_picker::normalize_hex_input(&input) else {
                    return;
                };
                if let Some(color) = color_picker::parse_hex(&input) {
                    if let Some(picker) = self
                        .active_color_picker
                        .as_mut()
                        .filter(|picker| picker.control_id == control_id)
                    {
                        let (hue, saturation, _) = color_picker::rgb_to_hsv(color);
                        if saturation > 0.0 {
                            picker.hue = hue;
                        }
                    }
                    self.set_mode_color(control_id, color);
                } else {
                    self.color_hex_drafts.insert(control_id.to_owned(), input);
                }
            }
            color_picker::Event::SaturationValueChanged { saturation, value } => {
                let Some(current) = self.mode_color(control_id) else {
                    return;
                };
                let hue = self
                    .active_color_picker
                    .as_ref()
                    .filter(|picker| picker.control_id == control_id)
                    .map_or_else(|| color_picker::rgb_to_hsv(current).0, |picker| picker.hue);
                self.set_mode_color(control_id, color_picker::hsv_to_rgb(hue, saturation, value));
            }
            color_picker::Event::HueChanged(hue) => {
                let Some(current) = self.mode_color(control_id) else {
                    return;
                };
                if let Some(picker) = self
                    .active_color_picker
                    .as_mut()
                    .filter(|picker| picker.control_id == control_id)
                {
                    picker.hue = hue;
                }
                let (_, saturation, value) = color_picker::rgb_to_hsv(current);
                self.set_mode_color(control_id, color_picker::hsv_to_rgb(hue, saturation, value));
            }
        }
    }
}

pub(super) fn control_page_available(capabilities: &DeviceCapabilities, page: ControlPage) -> bool {
    match page {
        ControlPage::Live => capabilities.live,
        ControlPage::Standalone => !capabilities.modes.is_empty(),
    }
}

fn preferred_control_page(capabilities: &DeviceCapabilities) -> ControlPage {
    if capabilities.live {
        ControlPage::Live
    } else {
        ControlPage::Standalone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DeviceMode;

    fn capabilities(live: bool, modes: &[&str]) -> DeviceCapabilities {
        DeviceCapabilities {
            live,
            modes: modes
                .iter()
                .map(|id| DeviceMode {
                    id: (*id).to_owned(),
                    name: (*id).to_owned(),
                    description: None,
                    data: Vec::new(),
                    controls: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn device_pages_keep_independent_selections() {
        let capabilities = capabilities(true, &["solid", "cycle"]);
        let mut keyboard = DevicePageState::new(&capabilities, Some("rainbow"));
        let mouse = DevicePageState::new(&capabilities, Some("plasma"));

        keyboard.control_page = ControlPage::Standalone;
        keyboard.selected_effect_id = Some("wave".into());
        keyboard.selected_mode_id = Some("cycle".into());

        assert_eq!(mouse.control_page, ControlPage::Live);
        assert_eq!(mouse.selected_effect_id.as_deref(), Some("plasma"));
        assert_eq!(mouse.selected_mode_id.as_deref(), Some("solid"));
    }

    #[test]
    fn reconcile_only_resets_unavailable_device_state() {
        let mut page =
            DevicePageState::new(&capabilities(true, &["solid", "cycle"]), Some("rainbow"));
        page.control_page = ControlPage::Standalone;
        page.selected_mode_id = Some("cycle".into());

        page.reconcile(&capabilities(true, &["cycle"]), Some("rainbow"));
        assert_eq!(page.control_page, ControlPage::Standalone);
        assert_eq!(page.selected_mode_id.as_deref(), Some("cycle"));

        page.reconcile(&capabilities(true, &[]), Some("rainbow"));
        assert_eq!(page.control_page, ControlPage::Live);
        assert_eq!(page.selected_mode_id, None);
        assert!(page.mode_settings.is_none());
    }
}
