"""Unpack an arm64 EFI zboot kernel (PE image with a compressed payload) to a raw Image: unzboot.py <in> <out>."""
import gzip
import struct
import subprocess
import sys

data = open(sys.argv[1], "rb").read()
if data[4:8] != b"zimg":
    sys.exit("not an EFI zboot image; header: %r" % data[:64])
offset, size = struct.unpack_from("<II", data, 8)
comp = data[24:32].split(b"\0")[0].decode()
payload = data[offset:offset + size]
print("zboot payload: %s, %d bytes at offset %d" % (comp, size, offset))
if comp == "gzip":
    raw = gzip.decompress(payload)
elif comp in ("zstd", "zstd22"):
    raw = subprocess.run(["zstd", "-dc"], input=payload, check=True, stdout=subprocess.PIPE).stdout
else:
    sys.exit("unsupported zboot compression %r" % comp)
open(sys.argv[2], "wb").write(raw)
