//! Monitor identity from `org.gnome.Mutter.DisplayConfig`, which (unlike
//! window access) is available without the extension.

use std::collections::HashMap;

use anyhow::Result;
use zbus::OwnedVariant;
use zbus::{Connection, proxy};

use crate::model::{Monitor, Rect};
use crate::shim::ShimMonitor;

type MonitorSpec = (String, String, String, String); // connector, vendor, product, serial
type Mode = (String, i32, i32, f64, f64, Vec<f64>, HashMap<String, OwnedVariant>);
type PhysicalMonitor = (MonitorSpec, Vec<Mode>, HashMap<String, OwnedVariant>);
type LogicalMonitor = (i32, i32, f64, u32, bool, Vec<MonitorSpec>, HashMap<String, OwnedVariant>);

#[proxy(
    interface = "org.gnome.Mutter.DisplayConfig",
    default_service = "org.gnome.Mutter.DisplayConfig",
    default_path = "/org/gnome/Mutter/DisplayConfig"
)]
trait DisplayConfig {
    #[allow(clippy::type_complexity)]
    fn get_current_state(
        &self,
    ) -> zbus::Result<(u32, Vec<PhysicalMonitor>, Vec<LogicalMonitor>, HashMap<String, OwnedVariant>)>;
}

/// One logical monitor as seen by DisplayConfig.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub x: i32,
    pub y: i32,
    pub connector: String,
    pub vendor: String,
    pub product: String,
    pub serial: String,
    pub display_name: Option<String>,
}

impl DisplayInfo {
    pub fn key(&self) -> String {
        format!("{}:{}:{}", self.vendor, self.product, self.serial)
    }
}

pub async fn current_displays(conn: &Connection) -> Result<Vec<DisplayInfo>> {
    let proxy = DisplayConfigProxy::new(conn).await?;
    let (_serial, physical, logical, _props) = proxy.get_current_state().await?;

    let names: HashMap<String, String> = physical
        .iter()
        .filter_map(|(spec, _modes, props)| {
            let name = props.get("display-name")?;
            let name = String::try_from(name.try_clone().ok()?).ok()?;
            Some((spec.0.clone(), name))
        })
        .collect();

    Ok(logical
        .into_iter()
        .filter_map(|(x, y, _scale, _transform, _primary, specs, _props)| {
            // Mirrored outputs share a logical monitor; the first one names it.
            let (connector, vendor, product, serial) = specs.into_iter().next()?;
            Some(DisplayInfo {
                x,
                y,
                display_name: names.get(&connector).cloned(),
                connector,
                vendor,
                product,
                serial,
            })
        })
        .collect())
}

/// Joins the shim's monitor list (indices, geometry, work area) with the
/// DisplayConfig identities by logical origin, which is unique per monitor.
/// Monitors that cannot be matched get an `index:N` key.
pub fn correlate(shim: &[ShimMonitor], displays: &[DisplayInfo]) -> Vec<Monitor> {
    shim.iter()
        .map(|m| {
            let display = displays.iter().find(|d| d.x == m.x && d.y == m.y);
            Monitor {
                key: display.map_or_else(|| format!("index:{}", m.index), DisplayInfo::key),
                connector: display.map_or_else(String::new, |d| d.connector.clone()),
                display_name: display.and_then(|d| d.display_name.clone()),
                rect: Rect { x: m.x, y: m.y, w: m.w, h: m.h },
                work_area: m.work_area,
                scale: m.scale,
                primary: m.primary,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shim_monitor(index: i32, x: i32, y: i32, w: i32, h: i32) -> ShimMonitor {
        ShimMonitor { index, x, y, w, h, work_area: Rect { x, y, w, h }, scale: 1.0, primary: index == 1 }
    }

    fn display(x: i32, y: i32, connector: &str, vendor: &str, product: &str, serial: &str) -> DisplayInfo {
        DisplayInfo {
            x,
            y,
            connector: connector.into(),
            vendor: vendor.into(),
            product: product.into(),
            serial: serial.into(),
            display_name: None,
        }
    }

    #[test]
    fn correlates_by_origin_regardless_of_order() {
        // Recorded from the real setup: portrait DP-2 left, primary DP-1 right.
        let shim = [shim_monitor(0, 0, 0, 1728, 3072), shim_monitor(1, 1728, 886, 1920, 1080)];
        let displays = [
            display(1728, 886, "DP-1", "GBT", "MO27U2", "25130B000566"),
            display(0, 0, "DP-2", "GSM", "LG Ultra HD", "0x01010101"),
        ];
        let monitors = correlate(&shim, &displays);
        assert_eq!(monitors[0].key, "GSM:LG Ultra HD:0x01010101");
        assert_eq!(monitors[0].connector, "DP-2");
        assert_eq!(monitors[1].key, "GBT:MO27U2:25130B000566");
        assert!(monitors[1].primary);
    }

    #[test]
    fn unmatched_monitor_gets_index_key() {
        let monitors = correlate(&[shim_monitor(0, 0, 0, 800, 600)], &[]);
        assert_eq!(monitors[0].key, "index:0");
        assert_eq!(monitors[0].connector, "");
    }
}
