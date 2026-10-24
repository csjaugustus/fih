//! The strip minimap: a row of app icons along the bottom of the screen, one
//! per column, labelled with its workspace number. Moving around the strip
//! shows the current workspace's row; switching workspaces shows a row for
//! every workspace with windows, the current one highlighted. It comes up
//! briefly while navigating (`[decorations.minimap] enabled`) and stays up
//! while toggled on with `strip_show`. Clicking an icon focuses that column,
//! or switches to its workspace.

use std::collections::HashMap;
use std::time::Duration;

use bevy::ecs::change_detection::DetectChanges;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::{Added, Changed, Has, With};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{NonSendMut, Populated, Query, Res, ResMut};
use bevy::math::IRect;
use bevy::time::{Time, Timer, TimerMode};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSColor, NSCompositingOperation, NSEvent,
    NSFloatingWindowLevel, NSFont, NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration,
    NSPanel, NSRunningApplication, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::CGFloat;
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString,
};

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

/// Height of the minimap panel with a single row, in points.
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
/// Height of one workspace's row, its tiles and the gap around them.
const ROW_HEIGHT: f64 = MINIMAP_HEIGHT - 2.0 * PANEL_PADDING;
/// Room at the left of each row for its workspace number.
const LABEL_WIDTH: f64 = 18.0;
const LABEL_FONT_SIZE: f64 = 11.0;
/// Icon strength for a column that is off screen, or on another workspace.
const OFF_SCREEN_ICON_ALPHA: f64 = 0.4;
/// The pinned-window colour: an orange that reads apart from the focus teal.
const PIN_RED: f64 = 1.0;
const PIN_GREEN: f64 = 0.62;
const PIN_BLUE: f64 = 0.04;
/// Height of the lock drawn on the pinned window's tile.
const PIN_LOCK_HEIGHT: f64 = 11.0;
/// The focus colour: the teal of the default focus border.
const FOCUS_RED: f64 = 0x5A as f64 / 255.0;
const FOCUS_GREEN: f64 = 0xC5 as f64 / 255.0;
const FOCUS_BLUE: f64 = 0xC9 as f64 / 255.0;
/// The last part of the timeout the minimap spends fading out.
const FADE_SECONDS: f32 = 0.3;

/// One column of a strip, in absolute screen x.
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapColumn {
    pub pid: Option<Pid>,
    pub x: i32,
    pub width: i32,
    pub focused: bool,
    /// The window pinned to an edge of the viewport, outside the columns.
    pub pinned: bool,
}

/// One workspace's strip.
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapRow {
    pub virtual_index: u32,
    /// The workspace on screen.
    pub active: bool,
    pub columns: Vec<MinimapColumn>,
}

/// What the minimap shows: a row per workspace, top to bottom, and the
/// visible range in absolute screen x.
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapSnapshot {
    pub rows: Vec<MinimapRow>,
    pub viewport: (i32, i32),
    /// Switching workspaces: the current row is highlighted, not a tile.
    pub workspaces: bool,
}

/// Where everything lands inside the panel, in points from its bottom left.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MinimapGeometry {
    /// Per row, top to bottom: the row's bottom edge and `(x, width)` of each
    /// column's slot, gap included.
    pub rows: Vec<(f64, Vec<(f64, f64)>)>,
    pub width: f64,
    pub height: f64,
}

