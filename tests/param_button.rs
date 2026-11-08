#![cfg(feature = "plugin")]

use std::sync::Mutex;

use egui_plugin::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use nice_plug_core::{
    context::{
        PluginApi,
        gui::{GuiContextInner, ParamSetter},
    },
    params::{BoolParam, InternalParamMut, Param, internals::ParamPtr},
    plugin::PluginState,
};
use nice_plug_egui::widgets::ParamButton;

struct Host {
    param: BoolParam,
    events: Mutex<Vec<&'static str>>,
}

impl Host {
    fn new() -> Self {
        Self {
            param: BoolParam::new("Bypass", false),
            events: Mutex::new(Vec::new()),
        }
    }

    fn automate(&self, value: bool) {
        // Test host emulation, with valid values and no concurrent access.
        unsafe {
            self.param._internal_set_plain_value(value);
        }
    }
}

impl GuiContextInner for Host {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    unsafe fn raw_begin_set_parameter(&self, _: ParamPtr) {
        self.events.lock().unwrap().push("begin");
    }
    unsafe fn raw_set_parameter_normalized(&self, _: ParamPtr, value: f32) {
        self.events.lock().unwrap().push("set");
        self.automate(self.param.preview_plain(value));
    }
    unsafe fn raw_end_set_parameter(&self, _: ParamPtr) {
        self.events.lock().unwrap().push("end");
    }
    fn get_state(&self) -> PluginState {
        panic!("not used in widget tests")
    }
    fn set_state(&self, _: PluginState) {
        panic!("not used in widget tests")
    }
    fn request_restart(&self) {}
}

fn frame(
    ctx: &Context,
    host: &Host,
    time: f64,
    events: Vec<Event>,
    enabled: bool,
) -> (Rect, bool, String) {
    let setter = ParamSetter::new(host);
    let mut rect = Rect::NOTHING;
    let mut changed = false;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0))),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            let response = ui.add_enabled(
                enabled,
                ParamButton::for_param(&host.param, &setter).with_label("Toggle bypass"),
            );
            rect = response.rect;
            changed = response.changed();
        },
    );
    output.textures_delta.clear();
    (rect, changed, format!("{:?}", output.shapes))
}

fn click(ctx: &Context, host: &Host, pos: Pos2, time: f64, enabled: bool) -> bool {
    let mut changed = false;
    for (offset, pressed) in [(0.0, true), (0.05, false)] {
        changed |= frame(
            ctx,
            host,
            time + offset,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            enabled,
        )
        .1;
    }
    changed
}

#[test]
fn clicks_flip_value_with_balanced_gestures_and_changed_response() {
    let ctx = Context::default();
    let host = Host::new();
    let (rect, _, _) = frame(&ctx, &host, 0.0, vec![], true);
    assert!(click(&ctx, &host, rect.center(), 0.1, true));
    assert!(host.param.modulated_plain_value());
    assert!(click(&ctx, &host, rect.center(), 0.8, true));
    assert!(!host.param.modulated_plain_value());
    assert_eq!(
        *host.events.lock().unwrap(),
        ["begin", "set", "end", "begin", "set", "end"]
    );
}

#[test]
fn automation_changes_selection_without_edits_and_next_click_flips_latest_value() {
    let ctx = Context::default();
    let host = Host::new();
    frame(&ctx, &host, 0.0, vec![], true);
    let (rect, _, before) = frame(&ctx, &host, 0.1, vec![], true);
    host.automate(true);
    frame(&ctx, &host, 0.2, vec![], true);
    let (_, changed, after) = frame(&ctx, &host, 0.8, vec![], true);
    assert_ne!(before, after);
    assert!(!changed);
    assert!(host.events.lock().unwrap().is_empty());
    assert!(click(&ctx, &host, rect.center(), 1.0, true));
    assert!(!host.param.modulated_plain_value());
}

#[test]
fn disabled_button_does_not_edit_parameter() {
    let ctx = Context::default();
    let host = Host::new();
    let (rect, _, _) = frame(&ctx, &host, 0.0, vec![], false);
    assert!(!click(&ctx, &host, rect.center(), 0.1, false));
    assert!(!host.param.modulated_plain_value());
    assert!(host.events.lock().unwrap().is_empty());
}
