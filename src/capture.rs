//! Turns what the shim and DisplayConfig report into a [`Session`].

use crate::model::{
    App, Launch, LaunchMode, Monitor, RelRect, SCHEMA, Session, WindowMatch, WindowSlot, Workspaces,
};
use crate::shim::{ShimWindow, ShimWorkspaces};

pub struct Meta {
    pub name: String,
    pub created: String,
    pub host: String,
    pub shell_version: String,
}

/// Shell assigns `window:N` ids to windows it cannot map to a desktop file.
fn desktop_id(window: &ShimWindow) -> Option<&str> {
    window.app_id.as_deref().filter(|id| !id.starts_with("window:"))
}

/// Groups windows into apps; also used to match open windows to saved ones.
pub fn app_key(window: &ShimWindow) -> String {
    desktop_id(window).or(window.wm_class.as_deref()).unwrap_or("unknown").to_owned()
}

/// `cmdline` returns the argv of a pid; injected so tests do not need /proc.
pub fn build_session(
    meta: Meta,
    mut windows: Vec<ShimWindow>,
    monitors: Vec<Monitor>,
    workspaces: &ShimWorkspaces,
    cmdline: impl Fn(i32) -> Option<Vec<String>>,
) -> Session {
    // Ascending workspace order is also the order restore must place them in.
    windows.sort_by_key(|w| (w.workspace, w.stable_seq));

    let mut apps: Vec<App> = Vec::new();
    for window in &windows {
        let key = app_key(window);
        let index = match apps.iter().position(|a| a.id == key) {
            Some(index) => index,
            None => {
                let launch = match desktop_id(window) {
                    Some(id) => Launch::Desktop { desktop_id: id.to_owned() },
                    None => cmdline(window.pid)
                        .filter(|argv| !argv.is_empty())
                        .map_or(Launch::None, |argv| Launch::Argv { argv }),
                };
                apps.push(App { id: key, launch, launch_mode: LaunchMode::default(), windows: Vec::new() });
                apps.len() - 1
            }
        };

        let monitor = usize::try_from(window.monitor).ok().and_then(|i| monitors.get(i));
        apps[index].windows.push(WindowSlot {
            matcher: WindowMatch {
                wm_class: window.wm_class.clone(),
                wm_class_instance: window.wm_class_instance.clone(),
                gtk_app_id: window.gtk_app_id.clone(),
                sandboxed_app_id: window.sandboxed_app_id.clone(),
                title: window.title.clone(),
            },
            monitor_key: monitor.map_or_else(|| format!("index:{}", window.monitor), |m| m.key.clone()),
            workspace: window.workspace,
            on_all_workspaces: window.on_all_workspaces,
            rect_rel: RelRect::from_abs(window.frame, monitor.map_or(window.frame, |m| m.work_area)),
            rect_abs: window.frame,
            maximized: window.maximized,
            fullscreen: window.fullscreen,
            minimized: window.minimized,
            above: window.above,
        });
    }

    Session {
        schema: SCHEMA,
        name: meta.name,
        created: meta.created,
        host: meta.host,
        shell_version: meta.shell_version,
        workspaces: Workspaces {
            count: workspaces.count,
            dynamic: workspaces.dynamic,
            only_on_primary: workspaces.only_on_primary,
        },
        monitors,
        apps,
    }
}

pub fn proc_cmdline(pid: i32) -> Option<Vec<String>> {
    if pid <= 0 {
        return None;
    }
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|&b| b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Maximized, Rect};

    fn window(seq: u32, app_id: Option<&str>, wm_class: &str, workspace: i32, monitor: i32) -> ShimWindow {
        ShimWindow {
            id: seq.to_string(),
            stable_seq: seq,
            app_id: app_id.map(Into::into),
            wm_class: Some(wm_class.into()),
            wm_class_instance: None,
            gtk_app_id: None,
            sandboxed_app_id: None,
            title: Some(format!("title {seq}")),
            pid: 100 + seq as i32,
            client: "wayland".into(),
            workspace,
            on_all_workspaces: workspace < 0,
            monitor,
            frame: Rect { x: 1728 + 480, y: 886 + 270, w: 960, h: 540 },
            maximized: Maximized::None,
            fullscreen: false,
            minimized: false,
            above: false,
        }
    }

    fn monitors() -> Vec<Monitor> {
        let monitor = |key: &str, rect: Rect| Monitor {
            key: key.into(),
            connector: String::new(),
            display_name: None,
            rect,
            work_area: rect,
            scale: 1.0,
            primary: false,
        };
        vec![
            monitor("left", Rect { x: 0, y: 0, w: 1728, h: 3072 }),
            monitor("right", Rect { x: 1728, y: 886, w: 1920, h: 1080 }),
        ]
    }

    fn meta() -> Meta {
        Meta {
            name: "test".into(),
            created: "2026-10-30T17:00:00Z".into(),
            host: "host".into(),
            shell_version: "51.1".into(),
        }
    }

    fn workspaces() -> ShimWorkspaces {
        ShimWorkspaces { count: 3, active: 0, dynamic: true, only_on_primary: true }
    }

    #[test]
    fn groups_windows_by_app_in_workspace_order() {
        let windows = vec![
            window(3, Some("firefox.desktop"), "firefox", 1, 1),
            window(1, Some("firefox.desktop"), "firefox", 0, 1),
            window(2, Some("org.gnome.Nautilus.desktop"), "org.gnome.Nautilus", -1, 0),
        ];
        let session = build_session(meta(), windows, monitors(), &workspaces(), |_| None);

        let ids: Vec<_> = session.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["org.gnome.Nautilus.desktop", "firefox.desktop"]);
        let firefox = &session.apps[1];
        assert_eq!(firefox.launch, Launch::Desktop { desktop_id: "firefox.desktop".into() });
        let titles: Vec<_> = firefox.windows.iter().map(|w| w.matcher.title.as_deref().unwrap()).collect();
        assert_eq!(titles, ["title 1", "title 3"]);
        assert_eq!(firefox.windows[0].monitor_key, "right");
        assert_eq!(firefox.windows[0].rect_rel, RelRect { x: 0.25, y: 0.25, w: 0.5, h: 0.5 });
        assert_eq!(session.window_count(), 3);
    }

    #[test]
    fn windows_without_desktop_file_fall_back_to_argv() {
        let windows =
            vec![window(1, Some("window:7"), "sublime_merge", 0, 1), window(2, None, "mystery", 0, 1)];
        let session = build_session(meta(), windows, monitors(), &workspaces(), |pid| {
            (pid == 101).then(|| vec!["/opt/sublime_merge/sublime_merge".into()])
        });

        assert_eq!(session.apps[0].id, "sublime_merge");
        assert_eq!(
            session.apps[0].launch,
            Launch::Argv { argv: vec!["/opt/sublime_merge/sublime_merge".into()] }
        );
        assert_eq!(session.apps[1].launch, Launch::None);
    }

    #[test]
    fn unknown_monitor_index_is_kept_as_index_key() {
        let session =
            build_session(meta(), vec![window(1, None, "x", 0, 5)], monitors(), &workspaces(), |_| None);
        assert_eq!(session.apps[0].windows[0].monitor_key, "index:5");
    }
}