/// Gives every column a [`SLOT_WIDTH`] slot, whatever its width, shrinking
/// the slots when the longest row would make the panel wider than
/// `max_width`. Rows stack downwards from the top, each with room for its
/// workspace number at the left.
pub(crate) fn minimap_geometry(snapshot: &MinimapSnapshot, max_width: f64) -> MinimapGeometry {
    let longest = snapshot
        .rows
        .iter()
        .map(|row| row.columns.len())
        .max()
        .unwrap_or(0)
        .max(1);
    #[allow(clippy::cast_precision_loss)]
    let longest = longest as f64;
    let available = (max_width - 2.0 * PANEL_PADDING - LABEL_WIDTH).max(1.0);
    let slot = SLOT_WIDTH.min(available / longest);
    #[allow(clippy::cast_precision_loss)]
    let height = 2.0 * PANEL_PADDING + snapshot.rows.len() as f64 * ROW_HEIGHT;
    MinimapGeometry {
        rows: snapshot
            .rows
            .iter()
            .enumerate()
            .map(|(row_index, row)| {
                #[allow(clippy::cast_precision_loss)]
                let bottom = height - PANEL_PADDING - (row_index + 1) as f64 * ROW_HEIGHT;
                #[allow(clippy::cast_precision_loss)]
                let tiles = (0..row.columns.len())
                    .map(|index| (PANEL_PADDING + LABEL_WIDTH + index as f64 * slot, slot))
                    .collect();
                (bottom, tiles)
            })
            .collect(),
        width: 2.0 * PANEL_PADDING + LABEL_WIDTH + longest * slot,
        height,
    }
}

/// Whether most of `column` is inside the visible range `viewport`. A pinned
/// window always is.
pub(crate) fn column_on_screen(column: &MinimapColumn, viewport: (i32, i32)) -> bool {
    if column.pinned {
        return true;
    }
    let (view_min, view_max) = viewport;
    let overlap = (column.x + column.width).min(view_max) - column.x.max(view_min);
    overlap * 2 >= column.width.max(1)
}

/// What the minimap is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MinimapMode {
    /// Moving around the strip: the current workspace's row.
    #[default]
    Strip,
    /// Switching workspaces: a row for every workspace with windows.
    Workspaces,
}

/// When the minimap is up, and why.
#[derive(Debug, Default, Resource)]
pub struct MinimapState {
    /// Counts down from the last navigation; `None` when not navigating.
    timer: Option<Timer>,
    /// Toggled by `strip_show`: stays up until toggled off.
    pinned: bool,
    mode: MinimapMode,
    /// The panel's height while it is on screen. It can be due (`visible`)
    /// yet hidden, e.g. on an empty strip.
    on_screen: Option<f64>,
}

impl MinimapState {
    /// The panel's height while it is drawn, which is what anything laid out
    /// around it has to go by.
    pub fn on_screen(&self) -> Option<f64> {
        self.on_screen
    }

    /// Records whether, and how tall, the panel is drawn, touching the state
    /// (and so its change detection) only when that changes.
    fn set_on_screen(state: &mut ResMut<Self>, on_screen: Option<f64>) {
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

    fn poke(&mut self, config: &Config, mode: MinimapMode) {
        let duration = Duration::from_secs_f64(config.minimap_duration());
        self.timer = Some(Timer::new(duration, TimerMode::Once));
        self.mode = mode;
    }
}

/// Shows the minimap on commands that move around the strip (focus, swap,
/// resize and the like) or between workspaces.
pub(crate) fn minimap_on_command(
    mut messages: MessageReader<Event>,
    config: Res<Config>,
    mut state: ResMut<MinimapState>,
) {
    let mode = messages.read().fold(None, |mode, event| match event {
        Event::Command {
            command: Command::Window(operation),
        } if switches_workspace(operation) => Some(MinimapMode::Workspaces),
        Event::Command {
            command: Command::Window(operation),
        } if navigates_strip(operation) => mode.or(Some(MinimapMode::Strip)),
        _ => mode,
    });
    if let Some(mode) = mode {
        state.poke(&config, mode);
    }
}

/// Whether `operation` moves between virtual workspaces.
fn switches_workspace(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::Virtual(_)
            | Operation::VirtualNumber(_)
            | Operation::VirtualAdd
            | Operation::VirtualOccupied(_)
            | Operation::FocusOrVirtual(_)
            | Operation::VirtualMove(_, _)
            | Operation::VirtualMoveNumber(_, _)
    )
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
    state.poke(&config, MinimapMode::Strip);
}

