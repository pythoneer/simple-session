//! `apply`: put already open windows where a session says they belong.
//!
//! Windows are placed in ascending workspace order (with dynamic workspaces
//! GNOME only appends a workspace when the trailing empty one gets a window).
//!
//! Two phases. First geometry, with every window unmaximized and visible:
//! after each pass the windows are read back and ones that are off are placed
//! again (when a client adjusts the requested size, Mutter drops the position,
//! so a second pass is normal). Then states (maximized, fullscreen,
//! minimized, above): Wayland clients resize asynchronously, and maximizing
//! before the new size is committed would make Mutter remember the old size
//! as the one to unmaximize to.

use std::time::Duration;

use anyhow::Result;

use crate::layout::{self, MapReason, MonitorMapping};
use crate::matching::{self, SlotRef};
use crate::model::{Maximized, Monitor, Session};
use crate::shim::{Placement, Shim, ShimWindow};

const MAX_PASSES: u32 = 3;
const SETTLE: Duration = Duration::from_millis(300);
const TOLERANCE: i32 = 2;

pub struct PlannedWindow {
    pub slot: SlotRef,
    pub window: ShimWindow,
    pub monitor: usize,
    pub placement: Placement,
    /// Pass in which the window ended up in place; `None` if it never did.
    pub done_in_pass: Option<u32>,
    /// The window as last read back, to report where a failed one ended up.
    pub last_seen: Option<ShimWindow>,
}

pub struct Plan {
    pub windows: Vec<PlannedWindow>,
    pub unmatched_slots: Vec<SlotRef>,
    pub current_monitors: Vec<Monitor>,
    /// Per saved monitor: (current index, reason).
    pub monitor_targets: Vec<(usize, MapReason)>,
}

pub fn plan(
    session: &Session,
    windows: Vec<ShimWindow>,
    current_monitors: Vec<Monitor>,
    dynamic_workspaces: bool,
) -> Plan {
    let mapping = MonitorMapping::new(&session.monitors, &current_monitors);
    let matching = matching::match_windows(session, &windows);
    let slot = |(a, s): SlotRef| &session.apps[a].windows[s];

    let explicit_workspace = |r: SlotRef| {
        let w = slot(r);
        (!w.on_all_workspaces && w.workspace >= 0).then_some(w.workspace)
    };
    let compact =
        layout::compact_workspaces(matching.pairs.iter().filter_map(|&(r, _)| explicit_workspace(r)));
    let workspace = |ws: i32| if dynamic_workspaces { compact(ws) } else { ws };

    let mut planned: Vec<PlannedWindow> = matching
        .pairs
        .iter()
        .map(|&(r, w)| {
            let s = slot(r);
            let primary = current_monitors.iter().position(|m| m.primary).unwrap_or(0);
            let (monitor, reason) =
                mapping.target(&session.monitors, &s.monitor_key).unwrap_or((primary, MapReason::Primary));
            let saved_monitor = session.monitors.iter().find(|m| m.key == s.monitor_key);
            let rect = layout::target_rect(s, saved_monitor, &current_monitors[monitor], reason);

            let (workspace, on_all_workspaces) = if layout::implicit_sticky(session, s) {
                (None, None)
            } else if s.on_all_workspaces {
                (None, Some(true))
            } else {
                (explicit_workspace(r).map(workspace), None)
            };

            PlannedWindow {
                slot: r,
                window: windows[w].clone(),
                monitor,
                placement: Placement {
                    workspace,
                    on_all_workspaces,
                    monitor: Some(monitor as i32),
                    rect: Some(rect),
                    maximized: s.maximized,
                    fullscreen: s.fullscreen,
                    minimized: s.minimized,
                    above: s.above,
                },
                done_in_pass: None,
                last_seen: None,
            }
        })
        .collect();
    planned.sort_by_key(|p| p.placement.workspace.unwrap_or(i32::MAX));

    Plan {
        windows: planned,
        unmatched_slots: matching.unmatched_slots,
        current_monitors,
        monitor_targets: mapping.targets,
    }
}

