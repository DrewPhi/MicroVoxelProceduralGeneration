#!/usr/bin/env bash
# Builds the Linux executable launcher and machine-local .desktop entry.
set -eu
cd -- "$(dirname -- "$0")/.."
cargo build --locked --release --manifest-path successor/Cargo.toml -p microvoxel-successor
launcher_tmp=$(mktemp ./MicroVoxel.XXXXXX.tmp)
trap 'rm -f "$launcher_tmp"' EXIT
MICROVOXEL_ENGINE=successor/target/release/microvoxel-successor rustc --edition=2021 -O -C strip=symbols tools/launcher.rs -o "$launcher_tmp"
chmod 755 "$launcher_tmp"
mv -f -- "$launcher_tmp" MicroVoxel
python3 - <<'PY'
from pathlib import Path
root=Path.cwd()
def quoted(path):
    return '"'+str(path).replace('\\','\\\\').replace('"','\\"').replace('`','\\`').replace('$','\\$')+'"'
entry=root/'MicroVoxel.desktop'
entry.write_text('[Desktop Entry]\nVersion=1.0\nType=Application\nName=MicroVoxel\nComment=Explore your procedural voxel world\n'
    +'Exec='+quoted(root/'MicroVoxel')+'\nPath='+str(root)+'\nIcon='+str(root/'assets/microvoxel.svg')
    +'\nTerminal=false\nCategories=Game;\nStartupNotify=true\n')
entry.chmod(0o755)
PY
printf 'Ready: %s/MicroVoxel\nDesktop entry: %s/MicroVoxel.desktop\n' "$PWD" "$PWD"