/// Ends a strip countdown when the workspace changes some other way than a
/// workspace command (which shows every workspace instead): the old strip is
/// about to go.
pub(crate) fn minimap_on_workspace_switch(
    _switched: Populated<(), Added<ActiveWorkspaceMarker>>,
    mut state: ResMut<MinimapState>,
) {
    if state.timer.is_some() && state.mode == MinimapMode::Strip {
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
            state.mode = MinimapMode::Strip;
        }
    }
}

/// Whether the minimap has anything to draw or take down this frame.
pub(crate) fn minimap_active(state: Res<MinimapState>) -> bool {
    state.pinned || state.timer.is_some() || state.is_changed()
}

/// The strips of the display in front, with whether each is on screen.
type MinimapStrips<'w, 's> = Query<
    'w,
    's,
    (
        &'static LayoutStrip,
        &'static Position,
        Option<&'static RepositionMarker>,
        &'static ChildOf,
        Has<ActiveWorkspaceMarker>,
    ),
>;

/// Draws the minimap for the active strip, or every workspace's strip while
/// switching, or takes it down once it times out.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_minimap(
    strips: MinimapStrips,
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
    let Some((active_strip, _, _, child_of, _)) = strips.iter().find(|strip| strip.4) else {
        return;
    };
    let Ok((display, dock)) = displays.get(child_of.parent()) else {
        return;
    };
    if !state.visible() {
        manager.hide();
        MinimapState::set_on_screen(&mut state, None);
        return;
    }

    let viewport = display.actual_display_bounds(dock, &config);
    let focused = focused.iter().next();
    let workspaces = state.mode == MinimapMode::Workspaces && !state.pinned;
    let mut rows = strips
        .iter()
        .filter(|(strip, _, _, _, active)| *active || workspaces && strip.id() == active_strip.id())
        .map(|(strip, Position(position), reposition, _, active)| {
            let strip_x = reposition.map_or(position.x, |target| target.0.x);
            MinimapRow {
                virtual_index: strip.virtual_index,
                active,
                columns: row_columns(strip, strip_x, viewport, &windows, focused),
            }
        })
        // Workspaces without windows are left out; the current one stays,
        // so it is clear where the switch landed.
        .filter(|row| row.active || !row.columns.is_empty())
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.virtual_index);
    // Nothing to show: an empty strip, even when toggled on.
    if rows.iter().all(|row| row.columns.is_empty()) {
        manager.hide();
        MinimapState::set_on_screen(&mut state, None);
        return;
    }
    let snapshot = MinimapSnapshot {
        rows,
        viewport: (viewport.min.x, viewport.max.x),
        workspaces,
    };

    let edge = display.edge_bounds(dock, &config);
    let bottom_center = NSPoint::new(
        f64::from(edge.min.x + edge.max.x) / 2.0,
        f64::from(edge.max.y),
    );
    let geometry = minimap_geometry(&snapshot, f64::from(edge.width()) * MAX_WIDTH_RATIO);
    manager.show(
        &snapshot,
        &geometry,
        state.opacity(),
        bottom_center,
        config.minimap_interactive(),
    );
    MinimapState::set_on_screen(&mut state, Some(geometry.height));
}

/// The columns of `strip`, offset to `strip_x`, with the pinned window at the
/// edge of `viewport` it is pinned to.
fn row_columns(
    strip: &LayoutStrip,
    strip_x: i32,
    viewport: IRect,
    windows: &Query<(&Window, &Bounds)>,
    focused: Option<Entity>,
) -> Vec<MinimapColumn> {
    let frame = |entity| {
        windows
            .get(entity)
            .ok()
            .map(|(_, Bounds(size))| IRect::new(0, 0, size.x, size.y))
    };
    let mut columns = strip
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
                pinned: false,
            })
        })
        .collect::<Vec<_>>();
    // The pinned window is no column, but it is on screen: it shows at the
    // edge it is pinned to.
    if let Some(pin) = strip.pinned()
        && let Ok((window, Bounds(size))) = windows.get(pin.entity)
    {
        let pinned = MinimapColumn {
            pid: window.pid().ok(),
            x: if pin.east {
                viewport.max.x - size.x
            } else {
                viewport.min.x
            },
            width: size.x,
            focused: focused == Some(pin.entity),
            pinned: true,
        };
        if pin.east {
            columns.push(pinned);
        } else {
            columns.insert(0, pinned);
        }
    }
    columns
}

