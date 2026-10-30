//! Pairs saved window slots with open windows. Pure, so it is unit-tested
//! without a shell.
//!
//! A window can only match a slot of the same app. Among those, titles decide
//! (exact match, then equal after normalizing, then similarity), and
//! remaining ties go by order: the n-th saved window of an app prefers the
//! n-th open one. Assignment is greedy by descending score, which is plenty
//! for a few dozen windows.
//!
//! Titles change while an app runs (unread counters, "modified" markers), so
//! they are normalized before comparing, and the suffix all titles of an app
//! share ("— Mozilla Firefox") is ignored for similarity: it would make every
//! pair of that app's windows look alike.

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

/// Lowercase, without counters like "(4 unread)" or "[3]", without leading
/// or trailing markers like "●", "*" or "◐", with collapsed whitespace.
pub fn normalize(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut text = String::with_capacity(lower.len());
    let mut rest = lower.as_str();
    while let Some(open) = rest.find(['(', '[']) {
        let close = if rest[open..].starts_with('(') { ')' } else { ']' };
        match rest[open..].find(close) {
            Some(len) if len <= 20 && rest[open..open + len].chars().any(|c| c.is_ascii_digit()) => {
                text.push_str(&rest[..open]);
                rest = &rest[open + len + 1..];
            }
            _ => {
                text.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    text.push_str(rest);

    let keep = |c: char| c.is_alphanumeric() || matches!(c, '~' | '/' | '.' | ')' | ']');
    let trimmed = text.trim_matches(|c: char| !keep(c));
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Length in chars of the suffix shared by all titles (0 for fewer than two).
fn common_suffix_len(titles: &[&str]) -> usize {
    if titles.len() < 2 {
        return 0;
    }
    let mut len = 0;
    let mut iters: Vec<_> = titles.iter().map(|t| t.chars().rev()).collect();
    loop {
        let mut current = None;
        for it in &mut iters {
            match (it.next(), current) {
                (Some(c), None) => current = Some(c),
                (Some(c), Some(prev)) if c == prev => {}
                _ => return len,
            }
        }
        len += 1;
    }
}

fn strip_suffix(title: &str, chars: usize) -> &str {
    let keep = title.chars().count().saturating_sub(chars);
    title.char_indices().nth(keep).map_or(title, |(i, _)| &title[..i])
}

fn title_score(saved: Option<&str>, open: Option<&str>, suffix_len: usize) -> f64 {
    let (Some(saved), Some(open)) = (saved, open) else { return 0.0 };
    if saved == open {
        return 50.0;
    }
    let (saved, open) = (normalize(saved), normalize(open));
    if saved == open {
        return 45.0;
    }
    40.0 * strsim::normalized_levenshtein(strip_suffix(&saved, suffix_len), strip_suffix(&open, suffix_len))
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

    let normalized: Vec<Option<String>> = windows.iter().map(|w| w.title.as_deref().map(normalize)).collect();

    let mut candidates: Vec<(f64, SlotRef, usize)> = Vec::new();
    for (a, app) in session.apps.iter().enumerate() {
        let saved_titles: Vec<String> =
            app.windows.iter().filter_map(|w| w.matcher.title.as_deref().map(normalize)).collect();
        let app_titles: Vec<&str> = saved_titles
            .iter()
            .map(String::as_str)
            .chain((0..windows.len()).filter(|&w| keys[w] == app.id).filter_map(|w| normalized[w].as_deref()))
            .collect();
        let suffix_len = common_suffix_len(&app_titles);

        for (s, slot) in app.windows.iter().enumerate() {
            for (w, window) in windows.iter().enumerate() {
                if keys[w] != app.id {
                    continue;
                }
                let order_penalty = s.abs_diff(rank[w]) as f64;
                let score = title_score(slot.matcher.title.as_deref(), window.title.as_deref(), suffix_len)
                    - order_penalty;
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
            restore_frame: None,
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
    fn normalizes_counters_markers_and_whitespace() {
        assert_eq!(normalize("Inbox (4 unread)"), "inbox");
        assert_eq!(normalize("● main.rs - simple-session - Code"), "main.rs - simple-session - code");
        assert_eq!(normalize("*Untitled Document 1 - gedit"), "untitled document 1 - gedit");
        assert_eq!(
            normalize("◐ Simple-session  GNOME window layouts"),
            "simple-session gnome window layouts"
        );
        assert_eq!(normalize("(3) Slack | general [12]"), "slack | general");
        // Brackets without digits are content, not counters.
        assert_eq!(normalize("notes (draft) [wip]"), "notes (draft) [wip]");
        assert_eq!(normalize("~/Documents"), "~/documents");
    }

    #[test]
    fn common_suffix_and_strip() {
        let titles = ["github — mozilla firefox", "gmail — mozilla firefox", "docs — mozilla firefox"];
        let len = common_suffix_len(&titles);
        assert_eq!(strip_suffix(titles[0], len), "github");
        assert_eq!(common_suffix_len(&["only one"]), 0);
        assert_eq!(strip_suffix("äöü x", 2), "äöü");
    }

    #[test]
    fn counters_and_markers_do_not_break_matching() {
        let s = session(vec![
            ("org.gnome.Evolution.desktop", vec!["Inbox (4 unread)", "Calendar"]),
            ("code.desktop", vec!["main.rs - simple-session - Code", "notes.md - notes - Code"]),
        ]);
        let windows = [
            open(1, "org.gnome.Evolution.desktop", "Calendar"),
            open(2, "org.gnome.Evolution.desktop", "Inbox (7 unread)"),
            open(3, "code.desktop", "● notes.md - notes - Code"),
            open(4, "code.desktop", "● main.rs - simple-session - Code"),
        ];
        let m = match_windows(&s, &windows);
        assert_eq!(m.pairs, [((0, 0), 1), ((0, 1), 0), ((1, 0), 3), ((1, 1), 2)]);
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
