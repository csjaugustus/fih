use std::collections::HashMap;
use std::time::Duration;

use bevy::app::{App, Plugin, PostUpdate, Update};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::lifecycle::{Add, Remove};
use bevy::ecs::message::{Message, MessageReader, MessageWriter};
use bevy::ecs::observer::On;
use bevy::ecs::query::{Added, Has, With, Without};
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs as _;
use bevy::ecs::system::{Commands, NonSend, Populated, Query, Res, ResMut, Single, SystemParam};
use bevy::math::IRect;
use bevy::prelude::Event as BevyEvent;
use bevy::time::Time;
use bevy::time::common_conditions::on_timer;
use std::pin::Pin;
use tracing::{Level, debug, error, info, instrument, trace, warn};

use super::{FocusedMarker, MouseHeldMarker, SystemTheme, Timeout, Unmanaged, VerifyFocus};
use crate::config::Config;
use crate::ecs::layout::LayoutStrip;
use crate::ecs::params::{ActiveDisplay, GlobalState, WindowCtx, Windows};
use crate::ecs::scroll::{PageGestureState, PageInFlight};
use crate::ecs::workspace::RestoreFocusMarker;
use crate::ecs::{
    ActiveWorkspaceMarker, Bounds, Position, RaiseWindow, RepositionMarker, ReshuffleAroundMarker,
    ResizeMarker, Scrolling, SendMessageTrigger, SpawnCommandsExt, StrayFocusEvent,
    VerifyWindowSize,
};
use crate::events::Event;
use crate::manager::{Application, Display, Origin, Window, WindowManager, origin_from};
use crate::platform::WorkspaceId;
use crate::util::round_px;

const REFRESH_WINDOW_CHECK_FREQ_MS: u64 = 1000;
pub(crate) const VERIFY_FOCUS_TIMEOUT: Duration = Duration::from_millis(300);

type MouseFocusStrips<'w, 's> = Query<
    'w,
    's,
    (
        &'static LayoutStrip,
        &'static ChildOf,
        Option<&'static Scrolling>,
        Has<ActiveWorkspaceMarker>,
        &'static Position,
        Option<&'static RepositionMarker>,
    ),
>;

#[derive(Default)]
pub struct TierMemory {
    pub last_managed: Option<Entity>,
    pub last_floating: Option<Entity>,
}

/// Keyed by `WorkspaceId` so toggling on one Space can't reach a window last
/// focused on another. Cleared on entity despawn (`forget`) so recycled
/// Entity IDs can't resolve to the wrong window, and on workspace despawn
/// (`forget_workspace`) to bound the map.
#[derive(Default, Resource)]
pub struct FocusHistory {
    by_workspace: HashMap<WorkspaceId, TierMemory>,
}

impl FocusHistory {
    pub fn record(
        &mut self,
        workspace: WorkspaceId,
        entity: Entity,
        unmanaged: Option<&Unmanaged>,
    ) {
        let slot = self.by_workspace.entry(workspace).or_default();
        match unmanaged {
            None => slot.last_managed = Some(entity),
            Some(Unmanaged::Floating) => slot.last_floating = Some(entity),
            Some(_) => {}
        }
    }

    pub fn last_managed(&self, workspace: WorkspaceId) -> Option<Entity> {
        self.by_workspace
            .get(&workspace)
            .and_then(|t| t.last_managed)
    }

    pub fn last_floating(&self, workspace: WorkspaceId) -> Option<Entity> {
        self.by_workspace
            .get(&workspace)
            .and_then(|t| t.last_floating)
    }

    pub fn forget(&mut self, entity: Entity) {
        for slot in self.by_workspace.values_mut() {
            if slot.last_managed == Some(entity) {
                slot.last_managed = None;
            }
            if slot.last_floating == Some(entity) {
                slot.last_floating = None;
            }
        }
    }

    pub fn forget_workspace(&mut self, workspace: WorkspaceId) {
        self.by_workspace.remove(&workspace);
    }
}

pub struct FocusEventsPlugin;

#[derive(Message)]
pub(crate) struct PageHapticRequest;

