//! The strip minimap: a row of app icons along the bottom of the screen, one
//! per column, with the visible part of the strip outlined. It comes up
//! briefly while navigating (`[decorations.minimap] enabled`) and stays up
//! while toggled on with `strip_show`. Clicking an icon focuses that column.

use std::collections::HashMap;
use std::time::Duration;

use bevy::ecs::change_detection::DetectChanges;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::{Added, Changed, With};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{NonSendMut, Populated, Query, Res, ResMut, Single};
use bevy::math::IRect;
use bevy::time::{Time, Timer, TimerMode};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSColor, NSCompositingOperation, NSEvent,
    NSFloatingWindowLevel, NSImage, NSPanel, NSRunningApplication, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::CGFloat;
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::commands::{Command, Direction, Operation};
use crate::config::Config;
use crate::ecs::layout::LayoutStrip;
use crate::ecs::{
    ActiveDisplayMarker, ActiveWorkspaceMarker, Bounds, DockPosition, FocusedMarker, Position,
    RepositionMarker, Scrolling,
};
use crate::events::{Event, EventSender};
use crate::manager::{Display, Window};
use crate::platform::Pid;

/// Height of the minimap panel, in points.
pub(crate) const MINIMAP_HEIGHT: f64 = 70.0;
/// Gap between the minimap and the bottom of the usable screen area.
pub(crate) const MINIMAP_BOTTOM_MARGIN: f64 = 24.0;
/// The minimap never grows past this share of the display width.
const MAX_WIDTH_RATIO: f64 = 0.8;
/// Space around the tiles, inside the panel. Half a gap more sits at the
/// left and right ends, where the outer tiles keep their share of it.
const PANEL_PADDING: f64 = 5.0;
const PANEL_RADIUS: f64 = 16.0;
const TILE_GAP: f64 = 9.0;
const TILE_WIDTH: f64 = 68.0;
const TILE_RADIUS: f64 = 10.0;
const ICON_SIZE: f64 = 30.0;
/// Room for a column's tile and the gap to the next one.
const SLOT_WIDTH: f64 = TILE_WIDTH + TILE_GAP;
/// Icon strength for a column that is off screen.
const OFF_SCREEN_ICON_ALPHA: f64 = 0.4;
/// The focus colour: the teal of the default focus border.
const FOCUS_RED: f64 = 0x5A as f64 / 255.0;
const FOCUS_GREEN: f64 = 0xC5 as f64 / 255.0;
const FOCUS_BLUE: f64 = 0xC9 as f64 / 255.0;
/// The last part of the timeout the minimap spends fading out.
const FADE_SECONDS: f32 = 0.3;

/// One column of the active strip, in absolute screen x.
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapColumn {
    pub pid: Option<Pid>,
    pub x: i32,
    pub width: i32,
    pub focused: bool,
}

/// What the minimap shows: the strip's columns and the visible range, both
/// in absolute screen x.
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapSnapshot {
    pub columns: Vec<MinimapColumn>,
    pub viewport: (i32, i32),
}

/// Where everything lands inside the panel, in points from its left edge.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MinimapGeometry {
    /// `(x, width)` of each column's slot, gap included.
    pub tiles: Vec<(f64, f64)>,
    /// Width of the whole panel.
    pub width: f64,
}

/// Gives every column a [`SLOT_WIDTH`] slot, whatever its width, shrinking
/// the slots when they would make the panel wider than `max_width`.
pub(crate) fn minimap_geometry(snapshot: &MinimapSnapshot, max_width: f64) -> MinimapGeometry {
    let columns = &snapshot.columns;
    #[allow(clippy::cast_precision_loss)]
    let count = columns.len() as f64;
    let available = (max_width - 2.0 * PANEL_PADDING).max(1.0);
    let slot = SLOT_WIDTH.min(available / count.max(1.0));
    MinimapGeometry {
        #[allow(clippy::cast_precision_loss)]
        tiles: (0..columns.len())
            .map(|index| (PANEL_PADDING + index as f64 * slot, slot))
            .collect(),
        width: count * slot + 2.0 * PANEL_PADDING,
    }
}

