//! `restore`: launch what is missing, wait for the windows, then place
//! everything like `apply`.
//!
//! Windows are only placed once launching has settled: titles (which matching
//! relies on) are often not final when a window is mapped, e.g. a browser
//! restoring its tabs.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::time::Instant;

use crate::capture::app_key;
use crate::layout;
use crate::matching;
use crate::model::{Launch, LaunchMode, Session};
use crate::shim::{Shim, ShimWindow};

/// Pause between launches, so apps do not all start at the same instant.
const STAGGER: Duration = Duration::from_millis(150);
/// Launching counts as settled when every launched app has shown a window
/// and nothing new appeared for this long.
const QUIET: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchItem {
    /// Index into `session.apps`.
    pub app: usize,
    /// Workspace hint for the launch, -1 for none.
    pub workspace: i32,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchPlan {
    pub launches: Vec<LaunchItem>,
    /// Apps with missing windows that have no way to be launched.
    pub not_launchable: Vec<usize>,
    /// Self-restoring apps that are already running: launching them again
    /// would open an extra window instead of the missing ones.
    pub already_running: Vec<usize>,
}

pub fn launch_plan(session: &Session, windows: &[ShimWindow], dynamic_workspaces: bool) -> LaunchPlan {
    let matching = matching::match_windows(session, windows);

    let explicit = |a: usize, s: usize| {
        let slot = &session.apps[a].windows[s];
        (!slot.on_all_workspaces && slot.workspace >= 0 && !layout::implicit_sticky(session, slot))
            .then_some(slot.workspace)
    };
    let all_slots =
        session.apps.iter().enumerate().flat_map(|(a, app)| (0..app.windows.len()).map(move |s| (a, s)));
    let compact = layout::compact_workspaces(all_slots.filter_map(|(a, s)| explicit(a, s)));
    let hint = |a: usize, s: usize| {
        explicit(a, s).map_or(-1, |ws| if dynamic_workspaces { compact(ws) } else { ws })
    };

    let mut plan = LaunchPlan::default();
    for (a, app) in session.apps.iter().enumerate() {
        let missing: Vec<usize> = matching
            .unmatched_slots
            .iter()
            .filter(|(app_index, _)| *app_index == a)
            .map(|&(_, s)| s)
            .collect();
        if missing.is_empty() {
            continue;
        }
        if app.launch == Launch::None {
            plan.not_launchable.push(a);
            continue;
        }
        match app.launch_mode {
            LaunchMode::Once => {
                if windows.iter().any(|w| app_key(w) == app.id) {
                    plan.already_running.push(a);
                } else {
                    let workspace =
                        missing.iter().map(|&s| hint(a, s)).filter(|&h| h >= 0).min().unwrap_or(-1);
                    plan.launches.push(LaunchItem { app: a, workspace });
                }
            }
            LaunchMode::PerWindow => {
                plan.launches.extend(missing.iter().map(|&s| LaunchItem { app: a, workspace: hint(a, s) }));
            }
        }
    }
    // Ascending workspace order, like placement; launches without a hint last.
    plan.launches.sort_by_key(|l| if l.workspace < 0 { i32::MAX } else { l.workspace });
    plan
}

#[derive(Debug, Default)]
pub struct LaunchOutcome {
    /// Per launch item: whether the shell accepted the launch.
    pub started: Vec<bool>,
    /// New windows seen per app index.
    pub arrived: HashMap<usize, usize>,
    pub timed_out: bool,
}

pub async fn launch_and_wait(
    shim: &Shim,
    session: &Session,
    plan: &LaunchPlan,
    open: &[ShimWindow],
    timeout: Duration,
) -> Result<LaunchOutcome> {
    let mut outcome = LaunchOutcome::default();
    if plan.launches.is_empty() {
        return Ok(outcome);
    }
    let stream = shim.window_ready_stream().await?;
    let mut stream = std::pin::pin!(stream);

    for item in &plan.launches {
        let started = match &session.apps[item.app].launch {
            Launch::Desktop { desktop_id } => {
                shim.launch_app(desktop_id, item.workspace).await.unwrap_or(false)
            }
            Launch::Argv { argv } => shim.launch_command(argv, item.workspace).await.unwrap_or(false),
            Launch::None => false,
        };
        outcome.started.push(started);
        tokio::time::sleep(STAGGER).await;
    }

    // Wait only for apps that actually started.
    let mut waiting: HashMap<usize, usize> = HashMap::new(); // app -> windows still expected
    for (item, &started) in plan.launches.iter().zip(&outcome.started) {
        if started {
            let app = &session.apps[item.app];
            let open_now = open.iter().filter(|w| app_key(w) == app.id).count();
            waiting.insert(item.app, app.windows.len().saturating_sub(open_now));
        }
    }

    let deadline = Instant::now() + timeout;
    loop {
        let all_arrived = waiting.values().all(|&n| n == 0);
        let all_showed_one = waiting.keys().all(|a| outcome.arrived.get(a).is_some_and(|&n| n > 0));
        if all_arrived {
            break;
        }
        let now = Instant::now();
        if now >= deadline {
            outcome.timed_out = true;
            break;
        }
        match tokio::time::timeout(QUIET.min(deadline - now), stream.next()).await {
            Ok(Some(window)) => {
                let key = app_key(&window);
                if let Some(a) = session.apps.iter().position(|app| app.id == key) {
                    *outcome.arrived.entry(a).or_default() += 1;
                    if let Some(n) = waiting.get_mut(&a) {
                        *n = n.saturating_sub(1);
                    }
                }
            }
            Ok(None) => break,
            // Quiet period over: done if every launched app has shown up
            // (apps that restore fewer windows than saved end here).
            Err(_) if all_showed_one => break,
            Err(_) => {}
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn slot(title: &str, workspace: i32) -> WindowSlot {
        WindowSlot {
            matcher: WindowMatch {
                wm_class: None,
                wm_class_instance: None,
                gtk_app_id: None,
                sandboxed_app_id: None,
                title: Some(title.into()),
            },
            monitor_key: "primary".into(),
            workspace,
            on_all_workspaces: workspace < 0,
            rect_rel: RelRect { x: 0.0, y: 0.0, w: 0.5, h: 0.5 },
            rect_abs: Rect { x: 0, y: 0, w: 100, h: 100 },
            maximized: Maximized::None,
            fullscreen: false,
            minimized: false,
            above: false,
        }
    }

    fn app(id: &str, launch: Launch, mode: LaunchMode, windows: Vec<WindowSlot>) -> App {
        App { id: id.into(), launch, launch_mode: mode, windows }
    }

    fn desktop(id: &str) -> Launch {
        Launch::Desktop { desktop_id: id.into() }
    }

    fn session(apps: Vec<App>) -> Session {
        let rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        Session {
            schema: SCHEMA,
            name: "t".into(),
            created: String::new(),
            host: String::new(),
            shell_version: String::new(),
            workspaces: Workspaces { count: 4, dynamic: true, only_on_primary: true },
            monitors: vec![Monitor {
                key: "primary".into(),
                connector: "DP-1".into(),
                display_name: None,
                rect,
                work_area: rect,
                scale: 1.0,
                primary: true,
            }],
            apps,
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
    fn launches_only_missing_windows_in_workspace_order() {
        let s = session(vec![
            app(
                "term.desktop",
                desktop("term.desktop"),
                LaunchMode::PerWindow,
                vec![slot("a", 4), slot("b", 0), slot("c", 2)],
            ),
            app("editor.desktop", desktop("editor.desktop"), LaunchMode::PerWindow, vec![slot("x", 0)]),
        ]);
        let plan = launch_plan(&s, &[open(1, "term.desktop", "b")], true);
        // Workspaces 0, 2, 4 compact to 0, 1, 2.
        assert_eq!(
            plan.launches,
            [
                LaunchItem { app: 1, workspace: 0 },
                LaunchItem { app: 0, workspace: 1 },
                LaunchItem { app: 0, workspace: 2 },
            ]
        );
        assert!(plan.not_launchable.is_empty() && plan.already_running.is_empty());
    }

    #[test]
    fn self_restoring_apps_launch_once_and_not_when_running() {
        let s = session(vec![
            app(
                "firefox.desktop",
                desktop("firefox.desktop"),
                LaunchMode::Once,
                vec![slot("a", 2), slot("b", 1)],
            ),
            app("idea.desktop", desktop("idea.desktop"), LaunchMode::Once, vec![slot("p", 3), slot("q", 3)]),
        ]);
        let plan = launch_plan(&s, &[open(1, "idea.desktop", "p")], false);
        assert_eq!(plan.launches, [LaunchItem { app: 0, workspace: 1 }]);
        assert_eq!(plan.already_running, [1]);
    }

    #[test]
    fn apps_without_launch_info_are_reported_and_sticky_ones_get_no_hint() {
        let s = session(vec![
            app("mystery", Launch::None, LaunchMode::PerWindow, vec![slot("m", 0)]),
            app("pinned.desktop", desktop("pinned.desktop"), LaunchMode::PerWindow, vec![slot("p", -1)]),
        ]);
        let plan = launch_plan(&s, &[], true);
        assert_eq!(plan.not_launchable, [0]);
        assert_eq!(plan.launches, [LaunchItem { app: 1, workspace: -1 }]);
    }
}
