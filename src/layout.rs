//! Where a saved window goes on the current desktop: which monitor, which
//! rectangle, which workspace. Pure, so it is unit-tested without a shell.

use std::collections::BTreeSet;
use std::fmt;

use crate::model::{Monitor, Rect, Session, WindowSlot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapReason {
    /// Same vendor:product:serial.
    Exact,
    /// Same connector (e.g. a monitor was replaced).
    Connector,
    /// Same logical size.
    Size,
    /// Nothing matched; fell back to the primary monitor.
    Primary,
}

impl fmt::Display for MapReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MapReason::Exact => "same monitor",
            MapReason::Connector => "same connector",
            MapReason::Size => "same size",
            MapReason::Primary => "fallback to primary",
        })
    }
}

type SameMonitor = fn(&Monitor, &Monitor) -> bool;

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorMapping {
    /// Per saved monitor (same order as `session.monitors`): index into the
    /// current monitors and why.
    pub targets: Vec<(usize, MapReason)>,
}

impl MonitorMapping {
    pub fn new(saved: &[Monitor], current: &[Monitor]) -> Self {
        let primary = current.iter().position(|m| m.primary).unwrap_or(0);
        let mut used = vec![false; current.len()];
        let mut targets: Vec<Option<(usize, MapReason)>> = vec![None; saved.len()];

        // Each pass only claims monitors not taken by a better pass, so two
        // saved monitors do not collapse onto one while another is free.
        let passes: [(MapReason, SameMonitor); 3] = [
            (MapReason::Exact, |s, c| s.key == c.key),
            (MapReason::Connector, |s, c| !s.connector.is_empty() && s.connector == c.connector),
            (MapReason::Size, |s, c| (s.rect.w, s.rect.h) == (c.rect.w, c.rect.h)),
        ];
        for (reason, same) in passes {
            for (i, s) in saved.iter().enumerate() {
                if targets[i].is_some() {
                    continue;
                }
                if let Some(c) = (0..current.len()).find(|&c| !used[c] && same(s, &current[c])) {
                    used[c] = true;
                    targets[i] = Some((c, reason));
                }
            }
        }
        MonitorMapping {
            targets: targets.into_iter().map(|t| t.unwrap_or((primary, MapReason::Primary))).collect(),
        }
    }

    /// Current monitor index for a saved monitor key.
    pub fn target(&self, saved: &[Monitor], key: &str) -> Option<(usize, MapReason)> {
        saved.iter().position(|m| m.key == key).map(|i| self.targets[i])
    }
}

/// The saved rectangle on the current monitor. The absolute rectangle is used
/// when the monitor is the same and its work area unchanged; otherwise the
/// relative one, clamped to fit the work area.
pub fn target_rect(slot: &WindowSlot, saved: Option<&Monitor>, current: &Monitor, reason: MapReason) -> Rect {
    if reason == MapReason::Exact && saved.is_some_and(|s| s.work_area == current.work_area) {
        return slot.rect_abs;
    }
    let area = current.work_area;
    let scale = |rel: f64, len: i32| (rel * f64::from(len)).round() as i32;
    let w = scale(slot.rect_rel.w, area.w).clamp(1, area.w);
    let h = scale(slot.rect_rel.h, area.h).clamp(1, area.h);
    let x = (area.x + scale(slot.rect_rel.x, area.w)).clamp(area.x, area.x + area.w - w);
    let y = (area.y + scale(slot.rect_rel.y, area.h)).clamp(area.y, area.y + area.h - h);
    Rect { x, y, w, h }
}

/// Maps saved workspace indices to dense ones (0, 1, 2, ...) in the same
/// order. With dynamic workspaces GNOME removes empty workspaces in the
/// middle, so a gap (from windows that are not open) would shift every window
/// above it anyway; compacting keeps the result deterministic.
pub fn compact_workspaces(workspaces: impl IntoIterator<Item = i32>) -> impl Fn(i32) -> i32 {
    let used: Vec<i32> =
        workspaces.into_iter().filter(|&w| w >= 0).collect::<BTreeSet<_>>().into_iter().collect();
    move |w| if w < 0 { w } else { used.iter().position(|&u| u == w).map_or(w, |p| p as i32) }
}

