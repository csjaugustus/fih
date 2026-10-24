use bevy::ecs::world::World;

use crate::commands::{Command, Direction, Operation};
use crate::config::{Config, MainOptions, WindowParams};
use crate::ecs::Unmanaged;
use crate::ecs::layout::LayoutStrip;
use crate::events::Event;

use super::*;

fn assert_floating(world: &mut World, window_id: i32) {
    let entity = find_window_entity(window_id, world);
    assert!(
        matches!(world.get::<Unmanaged>(entity), Some(Unmanaged::Floating)),
        "window {window_id} should be floating"
    );
    let mut strips = world.query::<&LayoutStrip>();
    assert!(
        strips.iter(world).all(|strip| !strip.contains(entity)),
        "window {window_id} should stay out of the strip"
    );
}

fn minimize_and_restore(window_id: i32) -> [Event; 2] {
    [
        Event::WindowMinimized { window_id },
        Event::WindowDeminimized { window_id },
    ]
}

#[test]
fn rule_floated_window_floats_again_after_minimize() {
    let mut params = WindowParams::new(".*", Some("test".to_string()));
    params.floating = Some(true);
    let config: Config = (MainOptions::default(), vec![params]).into();

    let mut commands = vec![Event::Command {
        command: Command::PrintState,
    }];
    commands.extend(minimize_and_restore(0));
    TestHarness::new()
        .with_config(config)
        .with_windows(1)
        .on_iteration(0, |world, _state| assert_floating(world, 0))
        .on_iteration(2, |world, _state| assert_floating(world, 0))
        .run(commands);
}

#[test]
fn hand_floated_window_floats_again_after_minimize() {
    let mut commands = vec![
        Event::MenuOpened { window_id: 0 },
        Event::Command {
            command: Command::Window(Operation::Focus(Direction::Nth(1))),
        },
        Event::Command {
            command: Command::Window(Operation::Manage),
        },
    ];
    commands.extend(minimize_and_restore(1));
    TestHarness::new()
        .with_windows(3)
        .on_iteration(2, |world, _state| assert_floating(world, 1))
        .on_iteration(4, |world, _state| assert_floating(world, 1))
        .run(commands);
}

#[test]
fn tiled_window_returns_to_the_strip_after_minimize() {
    TestHarness::new()
        .with_windows(2)
        .on_iteration(2, |world, _state| {
            let entity = find_window_entity(1, world);
            assert!(world.get::<Unmanaged>(entity).is_none());
            let mut strips = world.query::<&LayoutStrip>();
            assert!(strips.iter(world).any(|strip| strip.contains(entity)));
        })
        .run(vec![
            Event::Command {
                command: Command::PrintState,
            },
            Event::WindowMinimized { window_id: 1 },
            Event::WindowDeminimized { window_id: 1 },
        ]);
}
