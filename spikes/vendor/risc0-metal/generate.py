"""Generate Metal constraint kernels from the unchanged verifier polynomials (`poly_ext.rs`).

The verifier's polynomial is a nested sum: `AndEqz` adds `mul * x` and multiplies `mul` by the
mix, `AndCond` adds `cond * inner.tot * mul`. Multiplication distributes, so the whole value is a
flat sum of terms `(c_1 * ... * c_k) * x * mix^e`, each with its own exponent `e`. Field
arithmetic is exact, so any regrouping of that sum gives the same value bit for bit.

Each kernel ("stage") takes a contiguous run of terms. Terms that share a condition prefix share
one accumulator, as in the original nesting, and the stages add into one extension-field partial
sum per point (`acc`); the last stage divides by the vanishing polynomial and writes `check`.

Small kernels matter more than anything else here: the GPU runs out of registers on big ones
(2,400 operations per kernel ran 6x slower per point than 400). A node a later stage needs is
recomputed there from the trace columns, unless recomputing it would take `STORE` or more
operations: then its first stage writes it to `scratch` and later stages read it.

The same terms also become a NEON C evaluator (`NAME_neon.c`, 4 points per vector) for the
CPU's share of the domain: bigger stages, and every value a later stage needs is stored.

Usage: generate.py OUT_DIR [BUDGET] [STORE]   (STORE 0 recomputes everything)
"""
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
OUT = Path(sys.argv[1])
BUDGET = int(sys.argv[2]) if len(sys.argv) > 2 else 400
STORE = int(sys.argv[3]) if len(sys.argv) > 3 else 8
NEON_BUDGET = int(os.environ.get("R0_NEON_BUDGET", "1500"))
MU = 0x88000001  # P^-1 mod 2^32
# Experiments only (R0_METAL_EXP): "nocols" reads no trace columns, "nopm" no mix powers.
EXP = os.environ.get("R0_METAL_EXP", "")
PM = (lambda e: f"uint4({e}u, 7u, 9u, 11u)") if EXP == "nopm" else (lambda e: f"poly_mix[{e}]")
P = 2013265921
R = (1 << 32) % P
assert (-11 * R) % P == 1073741848