/// Whether most of `column` is inside the visible range `viewport`.
pub(crate) fn column_on_screen(column: &MinimapColumn, viewport: (i32, i32)) -> bool {
    let (view_min, view_max) = viewport;
    let overlap = (column.x + column.width).min(view_max) - column.x.max(view_min);
    overlap * 2 >= column.width.max(1)
}

/// When the minimap is up, and why.
#[derive(Debug, Default, Resource)]
pub struct MinimapState {
    /// Counts down from the last navigation; `None` when not navigating.
    timer: Option<Timer>,
    /// Toggled by `strip_show`: stays up until toggled off.
    pinned: bool,
    /// Whether the panel is actually on screen. It can be due (`visible`)
    /// yet hidden, e.g. on an empty strip.
    on_screen: bool,
}

impl MinimapState {
    /// Whether the panel is drawn right now, which is what anything laid out
    /// around it has to go by.
    pub fn on_screen(&self) -> bool {
        self.on_screen
    }

    /// Records whether the panel is drawn, touching the state (and so its
    /// change detection) only when that flips.
    fn set_on_screen(state: &mut ResMut<Self>, on_screen: bool) {
        if state.on_screen != on_screen {
            state.on_screen = on_screen;
        }
    }

    pub fn visible(&self) -> bool {
        self.pinned
            || self
                .timer
                .as_ref()
                .is_some_and(|timer| !timer.is_finished())
    }

    fn opacity(&self) -> f32 {
        if self.pinned {
            return 1.0;
        }
        self.timer.as_ref().map_or(0.0, |timer| {
            (timer.remaining_secs() / FADE_SECONDS).clamp(0.0, 1.0)
        })
    }

    fn poke(&mut self, config: &Config) {
        let duration = Duration::from_secs_f64(config.minimap_duration());
        self.timer = Some(Timer::new(duration, TimerMode::Once));
    }
}

/// Shows the minimap on commands that move around the strip: focus, swap,
/// resize and the like. Workspace switches have their own number popup.
pub(crate) fn minimap_on_command(
    mut messages: MessageReader<Event>,
    config: Res<Config>,
    mut state: ResMut<MinimapState>,
) {
    if messages.read().any(|event| {
        matches!(
            event,
            Event::Command {
                command: Command::Window(operation),
            } if navigates_strip(operation)
        )
    }) {
        state.poke(&config);
    }
}

/// Whether `operation` moves around, or reshapes, the current strip.
fn navigates_strip(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::Focus(_)
            | Operation::Swap(_)
            | Operation::Center
            | Operation::Resize(_)
            | Operation::StepWidth(_)
            | Operation::SetWidth(_)
            | Operation::FullWidth
            | Operation::Balance
            | Operation::Stack(_)
            | Operation::Pin(_)
            | Operation::ToggleTabbedDisplay
            | Operation::Snap
    )
}

/// The active strip, while a swipe or scroll is moving it.
type SwipedStrip<'w, 's> = Populated<
    'w,
    's,
    (),
    (
        With<ActiveWorkspaceMarker>,
        With<Scrolling>,
        Changed<Position>,
    ),
>;

/// Shows the minimap while the strip is swiped or scrolled.
pub(crate) fn minimap_on_swipe(
    _swiped: SwipedStrip,
    config: Res<Config>,
    mut state: ResMut<MinimapState>,
) {
    state.poke(&config);
}

/// Ends a navigation countdown when the workspace changes: the switch shows
/// its own number, which should not sit above a strip that is about to go.
pub(crate) fn minimap_on_workspace_switch(
    _switched: Populated<(), Added<ActiveWorkspaceMarker>>,
    mut state: ResMut<MinimapState>,
) {
    if state.timer.is_some() {
        state.timer = None;
    }
}

/// Toggles the minimap on `strip_show`.
pub(crate) fn strip_show_handler(
    mut messages: MessageReader<Event>,
    mut state: ResMut<MinimapState>,
) {
    for event in messages.read() {
        if let Event::Command {
            command: Command::StripShow,
        } = event
        {
            state.pinned = !state.pinned;
            state.timer = None;
        }
    }
}

/// Whether the minimap has anything to draw or take down this frame.
pub(crate) fn minimap_active(state: Res<MinimapState>) -> bool {
    state.pinned || state.timer.is_some() || state.is_changed()
}

