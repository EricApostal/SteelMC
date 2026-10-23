use steel_registry::{REGISTRY, init_vanilla_registry, vanilla_poi_types};
use steel_utils::{BlockPos, SectionPos};

use super::*;
use crate::poi::{OccupationStatus, PointOfInterestStorage};
use crate::test_support::fresh_test_world;

fn home_type_id() -> usize {
    REGISTRY
        .poi_types
        .id_from_key(&vanilla_poi_types::HOME.key)
        .expect("home POI type is registered")
}

/// A storage holding one home (bed) POI with a single ticket at `pos`.
fn storage_with_bed(pos: BlockPos) -> PointOfInterestStorage {
    let mut storage = PointOfInterestStorage::new();
    storage.add(pos, home_type_id(), 1);
    storage
}

#[test]
fn take_reserves_the_only_ticket_and_release_returns_it() {
    init_vanilla_registry();
    let bed = BlockPos::new(8, 64, 8);
    let mut storage = storage_with_bed(bed);
    let is_bed = |poi_type: PoiTypeRef| poi_type.key == vanilla_poi_types::HOME.key;

    let taken = storage.take(&is_bed, &|_, _| true, bed, 48);
    assert_eq!(taken, Some(bed));
    assert!(
        storage.is_occupied(bed),
        "a claimed bed must report no free tickets"
    );

    // Vanilla `take` only considers POIs with space, so the second attempt finds nothing.
    assert_eq!(storage.take(&is_bed, &|_, _| true, bed, 48), None);

    assert!(storage.release(bed));
    assert!(!storage.is_occupied(bed));
    assert_eq!(storage.take(&is_bed, &|_, _| true, bed, 48), Some(bed));
}

#[test]
fn take_ignores_pois_rejected_by_the_filter() {
    init_vanilla_registry();
    let bed = BlockPos::new(8, 64, 8);
    let mut storage = storage_with_bed(bed);
    let is_bed = |poi_type: PoiTypeRef| poi_type.key == vanilla_poi_types::HOME.key;

    assert_eq!(storage.take(&is_bed, &|_, pos| pos != bed, bed, 48), None);
    assert!(
        !storage.is_occupied(bed),
        "a rejected POI keeps its tickets"
    );
}

#[test]
fn release_reports_failure_for_unknown_positions() {
    init_vanilla_registry();
    let mut storage = storage_with_bed(BlockPos::new(8, 64, 8));

    assert!(!storage.release(BlockPos::new(200, 64, 200)));
}

#[test]
fn range_queries_use_the_three_dimensional_distance() {
    init_vanilla_registry();
    let center = BlockPos::new(0, 64, 0);
    let mut storage = PointOfInterestStorage::new();
    let near = BlockPos::new(3, 64, 0);
    let far = BlockPos::new(0, 64, 6);
    storage.add(near, home_type_id(), 1);
    storage.add(far, home_type_id(), 1);

    let is_bed = |poi_type: PoiTypeRef| poi_type.key == vanilla_poi_types::HOME.key;
    let found = storage.get_in_range(&is_bed, center, 4, OccupationStatus::Any);
    assert_eq!(found.len(), 1, "the 6-block-away POI is out of range");
    assert_eq!(found[0].1, near);

    // `find_closest` orders by squared distance, not by the square query order.
    assert_eq!(
        storage.find_closest(&is_bed, center, 48, OccupationStatus::Any),
        Some(near)
    );

    let closest_first =
        storage.find_all_closest_first_with_type(&is_bed, center, 48, OccupationStatus::Any);
    assert_eq!(
        closest_first
            .iter()
            .map(|(_, pos)| *pos)
            .collect::<Vec<_>>(),
        vec![near, far]
    );
}

#[test]
fn poi_type_and_exists_read_the_registered_type() {
    init_vanilla_registry();
    let bed = BlockPos::new(8, 64, 8);
    let storage = storage_with_bed(bed);

    assert_eq!(
        storage.poi_type(bed).map(|poi_type| &poi_type.key),
        Some(&vanilla_poi_types::HOME.key)
    );
    assert!(storage.exists(bed, &|poi_type| poi_type.key == vanilla_poi_types::HOME.key));
    assert!(!storage.exists(bed.offset(1, 0, 0), &|_| true));
}

