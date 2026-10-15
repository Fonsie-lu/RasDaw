#!/bin/sh
# Build RasDaw in release mode and install it for the current user:
# the binary into ~/.local/bin, plus a desktop entry and icons for the app launcher.
#
#   ./install.sh               build, then install binary, desktop entry and icons
#   ./install.sh --no-build    install the binary already in target/release
#   ./install.sh --uninstall   remove everything this script installed
#
# Set BIN_DIR to install somewhere else, e.g. BIN_DIR=/usr/local/bin ./install.sh
set -eu
cd "$(dirname "$0")"

BIN=rasdaw
BIN_DIR=${BIN_DIR:-$HOME/.local/bin}
DATA_DIR=${XDG_DATA_HOME:-$HOME/.local/share}
APPS_DIR=$DATA_DIR/applications
ICON_DIR=$DATA_DIR/icons/hicolor
ICON_SIZES="48 64 128 256"
build=1
uninstall=0

for arg in "$@"; do
    case $arg in
        --no-build) build=0 ;;
        --uninstall) uninstall=1 ;;
        -h|--help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $arg" >&2; exit 1 ;;
    esac
done

# Refreshes the launcher caches where the tools exist; harmless if they do not.
refresh_caches() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$APPS_DIR" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$ICON_DIR" || true
}

if [ "$uninstall" = 1 ]; then
    rm -f "$BIN_DIR/$BIN" "$APPS_DIR/$BIN.desktop" "$ICON_DIR/scalable/apps/$BIN.svg"
    for s in $ICON_SIZES; do
        rm -f "$ICON_DIR/${s}x${s}/apps/$BIN.png"
    done
    refresh_caches
    echo "removed $BIN_DIR/$BIN, its desktop entry and icons"
    exit 0
fi

if [ "$build" = 1 ]; then
    cargo build --release
fi

if [ ! -x "target/release/$BIN" ]; then
    echo "target/release/$BIN not found; run without --no-build" >&2
    exit 1
fi

mkdir -p "$BIN_DIR"
# Install via a temporary file and rename, so a running RasDaw is not overwritten in place.
install -m 755 "target/release/$BIN" "$BIN_DIR/.$BIN.new"
mv -f "$BIN_DIR/.$BIN.new" "$BIN_DIR/$BIN"
echo "installed $BIN_DIR/$BIN"

# Icons (hicolor theme) and desktop entry for the app launcher.
install -Dm644 assets/icons/rasdaw.svg "$ICON_DIR/scalable/apps/$BIN.svg"
for s in $ICON_SIZES; do
    install -Dm644 "assets/icons/rasdaw-$s.png" "$ICON_DIR/${s}x${s}/apps/$BIN.png"
done
mkdir -p "$APPS_DIR"
cat > "$APPS_DIR/$BIN.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=RasDaw
GenericName=Digital Audio Workstation
Comment=Bitwig-inspired DAW with CLAP plugin hosting, by Fonsie Audio
Exec="$BIN_DIR/$BIN" %f
Icon=$BIN
Terminal=false
Categories=AudioVideo;Audio;Midi;Sequencer;Music;
Keywords=daw;music;audio;midi;clap;sequencer;synth;
StartupWMClass=$BIN
StartupNotify=true
DESKTOP
refresh_caches
echo "installed $APPS_DIR/$BIN.desktop and icons"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "note: $BIN_DIR is not in PATH" ;;
esac
