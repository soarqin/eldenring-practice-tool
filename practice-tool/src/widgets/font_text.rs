use imgui::{
    sys, InputText, InputTextCallback, InputTextCallbackHandler, StyleColor, TextCallbackData, Ui,
};

use crate::fonts::{request_text, text_ready};

pub(super) fn display_text<'a>(ui: &Ui, text: &'a str) -> &'a str {
    if text_ready(ui, text) {
        text
    } else if text_ready(ui, "加载中...") {
        "加载中..."
    } else {
        "Loading..."
    }
}

struct InputFontState<'a> {
    ui: &'a Ui,
    color: [f32; 4],
    hint_color: [f32; 4],
    cancel: bool,
    pending: bool,
}

impl InputFontState<'_> {
    fn update(&mut self, text: &str) {
        request_text(self.ui, text);
        let pending = self.cancel || !text_ready(self.ui, text);
        self.pending = pending;
        // InputText reads these colors after callbacks; the outer style
        // tokens restore them, including when the callback never runs.
        for (role, mut color) in
            [(StyleColor::Text, self.color), (StyleColor::TextDisabled, self.hint_color)]
        {
            if pending {
                color[3] = 0.;
            }
            unsafe { (*sys::igGetStyle()).Colors[role as usize] = color.into() };
        }
    }
}

impl InputTextCallbackHandler for &mut InputFontState<'_> {
    fn on_always(&mut self, data: TextCallbackData) {
        self.update(data.str());
    }
}

