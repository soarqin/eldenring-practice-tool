//! Catalogs from the loaded regulation and the game's current-language FMGs.
//! Addresses come from the existing version table; no runtime signature scans.

mod graces;
mod items;
mod layout;
mod memory;
mod messages;

pub use graces::Grace;
pub use items::{Item, ItemCategory, ItemVariant, WeaponType};
use memory::ParamTables;
use messages::{Messages, NameKind};

use crate::codegen::base_addresses::BaseAddresses;

#[derive(Clone, Copy, Debug)]
pub struct GameData {
    regulation_manager: usize,
    msg_repository: usize,
}

impl GameData {
    pub fn new(addresses: &BaseAddresses) -> Self {
        Self {
            regulation_manager: addresses.cs_regulation_manager,
            msg_repository: addresses.msg_repository,
        }
    }

    /// Returns `None` while either the regulation or localized text is
    /// unloaded.
    pub fn items(&self) -> Option<Vec<Item>> {
        items::read(
            &ParamTables::read(self.regulation_manager, &[
                "EquipParamWeapon",
                "EquipParamGoods",
                "EquipParamProtector",
                "EquipParamAccessory",
                "EquipParamGem",
                "ReinforceParamWeapon",
                "ShopLineupParam",
                "MenuPropertySpecParam",
                "MenuValueTableParam",
            ])?,
            &Messages::read(self.msg_repository, &[
                NameKind::Goods,
                NameKind::Weapon,
                NameKind::Protector,
                NameKind::Accessory,
                NameKind::Gem,
                NameKind::Menu,
            ])?,
        )
    }

    /// Includes unactivated graces, as the practice tool's original list did.
    pub fn graces(&self) -> Option<Vec<Grace>> {
        graces::read(
            &ParamTables::read(self.regulation_manager, &[
                "BonfireWarpParam",
                "BonfireWarpSubCategoryParam",
                "BonfireWarpTabParam",
            ])?,
            &Messages::read(self.msg_repository, &[
                NameKind::Place,
                NameKind::Npc,
                NameKind::Menu,
            ])?,
        )
    }
}
