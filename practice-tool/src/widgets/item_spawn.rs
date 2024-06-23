use std::collections::BTreeMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use imgui::sys::{igGetCursorPosX, igGetCursorPosY, igGetWindowPos, igSetNextWindowPos, ImVec2};
use imgui::{Condition, TreeNodeFlags, WindowFlags};
use libeldenring::prelude::*;
use practice_tool_core::crossbeam_channel::Sender;
use practice_tool_core::key::Key;
use practice_tool_core::widgets::{scaling_factor, Widget, BUTTON_HEIGHT, BUTTON_WIDTH};

use super::font_text::{display_text, filter_input};
use super::string_match;
use crate::fonts::request_text;

static AFFINITIES: [(u32, &str); 13] = [
    (0, "无质变"),
    (100, "厚重"),
    (200, "锋利"),
    (300, "优质"),
    (400, "火焰"),
    (500, "焰术"),
    (600, "雷电"),
    (700, "神圣"),
    (800, "魔力"),
    (900, "寒冷"),
    (1000, "毒"),
    (1100, "血"),
    (1200, "神秘"),
];

const ISP_TAG: &str = "##item-spawn";

#[derive(Debug)]
pub(crate) struct ItemSpawner {
    func_ptr: usize,
    map_item_man: usize,
    data: GameData,
    hotkey_load: Option<Key>,
    hotkey_close: Key,
    sentinel: &'static Bitflag<u8>,
    label_load: String,
    label_close: String,
    qty: u32,
    current: Option<usize>,
    upgrade: u32,
    variant: usize,
    filter_string: String,
    source_filter: usize,
    include_unused: bool,
    logs: Vec<String>,
    items: Vec<Item>,
    filtered: Vec<usize>,
    last_read: Option<Instant>,
}

impl ItemSpawner {
    pub(crate) fn new(
        func_ptr: usize,
        map_item_man: usize,
        data: GameData,
        sentinel: &'static Bitflag<u8>,
        hotkey_load: Option<Key>,
        hotkey_close: Key,
    ) -> Self {
        Self {
            func_ptr,
            map_item_man,
            data,
            hotkey_load,
            hotkey_close,
            sentinel,
            label_load: hotkey_load
                .map(|k| format!("生成物品 ({k})"))
                .unwrap_or_else(|| "生成物品".into()),
            label_close: format!("关闭 ({hotkey_close})"),
            qty: 1,
            current: None,
            upgrade: 0,
            variant: 0,
            filter_string: String::new(),
            source_filter: 0,
            include_unused: false,
            logs: Vec::new(),
            items: Vec::new(),
            filtered: Vec::new(),
            last_read: None,
        }
    }

    fn refresh(&mut self) {
        let previous = self.current.and_then(|i| self.items.get(i)).map(|item| item.id);
        self.items = self.data.items().unwrap_or_default();
        self.current = previous.and_then(|id| self.items.iter().position(|item| item.id == id));
        self.last_read = Some(Instant::now());
        self.filter();
        self.clamp_options();
    }

