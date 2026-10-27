use std::collections::{BTreeMap, BTreeSet};

use super::memory::{at, bytes, value};

#[derive(Clone, Copy)]
pub(super) enum NameKind {
    Goods,
    Weapon,
    Protector,
    Accessory,
    Gem,
    Place,
    Npc,
    Menu,
}

impl NameKind {
    fn bundles(self) -> &'static [u32] {
        // Match MsgRepository's own accessors: patch, base game, DLC01, DLC02.
        // These are FMG identifiers, not memory addresses.
        match self {
            Self::Goods => &[111, 10, 319, 419],
            Self::Weapon => &[115, 11, 310, 410],
            Self::Protector => &[117, 12, 313, 413],
            Self::Accessory => &[113, 13, 316, 416],
            Self::Gem => &[35, 322, 422],
            Self::Place => &[120, 19, 329, 429],
            Self::Npc => &[119, 18, 328, 428],
            Self::Menu => &[200, 368, 468],
        }
    }
}

pub(super) struct Messages(BTreeMap<u32, BTreeMap<u32, String>>);

impl Messages {
    pub(super) fn read(repository_address: usize, kinds: &[NameKind]) -> Option<Self> {
        let repository: usize = value(repository_address)?;
        let header = bytes(repository, 0x18)?;
        let languages: usize = at(&header, 8)?;
        let language_count: u32 = at(&header, 0x10)?;
        let bundle_count: u32 = at(&header, 0x14)?;
        if language_count == 0 || language_count > 64 || bundle_count == 0 || bundle_count > 4096 {
            return None;
        }
        // The game's accessors use language slot 0, containing the current
        // locale.
        let bundles: usize = value(languages)?;
        let pointers = bytes(bundles, bundle_count as usize * 8)?;
        let wanted: BTreeSet<_> =
            kinds.iter().flat_map(|kind| kind.bundles().iter().copied()).collect();
        let mut tables = BTreeMap::new();
        for id in wanted {
            let Some(address) = at::<usize>(&pointers, id as usize * 8).filter(|p| *p != 0) else {
                continue;
            };
            if let Some(table) = read_fmg(address) {
                tables.insert(id, table);
            }
        }
        (!tables.is_empty()).then_some(Self(tables))
    }

    pub(super) fn name(&self, kind: NameKind, id: u32) -> Option<(String, bool)> {
        kind.bundles()
            .iter()
            .find_map(|bundle| Some((self.0.get(bundle)?.get(&id)?.clone(), *bundle >= 300)))
    }
}

fn read_fmg(address: usize) -> Option<BTreeMap<u32, String>> {
    let header = bytes(address, 0x28)?;
    let size: u32 = at(&header, 4)?;
    if !(0x28..=16 * 1024 * 1024).contains(&size) || header[0] != 0 || header[2] != 2 {
        return None;
    }
    // FD4 relocates this header field to an absolute pointer when loading FMGs.
    // Individual string offsets remain relative to the beginning of the FMG.
    let offsets: usize = at(&header, 0x18)?;
    let offsets = offsets.checked_sub(address)?;
    parse_fmg(&bytes(address, size as usize)?, offsets)
}

fn parse_fmg(data: &[u8], offsets: usize) -> Option<BTreeMap<u32, String>> {
    let group_count: u32 = at(data, 0xc)?;
    let string_count: u32 = at(data, 0x10)?;
    if group_count > string_count || string_count > 1_000_000 {
        return None;
    }
    let groups = data.get(0x28..0x28usize.checked_add(group_count as usize * 0x10)?)?;
    let pointers = data.get(offsets..offsets.checked_add(string_count as usize * 8)?)?;
    let mut result = BTreeMap::new();
    for group in groups.as_chunks::<0x10>().0 {
        let first_index: u32 = at(group, 0)?;
        let first_id: u32 = at(group, 4)?;
        let last_id: u32 = at(group, 8)?;
        let count = last_id.checked_sub(first_id)?.checked_add(1)?;
        if first_index.checked_add(count)? > string_count {
            return None;
        }
        for offset in 0..count {
            let string_offset: u64 = at(pointers, (first_index + offset) as usize * 8)?;
            if string_offset == 0 {
                continue;
            }
            let string_offset = usize::try_from(string_offset).ok()?;
            let remaining = data.get(string_offset..)?;
            let mut units = Vec::new();
            let mut terminated = false;
            for unit in remaining.as_chunks::<2>().0.iter().take(8192) {
                let unit = u16::from_le_bytes([unit[0], unit[1]]);
                if unit == 0 {
                    terminated = true;
                    break;
                }
                units.push(unit);
            }
            if !terminated {
                return None;
            }
            result.insert(first_id + offset, String::from_utf16_lossy(&units));
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localized_names_follow_patch_and_dlc_priority() {
        let messages = Messages(BTreeMap::from([
            (11, BTreeMap::from([(1000000, "Sword".into()), (1001000, "Black Knife".into())])),
            (115, BTreeMap::from([(1000000, "长剑".into()), (1001000, String::new())])),
            (310, BTreeMap::from([(2000000, "夜の剣".into())])),
        ]));
        assert_eq!(messages.name(NameKind::Weapon, 1000000), Some(("长剑".into(), false)));
        assert_eq!(messages.name(NameKind::Weapon, 2000000), Some(("夜の剣".into(), true)));
        // The game's accessor checks for a non-null string pointer, so an
        // explicitly blank patch overrides the base entry too.
        assert_eq!(messages.name(NameKind::Weapon, 1001000), Some((String::new(), false)));
        assert_eq!(messages.name(NameKind::Weapon, 0), None);
    }

    #[test]
    fn loaded_fmg_uses_absolute_offset_table_and_relative_utf16_strings() {
        let mut fmg = [0u8; 0x54];
        fmg[2] = 2;
        fmg[4..8].copy_from_slice(&0x54u32.to_le_bytes());
        fmg[0xc..0x10].copy_from_slice(&1u32.to_le_bytes());
        fmg[0x10..0x14].copy_from_slice(&1u32.to_le_bytes());
        let offsets = fmg.as_ptr() as usize + 0x38;
        fmg[0x18..0x20].copy_from_slice(&offsets.to_le_bytes());
        fmg[0x2c..0x30].copy_from_slice(&42u32.to_le_bytes());
        fmg[0x30..0x34].copy_from_slice(&42u32.to_le_bytes());
        fmg[0x38..0x40].copy_from_slice(&0x40u64.to_le_bytes());
        let text: Vec<_> = "赐福".encode_utf16().flat_map(u16::to_le_bytes).collect();
        fmg[0x40..0x40 + text.len()].copy_from_slice(&text);
        assert_eq!(read_fmg(fmg.as_ptr() as usize).unwrap().get(&42).unwrap(), "赐福");
        fmg[0x30..0x34].copy_from_slice(&100u32.to_le_bytes());
        assert!(read_fmg(fmg.as_ptr() as usize).is_none());
    }
}
