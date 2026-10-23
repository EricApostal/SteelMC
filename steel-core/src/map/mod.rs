//! Map item saved data: per-world filled-map state.
//!
//! A world owns its maps through [`MapStorage`]; individual map documents hold
//! their color buffers, markers and per-viewer dirty tracking in
//! [`MapItemSavedData`].

pub(crate) mod colors;
mod data;
mod decoration;
mod index;
mod storage;
mod world_maps;

pub use data::{
    HALF_MAP_SIZE, MAP_COLOR_BYTES, MAP_SIZE, MAX_SCALE, MapItemSavedData,
    TRACKED_DECORATION_LIMIT, calculate_rotation, clamp_map_coordinate, frame_decoration_key,
};
pub use decoration::{
    MapBanner, MapDecoration, MapFrame, banner_color, banner_decoration_type,
    decoration_type_by_key, frame_id,
};
pub use index::MapIndex;
pub use storage::{MapItemSnapshot, MapStorage};
