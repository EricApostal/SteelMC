//! Vanilla map color palette (`world/level/material/MapColor.java`).
//!
//! Vanilla hardcodes the 64 palette slots and their packed 8-bit ids; only the
//! per-state palette id is data, which is why `build_assets/blocks.json` carries
//! `map_color` and this module carries the colors themselves. A map pixel is a
//! packed `(palette id << 2) | brightness` byte, and the client expands it to
//! ARGB with [`MapColor::color_from_packed_id`].

/// One entry of vanilla's map color palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapColor {
    id: u8,
    col: u32,
}

/// Vanilla `MapColor.Brightness`: the shade applied to a map pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Brightness {
    /// Vanilla `MapColor.Brightness.LOW`.
    Low,
    /// Vanilla `MapColor.Brightness.NORMAL`.
    Normal,
    /// Vanilla `MapColor.Brightness.HIGH`.
    High,
    /// Vanilla `MapColor.Brightness.LOWEST`.
    Lowest,
}

impl Brightness {
    const VALUES: [Self; 4] = [Self::Low, Self::Normal, Self::High, Self::Lowest];

    /// Returns the two-bit brightness id stored in a packed map pixel.
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::High => 2,
            Self::Lowest => 3,
        }
    }

    /// Returns the vanilla RGB scale applied to the palette color.
    #[must_use]
    pub const fn modifier(self) -> u32 {
        match self {
            Self::Low => 180,
            Self::Normal => 220,
            Self::High => 255,
            Self::Lowest => 135,
        }
    }

    /// Decodes a two-bit brightness id from a packed map pixel.
    #[must_use]
    pub const fn by_id(id: u8) -> Self {
        Self::VALUES[(id & 3) as usize]
    }
}

impl MapColor {
    /// Vanilla's palette size; ids are stored in six bits.
    pub const COUNT: usize = 64;

