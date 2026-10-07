use eldenring::cs::{ItemCategory, ItemId, WorldChrMan};
use fromsoftware_shared::FromStatic;

#[inline(always)]
pub fn try_get_key_item_quantity(key_item_id: i32) -> Option<u32> {
    let Ok(world_chr_man) = (unsafe { WorldChrMan::instance() }) else {
        return None;
    };

    let Some(player_ptr) = &world_chr_man.main_player else {
        return None;
    };

    let player = player_ptr.as_ref();
    let player_game_data = unsafe { player.player_game_data.as_ref() };

    let Ok(item_id) = ItemId::new(ItemCategory::Goods, key_item_id as u32) else {
        return Some(0);
    };

    let items = &player_game_data.equipment.equip_inventory_data.items_data;

    for entry in items.items() {
        if entry.item_id == item_id {
            return Some(entry.quantity);
        }
    }

    Some(0)
}

#[inline(always)]
pub fn get_key_item_quantity(key_item_id: i32) -> u32 {
    try_get_key_item_quantity(key_item_id).unwrap_or(0)
}
