//! Focused `RabbitEntity` regression tests.

use std::io::Cursor;
use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::read_compound as read_borrowed_compound;
use steel_registry::{init_vanilla_registry, vanilla_attributes, vanilla_entities};

use super::*;

fn load_rabbit(nbt: &NbtCompound) -> RabbitEntity {
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_borrowed_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
    let rabbit = RabbitEntity::new(&vanilla_entities::RABBIT, 1, DVec3::ZERO, Weak::new());
    rabbit.load_additional((&borrowed).into());
    rabbit
}

#[test]
fn rabbit_type_round_trips_through_the_legacy_int_codec() {
    init_vanilla_registry();

    let rabbit = RabbitEntity::new(&vanilla_entities::RABBIT, 1, DVec3::ZERO, Weak::new());
    rabbit.set_variant(RabbitVariant::Gold);

    let mut nbt = NbtCompound::new();
    rabbit.save_additional(&mut nbt);

    assert_eq!(nbt.int("RabbitType"), Some(RabbitVariant::Gold.id()));
    assert_eq!(nbt.int("MoreCarrotTicks"), Some(0));
}

#[test]
fn rabbit_type_load_uses_default_for_missing_or_unknown_ids() {
    init_vanilla_registry();

    // Vanilla's sparse id codec maps `EVIL` to 99 and every unknown id to `DEFAULT`.
    let mut evil = NbtCompound::new();
    evil.insert("RabbitType", RabbitVariant::Evil.id());
    assert_eq!(load_rabbit(&evil).variant(), RabbitVariant::Evil);

    let mut unknown = NbtCompound::new();
    unknown.insert("RabbitType", 123);
    assert_eq!(load_rabbit(&unknown).variant(), RabbitVariant::Brown);

    assert_eq!(
        load_rabbit(&NbtCompound::new()).variant(),
        RabbitVariant::Brown
    );
}

#[test]
fn evil_variant_applies_and_releases_killer_bunny_state() {
    init_vanilla_registry();

    let rabbit = RabbitEntity::new(&vanilla_entities::RABBIT, 1, DVec3::ZERO, Weak::new());
    rabbit.set_variant(RabbitVariant::Evil);
    assert_eq!(rabbit.variant(), RabbitVariant::Evil);

    let attributes = rabbit.attributes().lock();
    assert_eq!(
        attributes.get_base_value(vanilla_attributes::ARMOR),
        Some(EVIL_ARMOR_VALUE)
    );
    assert_eq!(
        attributes.modifier_value(
            vanilla_attributes::ATTACK_DAMAGE,
            &EVIL_ATTACK_POWER_MODIFIER
        ),
        Some(EVIL_ATTACK_POWER_INCREMENT)
    );
    drop(attributes);
    assert!(rabbit.custom_name().is_some());

    rabbit.set_variant(RabbitVariant::Brown);
    let attributes = rabbit.attributes().lock();
    assert_eq!(
        attributes.modifier_value(
            vanilla_attributes::ATTACK_DAMAGE,
            &EVIL_ATTACK_POWER_MODIFIER
        ),
        None
    );
}
