use crate::commands::{Command, Direction, Operation};
use crate::config::Config;
use crate::events::Event;
use crate::minimap::MinimapState;
use bevy::ecs::world::World;

use super::*;

fn minimap_visible(world: &World) -> bool {
    world.resource::<MinimapState>().visible()
}

#[test]
fn navigating_shows_the_minimap_when_enabled() {
    let config = Config::try_from("[decorations.minimap]\nenabled = true\n[bindings]\n")
        .expect("config should parse");

    TestHarness::new()
        .with_config(config)
        .with_windows(3)
        .on_iteration(0, |world, _state| {
            assert!(!minimap_visible(world), "startup is not navigation");
        })
        .on_iteration(1, |world, _state| {
            assert!(minimap_visible(world), "moving focus shows the minimap");
        })
        .run(vec![
            Event::Command {
                command: Command::PrintState,
            },
            Event::Command {
                command: Command::Window(Operation::Focus(Direction::First)),
            },
        ]);
}

#[test]
fn navigating_leaves_the_minimap_hidden_by_default() {
    TestHarness::new()
        .with_windows(3)
        .on_iteration(1, |world, _state| {
            assert!(!minimap_visible(world));
        })
        .run(vec![
            Event::Command {
                command: Command::PrintState,
            },
            Event::Command {
                command: Command::Window(Operation::Focus(Direction::First)),
            },
        ]);
}

#[test]
fn strip_show_toggles_the_minimap() {
    TestHarness::new()
        .with_windows(2)
        .on_iteration(0, |world, _state| assert!(minimap_visible(world)))
        .on_iteration(1, |world, _state| assert!(!minimap_visible(world)))
        .run(vec![
            Event::Command {
                command: Command::StripShow,
            },
            Event::Command {
                command: Command::StripShow,
            },
        ]);
}

#[test]
fn switching_workspaces_shows_the_minimap() {
    let config = Config::try_from("[decorations.minimap]\nenabled = true\n[bindings]\n")
        .expect("config should parse");

    TestHarness::new()
        .with_config(config)
        .with_windows(2)
        .on_iteration(1, |world, _state| {
            assert!(
                minimap_visible(world),
                "a workspace switch shows every workspace's strip"
            );
        })
        .run(vec![
            Event::Command {
                command: Command::PrintState,
            },
            Event::Command {
                command: Command::Window(Operation::VirtualNumber(1)),
            },
        ]);
}

#[test]
fn switching_workspaces_mid_navigation_keeps_the_minimap_up() {
    let config = Config::try_from("[decorations.minimap]\nenabled = true\n[bindings]\n")
        .expect("config should parse");

    TestHarness::new()
        .with_config(config)
        .with_windows(2)
        .on_iteration(1, |world, _state| assert!(minimap_visible(world)))
        .on_iteration(2, |world, _state| {
            assert!(
                minimap_visible(world),
                "the switch turns the strip into every workspace's strip"
            );
        })
        .run(vec![
            Event::Command {
                command: Command::PrintState,
            },
            Event::Command {
                command: Command::Window(Operation::Focus(Direction::First)),
            },
            Event::Command {
                command: Command::Window(Operation::VirtualNumber(1)),
            },
        ]);
}

#[test]
fn the_minimap_replaces_the_workspace_number_popup() {
    let config = Config::try_from("[decorations.minimap]\nenabled = true\n[bindings]\n")
        .expect("config should parse");

    TestHarness::new()
        .with_config(config)
        .with_windows(2)
        .on_iteration(1, |world, _state| {
            let mut messages = world.query::<&crate::ecs::FlashMessage>();
            assert!(
                messages.iter(world).next().is_none(),
                "the minimap's rows show the number instead"
            );
        })
        .run(vec![
            Event::Command {
                command: Command::PrintState,
            },
            Event::Command {
                command: Command::Window(Operation::VirtualNumber(1)),
            },
        ]);
}