/// Whether a window (as read back from the shim) matches its placement.
pub fn in_place(window: &ShimWindow, p: &Placement) -> bool {
    if p.workspace.is_some_and(|ws| window.workspace != ws) {
        return false;
    }
    if window.maximized != p.maximized || window.fullscreen != p.fullscreen || window.minimized != p.minimized
    {
        return false;
    }
    // Maximized, fullscreen and minimized windows have no meaningful frame to check.
    if p.maximized != Maximized::None || p.fullscreen || p.minimized {
        return true;
    }
    p.rect.is_none_or(|r| {
        (window.frame.x - r.x).abs() <= TOLERANCE && (window.frame.y - r.y).abs() <= TOLERANCE
    })
}

/// The placement without states, for the geometry phase.
fn geometry_only(p: &Placement) -> Placement {
    Placement { maximized: Maximized::None, fullscreen: false, minimized: false, ..p.clone() }
}

fn has_state(p: &Placement) -> bool {
    p.maximized != Maximized::None || p.fullscreen || p.minimized || p.above
}

pub async fn execute(shim: &Shim, plan: &mut Plan) -> Result<()> {
    let mut geometry_done: Vec<Option<u32>> = vec![None; plan.windows.len()];
    for pass in 1..=MAX_PASSES {
        let pending: Vec<usize> = (0..plan.windows.len()).filter(|&i| geometry_done[i].is_none()).collect();
        if pending.is_empty() {
            break;
        }
        for &i in &pending {
            let p = &plan.windows[i];
            // A window closed since planning is reported as not placed.
            let _ = shim.place_window(&p.window.id, &geometry_only(&p.placement)).await;
        }
        tokio::time::sleep(SETTLE).await;

        let current = shim.windows().await?;
        for &i in &pending {
            let p = &mut plan.windows[i];
            p.last_seen = current.iter().find(|w| w.id == p.window.id).cloned();
            if p.last_seen.as_ref().is_some_and(|w| in_place(w, &geometry_only(&p.placement))) {
                geometry_done[i] = Some(pass);
            }
        }
    }

    let with_state: Vec<usize> =
        (0..plan.windows.len()).filter(|&i| has_state(&plan.windows[i].placement)).collect();
    for &i in &with_state {
        let p = &plan.windows[i];
        let _ = shim.set_window_state(&p.window.id, &p.placement).await;
    }
    if !with_state.is_empty() {
        tokio::time::sleep(SETTLE).await;
    }

    let current = shim.windows().await?;
    for (i, p) in plan.windows.iter_mut().enumerate() {
        if let Some(w) = current.iter().find(|w| w.id == p.window.id) {
            p.last_seen = Some(w.clone());
        }
        // The frame was checked in the geometry phase; now only the states.
        let states_ok = p.last_seen.as_ref().is_some_and(|w| {
            w.maximized == p.placement.maximized
                && w.fullscreen == p.placement.fullscreen
                && w.minimized == p.placement.minimized
                && w.above == p.placement.above
        });
        p.done_in_pass = geometry_done[i].filter(|_| states_ok);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn monitor(key: &str, rect: Rect, primary: bool) -> Monitor {
        Monitor {
            key: key.into(),
            connector: key.to_uppercase(),
            display_name: None,
            rect,
            work_area: rect,
            scale: 1.0,
            primary,
        }
    }

    const LEFT: Rect = Rect { x: 0, y: 0, w: 1728, h: 3072 };
    const RIGHT: Rect = Rect { x: 1728, y: 886, w: 1920, h: 1080 };

    fn slot(title: &str, monitor: &str, workspace: i32, rect_abs: Rect) -> WindowSlot {
        WindowSlot {
            matcher: WindowMatch {
                wm_class: None,
                wm_class_instance: None,
                gtk_app_id: None,
                sandboxed_app_id: None,
                title: Some(title.into()),
            },
            monitor_key: monitor.into(),
            workspace,
            on_all_workspaces: workspace < 0,
            rect_rel: RelRect { x: 0.0, y: 0.0, w: 0.5, h: 0.5 },
            rect_abs,
            maximized: Maximized::None,
            fullscreen: false,
            minimized: false,
            above: false,
        }
    }

    fn open(seq: u32, title: &str) -> ShimWindow {
        ShimWindow {
            id: seq.to_string(),
            stable_seq: seq,
            app_id: Some("app.desktop".into()),
            wm_class: None,
            wm_class_instance: None,
            gtk_app_id: None,
            sandboxed_app_id: None,
            title: Some(title.into()),
            pid: 1,
            client: "wayland".into(),
            workspace: 0,
            on_all_workspaces: false,
            monitor: 1,
            frame: Rect { x: 1800, y: 900, w: 100, h: 100 },
            restore_frame: None,
            maximized: Maximized::None,
            fullscreen: false,
            minimized: false,
            above: false,
        }
    }

    fn session(slots: Vec<WindowSlot>) -> Session {
        Session {
            schema: SCHEMA,
            name: "t".into(),
            created: String::new(),
            host: String::new(),
            shell_version: String::new(),
            workspaces: Workspaces { count: 4, dynamic: true, only_on_primary: true },
            monitors: vec![monitor("lg", LEFT, false), monitor("gbt", RIGHT, true)],
            apps: vec![App {
                id: "app.desktop".into(),
                launch: Launch::None,
                launch_mode: LaunchMode::PerWindow,
                windows: slots,
            }],
        }
    }

    #[test]
    fn plans_in_ascending_workspace_order_with_gaps_compacted() {
        let r = Rect { x: 1800, y: 900, w: 500, h: 400 };
        let s = session(vec![
            slot("three", "gbt", 3, r),
            slot("side", "lg", -1, Rect { x: 10, y: 10, w: 500, h: 400 }),
            slot("zero", "gbt", 0, r),
            slot("missing", "gbt", 1, r),
        ]);
        let windows = vec![open(1, "zero"), open(2, "three"), open(3, "side")];
        let current = vec![monitor("lg", LEFT, false), monitor("gbt", RIGHT, true)];
        let plan = plan(&s, windows, current, true);

        let order: Vec<_> =
            plan.windows.iter().map(|p| (p.window.title.clone().unwrap(), p.placement.workspace)).collect();
        assert_eq!(
            order,
            [("zero".to_owned(), Some(0)), ("three".to_owned(), Some(1)), ("side".to_owned(), None)]
        );
        // The window on the secondary monitor is only sticky because of
        // workspaces-only-on-primary, so no stick() is requested.
        let side = &plan.windows[2];
        assert_eq!(side.placement.on_all_workspaces, None);
        assert_eq!(side.monitor, 0);
        assert_eq!(side.placement.rect, Some(Rect { x: 10, y: 10, w: 500, h: 400 }));
        assert_eq!(plan.unmatched_slots, [(0, 3)]);
    }

    #[test]
    fn static_workspaces_are_not_compacted() {
        let r = Rect { x: 1800, y: 900, w: 500, h: 400 };
        let s = session(vec![slot("three", "gbt", 3, r)]);
        let current = vec![monitor("lg", LEFT, false), monitor("gbt", RIGHT, true)];
        let plan = plan(&s, vec![open(1, "three")], current, false);
        assert_eq!(plan.windows[0].placement.workspace, Some(3));
    }

    #[test]
    fn in_place_checks_position_workspace_and_state() {
        let mut w = open(1, "x");
        let mut p = Placement {
            workspace: Some(0),
            rect: Some(Rect { x: 1801, y: 899, w: 999, h: 999 }),
            ..Placement::default()
        };
        assert!(in_place(&w, &p), "size is not checked, position within tolerance");

        p.rect = Some(Rect { x: 1810, y: 900, w: 100, h: 100 });
        assert!(!in_place(&w, &p));

        p.maximized = Maximized::Both;
        assert!(!in_place(&w, &p));
        w.maximized = Maximized::Both;
        assert!(in_place(&w, &p), "maximized windows skip the frame check");

        p.workspace = Some(2);
        assert!(!in_place(&w, &p));
    }
}
