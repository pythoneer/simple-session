// Version-sensitive Mutter/Shell calls. When a GNOME release breaks the shim,
// this should be the only file that needs changes.

import Meta from 'gi://Meta';

const MAXIMIZE_NAMES = {
    [Meta.MaximizeFlags.HORIZONTAL]: 'horizontal',
    [Meta.MaximizeFlags.VERTICAL]: 'vertical',
    [Meta.MaximizeFlags.BOTH]: 'both',
};

const MAXIMIZE_FLAGS = {
    horizontal: Meta.MaximizeFlags.HORIZONTAL,
    vertical: Meta.MaximizeFlags.VERTICAL,
    both: Meta.MaximizeFlags.BOTH,
};

// GNOME 49+: get_maximize_flags(); older: get_maximized().
export function getMaximized(window) {
    const flags = window.get_maximize_flags
        ? window.get_maximize_flags()
        : window.get_maximized();
    return MAXIMIZE_NAMES[flags] ?? 'none';
}

// GNOME 49+: set_maximize_flags(flags) / set_unmaximize_flags(flags);
// older: maximize(flags) / unmaximize(flags).
export function setMaximized(window, name) {
    if (name === 'none' || !(name in MAXIMIZE_FLAGS)) {
        if (getMaximized(window) === 'none')
            return;
        if (window.set_unmaximize_flags)
            window.set_unmaximize_flags(Meta.MaximizeFlags.BOTH);
        else
            window.unmaximize(Meta.MaximizeFlags.BOTH);
        return;
    }

    if (window.set_maximize_flags)
        window.set_maximize_flags(MAXIMIZE_FLAGS[name]);
    else
        window.maximize(MAXIMIZE_FLAGS[name]);
}

export function clientType(window) {
    return window.get_client_type() === Meta.WindowClientType.X11 ? 'x11' : 'wayland';
}
