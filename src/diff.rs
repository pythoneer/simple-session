//! Differences between two sessions (or a session and the current desktop,
//! captured as a session). Windows are paired with the same matching that
//! `apply` uses, so the diff shows what `apply` would treat as one window.

use crate::matching::{self, SlotRef};
use crate::model::{Maximized, Rect, Session, WindowSlot};
use crate::shim::ShimWindow;

#[derive(Debug, PartialEq)]
pub enum Change {
    /// Only in the first session.
    Removed(SlotRef),
    /// Only in the second session (slot ref into the second session).
    Added(SlotRef),
    /// In both, with differences described for display.
    Changed { old: SlotRef, new: SlotRef, details: Vec<String> },
}

#[derive(Debug, Default)]
pub struct Diff {
    pub changes: Vec<Change>,
    pub unchanged: usize,
}

const TOLERANCE: i32 = 2;

/// The slots of a session as if they were open windows, for matching.
fn as_windows(session: &Session) -> (Vec<ShimWindow>, Vec<SlotRef>) {
    let mut windows = Vec::new();
    let mut refs = Vec::new();
    for (a, app) in session.apps.iter().enumerate() {
        for (s, slot) in app.windows.iter().enumerate() {
            windows.push(ShimWindow {
                id: format!("{a}:{s}"),
                stable_seq: windows.len() as u32,
                app_id: Some(app.id.clone()),
                wm_class: slot.matcher.wm_class.clone(),
                wm_class_instance: slot.matcher.wm_class_instance.clone(),
                gtk_app_id: slot.matcher.gtk_app_id.clone(),
                sandboxed_app_id: slot.matcher.sandboxed_app_id.clone(),
                title: slot.matcher.title.clone(),
                pid: 0,
                client: String::new(),
                workspace: slot.workspace,
                on_all_workspaces: slot.on_all_workspaces,
                monitor: -1,
                frame: slot.rect_abs,
                restore_frame: None,
                maximized: slot.maximized,
                fullscreen: slot.fullscreen,
                minimized: slot.minimized,
                above: slot.above,
            });
            refs.push((a, s));
        }
    }
    (windows, refs)
}

pub fn diff(old: &Session, new: &Session) -> Diff {
    let (windows, refs) = as_windows(new);
    let matching = matching::match_windows(old, &windows);

    let mut result = Diff::default();
    let mut seen = vec![false; refs.len()];
    for &(old_ref, w) in &matching.pairs {
        seen[w] = true;
        let details = describe(old, slot(old, old_ref), new, slot(new, refs[w]));
        if details.is_empty() {
            result.unchanged += 1;
        } else {
            result.changes.push(Change::Changed { old: old_ref, new: refs[w], details });
        }
    }
    result.changes.extend(matching.unmatched_slots.iter().map(|&r| Change::Removed(r)));
    result.changes.extend(refs.iter().zip(&seen).filter(|(_, seen)| !**seen).map(|(&r, _)| Change::Added(r)));
    result
}

fn slot(session: &Session, (a, s): SlotRef) -> &WindowSlot {
    &session.apps[a].windows[s]
}

fn describe(old_session: &Session, old: &WindowSlot, new_session: &Session, new: &WindowSlot) -> Vec<String> {
    let mut details = Vec::new();
    let workspace =
        |w: &WindowSlot| if w.on_all_workspaces { "all".to_owned() } else { w.workspace.to_string() };
    if workspace(old) != workspace(new) {
        details.push(format!("workspace {} -> {}", workspace(old), workspace(new)));
    }
    if old.monitor_key != new.monitor_key {
        details.push(format!(
            "monitor {} -> {}",
            monitor_label(old_session, &old.monitor_key),
            monitor_label(new_session, &new.monitor_key)
        ));
    }
    let moved = |a: Rect, b: Rect| {
        [(a.x, b.x), (a.y, b.y), (a.w, b.w), (a.h, b.h)].iter().any(|(p, q)| (p - q).abs() > TOLERANCE)
    };
    if moved(old.rect_abs, new.rect_abs) {
        details.push(format!("geometry {} -> {}", geometry(old.rect_abs), geometry(new.rect_abs)));
    }
    if old.maximized != new.maximized {
        details.push(format!("maximized {} -> {}", maximized(old.maximized), maximized(new.maximized)));
    }
    for (name, a, b) in [
        ("fullscreen", old.fullscreen, new.fullscreen),
        ("minimized", old.minimized, new.minimized),
        ("above", old.above, new.above),
    ] {
        if a != b {
            details.push(format!("{name} {a} -> {b}"));
        }
    }
    details
}

