#!/usr/bin/env bash
# Renders the Bosphorus view offscreen against a fake core module: screenshots in $1 (default
# /tmp/bc-test/probe). Needs a Qt 6 `qml` (QML_BIN) and the design system Basecamp 0.3.2 pins
# (logos-design-system e43a481, LOGOS_DESIGN_SYSTEM_QML). Extra args: setup | stale.
set -uo pipefail
cd "$(dirname "$0")"
OUT=${1:-/tmp/bc-test/probe}; shift || true
QML_BIN=${QML_BIN:-/tmp/bc-test/qtdecl/bin/qml}
DS=${LOGOS_DESIGN_SYSTEM_QML:-/tmp/bc-test/ds/src/qml}
QT_QML_DIR=${QT_QML_DIR:-$(dirname "$(dirname "$(readlink -f "$QML_BIN")")")/lib/qt-6/qml}
mkdir -p "$OUT"
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software QT_QUICK_CONTROLS_STYLE=Basic "$QML_BIN" -I "$QT_QML_DIR" -I "$DS" probe.qml -- "$@" "$OUT" 2>&1 \
  | grep -vE "Unsupported image format|Populating font|createPlatformOpenGLContext|propagateSizeHints" | sed 's/^qml: //'
