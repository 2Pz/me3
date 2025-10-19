#!/bin/sh

# Install me3 for the current user

bindir=$HOME/.local/bin
datadir=${XDG_DATA_HOME:-$HOME/.local/share}
confdir=${XDG_CONFIG_HOME:-$HOME/.config}

install -Dpm 0755 -t "${bindir}" bin/me3
install -Dpm 0644 -t "${datadir}/me3/windows-bin" bin/win64/me3-launcher.exe \
                                                  bin/win64/me3_mod_host.dll

install -Dpm 0644 -t "${datadir}/applications" dist/me3-launch.desktop
install -Dpm 0644 -t "${datadir}/mime/packages" dist/me3.xml
install -Dpm 0644 -t "${datadir}/icons/hicolor/128x128/apps" dist/me3.png

# Ensure PATH for user via systemd environment.d (no shell rc edits)
if [ -d "$HOME/.config" ]; then
    mkdir -p "$HOME/.config/environment.d"
    # Write PATH override to ensure ~/.local/bin is available for user sessions
    cat > "$HOME/.config/environment.d/10-me3.conf" <<EOF
PATH=$HOME/.local/bin:$PATH
EOF
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user import-environment PATH || true
    fi
fi

# Patch .desktop Exec/TryExec to absolute path so .me3 files work without PATH
appfile="${datadir}/applications/me3-launch.desktop"
if [ -f "$appfile" ]; then
    sed -i "s|^TryExec=.*|TryExec=${bindir}/me3|" "$appfile"
    sed -i "s|^Exec=.*|Exec=${bindir}/me3 launch -p %f|" "$appfile"
fi

# Optional fallback: ensure PATH for bash login shells (Steam Deck)
[ -f "$HOME/.bash_profile" ] || touch "$HOME/.bash_profile"
if ! grep -qs 'export PATH="$HOME/.local/bin:$PATH"' "$HOME/.bash_profile"; then
    printf '\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$HOME/.bash_profile"
fi

# install example profiles
if [ ! -d "${confdir}/me3/profiles" ]; then
    install -Dpm 0644 -t "${confdir}/me3/profiles" ./*.me3
    mkdir "${confdir}/me3/profiles/eldenring-mods"
    mkdir "${confdir}/me3/profiles/nightreign-mods"
    mkdir "${confdir}/me3/profiles/sekiro-mods"
fi