impl Plugin for FocusEventsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FocusHistory>();
        app.init_resource::<PointerPlace>();
        app.init_resource::<KeyboardIntent>();
        app.add_message::<PageHapticRequest>();
        app.add_systems(
            Update,
            (
                detect_focus_rejection.in_set(super::UpdateSet::Events),
                fix_window_size_on_focus.in_set(super::UpdateSet::Focus),
                note_keybinds
                    .in_set(super::UpdateSet::Events)
                    .run_if(bevy::ecs::schedule::common_conditions::on_message::<Event>),
                record_pointer_place
                    .in_set(super::UpdateSet::Layout)
                    .before(super::layout::layout_strip_changed),
                autocenter_window_on_focus
                    .in_set(super::UpdateSet::Layout)
                    .after(super::layout::layout_strip_changed)
                    .before(super::layout::reshuffle_layout_strip),
            ),
        );
        app.add_systems(
            PostUpdate,
            (
                complete_page_focus
                    .in_set(super::PostUpdateSet::Ui)
                    .run_if(|paging: Res<PageGestureState>| paging.in_flight.is_some()),
                play_page_haptic
                    .in_set(super::PostUpdateSet::Ui)
                    .after(complete_page_focus)
                    .run_if(
                        bevy::ecs::schedule::common_conditions::on_message::<PageHapticRequest>,
                    ),
                clear_pending_focus_layout.in_set(super::PostUpdateSet::Bookkeeping),
                mouse_follows_focus.in_set(super::PostUpdateSet::Ui),
                recover_lost_focus
                    .in_set(super::PostUpdateSet::Bookkeeping)
                    .run_if(on_timer(Duration::from_millis(
                        REFRESH_WINDOW_CHECK_FREQ_MS,
                    ))),
            ),
        );
        app.add_observer(dim_remove_window_trigger)
            .add_observer(dim_window_trigger)
            .add_observer(maintain_focus_singleton)
            .add_observer(maintain_verify_focus_singleton)
            .add_observer(virtual_strip_activated)
            .add_observer(stray_focus_observer)
            .add_observer(focus_window_trigger)
            .add_observer(raise_window_trigger);
    }
}

fn complete_page_focus(
    focused: Query<Entity, Added<FocusedMarker>>,
    mut paging: ResMut<PageGestureState>,
    time: Res<Time>,
    config: Res<Config>,
    mut haptics: MessageWriter<PageHapticRequest>,
) {
    let Some(PageInFlight::AwaitingFocus { target, deadline }) = paging.in_flight else {
        return;
    };
    if focused.iter().any(|entity| entity == target) {
        paging.in_flight = None;
        if config.swipe_gesture_haptics() {
            haptics.write(PageHapticRequest);
        }
    } else if let Some(deadline) = deadline {
        if time.elapsed() >= deadline {
            paging.reset();
        }
    } else {
        paging.in_flight = Some(PageInFlight::AwaitingFocus {
            target,
            deadline: Some(time.elapsed() + VERIFY_FOCUS_TIMEOUT),
        });
    }
}

fn play_page_haptic(
    mut requests: MessageReader<PageHapticRequest>,
    platform: Option<NonSend<Pin<Box<crate::platform::PlatformCallbacks>>>>,
) {
    for _ in requests.read() {
        if let Some(platform) = &platform {
            crate::platform::haptics::page_changed(platform.main_thread_marker);
        }
    }
}

#[cfg(test)]
mod paging_tests {
    use super::*;
    use bevy::MinimalPlugins;
    use bevy::ecs::message::{MessageCursor, Messages};

    #[test]
    fn delayed_page_confirmation_requests_one_haptic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<PageGestureState>();
        app.insert_resource(Config::default());
        app.add_message::<PageHapticRequest>();
        app.add_systems(PostUpdate, complete_page_focus);

        let target = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<PageGestureState>().in_flight =
            Some(PageInFlight::AwaitingFocus {
                target,
                deadline: Some(Duration::from_secs(1)),
            });
        app.update();
        assert!(
            app.world()
                .resource::<PageGestureState>()
                .in_flight
                .is_some()
        );

        app.world_mut().entity_mut(target).insert(FocusedMarker);
        app.update();
        assert!(
            app.world()
                .resource::<PageGestureState>()
                .in_flight
                .is_none()
        );
        let mut cursor = MessageCursor::<PageHapticRequest>::default();
        assert_eq!(
            cursor
                .read(app.world().resource::<Messages<PageHapticRequest>>())
                .count(),
            1
        );
        app.update();
        assert_eq!(
            cursor
                .read(app.world().resource::<Messages<PageHapticRequest>>())
                .count(),
            0
        );
    }

    #[test]
    fn timed_out_page_confirmation_requests_no_haptic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<PageGestureState>();
        app.insert_resource(Config::default());
        app.add_message::<PageHapticRequest>();
        app.add_systems(PostUpdate, complete_page_focus);

        let target = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<PageGestureState>().in_flight =
            Some(PageInFlight::AwaitingFocus {
                target,
                deadline: Some(Duration::ZERO),
            });
        app.update();
        assert!(
            app.world()
                .resource::<PageGestureState>()
                .in_flight
                .is_none()
        );
        let mut cursor = MessageCursor::<PageHapticRequest>::default();
        assert_eq!(
            cursor
                .read(app.world().resource::<Messages<PageHapticRequest>>())
                .count(),
            0
        );
    }
}

#[derive(BevyEvent)]
pub(super) struct FocusWindow {
    pub entity: Entity,
    pub raise: bool,
}

/// Protects command focus until its pending layout has queued movement targets.
#[derive(Component)]
pub(super) struct PendingFocusLayout;

fn clear_pending_focus_layout(
    pending: Populated<Entity, (With<PendingFocusLayout>, Without<ReshuffleAroundMarker>)>,
    mut commands: Commands,
) {
    for entity in pending {
        commands.entity(entity).try_remove::<PendingFocusLayout>();
    }
}

