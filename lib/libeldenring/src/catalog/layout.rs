// Packed PARAM field offsets, checked against Paramdex/Glorious Merchant and
// the game's regulation rows. Use only the prefix each reader actually needs.
// The legacy generated editable structs omit some reserved bitfields and cannot
// reliably describe these packed item fields.

pub(super) mod weapon {
    pub const SIZE: usize = 0x1a8;
    pub const SORT: usize = 0x08;
    pub const REINFORCE: usize = 0xda;
    pub const SORT_GROUP: usize = 0x190;
    pub const TYPE: usize = 0x1a6;
}

pub(super) mod menu_property {
    pub const SIZE: usize = 0x10;
    pub const WEAPON_TYPE: u32 = 300;
    pub const FORMAT: usize = 0x0e;
}

pub(super) mod menu_value {
    pub const SIZE: usize = 0x0c;
    pub const VALUE: usize = 0;
    pub const TEXT: usize = 4;
    pub const COMPARE: usize = 8;
}

pub(super) mod protector {
    pub const SIZE: usize = 0xd9;
    pub const SORT: usize = 0x04;
    pub const TYPE: usize = 0xd6;
    pub const SORT_GROUP: usize = 0xd8;
}

pub(super) mod accessory {
    pub const SIZE: usize = 0x34;
    pub const SORT: usize = 0x1c;
    pub const SORT_GROUP: usize = 0x33;
}

pub(super) mod goods {
    pub const SIZE: usize = 0x73;
    pub const SORT: usize = 0x20;
    pub const APPEARANCE_REPLACEMENT: usize = 0x24;
    pub const ICON: usize = 0x30;
    pub const MAX_QUANTITY: usize = 0x3a;
    pub const TYPE: usize = 0x3e;
    pub const FLAGS: usize = 0x48;
    pub const CONSUME_MASK: u8 = 0x80;
    pub const DISABLE_OFFLINE_MASK: u8 = 0x20;
    pub const SORT_GROUP: usize = 0x72;
}

pub(super) mod gem {
    pub const SIZE: usize = 0x2c;
    pub const SORT_GROUP: usize = 0x07;
    pub const SORT: usize = 0x28;
}

pub(super) mod shop {
    pub const SIZE: usize = 0x2c;
    pub const NAME_REPLACEMENT: usize = 0x28;
}

pub(super) mod grace {
    pub const SIZE: usize = 0x98;
    pub const ENTITY: usize = 0x08;
    pub const SORT: usize = 0x0e;
    pub const CATEGORY: usize = 0x14;
    pub const TEXT: usize = 0x30;
    pub const TEXT_TYPE: usize = 0x90;
}

pub(super) mod grace_category {
    pub const SIZE: usize = 0x0c;
    pub const TEXT: usize = 0x04;
    pub const TAB: usize = 0x08;
    pub const SORT: usize = 0x0a;
}

pub(super) mod grace_tab {
    pub const SIZE: usize = 0x0c;
    pub const SORT: usize = 0x08;
}
