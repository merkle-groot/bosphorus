"""Indented call tree from a demangled macOS `sample` file: FILE MIN_PERCENT MAX_DEPTH START_FRAME.

python3 sample_tree.py s.dm.txt 1.0 7 prove_segment_core   (main thread only)
"""
import re, sys
text = open(sys.argv[1]).read()
graph = text.split("Call graph:")[1].split("Total number in stack")[0]
lines = graph.splitlines()
main_total = int(re.match(r"\s*(\d+)", lines[1]).group(1))
thresh = float(sys.argv[2]); maxdepth = int(sys.argv[3]); start = sys.argv[4]
active = None
def short(n):
    n = re.sub(r"::h[0-9a-f]{16}", "", n)
    n = re.sub(r"<[^<>]*>", "", n); n = re.sub(r"<[^<>]*>", "", n); n = re.sub(r"<[^<>]*>", "", n)
    n = re.sub(r"\[[0-9a-f]+\]", "", n)
    return n
for line in lines[2:]:
    if re.match(r"^\s{4}\d+ Thread_", line): break
    m = re.match(r"^([\s+!:|]*)(\d+)\s+(.*?)\s+\(in ([^)]*)\)", line)
    if not m: continue
    depth = len(m.group(1)); count = int(m.group(2)); name = short(m.group(3))
    if active is None:
        if start in name: active = depth
        else: continue
    if depth < active: break
    if (depth - active) // 2 > maxdepth: continue
    if 100 * count / main_total >= thresh and "pthread" not in name and "psynch" not in name:
        print(f"{' ' * ((depth - active)//2)}{count:6d} {100*count/main_total:5.1f}%  {name[:120]}")