fn monitor_label(session: &Session, key: &str) -> String {
    session
        .monitors
        .iter()
        .find(|m| m.key == key)
        .filter(|m| !m.connector.is_empty())
        .map_or_else(|| key.to_owned(), |m| m.connector.clone())
}

fn geometry(r: Rect) -> String {
    format!("{}x{}+{}+{}", r.w, r.h, r.x, r.y)
}

fn maximized(m: Maximized) -> &'static str {
    match m {
        Maximized::None => "no",
        Maximized::Horizontal => "horizontally",
        Maximized::Vertical => "vertically",
        Maximized::Both => "yes",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn slot(title: &str, workspace: i32, rect: Rect) -> WindowSlot {
        WindowSlot {
            matcher: WindowMatch {
                wm_class: None,
                wm_class_instance: None,
                gtk_app_id: None,
                sandboxed_app_id: None,
                title: Some(title.into()),
            },
            monitor_key: "gbt".into(),
            workspace,
            on_all_workspaces: false,
            rect_rel: RelRect { x: 0.0, y: 0.0, w: 0.5, h: 0.5 },
            rect_abs: rect,
            maximized: Maximized::None,
            fullscreen: false,
            minimized: false,
            above: false,
        }
    }

    fn session(apps: Vec<(&str, Vec<WindowSlot>)>) -> Session {
        let rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        Session {
            schema: SCHEMA,
            name: "t".into(),
            created: String::new(),
            host: String::new(),
            shell_version: String::new(),
            workspaces: Workspaces { count: 2, dynamic: true, only_on_primary: true },
            monitors: vec![
                Monitor {
                    key: "gbt".into(),
                    connector: "DP-1".into(),
                    display_name: None,
                    rect,
                    work_area: rect,
                    scale: 1.0,
                    primary: true,
                },
                Monitor {
                    key: "lg".into(),
                    connector: "DP-2".into(),
                    display_name: None,
                    rect,
                    work_area: rect,
                    scale: 1.0,
                    primary: false,
                },
            ],
            apps: apps
                .into_iter()
                .map(|(id, windows)| App {
                    id: id.into(),
                    launch: Launch::None,
                    launch_mode: LaunchMode::PerWindow,
                    windows,
                })
                .collect(),
        }
    }

    const R: Rect = Rect { x: 100, y: 100, w: 800, h: 600 };

    #[test]
    fn identical_sessions_have_no_changes() {
        let s = session(vec![("a.desktop", vec![slot("x", 0, R), slot("y", 1, R)])]);
        let d = diff(&s, &s);
        assert!(d.changes.is_empty());
        assert_eq!(d.unchanged, 2);
    }

    #[test]
    fn reports_moves_additions_and_removals() {
        let old =
            session(vec![("a.desktop", vec![slot("x", 0, R)]), ("gone.desktop", vec![slot("g", 0, R)])]);
        let mut moved = slot("x", 1, Rect { x: 101, y: 300, w: 800, h: 600 });
        moved.monitor_key = "lg".into();
        moved.maximized = Maximized::Both;
        let new = session(vec![("new.desktop", vec![slot("n", 0, R)]), ("a.desktop", vec![moved])]);

        let d = diff(&old, &new);
        assert_eq!(d.unchanged, 0);
        assert_eq!(
            d.changes,
            [
                Change::Changed {
                    old: (0, 0),
                    new: (1, 0),
                    details: vec![
                        "workspace 0 -> 1".into(),
                        "monitor DP-1 -> DP-2".into(),
                        "geometry 800x600+100+100 -> 800x600+101+300".into(),
                        "maximized no -> yes".into(),
                    ],
                },
                Change::Removed((1, 0)),
                Change::Added((0, 0)),
            ]
        );
    }
}
