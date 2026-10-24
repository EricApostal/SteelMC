//! Shared persistence for the vanilla `Display` base fields.
//!
//! Vanilla's `Display.readAdditionalSaveData`/`addAdditionalSaveData` are inherited by
//! `BlockDisplay`, `ItemDisplay` and `TextDisplay`, so the tags live in one helper here
//! instead of being repeated (or dropped) in each implementation.

use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::entity_data::{Quaternionf, Vector3f};
use steel_registry::vanilla_entity_data::DisplayEntityData;

const TAG_TRANSFORMATION: &str = "transformation";
const TAG_TRANSLATION: &str = "translation";
const TAG_LEFT_ROTATION: &str = "left_rotation";
const TAG_SCALE: &str = "scale";
const TAG_RIGHT_ROTATION: &str = "right_rotation";
const TAG_BILLBOARD: &str = "billboard";
const TAG_INTERPOLATION_DURATION: &str = "interpolation_duration";
const TAG_START_INTERPOLATION: &str = "start_interpolation";
const TAG_TELEPORT_DURATION: &str = "teleport_duration";
const TAG_VIEW_RANGE: &str = "view_range";
const TAG_SHADOW_RADIUS: &str = "shadow_radius";
const TAG_SHADOW_STRENGTH: &str = "shadow_strength";
const TAG_WIDTH: &str = "width";
const TAG_HEIGHT: &str = "height";
const TAG_GLOW_COLOR_OVERRIDE: &str = "glow_color_override";
const TAG_BRIGHTNESS: &str = "brightness";
const TAG_BRIGHTNESS_BLOCK: &str = "block";
const TAG_BRIGHTNESS_SKY: &str = "sky";

/// Brightness override value meaning "use the level light".
pub const NO_BRIGHTNESS_OVERRIDE: i32 = -1;

/// Vanilla `Display.BillboardConstraints`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BillboardConstraints {
    /// Rotation is fixed (vanilla default).
    #[default]
    Fixed = 0,
    /// Only the vertical axis follows the viewer.
    Vertical = 1,
    /// Only the horizontal axis follows the viewer.
    Horizontal = 2,
    /// The whole entity faces the viewer.
    Center = 3,
}

impl BillboardConstraints {
    /// Returns the network/synced-data id.
    #[must_use]
    pub const fn id(self) -> i8 {
        self as i8
    }

    /// Maps a synced-data id, falling back to `Fixed` like vanilla's `ByIdMap` zero strategy.
    #[must_use]
    pub const fn from_id(id: i8) -> Self {
        match id {
            1 => Self::Vertical,
            2 => Self::Horizontal,
            3 => Self::Center,
            _ => Self::Fixed,
        }
    }

    /// Returns the codec name used in NBT.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
            Self::Center => "center",
        }
    }

    /// Parses a codec name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "fixed" => Some(Self::Fixed),
            "vertical" => Some(Self::Vertical),
            "horizontal" => Some(Self::Horizontal),
            "center" => Some(Self::Center),
            _ => None,
        }
    }
}

/// Writes the vanilla `Display` base fields into `nbt`.
pub(super) fn save(data: &DisplayEntityData, nbt: &mut NbtCompound) {
    let translation = *data.translation.get();
    let scale = *data.scale.get();
    let left_rotation = *data.left_rotation.get();
    let right_rotation = *data.right_rotation.get();

    let mut transformation = NbtCompound::new();
    transformation.insert(
        TAG_TRANSLATION,
        float_list(&[translation.x, translation.y, translation.z]),
    );
    transformation.insert(
        TAG_LEFT_ROTATION,
        float_list(&[
            left_rotation.x,
            left_rotation.y,
            left_rotation.z,
            left_rotation.w,
        ]),
    );
    transformation.insert(TAG_SCALE, float_list(&[scale.x, scale.y, scale.z]));
    transformation.insert(
        TAG_RIGHT_ROTATION,
        float_list(&[
            right_rotation.x,
            right_rotation.y,
            right_rotation.z,
            right_rotation.w,
        ]),
    );
    nbt.insert(TAG_TRANSFORMATION, transformation);

    nbt.insert(
        TAG_BILLBOARD,
        BillboardConstraints::from_id(*data.billboard_render_constraints.get()).name(),
    );
    nbt.insert(
        TAG_INTERPOLATION_DURATION,
        *data.transformation_interpolation_duration.get(),
    );
    nbt.insert(
        TAG_TELEPORT_DURATION,
        *data.pos_rot_interpolation_duration.get(),
    );
    nbt.insert(TAG_VIEW_RANGE, *data.view_range.get());
    nbt.insert(TAG_SHADOW_RADIUS, *data.shadow_radius.get());
    nbt.insert(TAG_SHADOW_STRENGTH, *data.shadow_strength.get());
    nbt.insert(TAG_WIDTH, *data.width.get());
    nbt.insert(TAG_HEIGHT, *data.height.get());
    nbt.insert(TAG_GLOW_COLOR_OVERRIDE, *data.glow_color_override.get());

    // Vanilla stores a nullable `Brightness { block, sky }`; the packed form uses
    // `block << 4 | sky << 20` and `-1` means "no override".
    let brightness = *data.brightness_override.get();
    if brightness != NO_BRIGHTNESS_OVERRIDE {
        let mut compound = NbtCompound::new();
        compound.insert(TAG_BRIGHTNESS_BLOCK, brightness >> 4 & 15);
        compound.insert(TAG_BRIGHTNESS_SKY, brightness >> 20 & 15);
        nbt.insert(TAG_BRIGHTNESS, compound);
    }
}

