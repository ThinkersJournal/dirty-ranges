#!/bin/sh
# Download the newest Ubuntu noble-updates arm64 64 KiB-page kernel (Debian ships none) and unpack its image to <dir>/Image.
set -eu
dir=$1
mkdir -p "$dir/pkg"
base=https://ports.ubuntu.com/ubuntu-ports
curl -fsSL "$base/dists/noble-updates/main/binary-arm64/Packages.xz" | xz -dc > "$dir/Packages"
# Package names look like linux-image-unsigned-7.0.0-38-generic-64k.
name=$(sed -n 's/^Package: \(linux-image-unsigned-[0-9][0-9.-]*-generic-64k\)$/\1/p' "$dir/Packages" | sort -V | tail -n 1)
test -n "$name" || { echo "no 64k kernel package in Ubuntu noble-updates:"; grep '^Package: linux-image' "$dir/Packages" | grep -i 64k; exit 1; }
file=$(awk -v n="$name" '$1=="Package:"{p=$2} $1=="Filename:" && p==n {print $2}' "$dir/Packages" | tail -n 1)
echo "kernel package: $name ($file)"
curl -fsSL -o "$dir/kernel.deb" "$base/$file"
dpkg-deb -x "$dir/kernel.deb" "$dir/pkg"
img=$(ls "$dir"/pkg/boot/vmlinuz-* | head -n 1)
# Ubuntu arm64 kernels are EFI zboot images; QEMU needs the raw Image inside.
if [ "$(dd if="$img" bs=1 skip=4 count=4 2>/dev/null)" = zimg ]; then python3 "$(dirname "$0")/unzboot.py" "$img" "$dir/Image"; else cp "$img" "$dir/Image"; fi
file "$dir/Image" || true
ls -l "$dir/Image"
