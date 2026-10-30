import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {Shim} from './shim.js';

export default class SimpleSessionShimExtension extends Extension {
    enable() {
        const file = this.dir.get_child('dbus').get_child('de.dbensing.SimpleSession.Shim1.xml');
        const [, bytes] = file.load_contents(null);
        this._shim = new Shim(new TextDecoder().decode(bytes));
    }

    disable() {
        this._shim?.destroy();
        this._shim = null;
    }
}
