use egui_plugin as egui;
use nice_plug::prelude::{FloatParam, Param, ParamSetter};

/// Two independent host parameters in one control. Button ownership is locked
/// until release, including when the pointer leaves the widget.
pub(super) fn dual_knob(
    ui: &mut egui::Ui,
    amount: &FloatParam,
    frequency: &FloatParam,
    setter: &ParamSetter<'_>,
) -> egui::Response {
    knob(ui, &[amount, frequency], setter)
}

/// Single-parameter counterpart with the same ring rendering and gestures.
pub(super) fn single_knob(
    ui: &mut egui::Ui,
    param: &FloatParam,
    setter: &ParamSetter<'_>,
) -> egui::Response {
    ui.vertical_centered(|ui| {
        let response = knob(ui, &[param], setter);
        ui.label(param.to_string());
        response.on_hover_text(param.name())
    })
    .inner
}

fn knob(ui: &mut egui::Ui, params: &[&FloatParam], setter: &ParamSetter<'_>) -> egui::Response {
    let dual = params.len() == 2;
    let size = if dual { 56.0 } else { 48.0 };
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
    let id = response.id.with("dual-gesture");
    let mut active = ui.data(|data| data.get_temp::<usize>(id));
    let buttons = [egui::PointerButton::Primary, egui::PointerButton::Secondary];
    if active.is_none() && ui.is_enabled() && response.hovered() {
        for (index, button) in buttons.into_iter().take(params.len()).enumerate() {
            if ui.input(|input| input.pointer.button_pressed(button)) {
                setter.begin_set_parameter(params[index]);
                active = Some(index);
                break;
            }
        }
    }
    if let Some(index) = active {
        let param = params[index];
        let down = ui.input(|input| input.pointer.button_down(buttons[index]));
        if !down || !ui.is_enabled() {
            setter.end_set_parameter(param);
            active = None;
        } else {
            let (delta, fine) = ui.input(|input| (input.pointer.delta().y, input.modifiers.shift));
            // Do not count the move into the control on the press frame.
            if !ui.input(|input| input.pointer.button_pressed(buttons[index])) && delta != 0.0 {
                let sensitivity = if fine { 0.0005 } else { 0.005 };
                let value = param.preview_plain(
                    (param.modulated_normalized_value() - delta * sensitivity).clamp(0.0, 1.0),
                );
                if value != param.modulated_plain_value() {
                    setter.set_parameter(param, value);
                    response.mark_changed();
                }
            }
            let reset = ui.input(|input| input.pointer.button_pressed(buttons[index]))
                && ui.input(|input| input.modifiers.ctrl || input.modifiers.command);
            if reset {
                setter.set_parameter(param, param.default_plain_value());
                response.mark_changed();
            }
        }
    }
    for (index, button) in buttons.into_iter().take(params.len()).enumerate() {
        if ui.is_enabled() && response.double_clicked_by(button) {
            setter.begin_set_parameter(params[index]);
            setter.set_parameter(params[index], params[index].default_plain_value());
            setter.end_set_parameter(params[index]);
            response.mark_changed();
        }
    }
    ui.data_mut(|data| {
        if let Some(index) = active {
            data.insert_temp(id, index);
        } else {
            data.remove::<usize>(id);
        }
    });
    let center = rect.center();
    for (index, param) in params.iter().enumerate() {
        let radius = if index == 0 { size * 0.5 - 4.0 } else { 15.0 };
        let base = if index == 0 {
            ui.visuals().selection.stroke.color
        } else {
            egui::Color32::from_rgb(235, 171, 76)
        };
        let color = if !ui.is_enabled() {
            ui.visuals().weak_text_color()
        } else if active == Some(index) {
            base.gamma_multiply(1.3)
        } else {
            base
        };
        let arc = |value: f32| {
            let start = std::f32::consts::PI * 0.75;
            (0..=48)
                .map(|step| {
                    let angle = start + std::f32::consts::PI * 1.5 * value * step as f32 / 48.0;
                    center + egui::vec2(angle.cos(), angle.sin()) * radius
                })
                .collect::<Vec<_>>()
        };
        ui.painter().add(egui::Shape::line(
            arc(1.0),
            egui::Stroke::new(3.0, ui.visuals().widgets.inactive.bg_fill),
        ));
        let value = param.modulated_normalized_value();
        ui.painter()
            .add(egui::Shape::line(arc(value), egui::Stroke::new(3.0, color)));
        let angle = std::f32::consts::PI * (0.75 + 1.5 * value);
        ui.painter().circle_filled(
            center + egui::vec2(angle.cos(), angle.sin()) * radius,
            3.0,
            color,
        );
    }
    response.on_hover_text(if dual {
        "Left drag: attenuation · Right drag: frequency\nShift: fine · Double-click or Ctrl/Cmd-click: reset that parameter"
    } else {
        "Drag: adjust · Shift: fine\nDouble-click or Ctrl/Cmd-click: reset"
    })
}

