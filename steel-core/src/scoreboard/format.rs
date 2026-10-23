//! Per-score display overrides.
//!
//! Mirrors vanilla `PlayerScoreEntry` and `NumberFormat`. Vanilla's `StyledFormat`
//! wraps a full `Style`; Steel stores the decoration half (`Format`) here because
//! interleaved click/hover payloads are not part of a scoreboard render and are
//! carried by the command layer instead.

use serde::{Deserialize, Serialize};
use text_components::{TextComponent, format::Format};

/// Vanilla `NumberFormat`: how a scoreboard value is rendered.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberFormat {
    /// Renders nothing instead of the value.
    #[default]
    #[serde(rename = "blank")]
    Blank,
    /// Renders its component instead of the value.
    #[serde(rename = "fixed")]
    Fixed(TextComponent),
    /// Renders the value with a decoration style.
    #[serde(rename = "styled")]
    Styled(Format),
}

impl NumberFormat {
    /// Renders a score value the way vanilla `NumberFormat.format` does.
    #[must_use]
    pub fn format(&self, value: i32) -> TextComponent {
        match self {
            Self::Blank => TextComponent::new(),
            Self::Fixed(component) => component.clone(),
            Self::Styled(format) => TextComponent {
                format: format.clone(),
                ..TextComponent::plain(value.to_string())
            },
        }
    }
}

/// Vanilla `PlayerScoreEntry` extras stored with each score.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScoreExtras {
    /// Optional display-name override replacing the raw holder name.
    pub display: Option<TextComponent>,
    /// Optional number-format override for the scoreboard render.
    pub number_format: Option<NumberFormat>,
}

impl ScoreExtras {
    /// Creates empty extras.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            display: None,
            number_format: None,
        }
    }

    /// Returns whether any override is set.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.display.is_none() && self.number_format.is_none()
    }

    /// Vanilla `PlayerScoreEntry.ownerName`: the display component or the raw name.
    #[must_use]
    pub fn owner_name(&self, holder: &str) -> TextComponent {
        self.display
            .clone()
            .unwrap_or_else(|| TextComponent::plain(holder.to_owned()))
    }

    /// Vanilla `PlayerScoreEntry.formatValue`: formats with the override or default.
    #[must_use]
    pub fn format_value(&self, value: i32, default: &NumberFormat) -> TextComponent {
        let format = self.number_format.as_ref().unwrap_or(default);
        format.format(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{NumberFormat, ScoreExtras};
    use text_components::{TextComponent, format::Format};

    #[test]
    fn extras_default_to_no_display_or_format() {
        let extras = ScoreExtras::new();
        assert!(extras.is_empty());
        assert_eq!(
            extras.owner_name("Steve"),
            TextComponent::plain("Steve".to_owned())
        );
        assert_eq!(extras.display, None);
        let fallback = NumberFormat::Blank;
        assert_eq!(extras.format_value(3, &fallback), fallback.format(3));
    }

    #[test]
    fn fixed_format_renders_its_component() {
        let fixed = NumberFormat::Fixed(TextComponent::plain("★".to_owned()));
        assert_eq!(fixed.format(9), TextComponent::plain("★".to_owned()));
    }

    #[test]
    fn styled_format_keeps_its_decoration() {
        let styled = NumberFormat::Styled(Format::new());
        assert_eq!(styled.format(9), TextComponent::plain("9".to_owned()));
    }

    #[test]
    fn hidden_prefix_pairs_with_owner_name() {
        let mut extras = ScoreExtras::new();
        extras.display = Some(TextComponent::plain("alias".to_owned()));
        assert!(!extras.is_empty());
        assert_eq!(
            extras.owner_name("#hidden"),
            TextComponent::plain("alias".to_owned())
        );
    }
}