#[instrument(level = Level::DEBUG, skip_all, fields(trigger))]
fn maintain_focus_singleton(
    trigger: On<Add, FocusedMarker>,
    windows: Query<(Entity, Has<FocusedMarker>), With<Window>>,
    mut config: GlobalState,
    mut commands: Commands,
) {
    let focused_entity = trigger.event().entity;

    for (entity, focused) in windows {
        if focused
            && entity != focused_entity
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            debug!("window {entity} lost focus.");
            entity_commands.try_remove::<FocusedMarker>();
        }
    }

    // Check if the reshuffle was caused by a keyboard switch or mouse move.
    // Skip reshuffle if caused by mouse - because then it won't center.
    if config.ffm_flag().is_none() {
        config.set_skip_reshuffle(false);
        commands
            .entity(focused_entity)
            .try_insert(PendingFocusLayout);
    }
    config.set_ffm_flag(None);
}

/// Whether two windows are members of one native tab group: the app shows one
/// of them at a time, and focusing any of them can leave the focus on the one
/// the app decided to show.
///
/// The strip knows the ones it has already grouped. The rest are recognised the
/// same way [`super::systems::detect_tabbed_windows`] recognises them in the
/// first place: same app, same frame.
fn shares_a_tab_group(
    workspaces: &Query<(Entity, &mut LayoutStrip)>,
    windows: &Windows,
    target: Entity,
    actual: Entity,
) -> bool {
    if workspaces.iter().any(|(_, strip)| {
        strip
            .tab_group(target)
            .is_some_and(|group| group.contains(&actual))
    }) {
        return true;
    }

    let parent_of = |entity: Entity| {
        windows
            .get(entity)
            .and_then(|window| windows.find_parent(window.id()))
            .map(|(_, _, parent)| parent)
    };
    let (Some(target_app), Some(actual_app)) = (parent_of(target), parent_of(actual)) else {
        return false;
    };
    if target_app != actual_app {
        return false;
    }

    windows
        .frame(target)
        .zip(windows.frame(actual))
        .is_some_and(|(target_frame, actual_frame)| {
            target_frame.min.chebyshev_distance(actual_frame.min) <= 1
                && target_frame.size().chebyshev_distance(actual_frame.size()) <= 1
        })
}

#[instrument(level = Level::DEBUG, skip_all, fields(focused))]
pub(super) fn fix_window_size_on_focus(
    focused: Single<(Entity, Has<VerifyWindowSize>), Added<FocusedMarker>>,
    mut windows: Query<(&mut Window, &mut Bounds, Has<ResizeMarker>)>,
    mut commands: Commands,
) {
    let (entity, verify_size) = *focused;
    if !verify_size {
        return;
    }
    if let Ok(mut entity_commands) = commands.get_entity(entity) {
        entity_commands.try_remove::<VerifyWindowSize>();
    }
    if let Ok((mut window, mut bounds, resizing)) = windows.get_mut(entity)
        && !resizing
        && let Ok(frame) = window.update_frame()
        && frame.size() != bounds.0
    {
        debug!("fixing window {} size!", window.id());
        bounds.0 = frame.size();
    }
}

/// Ensures at most one window entity carries [`VerifyFocus`] at any time, so
/// rapid focus switching cancels verification on intermediate windows.
#[instrument(level = Level::DEBUG, skip_all, fields(trigger))]
fn maintain_verify_focus_singleton(
    trigger: On<Add, VerifyFocus>,
    verifying: Query<Entity, With<VerifyFocus>>,
    mut commands: Commands,
) {
    let target = trigger.event().entity;
    for entity in verifying {
        if entity != target
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            entity_commands.try_remove::<(Timeout, VerifyFocus)>();
        }
    }
}

#[instrument(level = Level::DEBUG, skip_all)]
fn detect_focus_rejection(
    pending: Populated<(Entity, &Window, &ChildOf, &Timeout), With<VerifyFocus>>,
    apps: Query<&Application>,
    mut workspaces: Query<(Entity, &mut LayoutStrip)>,
    windows: Windows,
    mut commands: Commands,
) {
    for (target_entity, target_window, child_of, timeout) in &pending {
        if !timeout.timer.is_finished() {
            continue;
        }
        let Ok(app) = apps.get(child_of.parent()) else {
            continue;
        };
        // Only treat as a rejection if the target window's application is
        // currently frontmost and reports a different window of the same app
        // as focused.
        if !app.is_frontmost() {
            continue;
        }
        let Ok(actual_window_id) = app.focused_window_id() else {
            continue;
        };
        if actual_window_id == target_window.id() {
            continue;
        }
        let Some((actual_window, actual_entity, actual_parent)) =
            windows.find_parent(actual_window_id)
        else {
            continue;
        };
        if actual_parent != child_of.parent() {
            continue;
        }
        // If focus in the ECS world has already moved to a third window (neither
        // `target_entity` nor `actual_entity`), do not treat it as a rejection.
        if windows
            .focused()
            .is_some_and(|(_, focused)| focused != target_entity && focused != actual_entity)
        {
            continue;
        }

        // Native tabs share one slot: asking for a background tab makes the app
        // select it, and the focus notification names whichever tab of the group
        // the app ended up showing. That is the app doing what was asked, not
        // refusing it — floating the window here is how a tabbed terminal ends up
        // scattered across the layout as windows nothing tiles.
        if shares_a_tab_group(&workspaces, &windows, target_entity, actual_entity) {
            debug!(
                "focus landed on tab sibling {actual_entity} of {target_entity}; not a rejection.",
            );
            continue;
        }

        info!(
            "focus rejection detected: requested window {} ({target_entity}), app focused window {} ({actual_entity}) instead; floating {target_entity}.",
            target_window.id(),
            actual_window.id(),
        );
        if let Ok(mut entity_commands) = commands.get_entity(target_entity) {
            entity_commands
                .try_remove::<(Timeout, VerifyFocus)>()
                .try_insert(Unmanaged::Floating);
        }
        for (_, mut strip) in &mut workspaces {
            if strip.contains(target_entity) {
                strip.remove(target_entity);
            }
        }
        commands.focus_entity(actual_entity, false);
    }
}

