"""Summarise R0_TL=1 timeline logs: each top-level proof's length, then each stage's total.

Usage: tl_summary.py LOG [LOG ...]
"""
import re, sys
for f in sys.argv[1:]:
    lines=[l for l in open(f) if l.startswith('[tl')]
    spans=[]
    for l in lines:
        m=re.match(r"\[tl\s+([\d.]+)\s+([\d.]+)\] (.*)",l.rstrip('\n'))
        spans.append((float(m[1]),float(m[2]),m[3]))
    spans.sort()
    tops=[s for s in spans if not s[2].startswith(' ') and 'prove' in s[2]]
    print(f, 'proofs:', ' '.join(f"{t[1]:.2f}" for t in tops))
    tot={}
    for s in spans:
        if not s[2].startswith('  ') or s[2].startswith('   '): continue
        k=re.sub(r"\(.*?\)|po2 \d+","",s[2]).strip()
        tot[k]=tot.get(k,0)+s[1]
    print('   ', ' | '.join(f"{k} {v:.2f}" for k,v in sorted(tot.items(), key=lambda kv:-kv[1])))