pub(super) fn q_value(ui: &mut egui::Ui, param: &FloatParam, setter: &ParamSetter<'_>) {
    let mut value = param.modulated_plain_value();
    let response = ui.add(
        egui::DragValue::new(&mut value)
            .range(0.2..=8.0)
            .speed(0.01)
            .fixed_decimals(2)
            .prefix("Q "),
    );
    // Atomic edits also cover keyboard/text entry, with balanced host gestures.
    if response.changed() && value.is_finite() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, value);
        setter.end_set_parameter(param);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug::prelude::FloatRange;
    use nice_plug_core::params::InternalParamMut;
    use nice_plug_core::{
        context::{PluginApi, gui::GuiContextInner},
        params::internals::ParamPtr,
        plugin::PluginState,
    };
    use std::sync::Mutex;

    struct Host {
        params: [FloatParam; 2],
        events: Mutex<Vec<(usize, &'static str)>>,
    }
    impl Host {
        fn new() -> Self {
            Self {
                params: [
                    FloatParam::new(
                        "Amount",
                        6.0,
                        FloatRange::Linear {
                            min: 0.0,
                            max: 24.0,
                        },
                    ),
                    FloatParam::new(
                        "Frequency",
                        250.0,
                        FloatRange::Linear {
                            min: 20.0,
                            max: 2000.0,
                        },
                    ),
                ],
                events: Mutex::new(Vec::new()),
            }
        }
        fn index(&self, ptr: ParamPtr) -> usize {
            self.params
                .iter()
                .position(|param| param.as_ptr() == ptr)
                .unwrap()
        }
    }
    impl GuiContextInner for Host {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }
        unsafe fn raw_begin_set_parameter(&self, ptr: ParamPtr) {
            self.events.lock().unwrap().push((self.index(ptr), "begin"));
        }
        unsafe fn raw_set_parameter_normalized(&self, ptr: ParamPtr, value: f32) {
            let index = self.index(ptr);
            self.events.lock().unwrap().push((index, "set"));
            // Single-threaded test host; valid normalized values only.
            unsafe {
                self.params[index]
                    ._internal_set_plain_value(self.params[index].preview_plain(value));
            }
        }
        unsafe fn raw_end_set_parameter(&self, ptr: ParamPtr) {
            self.events.lock().unwrap().push((self.index(ptr), "end"));
        }
        fn get_state(&self) -> PluginState {
            panic!("unused")
        }
        fn set_state(&self, _: PluginState) {
            panic!("unused")
        }
        fn request_restart(&self) {}
    }
    fn frame(
        ctx: &egui::Context,
        host: &Host,
        time: f64,
        events: Vec<egui::Event>,
        enabled: bool,
    ) -> (egui::Rect, String) {
        frame_kind(ctx, host, time, events, enabled, true)
    }

    fn frame_kind(
        ctx: &egui::Context,
        host: &Host,
        time: f64,
        events: Vec<egui::Event>,
        enabled: bool,
        dual: bool,
    ) -> (egui::Rect, String) {
        let mut rect = egui::Rect::NOTHING;
        let setter = ParamSetter::new(host);
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 300.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                // Inject the modifier state explicitly for headless input.
                ui.input_mut(|input| input.modifiers = modifiers);
                ui.add_enabled_ui(enabled, |ui| {
                    rect = if dual {
                        dual_knob(ui, &host.params[0], &host.params[1], &setter).rect
                    } else {
                        single_knob(ui, &host.params[0], &setter).rect
                    };
                });
            },
        );
        output.textures_delta.clear();
        (rect, format!("{:?}", output.shapes))
    }
    fn button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    #[test]
    fn each_button_edits_only_its_parameter_and_balances_gestures_outside() {
        for (index, mouse) in [egui::PointerButton::Primary, egui::PointerButton::Secondary]
            .into_iter()
            .enumerate()
        {
            let ctx = egui::Context::default();
            let host = Host::new();
            let (rect, _) = frame(&ctx, &host, 0.0, vec![], true);
            let start = rect.center();
            frame(
                &ctx,
                &host,
                0.1,
                vec![egui::Event::PointerMoved(start), button(start, mouse, true)],
                true,
            );
            let end = start - egui::vec2(0.0, 70.0);
            frame(&ctx, &host, 0.2, vec![egui::Event::PointerMoved(end)], true);
            frame(&ctx, &host, 0.3, vec![button(end, mouse, false)], true);
            assert!(host.params[index].value() > host.params[index].default_plain_value());
            assert_eq!(
                host.params[1 - index].value(),
                host.params[1 - index].default_plain_value()
            );
            assert_eq!(
                *host.events.lock().unwrap(),
                [(index, "begin"), (index, "set"), (index, "end")]
            );
        }
    }
    #[test]
    fn automation_repaints_without_edits_and_disabled_control_does_not_edit() {
        let ctx = egui::Context::default();
        let host = Host::new();
        frame(&ctx, &host, 0.0, vec![], true);
        let (rect, before) = frame(&ctx, &host, 0.1, vec![], true);
        unsafe {
            host.params[1]._internal_set_plain_value(1500.0);
        }
        let (_, after) = frame(&ctx, &host, 0.2, vec![], true);
        assert_ne!(before, after);
        let pos = rect.center();
        frame(
            &ctx,
            &host,
            0.3,
            vec![
                egui::Event::PointerMoved(pos),
                button(pos, egui::PointerButton::Secondary, true),
            ],
            false,
        );
        frame(
            &ctx,
            &host,
            0.4,
            vec![egui::Event::PointerMoved(pos - egui::vec2(0.0, 15.0))],
            false,
        );
        frame(
            &ctx,
            &host,
            0.5,
            vec![button(pos, egui::PointerButton::Secondary, false)],
            false,
        );
        assert!(host.events.lock().unwrap().is_empty());
    }
    #[test]
    fn double_click_resets_only_selected_parameter() {
        for (index, mouse) in [egui::PointerButton::Primary, egui::PointerButton::Secondary]
            .into_iter()
            .enumerate()
        {
            let ctx = egui::Context::default();
            let host = Host::new();
            unsafe {
                host.params[index]._internal_set_plain_value(host.params[index].preview_plain(0.8));
            }
            let (rect, _) = frame(&ctx, &host, 0.0, vec![], true);
            let pos = rect.center();
            for (time, pressed) in [(0.1, true), (0.15, false), (0.2, true), (0.25, false)] {
                frame(
                    &ctx,
                    &host,
                    time,
                    vec![egui::Event::PointerMoved(pos), button(pos, mouse, pressed)],
                    true,
                );
            }
            assert_eq!(
                host.params[index].value(),
                host.params[index].default_plain_value()
            );
            let events = host.events.lock().unwrap();
            assert_eq!(
                events.iter().filter(|(_, e)| *e == "begin").count(),
                events.iter().filter(|(_, e)| *e == "end").count()
            );
            assert!(events.iter().all(|(i, _)| *i == index));
        }
    }

    #[test]
    fn single_ring_drag_fine_adjustment_and_reset_keep_balanced_host_gestures() {
        for fine in [false, true] {
            let ctx = egui::Context::default();
            let host = Host::new();
            let (rect, _) = frame_kind(&ctx, &host, 0.0, vec![], true, false);
            assert_eq!(rect.size(), egui::vec2(48.0, 48.0));
            let start = rect.center();
            let mouse = egui::PointerButton::Primary;
            frame_kind(
                &ctx,
                &host,
                0.1,
                vec![egui::Event::PointerMoved(start), button(start, mouse, true)],
                true,
                false,
            );
            let end = start - egui::vec2(0.0, 20.0);
            let mut events = vec![egui::Event::PointerMoved(end)];
            if fine {
                events.push(egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                });
            }
            frame_kind(&ctx, &host, 0.2, events, true, false);
            frame_kind(
                &ctx,
                &host,
                0.3,
                vec![button(end, mouse, false)],
                true,
                false,
            );
            let expected = if fine { 6.24 } else { 8.4 };
            assert!(
                (host.params[0].value() - expected).abs() < 0.001,
                "fine={fine}: actual={}, expected={expected}",
                host.params[0].value()
            );
            assert_eq!(
                *host.events.lock().unwrap(),
                [(0, "begin"), (0, "set"), (0, "end")]
            );
            for (time, pressed) in [(0.5, true), (0.55, false), (0.6, true), (0.65, false)] {
                frame_kind(
                    &ctx,
                    &host,
                    time,
                    vec![
                        egui::Event::PointerMoved(start),
                        button(start, mouse, pressed),
                    ],
                    true,
                    false,
                );
            }
            assert_eq!(host.params[0].value(), 6.0);
            let events = host.events.lock().unwrap();
            assert_eq!(
                events.iter().filter(|(_, e)| *e == "begin").count(),
                events.iter().filter(|(_, e)| *e == "end").count()
            );
        }
    }

    #[test]
    fn single_ring_tracks_automation_and_ignores_secondary_drag() {
        let ctx = egui::Context::default();
        let host = Host::new();
        frame_kind(&ctx, &host, 0.0, vec![], true, false);
        let (rect, before) = frame_kind(&ctx, &host, 0.1, vec![], true, false);
        unsafe {
            host.params[0]._internal_set_plain_value(18.0);
        }
        let (_, after) = frame_kind(&ctx, &host, 0.2, vec![], true, false);
        assert_ne!(before, after);
        let start = rect.center();
        let mouse = egui::PointerButton::Secondary;
        frame_kind(
            &ctx,
            &host,
            0.3,
            vec![egui::Event::PointerMoved(start), button(start, mouse, true)],
            true,
            false,
        );
        frame_kind(
            &ctx,
            &host,
            0.4,
            vec![egui::Event::PointerMoved(start - egui::vec2(0.0, 30.0))],
            true,
            false,
        );
        frame_kind(
            &ctx,
            &host,
            0.5,
            vec![button(start, mouse, false)],
            true,
            false,
        );
        assert_eq!(host.params[0].value(), 18.0);
        assert!(host.events.lock().unwrap().is_empty());
    }
}
