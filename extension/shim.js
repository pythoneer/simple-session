// de.dbensing.SimpleSession.Shim1: reports facts and executes commands.
// No matching, retries, timing or storage here; that is the CLI's job.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';

import * as Config from 'resource:///org/gnome/shell/misc/config.js';

import * as Compat from './compat.js';

const API_VERSION = 1;
export const OBJECT_PATH = '/de/dbensing/SimpleSession/Shim';

function isTracked(window) {
    return window.get_window_type() === Meta.WindowType.NORMAL &&
        !window.is_skip_taskbar() &&
        !window.get_transient_for();
}

function rect(r) {
    return {x: r.x, y: r.y, w: r.width, h: r.height};
}

function describeWindow(window) {
    const app = Shell.WindowTracker.get_default().get_window_app(window);
    const workspace = window.get_workspace();
    return {
        id: String(window.get_id()),
        stable_seq: window.get_stable_sequence(),
        app_id: app?.get_id() ?? null,
        wm_class: window.get_wm_class(),
        wm_class_instance: window.get_wm_class_instance(),
        gtk_app_id: window.get_gtk_application_id(),
        sandboxed_app_id: window.get_sandboxed_app_id(),
        title: window.get_title(),
        pid: window.get_pid(),
        client: Compat.clientType(window),
        workspace: window.is_on_all_workspaces() || !workspace ? -1 : workspace.index(),
        on_all_workspaces: window.is_on_all_workspaces(),
        monitor: window.get_monitor(),
        frame: rect(window.get_frame_rect()),
        maximized: Compat.getMaximized(window),
        fullscreen: window.is_fullscreen(),
        minimized: window.minimized,
        above: window.is_above(),
    };
}

function findWindow(id) {
    const window = global.display.list_all_windows()
        .find(w => String(w.get_id()) === id);
    if (!window)
        throw new Error(`No window with id ${id}`);
    return window;
}

function ensureWorkspaces(window, count) {
    // Same approach as the auto-move-windows extension: with dynamic
    // workspaces the last workspace must be occupied before appending.
    const manager = global.workspace_manager;
    for (let i = manager.n_workspaces; i < count; i++) {
        window?.change_workspace_by_index(i - 1, false);
        manager.append_new_workspace(false, 0);
    }
}

// Only touches states that differ: e.g. unmake_fullscreen() on a window that
// is not fullscreen cancels a pending resize of a Wayland client.
function applyState(window, state) {
    if ('above' in state && state.above !== window.is_above()) {
        if (state.above)
            window.make_above();
        else
            window.unmake_above();
    }
    if ('maximized' in state && state.maximized !== Compat.getMaximized(window))
        Compat.setMaximized(window, state.maximized);
    if ('fullscreen' in state && state.fullscreen !== window.is_fullscreen()) {
        if (state.fullscreen)
            window.make_fullscreen();
        else
            window.unmake_fullscreen();
    }
    if ('minimized' in state && state.minimized !== window.minimized) {
        if (state.minimized)
            window.minimize();
        else
            window.unminimize();
    }
}

export class Shim {
    constructor(interfaceXml) {
        this._dbusImpl = Gio.DBusExportedObject.wrapJSObject(interfaceXml, this);
        this._dbusImpl.export(Gio.DBus.session, OBJECT_PATH);

        this._pending = new Map();
        global.display.connectObject('window-created',
            (_display, window) => this._onWindowCreated(window), this);
    }

    destroy() {
        global.display.disconnectObject(this);
        for (const [window, cleanup] of this._pending) {
            cleanup();
            this._pending.delete(window);
        }
        this._dbusImpl.unexport();
        this._dbusImpl = null;
    }

    _onWindowCreated(window) {
        // WindowReady is emitted once the window is mapped. Unlike the
        // actor's first-frame, this also fires for windows opened on a
        // workspace that is not visible (those are never painted).
        const emitReady = () => {
            window.disconnectObject(this);
            this._pending.delete(window);
            if (!isTracked(window))
                return;
            window.connectObject('unmanaged', () => {
                window.disconnectObject(this);
                this._emit('WindowClosed', String(window.get_id()));
            }, this);
            this._emit('WindowReady', JSON.stringify(describeWindow(window)));
        };

        if (window.mapped) {
            emitReady();
            return;
        }
        this._pending.set(window, () => window.disconnectObject(this));
        window.connectObject(
            'notify::mapped', () => {
                if (window.mapped)
                    emitReady();
            },
            'unmanaged', () => {
                window.disconnectObject(this);
                this._pending.delete(window);
            }, this);
    }

