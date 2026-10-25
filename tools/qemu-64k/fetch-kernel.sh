#!/bin/sh
# Download the newest Debian stable arm64 64 KiB-page kernel package and unpack its image to <dir>/Image.
set -eu
dir=$1
mkdir -p "$dir/pkg"
base=https://deb.debian.org/debian
curl -fsSL "$base/dists/stable/main/binary-arm64/Packages.xz" | xz -dc > "$dir/Packages"
# Package names look like linux-image-6.12.48-arm64-64k or linux-image-6.12.48-arm64-64k-unsigned.
name=$(sed -n 's/^Package: \(linux-image-[0-9][0-9.-]*-arm64-64k\(-unsigned\)\?\)$/\1/p' "$dir/Packages" | sort -V | tail -n 1)
test -n "$name" || { echo "no 64k kernel package in Debian stable:"; grep '^Package: linux-image' "$dir/Packages" | grep -i 64k; exit 1; }
file=$(awk -v n="$name" '$1=="Package:"{p=$2} $1=="Filename:" && p==n {print $2}' "$dir/Packages" | tail -n 1)
echo "kernel package: $name ($file)"
curl -fsSL -o "$dir/kernel.deb" "$base/$file"
dpkg-deb -x "$dir/kernel.deb" "$dir/pkg"
img=$(ls "$dir"/pkg/boot/vmlinuz-* | head -n 1)
if gzip -t "$img" 2>/dev/null; then gunzip -c "$img" > "$dir/Image"; else cp "$img" "$dir/Image"; fi
grep -a -c 'CONFIG_ARM64_64K_PAGES' "$dir"/pkg/boot/config-* || true
grep -h 'CONFIG_ARM64_64K_PAGES\|CONFIG_ARM64_PA_BITS=' "$dir"/pkg/boot/config-*
ls -l "$dir/Image"
