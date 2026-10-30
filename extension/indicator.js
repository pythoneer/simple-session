// Top-bar menu: save, restore, apply, overwrite and delete sessions.
// Every action runs the simple-session CLI and shows its last output line as
// a notification. No session logic lives here, and the shim does not depend
// on this module.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

Gio._promisify(Gio.Subprocess.prototype, 'communicate_utf8_async');

const TITLE = 'Simple Session';

// The CLI may not be in the shell's PATH: `cargo install` puts it in
// ~/.cargo/bin. SIMPLE_SESSION_CLI overrides the lookup (nested test shell).
function findCli() {
    const override = GLib.getenv('SIMPLE_SESSION_CLI');
    if (override)
        return override;
    const home = GLib.get_home_dir();
    return GLib.find_program_in_path('simple-session') ??
        [`${home}/.cargo/bin/simple-session`, `${home}/.local/bin/simple-session`]
            .find(path => GLib.file_test(path, GLib.FileTest.IS_EXECUTABLE)) ??
        null;
}

function lastLine(text) {
    return text.trim().split('\n').pop().trim();
}

function infoItem(text) {
    const item = new PopupMenu.PopupMenuItem(text, {reactive: false});
    item.label.add_style_class_name('popup-inactive-menu-item');
    return item;
}

class SaveItem extends PopupMenu.PopupBaseMenuItem {
    static {
        GObject.registerClass(this);
    }

    constructor(onSave) {
        super({reactive: false, can_focus: false});
        this.entry = new St.Entry({
            hint_text: 'Save current windows as…',
            can_focus: true,
            x_expand: true,
        });
        this.entry.clutter_text.connect('activate', () => {
            const name = this.entry.get_text().trim();
            if (!name)
                return;
            this.entry.set_text('');
            onSave(name);
        });
        this.add_child(this.entry);
    }
}

export class Indicator extends PanelMenu.Button {
    static {
        GObject.registerClass(this);
    }

    constructor() {
        super(0.5, TITLE);

        this._icon = new St.Icon({
            icon_name: 'view-grid-symbolic',
            style_class: 'system-status-icon',
        });
        this.add_child(this._icon);

        this._cancellable = new Gio.Cancellable();
        this._busy = false;
        this._sessions = [];

        this._saveItem = new SaveItem(name => this._save(name));
        this.menu.addMenuItem(this._saveItem);
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._section = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._section);

        this.menu.connectObject('open-state-changed', (_menu, open) => {
            if (open)
                this._refresh().catch(e => console.error(`simple-session: ${e}`));
        }, this);
        this.connect('destroy', () => this._cancellable.cancel());
    }

    async _run(args) {
        const cli = findCli();
        if (!cli)
            throw new Error('simple-session CLI not found; install it with "make install-cli"');
        const proc = Gio.Subprocess.new([cli, ...args],
            Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE);
        const [stdout, stderr] = await proc.communicate_utf8_async(null, this._cancellable);
        return {ok: proc.get_successful(), stdout: stdout ?? '', stderr: stderr ?? ''};
    }

    async _refresh() {
        let sessions = [];
        let problem = null;
        try {
            const result = await this._run(['list', '--json']);
            if (result.ok)
                sessions = JSON.parse(result.stdout);
            else
                problem = lastLine(result.stderr) || 'Listing sessions failed';
        } catch (e) {
            if (e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                return;
            problem = e.message;
        }

        this._sessions = sessions;
        this._section.removeAll();
        if (problem) {
            this._section.addMenuItem(infoItem(problem));
            return;
        }
        if (sessions.length === 0) {
            this._section.addMenuItem(infoItem('No saved sessions'));
            return;
        }
        for (const session of sessions) {
            const name = session.name;
            const label = session.error
                ? `${name} (unreadable)`
                : `${name} · ${session.windows} windows`;
            const item = new PopupMenu.PopupSubMenuMenuItem(label);
            if (!session.error) {
                item.menu.addAction('Restore',
                    () => this._action('Restore', name, ['restore', '--', name]));
                item.menu.addAction('Apply to open windows',
                    () => this._action('Apply', name, ['apply', '--', name]));
                item.menu.addAction('Overwrite with current windows',
                    () => this._action('Save', name, ['save', '--force', '--', name]));
            }
            item.menu.addAction('Delete',
                () => this._action('Delete', name, ['delete', '--', name]));
            this._section.addMenuItem(item);
        }
    }

    _save(name) {
        this.menu.close();
        if (this._sessions.some(s => s.name === name)) {
            Main.notify(`Session "${name}" already exists`,
                'Use "Overwrite with current windows" in its submenu.');
            return;
        }
        this._action('Save', name, ['save', '--', name]);
    }

    async _action(verb, name, args) {
        if (this._busy) {
            Main.notify(TITLE, 'Another action is still running.');
            return;
        }
        this._busy = true;
        this._icon.opacity = 128;
        let title = `${verb} "${name}"`;
        let summary;
        let ok = false;
        try {
            const result = await this._run(args);
            ok = result.ok;
            const error = lastLine(result.stderr).replace(/^Error: /, '');
            summary = ok ? lastLine(result.stdout) : error || lastLine(result.stdout);
        } catch (e) {
            if (e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                return;
            summary = e.message;
        } finally {
            this._busy = false;
            this._icon.opacity = 255;
        }
        if (!ok)
            title += ' failed';
        console.log(`simple-session: ${title}: ${summary}`);
        if (ok)
            Main.notify(title, summary);
        else
            Main.notifyError(title, summary);
    }
}
