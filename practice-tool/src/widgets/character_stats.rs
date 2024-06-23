use libeldenring::prelude::*;
use practice_tool_core::key::Key;
use practice_tool_core::widgets::stats_editor::{Datum, Stats, StatsEditor};
use practice_tool_core::widgets::Widget;

#[derive(Debug)]
struct CharacterStatsEdit {
    ptr: PointerChain<CharacterStats>,
    stats: Option<CharacterStats>,
}

impl Stats for CharacterStatsEdit {
    fn data(&mut self) -> Option<impl Iterator<Item = Datum>> {
        self.stats.as_mut().map(|s| {
            [
                Datum::int("等级", &mut s.level, 1, 713),
                Datum::int("生命力", &mut s.vigor, 1, 99),
                Datum::int("集中力", &mut s.mind, 1, 99),
                Datum::int("耐力", &mut s.endurance, 1, 99),
                Datum::int("力气", &mut s.strength, 1, 99),
                Datum::int("灵巧", &mut s.dexterity, 1, 99),
                Datum::int("智力", &mut s.intelligence, 1, 99),
                Datum::int("信仰", &mut s.faith, 1, 99),
                Datum::int("感应", &mut s.arcane, 1, 99),
                Datum::int("卢恩", &mut s.runes, 0, i32::MAX),
            ]
            .into_iter()
        })
    }

    fn read(&mut self) {
        self.stats = self.ptr.read();
    }

    fn write(&mut self) {
        if let Some(stats) = self.stats.clone() {
            self.ptr.write(stats);
        }
    }

    fn clear(&mut self) {
        self.stats = None;
    }
}

pub(crate) fn character_stats_edit(
    character_stats: PointerChain<CharacterStats>,
    key_open: Option<Key>,
    key_close: Key,
) -> Box<dyn Widget> {
    Box::new(StatsEditor::new(
        CharacterStatsEdit { ptr: character_stats, stats: None },
        key_open,
        Some(key_close),
    ))
}