#[test]
fn village_distance_tracker_measures_hops_to_an_occupied_village_center() {
    init_vanilla_registry();
    let bed = BlockPos::new(8, 64, 8);
    let mut storage = storage_with_bed(bed);
    let center = SectionPos::from_block_pos(bed);

    // Vanilla `isVillageCenter` requires an *occupied* village POI, so an unclaimed bed
    // is not a village yet.
    assert!(!storage.is_village_center(center));
    assert_eq!(
        storage.sections_to_village(center),
        MAX_VILLAGE_DISTANCE + 1
    );

    assert!(storage.reserve_ticket(bed));

    assert!(storage.is_village_center(center));
    assert_eq!(storage.sections_to_village(center), 0);
    // 26-neighbour steps: both an axis neighbour and a body diagonal are one hop away.
    assert_eq!(
        storage.sections_to_village(SectionPos::new(center.x(), center.y(), center.z() + 1)),
        1
    );
    assert_eq!(
        storage.sections_to_village(SectionPos::new(
            center.x() + 1,
            center.y() + 1,
            center.z() + 1
        )),
        1
    );
    assert_eq!(
        storage.sections_to_village(SectionPos::new(center.x() + 6, center.y(), center.z())),
        MAX_VILLAGE_DISTANCE
    );
    // Levels beyond the tracking range collapse to the untracked value.
    assert_eq!(
        storage.sections_to_village(SectionPos::new(center.x() + 7, center.y(), center.z())),
        MAX_VILLAGE_DISTANCE + 1
    );
    assert_eq!(
        storage.sections_to_village(SectionPos::new(center.x() + 40, center.y(), center.z())),
        MAX_VILLAGE_DISTANCE + 1
    );
}

#[test]
fn world_village_queries_follow_the_distance_tracker() {
    init_vanilla_registry();
    let fixture = fresh_test_world("village_distance_queries");
    let world = &fixture.world;
    let bed = BlockPos::new(8, 64, 8);
    world.poi_storage.lock().add(bed, home_type_id(), 1);
    world.poi_storage.lock().reserve_ticket(bed);
    let village_section = SectionPos::from_block_pos(bed);

    assert!(world.is_village(bed), "an occupied bed is a village");
    assert!(world.is_close_to_village(bed, 1));
    assert_eq!(world.sections_to_village(village_section), 0);
    assert_eq!(
        world.poi_type_at(bed).map(|poi_type| &poi_type.key),
        Some(&vanilla_poi_types::HOME.key)
    );

    // One section east: still a village (vanilla `isVillage` allows one section), but the
    // block position itself is no longer a village *centre* for the section lookup.
    let one_away = bed.offset(20, 0, 0);
    assert_eq!(
        world.sections_to_village(SectionPos::from_block_pos(one_away)),
        1
    );
    assert!(world.is_village(one_away));

    // Three sections east: outside `isVillage`, still close for a relaxed query.
    let three_away = bed.offset(44, 0, 0);
    let far_section = SectionPos::from_block_pos(three_away);
    assert_eq!(world.sections_to_village(far_section), 3);
    assert!(!world.is_village(three_away));
    assert!(world.is_close_to_village(three_away, 3));
    assert!(!world.is_close_to_village(three_away, 2));

    // From a section outside the village, the closest section within the cube is the
    // village centre; from the centre itself there is nothing closer.
    assert_eq!(
        find_section_closest_to_village(world, far_section, 3),
        village_section
    );
    assert_eq!(
        find_section_closest_to_village(world, village_section, 2),
        village_section
    );

    assert!(world.release_poi(bed));
    assert!(!world.is_village(bed));
}

#[test]
fn section_helpers_match_vanilla_section_positions() {
    let section = SectionPos::new(1, 4, -1);
    assert_eq!(section_center(section), BlockPos::new(24, 72, -8));

    let cube = sections_in_cube(SectionPos::new(0, 0, 0), 1);
    assert_eq!(cube.len(), 27);
    assert!(cube.contains(&SectionPos::new(-1, -1, -1)));
    assert!(cube.contains(&SectionPos::new(1, 1, 1)));
}
