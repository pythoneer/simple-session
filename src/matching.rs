//! Pairs saved window slots with open windows. Pure, so it is unit-tested
//! without a shell.
//!
//! A window can only match a slot of the same app. Among those, titles decide
//! (exact match, then similarity), and remaining ties go by order: the n-th
//! saved window of an app prefers the n-th open one. Assignment is greedy by
//! descending score, which is plenty for a few dozen windows.

use crate::capture::app_key;
use crate::model::Session;
use crate::shim::ShimWindow;

/// Identifies a slot as (index into `session.apps`, index into its `windows`).
pub type SlotRef = (usize, usize);

#[derive(Debug, Default, PartialEq)]
pub struct Matching {
    /// (slot, index into the open windows)
    pub pairs: Vec<(SlotRef, usize)>,
    pub unmatched_slots: Vec<SlotRef>,
}

fn title_score(saved: Option<&str>, open: Option<&str>) -> f64 {
    match (saved, open) {
        (Some(a), Some(b)) if a == b => 50.0,
        (Some(a), Some(b)) => 40.0 * strsim::normalized_levenshtein(&a.to_lowercase(), &b.to_lowercase()),
        _ => 0.0,
    }
}

pub fn match_windows(session: &Session, windows: &[ShimWindow]) -> Matching {
    let keys: Vec<String> = windows.iter().map(app_key).collect();

    // Rank of each open window among the open windows of its app.
    let mut order: Vec<usize> = (0..windows.len()).collect();
    order.sort_by_key(|&i| (windows[i].workspace, windows[i].stable_seq));
    let mut rank = vec![0usize; windows.len()];
    for (pos, &i) in order.iter().enumerate() {
        rank[i] = order[..pos].iter().filter(|&&j| keys[j] == keys[i]).count();
    }

    let mut candidates: Vec<(f64, SlotRef, usize)> = Vec::new();
    for (a, app) in session.apps.iter().enumerate() {
        for (s, slot) in app.windows.iter().enumerate() {
            for (w, window) in windows.iter().enumerate() {
                if keys[w] != app.id {
                    continue;
                }
                let order_penalty = s.abs_diff(rank[w]) as f64;
                let score =
                    title_score(slot.matcher.title.as_deref(), window.title.as_deref()) - order_penalty;
                candidates.push((score, (a, s), w));
            }
        }
    }
    candidates.sort_by(|x, y| y.0.total_cmp(&x.0));

    let mut matching = Matching::default();
    let mut window_taken = vec![false; windows.len()];
    for (_, slot, w) in candidates {
        if window_taken[w] || matching.pairs.iter().any(|(s, _)| *s == slot) {
            continue;
        }
        window_taken[w] = true;
        matching.pairs.push((slot, w));
    }
    for (a, app) in session.apps.iter().enumerate() {
        for s in 0..app.windows.len() {
            if !matching.pairs.iter().any(|(slot, _)| *slot == (a, s)) {
                matching.unmatched_slots.push((a, s));
            }
        }
    }
    matching.pairs.sort();
    matching
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn slot(title: &str) -> WindowSlot {
        WindowSlot {
            matcher: WindowMatch {
                wm_class: None,
                wm_class_instance: None,
                gtk_app_id: None,
                sandboxed_app_id: None,
                title: Some(title.into()),
            },
            monitor_key: "m".into(),
            workspace: 0,
            on_all_workspaces: false,
            rect_rel: RelRect { x: 0.0, y: 0.0, w: 0.5, h: 0.5 },
            rect_abs: Rect { x: 0, y: 0, w: 100, h: 100 },
            maximized: Maximized::None,
            fullscreen: false,
            minimized: false,
            above: false,
        }
    }

    fn session(apps: Vec<(&str, Vec<&str>)>) -> Session {
        Session {
            schema: SCHEMA,
            name: "t".into(),
            created: String::new(),
            host: String::new(),
            shell_version: String::new(),
            workspaces: Workspaces { count: 1, dynamic: true, only_on_primary: true },
            monitors: Vec::new(),
            apps: apps
                .into_iter()
                .map(|(id, titles)| App {
                    id: id.into(),
                    launch: Launch::None,
                    launch_mode: LaunchMode::PerWindow,
                    windows: titles.into_iter().map(slot).collect(),
                })
                .collect(),
        }
    }

    fn open(seq: u32, app_id: &str, title: &str) -> ShimWindow {
        ShimWindow {
            id: seq.to_string(),
            stable_seq: seq,
            app_id: Some(app_id.into()),
            wm_class: None,
            wm_class_instance: None,
            gtk_app_id: None,
            sandboxed_app_id: None,
            title: Some(title.into()),
            pid: 1,
            client: "wayland".into(),
            workspace: 0,
            on_all_workspaces: false,
            monitor: 0,
            frame: Rect { x: 0, y: 0, w: 10, h: 10 },
            maximized: Maximized::None,
            fullscreen: false,
            minimized: false,
            above: false,
        }
    }

    #[test]
    fn matches_by_title_not_by_order() {
        let s = session(vec![("firefox.desktop", vec!["Mail — Firefox", "Docs — Firefox"])]);
        let windows =
            [open(1, "firefox.desktop", "Docs — Firefox"), open(2, "firefox.desktop", "Mail — Firefox")];
        let m = match_windows(&s, &windows);
        assert_eq!(m.pairs, [((0, 0), 1), ((0, 1), 0)]);
        assert!(m.unmatched_slots.is_empty());
    }

    #[test]
    fn similar_titles_win_over_order() {
        let s = session(vec![("code.desktop", vec!["main.rs - simple-session", "README.md - notes"])]);
        let windows = [open(1, "code.desktop", "notes"), open(2, "code.desktop", "lib.rs - simple-session")];
        let m = match_windows(&s, &windows);
        assert_eq!(m.pairs, [((0, 0), 1), ((0, 1), 0)]);
    }

    #[test]
    fn identical_titles_fall_back_to_order() {
        let s = session(vec![("term.desktop", vec!["~", "~", "~"])]);
        let windows =
            [open(5, "term.desktop", "~"), open(3, "term.desktop", "~"), open(4, "term.desktop", "~")];
        let m = match_windows(&s, &windows);
        // stable_seq order 3, 4, 5 -> slots 0, 1, 2
        assert_eq!(m.pairs, [((0, 0), 1), ((0, 1), 2), ((0, 2), 0)]);
    }

    #[test]
    fn never_matches_across_apps_and_reports_missing_slots() {
        let s = session(vec![("a.desktop", vec!["same"]), ("b.desktop", vec!["same", "other"])]);
        let windows = [open(1, "b.desktop", "same"), open(2, "c.desktop", "other")];
        let m = match_windows(&s, &windows);
        assert_eq!(m.pairs, [((1, 0), 0)]);
        assert_eq!(m.unmatched_slots, [(0, 0), (1, 1)]);
    }
}
