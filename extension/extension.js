import GLib from 'gi://GLib';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {Shim} from './shim.js';

export default class SimpleSessionShimExtension extends Extension {
    enable() {
        const file = this.dir.get_child('dbus').get_child('de.dbensing.SimpleSession.Shim1.xml');
        const [, bytes] = file.load_contents(null);
        this._shim = new Shim(new TextDecoder().decode(bytes));

        // Test hook for scripts/nested.sh, which drives the menu through
        // org.gnome.Shell.Eval. Never set in a real session.
        if (GLib.getenv('SIMPLE_SESSION_NESTED_UNSAFE') === '1')
            global.context.unsafe_mode = true;

        this._loadIndicator();
    }

    disable() {
        this._indicator?.destroy();
        this._indicator = null;
        this._shim?.destroy();
        this._shim = null;
    }

    // The top-bar menu uses more shell UI API than the shim and is more likely
    // to break on a GNOME update. Loading it dynamically keeps even an import
    // failure from taking the shim down.
    async _loadIndicator() {
        try {
            const {Indicator} = await import('./indicator.js');
            if (!this._shim)
                return; // disabled while loading
            this._indicator = new Indicator();
            Main.panel.addToStatusArea(this.uuid, this._indicator);
        } catch (e) {
            console.error(`simple-session: top-bar menu disabled: ${e}`);
            this._indicator?.destroy();
            this._indicator = null;
        }
    }
}
