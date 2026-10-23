//! Vanilla `Activity`.

use steel_utils::Identifier;

/// Vanilla `Activity`: the set of activities a brain can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Activity {
    Core,
    Idle,
    Work,
    Play,
    Rest,
    Meet,
    Panic,
    Raid,
    PreRaid,
    Hide,
    Fight,
    Celebrate,
    AdmireItem,
    Avoid,
    Ride,
    PlayDead,
    LongJump,
    Ram,
    Tongue,
    Swim,
    LaySpawn,
    Sniff,
    Investigate,
    Roar,
    Emerge,
    Dig,
}

impl Activity {
    /// Every activity, in vanilla registration order.
    pub const ALL: [Self; 26] = [
        Self::Core,
        Self::Idle,
        Self::Work,
        Self::Play,
        Self::Rest,
        Self::Meet,
        Self::Panic,
        Self::Raid,
        Self::PreRaid,
        Self::Hide,
        Self::Fight,
        Self::Celebrate,
        Self::AdmireItem,
        Self::Avoid,
        Self::Ride,
        Self::PlayDead,
        Self::LongJump,
        Self::Ram,
        Self::Tongue,
        Self::Swim,
        Self::LaySpawn,
        Self::Sniff,
        Self::Investigate,
        Self::Roar,
        Self::Emerge,
        Self::Dig,
    ];

    /// Vanilla registration name (without namespace).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Idle => "idle",
            Self::Work => "work",
            Self::Play => "play",
            Self::Rest => "rest",
            Self::Meet => "meet",
            Self::Panic => "panic",
            Self::Raid => "raid",
            Self::PreRaid => "pre_raid",
            Self::Hide => "hide",
            Self::Fight => "fight",
            Self::Celebrate => "celebrate",
            Self::AdmireItem => "admire_item",
            Self::Avoid => "avoid",
            Self::Ride => "ride",
            Self::PlayDead => "play_dead",
            Self::LongJump => "long_jump",
            Self::Ram => "ram",
            Self::Tongue => "tongue",
            Self::Swim => "swim",
            Self::LaySpawn => "lay_spawn",
            Self::Sniff => "sniff",
            Self::Investigate => "investigate",
            Self::Roar => "roar",
            Self::Emerge => "emerge",
            Self::Dig => "dig",
        }
    }

    #[must_use]
    pub fn identifier(self) -> Identifier {
        Identifier::vanilla_static(self.name())
    }

    /// Parses a vanilla activity name, ignoring the `minecraft:` namespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let path = name.strip_prefix("minecraft:").unwrap_or(name);
        Self::ALL
            .into_iter()
            .find(|activity| activity.name() == path)
    }
}
