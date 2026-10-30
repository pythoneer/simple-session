//! Client for the shim extension (`de.dbensing.SimpleSession.Shim1`).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use zbus::{Connection, proxy};

use crate::model::{Maximized, Rect};

pub const API_VERSION: u32 = 1;
const EXTENSION_UUID: &str = "simple-session@dbensing.de";

#[proxy(
    interface = "de.dbensing.SimpleSession.Shim1",
    default_service = "org.gnome.Shell",
    default_path = "/de/dbensing/SimpleSession/Shim"
)]
trait Shim1 {
    fn get_version(&self) -> zbus::Result<(u32, String)>;
    fn list_windows(&self) -> zbus::Result<String>;
    fn list_monitors(&self) -> zbus::Result<String>;
    fn get_workspaces(&self) -> zbus::Result<String>;
    fn place_window(&self, id: &str, placement_json: &str) -> zbus::Result<bool>;
    fn set_window_state(&self, id: &str, state_json: &str) -> zbus::Result<bool>;
    fn launch_app(&self, desktop_id: &str, workspace: i32) -> zbus::Result<bool>;
    fn launch_command(&self, argv: &[&str], workspace: i32) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn window_ready(&self, window_json: String) -> zbus::Result<()>;
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShimWindow {
    pub id: String,
    pub stable_seq: u32,
    pub app_id: Option<String>,
    pub wm_class: Option<String>,
    pub wm_class_instance: Option<String>,
    pub gtk_app_id: Option<String>,
    pub sandboxed_app_id: Option<String>,
    pub title: Option<String>,
    pub pid: i32,
    #[allow(dead_code)] // reported by the shim; useful when debugging placement
    pub client: String,
    pub workspace: i32,
    pub on_all_workspaces: bool,
    pub monitor: i32,
    pub frame: Rect,
    pub maximized: Maximized,
    pub fullscreen: bool,
    pub minimized: bool,
    pub above: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShimMonitor {
    pub index: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub work_area: Rect,
    pub scale: f64,
    pub primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShimWorkspaces {
    pub count: i32,
    pub active: i32,
    pub dynamic: bool,
    pub only_on_primary: bool,
}

/// Argument of `PlaceWindow`; absent fields are left unchanged by the shim.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Placement {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_all_workspaces: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    pub maximized: Maximized,
    pub fullscreen: bool,
    pub minimized: bool,
    pub above: bool,
}

pub struct Shim {
    proxy: Shim1Proxy<'static>,
    pub shell_version: String,
}

impl Shim {
    /// Connects and checks the interface version.
    pub async fn connect(conn: &Connection) -> Result<Self> {
        let proxy = Shim1Proxy::new(conn).await?;
        let (api, shell_version) = proxy.get_version().await.with_context(|| {
            format!(
                "the shim extension is not reachable; is {EXTENSION_UUID} installed and \
                 enabled? (on Wayland a newly installed extension needs a re-login)"
            )
        })?;
        if api != API_VERSION {
            bail!("shim speaks API version {api}, this CLI needs {API_VERSION}; update both");
        }
        Ok(Shim { proxy, shell_version })
    }

    pub async fn windows(&self) -> Result<Vec<ShimWindow>> {
        Ok(serde_json::from_str(&self.proxy.list_windows().await?)?)
    }

    pub async fn monitors(&self) -> Result<Vec<ShimMonitor>> {
        Ok(serde_json::from_str(&self.proxy.list_monitors().await?)?)
    }

    pub async fn workspaces(&self) -> Result<ShimWorkspaces> {
        Ok(serde_json::from_str(&self.proxy.get_workspaces().await?)?)
    }

    /// Launches a desktop app; `false` if the shell does not know the id.
    /// `workspace` is a hint (-1 for none) that apps with startup
    /// notification follow.
    pub async fn launch_app(&self, desktop_id: &str, workspace: i32) -> Result<bool> {
        Ok(self.proxy.launch_app(desktop_id, workspace).await?)
    }

    pub async fn launch_command(&self, argv: &[String], workspace: i32) -> Result<bool> {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        Ok(self.proxy.launch_command(&argv, workspace).await?)
    }

    /// Windows as they get mapped. Subscribe before launching anything so no
    /// window is missed.
    pub async fn window_ready_stream(&self) -> Result<impl futures_util::Stream<Item = ShimWindow> + use<>> {
        use futures_util::StreamExt;
        let stream = self.proxy.receive_window_ready().await?;
        Ok(stream.filter_map(|signal| async move {
            let json = signal.args().ok()?.window_json;
            serde_json::from_str(&json).ok()
        }))
    }

    pub async fn place_window(&self, id: &str, placement: &Placement) -> Result<()> {
        self.proxy.place_window(id, &serde_json::to_string(placement)?).await?;
        Ok(())
    }

    /// Sets only maximized/fullscreen/minimized/above (the shim changes only
    /// states that differ).
    pub async fn set_window_state(&self, id: &str, placement: &Placement) -> Result<()> {
        let state = serde_json::json!({
            "maximized": placement.maximized,
            "fullscreen": placement.fullscreen,
            "minimized": placement.minimized,
            "above": placement.above,
        });
        self.proxy.set_window_state(id, &state.to_string()).await?;
        Ok(())
    }
}
