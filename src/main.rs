mod capture;
mod display;
mod model;
mod shim;
mod store;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use zbus::Connection;

use crate::model::{Launch, Maximized, Session, WindowSlot};
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
    List,
    /// Show the windows of a saved session.
    Show {
        name: String,
        /// Print the raw session file.
        #[arg(long)]
        json: bool,
    },
    /// Delete a saved session (a copy is kept in backups/).
    Delete { name: String },
    /// Check the shim extension, monitors and session storage.
    Doctor,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store::open_default()?;
    match cli.command {
        Command::Save { name, force } => save(&store, name, force).await,
        Command::List => list(&store),
        Command::Show { name, json } => show(&store, &name, json),
        Command::Delete { name } => {
            store.delete(&name)?;
            println!("deleted '{name}'");
            Ok(())
        }
        Command::Doctor => doctor(&store).await,
    }
}

async fn save(store: &Store, name: String, force: bool) -> Result<()> {
    // Fail before talking to the shell if the save would be refused anyway.
    if store.exists(&name)? && !force {
        anyhow::bail!("session '{name}' already exists (use --force to overwrite)");
    }

    let conn = Connection::session().await.context("cannot connect to the session bus")?;
    let shim = Shim::connect(&conn).await?;
    let (windows, shim_monitors, workspaces) =
        tokio::try_join!(shim.windows(), shim.monitors(), shim.workspaces())?;
    let displays = display::current_displays(&conn).await.unwrap_or_else(|e| {
        eprintln!("warning: no monitor identities from DisplayConfig ({e}); using monitor indices");
        Vec::new()
    });

    let meta = capture::Meta {
        name,
        created: jiff::Timestamp::now().round(jiff::Unit::Second)?.to_string(),
        host: hostname(),
        shell_version: shim.shell_version.clone(),
    };
    let monitors = display::correlate(&shim_monitors, &displays);
    let session = capture::build_session(meta, windows, monitors, &workspaces, capture::proc_cmdline);
    let path = store.save(&session, force)?;

    println!(
        "saved '{}': {} windows from {} apps -> {}",
        session.name,
        session.window_count(),
        session.apps.len(),
        path.display()
    );
    for app in session.apps.iter().filter(|a| a.launch == Launch::None) {
        println!("  note: {} cannot be relaunched (no desktop file or command line)", app.id);
    }
    Ok(())
}

fn list(store: &Store) -> Result<()> {
    let sessions = store.list()?;
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
            format!("{}x{}+{}+{}", w.rect_abs.w, w.rect_abs.h, w.rect_abs.x, w.rect_abs.y),
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