// ── AppKit ──────────────────────────────────────────────────────────────

/// What clicking a tile does.
#[derive(Clone, Copy)]
enum TileClick {
    /// Focus the column at this place in the current strip.
    Focus(usize),
    /// Switch to this workspace.
    Workspace(u32),
    /// Nothing: the pinned window, which no column number reaches.
    Nothing,
}

struct Tile {
    rect: NSRect,
    icon: Option<Retained<NSImage>>,
    focused: bool,
    dimmed: bool,
    pinned: bool,
    click: TileClick,
}

/// A workspace's row: its number, and whether it is the one highlighted.
struct RowLabel {
    rect: NSRect,
    number: Retained<NSAttributedString>,
    highlighted: bool,
}

struct MinimapViewIvars {
    rows: Vec<RowLabel>,
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

            // The current workspace's row, while switching: the same teal as a
            // focused tile, around the whole row.
            for row in ivars.rows.iter().filter(|row| row.highlighted) {
                let rect = NSRect::new(
                    NSPoint::new(row.rect.origin.x + 1.0, row.rect.origin.y + 1.0),
                    NSSize::new(row.rect.size.width - 2.0, row.rect.size.height - 2.0),
                );
                let radius = TILE_RADIUS + TILE_GAP / 2.0;
                let path =
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
                NSColor::colorWithSRGBRed_green_blue_alpha(FOCUS_RED, FOCUS_GREEN, FOCUS_BLUE, 0.10)
                    .setFill();
                path.fill();
                NSColor::colorWithSRGBRed_green_blue_alpha(FOCUS_RED, FOCUS_GREEN, FOCUS_BLUE, 1.0)
                    .setStroke();
                path.setLineWidth(2.0);
                path.stroke();
            }
            // Each row's workspace number, in the margin at its left.
            for row in &ivars.rows {
                let text_size: NSSize = unsafe { msg_send![&row.number, size] };
                let origin = NSPoint::new(
                    row.rect.origin.x + (LABEL_WIDTH - text_size.width) / 2.0 + 2.0,
                    row.rect.origin.y + (row.rect.size.height - text_size.height) / 2.0,
                );
                let _: () = unsafe { msg_send![&row.number, drawAtPoint: origin] };
            }

