#![cfg(feature = "plugin")]

use std::sync::{Arc, Mutex};

use egui_plugin::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use nice_plug_core::{
    context::{
        PluginApi,
        gui::{GuiContextInner, ParamSetter},
    },
    params::{FloatParam, InternalParamMut, Param, internals::ParamPtr, range::FloatRange},
    plugin::PluginState,
};
use nice_plug_egui::widgets::ParamKnob;

struct Host {
    param: Arc<FloatParam>,
    events: Mutex<Vec<&'static str>>,
}

impl Host {
    fn new() -> Self {
        Self {
            param: Arc::new(
                FloatParam::new(
                    "Time",
                    2.5,
                    FloatRange::Skewed {
                        min: 0.1,
                        max: 20.0,
                        factor: FloatRange::skew_factor(-2.0),
                    },
                )
                .with_unit(" s"),
            ),
            events: Mutex::new(Vec::new()),
        }
    }

    fn automate(&self, value: f32) {
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
    unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {
        self.events.lock().unwrap().push("begin");
    }
    unsafe fn raw_set_parameter_normalized(&self, _param: ParamPtr, normalized: f32) {
        self.events.lock().unwrap().push("set");
        self.automate(self.param.preview_plain(normalized));
    }
    unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {
        self.events.lock().unwrap().push("end");
    }
    fn get_state(&self) -> PluginState {
        panic!("not used in widget tests")
    }
    fn set_state(&self, _state: PluginState) {
        panic!("not used in widget tests")
    }
    fn request_restart(&self) {}
}

fn frame(ctx: &Context, host: &Host, time: f64, events: Vec<Event>) -> (Rect, String) {
    let setter = ParamSetter::new(host);
    let mut rect = Rect::NOTHING;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0))),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            rect = ui
                .add(ParamKnob::for_param(host.param.as_ref(), &setter).with_scroll(true))
                .rect;
        },
    );
    output.textures_delta.clear(); // Headless test: no renderer consumes texture uploads.
    (rect, format!("{:?}", output.shapes))
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn knob_is_centered_in_available_width() {
    let ctx = Context::default();
    let host = Host::new();
    let (rect, _) = frame(&ctx, &host, 0.0, vec![]);
    assert!((rect.center().x - 200.0).abs() <= 1.0, "{rect:?}");
}

#[test]
fn host_automation_updates_rendering_without_sending_edits() {
    let ctx = Context::default();
    let host = Host::new();
    frame(&ctx, &host, 0.0, vec![]);
    let (_, before) = frame(&ctx, &host, 0.1, vec![]);
    host.automate(15.0);
    let (_, after) = frame(&ctx, &host, 0.2, vec![]);
    assert_ne!(before, after);
    assert!(host.events.lock().unwrap().is_empty());
}

#[test]
fn drag_is_one_balanced_host_gesture() {
    let ctx = Context::default();
    let host = Host::new();
    let (rect, _) = frame(&ctx, &host, 0.0, vec![]);
    let start = rect.center();
    frame(
        &ctx,
        &host,
        0.1,
        vec![Event::PointerMoved(start), button(start, true)],
    );
    let end = start - Vec2::new(0.0, 20.0);
    frame(&ctx, &host, 0.2, vec![Event::PointerMoved(end)]);
    frame(&ctx, &host, 0.3, vec![button(end, false)]);
    let events = host.events.lock().unwrap();
    assert_eq!(events.first(), Some(&"begin"));
    assert_eq!(events.last(), Some(&"end"));
    assert!(events.contains(&"set"));
    assert_eq!(events.iter().filter(|&&e| e == "begin").count(), 1);
    assert_eq!(events.iter().filter(|&&e| e == "end").count(), 1);
    assert!(host.param.modulated_plain_value() > 2.5);
}

#[test]
fn scroll_and_double_click_notify_host_and_reset_to_default() {
    let ctx = Context::default();
    let host = Host::new();
    let (rect, _) = frame(&ctx, &host, 0.0, vec![]);
    let pos = rect.center();
    frame(
        &ctx,
        &host,
        0.1,
        vec![
            Event::PointerMoved(pos),
            Event::MouseWheel {
                phase: egui_plugin::TouchPhase::Move,
                source: egui_plugin::MouseWheelSource::Unknown,
                unit: egui_plugin::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, 10.0),
                modifiers: Modifiers::NONE,
            },
        ],
    );
    assert_eq!(*host.events.lock().unwrap(), ["begin", "set", "end"]);
    assert!(host.param.modulated_plain_value() > 2.5);
    for (time, pressed) in [(0.2, true), (0.25, false), (0.3, true), (0.35, false)] {
        frame(&ctx, &host, time, vec![button(pos, pressed)]);
    }
    assert_eq!(
        host.param.modulated_plain_value(),
        host.param.default_plain_value()
    );
    let events = host.events.lock().unwrap();
    assert_eq!(
        events.iter().filter(|&&e| e == "begin").count(),
        events.iter().filter(|&&e| e == "end").count()
    );
}

#[test]
fn stepped_parameter_sends_snapped_values_to_host() {
    let ctx = Context::default();
    let mut host = Host::new();
    host.param = Arc::new(
        FloatParam::new("Steps", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
            .with_step_size(0.1),
    );
    let (rect, _) = frame(&ctx, &host, 0.0, vec![]);
    frame(
        &ctx,
        &host,
        0.1,
        vec![
            Event::PointerMoved(rect.center()),
            Event::MouseWheel {
                phase: egui_plugin::TouchPhase::Move,
                source: egui_plugin::MouseWheelSource::Unknown,
                unit: egui_plugin::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, 27.0),
                modifiers: Modifiers::NONE,
            },
        ],
    );
    assert!((host.param.modulated_plain_value() - 0.6).abs() < 1e-6);
    assert_eq!(*host.events.lock().unwrap(), ["begin", "set", "end"]);
}
