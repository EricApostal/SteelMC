//! Vanilla `net.minecraft.world.entity.ai.behavior` package.
//!
//! Concrete behaviors shared by villagers, iron golems and illagers. The control
//! machinery they plug into (behavior tree, `Trigger`s, `GateBehavior`) lives in
//! [`crate::entity::ai::brain`]; this module holds the behavior implementations
//! themselves, named after their vanilla counterparts.
//!
//! Steel-only helpers that vanilla keeps on `Villager`/`PoiManager`/`Brain` live in
//! [`poi`], [`utils`] and [`villager`].

#![expect(
    dead_code,
    reason = "behavior library is land-and-wire: packages register the members below, some helpers are only used by packages landing in later waves"
)]

pub mod acquire_poi;
pub mod assign_profession_from_job_site;
pub mod celebrate_villagers_survived_raid;
pub mod do_nothing;
pub mod give_gift_to_hero;
pub mod go_to_closest_village;
pub mod go_to_potential_job_site;
pub mod go_to_wanted_item;
pub mod harvest_farmland;
pub mod inside_brownian_walk;
pub mod interact_with;
pub mod interact_with_door;
pub mod jump_on_bed;
pub mod locate_hiding_place;
pub mod look_and_follow_trading_player;
pub mod look_at_target_sink;
pub mod move_to_sky_seeing_spot;
pub mod move_to_target_sink;
pub mod play_tag_with_other_kids;
pub mod poi;
pub mod poi_competitor_scan;
pub mod random_pos;
pub mod react_to_bell;
pub mod reset_profession;
pub mod reset_raid_status;
pub mod ring_bell;
pub mod set_closest_home_as_walk_target;
pub mod set_entity_look_target;
pub mod set_hidden_state;
pub mod set_look_and_interact;
pub mod set_raid_status;
pub mod set_walk_target_away_from;
pub mod set_walk_target_from_block_memory;
pub mod set_walk_target_from_look_target;
pub mod show_trades_to_player;
pub mod sleep_in_bed;
pub mod socialize_at_bell;
pub mod stroll_around_poi;
pub mod stroll_to_poi;
pub mod stroll_to_poi_list;
pub mod swim;
pub mod trade_with_villager;
pub mod update_activity_from_schedule;
pub mod use_bonemeal;
pub mod utils;
pub mod validate_nearby_poi;
pub mod village_bound_random_stroll;
pub mod villager;
pub mod villager_calm_down;
pub mod villager_goal_packages;
pub mod villager_make_love;
pub mod villager_panic_trigger;
pub mod wake_up;
pub mod work_at_composter;
pub mod work_at_poi;
pub mod yield_job_site;