/// Reads the vanilla `Display` base fields from `nbt`, restoring vanilla defaults for
/// missing tags.
///
/// NOTE: vanilla's `Transformation.EXTENDED_CODEC` also accepts a legacy 16-float matrix;
/// only the record form is read here.
pub(super) fn load(data: &mut DisplayEntityData, nbt: BorrowedNbtCompoundView<'_, '_>) {
    let defaults = DisplayEntityData::new();

    if let Some(transformation) = nbt.compound(TAG_TRANSFORMATION) {
        if let Some(translation) = read_vector3(&transformation, TAG_TRANSLATION) {
            data.translation.set(translation);
        }
        if let Some(rotation) = read_quaternion(&transformation, TAG_LEFT_ROTATION) {
            data.left_rotation.set(rotation);
        }
        if let Some(scale) = read_vector3(&transformation, TAG_SCALE) {
            data.scale.set(scale);
        }
        if let Some(rotation) = read_quaternion(&transformation, TAG_RIGHT_ROTATION) {
            data.right_rotation.set(rotation);
        }
    }

    data.billboard_render_constraints.set(
        nbt.string(TAG_BILLBOARD)
            .and_then(|name| BillboardConstraints::from_name(&name.to_str()))
            .unwrap_or_default()
            .id(),
    );
    data.transformation_interpolation_duration.set(
        nbt.int(TAG_INTERPOLATION_DURATION)
            .unwrap_or(*defaults.transformation_interpolation_duration.get()),
    );
    // Vanilla loads `start_interpolation` but never writes it back.
    data.transformation_interpolation_start_delta_ticks.set(
        nbt.int(TAG_START_INTERPOLATION).unwrap_or(
            *defaults
                .transformation_interpolation_start_delta_ticks
                .get(),
        ),
    );
    data.pos_rot_interpolation_duration.set(
        nbt.int(TAG_TELEPORT_DURATION)
            .unwrap_or(*defaults.pos_rot_interpolation_duration.get())
            .clamp(0, 59),
    );
    data.view_range.set(
        nbt.float(TAG_VIEW_RANGE)
            .unwrap_or(*defaults.view_range.get()),
    );
    data.shadow_radius.set(
        nbt.float(TAG_SHADOW_RADIUS)
            .unwrap_or(*defaults.shadow_radius.get()),
    );
    data.shadow_strength.set(
        nbt.float(TAG_SHADOW_STRENGTH)
            .unwrap_or(*defaults.shadow_strength.get()),
    );
    data.width
        .set(nbt.float(TAG_WIDTH).unwrap_or(*defaults.width.get()));
    data.height
        .set(nbt.float(TAG_HEIGHT).unwrap_or(*defaults.height.get()));
    data.glow_color_override.set(
        nbt.int(TAG_GLOW_COLOR_OVERRIDE)
            .unwrap_or(*defaults.glow_color_override.get()),
    );

    data.brightness_override
        .set(
            nbt.compound(TAG_BRIGHTNESS)
                .map_or(NO_BRIGHTNESS_OVERRIDE, |brightness| {
                    let block = brightness
                        .int(TAG_BRIGHTNESS_BLOCK)
                        .unwrap_or(0)
                        .clamp(0, 15);
                    let sky = brightness.int(TAG_BRIGHTNESS_SKY).unwrap_or(0).clamp(0, 15);
                    block << 4 | sky << 20
                }),
        );
}

fn float_list(values: &[f32]) -> NbtTag {
    NbtTag::List(NbtList::Float(values.to_vec()))
}

fn read_vector3(nbt: &BorrowedNbtCompoundView<'_, '_>, tag: &str) -> Option<Vector3f> {
    let values = nbt.list(tag)?.floats()?;
    let [x, y, z] = values.as_slice() else {
        return None;
    };
    Some(Vector3f::new(*x, *y, *z))
}

fn read_quaternion(nbt: &BorrowedNbtCompoundView<'_, '_>, tag: &str) -> Option<Quaternionf> {
    let values = nbt.list(tag)?.floats()?;
    let [x, y, z, w] = values.as_slice() else {
        return None;
    };
    Some(normalize_quaternion(Quaternionf::new(*x, *y, *z, *w)))
}

/// Normalizes a quaternion like vanilla's `QUATERNIONF_COMPONENTS` codec.
///
/// A zero-length quaternion (which vanilla would turn into `NaN`) stays the identity.
fn normalize_quaternion(quaternion: Quaternionf) -> Quaternionf {
    let length_squared = quaternion.x * quaternion.x
        + quaternion.y * quaternion.y
        + quaternion.z * quaternion.z
        + quaternion.w * quaternion.w;
    if length_squared <= f32::EPSILON {
        return Quaternionf::IDENTITY;
    }
    let inverse_length = length_squared.sqrt().recip();
    Quaternionf::new(
        quaternion.x * inverse_length,
        quaternion.y * inverse_length,
        quaternion.z * inverse_length,
        quaternion.w * inverse_length,
    )
}