    fn filter(&mut self) {
        let filter = self.filter_string.trim();
        let decimal = filter.parse::<u32>().ok();
        let hexadecimal = filter
            .strip_prefix("0x")
            .or_else(|| filter.strip_prefix("0X"))
            .or_else(|| (filter.len() == 8).then_some(filter))
            .and_then(|id| u32::from_str_radix(id, 16).ok());
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                (self.include_unused || item.category != ItemCategory::Unused)
                    && (self.source_filter == 0 || item.dlc == (self.source_filter == 2))
                    && (string_match(filter, &item.name)
                        || [decimal, hexadecimal]
                            .into_iter()
                            .flatten()
                            .any(|id| id == item.id || id == item.id & 0x0fff_ffff))
            })
            .map(|(index, _)| index)
            .collect();
        if self.current.is_some_and(|index| !self.filtered.contains(&index)) {
            self.select(None);
        }
    }

    fn select(&mut self, index: Option<usize>) {
        if self.current != index {
            self.current = index;
            self.variant = 0;
            self.upgrade = 0;
            self.clamp_options();
        }
    }

    fn clear(&mut self) {
        self.filter_string.clear();
        self.source_filter = 0;
        self.include_unused = false;
        self.qty = 1;
        self.current = None;
        self.variant = 0;
        self.upgrade = 0;
        self.filter();
    }

    fn clamp_options(&mut self) {
        if let Some(item) = self.current.and_then(|index| self.items.get(index)) {
            self.variant = self.variant.min(item.variants.len().saturating_sub(1));
            self.upgrade = self
                .upgrade
                .min(item.variants.get(self.variant).map(|v| v.max_upgrade).unwrap_or(0));
            self.qty = self.qty.clamp(1, item.max_quantity);
        } else {
            self.variant = 0;
            self.upgrade = 0;
        }
    }

    fn render_item(&self, ui: &imgui::Ui, index: usize, selection: &mut Option<usize>) {
        let item = &self.items[index];
        let _id = ui.push_id_int(item.id as i32);
        let label = format!("{}###row", display_text(ui, &item.name));
        if ui.selectable_config(&label).selected(self.current == Some(index)).build() {
            *selection = Some(index);
        }
        if ui.is_item_visible() {
            request_text(ui, &item.name);
        }
    }

    fn weapon_groups(&self) -> Vec<(u32, &str, Vec<usize>)> {
        let mut groups = BTreeMap::<u32, (&str, Vec<usize>)>::new();
        for &index in &self.filtered {
            let item = &self.items[index];
            if item.category != ItemCategory::Weapons {
                continue;
            }
            let kind = item.weapon_type.as_ref();
            let id = kind.map(|kind| kind.id).unwrap_or(0);
            let name = kind.map(|kind| kind.name.as_str()).unwrap_or("其他武器");
            groups.entry(id).or_insert_with(|| (name, Vec::new())).1.push(index);
        }
        // Use the complete catalog's PARAM order so a partial filter cannot
        // change the order of the remaining weapon types.
        let mut ordered = Vec::with_capacity(groups.len());
        for item in &self.items {
            if item.category != ItemCategory::Weapons {
                continue;
            }
            let id = item.weapon_type.as_ref().map(|kind| kind.id).unwrap_or(0);
            if let Some((name, indices)) = groups.remove(&id) {
                ordered.push((id, name, indices));
            }
        }
        ordered
    }

    fn spawn(&mut self) {
        if self.sentinel.get().is_none() {
            self.logs.push("尚未进入游戏，无法生成物品".into());
            return;
        }
        if self.items.is_empty() {
            self.refresh();
        }
        self.clamp_options();
        let Some(item) = self.current.and_then(|index| self.items.get(index)) else {
            self.logs.push("未选择物品".into());
            return;
        };
        let Some(item_id) = item.spawn_id(self.variant, self.upgrade) else {
            self.logs.push("无效的强化等级或质变".into());
            return;
        };
        let Some(manager) =
            PointerChain::<usize>::new(&[self.map_item_man]).read().filter(|p| *p != 0)
        else {
            self.logs.push("物品管理器尚未加载".into());
            return;
        };
        self.logs.push(format!(
            "生成 {} x {} +{} (#{:08X})",
            self.qty, item.name, self.upgrade, item_id
        ));
        unsafe {
            spawn_item(self.func_ptr, manager, item_id, self.qty);
        }
    }
}

