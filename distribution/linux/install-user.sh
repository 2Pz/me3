#!/bin/sh

# Install me3 for the current user

bindir=$HOME/.local/bin
datadir=${XDG_DATA_HOME:-$HOME/.local/share}
confdir=${XDG_CONFIG_HOME:-$HOME/.config}
me3bindir="${datadir}/me3/bin"

install -Dpm 0755 -t "${me3bindir}" bin/me3
install -Dpm 0644 -t "${datadir}/me3/windows-bin" bin/win64/me3-launcher.exe \
                                                  bin/win64/me3_mod_host.dll

install -Dpm 0644 -t "${datadir}/applications" dist/me3-launch.desktop
install -Dpm 0644 -t "${datadir}/mime/packages" dist/me3.xml
install -Dpm 0644 -t "${datadir}/icons/hicolor/128x128/apps" dist/me3.png

# Create per-shell environment snippets
ensure_env_dir="${confdir}/me3"
mkdir -p "${ensure_env_dir}"

# POSIX sh/bash/zsh
cat > "${ensure_env_dir}/me3-env.sh" <<EOF
export PATH="${me3bindir}:$PATH"
EOF

# fish
cat > "${ensure_env_dir}/me3-env.fish" <<EOF
set -gx PATH ${me3bindir} $PATH
EOF

# csh/tcsh
cat > "${ensure_env_dir}/me3-env.csh" <<EOF
set path = ( ${me3bindir} $path )
EOF

# Helper to append sourcing block to an rcfile if not present
add_source_block() {
    rcfile="$1"
    opener="$2"
    closer="$3"
    line="$4"
    [ -f "$rcfile" ] || touch "$rcfile"
    if ! grep -q "^# >>> me3 >>>$" "$rcfile" 2>/dev/null; then
        cp "$rcfile" "${rcfile}.me3.bak" 2>/dev/null || true
        {
            printf '%s\n' "$opener"
            printf '%s\n' "$line"
            printf '%s\n' "$closer"
        } >> "$rcfile"
    fi
}

# bash
add_source_block "$HOME/.bashrc"    "# >>> me3 >>>" "# <<< me3 <<<" "[ -f \"$ensure_env_dir/me3-env.sh\" ] && . \"$ensure_env_dir/me3-env.sh\""
add_source_block "$HOME/.bash_profile" "# >>> me3 >>>" "# <<< me3 <<<" "[ -f \"$ensure_env_dir/me3-env.sh\" ] && . \"$ensure_env_dir/me3-env.sh\""

# zsh
add_source_block "$HOME/.zshrc"     "# >>> me3 >>>" "# <<< me3 <<<" "[ -f \"$ensure_env_dir/me3-env.sh\" ] && . \"$ensure_env_dir/me3-env.sh\""

# fish
add_source_block "$HOME/.config/fish/config.fish" "# >>> me3 >>>" "# <<< me3 <<<" "test -f $ensure_env_dir/me3-env.fish; and source $ensure_env_dir/me3-env.fish"

# csh/tcsh
add_source_block "$HOME/.cshrc"     "# >>> me3 >>>" "# <<< me3 <<<" "if ( -f $ensure_env_dir/me3-env.csh ) source $ensure_env_dir/me3-env.csh"
add_source_block "$HOME/.tcshrc"    "# >>> me3 >>>" "# <<< me3 <<<" "if ( -f $ensure_env_dir/me3-env.csh ) source $ensure_env_dir/me3-env.csh"

# Patch .desktop Exec/TryExec to absolute path in me3 bin dir
appfile="${datadir}/applications/me3-launch.desktop"
if [ -f "$appfile" ]; then
    sed -i "s|^TryExec=.*|TryExec=${me3bindir}/me3|" "$appfile"
    sed -i "s|^Exec=.*|Exec=${me3bindir}/me3 launch -p %f|" "$appfile"
fi

# install example profiles
if [ ! -d "${confdir}/me3/profiles" ]; then
    install -Dpm 0644 -t "${confdir}/me3/profiles" ./*.me3
    mkdir "${confdir}/me3/profiles/eldenring-mods"
    mkdir "${confdir}/me3/profiles/nightreign-mods"
    mkdir "${confdir}/me3/profiles/sekiro-mods"
fi