/// Draws the minimap for the active strip, or takes it down once it times out.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_minimap(
    strip: Single<
        (&LayoutStrip, &Position, Option<&RepositionMarker>, &ChildOf),
        With<ActiveWorkspaceMarker>,
    >,
    displays: Query<(&Display, Option<&DockPosition>), With<ActiveDisplayMarker>>,
    windows: Query<(&Window, &Bounds)>,
    focused: Query<Entity, With<FocusedMarker>>,
    config: Res<Config>,
    clock: Res<Time>,
    mut state: ResMut<MinimapState>,
    manager: Option<NonSendMut<MinimapManager>>,
) {
    let Some(mut manager) = manager else {
        return;
    };
    // Only touch the state mutably while counting down: a mutable borrow
    // marks it changed, which would keep `minimap_active` true for good.
    if state.timer.is_some()
        && let Some(timer) = state.timer.as_mut()
        && timer.tick(clock.delta()).is_finished()
    {
        state.timer = None;
    }
    let (layout_strip, Position(position), reposition, child_of) = *strip;
    let Ok((display, dock)) = displays.get(child_of.parent()) else {
        return;
    };
    if !state.visible() {
        manager.hide();
        MinimapState::set_on_screen(&mut state, false);
        return;
    }

    let strip_x = reposition.map_or(position.x, |target| target.0.x);
    let frame = |entity| {
        windows
            .get(entity)
            .ok()
            .map(|(_, Bounds(size))| IRect::new(0, 0, size.x, size.y))
    };
    let focused = focused.iter().next();
    let columns = layout_strip
        .column_positions(&frame)
        .filter_map(|(column, x)| {
            let top = column.top()?;
            Some(MinimapColumn {
                pid: windows
                    .get(top)
                    .ok()
                    .and_then(|(window, _)| window.pid().ok()),
                x: strip_x + x,
                width: column.width(&frame)?,
                focused: focused.is_some_and(|entity| column.window_iter().any(|e| e == entity)),
            })
        })
        .collect::<Vec<_>>();
    // An empty strip has nothing to show, even when toggled on.
    if columns.is_empty() {
        manager.hide();
        MinimapState::set_on_screen(&mut state, false);
        return;
    }
    let viewport = display.actual_display_bounds(dock, &config);
    let snapshot = MinimapSnapshot {
        columns,
        viewport: (viewport.min.x, viewport.max.x),
    };

    let edge = display.edge_bounds(dock, &config);
    let bottom_center = NSPoint::new(
        f64::from(edge.min.x + edge.max.x) / 2.0,
        f64::from(edge.max.y),
    );
    let max_width = f64::from(edge.width()) * MAX_WIDTH_RATIO;
    manager.show(
        &snapshot,
        state.opacity(),
        bottom_center,
        max_width,
        config.minimap_interactive(),
    );
    MinimapState::set_on_screen(&mut state, true);
}

// ── AppKit ──────────────────────────────────────────────────────────────

struct Tile {
    rect: NSRect,
    icon: Option<Retained<NSImage>>,
    focused: bool,
    on_screen: bool,
}

