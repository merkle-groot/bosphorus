"""Share of a macOS `sample` profile per operation, on the thread running execute_and_prove.

sample PID 70 1 -mayDie -file s.txt; rustfilt -i s.txt -o s.dm.txt
python3 sample_ops.py s.dm.txt "risc0_metal::evaluate,batch_expand_into_evaluate_ntt,hash_rows,step_accum"

A frame counts once per stack (nested matches are not double counted). Avoid names that also
match outer frames (e.g. "execute", "alloc").
"""
import re, sys, collections
text = open(sys.argv[1]).read()
graph = text.split("Call graph:")[1].split("Total number in stack")[0]
names = sys.argv[2].split(",")
blocks = re.split(r"\n(?=    \d+ Thread_)", graph)
best = None
for b in blocks:
    m = re.match(r"\s*(\d+) Thread_", b)
    if not m: continue
    if "execute_and_prove" in b and (best is None or int(m.group(1)) > best[0]): best = (int(m.group(1)), b)
total, block = best
agg = collections.Counter(); stack = []
for line in block.splitlines()[1:]:
    m = re.match(r"^([\s+!:|]*)(\d+)\s+(.*?)\s+\(in ([^)]*)\)", line)
    if not m: continue
    depth = len(m.group(1)); count = int(m.group(2)); name = m.group(3)
    while stack and stack[-1][0] >= depth: stack.pop()
    hit = next((n for n in names if n in name), None)
    inside = any(h for _, h in stack)
    if hit and not inside: agg[hit] += count
    stack.append((depth, bool(hit) or inside))
for n in names:
    if agg[n]: print(f"{agg[n]:6d} {100*agg[n]/total:5.1f}%  {n}")
print("proving thread samples", total)