impl Widget for ItemSpawner {
    fn render(&mut self, ui: &imgui::Ui) {
        let scale = scaling_factor(ui);
        let button_height = BUTTON_HEIGHT;
        let (x, y) = unsafe {
            let mut wnd_pos = ImVec2::default();
            igGetWindowPos(&mut wnd_pos);
            (igGetCursorPosX() + wnd_pos.x, igGetCursorPosY() + wnd_pos.y)
        };
        if ui.button_with_size(&self.label_load, [BUTTON_WIDTH * scale, button_height]) {
            self.refresh();
            ui.open_popup(ISP_TAG);
        }
        unsafe {
            igSetNextWindowPos(
                ImVec2::new(x + 200. * scale, y),
                Condition::Always as i8 as _,
                ImVec2::new(0., 0.),
            )
        };
        if let Some(_token) = ui
            .modal_popup_config(ISP_TAG)
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_RESIZE
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_SCROLLBAR
                    | WindowFlags::ALWAYS_AUTO_RESIZE,
            )
            .begin_popup()
        {
            let button_height = button_height * scale;
            let _width = ui.push_item_width(400.);
            if self.items.is_empty()
                && self.last_read.is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
            {
                self.refresh();
            }
            let mut changed = filter_input(
                ui,
                "##item-spawn-filter",
                "筛选名称或 ID...",
                &mut self.filter_string,
            );
            changed |= ui.combo_simple_string("##item-source", &mut self.source_filter, &[
                "所有物品",
                "游戏本体",
                "DLC",
            ]);
            changed |= ui.checkbox("显示未使用/删减内容", &mut self.include_unused);
            ui.same_line();
            if ui.button("刷新") {
                self.refresh();
            }
            if changed {
                self.filter();
            }
            let mut selection = self.current;
            ui.child_window("##item-spawn-list").size([400., 220.]).build(|| {
                for category in ItemCategory::ALL {
                    if !self.filtered.iter().any(|&index| self.items[index].category == category) {
                        continue;
                    }
                    let node = ui.tree_node_config(category.label());
                    let node = if self.filter_string.is_empty() {
                        node
                    } else {
                        node.opened(true, Condition::Always)
                    };
                    node.flags(TreeNodeFlags::SPAN_AVAIL_WIDTH).build(|| {
                        let indices = self
                            .filtered
                            .iter()
                            .copied()
                            .filter(|&index| self.items[index].category == category);
                        if category == ItemCategory::Weapons {
                            for (id, name, indices) in self.weapon_groups() {
                                let _id = ui.push_id_int(id as i32);
                                let node = ui
                                    .tree_node_config("group")
                                    .label::<&str, _>(display_text(ui, name));
                                let node = if self.filter_string.is_empty() {
                                    node
                                } else {
                                    node.opened(true, Condition::Always)
                                };
                                let token = node.flags(TreeNodeFlags::SPAN_AVAIL_WIDTH).push();
                                if ui.is_item_visible() {
                                    request_text(ui, name);
                                }
                                if let Some(_token) = token {
                                    for index in indices {
                                        self.render_item(ui, index, &mut selection);
                                    }
                                }
                            }
                        } else {
                            for index in indices {
                                self.render_item(ui, index, &mut selection);
                            }
                        }
                    });
                }
                if self.items.is_empty() {
                    ui.text("等待游戏数据...");
                } else if self.filtered.is_empty() {
                    ui.text("没有匹配的物品");
                }
            });
            self.select(selection);
            let item = self.current.and_then(|index| self.items.get(index));
            let labels: Vec<String> = item
                .map(|item| {
                    item.variants
                        .iter()
                        .map(|v| {
                            AFFINITIES
                                .iter()
                                .find(|(offset, _)| *offset == v.affinity)
                                .map(|(_, name)| (*name).to_owned())
                                .unwrap_or_else(|| format!("质变 {}", v.affinity))
                        })
                        .collect()
                })
                .unwrap_or_else(|| vec![AFFINITIES[0].1.into()]);
            {
                let _disabled = ui.begin_disabled(labels.len() <= 1);
                ui.set_next_item_width(195.);
                if ui.combo_simple_string("##item-spawn-affinity", &mut self.variant, &labels) {
                    self.upgrade = self.upgrade.min(
                        item.and_then(|item| item.variants.get(self.variant))
                            .map(|variant| variant.max_upgrade)
                            .unwrap_or(0),
                    );
                }
            }
            ui.same_line();
            let max_upgrade = item
                .and_then(|item| item.variants.get(self.variant))
                .map(|variant| variant.max_upgrade)
                .unwrap_or(0);
            {
                let _disabled = ui.begin_disabled(max_upgrade == 0);
                let upgrades: Vec<_> = (0..=max_upgrade).map(|level| format!("+{level}")).collect();
                let mut upgrade = self.upgrade as usize;
                ui.set_next_item_width(195.);
                if ui.combo_simple_string("##item-spawn-upgrade", &mut upgrade, &upgrades) {
                    self.upgrade = upgrade as u32;
                }
            }
            {
                let _disabled = ui.begin_disabled(item.is_none());
                let _width = ui.push_item_width(0.);
                ui.slider_config("数量", 1, item.map(|item| item.max_quantity).unwrap_or(99))
                    .build(&mut self.qty);
            }
            {
                let _disabled =
                    ui.begin_disabled(self.current.is_none() || self.sentinel.get().is_none());
                if ui.button_with_size(&self.label_load, [400., button_height]) {
                    self.spawn();
                }
            }
            if ui.button_with_size("清除", [400., button_height]) {
                self.clear();
            }
            if ui.button_with_size(&self.label_close, [400., button_height])
                || (self.hotkey_close.is_pressed(ui)
                    && !(ui.io().want_capture_keyboard && ui.is_any_item_active()))
            {
                ui.close_current_popup();
            }
        }
    }

    fn interact(&mut self, ui: &imgui::Ui) {
        if self.hotkey_load.is_some_and(|key| key.is_pressed(ui)) {
            self.spawn();
        }
    }

    fn log(&mut self, tx: &Sender<String>) {
        for log in self.logs.drain(..) {
            tx.send(log).ok();
        }
    }
}

unsafe fn spawn_item(function: usize, manager: usize, item_id: u32, qty: u32) {
    #[repr(C)]
    struct SpawnRequest {
        one: u32,
        item_id: u32,
        qty: u32,
        dur: i32,
        gem: i32,
    }

    type SpawnItemFn = extern "system" fn(*const c_void, *mut SpawnRequest, *mut u32, u32);
    let spawn_fn_ptr = std::mem::transmute::<usize, SpawnItemFn>(function);
    let mut spawn_request = SpawnRequest { one: 1, item_id, qty, dur: -1, gem: -1 };
    let mut output = 0u32;

    spawn_fn_ptr(manager as *const c_void, &mut spawn_request, &mut output, 0);
}
