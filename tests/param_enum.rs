#![cfg(feature = "plugin")]

use std::sync::Mutex;

use egui_plugin::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use nice_plug_core::{
    context::{
        PluginApi,
        gui::{GuiContextInner, ParamSetter},
    },
    params::{EnumParam, InternalParamMut, Param, enums::Enum, internals::ParamPtr},
    plugin::PluginState,
};
use nice_plug_egui::widgets::{ParamComboBox, ParamEnumButton};

#[derive(Enum, PartialEq)]
enum Source {
    Internal,
    #[name = "External input"]
    External,
    Other,
}

struct Host {
    param: EnumParam<Source>,
    events: Mutex<Vec<&'static str>>,
}

impl Host {
    fn new() -> Self {
        Self {
            param: EnumParam::new("Source", Source::Internal),
            events: Mutex::new(Vec::new()),
        }
    }

    fn automate(&self, value: Source) {
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

#[derive(Clone, Copy)]
enum View {
    Combo,
    Buttons,
    Toggle,
}

struct Output {
    rects: Vec<Rect>,
    changed: bool,
    shapes: String,
    external_label: Option<Rect>,
}

fn find_label(shape: &egui_plugin::epaint::Shape) -> Option<Rect> {
    use egui_plugin::epaint::Shape;
    match shape {
        Shape::Text(text) if text.galley.text() == "External input" => {
            Some(Rect::from_min_size(text.pos, text.galley.size()))
        }
        Shape::Vec(shapes) => shapes.iter().find_map(find_label),
        _ => None,
    }
}

fn frame(
    ctx: &Context,
    host: &Host,
    time: f64,
    events: Vec<Event>,
    view: View,
    enabled: bool,
) -> Output {
    let setter = ParamSetter::new(host);
    let mut result = Output {
        rects: Vec::new(),
        changed: false,
        shapes: String::new(),
        external_label: None,
    };
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0))),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            ui.add_enabled_ui(enabled, |ui| match view {
                View::Toggle => {
                    let response = ui.add(ParamEnumButton::cycling(&host.param, &setter));
                    result.rects.push(response.rect);
                    result.changed |= response.changed();
                }
                View::Combo => {
                    let response =
                        ui.add(ParamComboBox::for_param(&host.param, &setter).with_width(200.0));
                    result.rects.push(response.rect);
                    result.changed |= response.changed();
                }
                View::Buttons => {
                    ui.horizontal(|ui| {
                        for source in [Source::Internal, Source::External] {
                            let response =
                                ui.add(ParamEnumButton::for_param(&host.param, &setter, source));
                            result.rects.push(response.rect);
                            result.changed |= response.changed();
                        }
                    });
                }
            });
        },
    );
    output.textures_delta.clear();
    result.shapes = format!("{:?}", output.shapes);
    result.external_label = output
        .shapes
        .iter()
        .find_map(|shape| find_label(&shape.shape));
    result
}

fn click(ctx: &Context, host: &Host, pos: Pos2, time: f64, view: View, enabled: bool) -> Output {
    frame(
        ctx,
        host,
        time,
        vec![Event::PointerMoved(pos), pointer(pos, true)],
        view,
        enabled,
    );
    frame(
        ctx,
        host,
        time + 0.05,
        vec![pointer(pos, false)],
        view,
        enabled,
    )
}