def generate(name, subdir):
    src = ROOT.parent / f"risc0-circuit-{name}" / "src" / subdir
    poly = (src / "poly_ext.rs").read_text()
    taps = [tuple(map(int, m)) for m in re.findall(
        r"TapData\s*\{\s*offset:\s*(\d+),\s*back:\s*(\d+),\s*group:\s*(\d+)", (src / "taps.rs").read_text())]
    assert taps

    # Hash-consed nodes, created in topological order. kind: "leaf" (expr) or add/sub/mul.
    nodes, cache = [], {}

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
        i = node("leaf", (), len(vals), expr)
        nodes[i]["lit"] = vals
        return i

    zero, one = literal([0]), literal([1])

    def op(kind, a, b):
        if kind == "add" and a == zero: return b
        if kind in ("add", "sub") and b == zero: return a
        if kind == "mul":
            if a == zero or b == zero: return zero
            if a == one: return b
            if b == one: return a
        return node(kind, (a, b), max(nodes[a]["width"], nodes[b]["width"]))

    fp, mix = [], []  # fp: node ids; mix: ("true",) | ("eqz", chain, x) | ("cond", chain, c, inner)
    for m in re.finditer(r"PolyExtStep::(\w+)(?:\(([^)]*)\))?", poly):
        kind = m[1]
        a = list(map(int, m[2].split(','))) if m[2] else []
        if kind in ("Const", "ConstExt"):
            fp.append(literal(a))
        elif kind == "Get":
            col, back, group = taps[a[0]]
            expr = (f"(cycle * {col + 1}u + {back}u)" if EXP == "nocols"
                    else f"group{group}[{col}u * domain + ((cycle - {4 * back}u) & mask)]")
            fp.append(node("leaf", (), 1, expr))
            nodes[fp[-1]]["tap"] = (group, col, back)
        elif kind == "GetGlobal":
            fp.append(node("leaf", (), 1, f"global{a[0]}[{a[1]}]"))
            nodes[fp[-1]]["glob"] = (a[0], a[1])
        elif kind in ("Add", "Sub", "Mul"):
            fp.append(op(kind.lower(), fp[a[0]], fp[a[1]]))
        elif kind == "True":
            mix.append(("true",))
        elif kind == "AndEqz":
            mix.append(("eqz", a[0], fp[a[1]]))
        elif kind == "AndCond":
            mix.append(("cond", a[0], fp[a[1]], a[2]))
        else:
            raise ValueError(kind)
    ret = int(re.search(r"ret:\s*(\d+)", poly)[1])

    # Exponent of each chain's `mul` (mix^count), then the flat term list in chain order.
    count = []
    for m in mix:
        count.append(0 if m[0] == "true" else count[m[1]] + 1 if m[0] == "eqz" else count[m[1]] + count[m[3]])
    terms = []  # (conds tuple outer → inner, x node, exponent)

    def expand(var, base, conds):
        spine = []
        while mix[var][0] != "true":
            spine.append(var)
            var = mix[var][1]
        for v in reversed(spine):
            m = mix[v]
            if m[0] == "eqz":
                terms.append((conds, m[2], base + count[m[1]]))
            else:
                expand(m[3], base + count[m[1]], conds + (m[2],))

    sys.setrecursionlimit(100000)
    expand(ret, 0, ())
    terms = [t for t in terms if t[1] != zero and zero not in t[0]]
    exponents = [t[2] for t in terms]
    assert len(set(exponents)) == len(exponents), "each term has its own mix power"

    def is_op(i):
        return nodes[i]["kind"] != "leaf"

    # Last term (by index) whose computation reaches each node.
    last_use = [-1] * len(nodes)
    for k, (conds, x, _) in enumerate(terms):
        stack = [x, *conds]
        seen = set()
        while stack:
            i = stack.pop()
            if i in seen or not is_op(i): continue
            seen.add(i)
            last_use[i] = k
            stack.extend(nodes[i]["args"])

    def term_cost(t):
        return 4 + (16 if nodes[t[1]]["width"] == 4 else 4)

    def collect(roots, have, stored, new, loads):
        """Nodes to compute for `roots` given those already in this stage (`have`) or stored."""
        stack = list(roots)
        while stack:
            i = stack.pop()
            if not is_op(i) or i in have or i in new: continue
            if i in stored:
                loads.add(i)
                continue
            new.add(i)
            stack.extend(nodes[i]["args"])

    def partition(budget, store):
        """Greedy partition of the ordered terms into stages, with scratch slots for stored values."""

        def costly(i, stored):
            """True if recomputing node i (down to leaves or stored nodes) takes `store`+ operations."""
            if store == 0: return False
            seen, stack = set(), [i]
            while stack:
                j = stack.pop()
                if not is_op(j) or j in seen or j in stored: continue
                seen.add(j)
                if len(seen) >= store: return True
                stack.extend(nodes[j]["args"])
            return False

        # A stage grows until the nodes it computes, the stored values it reads and its term work
        # pass the budget.
        stages = []  # dict(terms, compute, loads, stores)
        stored = set()
        cur = dict(terms=[], compute=set(), loads=set())

        def close(next_term):
            stores = set()
            for i in sorted(cur["compute"]):
                if last_use[i] >= next_term and costly(i, stored | stores):
                    stores.add(i)
            stages.append(dict(cur, stores=stores))
            stored.update(stores)

        for k, t in enumerate(terms):
            new, loads = set(), set()
            collect([t[1], *t[0]], cur["compute"], stored, new, loads)
            cost = (len(cur["compute"]) + len(new) + len(cur["loads"] | loads)
                    + sum(term_cost(u) for u in cur["terms"]) + term_cost(t))
            if cur["terms"] and cost > budget:
                close(k)
                cur = dict(terms=[], compute=set(), loads=set())
                new, loads = set(), set()
                collect([t[1], *t[0]], cur["compute"], stored, new, loads)
            cur["terms"].append(t)
            cur["compute"] |= new
            cur["loads"] |= loads
        close(len(terms))

        # Drop stores nobody reads, then give each stored value scratch slots for its live range.
        read = set().union(*(s["loads"] for s in stages))
        for s in stages:
            s["stores"] &= read
        first = {i: k for k, s in enumerate(stages) for i in s["stores"]}
        last = {}
        for k, s in enumerate(stages):
            for i in s["loads"]:
                last[i] = k
        slots, free, size, events = {}, [], 0, sorted(first, key=lambda i: first[i])
        live = []  # (last stage, slot list)
        for i in events:
            still = []
            for end, ss in live:
                if end < first[i]: free.extend(ss)
                else: still.append((end, ss))
            live = still
            ss = []
            for _ in range(nodes[i]["width"]):
                if free: ss.append(free.pop())
                else: ss.append(size); size += 1
            slots[i] = ss
            live.append((last[i], ss))
        return stages, slots, size, first

    stages, slots, size, first = partition(BUDGET, STORE)

    def emit_stage(k, s, lines):
        emitted = set()

        def ref(i):
            n = nodes[i]
            if not is_op(i):
                if n["expr"].startswith(("group", "global", "(cycle")):
                    if i not in emitted:
                        emitted.add(i)
                        lines.append(f"    uint v{i} = {n['expr']};")
                    return f"v{i}"
                return n["expr"]
            if i in emitted:
                return f"v{i}"
            if i in s["loads"]:
                emitted.add(i)
                parts = [f"scratch[{slot}u * stride + lane]" for slot in slots[i]]
                if len(parts) == 1:
                    lines.append(f"    uint v{i} = {parts[0]};")
                else:
                    lines.append(f"    uint4 v{i} = uint4({', '.join(parts)});")
                return f"v{i}"
            a, b = n["args"]
            x, y = ref(a), ref(b)
            if n["width"] == 4 and n["kind"] != "mul":
                if nodes[a]["width"] == 1: x = f"ext({x})"
                if nodes[b]["width"] == 1: y = f"ext({y})"
            emitted.add(i)
            lines.append(f"    {'uint' if n['width'] == 1 else 'uint4'} v{i} = {n['kind']}({x}, {y});")
            if i in s["stores"]:
                for j, slot in enumerate(slots[i]):
                    suffix = '' if n["width"] == 1 else '.' + 'xyzw'[j]
                    lines.append(f"    scratch[{slot}u * stride + lane] = v{i}{suffix};")
            return f"v{i}"

        stack = []  # open condition groups: (cond node, accumulator name)

        def close_group():
            cond, acc_name = stack.pop()
            c = ref(cond)
            parent = stack[-1][1] if stack else "s0"
            lines.append(f"    {parent} = add({parent}, mul({acc_name}, {c}));")

        for conds, x, e in s["terms"]:
            common = 0
            while common < len(stack) and common < len(conds) and stack[common][0] == conds[common]:
                common += 1
            while len(stack) > common:
                close_group()
            for c in conds[common:]:
                acc_name = f"g{len(lines)}"
                lines.append(f"    uint4 {acc_name} = uint4(0u);")
                stack.append((c, acc_name))
            target = stack[-1][1] if stack else "s0"
            v = ref(x)
            lines.append(f"    {target} = add({target}, mul({v}, {PM(e)}));")
        while stack:
            close_group()
        # A store whose node no term here needs directly still has to happen.
        for i in sorted(s["stores"] - emitted):
            ref(i)
        return len([i for i in emitted if is_op(i) and i not in s["loads"]])

    text = ['// Generated from RISC Zero verifier polynomials (Apache-2.0). See generate.py.', '#include "field.h"']
    total_ops = 0
    for k, s in enumerate(stages):
        is_first, is_last = k == 0, k == len(stages) - 1
        lines = [f'''kernel void {name}_{k}(
    device uint4* acc [[buffer(0)]],
    const device uint* group0 [[buffer(1)]],
    const device uint* group1 [[buffer(2)]],
    const device uint* group2 [[buffer(3)]],
    const device uint* global0 [[buffer(4)]],
    const device uint* global1 [[buffer(5)]],
    const device uint4* poly_mix [[buffer(6)]],
    device uint* check [[buffer(7)]],
    constant uint4& cfg [[buffer(8)]],
    const device uint* divisors [[buffer(9)]],
    device uint* scratch [[buffer(10)]],
    uint lane [[thread_position_in_grid]]) {{
    uint domain = cfg.x, cycle = cfg.y + lane, stride = cfg.z, mask = domain - 1u;
    if (lane >= cfg.w) return;
    uint4 s0 = uint4(0u);''']
        total_ops += emit_stage(k, s, lines) + len(s["terms"])
        if is_first and is_last:
            lines.append("    uint4 ret = mul(s0, divisors[cycle & 3u]);")
        elif is_first:
            lines.append("    acc[lane] = s0;")
        elif not is_last:
            lines.append("    acc[lane] = add(acc[lane], s0);")
        else:
            lines.append("    uint4 ret = mul(add(acc[lane], s0), divisors[cycle & 3u]);")
        if is_last:
            for j, comp in enumerate('xyzw'):
                lines.append(f"    check[{j}u * domain + cycle] = ret.{comp};")
        lines.append('}')
        text.extend(lines)
    (OUT / f"{name}.metal").write_text('\n'.join(text) + '\n')

    # NEON evaluator for the CPU's share of the domain: 4 consecutive points per vector (V), an
    # extension element as 4 of them (E). The cycle is a multiple of 4, and so is every tap's
    # offset, so a tap's 4 values are contiguous even when they wrap around the domain.
    nstages, nslots, nsize, _ = partition(NEON_BUDGET, 1)
    width = lambda i: nodes[i]["width"]

    def c_stage(k, st, out):
        emitted = set()

        def ref(i):
            n = nodes[i]
            if not is_op(i):
                if "lit" in n:
                    vals = n["lit"]
                    return f"vdupq_n_u32({vals[0]}u)" if len(vals) == 1 else "ext4(" + ", ".join(f"{v}u" for v in vals) + ")"
                if i not in emitted:
                    emitted.add(i)
                    if "tap" in n:
                        g, col, back = n["tap"]
                        out.append(f"    V v{i} = vld1q_u32(g{g} + {col}u * domain + ((cycle - {4 * back}u) & mask));")
                    else:
                        a, b = n["glob"]
                        out.append(f"    V v{i} = vdupq_n_u32(gl{a}[{b}]);")
                return f"v{i}"
            if i in emitted:
                return f"v{i}"
            if i in st["loads"]:
                emitted.add(i)
                ss = nslots[i]
                if len(ss) == 1:
                    out.append(f"    V v{i} = sc[{ss[0]}];")
                else:
                    out.append(f"    E v{i} = {{{{sc[{ss[0]}], sc[{ss[1]}], sc[{ss[2]}], sc[{ss[3]}]}}}};")
                return f"v{i}"
            a, b = n["args"]
            x, y = ref(a), ref(b)
            kind = n["kind"]
            if n["width"] == 1:
                if kind == "mul" and "lit" in nodes[b]:
                    expr = f"mulk({x}, {y}, vdupq_n_u32({nodes[b]['lit'][0] * MU % 2**32}u))"
                elif kind == "mul" and "lit" in nodes[a]:
                    expr = f"mulk({y}, {x}, vdupq_n_u32({nodes[a]['lit'][0] * MU % 2**32}u))"
                else:
                    expr = f"{kind}1({x}, {y})"
                out.append(f"    V v{i} = {expr};")
            else:
                if kind == "mul":
                    if width(a) == 4 and width(b) == 4: expr = f"mul4({x}, {y})"
                    elif width(a) == 4: expr = f"mul41({x}, {y})"
                    else: expr = f"mul41({y}, {x})"
                else:
                    if width(a) == 1: x = f"ext1({x})"
                    if width(b) == 1: y = f"ext1({y})"
                    expr = f"{kind}4({x}, {y})"
                out.append(f"    E v{i} = {expr};")
            emitted.add(i)
            if i in st["stores"]:
                ss = nslots[i]
                if len(ss) == 1:
                    out.append(f"    sc[{ss[0]}] = v{i};")
                else:
                    for j, slot in enumerate(ss):
                        out.append(f"    sc[{slot}] = v{i}.c[{j}];")
            return f"v{i}"

        stack = []

        def close_group():
            cond, acc_name = stack.pop()
            c = ref(cond)
            parent = stack[-1][1] if stack else "s0"
            m = f"mul4({acc_name}, {c})" if width(cond) == 4 else f"mul41({acc_name}, {c})"
            out.append(f"    {parent} = add4({parent}, {m});")

        for conds, x, e in st["terms"]:
            common = 0
            while common < len(stack) and common < len(conds) and stack[common][0] == conds[common]:
                common += 1
            while len(stack) > common:
                close_group()
            for c in conds[common:]:
                acc_name = f"a{len(out)}"
                out.append(f"    E {acc_name} = zero4();")
                stack.append((c, acc_name))
            target = stack[-1][1] if stack else "s0"
            v = ref(x)
            if width(x) == 1:
                out.append(f"    {target} = addpm({target}, {v}, pmu + {8 * e});")
            else:
                out.append(f"    {target} = add4({target}, mul4({v}, loadpm(pm + {4 * e})));")
        while stack:
            close_group()
        for i in sorted(st["stores"] - emitted):
            ref(i)

    args = ("const uint32_t* g0, const uint32_t* g1, const uint32_t* g2, const uint32_t* gl0, "
            "const uint32_t* gl1, const uint32_t* pm, const uint32_t* pmu, uint32_t domain, "
            "uint32_t cycle, uint32_t mask, E* acc, V* sc")
    c = ['// Generated from RISC Zero verifier polynomials (Apache-2.0). See generate.py.', '#include "neon.h"']
    for k, st in enumerate(nstages):
        out = [f"__attribute__((noinline)) static void {name}_st{k}({args}) {{", "    E s0 = zero4();"]
        c_stage(k, st, out)
        out.append("    *acc = add4(*acc, s0);")
        out.append("}")
        c.extend(out)
    calls = "\n".join(f"        {name}_st{k}(g0, g1, g2, gl0, gl1, pm, pmu, domain, cycle, mask, &acc, sc);"
                       for k in range(len(nstages)))
    c.append(f"""void {name}_neon(const uint32_t* g0, const uint32_t* g1, const uint32_t* g2, const uint32_t* gl0,
                 const uint32_t* gl1, const uint32_t* pm, const uint32_t* pmu, const uint32_t* divisors,
                 uint32_t* check, uint32_t domain, uint32_t start, uint32_t end) {{
    uint32_t mask = domain - 1u;
    V sc[{max(nsize, 1)}];
    V div = vld1q_u32(divisors);
    for (uint32_t cycle = start; cycle < end; cycle += 4) {{
        E acc = zero4();
{calls}
        E ret = mul41(acc, div);
        for (int k = 0; k < 4; k++) vst1q_u32(check + k * domain + cycle, ret.c[k]);
    }}
}}""")
    (OUT / f"{name}_neon.c").write_text('\n'.join(c) + '\n')
    meta = dict(stages=len(stages), slots=size, terms=len(terms), max_power=max(exponents),
                neon_stages=len(nstages), neon_slots=nsize,
                node_ops=sum(1 for i in range(len(nodes)) if is_op(i)), emitted_ops=total_ops,
                stored=len(first), loads=sum(len(s["loads"]) for s in stages))
    print(name, meta)
    return meta


OUT.mkdir(parents=True, exist_ok=True)
metadata = {name: generate(name, sub) for name, sub in [('rv32im', 'zirgen'), ('recursion', ''), ('keccak', 'zirgen')]}
(OUT / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
(OUT / 'metadata.rs').write_text('\n'.join(
    f'pub const {name.upper()}: (usize, usize, usize) = ({v["stages"]}, {v["slots"]}, {v["max_power"]});'
    for name, v in metadata.items()) + '\n')