    /// Vanilla `MapColor.NONE`.
    pub const NONE: Self = Self::new(0, 0);
    /// Vanilla `MapColor.GRASS`.
    pub const GRASS: Self = Self::new(1, 8368696);
    /// Vanilla `MapColor.SAND`.
    pub const SAND: Self = Self::new(2, 16247203);
    /// Vanilla `MapColor.WOOL`.
    pub const WOOL: Self = Self::new(3, 13092807);
    /// Vanilla `MapColor.FIRE`.
    pub const FIRE: Self = Self::new(4, 16711680);
    /// Vanilla `MapColor.ICE`.
    pub const ICE: Self = Self::new(5, 10526975);
    /// Vanilla `MapColor.METAL`.
    pub const METAL: Self = Self::new(6, 10987431);
    /// Vanilla `MapColor.PLANT`.
    pub const PLANT: Self = Self::new(7, 31744);
    /// Vanilla `MapColor.SNOW`.
    pub const SNOW: Self = Self::new(8, 16777215);
    /// Vanilla `MapColor.CLAY`.
    pub const CLAY: Self = Self::new(9, 10791096);
    /// Vanilla `MapColor.DIRT`.
    pub const DIRT: Self = Self::new(10, 9923917);
    /// Vanilla `MapColor.STONE`.
    pub const STONE: Self = Self::new(11, 7368816);
    /// Vanilla `MapColor.WATER`.
    pub const WATER: Self = Self::new(12, 4210943);
    /// Vanilla `MapColor.WOOD`.
    pub const WOOD: Self = Self::new(13, 9402184);
    /// Vanilla `MapColor.QUARTZ`.
    pub const QUARTZ: Self = Self::new(14, 16776437);
    /// Vanilla `MapColor.COLOR_ORANGE`.
    pub const COLOR_ORANGE: Self = Self::new(15, 14188339);
    /// Vanilla `MapColor.COLOR_MAGENTA`.
    pub const COLOR_MAGENTA: Self = Self::new(16, 11685080);
    /// Vanilla `MapColor.COLOR_LIGHT_BLUE`.
    pub const COLOR_LIGHT_BLUE: Self = Self::new(17, 6724056);
    /// Vanilla `MapColor.COLOR_YELLOW`.
    pub const COLOR_YELLOW: Self = Self::new(18, 15066419);
    /// Vanilla `MapColor.COLOR_LIGHT_GREEN`.
    pub const COLOR_LIGHT_GREEN: Self = Self::new(19, 8375321);
    /// Vanilla `MapColor.COLOR_PINK`.
    pub const COLOR_PINK: Self = Self::new(20, 15892389);
    /// Vanilla `MapColor.COLOR_GRAY`.
    pub const COLOR_GRAY: Self = Self::new(21, 5000268);
    /// Vanilla `MapColor.COLOR_LIGHT_GRAY`.
    pub const COLOR_LIGHT_GRAY: Self = Self::new(22, 10066329);
    /// Vanilla `MapColor.COLOR_CYAN`.
    pub const COLOR_CYAN: Self = Self::new(23, 5013401);
    /// Vanilla `MapColor.COLOR_PURPLE`.
    pub const COLOR_PURPLE: Self = Self::new(24, 8339378);
    /// Vanilla `MapColor.COLOR_BLUE`.
    pub const COLOR_BLUE: Self = Self::new(25, 3361970);
    /// Vanilla `MapColor.COLOR_BROWN`.
    pub const COLOR_BROWN: Self = Self::new(26, 6704179);
    /// Vanilla `MapColor.COLOR_GREEN`.
    pub const COLOR_GREEN: Self = Self::new(27, 6717235);
    /// Vanilla `MapColor.COLOR_RED`.
    pub const COLOR_RED: Self = Self::new(28, 10040115);
    /// Vanilla `MapColor.COLOR_BLACK`.
    pub const COLOR_BLACK: Self = Self::new(29, 1644825);
    /// Vanilla `MapColor.GOLD`.
    pub const GOLD: Self = Self::new(30, 16445005);
    /// Vanilla `MapColor.DIAMOND`.
    pub const DIAMOND: Self = Self::new(31, 6085589);
    /// Vanilla `MapColor.LAPIS`.
    pub const LAPIS: Self = Self::new(32, 4882687);
    /// Vanilla `MapColor.EMERALD`.
    pub const EMERALD: Self = Self::new(33, 55610);
    /// Vanilla `MapColor.PODZOL`.
    pub const PODZOL: Self = Self::new(34, 8476209);
    /// Vanilla `MapColor.NETHER`.
    pub const NETHER: Self = Self::new(35, 7340544);
    /// Vanilla `MapColor.TERRACOTTA_WHITE`.
    pub const TERRACOTTA_WHITE: Self = Self::new(36, 13742497);
    /// Vanilla `MapColor.TERRACOTTA_ORANGE`.
    pub const TERRACOTTA_ORANGE: Self = Self::new(37, 10441252);
    /// Vanilla `MapColor.TERRACOTTA_MAGENTA`.
    pub const TERRACOTTA_MAGENTA: Self = Self::new(38, 9787244);
    /// Vanilla `MapColor.TERRACOTTA_LIGHT_BLUE`.
    pub const TERRACOTTA_LIGHT_BLUE: Self = Self::new(39, 7367818);
    /// Vanilla `MapColor.TERRACOTTA_YELLOW`.
    pub const TERRACOTTA_YELLOW: Self = Self::new(40, 12223780);
    /// Vanilla `MapColor.TERRACOTTA_LIGHT_GREEN`.
    pub const TERRACOTTA_LIGHT_GREEN: Self = Self::new(41, 6780213);
    /// Vanilla `MapColor.TERRACOTTA_PINK`.
    pub const TERRACOTTA_PINK: Self = Self::new(42, 10505550);
    /// Vanilla `MapColor.TERRACOTTA_GRAY`.
    pub const TERRACOTTA_GRAY: Self = Self::new(43, 3746083);
    /// Vanilla `MapColor.TERRACOTTA_LIGHT_GRAY`.
    pub const TERRACOTTA_LIGHT_GRAY: Self = Self::new(44, 8874850);
    /// Vanilla `MapColor.TERRACOTTA_CYAN`.
    pub const TERRACOTTA_CYAN: Self = Self::new(45, 5725276);
    /// Vanilla `MapColor.TERRACOTTA_PURPLE`.
    pub const TERRACOTTA_PURPLE: Self = Self::new(46, 8014168);
    /// Vanilla `MapColor.TERRACOTTA_BLUE`.
    pub const TERRACOTTA_BLUE: Self = Self::new(47, 4996700);
    /// Vanilla `MapColor.TERRACOTTA_BROWN`.
    pub const TERRACOTTA_BROWN: Self = Self::new(48, 4993571);
    /// Vanilla `MapColor.TERRACOTTA_GREEN`.
    pub const TERRACOTTA_GREEN: Self = Self::new(49, 5001770);
    /// Vanilla `MapColor.TERRACOTTA_RED`.
    pub const TERRACOTTA_RED: Self = Self::new(50, 9321518);
    /// Vanilla `MapColor.TERRACOTTA_BLACK`.
    pub const TERRACOTTA_BLACK: Self = Self::new(51, 2430480);
    /// Vanilla `MapColor.CRIMSON_NYLIUM`.
    pub const CRIMSON_NYLIUM: Self = Self::new(52, 12398641);
    /// Vanilla `MapColor.CRIMSON_STEM`.
    pub const CRIMSON_STEM: Self = Self::new(53, 9715553);
    /// Vanilla `MapColor.CRIMSON_HYPHAE`.
    pub const CRIMSON_HYPHAE: Self = Self::new(54, 6035741);
    /// Vanilla `MapColor.WARPED_NYLIUM`.
    pub const WARPED_NYLIUM: Self = Self::new(55, 1474182);
    /// Vanilla `MapColor.WARPED_STEM`.
    pub const WARPED_STEM: Self = Self::new(56, 3837580);
    /// Vanilla `MapColor.WARPED_HYPHAE`.
    pub const WARPED_HYPHAE: Self = Self::new(57, 5647422);
    /// Vanilla `MapColor.WARPED_WART_BLOCK`.
    pub const WARPED_WART_BLOCK: Self = Self::new(58, 1356933);
    /// Vanilla `MapColor.DEEPSLATE`.
    pub const DEEPSLATE: Self = Self::new(59, 6579300);
    /// Vanilla `MapColor.RAW_IRON`.
    pub const RAW_IRON: Self = Self::new(60, 14200723);
    /// Vanilla `MapColor.GLOW_LICHEN`.
    pub const GLOW_LICHEN: Self = Self::new(61, 8365974);

