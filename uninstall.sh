#!/bin/bash
# Apolo9 · macOS uninstaller. The envelopes discharge; the capacitors remember nothing.
rm -rf "$HOME/Library/Audio/Plug-Ins/Components/Apolo9.component" \
       "$HOME/Library/Audio/Plug-Ins/VST3/Apolo9.vst3" \
       "$HOME/Library/Audio/Plug-Ins/CLAP/Apolo9.clap" \
       "$HOME/Library/Audio/Presets/Jardim Sonoro/Apolo9"
killall -9 AudioComponentRegistrar 2>/dev/null || true
echo "Apolo9 has gone quiet."