#[instrument(level = Level::DEBUG, skip_all, fields(trigger))]
fn autocenter_window_on_focus(
    focused: Single<Entity, Added<FocusedMarker>>,
    mouse_held: Query<&MouseHeldMarker>,
    restored: Query<&RestoreFocusMarker>,
    global_state: GlobalState,
    active_display: ActiveDisplay,
    mut ctx: WindowCtx,
) {
    let entity = *focused;

    // Skip auto-centering when this focus came from a workspace restore, since
    // the strip is already at its saved origin. window_focused_trigger and
    // timeout_ticker are responsible for clearing the marker.
    if restored.iter().any(|marker| marker.entity == entity) {
        return;
    }

    if global_state.skip_reshuffle() || global_state.initializing() || !mouse_held.is_empty() {
        return;
    }
    if active_display.active_strip().tabbed(entity) {
        return;
    }
    ctx.commands.reshuffle_around(entity);
}

#[instrument(level = Level::DEBUG, skip_all, fields(trigger))]
#[allow(clippy::too_many_arguments)]
fn mouse_follows_focus(
    focused: Single<Entity, Added<FocusedMarker>>,
    windows: Windows,
    global_state: GlobalState,
    config: Res<Config>,
    window_manager: Res<WindowManager>,
    displays: Query<&Display>,
    workspaces: MouseFocusStrips,
    restored: Query<&RestoreFocusMarker>,
    mut place: ResMut<PointerPlace>,
    keyboard: KeyboardNavigation,
) {
    let entity = *focused;
    let recorded = place
        .0
        .take()
        .filter(|(focused, _)| *focused == entity)
        .map(|(_, fraction)| fraction);
    // Only focus moved from the keyboard brings the pointer along: closing a
    // window, clicking, switching apps or using the menu leaves it alone.
    if !keyboard.recent() {
        return;
    }
    let Some(window) = windows.get(entity) else {
        return;
    };
    if workspaces
        .iter()
        .find_map(|(_, _, scrolling, active, _, _)| if active { scrolling } else { None })
        .is_some_and(|scrolling| scrolling.is_user_swiping)
    {
        debug!("Suppressing center mouse due to a swipe");
        return;
    }

    trace!(
        "window {}, skip_reshuffle {}, ffm flag {:?}.",
        window.id(),
        global_state.skip_reshuffle(),
        global_state.ffm_flag()
    );
    // A focus change the pointer caused, such as a click into the window,
    // leaves the pointer where it is: it is already on the window.
    let pointer = window_manager.cursor_position().map(origin_from);
    let pointer_on_window = windows
        .frame(entity)
        .zip(pointer)
        .is_some_and(|(frame, pointer)| frame.contains(pointer));

    if config.mouse_follows_focus()
        && !pointer_on_window
        && !global_state.skip_reshuffle()
        && global_state.ffm_flag().is_none_or(|id| id != window.id())
        && let Some(frame) = windows.moving_frame(entity)
        && let Some((_, child, _, _, position, reposition)) = workspaces
            .into_iter()
            .find(|(strip, _, _, _, _, _)| strip.contains(entity))
        && let Ok(display) = displays.get(child.parent())
    {
        // Project the window's slot onto the strip destination when the window
        // does not carry its own auto-center target (workspace restore or
        // auto-center disabled), since the strip moves before its members are
        // laid out on a following tick.
        let frame = if (restored.iter().any(|marker| marker.entity == entity)
            || !config.auto_center())
            && let Some(layout) = windows.layout_position(entity)
        {
            let origin = layout.0 + reposition.map_or(position.0, |target| target.0);
            IRect::from_corners(origin, origin + frame.size())
        } else {
            frame
        };
        // A window off screen is about to slide into view; aim at where it
        // will be, not at the sliver showing now.
        let frame = settle_into_view(frame, display.bounds());
        let visible = display.bounds().intersect(frame);
        if visible.size().length_squared() > 5000 {
            let origin = pointer.map_or_else(
                || visible.center(),
                |pointer| {
                    // Recorded before the strip moved, when there was a
                    // chance to; otherwise read off the windows as they are.
                    let fraction = recorded
                        .unwrap_or_else(|| pointer_place(pointer, entity, &windows, &workspaces));
                    follow_pointer(pointer, fraction, frame, visible)
                },
            );
            debug!("moving the pointer to {} {origin}", window.id());
            window_manager.warp_mouse(origin);
        }
    }
}

