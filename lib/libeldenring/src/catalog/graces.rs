use std::collections::BTreeMap;

use super::layout::{grace, grace_category, grace_tab};
use super::memory::{at, ParamTables};
use super::messages::{Messages, NameKind};

#[derive(Debug)]
pub struct Grace {
    pub id: u32,
    pub entity_id: u32,
    pub name: String,
    pub region: String,
    pub label: String,
}

fn destination(entity: u32, category: i32) -> Option<u32> {
    // Row 0 is the initial spawn point, which also belongs in this list.
    // Entity IDs in the param already are warp targets.
    (entity != 0 && entity != u32::MAX && category >= 0).then_some(entity)
}

pub(super) fn read(params: &ParamTables, messages: &Messages) -> Option<Vec<Grace>> {
    let tabs: BTreeMap<_, _> = params
        .get("BonfireWarpTabParam")?
        .rows(grace_tab::SIZE)
        .filter_map(|(id, row)| Some((id, at::<i32>(&row, grace_tab::SORT)?)))
        .collect();
    let categories: BTreeMap<_, _> = params
        .get("BonfireWarpSubCategoryParam")?
        .rows(grace_category::SIZE)
        .filter_map(|(id, row)| {
            let text: i32 = at(&row, grace_category::TEXT)?;
            let tab: u16 = at(&row, grace_category::TAB)?;
            let sort: u16 = at(&row, grace_category::SORT)?;
            let name = messages.name(NameKind::Menu, u32::try_from(text).ok()?)?.0;
            Some((id as i32, (name, (*tabs.get(&u32::from(tab)).unwrap_or(&0), sort))))
        })
        .collect();
    let mut graces = Vec::new();
    for (id, row) in params.get("BonfireWarpParam")?.rows(grace::SIZE) {
        let entity: u32 = at(&row, grace::ENTITY)?;
        let category: i32 = at(&row, grace::CATEGORY)?;
        let Some(entity_id) = destination(entity, category) else {
            continue;
        };
        let mut name = None;
        for slot in 0..8 {
            let text: i32 = at(&row, grace::TEXT + slot * 12)?;
            let kind: u8 = at(&row, grace::TEXT_TYPE + slot)?;
            if text < 0 {
                continue;
            }
            let kind = match kind {
                0 => NameKind::Place,
                1 => NameKind::Npc,
                _ => continue,
            };
            if let Some((text, _)) = messages.name(kind, text as u32) {
                if !text.is_empty() {
                    name = Some(text);
                    break;
                }
            }
        }
        // The spawn point can reference a missing FMG entry. Keep it usable,
        // while preferring its game-provided name whenever one is available.
        let Some(name) = name.or_else(|| (id == 0).then(|| "侯王礼拜堂 (新游戏)".into()))
        else {
            continue;
        };
        let (region, region_sort) = categories.get(&category).cloned().unwrap_or_default();
        let sort: u16 = at(&row, grace::SORT)?;
        let label = if region.is_empty() { name.clone() } else { format!("[{region}] {name}") };
        graces.push(((id != 0, region_sort, sort, id), Grace {
            id,
            entity_id,
            name,
            region,
            label,
        }));
    }
    graces.sort_by_key(|(sort, _)| *sort);
    let graces: Vec<_> = graces.into_iter().map(|(_, grace)| grace).collect();
    (!graces.is_empty()).then_some(graces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn birth_point_and_graces_are_valid_destinations() {
        assert_eq!(destination(4001950, 0), Some(4001950));
        assert_eq!(destination(10001950, 10000), Some(10001950));
        assert_eq!(destination(0, 0), None);
        assert_eq!(destination(u32::MAX, 10000), None);
        assert_eq!(destination(10001950, -1), None);
    }
}
