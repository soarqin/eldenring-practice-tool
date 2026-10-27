use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use super::layout::{accessory, gem, goods, menu_property, menu_value, protector, shop, weapon};
use super::memory::{at, ParamTables};
use super::messages::{Messages, NameKind};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ItemCategory {
    Weapons,
    Ammunition,
    Head,
    Chest,
    Arms,
    Legs,
    Talismans,
    Sorceries,
    Incantations,
    AshesOfWar,
    SpiritAshes,
    Consumables,
    Materials,
    Miscellaneous,
    Unused,
}

impl ItemCategory {
    pub const ALL: [Self; 15] = [
        Self::Weapons,
        Self::Ammunition,
        Self::Head,
        Self::Chest,
        Self::Arms,
        Self::Legs,
        Self::Talismans,
        Self::Sorceries,
        Self::Incantations,
        Self::AshesOfWar,
        Self::SpiritAshes,
        Self::Consumables,
        Self::Materials,
        Self::Miscellaneous,
        Self::Unused,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Weapons => "Weapons",
            Self::Ammunition => "Ammunition",
            Self::Head => "Armor: Head",
            Self::Chest => "Armor: Chest",
            Self::Arms => "Armor: Arms",
            Self::Legs => "Armor: Legs",
            Self::Talismans => "Talismans",
            Self::Sorceries => "Sorceries",
            Self::Incantations => "Incantations",
            Self::AshesOfWar => "Ashes of War",
            Self::SpiritAshes => "Spirit Ashes",
            Self::Consumables => "Consumables",
            Self::Materials => "Materials",
            Self::Miscellaneous => "Miscellaneous / Key Items",
            Self::Unused => "Unused / Cut Content",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ItemVariant {
    pub affinity: u32,
    pub max_upgrade: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeaponType {
    /// The first MenuValueTableParam row for this displayed type. Shared text
    /// IDs (for example, the two fist types) belong to the same group.
    pub id: u32,
    pub name: String,
}

#[derive(Debug)]
pub struct Item {
    /// Includes the game's inventory category bits.
    pub id: u32,
    pub name: String,
    pub category: ItemCategory,
    pub weapon_type: Option<WeaponType>,
    pub dlc: bool,
    pub max_quantity: u32,
    pub variants: Vec<ItemVariant>,
    sort: (u8, i32),
}

impl Item {
    pub fn spawn_id(&self, variant: usize, upgrade: u32) -> Option<u32> {
        let variant = self.variants.get(variant)?;
        if upgrade > variant.max_upgrade {
            return None;
        }
        self.id.checked_add(variant.affinity)?.checked_add(upgrade)
    }

    fn sort_key(&self) -> (ItemCategory, i32, i32, u32) {
        let (group, order) = self.sort;
        let (first, second) =
            if matches!(self.category, ItemCategory::Weapons | ItemCategory::Ammunition) {
                // Weapon sortGroupId is reused by melee weapons, bows, and
                // shields. sortId places these sections in inventory order.
                (order, i32::from(group))
            } else {
                (i32::from(group), order)
            };
        (self.category, first, second, self.id)
    }
}

fn goods_category(
    id: u32,
    kind: u8,
    group: u8,
    consume: bool,
    offline_disabled: bool,
) -> Option<ItemCategory> {
    use ItemCategory::*;
    // Glorious Merchant's auto-granted, gesture and tutorial exclusions.
    if matches!(id, 107 | 113 | 114 | 115 | 135 | 251 | 1000..=1025 | 1050..=1075)
        || (kind == 0 && group == 250)
        || (kind == 12 && group == 20)
        || (matches!(kind, 7 | 8) && !id.is_multiple_of(100))
    {
        return None;
    }
    Some(if matches!(id, 10010 | 10020) {
        Consumables
    } else {
        match kind {
            0 if consume && !offline_disabled => Consumables,
            0 | 1 | 9 | 10 | 12 | 15 => Miscellaneous,
            2 | 14 => Materials,
            3 | 11 | 100 | 101 => Consumables,
            5 | 17 => Sorceries,
            16 | 18 => Incantations,
            7 | 8 => SpiritAshes,
            _ => return None,
        }
    })
}

fn unused_name(name: &str) -> bool {
    name.starts_with("[ERROR]") || name.starts_with("[ERR]")
}

fn weapon_types(
    rows: impl Iterator<Item = (u32, Vec<u8>)>,
    range: Range<u32>,
    mut name: impl FnMut(u32) -> Option<String>,
) -> Option<BTreeMap<u16, WeaponType>> {
    let mut types = BTreeMap::new();
    let mut shared = BTreeMap::<u32, WeaponType>::new();
    // Canonical IDs for shared labels must not depend on file row order.
    // Inventory order comes from EquipParamWeapon.sortId.
    let rows: BTreeMap<_, _> = rows.filter(|(id, _)| range.contains(id)).collect();
    for (id, row) in rows {
        if at::<i8>(&row, menu_value::COMPARE)? != 0 {
            continue;
        }
        let Ok(kind) = u16::try_from(at::<i32>(&row, menu_value::VALUE)?) else {
            continue;
        };
        let Ok(text) = u32::try_from(at::<i32>(&row, menu_value::TEXT)?) else {
            continue;
        };
        let group = shared.entry(text).or_insert_with(|| WeaponType {
            id,
            name: name(text)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("Weapon type {kind}")),
        });
        types.entry(kind).or_insert_with(|| group.clone());
    }
    Some(types)
}

fn read_weapon_types(
    params: &ParamTables,
    messages: &Messages,
) -> Option<BTreeMap<u16, WeaponType>> {
    let properties = params.get("MenuPropertySpecParam")?;
    let row = properties.row(menu_property::WEAPON_TYPE, menu_property::SIZE)?;
    let first = u32::from(at::<u16>(&row, menu_property::FORMAT)?);
    let next = properties
        .rows(menu_property::SIZE)
        .filter_map(|(_, row)| Some(u32::from(at::<u16>(&row, menu_property::FORMAT)?)))
        .filter(|&format| format > first)
        .min()
        .unwrap_or(u32::MAX);
    weapon_types(params.get("MenuValueTableParam")?.rows(menu_value::SIZE), first..next, |id| {
        messages.name(NameKind::Menu, id).map(|(name, _)| name)
    })
}

pub(super) fn read(params: &ParamTables, messages: &Messages) -> Option<Vec<Item>> {
    let weapons = params.get("EquipParamWeapon")?;
    let goods = params.get("EquipParamGoods")?;
    let reinforcements = params.get("ReinforceParamWeapon")?;
    let weapon_types = read_weapon_types(params, messages)?;
    let mut max_levels = BTreeMap::<i16, u32>::new();
    for id in reinforcements.ids() {
        let level = id % 50;
        if let Ok(path) = i16::try_from(id - level) {
            max_levels.entry(path).and_modify(|max| *max = (*max).max(level)).or_insert(level);
        }
    }

    let mut weapon_variants = BTreeMap::<u32, Vec<ItemVariant>>::new();
    for (id, row) in weapons.rows(weapon::SIZE) {
        if !id.is_multiple_of(100) {
            continue;
        }
        let path: i16 = at(&row, weapon::REINFORCE)?;
        let kind: u16 = at(&row, weapon::TYPE)?;
        weapon_variants.entry(id - id % 10000).or_default().push(ItemVariant {
            affinity: id % 10000,
            max_upgrade: if matches!(kind, 81 | 83 | 85 | 86) {
                0
            } else {
                *max_levels.get(&path).unwrap_or(&0)
            },
        });
    }

    let mut items = Vec::new();
    for id in weapons.ids().filter(|id| id.is_multiple_of(10000) && *id != 110000) {
        let row = weapons.row(id, weapon::SIZE)?;
        let (name, dlc) = messages.name(NameKind::Weapon, id).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let kind: u16 = at(&row, weapon::TYPE)?;
        let unused = unused_name(&name) || id == 33290000;
        let category = if unused {
            ItemCategory::Unused
        } else if matches!(kind, 81 | 83 | 85 | 86) {
            ItemCategory::Ammunition
        } else {
            ItemCategory::Weapons
        };
        let mut variants = weapon_variants.remove(&id).unwrap_or_default();
        variants.sort_by_key(|v| v.affinity);
        if variants.is_empty() {
            continue;
        }
        items.push(Item {
            id,
            name,
            dlc,
            category,
            weapon_type: Some(weapon_types.get(&kind).cloned().unwrap_or_else(|| WeaponType {
                // Keep weapons with unknown/modded types visible. PARAM row
                // IDs in the type table precede the next format group.
                id: 0x1_0000 + u32::from(kind),
                name: format!("Weapon type {kind}"),
            })),
            variants,
            max_quantity: 99,
            sort: (at(&row, weapon::SORT_GROUP)?, at(&row, weapon::SORT)?),
        });
    }

    for (id, row) in params.get("EquipParamProtector")?.rows(protector::SIZE) {
        if matches!(id, 10000 | 10100 | 10200 | 10300) {
            continue;
        }
        let kind: u8 = at(&row, protector::TYPE)?;
        let category = match kind {
            0 => ItemCategory::Head,
            1 => ItemCategory::Chest,
            2 => ItemCategory::Arms,
            3 => ItemCategory::Legs,
            _ => continue,
        };
        let (name, dlc) = messages.name(NameKind::Protector, id).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let unused = unused_name(&name)
            || matches!(
                id,
                610000
                    | 610100
                    | 610200
                    | 610300
                    | 611000
                    | 611100
                    | 955000
                    | 955100
                    | 955200
                    | 955300
                    | 956100
            );
        items.push(Item {
            id: id | 0x10000000,
            name,
            dlc,
            category: if unused { ItemCategory::Unused } else { category },
            weapon_type: None,
            max_quantity: 99,
            variants: vec![ItemVariant { affinity: 0, max_upgrade: 0 }],
            sort: (at(&row, protector::SORT_GROUP)?, at(&row, protector::SORT)?),
        });
    }

    for (id, row) in params.get("EquipParamAccessory")?.rows(accessory::SIZE) {
        let (name, dlc) = messages.name(NameKind::Accessory, id).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let unused = unused_name(&name) || matches!(id, 3100 | 6120 | 6121);
        items.push(Item {
            id: id | 0x20000000,
            name,
            dlc,
            category: if unused { ItemCategory::Unused } else { ItemCategory::Talismans },
            weapon_type: None,
            max_quantity: 99,
            variants: vec![ItemVariant { affinity: 0, max_upgrade: 0 }],
            sort: (at(&row, accessory::SORT_GROUP)?, at(&row, accessory::SORT)?),
        });
    }

    let mut dummy_goods = BTreeSet::<i32>::new();
    for (_, row) in params.get("ShopLineupParam")?.rows(shop::SIZE) {
        let id: i32 = at(&row, shop::NAME_REPLACEMENT)?;
        if id >= 0 {
            dummy_goods.insert(id);
        }
    }
    for (_, row) in goods.rows(goods::SIZE) {
        let id: i32 = at(&row, goods::APPEARANCE_REPLACEMENT)?;
        if id >= 0 {
            dummy_goods.insert(id);
        }
    }
    let goods_ids: BTreeSet<_> = goods.ids().collect();
    for (id, row) in goods.rows(goods::SIZE) {
        if dummy_goods.contains(&(id as i32)) {
            continue;
        }
        let kind: u8 = at(&row, goods::TYPE)?;
        let group: u8 = at(&row, goods::SORT_GROUP)?;
        let flags: u8 = at(&row, goods::FLAGS)?;
        let Some(category) = goods_category(
            id,
            kind,
            group,
            flags & goods::CONSUME_MASK != 0,
            flags & goods::DISABLE_OFFLINE_MASK != 0,
        ) else {
            continue;
        };
        let (name, dlc) = messages.name(NameKind::Goods, id).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let icon: u16 = at(&row, goods::ICON)?;
        let unused = unused_name(&name) || icon == 0 || matches!(id, 8860 | 8861 | 2008023);
        let max_upgrade = if matches!(kind, 7 | 8) {
            (1..100).take_while(|level| goods_ids.contains(&(id + level))).last().unwrap_or(0)
        } else {
            0
        };
        let max_quantity: i16 = at(&row, goods::MAX_QUANTITY)?;
        items.push(Item {
            id: id | 0x40000000,
            name,
            dlc,
            category: if unused { ItemCategory::Unused } else { category },
            weapon_type: None,
            max_quantity: (max_quantity as i32).clamp(1, 999) as u32,
            variants: vec![ItemVariant { affinity: 0, max_upgrade }],
            sort: (group, at(&row, goods::SORT)?),
        });
    }

    for (id, row) in params.get("EquipParamGem")?.rows(gem::SIZE) {
        let (name, dlc) = messages.name(NameKind::Gem, id).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let unused = unused_name(&name);
        items.push(Item {
            id: id | 0x80000000,
            name,
            dlc,
            category: if unused { ItemCategory::Unused } else { ItemCategory::AshesOfWar },
            weapon_type: None,
            max_quantity: 99,
            variants: vec![ItemVariant { affinity: 0, max_upgrade: 0 }],
            sort: (at(&row, gem::SORT_GROUP)?, at(&row, gem::SORT)?),
        });
    }
    items.sort_by_key(Item::sort_key);
    (!items.is_empty()).then_some(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weapons_follow_inventory_order_across_reused_sort_groups() {
        let mut items: Vec<_> = [
            (67500000, (25, 1150500)), // Light greatsword, DLC.
            (40000000, (10, 5000000)), // Light bow reuses the dagger group.
            (24000000, (10, 7000000)), // Torch also reuses that group.
            (3500000, (60, 1504000)),  // Heavy thrusting sword.
            (2080000, (70, 1612000)),  // Curved sword.
            (2000000, (20, 1101000)),  // Straight sword.
            (1000000, (10, 1000000)),  // Dagger.
            (63500000, (15, 1050000)), // Throwing blade, DLC.
        ]
        .into_iter()
        .map(|(id, sort)| Item {
            id,
            name: String::new(),
            category: ItemCategory::Weapons,
            weapon_type: None,
            dlc: false,
            max_quantity: 99,
            variants: Vec::new(),
            sort,
        })
        .collect();
        items.sort_by_key(Item::sort_key);
        assert_eq!(items.iter().map(|item| item.id).collect::<Vec<_>>(), [
            1000000, 63500000, 2000000, 67500000, 3500000, 2080000, 40000000, 24000000,
        ]);
    }

    #[test]
    fn weapon_types_follow_the_menu_table_and_merge_shared_labels() {
        let row = |id, kind: i32, text: i32| {
            let mut data = vec![0; menu_value::SIZE];
            data[0..4].copy_from_slice(&kind.to_le_bytes());
            data[4..8].copy_from_slice(&text.to_le_bytes());
            (id, data)
        };
        let types = weapon_types(
            [
                row(300, 1, 80000), // Another menu format has the same value.
                row(141, 35, 60090),
                row(177, 95, 60187), // DLC type, not part of the old name list.
                row(120, 1, 60010),
                row(140, 33, 60090),
                row(121, 3, 60015),
                row(110, 0, 11320),
            ]
            .into_iter(),
            120..300,
            |id| match id {
                60010 => Some("短剑".into()),
                60015 => Some("直剑".into()),
                60090 => Some("拳头".into()),
                60187 => Some("獣爪".into()),
                _ => None,
            },
        )
        .unwrap();
        assert_eq!(types[&1], WeaponType { id: 120, name: "短剑".into() });
        assert_eq!(types[&3], WeaponType { id: 121, name: "直剑".into() });
        assert_eq!(types[&33], types[&35]);
        assert_eq!(types[&35], WeaponType { id: 140, name: "拳头".into() });
        assert_eq!(types[&95], WeaponType { id: 177, name: "獣爪".into() });
        assert_eq!(types.len(), 5);
    }

    #[test]
    fn merchant_filters_and_goods_categories() {
        assert_eq!(goods_category(107, 0, 0, true, false), None);
        assert_eq!(goods_category(1005, 0, 0, true, false), None);
        assert_eq!(goods_category(500, 0, 250, false, false), None);
        assert_eq!(goods_category(500, 12, 20, false, false), None);
        assert_eq!(goods_category(200001, 7, 0, false, false), None);
        assert_eq!(goods_category(200000, 7, 0, false, false), Some(ItemCategory::SpiritAshes));
        assert_eq!(goods_category(10010, 14, 0, false, false), Some(ItemCategory::Consumables));
        assert_eq!(goods_category(500, 5, 0, false, false), Some(ItemCategory::Sorceries));
        assert_eq!(goods_category(500, 18, 0, false, false), Some(ItemCategory::Incantations));
    }

    #[test]
    fn spawn_ids_only_accept_loaded_variants_and_upgrade_limits() {
        let item = Item {
            id: 1000000,
            name: "长剑".into(),
            category: ItemCategory::Weapons,
            weapon_type: None,
            dlc: false,
            max_quantity: 99,
            sort: (0, 0),
            variants: vec![ItemVariant { affinity: 0, max_upgrade: 25 }, ItemVariant {
                affinity: 100,
                max_upgrade: 10,
            }],
        };
        assert_eq!(item.spawn_id(1, 10), Some(1000110));
        assert_eq!(item.spawn_id(1, 11), None);
        assert_eq!(item.spawn_id(2, 0), None);
    }
}