fn pointer(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn enum_buttons_select_without_redundant_edits() {
    let ctx = Context::default();
    let host = Host::new();
    let output = frame(&ctx, &host, 0.0, vec![], View::Buttons, true);
    assert!(
        !click(
            &ctx,
            &host,
            output.rects[0].center(),
            0.1,
            View::Buttons,
            true
        )
        .changed
    );
    assert!(host.events.lock().unwrap().is_empty());
    assert!(
        click(
            &ctx,
            &host,
            output.rects[1].center(),
            0.8,
            View::Buttons,
            true
        )
        .changed
    );
    assert_eq!(host.param.value().to_index(), 1);
    assert_eq!(*host.events.lock().unwrap(), ["begin", "set", "end"]);
    assert!(
        !click(
            &ctx,
            &host,
            output.rects[1].center(),
            1.5,
            View::Buttons,
            true
        )
        .changed
    );
    assert_eq!(host.events.lock().unwrap().len(), 3);
    assert!(
        click(
            &ctx,
            &host,
            output.rects[0].center(),
            2.2,
            View::Buttons,
            true
        )
        .changed
    );
    assert_eq!(host.param.value().to_index(), 0);
}

#[test]
fn combo_selects_variant_using_its_display_name_and_balanced_gesture() {
    let ctx = Context::default();
    let host = Host::new();
    let output = frame(&ctx, &host, 0.0, vec![], View::Combo, true);
    click(
        &ctx,
        &host,
        output.rects[0].center(),
        0.1,
        View::Combo,
        true,
    );
    let output = frame(&ctx, &host, 0.3, vec![], View::Combo, true);
    let pos = output
        .external_label
        .expect("popup contains the custom enum label")
        .center();
    let output = click(&ctx, &host, pos, 0.4, View::Combo, true);
    assert!(output.changed);
    assert_eq!(host.param.value().to_index(), 1);
    assert_eq!(*host.events.lock().unwrap(), ["begin", "set", "end"]);
    let output = frame(&ctx, &host, 0.7, vec![], View::Combo, true);
    click(
        &ctx,
        &host,
        output.rects[0].center(),
        0.8,
        View::Combo,
        true,
    );
    let output = frame(&ctx, &host, 1.0, vec![], View::Combo, true);
    assert!(
        !click(
            &ctx,
            &host,
            output.external_label.unwrap().center(),
            1.1,
            View::Combo,
            true
        )
        .changed
    );
    assert_eq!(host.events.lock().unwrap().len(), 3);
}

#[test]
fn automation_updates_both_widgets_without_sending_edits() {
    for view in [View::Combo, View::Buttons, View::Toggle] {
        let ctx = Context::default();
        let host = Host::new();
        frame(&ctx, &host, 0.0, vec![], view, true);
        let before = frame(&ctx, &host, 0.1, vec![], view, true);
        host.automate(Source::External);
        frame(&ctx, &host, 0.2, vec![], view, true);
        let after = frame(&ctx, &host, 0.8, vec![], view, true);
        assert_ne!(before.shapes, after.shapes);
        assert!(!after.changed);
        assert!(host.events.lock().unwrap().is_empty());
    }
}

#[test]
fn disabled_widgets_do_not_edit_parameters() {
    for view in [View::Combo, View::Buttons, View::Toggle] {
        let ctx = Context::default();
        let host = Host::new();
        let output = frame(&ctx, &host, 0.0, vec![], view, false);
        let rect = *output.rects.last().unwrap();
        let output = click(&ctx, &host, rect.center(), 0.1, view, false);
        assert!(!output.changed);
        assert_eq!(host.param.value().to_index(), 0);
        assert!(host.events.lock().unwrap().is_empty());
    }
}

#[test]
fn single_button_cycles_labels_and_wraps_with_one_gesture_per_click() {
    let ctx = Context::default();
    let host = Host::new();
    for (step, expected) in [1, 2, 0].into_iter().enumerate() {
        let time = step as f64 + 0.1;
        let output = frame(&ctx, &host, time, vec![], View::Toggle, true);
        assert!(
            click(
                &ctx,
                &host,
                output.rects[0].center(),
                time + 0.1,
                View::Toggle,
                true
            )
            .changed
        );
        assert_eq!(host.param.value().to_index(), expected);
        let output = frame(&ctx, &host, time + 0.3, vec![], View::Toggle, true);
        assert!(output.shapes.contains(Source::variants()[expected]));
    }
    assert_eq!(
        *host.events.lock().unwrap(),
        ["begin", "set", "end"].repeat(3)
    );
}