struct MinimapViewIvars {
    tiles: Vec<Tile>,
    /// Where clicks go; `None` when the minimap is not interactive.
    events: Option<EventSender>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "FihMinimapView"]
    #[ivars = MinimapViewIvars]
    struct MinimapView;

    impl MinimapView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let ivars = self.ivars();
            // Drawn at full strength; the panel's own alpha does the fading.
            let alpha = CGFloat::from;
            let bounds = self.bounds();

            NSColor::colorWithSRGBRed_green_blue_alpha(0.11, 0.11, 0.12, alpha(0.9)).setFill();
            let panel = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                bounds,
                PANEL_RADIUS,
                PANEL_RADIUS,
            );
            panel.fill();
            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, alpha(0.12)).setStroke();
            panel.setLineWidth(1.0);
            panel.stroke();

            for tile in &ivars.tiles {
                let (fill, border, line_width) = if tile.focused {
                    (
                        NSColor::colorWithSRGBRed_green_blue_alpha(
                            FOCUS_RED, FOCUS_GREEN, FOCUS_BLUE, 0.16,
                        ),
                        NSColor::colorWithSRGBRed_green_blue_alpha(
                            FOCUS_RED, FOCUS_GREEN, FOCUS_BLUE, 1.0,
                        ),
                        2.0,
                    )
                } else {
                    (
                        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.06),
                        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.10),
                        1.0,
                    )
                };
                // Inset by half the line so the border stays inside the tile.
                let inset = line_width / 2.0;
                let rect = NSRect::new(
                    NSPoint::new(tile.rect.origin.x + inset, tile.rect.origin.y + inset),
                    NSSize::new(
                        tile.rect.size.width - line_width,
                        tile.rect.size.height - line_width,
                    ),
                );
                let path =
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, TILE_RADIUS, TILE_RADIUS);
                fill.setFill();
                path.fill();
                border.setStroke();
                path.setLineWidth(line_width);
                path.stroke();

                let Some(icon) = &tile.icon else {
                    continue;
                };
                let size = ICON_SIZE
                    .min(tile.rect.size.width - 12.0)
                    .min(tile.rect.size.height - 12.0);
                if size < 12.0 {
                    continue;
                }
                let icon_rect = NSRect::new(
                    NSPoint::new(
                        tile.rect.origin.x + (tile.rect.size.width - size) / 2.0,
                        tile.rect.origin.y + (tile.rect.size.height - size) / 2.0,
                    ),
                    NSSize::new(size, size),
                );
                icon.drawInRect_fromRect_operation_fraction(
                    icon_rect,
                    NSRect::ZERO,
                    NSCompositingOperation::SourceOver,
                    if tile.on_screen {
                        1.0
                    } else {
                        OFF_SCREEN_ICON_ALPHA
                    },
                );
            }
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let ivars = self.ivars();
            let Some(events) = &ivars.events else {
                return;
            };
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let Some(index) = ivars.tiles.iter().position(|tile| {
                point.x >= tile.rect.origin.x && point.x <= tile.rect.origin.x + tile.rect.size.width
            }) else {
                return;
            };
            let command = Command::Window(Operation::Focus(Direction::Nth(index)));
            if let Err(err) = events.send(Event::Command { command }) {
                tracing::warn!("minimap click: {err}");
            }
        }
    }
);

