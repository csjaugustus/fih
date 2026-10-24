//! How many columns of the screen width each workspace is laid out in.
//!
//! A workspace takes the `[columns]` default for the kind of display it is
//! on - the Mac's own screen or an external monitor - until a count is chosen
//! for it. A choice is remembered for the kind of display it was made on, so
//! a workspace moving between the laptop and a monitor keeps each one's own.
//! Choices are saved across restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::ecs::change_detection::DetectChangesMut;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Res, ResMut};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::Config;
use crate::ecs::layout::LayoutStrip;

/// The counts chosen for one workspace, by the kind of display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Choice {
    builtin: Option<u8>,
    external: Option<u8>,
}

impl Choice {
    fn on(self, builtin: bool) -> Option<u8> {
        if builtin { self.builtin } else { self.external }
    }

    fn set(&mut self, builtin: bool, count: u8) {
        *if builtin {
            &mut self.builtin
        } else {
            &mut self.external
        } = Some(count);
    }
}

/// Column counts chosen per workspace and kind of display.
#[derive(Debug, Default, Resource, Serialize, Deserialize)]
pub struct ColumnChoices {
    /// Keyed by the macOS Space and the virtual workspace's index in it.
    choices: HashMap<String, Choice>,
    #[serde(skip)]
    dirty: bool,
}

fn key(strip: &LayoutStrip) -> String {
    format!("{}:{}", strip.id(), strip.virtual_index)
}

impl ColumnChoices {
    /// The count chosen for `strip` on the Mac's own screen (`builtin`) or
    /// an external monitor, if any.
    pub fn chosen(&self, strip: &LayoutStrip, builtin: bool) -> Option<u8> {
        self.choices
            .get(&key(strip))
            .and_then(|choice| choice.on(builtin))
    }

    /// The columns `strip` is laid out in on the Mac's own screen (`builtin`)
    /// or an external monitor: the count chosen for it there, or the
    /// configured default for that kind of display.
    pub fn count(&self, strip: &LayoutStrip, builtin: bool, config: &Config) -> u8 {
        self.chosen(strip, builtin)
            .unwrap_or_else(|| config.default_columns(builtin))
    }

    /// Chooses `count` columns for `strip` on the Mac's own screen
    /// (`builtin`) or an external monitor.
    pub fn choose(&mut self, strip: &LayoutStrip, builtin: bool, count: u8) {
        let count = count.clamp(1, 6);
        let choice = self.choices.entry(key(strip)).or_default();
        if choice.on(builtin) != Some(count) {
            choice.set(builtin, count);
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

    const BUILTIN: bool = true;
    const EXTERNAL: bool = false;

    #[test]
    fn workspaces_take_the_default_for_their_kind_of_display() {
        let config = Config::try_from("[columns]\nbuiltin = 2\nexternal = 3\n[bindings]\n")
            .expect("config should parse");
        let choices = ColumnChoices::default();
        let strip = LayoutStrip::new(7, 0);
        assert_eq!(choices.count(&strip, BUILTIN, &config), 2);
        assert_eq!(choices.count(&strip, EXTERNAL, &config), 3);
    }

    #[test]
    fn a_choice_holds_for_the_kind_of_display_it_was_made_on() {
        let config = Config::try_from("[columns]\nbuiltin = 2\nexternal = 3\n[bindings]\n")
            .expect("config should parse");
        let mut choices = ColumnChoices::default();
        let strip = LayoutStrip::new(7, 0);
        choices.choose(&strip, BUILTIN, 3);
        assert_eq!(choices.count(&strip, BUILTIN, &config), 3);
        // On a monitor it keeps the monitor's own (here the default).
        assert_eq!(choices.count(&strip, EXTERNAL, &config), 3);
        choices.choose(&strip, EXTERNAL, 2);
        assert_eq!(choices.count(&strip, EXTERNAL, &config), 2);
        assert_eq!(choices.count(&strip, BUILTIN, &config), 3);
        // Another workspace is untouched.
        assert_eq!(choices.count(&LayoutStrip::new(7, 1), BUILTIN, &config), 2);
    }

    #[test]
    fn without_a_columns_section_every_display_has_two() {
        let config = Config::try_from("[bindings]\n").expect("config should parse");
        let choices = ColumnChoices::default();
        let strip = LayoutStrip::new(7, 0);
        assert_eq!(choices.count(&strip, BUILTIN, &config), 2);
        assert_eq!(choices.count(&strip, EXTERNAL, &config), 2);
    }
}
