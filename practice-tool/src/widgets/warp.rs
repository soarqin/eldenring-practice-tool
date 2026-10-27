use std::mem;
use std::time::{Duration, Instant};

use hudhook::tracing::info;
use imgui::sys::{igGetCursorPosX, igGetCursorPosY, igGetWindowPos, igSetNextWindowPos, ImVec2};
use imgui::{Condition, WindowFlags};
use libeldenring::prelude::*;
use practice_tool_core::key::Key;
use practice_tool_core::widgets::{scaling_factor, Widget, BUTTON_HEIGHT, BUTTON_WIDTH};

use super::font_text::{display_text, filter_input};
use super::string_match;
use crate::fonts::request_text;

type WarpFunc = extern "system" fn(u64, u64, u32);
const POPUP_TAG: &str = "##warp";

#[derive(Debug)]
pub(crate) struct Warp {
    label_close: String,
    hotkey_close: Key,
    warp_ptr: usize,
    arg1: &'static PointerChain<u64>,
    arg2: &'static PointerChain<u64>,
    sentinel: &'static Bitflag<u8>,
    data: GameData,
    graces: Vec<Grace>,
    current_grace: Option<usize>,
    filter_string: String,
    filtered: Vec<usize>,
    last_read: Option<Instant>,
}

impl Warp {
    pub(crate) fn new(
        warp_ptr: usize,
        arg1: &'static PointerChain<u64>,
        arg2: &'static PointerChain<u64>,
        data: GameData,
        sentinel: &'static Bitflag<u8>,
        hotkey_close: Key,
    ) -> Self {
        Self {
            label_close: format!("Close ({hotkey_close})"),
            hotkey_close,
            warp_ptr,
            arg1,
            arg2,
            data,
            sentinel,
            graces: Vec::new(),
            current_grace: None,
            filter_string: String::new(),
            filtered: Vec::new(),
            last_read: None,
        }
    }

    fn refresh(&mut self) {
        let previous = self.current_grace.and_then(|i| self.graces.get(i)).map(|g| g.id);
        self.graces = self.data.graces().unwrap_or_default();
        self.current_grace = previous.and_then(|id| self.graces.iter().position(|g| g.id == id));
        self.last_read = Some(Instant::now());
        self.filter();
    }

    fn filter(&mut self) {
        self.filtered = self
            .graces
            .iter()
            .enumerate()
            .filter(|(_, grace)| string_match(&self.filter_string, &grace.label))
            .map(|(index, _)| index)
            .collect();
        if !self.current_grace.is_some_and(|i| self.filtered.contains(&i)) {
            self.current_grace = self.filtered.first().copied();
        }
    }

    fn warp(&self) {
        let Some(grace) = self.current_grace.and_then(|i| self.graces.get(i)) else {
            return;
        };
        if self.sentinel.get().is_none() {
            return;
        }
        let (Some(arg1), Some(arg2)) = (self.arg1.read(), self.arg2.read()) else {
            return;
        };
        if arg1 == 0 || arg2 == 0 {
            return;
        }
        let warp_fn: WarpFunc = unsafe { mem::transmute(self.warp_ptr) };
        info!("Warping to {} (entity {})", grace.name, grace.entity_id);
        warp_fn(arg1, arg2, grace.entity_id);
    }
}

impl Widget for Warp {
    fn render(&mut self, ui: &imgui::Ui) {
        let scale = scaling_factor(ui);
        let button_height = BUTTON_HEIGHT;
        let (x, y) = unsafe {
            let mut wnd_pos = ImVec2::default();
            igGetWindowPos(&mut wnd_pos);
            (igGetCursorPosX() + wnd_pos.x, igGetCursorPosY() + wnd_pos.y)
        };
        if ui.button_with_size("Warp to Grace", [BUTTON_WIDTH * scale, button_height]) {
            self.refresh();
            ui.open_popup(POPUP_TAG);
        }
        unsafe {
            igSetNextWindowPos(
                ImVec2::new(x + 200. * scale, y),
                Condition::Always as i8 as _,
                ImVec2::new(0., 0.),
            )
        };
        if let Some(_token) = ui
            .modal_popup_config(POPUP_TAG)
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_RESIZE
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_SCROLLBAR
                    | WindowFlags::ALWAYS_AUTO_RESIZE,
            )
            .begin_popup()
        {
            if self.graces.is_empty()
                && self.last_read.is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
            {
                self.refresh();
            }
            let _width = ui.push_item_width(400.);
            if filter_input(ui, "##warp-filter", "Filter...", &mut self.filter_string) {
                self.filter();
            }
            let preview = self
                .current_grace
                .and_then(|i| self.graces.get(i))
                .map(|g| g.label.as_str())
                .unwrap_or("No matching graces");
            request_text(ui, preview);
            if let Some(_combo) = ui.begin_combo("##warp-graces", display_text(ui, preview)) {
                for &index in &self.filtered {
                    let grace = &self.graces[index];
                    let _id = ui.push_id_int(grace.id as i32);
                    let selected = self.current_grace == Some(index);
                    let label = format!("{}###row", display_text(ui, &grace.label));
                    if ui.selectable_config(&label).selected(selected).build() {
                        self.current_grace = Some(index);
                    }
                    if ui.is_item_visible() {
                        request_text(ui, &grace.label);
                    }
                    if selected {
                        ui.set_item_default_focus();
                    }
                }
            }
            if self.graces.is_empty() {
                ui.text("Waiting for game data...");
            }
            {
                let _disabled = ui
                    .begin_disabled(self.current_grace.is_none() || self.sentinel.get().is_none());
                if ui.button_with_size("Warp", [400., button_height]) {
                    self.warp();
                }
            }
            if ui.button_with_size(&self.label_close, [400., button_height])
                || (self.hotkey_close.is_pressed(ui) && !ui.is_any_item_active())
            {
                ui.close_current_popup();
            }
        }
    }
}
