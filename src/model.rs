//! Session file format (schema 1). See the architecture doc for the rationale
//! behind each field.

use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A rectangle relative to a monitor's work area (0.0..1.0 when inside it).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RelRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RelRect {
    pub fn from_abs(rect: Rect, area: Rect) -> Self {
        let rel = |v: i32, origin: i32, len: i32| f64::from(v - origin) / f64::from(len.max(1));
        RelRect {
            x: rel(rect.x, area.x, area.w),
            y: rel(rect.y, area.y, area.h),
            w: rel(rect.w, 0, area.w),
            h: rel(rect.h, 0, area.h),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Maximized {
    #[default]
    None,
    Horizontal,
    Vertical,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub schema: u32,
    pub name: String,
    /// RFC 3339 timestamp (UTC).
    pub created: String,
    pub host: String,
    pub shell_version: String,
    pub workspaces: Workspaces,
    pub monitors: Vec<Monitor>,
    pub apps: Vec<App>,
}

impl Session {
    pub fn window_count(&self) -> usize {
        self.apps.iter().map(|a| a.windows.len()).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspaces {
    pub count: i32,
    pub dynamic: bool,
    pub only_on_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    /// `vendor:product:serial`, stable across connector renumbering.
    pub key: String,
    pub connector: String,
    pub display_name: Option<String>,
    pub rect: Rect,
    pub work_area: Rect,
    pub scale: f64,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    /// Shell app id (`firefox.desktop`), or the wm_class when the window has no
    /// desktop file.
    pub id: String,
    pub launch: Launch,
    pub launch_mode: LaunchMode,
    pub windows: Vec<WindowSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Launch {
    Desktop {
        desktop_id: String,
    },
    Argv {
        argv: Vec<String>,
    },
    /// Cannot be relaunched; only placed if already open.
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    /// One launch per saved window.
    #[default]
    PerWindow,
    /// Launch once; the app restores its own windows (browsers, IDEs).
    Once,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSlot {
    #[serde(rename = "match")]
    pub matcher: WindowMatch,
    pub monitor_key: String,
    /// -1 when on all workspaces.
    pub workspace: i32,
    pub on_all_workspaces: bool,
    pub rect_rel: RelRect,
    pub rect_abs: Rect,
    pub maximized: Maximized,
    pub fullscreen: bool,
    pub minimized: bool,
    pub above: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowMatch {
    pub wm_class: Option<String>,
    pub wm_class_instance: Option<String>,
    pub gtk_app_id: Option<String>,
    pub sandboxed_app_id: Option<String>,
    pub title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_rect_is_relative_to_work_area() {
        let area = Rect { x: 1728, y: 886, w: 1920, h: 1080 };
        let rel = RelRect::from_abs(Rect { x: 1728 + 480, y: 886 + 270, w: 960, h: 540 }, area);
        assert_eq!(rel, RelRect { x: 0.25, y: 0.25, w: 0.5, h: 0.5 });
    }

    #[test]
    fn launch_serializes_with_type_tag() {
        let json = serde_json::to_string(&Launch::Desktop { desktop_id: "a.desktop".into() }).unwrap();
        assert_eq!(json, r#"{"type":"desktop","desktop_id":"a.desktop"}"#);
        let none: Launch = serde_json::from_str(r#"{"type":"none"}"#).unwrap();
        assert_eq!(none, Launch::None);
    }
}
