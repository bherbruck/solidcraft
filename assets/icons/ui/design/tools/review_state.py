"""Shared icon review state: read and write design/review.json (standard library only).

  {"version": 1, "final": {"<icon id>": "<sha256 of its master svg(s) when accepted>"}, "notes": {"<icon id>": "text"}}

Legacy acceptance lists migrate once when the review server starts; existing hashes are never refreshed implicitly.
"""
import os, re, json, glob, hashlib, tomllib, threading, uuid

LOCK = threading.RLock()

D = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))     # .../assets/icons/ui/design
REVIEW = D + '/review.json'

def aliases():
    return tomllib.load(open(D + '/aliases.toml', 'rb')).get('alias', {})
def master_of(icon_id, al=None):
    v = (aliases() if al is None else al).get(icon_id)
    return (v['master'] if isinstance(v, dict) else v) if v else icon_id
def lf(data):
    """Text as committed: a checkout with CRLF line endings (Git for Windows' default) hashes as LF."""
    return data.replace(b'\r\n', b'\n')
def master_hash(master, design=None):
    d = D if design is None else design
    h = hashlib.sha256()
    for f in sorted(glob.glob(d + '/svg/*/' + master + '.svg')):
        h.update(os.path.relpath(f, d).replace(os.sep, '/').encode()); h.update(lf(open(f, 'rb').read()))
    return h.hexdigest()
def load():
    try: r = json.load(open(REVIEW, encoding='utf-8'))
    except FileNotFoundError: r = {}
    fin = r.get('final', {})
    if isinstance(fin, list): fin = {i: '' for i in fin}
    return {'final': dict(fin), 'notes': {k: v for k, v in r.get('notes', {}).items() if isinstance(v, str) and v.strip()}}
def valid_final(state=None):
    """Acceptance belongs to a particular artwork revision, not just an id."""
    state = load() if state is None else state
    al = aliases()
    return {i: h for i, h in state['final'].items()
            if h and glob.glob(D + '/svg/*/' + master_of(i, al) + '.svg')
            and h == master_hash(master_of(i, al))}
def save(final, notes):
    """final: {id: hash or ''}; a missing hash is filled in from the master as it is now."""
    ok = lambda i: re.fullmatch(r'[a-z][a-z0-9_]*', i)
    al = aliases()
    fin = {i: (h or master_hash(master_of(i, al))) for i, h in final.items() if ok(i)}
    nts = {k: v for k, v in notes.items() if ok(k) and isinstance(v, str) and v.strip()}
    temporary = REVIEW + '.' + uuid.uuid4().hex + '.tmp'
    with LOCK:
        try:
            with open(temporary, 'w', encoding='utf-8') as f:
                json.dump({'version': 1, 'final': dict(sorted(fin.items())), 'notes': dict(sorted(nts.items()))}, f, indent=1); f.write('\n')
            os.replace(temporary, REVIEW)
        finally:
            if os.path.exists(temporary): os.unlink(temporary)
def resolve(masters_or_ids):
    """The icons were redrawn: drop their notes, and the notes on aliases of the same masters. Returns the ids cleared."""
    r = load(); al = aliases(); done = set(masters_or_ids)
    cleared = sorted(i for i in r['notes'] if i in done or master_of(i, al) in done)
    for i in cleared: del r['notes'][i]
    if cleared: save(r['final'], r['notes'])
    return cleared


def resolve_export(request, written):
    """Clear only still-matching exported notes on masters actually redrawn.

    Call after applying a snapshot's artwork feedback. Newer notes and final
    aliases are retained; this helper does not move requests or draw anything.
    """
    with LOCK:
        state = load()
        protected = {master_of(i) for i in valid_final(state)}
        cleared = []
        for icon, expected in request.get('notes', {}).items():
            master = master_of(icon)
            if master in written and master not in protected and state['notes'].get(icon) == expected:
                del state['notes'][icon]
                cleared.append(icon)
        if cleared: save(state['final'], state['notes'])
        return sorted(cleared)
