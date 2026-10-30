mod apply;
mod capture;
mod config;
mod diff;
mod display;
mod layout;
mod matching;
mod model;
mod restore;
mod shim;
mod store;

use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use zbus::Connection;

use crate::config::Config;
use crate::layout::MapReason;
use crate::model::{Launch, Maximized, Monitor, Rect, Session, WindowSlot};
use crate::shim::Shim;
use crate::store::Store;

/// Save the open application windows as named sessions and bring them back.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Save the current windows as a session.
    Save {
        name: String,
        /// Overwrite an existing session (the old one is kept in backups/).
        #[arg(short, long)]
        force: bool,
    },
    /// List saved sessions.
    List {
        /// Machine-readable output (used by the top-bar menu).
        #[arg(long)]
        json: bool,
    },
    /// Show the windows of a saved session.
    Show {
        name: String,
        /// Print the raw session file.
        #[arg(long)]
        json: bool,
    },
    /// Move already open windows to where a session has them (launches nothing).
    Apply {
        name: String,
        /// Only print what would be moved.
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
    /// Launch the apps of a session that are not open and place all windows.
    Restore {
        name: String,
        /// Only print what would be launched and moved.
        #[arg(short = 'n', long)]
        dry_run: bool,
        /// Seconds to wait for launched apps to show their windows.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    /// Show how a session differs from the current desktop or another session.
    Diff {
        name: String,
        /// Compare with this session instead of the current desktop.
        other: Option<String>,
    },
    /// Rename a saved session.
    Rename { old: String, new: String },
    /// Delete a saved session (a copy is kept in backups/).
    Delete { name: String },
    /// Check the shim extension, monitors and session storage.
    Doctor,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store::open_default()?;
    let config = Config::load_default()?;
    match cli.command {
        Command::Save { name, force } => save(&store, &config, name, force).await,
        Command::List { json } => list(&store, json),
        Command::Show { name, json } => show(&store, &name, json),
        Command::Apply { name, dry_run } => apply(&store, &config, &name, dry_run).await,
        Command::Restore { name, dry_run, timeout } => {
            restore(&store, &config, &name, dry_run, Duration::from_secs(timeout)).await
        }
        Command::Diff { name, other } => diff(&store, &config, &name, other.as_deref()).await,
        Command::Rename { old, new } => {
            store.rename(&old, &new)?;
            println!("renamed '{old}' to '{new}'");
            Ok(())
        }
        Command::Delete { name } => {
            store.delete(&name)?;
            println!("deleted '{name}'");
            Ok(())
        }
        Command::Doctor => doctor(&store).await,
    }
}

async fn save(store: &Store, config: &Config, name: String, force: bool) -> Result<()> {
    // Fail before talking to the shell if the save would be refused anyway.
    if store.exists(&name)? && !force {
        anyhow::bail!("session '{name}' already exists (use --force to overwrite)");
    }

    let session = capture_current(config, name).await?;
    let path = store.save(&session, force)?;

    for app in session.apps.iter().filter(|a| a.launch == Launch::None) {
        println!("note: {} cannot be relaunched (no desktop file or command line)", app.id);
    }
    println!("{}", path.display());
    println!("saved '{}': {} windows from {} apps", session.name, session.window_count(), session.apps.len());
    Ok(())
}

/// The current desktop as a session.
async fn capture_current(config: &Config, name: String) -> Result<Session> {
    let conn = Connection::session().await.context("cannot connect to the session bus")?;
    let shim = Shim::connect(&conn).await?;
    let (windows, workspaces) = tokio::try_join!(shim.windows(), shim.workspaces())?;
    let monitors = current_monitors(&conn, &shim).await?;

    let meta = capture::Meta {
        name,
        created: jiff::Timestamp::now().round(jiff::Unit::Second)?.to_string(),
        host: hostname(),
        shell_version: shim.shell_version.clone(),
    };
    Ok(capture::build_session(meta, windows, monitors, &workspaces, config, capture::proc_cmdline))
}

async fn diff(store: &Store, config: &Config, name: &str, other: Option<&str>) -> Result<()> {
    let mut old = store.load(name)?;
    config.apply_to(&mut old);
    let (new, label) = match other {
        Some(other) => (store.load(other)?, format!("'{other}'")),
        None => (capture_current(config, "current".to_owned()).await?, "the current desktop".to_owned()),
    };

    let result = diff::diff(&old, &new);
    let line = |session: &Session, (a, s): matching::SlotRef| {
        let title = session.apps[a].windows[s].matcher.title.as_deref().unwrap_or("");
        format!("{:32} {}", truncate(&session.apps[a].id, 32), truncate(title, 44))
    };
    let (mut removed, mut added) = (0, 0);
    for change in &result.changes {
        match change {
            diff::Change::Changed { old: r, details, .. } => {
                println!("~ {}", line(&old, *r));
                for detail in details {
                    println!("      {detail}");
                }
            }
            diff::Change::Removed(r) => {
                removed += 1;
                println!("- {}", line(&old, *r));
            }
            diff::Change::Added(r) => {
                added += 1;
                println!("+ {}", line(&new, *r));
            }
        }
    }
    println!(
        "'{name}' vs {label}: {} unchanged, {} changed, {removed} only in '{name}' (-), {added} only in {label} (+)",
        result.unchanged,
        result.changes.len() - removed - added,
    );
    Ok(())
}

async fn current_monitors(conn: &Connection, shim: &Shim) -> Result<Vec<Monitor>> {
    let displays = display::current_displays(conn).await.unwrap_or_else(|e| {
        eprintln!("warning: no monitor identities from DisplayConfig ({e}); using monitor indices");
        Vec::new()
    });
    Ok(display::correlate(&shim.monitors().await?, &displays))
}

async fn apply(store: &Store, config: &Config, name: &str, dry_run: bool) -> Result<()> {
    let mut session = store.load(name)?;
    config.apply_to(&mut session);
    let conn = Connection::session().await.context("cannot connect to the session bus")?;
    let shim = Shim::connect(&conn).await?;
    let (windows, workspaces) = tokio::try_join!(shim.windows(), shim.workspaces())?;
    let monitors = current_monitors(&conn, &shim).await?;

    let mut plan = apply::plan(&session, windows, monitors, workspaces.dynamic);
    print_monitor_mapping(&session, &plan);
    if !dry_run {
        apply::execute(&shim, &mut plan).await?;
    }
    finish_placement(&session, &plan, dry_run)
}

async fn restore(store: &Store, config: &Config, name: &str, dry_run: bool, timeout: Duration) -> Result<()> {
    let mut session = store.load(name)?;
    config.apply_to(&mut session);
    let conn = Connection::session().await.context("cannot connect to the session bus")?;
    let shim = Shim::connect(&conn).await?;
    let (windows, workspaces) = tokio::try_join!(shim.windows(), shim.workspaces())?;
    let monitors = current_monitors(&conn, &shim).await?;

    let launch_plan = restore::launch_plan(&session, &windows, workspaces.dynamic);
    for &a in &launch_plan.already_running {
        println!(
            "skip  {:32} already running; it reopens its own windows, so it is not launched again",
            truncate(&session.apps[a].id, 32)
        );
    }
    for &a in &launch_plan.not_launchable {
        println!(
            "skip  {:32} cannot be launched (no desktop file or command)",
            truncate(&session.apps[a].id, 32)
        );
    }

    if dry_run {
        for item in &launch_plan.launches {
            println!(
                "launch {:31} ws {:>3}",
                truncate(&session.apps[item.app].id, 31),
                workspace_hint(item.workspace)
            );
        }
        let plan = apply::plan(&session, windows, monitors, workspaces.dynamic);
        print_monitor_mapping(&session, &plan);
        println!(
            "{} launches; windows that are open now would be placed like this:",
            launch_plan.launches.len()
        );
        return finish_placement(&session, &plan, true);
    }

    if !launch_plan.launches.is_empty() {
        let outcome = restore::launch_and_wait(&shim, &session, &launch_plan, &windows, timeout).await?;
        for (item, started) in launch_plan.launches.iter().zip(&outcome.started) {
            println!(
                "{} {:31} ws {:>3}",
                if *started { "launch" } else { "FAIL  " },
                truncate(&session.apps[item.app].id, 31),
                workspace_hint(item.workspace)
            );
        }
        if outcome.timed_out {
            println!("timed out after {}s waiting for windows; placing what is there", timeout.as_secs());
        }
    }

    let windows = shim.windows().await?;
    let mut plan = apply::plan(&session, windows, monitors, workspaces.dynamic);
    print_monitor_mapping(&session, &plan);
    apply::execute(&shim, &mut plan).await?;
    finish_placement(&session, &plan, false)
}

fn workspace_hint(ws: i32) -> String {
    if ws < 0 { "-".to_owned() } else { ws.to_string() }
}

fn print_monitor_mapping(session: &Session, plan: &apply::Plan) {
    for (saved, &(index, reason)) in session.monitors.iter().zip(&plan.monitor_targets) {
        if reason != MapReason::Exact {
            let current = &plan.current_monitors[index];
            println!("monitor {} -> {} ({reason})", monitor_name(saved), monitor_name(current));
        }
    }
}

/// Prints the placement report; exits with status 1 if windows failed.
fn finish_placement(session: &Session, plan: &apply::Plan, dry_run: bool) -> Result<()> {
    for p in &plan.windows {
        let (a, s) = p.slot;
        let slot = &session.apps[a].windows[s];
        let status = match (dry_run, p.done_in_pass) {
            (true, _) => "plan",
            (false, Some(1)) => "ok",
            (false, Some(_)) => "ok*",
            (false, None) => "FAIL",
        };
        let workspace = match (p.placement.workspace, p.placement.on_all_workspaces) {
            (Some(ws), _) => ws.to_string(),
            (None, Some(true)) => "all".to_owned(),
            (None, _) => "-".to_owned(),
        };
        println!(
            "{status:4}  {:32} ws {:>3}  {:6} {:20} {}",
            truncate(&session.apps[a].id, 32),
            workspace,
            monitor_name(&plan.current_monitors[p.monitor]),
            p.placement.rect.map(geometry).unwrap_or_default(),
            state_label(slot)
        );
        if !dry_run && p.done_in_pass.is_none() {
            match &p.last_seen {
                Some(w) => println!(
                    "      is at ws {}  {}  maximized={:?} minimized={}",
                    w.workspace,
                    geometry(w.frame),
                    w.maximized,
                    w.minimized
                ),
                None => println!("      window is gone"),
            }
        }
    }
    for &(a, s) in &plan.unmatched_slots {
        let title = session.apps[a].windows[s].matcher.title.as_deref().unwrap_or("");
        println!("open? {:32} {}", truncate(&session.apps[a].id, 32), truncate(title, 44));
    }

    let failed = plan.windows.iter().filter(|p| p.done_in_pass.is_none()).count();
    if dry_run {
        println!(
            "{} windows would be placed, {} saved windows are not open",
            plan.windows.len(),
            plan.unmatched_slots.len()
        );
    } else {
        if plan.windows.iter().any(|p| p.done_in_pass.is_some_and(|pass| pass > 1)) {
            println!("(ok* = placed after a retry)");
        }
        // Last line doubles as the summary the top-bar menu shows.
        println!(
            "{} placed, {} failed, {} saved windows not open",
            plan.windows.len() - failed,
            failed,
            plan.unmatched_slots.len()
        );
        if failed > 0 {
            std::process::exit(1);
        }
    }
    Ok(())
}

fn monitor_name(m: &Monitor) -> String {
    if m.connector.is_empty() { m.key.clone() } else { m.connector.clone() }
}

fn geometry(r: Rect) -> String {
    format!("{}x{}+{}+{}", r.w, r.h, r.x, r.y)
}

fn list(store: &Store, json: bool) -> Result<()> {
    let sessions = store.list()?;
    if json {
        let entries: Vec<_> = sessions
            .iter()
            .map(|(name, session)| match session {
                Ok(s) => serde_json::json!({
                    "name": name,
                    "created": s.created,
                    "windows": s.window_count(),
                    "apps": s.apps.len(),
                }),
                Err(e) => serde_json::json!({ "name": name, "error": format!("{e:#}") }),
            })
            .collect();
        println!("{}", serde_json::to_string(&entries)?);
        return Ok(());
    }
    if sessions.is_empty() {
        println!("no sessions saved yet (in {})", store.dir().display());
        return Ok(());
    }
    let width = sessions.iter().map(|(n, _)| n.len()).max().unwrap_or(0).max(4);
    println!("{:width$}  {:20}  {:>7}  {:>4}", "NAME", "SAVED", "WINDOWS", "APPS");
    for (name, session) in sessions {
        match session {
            Ok(s) => println!(
                "{name:width$}  {:20}  {:>7}  {:>4}",
                local_time(&s.created),
                s.window_count(),
                s.apps.len()
            ),
            Err(e) => println!("{name:width$}  unreadable: {e:#}"),
        }
    }
    Ok(())
}

fn show(store: &Store, name: &str, json: bool) -> Result<()> {
    let session = store.load(name)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&session)?);
        return Ok(());
    }

    println!(
        "{}: saved {} on {}, GNOME {}, {} windows",
        session.name,
        local_time(&session.created),
        session.host,
        session.shell_version,
        session.window_count()
    );
    println!("\nMonitors:");
    for m in &session.monitors {
        println!(
            "  {:6} {:28} {}x{}+{}+{} @{}{}",
            if m.connector.is_empty() { "?" } else { &m.connector },
            truncate(m.display_name.as_deref().unwrap_or(&m.key), 28),
            m.rect.w,
            m.rect.h,
            m.rect.x,
            m.rect.y,
            m.scale,
            if m.primary { "  primary" } else { "" }
        );
    }

    println!("\nWindows:");
    println!("  {:32} {:>3}  {:6} {:20} {:9} LAUNCH", "APP / TITLE", "WS", "MON", "GEOMETRY", "STATE");
    let mut rows: Vec<_> =
        session.apps.iter().flat_map(|app| app.windows.iter().map(move |w| (app, w))).collect();
    rows.sort_by_key(|(_, w)| w.workspace);
    for (app, w) in rows {
        println!(
            "  {:32} {:>3}  {:6} {:20} {:9} {}",
            truncate(&app.id, 32),
            if w.on_all_workspaces { "all".to_owned() } else { w.workspace.to_string() },
            monitor_label(&session, w),
            geometry(w.rect_abs),
            state_label(w),
            match &app.launch {
                Launch::Desktop { .. } => "desktop",
                Launch::Argv { .. } => "command",
                Launch::None => "-",
            }
        );
        if let Some(title) = w.matcher.title.as_deref().filter(|t| !t.is_empty()) {
            println!("    {}", truncate(title, 76));
        }
    }
    Ok(())
}