    /// Creates a palette entry.
    ///
    /// Vanilla throws for ids outside `0..=63`; Steel asserts in debug builds
    /// and relies on the palette table for lookups.
    #[must_use]
    pub const fn new(id: u8, col: u32) -> Self {
        assert!(id < Self::COUNT as u8, "map color id must be below 64");
        Self { id, col }
    }

    /// Returns the palette id.
    #[must_use]
    pub const fn id(self) -> u8 {
        self.id
    }

    /// Returns the palette color as opaque `0xRRGGBB` (vanilla `MapColor.col`).
    #[must_use]
    pub const fn col(self) -> u32 {
        self.col
    }

    /// Vanilla `MapColor.byId`.
    ///
    /// The two unassigned slots (62, 63) and every out-of-range id resolve to
    /// [`Self::NONE`] instead of vanilla's null/NPE.
    #[must_use]
    pub const fn by_id(id: u8) -> Self {
        if id < Self::COUNT as u8 {
            Self::PALETTE[id as usize]
        } else {
            Self::NONE
        }
    }

    /// Vanilla `MapColor.calculateARGBColor`.
    #[must_use]
    pub const fn calculate_argb_color(self, brightness: Brightness) -> u32 {
        if self.id == 0 {
            return 0;
        }
        let scale = brightness.modifier();
        let red = (self.col >> 16 & 0xFF) * scale / 255;
        let green = (self.col >> 8 & 0xFF) * scale / 255;
        let blue = (self.col & 0xFF) * scale / 255;
        0xFF00_0000 | red << 16 | green << 8 | blue
    }

