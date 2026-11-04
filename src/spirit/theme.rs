//! Card themes: which element a card's spirit is made of.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Fire,
    Water,
    Poison,
    Lightning,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Fire, Theme::Water, Theme::Poison, Theme::Lightning];

    /// Next theme in `ALL` order, wrapping from `Lightning` back to `Fire`.
    pub fn next(self) -> Theme {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_fire_and_next_cycles_in_order() {
        assert_eq!(Theme::default(), Theme::Fire);
        let mut t = Theme::Fire;
        let mut seen = vec![t];
        for _ in 0..4 {
            t = t.next();
            seen.push(t);
        }
        assert_eq!(seen, [Theme::Fire, Theme::Water, Theme::Poison, Theme::Lightning, Theme::Fire]);
    }

    #[test]
    fn serde_names_are_lowercase() {
        #[derive(Serialize, Deserialize)]
        struct W {
            t: Theme,
        }
        for (t, name) in [(Theme::Fire, "fire"), (Theme::Water, "water"), (Theme::Poison, "poison"), (Theme::Lightning, "lightning")] {
            assert_eq!(toml::to_string(&W { t }).unwrap().trim(), format!("t = \"{name}\""));
            assert_eq!(toml::from_str::<W>(&format!("t = \"{name}\"")).unwrap().t, t);
        }
        assert!(toml::from_str::<W>("t = \"Fire\"").is_err());
        assert!(toml::from_str::<W>("t = \"bogus\"").is_err());
        assert!(toml::from_str::<W>("t = \"melee\"").is_err(), "melee was removed");
        assert!(toml::from_str::<W>("t = \"ice\"").is_err(), "ice was replaced by poison");
        assert!(toml::from_str::<W>("t = \"Poison\"").is_err());
    }
}