async fn doctor(store: &Store) -> Result<()> {
    let conn = Connection::session().await.context("cannot connect to the session bus")?;
    let shim = match Shim::connect(&conn).await {
        Ok(shim) => {
            println!("ok    shim reachable (API {}, GNOME Shell {})", shim::API_VERSION, shim.shell_version);
            Some(shim)
        }
        Err(e) => {
            println!("FAIL  {e:#}");
            None
        }
    };

    let displays = match display::current_displays(&conn).await {
        Ok(displays) => {
            println!("ok    DisplayConfig: {} logical monitors", displays.len());
            displays
        }
        Err(e) => {
            println!("FAIL  DisplayConfig: {e:#}");
            Vec::new()
        }
    };

    if let Some(shim) = &shim {
        let monitors = display::correlate(&shim.monitors().await?, &displays);
        for m in &monitors {
            let status = if m.connector.is_empty() { "WARN " } else { "ok   " };
            println!(
                "{status} monitor {:6} {:28} {}x{}+{}+{} @{}{}",
                if m.connector.is_empty() { "?" } else { &m.connector },
                truncate(m.display_name.as_deref().unwrap_or(&m.key), 28),
                m.rect.w,
                m.rect.h,
                m.rect.x,
                m.rect.y,
                m.scale,
                if m.primary { " primary" } else { "" }
            );
        }
        let ws = shim.workspaces().await?;
        println!(
            "ok    workspaces: {} (active {}), dynamic={}, only-on-primary={}",
            ws.count, ws.active, ws.dynamic, ws.only_on_primary
        );
        println!("ok    {} windows visible to the shim", shim.windows().await?.len());
    }

    println!("ok    sessions stored in {}", store.dir().display());
    if shim.is_none() {
        std::process::exit(1);
    }
    Ok(())
}

fn monitor_label(session: &Session, w: &WindowSlot) -> String {
    session
        .monitors
        .iter()
        .find(|m| m.key == w.monitor_key)
        .map_or_else(|| "?".to_owned(), |m| m.connector.clone())
}

fn state_label(w: &WindowSlot) -> String {
    let mut parts = Vec::new();
    match w.maximized {
        Maximized::None => {}
        Maximized::Both => parts.push("max"),
        Maximized::Horizontal => parts.push("max-h"),
        Maximized::Vertical => parts.push("max-v"),
    }
    if w.fullscreen {
        parts.push("full");
    }
    if w.minimized {
        parts.push("min");
    }
    if w.above {
        parts.push("above");
    }
    if parts.is_empty() { "-".to_owned() } else { parts.join(",") }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        s.chars().take(max - 1).chain(std::iter::once('…')).collect()
    }
}

fn local_time(rfc3339: &str) -> String {
    rfc3339
        .parse::<jiff::Timestamp>()
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| rfc3339.to_owned())
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_owned()).unwrap_or_default()
}