pub(super) fn filter_input(ui: &Ui, label: &str, hint: &str, text: &mut String) -> bool {
    debug_assert!(label.starts_with("##"), "filter_input requires a hidden label");
    request_text(ui, hint);
    let hint = display_text(ui, hint);
    let color = ui.style_color(StyleColor::Text);
    let hint_color = ui.style_color(StyleColor::TextDisabled);
    let width = ui.calc_item_width();
    // Cancellation skips ALWAYS; hide native text until the restored buffer
    // can be checked below. Input, selection and undo remain native.
    let cancel = ui.is_key_pressed(imgui::Key::Escape)
        || (ui.io().config_flags.contains(imgui::ConfigFlags::NAV_ENABLE_GAMEPAD)
            && ui.io().backend_flags.contains(imgui::BackendFlags::HAS_GAMEPAD)
            && ui.is_key_pressed(imgui::Key::GamepadFaceRight));
    let style = ui.push_style_color(StyleColor::Text, color);
    let hint_style = ui.push_style_color(StyleColor::TextDisabled, hint_color);
    let mut state = InputFontState { ui, color, hint_color, cancel, pending: false };
    state.update(text);
    let changed = InputText::new(ui, label, text)
        .hint(hint)
        .callback(InputTextCallback::ALWAYS, &mut state)
        .build();
    drop(hint_style);
    drop(style);
    if cancel {
        request_text(ui, text);
    }

    if state.pending && ui.is_item_visible() {
        let min = ui.item_rect_min();
        let mut max = ui.item_rect_max();
        max[0] = min[0] + width;
        let style = ui.clone_style();
        let pos = [min[0] + style.frame_padding[0], min[1] + style.frame_padding[1]];
        let (text, mut color) = if cancel && text.is_empty() {
            (hint, hint_color)
        } else {
            (display_text(ui, text), color)
        };
        color[3] *= style.alpha;
        let draw = ui.get_window_draw_list();
        draw.with_clip_rect_intersect(min, max, || draw.add_text(pos, color, text));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_glyph(data: &imgui::DrawData, uv: [f32; 4]) -> bool {
        let corners = [[uv[0], uv[1]], [uv[2], uv[1]], [uv[2], uv[3]], [uv[0], uv[3]]];
        data.draw_lists().any(|list| {
            list.vtx_buffer().windows(4).any(|quad| {
                quad.iter().zip(corners).all(|(vertex, expected)| vertex.uv == expected)
            })
        })
    }

    #[test]
    fn pending_chinese_hint_uses_loading_before_and_after_cancel() {
        let _lock = crate::fonts::IMGUI_TEST_LOCK.lock().unwrap();
        crate::fonts::FontFallback::default().reset();
        let mut ctx = imgui::Context::create();
        ctx.set_ini_filename(None);
        ctx.io_mut().display_size = [400., 300.];
        ctx.io_mut().delta_time = 1. / 60.;
        ctx.io_mut().config_input_trickle_event_queue = false;
        ctx.fonts().build_rgba32_texture();
        let mut text = String::new();
        let mut fallback_uv = [0.; 4];
        let mut loading_uv = [0.; 4];

        for frame in 0..5 {
            if frame == 2 {
                ctx.io_mut().add_input_character('A');
            } else if frame == 3 || frame == 4 {
                ctx.io_mut().add_key_event(imgui::Key::Escape, frame == 3);
            }
            let ui = ctx.frame();
            ui.window("##hint-test")
                .position([10., 10.], imgui::Condition::Always)
                .size([300., 100.], imgui::Condition::Always)
                .title_bar(false)
                .build(|| {
                    if frame == 0 {
                        ui.set_keyboard_focus_here();
                    }
                    assert!(!text_ready(ui, "筛选..."));
                    assert_eq!(display_text(ui, "筛选..."), "Loading...");
                    filter_input(ui, "##filter", "筛选...", &mut text);
                    assert_eq!(text, if frame == 2 { "A" } else { "" });
                    if frame == 1 {
                        assert!(ui.is_item_active());
                    } else if frame == 3 {
                        assert!(!ui.is_item_active());
                    }
                    let font = ui.current_font() as *const imgui::Font as *mut sys::ImFont;
                    let uv = |character| unsafe {
                        let glyph = &*sys::ImFont_FindGlyph(font, character);
                        [glyph.U0, glyph.V0, glyph.U1, glyph.V1]
                    };
                    fallback_uv = uv('筛' as u32);
                    loading_uv = uv('L' as u32);
                });
            let draw = ctx.render();
            if [1, 3, 4].contains(&frame) {
                assert!(!contains_glyph(draw, fallback_uv));
                assert!(contains_glyph(draw, loading_uv));
            }
        }
        crate::fonts::FontFallback::default().reset();
    }

    #[test]
    fn new_input_waits_without_changing_buffer_focus_or_style() {
        struct Clipboard;
        impl imgui::ClipboardBackend for Clipboard {
            fn get(&mut self) -> Option<String> {
                Some("文".into())
            }

            fn set(&mut self, _: &str) {}
        }

        let _lock = crate::fonts::IMGUI_TEST_LOCK.lock().unwrap();
        crate::fonts::FontFallback::default().reset();
        let mut ctx = imgui::Context::create();
        ctx.set_ini_filename(None);
        ctx.set_clipboard_backend(Clipboard);
        ctx.io_mut().display_size = [400., 300.];
        ctx.io_mut().delta_time = 1. / 60.;
        ctx.io_mut().config_input_trickle_event_queue = false;
        ctx.fonts().build_rgba32_texture();
        let mut text = String::new();
        let mut fallback_uv = [0.; 4];
        let mut loading_uv = [0.; 4];
        let mut hint_uv = [0.; 4];
        let mut ascii_uv = [0.; 4];

        for frame in 0..16 {
            let io = ctx.io_mut();
            match frame {
                2 => io.add_input_character('中'),
                3 => {
                    io.add_key_event(imgui::Key::ModCtrl, true);
                    io.add_key_event(imgui::Key::A, true);
                },
                4 => {
                    io.add_key_event(imgui::Key::A, false);
                    io.add_key_event(imgui::Key::V, true);
                },
                5 => {
                    io.add_key_event(imgui::Key::V, false);
                    io.add_key_event(imgui::Key::ModCtrl, false);
                    io.add_key_event(imgui::Key::Backspace, true);
                },
                6 => {
                    io.add_key_event(imgui::Key::Backspace, false);
                    io.add_input_character('A');
                },
                7 => io.add_key_event(imgui::Key::Escape, true),
                8 => {
                    io.add_key_event(imgui::Key::Escape, false);
                    text = "中".into();
                },
                9 => {
                    io.add_key_event(imgui::Key::ModCtrl, true);
                    io.add_key_event(imgui::Key::A, true);
                },
                10 => {
                    io.add_key_event(imgui::Key::A, false);
                    io.add_key_event(imgui::Key::ModCtrl, false);
                    io.add_input_character('A');
                },
                11 => io.add_key_event(imgui::Key::Escape, true),
                12 => {
                    io.add_key_event(imgui::Key::Escape, false);
                    text = "A".into();
                },
                13 | 15 => io.add_key_event(imgui::Key::Escape, true),
                14 => {
                    io.add_key_event(imgui::Key::Escape, false);
                    text.clear();
                },
                _ => {},
            }
            let ui = ctx.frame();
            ui.window("##input-test")
                .position([10., 10.], imgui::Condition::Always)
                .size([300., 100.], imgui::Condition::Always)
                .title_bar(false)
                .build(|| {
                    if frame == 0 || frame == 8 {
                        ui.set_keyboard_focus_here();
                    }
                    let color = ui.style_color(StyleColor::Text);
                    let hint_color = ui.style_color(StyleColor::TextDisabled);
                    filter_input(ui, "##filter", "Filter...", &mut text);
                    assert_eq!(ui.style_color(StyleColor::Text), color);
                    assert_eq!(ui.style_color(StyleColor::TextDisabled), hint_color);
                    if [1, 2, 3, 4, 5, 6, 9, 10].contains(&frame) {
                        assert!(ui.is_item_active());
                    }
                    if frame == 2 || frame == 4 || frame == 11 {
                        assert_eq!(text, if frame == 4 { "文" } else { "中" });
                        assert!(!text_ready(ui, &text));
                        let font = ui.current_font() as *const imgui::Font as *mut sys::ImFont;
                        let uv = |character| unsafe {
                            let glyph = &*sys::ImFont_FindGlyph(font, character);
                            [glyph.U0, glyph.V0, glyph.U1, glyph.V1]
                        };
                        fallback_uv = uv('中' as u32);
                        loading_uv = uv('L' as u32);
                        hint_uv = uv('F' as u32);
                        ascii_uv = uv('A' as u32);
                    }
                    if frame == 5 {
                        assert!(text.is_empty());
                    }
                    if frame == 6 || frame == 10 {
                        assert_eq!(text, "A");
                        assert!(text_ready(ui, &text));
                    }
                    if frame == 7 || frame == 11 {
                        assert_eq!(text, if frame == 7 { "" } else { "中" });
                        assert!(!ui.is_item_active());
                    }
                });
            let draw = ctx.render();
            if frame == 2 || frame == 4 || frame == 11 {
                assert!(!contains_glyph(draw, fallback_uv));
                assert!(contains_glyph(draw, loading_uv));
                assert!(!contains_glyph(draw, hint_uv));
            }
            if [5, 6, 7, 13, 15].contains(&frame) {
                assert!(!contains_glyph(draw, loading_uv));
            }
            if frame == 7 || frame == 15 {
                assert!(contains_glyph(draw, hint_uv));
            }
            if frame == 13 {
                assert!(contains_glyph(draw, ascii_uv));
            }
        }
        crate::fonts::FontFallback::default().reset();
    }
}