            for tile in &ivars.tiles {
                let (fill, border, line_width) = if tile.pinned {
                    (
                        NSColor::colorWithSRGBRed_green_blue_alpha(
                            PIN_RED,
                            PIN_GREEN,
                            PIN_BLUE,
                            if tile.focused { 0.18 } else { 0.08 },
                        ),
                        NSColor::colorWithSRGBRed_green_blue_alpha(PIN_RED, PIN_GREEN, PIN_BLUE, 1.0),
                        if tile.focused { 2.0 } else { 1.5 },
                    )
                } else if tile.focused {
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

                if tile.pinned {
                    draw_pin_lock(tile.rect);
                }

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
                    if tile.dimmed {
                        OFF_SCREEN_ICON_ALPHA
                    } else {
                        1.0
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
            let Some(tile) = ivars.tiles.iter().find(|tile| {
                point.x >= tile.rect.origin.x
                    && point.x <= tile.rect.origin.x + tile.rect.size.width
                    && point.y >= tile.rect.origin.y
                    && point.y <= tile.rect.origin.y + tile.rect.size.height
            }) else {
                return;
            };
            let operation = match tile.click {
                TileClick::Focus(index) => Operation::Focus(Direction::Nth(index)),
                TileClick::Workspace(virtual_index) => Operation::VirtualNumber(virtual_index),
                TileClick::Nothing => return,
            };
            let command = Command::Window(operation);
            if let Err(err) = events.send(Event::Command { command }) {
                tracing::warn!("minimap click: {err}");
            }
        }
    }
);

/// A row's workspace number, one-based, brighter for the current workspace.
fn row_label(virtual_index: u32, active: bool) -> Retained<NSAttributedString> {
    let font = NSFont::monospacedDigitSystemFontOfSize_weight(LABEL_FONT_SIZE, unsafe {
        NSFontWeightSemibold
    });
    let color =
        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, if active { 0.9 } else { 0.45 });
    let keys = [
        &*NSString::from_str("NSFont"),
        &*NSString::from_str("NSColor"),
    ];
    let objects = [&*font as &AnyObject, &*color as &AnyObject];
    let attributes = NSDictionary::from_slices(&keys, &objects);
    unsafe {
        msg_send![
            NSAttributedString::alloc(),
            initWithString: &*NSString::from_str(&(virtual_index + 1).to_string()),
            attributes: &*attributes
        ]
    }
}

