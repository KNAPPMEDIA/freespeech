#!/bin/bash
# Baut FreeSpeech auf einem Mac mit Apple-Prozessor.
# Aufruf im Terminal:  bash build-mac.sh
# Ergebnis: FreeSpeech.app und eine .dmg unter src-tauri/target/release/bundle/
set -e
cd "$(dirname "$0")"

fehlt=0
if ! xcode-select -p >/dev/null 2>&1; then
  echo "Es fehlen die Xcode-Werkzeuge. Installieren mit:  xcode-select --install"
  fehlt=1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "Es fehlt Rust. Installieren von:  https://rustup.rs"
  fehlt=1
fi
if ! command -v node >/dev/null 2>&1; then
  echo "Es fehlt Node.js. Installieren von:  https://nodejs.org"
  fehlt=1
fi
if ! command -v cmake >/dev/null 2>&1; then
  echo "Es fehlt cmake. Installieren mit:  brew install cmake"
  fehlt=1
fi
if [ "$fehlt" = "1" ]; then
  echo "Bitte das Fehlende installieren und das Skript nochmal starten."
  exit 1
fi

echo "1/2 Symbole erzeugen"
npx --yes @tauri-apps/cli@2 icon src-tauri/icons/icon-1024.png --output src-tauri/icons

echo "2/2 App bauen (beim ersten Mal 10 bis 20 Minuten)"
set -o pipefail
if npx --yes @tauri-apps/cli@2 build 2>&1 | tee build.log; then
  echo
  echo "Fertig. Die App liegt hier:"
  ls -d src-tauri/target/release/bundle/macos/*.app src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null
else
  echo
  echo "Der Bau ist fehlgeschlagen. Bitte die Datei build.log zurueckschicken."
  exit 1
fi
