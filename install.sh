#!/bin/sh
# Build RasDaw in release mode and install the binary into ~/.local/bin.
#
#   ./install.sh               build, then copy target/release/rasdaw to ~/.local/bin/rasdaw
#   ./install.sh --no-build    install the binary already in target/release
#   ./install.sh --uninstall   remove the installed binary
#
# Set BIN_DIR to install somewhere else, e.g. BIN_DIR=/usr/local/bin ./install.sh
set -eu
cd "$(dirname "$0")"

BIN=rasdaw
BIN_DIR=${BIN_DIR:-$HOME/.local/bin}
build=1
uninstall=0

for arg in "$@"; do
    case $arg in
        --no-build) build=0 ;;
        --uninstall) uninstall=1 ;;
        -h|--help) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $arg" >&2; exit 1 ;;
    esac
done

if [ "$uninstall" = 1 ]; then
    rm -f "$BIN_DIR/$BIN"
    echo "removed $BIN_DIR/$BIN"
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

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "note: $BIN_DIR is not in PATH" ;;
esac
