#!/bin/sh
# Build <out> (a gzip cpio initramfs) holding busybox, a getconf stand-in and the test executables listed in <list>.
set -eu
out=$1
list=$2
root=$(mktemp -d)
mkdir -p "$root/bin" "$root/proc" "$root/sys" "$root/dev" "$root/tmp" "$root/tests"
cp /bin/busybox "$root/bin/busybox"
cat > getconf.c <<'C'
#include <stdio.h>
#include <unistd.h>
int main(void) { printf("getconf PAGESIZE = %ld\n", sysconf(_SC_PAGESIZE)); return 0; }
C
gcc -static -O2 -o "$root/bin/getconf-pagesize" getconf.c
n=0
while read -r exe; do
  n=$((n + 1))
  cp "$exe" "$root/tests/$(basename "$exe")"
done < "$list"
cat > "$root/init" <<'I'
#!/bin/busybox sh
/bin/busybox mkdir -p /proc /sys /dev /tmp
/bin/busybox mount -t devtmpfs dev /dev
exec >/dev/console 2>&1 </dev/console
echo "INIT-START"
/bin/busybox --install -s /bin
mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t tmpfs tmp /tmp
echo "kernel: $(uname -srm)"
/bin/getconf-pagesize
grep -m1 '^KernelPageSize' /proc/self/smaps
grep -m1 '^MMUPageSize' /proc/self/smaps
failed=0
for t in /tests/*; do
  echo "== $t"
  "$t" --test-threads=1 || { echo "FAILED: $t (status $?)"; failed=$((failed + 1)); }
done
echo "QEMU64K-RESULT failed=$failed"
poweroff -f
I
chmod +x "$root/init"
(cd "$root" && find . | cpio -o -H newc --quiet) | gzip -9 > "$out"
ls -l "$out"