/// Where `frame` comes to rest once the strip slides it into `viewport`: just
/// far enough to be fully visible, flush with the edge it came in from, the
/// way focusing a window off screen brings it into view. A window wider than
/// the viewport lines up with its left edge.
pub(crate) fn settle_into_view(frame: IRect, viewport: IRect) -> IRect {
    let shift = if frame.width() >= viewport.width() || frame.min.x < viewport.min.x {
        viewport.min.x - frame.min.x
    } else if frame.max.x > viewport.max.x {
        viewport.max.x - frame.max.x
    } else {
        0
    };
    IRect::from_corners(
        frame.min + Origin::new(shift, 0),
        frame.max + Origin::new(shift, 0),
    )
}

/// Where on a window the pointer was when focus moved to another one, as
/// fractions of its width and height, recorded before the strip starts to
/// slide; `None` when it was on no window.
#[derive(Debug, Default, Resource)]
pub struct PointerPlace(Option<(Entity, Option<(f64, f64)>)>);

/// When a fih key binding was last pressed, so the focus changes it leads to
/// can bring the pointer along.
#[derive(Debug, Default, Resource)]
pub struct KeyboardIntent(Option<Duration>);

/// How long after a key binding a focus change still counts as its doing.
const KEYBOARD_INTENT_WINDOW: Duration = Duration::from_millis(500);

/// Whether a fih key binding was pressed just now.
#[derive(SystemParam)]
struct KeyboardNavigation<'w> {
    intent: Res<'w, KeyboardIntent>,
    clock: Res<'w, Time>,
}

impl KeyboardNavigation<'_> {
    /// Every focus change shortly after the key counts: one key can settle
    /// focus over a few steps (across displays, onto a restored window).
    fn recent(&self) -> bool {
        self.intent.0.is_some_and(|pressed| {
            self.clock.elapsed().saturating_sub(pressed) <= KEYBOARD_INTENT_WINDOW
        })
    }
}

/// Notes key binding presses for [`mouse_follows_focus`].
fn note_keybinds(
    mut messages: MessageReader<Event>,
    clock: Res<Time>,
    mut intent: ResMut<KeyboardIntent>,
) {
    if messages
        .read()
        .any(|event| matches!(event, Event::KeybindPressed))
    {
        intent.0 = Some(clock.elapsed());
    }
}

/// Records where on its window the pointer is as focus moves, while every
/// window is still where the user last saw it.
fn record_pointer_place(
    focused: Single<Entity, Added<FocusedMarker>>,
    windows: Windows,
    workspaces: MouseFocusStrips,
    window_manager: Res<WindowManager>,
    config: Res<Config>,
    mut place: ResMut<PointerPlace>,
) {
    if !config.mouse_follows_focus() {
        return;
    }
    let entity = *focused;
    let fraction = window_manager
        .cursor_position()
        .map(origin_from)
        .and_then(|pointer| pointer_place(pointer, entity, &windows, &workspaces));
    place.0 = Some((entity, fraction));
}

/// Where `pointer` is on whichever window of the active strip it is on,
/// other than `focused`.
fn pointer_place(
    pointer: Origin,
    focused: Entity,
    windows: &Windows,
    workspaces: &MouseFocusStrips,
) -> Option<(f64, f64)> {
    workspaces
        .iter()
        .filter(|(_, _, _, active, _, _)| *active)
        .flat_map(|(strip, _, _, _, _, _)| strip.all_windows())
        .filter(|other| *other != focused)
        .filter_map(|other| windows.frame(other))
        .find_map(|frame| place_on(pointer, frame))
}

/// Where `pointer` is on `frame`, as fractions of its width and height, if
/// it is on it at all.
pub(crate) fn place_on(pointer: Origin, frame: IRect) -> Option<(f64, f64)> {
    (frame.contains(pointer) && frame.width() > 0 && frame.height() > 0).then(|| {
        (
            f64::from(pointer.x - frame.min.x) / f64::from(frame.width()),
            f64::from(pointer.y - frame.min.y) / f64::from(frame.height()),
        )
    })
}

/// Where the pointer goes when focus moves to a window at `to`, of which
/// `visible` is on screen. A pointer that was on a window (at `fraction` of
/// it) keeps that place on the new one - a third of the way across stays a
/// third of the way across - and anywhere else it moves just far enough to
/// be on the new window. Either way it lands on the visible part.
pub(crate) fn follow_pointer(
    pointer: Origin,
    fraction: Option<(f64, f64)>,
    to: IRect,
    visible: IRect,
) -> Origin {
    // A pixel in from the edge, so the pointer is on the window, not its border.
    const INSET: i32 = 1;
    let target = match fraction {
        Some((across, down)) => Origin::new(
            to.min.x + round_px(across * f64::from(to.width())),
            to.min.y + round_px(down * f64::from(to.height())),
        ),
        None => pointer,
    };
    let max = (visible.max - Origin::splat(INSET)).max(visible.min);
    target.clamp((visible.min + Origin::splat(INSET)).min(max), max)
}

