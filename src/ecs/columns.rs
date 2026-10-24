//! How many columns of the screen width each workspace is laid out in.
//!
//! A workspace takes the `[columns]` default for the kind of screen it is on
//! (the Mac's own, or an external monitor) until a count is chosen for it.
//! A choice is remembered for the screen it was made on, so a workspace
//! moving between the laptop and one or more monitors keeps each screen's
//! own. Choices are saved across restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::ecs::change_detection::DetectChangesMut;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Res, ResMut};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::Config;
use crate::ecs::layout::LayoutStrip;
use crate::manager::Screen;

/// Column counts chosen per workspace and screen.
#[derive(Debug, Default, Resource, Serialize, Deserialize)]
pub struct ColumnChoices {
    /// By workspace (the macOS Space and the virtual workspace's index in
    /// it), then by screen.
    picks: HashMap<String, HashMap<String, u8>>,
    #[serde(skip)]
    dirty: bool,
}

fn key(strip: &LayoutStrip) -> String {
    format!("{}:{}", strip.id(), strip.virtual_index)
}

impl ColumnChoices {
    /// The count chosen for `strip` on `screen`, if any.
    pub fn chosen(&self, strip: &LayoutStrip, screen: &Screen) -> Option<u8> {
        self.picks
            .get(&key(strip))
            .and_then(|screens| screens.get(&screen.uuid))
            .copied()
    }

    /// The columns `strip` is laid out in on `screen`: the count chosen for
    /// it there, or the configured default for that kind of screen.
    pub fn count(&self, strip: &LayoutStrip, screen: &Screen, config: &Config) -> u8 {
        self.chosen(strip, screen)
            .unwrap_or_else(|| config.default_columns(screen.builtin))
    }

    /// Chooses `count` columns for `strip` on `screen`.
    pub fn choose(&mut self, strip: &LayoutStrip, screen: &Screen, count: u8) {
        let count = count.clamp(1, 6);
        let screens = self.picks.entry(key(strip)).or_default();
        if screens.get(&screen.uuid) != Some(&count) {
            screens.insert(screen.uuid.clone(), count);
            self.dirty = true;
        }
    }

    fn load(path: &Path) -> Self {
        let data = match std::fs::read_to_string(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                warn!(%error, path = %path.display(), "unable to read column choices");
                return Self::default();
            }
        };
        serde_json::from_str(&data).unwrap_or_else(|error| {
            warn!(%error, path = %path.display(), "ignoring invalid column choices");
            Self::default()
        })
    }

    fn save(&self, path: &Path) -> crate::errors::Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, data)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

/// Where [`ColumnChoices`] are saved.
#[derive(Resource)]
pub struct ColumnChoicesFile(PathBuf);

/// Loads the saved column choices, for the real app only.
pub fn load_column_choices(app: &mut bevy::app::App) {
    if let Some(path) = xdg::BaseDirectories::with_prefix("fih").get_state_file("columns.json") {
        app.insert_resource(ColumnChoices::load(&path))
            .insert_resource(ColumnChoicesFile(path));
    }
}

/// Saves the column choices after one changes.
pub fn save_column_choices(
    mut choices: ResMut<ColumnChoices>,
    file: Option<Res<ColumnChoicesFile>>,
) {
    if !choices.dirty {
        return;
    }
    let Some(file) = file else {
        return;
    };
    match choices.save(&file.0) {
        Ok(()) => choices.bypass_change_detection().dirty = false,
        Err(error) => warn!(%error, path = %file.0.display(), "unable to save column choices"),
    }
}

#[cfg(test)]
mod tests {
    use super::ColumnChoices;
    use crate::config::Config;
    use crate::ecs::layout::LayoutStrip;
    use crate::manager::Screen;

    fn screen(uuid: &str, builtin: bool) -> Screen {
        Screen {
            uuid: uuid.to_string(),
            builtin,
        }
    }

    fn config() -> Config {
        Config::try_from("[columns]\nbuiltin = 2\nexternal = 3\n[bindings]\n")
            .expect("config should parse")
    }

    #[test]
    fn workspaces_take_the_default_for_their_kind_of_screen() {
        let choices = ColumnChoices::default();
        let strip = LayoutStrip::new(7, 0);
        assert_eq!(choices.count(&strip, &screen("laptop", true), &config()), 2);
        assert_eq!(choices.count(&strip, &screen("dell", false), &config()), 3);
    }

    #[test]
    fn a_choice_holds_for_the_screen_it_was_made_on() {
        let config = config();
        let mut choices = ColumnChoices::default();
        let strip = LayoutStrip::new(7, 0);
        let (laptop, dell, lg) = (
            screen("laptop", true),
            screen("dell", false),
            screen("lg", false),
        );
        choices.choose(&strip, &laptop, 3);
        choices.choose(&strip, &dell, 2);
        assert_eq!(choices.count(&strip, &laptop, &config), 3);
        assert_eq!(choices.count(&strip, &dell, &config), 2);
        // Another monitor keeps its own: here the external default.
        assert_eq!(choices.count(&strip, &lg, &config), 3);
        // Another workspace is untouched.
        assert_eq!(choices.count(&LayoutStrip::new(7, 1), &laptop, &config), 2);
    }

    #[test]
    fn without_a_columns_section_every_screen_has_two() {
        let config = Config::try_from("[bindings]\n").expect("config should parse");
        let choices = ColumnChoices::default();
        let strip = LayoutStrip::new(7, 0);
        assert_eq!(choices.count(&strip, &screen("laptop", true), &config), 2);
        assert_eq!(choices.count(&strip, &screen("dell", false), &config), 2);
    }
}
