"""Print the test executables named in `cargo test --no-run --message-format=json` output, one per line."""
import json
import sys

seen = []
with open(sys.argv[1], encoding="utf-8") as f:
    for line in f:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        exe = msg.get("executable")
        if msg.get("reason") == "compiler-artifact" and exe and msg.get("profile", {}).get("test") and exe not in seen:
            seen.append(exe)
if not seen:
    sys.exit("no test executables found")
print("\n".join(seen))