    _emit(signal, value) {
        this._dbusImpl?.emit_signal(signal, new GLib.Variant('(s)', [value]));
    }

    GetVersion() {
        return [API_VERSION, Config.PACKAGE_VERSION];
    }

    ListWindows() {
        const windows = global.display.list_all_windows()
            .filter(isTracked)
            .sort((a, b) => a.get_stable_sequence() - b.get_stable_sequence())
            .map(describeWindow);
        return JSON.stringify(windows);
    }

    ListMonitors() {
        const display = global.display;
        const workspace = global.workspace_manager.get_active_workspace();
        const primary = display.get_primary_monitor();
        const monitors = [];
        for (let i = 0; i < display.get_n_monitors(); i++) {
            const geometry = rect(display.get_monitor_geometry(i));
            monitors.push({
                index: i,
                ...geometry,
                work_area: rect(workspace.get_work_area_for_monitor(i)),
                scale: display.get_monitor_scale(i),
                primary: i === primary,
            });
        }
        return JSON.stringify(monitors);
    }

    GetWorkspaces() {
        const mutter = new Gio.Settings({schema_id: 'org.gnome.mutter'});
        return JSON.stringify({
            count: global.workspace_manager.n_workspaces,
            active: global.workspace_manager.get_active_workspace_index(),
            dynamic: mutter.get_boolean('dynamic-workspaces'),
            only_on_primary: mutter.get_boolean('workspaces-only-on-primary'),
        });
    }

    EnsureWorkspaces(count) {
        ensureWorkspaces(null, count);
        return global.workspace_manager.n_workspaces;
    }

    // placement: {workspace?, on_all_workspaces?, monitor?, rect?: {x,y,w,h},
    //             maximized?, fullscreen?, minimized?, above?}
    PlaceWindow(id, placementJson) {
        const window = findWindow(id);
        const p = JSON.parse(placementJson);

        if (window.is_fullscreen())
            window.unmake_fullscreen();
        Compat.setMaximized(window, 'none');
        if (window.minimized)
            window.unminimize();

        if (p.on_all_workspaces) {
            window.stick();
        } else if (Number.isInteger(p.workspace) && p.workspace >= 0) {
            if (window.is_on_all_workspaces())
                window.unstick();
            ensureWorkspaces(window, p.workspace + 1);
            window.change_workspace_by_index(p.workspace, false);
        }

        if (Number.isInteger(p.monitor) && p.monitor >= 0 &&
            p.monitor < global.display.get_n_monitors() &&
            window.get_monitor() !== p.monitor)
            window.move_to_monitor(p.monitor);

        if (p.rect) {
            window.move_resize_frame(true, p.rect.x, p.rect.y, p.rect.w, p.rect.h);
            // If the client adjusts the size (size increments, fixed
            // dimensions), Mutter drops the position of a combined
            // move+resize. X11 clients resize synchronously, so a separate
            // move fixes that right away. Wayland clients resize
            // asynchronously and a move now would re-send the old size and
            // cancel the resize; the CLI places them again instead.
            if (Compat.clientType(window) === 'x11')
                window.move_frame(true, p.rect.x, p.rect.y);
        }

        applyState(window, p);
        return true;
    }

    SetWindowState(id, stateJson) {
        applyState(findWindow(id), JSON.parse(stateJson));
        return true;
    }

    LaunchApp(desktopId, workspace) {
        const app = Shell.AppSystem.get_default().lookup_app(desktopId);
        if (!app)
            return false;
        app.launch(0, workspace, Shell.AppLaunchGpu.APP_PREF);
        return true;
    }

    LaunchCommand(argv, workspace) {
        if (argv.length === 0)
            return false;
        const commandline = argv.map(a => GLib.shell_quote(a)).join(' ');
        const info = Gio.AppInfo.create_from_commandline(commandline, null,
            Gio.AppInfoCreateFlags.NONE);
        info.launch([], global.create_app_launch_context(0, workspace));
        return true;
    }
}
