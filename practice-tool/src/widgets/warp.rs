use std::mem;
use std::time::{Duration, Instant};

use hudhook::tracing::info;
use imgui::sys::{igGetCursorPosX, igGetCursorPosY, igGetWindowPos, igSetNextWindowPos, ImVec2};
use imgui::{Condition, WindowFlags};
use libeldenring::prelude::*;
use practice_tool_core::key::Key;
use practice_tool_core::widgets::{scaling_factor, Widget, BUTTON_HEIGHT, BUTTON_WIDTH};

use super::font_text::{display_text, filter_input};
use super::pinyin_match::{pinyin_match, pinyin_match_start, Segment};
use super::string_match;
use crate::fonts::request_text;

type WarpFunc = extern "system" fn(u64, u64, u32);
const POPUP_TAG: &str = "##warp";

#[derive(Debug)]
struct GracePinyinEntry {
    region: Segment,
    location: Segment,
}

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
    pinyin: Vec<GracePinyinEntry>,
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
            label_close: format!("关闭 ({hotkey_close})"),
            hotkey_close,
            warp_ptr,
            arg1,
            arg2,
            data,
            sentinel,
            graces: Vec::new(),
            pinyin: Vec::new(),
            current_grace: None,
            filter_string: String::new(),
            filtered: Vec::new(),
            last_read: None,
        }
    }

    fn refresh(&mut self) {
        self.set_graces(self.data.graces().unwrap_or_default());
        self.last_read = Some(Instant::now());
    }

    fn set_graces(&mut self, graces: Vec<Grace>) {
        let previous = self.current_grace.and_then(|i| self.graces.get(i)).map(|g| g.id);
        self.pinyin = graces
            .iter()
            .map(|grace| GracePinyinEntry {
                region: Segment::from_name(&grace.region),
                location: Segment::from_name(&grace.name),
            })
            .collect();
        self.graces = graces;
        self.current_grace = previous.and_then(|id| self.graces.iter().position(|g| g.id == id));
        self.filter();
    }

    fn filter(&mut self) {
        if self.filter_string.is_empty() {
            self.filtered = (0..self.graces.len()).collect();
        } else {
            let query = &self.filter_string;
            let mut scored: Vec<(usize, u32)> = self
                .graces
                .iter()
                .enumerate()
                .filter_map(|(index, grace)| {
                    let pinyin = &self.pinyin[index];
                    if pinyin_match_start(query, &pinyin.location)
                        || string_match(query, &grace.name)
                    {
                        Some((index, 2))
                    } else if pinyin_match(query, &pinyin.region)
                        || string_match(query, &grace.region)
                        || pinyin_match(query, &pinyin.location)
                        || string_match(query, &grace.label)
                    {
                        Some((index, 1))
                    } else {
                        None
                    }
                })
                .collect();
            scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));
            self.filtered = scored.into_iter().map(|(index, _)| index).collect();
        }
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
        info!("传送到 {} (实体 {})", grace.name, grace.entity_id);
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
        if ui.button_with_size("传送到赐福点", [BUTTON_WIDTH * scale, button_height]) {
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
            if filter_input(ui, "##warp-filter", "过滤器...", &mut self.filter_string) {
                self.filter();
            }
            let preview = self
                .current_grace
                .and_then(|i| self.graces.get(i))
                .map(|g| g.label.as_str())
                .unwrap_or("没有匹配的赐福");
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
                ui.text("等待游戏数据...");
            }
            {
                let _disabled = ui
                    .begin_disabled(self.current_grace.is_none() || self.sentinel.get().is_none());
                if ui.button_with_size("传送", [400., button_height]) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn grace(id: u32, region: &str, name: &str) -> Grace {
        Grace {
            id,
            entity_id: id,
            name: name.into(),
            region: region.into(),
            label: format!("[{region}] {name}"),
        }
    }

    fn warp(graces: Vec<Grace>) -> Warp {
        let argument = Box::leak(Box::new(PointerChain::new(&[0])));
        let sentinel = Box::leak(Box::new(Bitflag::new(PointerChain::new(&[0]), 1)));
        let addresses = base_addresses::BaseAddresses::from(Version::V2_07_2);
        let mut warp = Warp::new(
            0,
            argument,
            argument,
            GameData::new(&addresses),
            sentinel,
            Key::try_from("escape").unwrap(),
        );
        warp.set_graces(graces);
        warp
    }

    #[test]
    fn location_matches_precede_region_matches_and_keep_runtime_order_for_ties() {
        let mut warp = warp(vec![
            grace(1, "匕首", "山洞"),
            grace(2, "宁姆格福", "匕首山洞"),
            grace(3, "盖利德", "匕首洞窟"),
            grace(4, "湖之利耶尼亚", "古老匕首"),
        ]);
        warp.filter_string = "bs".into();
        warp.filter();

        assert_eq!(warp.filtered, vec![1, 2, 0, 3]);
        assert_eq!(warp.current_grace, Some(0));
        warp.current_grace = None;
        warp.filter();
        assert_eq!(warp.current_grace, Some(1));
    }

    #[test]
    fn clearing_query_restores_runtime_order_and_empty_matches_clear_selection() {
        let mut warp = warp(vec![grace(1, "宁姆格福", "山洞"), grace(2, "宁姆格福", "匕首")]);
        warp.filter_string = "bs".into();
        warp.filter();
        assert_eq!(warp.filtered, vec![1]);
        assert_eq!(warp.current_grace, Some(1));

        warp.filter_string.clear();
        warp.filter();
        assert_eq!(warp.filtered, vec![0, 1]);
        assert_eq!(warp.current_grace, Some(1));

        warp.filter_string = "xyz".into();
        warp.filter();
        assert!(warp.filtered.is_empty());
        assert_eq!(warp.current_grace, None);

        warp.filter_string.clear();
        warp.filter();
        assert_eq!(warp.filtered, vec![0, 1]);
        assert_eq!(warp.current_grace, Some(0));
    }

    #[test]
    fn replacing_catalog_rebuilds_pinyin_and_preserves_selection_by_id() {
        let mut warp = warp(vec![grace(1, "宁姆格福", "匕首"), grace(2, "宁姆格福", "长弓")]);
        warp.filter_string = "bs".into();
        warp.filter();

        warp.set_graces(vec![grace(2, "宁姆格福", "长弓"), grace(1, "宁姆格福", "匕首")]);
        assert_eq!(warp.filtered, vec![1]);
        assert_eq!(warp.current_grace, Some(1));

        warp.set_graces(vec![grace(2, "宁姆格福", "大赐福"), grace(1, "宁姆格福", "山洞")]);
        assert!(warp.filtered.is_empty());
        assert_eq!(warp.current_grace, None);

        warp.filter_string = "ci".into();
        warp.filter();
        assert_eq!(warp.filtered, vec![0]);
        assert_eq!(warp.current_grace, Some(0));

        warp.set_graces(Vec::new());
        assert!(warp.pinyin.is_empty());
        assert!(warp.filtered.is_empty());
        assert_eq!(warp.current_grace, None);
    }
}