    /// Vanilla `MapColor.getPackedId`: the byte stored in a map's color buffer.
    #[must_use]
    pub const fn packed_id(self, brightness: Brightness) -> u8 {
        (self.id << 2) | (brightness.id() & 3)
    }

    /// Vanilla `MapColor.getColorFromPackedId`: ARGB for a map color buffer byte.
    #[must_use]
    pub const fn color_from_packed_id(packed_id: u8) -> u32 {
        Self::by_id(packed_id >> 2).calculate_argb_color(Brightness::by_id(packed_id))
    }

    const PALETTE: [Self; 64] = [
        Self::NONE,
        Self::GRASS,
        Self::SAND,
        Self::WOOL,
        Self::FIRE,
        Self::ICE,
        Self::METAL,
        Self::PLANT,
        Self::SNOW,
        Self::CLAY,
        Self::DIRT,
        Self::STONE,
        Self::WATER,
        Self::WOOD,
        Self::QUARTZ,
        Self::COLOR_ORANGE,
        Self::COLOR_MAGENTA,
        Self::COLOR_LIGHT_BLUE,
        Self::COLOR_YELLOW,
        Self::COLOR_LIGHT_GREEN,
        Self::COLOR_PINK,
        Self::COLOR_GRAY,
        Self::COLOR_LIGHT_GRAY,
        Self::COLOR_CYAN,
        Self::COLOR_PURPLE,
        Self::COLOR_BLUE,
        Self::COLOR_BROWN,
        Self::COLOR_GREEN,
        Self::COLOR_RED,
        Self::COLOR_BLACK,
        Self::GOLD,
        Self::DIAMOND,
        Self::LAPIS,
        Self::EMERALD,
        Self::PODZOL,
        Self::NETHER,
        Self::TERRACOTTA_WHITE,
        Self::TERRACOTTA_ORANGE,
        Self::TERRACOTTA_MAGENTA,
        Self::TERRACOTTA_LIGHT_BLUE,
        Self::TERRACOTTA_YELLOW,
        Self::TERRACOTTA_LIGHT_GREEN,
        Self::TERRACOTTA_PINK,
        Self::TERRACOTTA_GRAY,
        Self::TERRACOTTA_LIGHT_GRAY,
        Self::TERRACOTTA_CYAN,
        Self::TERRACOTTA_PURPLE,
        Self::TERRACOTTA_BLUE,
        Self::TERRACOTTA_BROWN,
        Self::TERRACOTTA_GREEN,
        Self::TERRACOTTA_RED,
        Self::TERRACOTTA_BLACK,
        Self::CRIMSON_NYLIUM,
        Self::CRIMSON_STEM,
        Self::CRIMSON_HYPHAE,
        Self::WARPED_NYLIUM,
        Self::WARPED_STEM,
        Self::WARPED_HYPHAE,
        Self::WARPED_WART_BLOCK,
        Self::DEEPSLATE,
        Self::RAW_IRON,
        Self::GLOW_LICHEN,
        Self::NONE,
        Self::NONE,
    ];
}

#[cfg(test)]
mod tests {
    use super::{Brightness, MapColor};

    #[test]
    fn packed_pixels_expand_to_vanilla_argb() {
        // `(id << 2) | brightness`: grass at HIGH is fully scaled, NORMAL is 220/255.
        assert_eq!(MapColor::GRASS.packed_id(Brightness::High), 0b0000_0110);
        assert_eq!(MapColor::color_from_packed_id(0b0000_0110), 0xFF7F_B238);
        assert_eq!(
            MapColor::color_from_packed_id(MapColor::GRASS.packed_id(Brightness::Normal)),
            0xFF6D_9930
        );
        // NONE renders as transparent black at every brightness.
        assert_eq!(
            MapColor::color_from_packed_id(MapColor::NONE.packed_id(Brightness::High)),
            0
        );
        // The two unassigned palette slots fall back to NONE.
        assert_eq!(MapColor::by_id(62), MapColor::NONE);
        assert_eq!(MapColor::by_id(63), MapColor::NONE);
    }
}