fn dim_window_trigger(
    trigger: On<Add, FocusedMarker>,
    windows: Windows,
    window_manager: Res<WindowManager>,
    config: Res<Config>,
    theme: Option<Res<SystemTheme>>,
) {
    let Some(window) = windows.get(trigger.event().entity) else {
        return;
    };

    let dark = theme.is_some_and(|theme| theme.is_dark);
    if config.window_dim_ratio(dark).is_some() {
        window_manager.dim_windows(&[window.id()], 0.0);
    }
}

fn dim_remove_window_trigger(
    trigger: On<Remove, FocusedMarker>,
    windows: Windows,
    active_display: ActiveDisplay,
    window_manager: Res<WindowManager>,
    config: Res<Config>,
    theme: Option<Res<SystemTheme>>,
) {
    let Some((window, _, None)) = windows.get_managed(trigger.event().entity) else {
        return;
    };

    let same_display = active_display
        .active_strip()
        .contains(trigger.event().entity);
    if !same_display {
        // Do not dim the window loosing focus on another display.
        return;
    }

    let dark = theme.is_some_and(|theme| theme.is_dark);
    if let Some(dim_ratio) = config.window_dim_ratio(dark) {
        window_manager.dim_windows(&[window.id()], dim_ratio);
    }
}

#[instrument(level = Level::DEBUG, skip_all, fields(trigger))]
fn virtual_strip_activated(
    trigger: On<Add, FocusedMarker>,
    workspaces: Query<(Entity, &LayoutStrip, Has<ActiveWorkspaceMarker>)>,
    mut commands: Commands,
) {
    let owner_strip = workspaces.into_iter().find_map(|(entity, strip, active)| {
        (strip.contains(trigger.entity) && !active).then_some(entity)
    });
    if let Some(entity) = owner_strip
        && let Ok(mut entity_commands) = commands.get_entity(entity)
    {
        entity_commands.try_insert(ActiveWorkspaceMarker);
    }
}

fn focus_window_trigger(
    trigger: On<FocusWindow>,
    windows: Windows,
    apps: Query<&Application>,
    verifying: Query<Entity, With<VerifyFocus>>,
    mut commands: Commands,
) {
    let FocusWindow { entity, raise } = *trigger.event();
    for other in verifying {
        if other != entity
            && let Ok(mut entity_commands) = commands.get_entity(other)
        {
            entity_commands.try_remove::<(Timeout, VerifyFocus)>();
        }
    }
    let Some(window) = windows.get(entity) else {
        return;
    };
    let Some(psn) = windows.psn(window.id(), &apps) else {
        return;
    };
    if !raise
        && let Some((focused_window, _)) = windows.focused()
        && let Some(focused_psn) = windows.psn(focused_window.id(), &apps)
    {
        window.focus_without_raise(psn, focused_window, focused_psn);
    } else {
        window.focus_with_raise(psn);
    }
}

fn raise_window_trigger(
    trigger: On<RaiseWindow>,
    windows: Query<(Entity, &Window, &Position, &Bounds)>,
    active_display: ActiveDisplay,
    config: Res<Config>,
) {
    let RaiseWindow { entity, with_strip } = *trigger.event();

    let Ok((focus, window, _, _)) = windows.get(entity) else {
        return;
    };

    if with_strip {
        let viewport = active_display.actual_bounds(&config);
        let strip = active_display.active_strip();
        strip
            .all_windows()
            .into_iter()
            .filter_map(|entity| {
                if entity == focus {
                    None
                } else {
                    windows.get(entity).ok()
                }
            })
            .filter(|(_, _, origin, size)| {
                let frame = IRect::from_corners(origin.0, origin.0 + size.0);
                viewport.intersect(frame).width() > 50
            })
            .for_each(|(_, window, _, _)| {
                window.raise_without_focus();
            });
    }

    // Raise the focused window last, because raised windows get OS focus events.
    window.raise_without_focus();
}

#[instrument(level = Level::DEBUG, skip_all)]
fn recover_lost_focus(
    windows: Windows,
    active_workspace: Query<&LayoutStrip, With<ActiveWorkspaceMarker>>,
    mut commands: Commands,
) {
    if windows.focused().is_some() {
        return;
    }
    if let Ok(strip) = active_workspace
        .single()
        .inspect_err(|err| error!("Unable to get current workspace: {err}"))
        && let Some(entity) = strip.first().ok().and_then(|col| col.top())
    {
        error!("Lost focus marker, recovering!");
        commands.focus_entity(entity, false);
    }
}