/// Draws a small orange lock in the top-right corner of `tile`, marking the
/// pinned window.
fn draw_pin_lock(tile: NSRect) {
    let Some(symbol) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("lock.fill"),
        None,
    ) else {
        return;
    };
    let orange = NSColor::colorWithSRGBRed_green_blue_alpha(PIN_RED, PIN_GREEN, PIN_BLUE, 1.0);
    let colored = NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[orange]),
    );
    let Some(lock) = symbol.imageWithSymbolConfiguration(&colored) else {
        return;
    };
    let natural = lock.size();
    if natural.height <= 0.0 {
        return;
    }
    let size = NSSize::new(
        PIN_LOCK_HEIGHT * natural.width / natural.height,
        PIN_LOCK_HEIGHT,
    );
    let inset = 5.0;
    let rect = NSRect::new(
        NSPoint::new(
            tile.origin.x + tile.size.width - size.width - inset,
            tile.origin.y + tile.size.height - size.height - inset,
        ),
        size,
    );
    lock.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
}

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
        geometry: &MinimapGeometry,
        opacity: f32,
        bottom_center_abs_cg: NSPoint,
        interactive: bool,
    ) {
        let key = (snapshot.clone(), interactive);
        if self.shown.as_ref() == Some(&key) {
            if let Some(panel) = &self.panel {
                panel.setAlphaValue(CGFloat::from(opacity));
            }
            return;
        }

        let tile_height = ROW_HEIGHT - TILE_GAP;
        let mut tiles = Vec::new();
        let mut rows = Vec::new();
        for (row, (bottom, slots)) in snapshot.rows.iter().zip(&geometry.rows) {
            let highlighted = snapshot.workspaces && row.active;
            let mut next_column = 0;
            for (column, &(x, width)) in row.columns.iter().zip(slots) {
                let click = if column.pinned {
                    TileClick::Nothing
                } else if row.active {
                    next_column += 1;
                    TileClick::Focus(next_column - 1)
                } else {
                    TileClick::Workspace(row.virtual_index)
                };
                tiles.push(Tile {
                    rect: NSRect::new(
                        NSPoint::new(x + TILE_GAP / 2.0, bottom + TILE_GAP / 2.0),
                        NSSize::new((width - TILE_GAP).max(1.0), tile_height),
                    ),
                    icon: column.pid.and_then(|pid| self.icon(pid)),
                    // While switching, the row is what is highlighted.
                    focused: column.focused && row.active && !snapshot.workspaces,
                    dimmed: !row.active || !column_on_screen(column, snapshot.viewport),
                    pinned: column.pinned,
                    click,
                });
            }
            let row_width = slots
                .last()
                .map_or(PANEL_PADDING + LABEL_WIDTH + SLOT_WIDTH, |(x, width)| {
                    x + width
                });
            rows.push(RowLabel {
                rect: NSRect::new(
                    NSPoint::new(PANEL_PADDING, *bottom),
                    NSSize::new(row_width - PANEL_PADDING, ROW_HEIGHT),
                ),
                number: row_label(row.virtual_index, row.active),
                highlighted,
            });
        }

        let size = NSSize::new(geometry.width, geometry.height);
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
                rows,
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
        LABEL_WIDTH, MINIMAP_HEIGHT, MinimapColumn, MinimapRow, MinimapSnapshot, PANEL_PADDING,
        ROW_HEIGHT, SLOT_WIDTH, column_on_screen, minimap_geometry,
    };

    fn column(x: i32, width: i32) -> MinimapColumn {
        MinimapColumn {
            pid: None,
            x,
            width,
            focused: false,
            pinned: false,
        }
    }

    fn row(virtual_index: u32, active: bool, columns: Vec<MinimapColumn>) -> MinimapRow {
        MinimapRow {
            virtual_index,
            active,
            columns,
        }
    }

    fn snapshot(rows: Vec<MinimapRow>) -> MinimapSnapshot {
        MinimapSnapshot {
            rows,
            viewport: (0, 1000),
            workspaces: false,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn every_column_gets_one_tile_slot_after_the_label() {
        let snapshot = snapshot(vec![row(
            0,
            true,
            vec![column(-1000, 1000), column(0, 500), column(500, 2000)],
        )]);
        let geometry = minimap_geometry(&snapshot, 10_000.0);
        let (bottom, tiles) = &geometry.rows[0];
        assert!(close(*bottom, PANEL_PADDING));
        for (index, &(x, width)) in tiles.iter().enumerate() {
            assert!(close(width, SLOT_WIDTH));
            assert!(close(
                x,
                PANEL_PADDING + LABEL_WIDTH + f64::from(u8::try_from(index).unwrap()) * SLOT_WIDTH
            ));
        }
        assert!(close(
            geometry.width,
            3.0 * SLOT_WIDTH + LABEL_WIDTH + 2.0 * PANEL_PADDING
        ));
        assert!(
            close(geometry.height, MINIMAP_HEIGHT),
            "one row is the usual height"
        );
    }

    #[test]
    fn rows_stack_from_the_top_and_the_longest_sets_the_width() {
        let snapshot = snapshot(vec![
            row(0, true, vec![column(0, 500)]),
            row(
                2,
                false,
                vec![column(0, 500), column(500, 500), column(1000, 500)],
            ),
        ]);
        let geometry = minimap_geometry(&snapshot, 10_000.0);
        assert!(close(
            geometry.height,
            2.0 * ROW_HEIGHT + 2.0 * PANEL_PADDING
        ));
        // The first row is the top one.
        assert!(close(geometry.rows[0].0, PANEL_PADDING + ROW_HEIGHT));
        assert!(close(geometry.rows[1].0, PANEL_PADDING));
        assert!(close(
            geometry.width,
            3.0 * SLOT_WIDTH + LABEL_WIDTH + 2.0 * PANEL_PADDING
        ));
    }

    #[test]
    fn long_strips_shrink_to_fit() {
        let snapshot = snapshot(vec![row(
            0,
            true,
            (0..40).map(|i| column(i * 1000, 1000)).collect(),
        )]);
        let geometry = minimap_geometry(&snapshot, 800.0);
        assert!(close(geometry.width, 800.0));
        assert!(geometry.rows[0].1[0].1 < SLOT_WIDTH);
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

    #[test]
    fn a_pinned_window_is_always_on_screen() {
        let mut pinned = column(-5000, 500);
        pinned.pinned = true;
        assert!(column_on_screen(&pinned, (0, 1000)));
    }
}
