#!/bin/bash
# Apolo9 · macOS installer. Jardim Sonoro, 2026.
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AU="$HOME/Library/Audio/Plug-Ins/Components"
VST3="$HOME/Library/Audio/Plug-Ins/VST3"
CLAP="$HOME/Library/Audio/Plug-Ins/CLAP"
PRESETS="$HOME/Library/Audio/Presets/Jardim Sonoro/Apolo9"
mkdir -p "$AU" "$VST3" "$CLAP" "$PRESETS"
echo "Apolo9: warming up eight oscillators…"
for b in Apolo9.component:"$AU" Apolo9.vst3:"$VST3" Apolo9.clap:"$CLAP"; do
  name="${b%%:*}"; dest="${b#*:}"
  if [ -d "$DIR/$name" ]; then
    rm -rf "$dest/$name"
    cp -R "$DIR/$name" "$dest/"
    xattr -cr "$dest/$name" 2>/dev/null || true
    echo "  -> $dest/$name"
  fi
done
if [ -d "$DIR/Presets" ]; then cp "$DIR"/Presets/*.preset "$PRESETS/" 2>/dev/null || true; fi
killall -9 AudioComponentRegistrar 2>/dev/null || true
echo "Done. Restart your DAW and touch gently."