pub(super) fn stray_focus_observer(
    trigger: On<Add, Window>,
    focus_events: Populated<(Entity, &StrayFocusEvent)>,
    windows: Windows,
    mut commands: Commands,
) {
    let entity = trigger.event().entity;
    let Some(window_id) = windows.get(entity).map(|window| window.id()) else {
        return;
    };

    focus_events
        .iter()
        .filter(|(_, stray_focus)| stray_focus.0 == window_id)
        .for_each(|(timeout_entity, _)| {
            debug!("Re-queueing lost focus event for window id {window_id}.");
            commands.trigger(SendMessageTrigger(Event::WindowFocused { window_id }));
            if let Ok(mut entity_commands) = commands.get_entity(timeout_entity) {
                entity_commands.try_despawn();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::MockWindowApi;
    use crate::manager::app::MockApplicationApi;
    use crate::platform::WinID;
    use bevy::ecs::world::World;

    fn spawn_app(world: &mut World, frontmost: bool, focused_id: WinID) -> Entity {
        let mut api = MockApplicationApi::new();
        api.expect_is_frontmost().returning(move || frontmost);
        api.expect_focused_window_id()
            .returning(move || Ok(focused_id));
        world.spawn(Application::new(Box::new(api))).id()
    }

    fn spawn_window(world: &mut World, app: Entity, id: WinID) -> Entity {
        let mut api = MockWindowApi::new();
        api.expect_id().returning(move || id);
        world
            .spawn((
                Window::new(Box::new(api)),
                Position(bevy::math::IVec2::ZERO),
                Bounds(bevy::math::IVec2::new(800, 600)),
                ChildOf(app),
            ))
            .id()
    }

    fn expired_verify_timeout() -> Timeout {
        let mut timeout = Timeout::for_component::<VerifyFocus>(VERIFY_FOCUS_TIMEOUT);
        timeout.timer.tick(VERIFY_FOCUS_TIMEOUT);
        timeout
    }

    #[test]
    fn record_and_read_per_tier() {
        let mut world = World::new();
        let managed = world.spawn(()).id();
        let floating = world.spawn(()).id();
        let mut history = FocusHistory::default();

        history.record(1, managed, None);
        history.record(1, floating, Some(&Unmanaged::Floating));

        assert_eq!(history.last_managed(1), Some(managed));
        assert_eq!(history.last_floating(1), Some(floating));
    }

    #[test]
    fn record_ignores_minimized_and_hidden() {
        let mut world = World::new();
        let entity = world.spawn(()).id();
        let mut history = FocusHistory::default();

        history.record(1, entity, Some(&Unmanaged::Minimized));
        history.record(1, entity, Some(&Unmanaged::Hidden));

        assert_eq!(history.last_managed(1), None);
        assert_eq!(history.last_floating(1), None);
    }

    #[test]
    fn per_workspace_isolation() {
        let mut world = World::new();
        let a = world.spawn(()).id();
        let b = world.spawn(()).id();
        let mut history = FocusHistory::default();

        history.record(1, a, None);
        history.record(2, b, None);

        assert_eq!(history.last_managed(1), Some(a));
        assert_eq!(history.last_managed(2), Some(b));
    }

    #[test]
    fn forget_clears_entity_across_workspaces() {
        let mut world = World::new();
        let target = world.spawn(()).id();
        let other = world.spawn(()).id();
        let mut history = FocusHistory::default();

        history.record(1, target, None);
        history.record(2, target, Some(&Unmanaged::Floating));
        history.record(2, other, None);

        history.forget(target);

        assert_eq!(history.last_managed(1), None);
        assert_eq!(history.last_floating(2), None);
        assert_eq!(history.last_managed(2), Some(other));
    }

    #[test]
    fn forget_workspace_drops_entry() {
        let mut world = World::new();
        let entity = world.spawn(()).id();
        let mut history = FocusHistory::default();

        history.record(1, entity, None);
        history.forget_workspace(1);

        assert_eq!(history.last_managed(1), None);
    }

    /// Native tabs share a slot: the app answering with a sibling of the tab
    /// group is it doing what was asked, so the requested window must keep its
    /// place in the layout.
    #[test]
    fn focus_landing_on_a_tab_sibling_is_not_a_rejection() {
        let mut world = World::new();
        let app = spawn_app(&mut world, true, 2);
        let target = spawn_window(&mut world, app, 1);
        let sibling = spawn_window(&mut world, app, 2);

        let mut strip = LayoutStrip::default();
        strip.append(target);
        strip
            .convert_to_tabs(target, sibling)
            .expect("target is in the strip");
        world.spawn(strip);

        world
            .entity_mut(target)
            .insert((FocusedMarker, VerifyFocus, expired_verify_timeout()));

        let system_id = world.register_system(detect_focus_rejection);
        _ = world.run_system(system_id);

        assert!(
            world.get::<Unmanaged>(target).is_none(),
            "a tab sibling taking the focus must not float the requested tab"
        );
        let mut strips = world.query::<&LayoutStrip>();
        assert!(
            strips.single(&world).expect("one strip").contains(target),
            "and must not take it out of the layout"
        );
    }

    #[test]
    fn focus_rejection_floats_target_and_clears_verify_focus() {
        let mut world = World::new();
        let app = spawn_app(&mut world, true, 2);
        let target = spawn_window(&mut world, app, 1);
        let actual = spawn_window(&mut world, app, 2);
        // Move actual so it does not share a frame with target (not a tab group).
        world
            .entity_mut(actual)
            .insert(Position(bevy::math::IVec2::new(800, 0)));

        let mut strip = LayoutStrip::default();
        strip.append(target);
        strip.append(actual);
        world.spawn(strip);

        world
            .entity_mut(target)
            .insert((FocusedMarker, VerifyFocus, expired_verify_timeout()));

        let system_id = world.register_system(detect_focus_rejection);
        _ = world.run_system(system_id);

        assert!(
            world
                .get::<Unmanaged>(target)
                .is_some_and(|u| matches!(u, Unmanaged::Floating))
        );
        assert!(world.get::<VerifyFocus>(target).is_none());
        assert!(world.get::<Timeout>(target).is_none());
    }

    #[test]
    fn rapid_switching_cancels_verify_focus_on_previous_window() {
        let mut world = World::new();
        world.add_observer(maintain_verify_focus_singleton);

        let app = spawn_app(&mut world, true, 2);
        let first = spawn_window(&mut world, app, 1);
        let second = spawn_window(&mut world, app, 2);

        world
            .entity_mut(first)
            .insert((VerifyFocus, expired_verify_timeout()));
        world
            .entity_mut(second)
            .insert((VerifyFocus, expired_verify_timeout()));
        world.flush();

        assert!(
            world.get::<VerifyFocus>(first).is_none(),
            "arming VerifyFocus on a new window must cancel it on the previous window"
        );
        assert!(world.get::<Timeout>(first).is_none());
        assert!(world.get::<VerifyFocus>(second).is_some());
    }
}

#[cfg(test)]
mod follow_pointer_tests {
    use bevy::math::IRect;

    use super::{follow_pointer, place_on};
    use crate::manager::Origin;

    const SCREEN: IRect = IRect {
        min: Origin::new(0, 0),
        max: Origin::new(2000, 1000),
    };

    #[test]
    fn keeps_the_pointer_place_within_the_window() {
        let from = IRect::new(0, 0, 1000, 1000);
        let to = IRect::new(1000, 0, 2000, 1000);
        // 30% across, 70% down stays 30% across, 70% down.
        assert_eq!(
            follow_pointer(
                Origin::new(300, 700),
                place_on(Origin::new(300, 700), from),
                to,
                SCREEN
            ),
            Origin::new(1300, 700)
        );
    }

    #[test]
    fn scales_between_windows_of_different_widths() {
        let from = IRect::new(0, 0, 500, 1000);
        let to = IRect::new(500, 0, 1500, 1000);
        // Half way across a narrow window is half way across a wide one.
        assert_eq!(
            follow_pointer(
                Origin::new(250, 500),
                place_on(Origin::new(250, 500), from),
                to,
                SCREEN
            ),
            Origin::new(1000, 500)
        );
    }

    #[test]
    fn a_pointer_off_the_old_window_moves_just_onto_the_new_one() {
        let from = IRect::new(0, 0, 990, 1000);
        let to = IRect::new(1000, 0, 2000, 1000);
        let visible = SCREEN.intersect(to);
        // In the gap between the two: one step right, onto the new window.
        assert_eq!(
            follow_pointer(
                Origin::new(995, 400),
                place_on(Origin::new(995, 400), from),
                to,
                visible
            ),
            Origin::new(1001, 400)
        );
        // Already level with it vertically, nothing else changes.
        assert_eq!(
            follow_pointer(Origin::new(-50, 400), None, to, visible),
            Origin::new(1001, 400)
        );
    }

    #[test]
    fn a_window_off_screen_settles_flush_with_the_edge_it_comes_from() {
        let screen = IRect::new(0, 0, 2000, 1000);
        // Off to the right: comes in flush with the right edge.
        assert_eq!(
            super::settle_into_view(IRect::new(2500, 0, 3500, 1000), screen),
            IRect::new(1000, 0, 2000, 1000)
        );
        // Off to the left: flush with the left edge.
        assert_eq!(
            super::settle_into_view(IRect::new(-1500, 0, -500, 1000), screen),
            IRect::new(0, 0, 1000, 1000)
        );
        // Already in view: stays put.
        assert_eq!(
            super::settle_into_view(IRect::new(500, 0, 1500, 1000), screen),
            IRect::new(500, 0, 1500, 1000)
        );
    }

    #[test]
    fn keeps_the_pointer_place_in_a_window_coming_from_off_screen() {
        let from = IRect::new(0, 0, 1000, 1000);
        let to = super::settle_into_view(IRect::new(2500, 0, 3500, 1000), SCREEN);
        assert_eq!(
            follow_pointer(
                Origin::new(300, 700),
                place_on(Origin::new(300, 700), from),
                to,
                SCREEN.intersect(to)
            ),
            Origin::new(1300, 700)
        );
    }

    #[test]
    fn lands_on_the_visible_part_of_the_window() {
        let from = IRect::new(0, 0, 1000, 1000);
        // Mostly off the right edge of the screen.
        let to = IRect::new(1800, 0, 2800, 1000);
        let visible = SCREEN.intersect(to);
        let landed = follow_pointer(
            Origin::new(500, 500),
            place_on(Origin::new(500, 500), from),
            to,
            visible,
        );
        assert!(visible.contains(landed), "{landed:?} should be on screen");
        assert_eq!(landed.y, 500);
    }
}