impl MinimapView {
    fn new(mtm: MainThreadMarker, frame: NSRect, ivars: MinimapViewIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

/// Owns the minimap panel and the app icons it draws.
pub struct MinimapManager {
    mtm: MainThreadMarker,
    events: EventSender,
    panel: Option<Retained<NSPanel>>,
    icons: HashMap<Pid, Option<Retained<NSImage>>>,
    /// What is drawn, so an unchanged frame is not redrawn. Opacity is left
    /// out: fading only changes the panel's alpha, which costs no redraw.
    shown: Option<(MinimapSnapshot, bool)>,
}

impl MinimapManager {
    pub fn new(mtm: MainThreadMarker, events: EventSender) -> Self {
        Self {
            mtm,
            events,
            panel: None,
            icons: HashMap::new(),
            shown: None,
        }
    }

    fn icon(&mut self, pid: Pid) -> Option<Retained<NSImage>> {
        // Kept across shows, since loading an app icon is slow; bounded so
        // apps that come and go cannot grow it without end.
        const MAX_CACHED_ICONS: usize = 64;
        if self.icons.len() >= MAX_CACHED_ICONS && !self.icons.contains_key(&pid) {
            self.icons.clear();
        }
        self.icons
            .entry(pid)
            .or_insert_with(|| {
                NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                    .and_then(|app| app.icon())
            })
            .clone()
    }

    fn show(
        &mut self,
        snapshot: &MinimapSnapshot,
        opacity: f32,
        bottom_center_abs_cg: NSPoint,
        max_width: f64,
        interactive: bool,
    ) {
        let key = (snapshot.clone(), interactive);
        if self.shown.as_ref() == Some(&key) {
            if let Some(panel) = &self.panel {
                panel.setAlphaValue(CGFloat::from(opacity));
            }
            return;
        }

        let geometry = minimap_geometry(snapshot, max_width);
        // The same space above and below the tiles as at the panel's ends.
        let edge = PANEL_PADDING + TILE_GAP / 2.0;
        let tile_height = MINIMAP_HEIGHT - 2.0 * edge;
        let tiles = snapshot
            .columns
            .iter()
            .zip(&geometry.tiles)
            .map(|(column, &(x, width))| Tile {
                rect: NSRect::new(
                    NSPoint::new(x + TILE_GAP / 2.0, edge),
                    NSSize::new((width - TILE_GAP).max(1.0), tile_height),
                ),
                icon: column.pid.and_then(|pid| self.icon(pid)),
                focused: column.focused,
                on_screen: column_on_screen(column, snapshot.viewport),
            })
            .collect();

        let size = NSSize::new(geometry.width, MINIMAP_HEIGHT);
        let screen_height = crate::overlay::primary_screen_height(self.mtm);
        let frame = NSRect::new(
            NSPoint::new(
                bottom_center_abs_cg.x - size.width / 2.0,
                screen_height - bottom_center_abs_cg.y + MINIMAP_BOTTOM_MARGIN,
            ),
            size,
        );
        let view = MinimapView::new(
            self.mtm,
            NSRect::new(NSPoint::new(0.0, 0.0), size),
            MinimapViewIvars {
                tiles,
                events: interactive.then(|| self.events.clone()),
            },
        );

        let panel = self
            .panel
            .get_or_insert_with(|| make_panel(self.mtm, frame));
        panel.setIgnoresMouseEvents(!interactive);
        panel.setAlphaValue(CGFloat::from(opacity));
        panel.setContentView(Some(&view));
        panel.setFrame_display(frame, true);
        panel.orderFront(None::<&AnyObject>);
        self.shown = Some(key);
    }

    fn hide(&mut self) {
        if let Some(panel) = &self.panel {
            panel.orderOut(None::<&AnyObject>);
        }
        self.shown = None;
    }
}

/// A borderless panel that takes clicks without activating fih, so the click
/// that focuses a window does not first pull focus away from the current one.
fn make_panel(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSPanel> {
    let panel: Retained<NSPanel> = unsafe {
        msg_send![
            NSPanel::alloc(mtm),
            initWithContentRect: frame,
            styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setLevel(NSFloatingWindowLevel + 1);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenNone,
    );
    panel
}

#[cfg(test)]
mod tests {
    use super::{
        MinimapColumn, MinimapSnapshot, PANEL_PADDING, SLOT_WIDTH, column_on_screen,
        minimap_geometry,
    };

    fn column(x: i32, width: i32) -> MinimapColumn {
        MinimapColumn {
            pid: None,
            x,
            width,
            focused: false,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn every_column_gets_one_tile_slot() {
        let snapshot = MinimapSnapshot {
            columns: vec![column(-1000, 1000), column(0, 500), column(500, 2000)],
            viewport: (0, 1000),
        };
        let geometry = minimap_geometry(&snapshot, 10_000.0);
        for (index, &(x, width)) in geometry.tiles.iter().enumerate() {
            assert!(close(width, SLOT_WIDTH));
            assert!(close(
                x,
                PANEL_PADDING + f64::from(u8::try_from(index).unwrap()) * SLOT_WIDTH
            ));
        }
        assert!(close(
            geometry.width,
            3.0 * SLOT_WIDTH + 2.0 * PANEL_PADDING
        ));
    }

    #[test]
    fn long_strips_shrink_to_fit() {
        let snapshot = MinimapSnapshot {
            columns: (0..40).map(|i| column(i * 1000, 1000)).collect(),
            viewport: (0, 1000),
        };
        let geometry = minimap_geometry(&snapshot, 800.0);
        assert!(close(geometry.width, 800.0));
        assert!(geometry.tiles[0].1 < SLOT_WIDTH);
    }

    #[test]
    fn a_column_counts_as_on_screen_when_most_of_it_is() {
        let viewport = (0, 1000);
        assert!(column_on_screen(&column(0, 500), viewport));
        assert!(column_on_screen(&column(600, 800), viewport), "half in");
        assert!(
            !column_on_screen(&column(800, 800), viewport),
            "a quarter in"
        );
        assert!(!column_on_screen(&column(-1000, 1000), viewport));
        assert!(!column_on_screen(&column(1000, 500), viewport));
    }
}
