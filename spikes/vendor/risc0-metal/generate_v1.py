"""Generate bounded-register Metal kernels from the unchanged verifier polynomial.

Each stage evaluates a slice of the SSA graph. Values that cross stages use a
reused, column-major scratch buffer; independent cycle tiles bound its memory.
No constraints are removed. Inputs, constants and mix powers are reloaded at use.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
OUT = Path(sys.argv[1])
CHUNK = int(sys.argv[2]) if len(sys.argv) > 2 else 400
P = 2013265921
R = (1 << 32) % P
assert (-11 * R) % P == 1073741848


def generate(name, subdir):
    src = ROOT.parent / f"risc0-circuit-{name}" / "src" / subdir
    poly = (src / "poly_ext.rs").read_text()
    taptext = (src / "taps.rs").read_text()
    taps = [tuple(map(int, m)) for m in re.findall(
        r"TapData\s*\{\s*offset:\s*(\d+),\s*back:\s*(\d+),\s*group:\s*(\d+)", taptext)]
    assert taps
    nodes = []
    cache = {}

    def node(kind, args, width, expr=None):
        key = (kind, tuple(args), width, expr)
        if key not in cache:
            cache[key] = len(nodes)
            nodes.append(dict(kind=kind, args=tuple(args), width=width, expr=expr))
        return cache[key]

    def literal(values):
        if len(values) == 4 and values[1:] == [0, 0, 0]:
            values = values[:1]
        vals = [v * R % P for v in values]
        expr = f"{vals[0]}u" if len(vals) == 1 else "uint4(" + ",".join(f"{v}u" for v in vals) + ")"
        return node("leaf", (), len(vals), expr)

    zero, one = literal([0]), literal([1])

    def op(kind, a, b):
        if kind == "add" and a == zero: return b
        if kind in ("add", "sub") and b == zero: return a
        if kind == "mul":
            if a == zero or b == zero: return zero
            if a == one: return b
            if b == one: return a
        return node(kind, (a, b), max(nodes[a]["width"], nodes[b]["width"]))

    def power(exponent):
        return one if exponent == 0 else node("leaf", (), 4, f"poly_mix[{exponent}]")

    fp, mix = [], []
    for m in re.finditer(r"PolyExtStep::(\w+)(?:\(([^)]*)\))?", poly):
        kind = m[1]
        a = list(map(int, m[2].split(','))) if m[2] else []
        if kind in ("Const", "ConstExt"):
            fp.append(literal(a))
        elif kind == "Get":
            col, back, group = taps[a[0]]
            fp.append(node("leaf", (), 1, f"group{group}[{col}u * domain + ((cycle - {4 * back}u) & mask)]"))
        elif kind == "GetGlobal":
            fp.append(node("leaf", (), 1, f"global{a[0]}[{a[1]}]"))
        elif kind in ("Add", "Sub", "Mul"):
            fp.append(op(kind.lower(), fp[a[0]], fp[a[1]]))
        elif kind == "True":
            mix.append((zero, 0))
        elif kind == "AndEqz":
            total, exponent = mix[a[0]]
            mix.append((op("add", total, op("mul", fp[a[1]], power(exponent))), exponent + 1))
        elif kind == "AndCond":
            total, exponent = mix[a[0]]
            inner, count = mix[a[2]]
            mix.append((op("add", total, op("mul", op("mul", fp[a[1]], inner), power(exponent))), exponent + count))
        else:
            raise ValueError(kind)
    result = mix[int(re.search(r"ret:\s*(\d+)", poly)[1])][0]

    # Keep only nodes reachable from the final constraint value, in original topological order.
    reachable = set()
    todo = [result]
    while todo:
        i = todo.pop()
        if i in reachable: continue
        reachable.add(i)
        todo.extend(nodes[i]["args"])
    operations = [i for i in sorted(reachable) if nodes[i]["kind"] != "leaf"]
    stages = [operations[i:i+CHUNK] for i in range(0, len(operations), CHUNK)]
    owner = {i: stage for stage, ids in enumerate(stages) for i in ids}
    last = dict(owner)
    for i in operations:
        for dep in nodes[i]["args"]:
            if dep in owner: last[dep] = max(last[dep], owner[i])
    # Reuse scalar slots once their last consuming stage has completed. Allocation never
    # aliases a value with one consumed in its defining stage.
    slots, active, free, size = {}, [], [], 0
    for stage, ids in enumerate(stages):
        remaining = []
        for end, indices in active:
            if end < stage: free.extend(indices)
            else: remaining.append((end, indices))
        active = remaining
        for i in ids:
            if last[i] <= stage: continue
            indices = []
            for _ in range(nodes[i]["width"]):
                if free: indices.append(free.pop())
                else: indices.append(size); size += 1
            slots[i] = indices
            active.append((last[i], indices))

    text = ['// Generated from RISC Zero verifier polynomials (Apache-2.0).', '#include "field.h"']
    for stage, ids in enumerate(stages):
        lines = [f'''kernel void {name}_{stage}(
    device uint* check [[buffer(0)]],
    const device uint* group0 [[buffer(1)]],
    const device uint* group1 [[buffer(2)]],
    const device uint* group2 [[buffer(3)]],
    const device uint* global0 [[buffer(4)]],
    const device uint* global1 [[buffer(5)]],
    const device uint4* poly_mix [[buffer(6)]],
    device uint* scratch [[buffer(7)]],
    constant uint4& cfg [[buffer(8)]],
    const device uint* divisors [[buffer(9)]],
    uint lane [[thread_position_in_grid]]) {{
    uint domain = cfg.x, cycle = cfg.y + lane, stride = cfg.z, mask = domain - 1u;
    if (lane >= cfg.w) return;''']
        needed = sorted({d for i in ids for d in nodes[i]["args"] if d in owner and owner[d] < stage})
        for i in needed:
            indices = slots[i]
            expr = [f"scratch[{slot}u * stride + lane]" for slot in indices]
            rhs = expr[0] if len(expr) == 1 else "uint4(" + ",".join(expr) + ")"
            lines.append(f"    {'uint' if len(expr)==1 else 'uint4'} v{i} = {rhs};")
        def ref(i):
            return nodes[i]["expr"] if nodes[i]["kind"] == "leaf" else f"v{i}"
        for i in ids:
            n = nodes[i]
            a, b = n["args"]
            x, y = ref(a), ref(b)
            if n["width"] == 4 and n["kind"] != "mul":
                if nodes[a]["width"] == 1: x = f"ext({x})"
                if nodes[b]["width"] == 1: y = f"ext({y})"
            lines.append(f"    {'uint' if n['width']==1 else 'uint4'} v{i} = {n['kind']}({x}, {y});")
        for i in ids:
            if i not in slots: continue
            for j, slot in enumerate(slots[i]):
                suffix = '' if nodes[i]["width"] == 1 else '.' + 'xyzw'[j]
                lines.append(f"    scratch[{slot}u * stride + lane] = v{i}{suffix};")
        if stage == len(stages)-1:
            val = ref(result)
            if nodes[result]["width"] == 1: val = f"ext({val})"
            lines.append(f"    uint4 ret = mul({val}, divisors[cycle & 3u]);")
            for j, comp in enumerate('xyzw'):
                lines.append(f"    check[{j}u * domain + cycle] = ret.{comp};")
        lines.append('}')
        text.extend(lines)
    (OUT / f"{name}.metal").write_text('\n'.join(text)+'\n')
    max_power = max([int(re.search(r"poly_mix\[(\d+)\]", n["expr"])[1]) for n in nodes if n["expr"] and n["expr"].startswith("poly_mix[")] + [0])
    meta = dict(stages=len(stages), slots=size, operations=len(operations), max_power=max_power)
    print(name, meta)
    return meta

OUT.mkdir(parents=True, exist_ok=True)
metadata = {name: generate(name, sub) for name, sub in [('rv32im','zirgen'),('recursion',''),('keccak','zirgen')]}
(OUT/'metadata.json').write_text(json.dumps(metadata, indent=2)+'\n')
(OUT/'metadata.rs').write_text('\n'.join(
    f'pub const {name.upper()}: (usize, usize, usize) = ({v["stages"]}, {v["slots"]}, {v["max_power"]});' for name,v in metadata.items()))