/// Whether a window's "on all workspaces" state is only a side effect of
/// `workspaces-only-on-primary` (windows on secondary monitors are shown on
/// every workspace) rather than something the user set.
pub fn implicit_sticky(session: &Session, slot: &WindowSlot) -> bool {
    session.workspaces.only_on_primary
        && slot.on_all_workspaces
        && session.monitors.iter().find(|m| m.key == slot.monitor_key).is_some_and(|m| !m.primary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Maximized, RelRect, WindowMatch};

    fn monitor(key: &str, connector: &str, rect: Rect, primary: bool) -> Monitor {
        Monitor {
            key: key.into(),
            connector: connector.into(),
            display_name: None,
            rect,
            work_area: rect,
            scale: 1.0,
            primary,
        }
    }

    const LEFT: Rect = Rect { x: 0, y: 0, w: 1728, h: 3072 };
    const RIGHT: Rect = Rect { x: 1728, y: 886, w: 1920, h: 1080 };

    fn slot(rect_abs: Rect, rect_rel: RelRect) -> WindowSlot {
        WindowSlot {
            matcher: WindowMatch {
                wm_class: None,
                wm_class_instance: None,
                gtk_app_id: None,
                sandboxed_app_id: None,
                title: None,
            },
            monitor_key: "lg".into(),
            workspace: -1,
            on_all_workspaces: true,
            rect_rel,
            rect_abs,
            maximized: Maximized::None,
            fullscreen: false,
            minimized: false,
            above: false,
        }
    }

    #[test]
    fn maps_exact_then_connector_then_size_then_primary() {
        let saved = [
            monitor("lg", "DP-2", LEFT, false),
            monitor("gbt", "DP-1", RIGHT, true),
            monitor("old", "HDMI-1", Rect { x: 0, y: 0, w: 800, h: 600 }, false),
        ];
        // Connectors swapped, the Gigabyte was replaced by another 1920x1080 on HDMI-2.
        let current = [monitor("other", "HDMI-2", RIGHT, true), monitor("lg", "DP-1", LEFT, false)];
        let mapping = MonitorMapping::new(&saved, &current);
        assert_eq!(mapping.targets, [(1, MapReason::Exact), (0, MapReason::Size), (0, MapReason::Primary)]);
        assert_eq!(mapping.target(&saved, "gbt"), Some((0, MapReason::Size)));
        assert_eq!(mapping.target(&saved, "missing"), None);
    }

    #[test]
    fn connector_match_beats_size_match() {
        let saved = [monitor("a", "DP-1", RIGHT, true)];
        let current = [monitor("x", "DP-2", RIGHT, false), monitor("y", "DP-1", LEFT, true)];
        assert_eq!(MonitorMapping::new(&saved, &current).targets, [(1, MapReason::Connector)]);
    }

    #[test]
    fn same_monitor_uses_absolute_rect() {
        let abs = Rect { x: 89, y: 1007, w: 1372, h: 823 };
        let s = slot(abs, RelRect { x: 0.0, y: 0.0, w: 0.1, h: 0.1 });
        let m = monitor("lg", "DP-2", LEFT, false);
        assert_eq!(target_rect(&s, Some(&m), &m, MapReason::Exact), abs);
    }

    #[test]
    fn other_monitor_scales_relative_rect_and_clamps() {
        let s = slot(Rect { x: 0, y: 0, w: 0, h: 0 }, RelRect { x: 0.5, y: 0.5, w: 0.25, h: 0.25 });
        let saved = monitor("lg", "DP-2", LEFT, false);
        let current = monitor("gbt", "DP-1", RIGHT, true);
        assert_eq!(
            target_rect(&s, Some(&saved), &current, MapReason::Primary),
            Rect { x: 1728 + 960, y: 886 + 540, w: 480, h: 270 }
        );

        // Taller than the target work area (portrait -> landscape): clamp size and keep it on screen.
        let tall = slot(Rect { x: 0, y: 0, w: 0, h: 0 }, RelRect { x: 0.1, y: 0.9, w: 0.5, h: 1.6 });
        assert_eq!(
            target_rect(&tall, Some(&saved), &current, MapReason::Primary),
            Rect { x: 1728 + 192, y: 886, w: 960, h: 1080 }
        );
    }

    #[test]
    fn compacts_gaps_and_keeps_all_workspaces() {
        let map = compact_workspaces([0, 3, -1, 3, 5]);
        assert_eq!([0, 3, 5, -1].map(&map), [0, 1, 2, -1]);
    }
}
