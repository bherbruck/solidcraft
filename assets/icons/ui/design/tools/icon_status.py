#!/usr/bin/env python3
"""Repository-owned icon review tool (Python 3.11+, standard library only).

--serve [PORT]  Loopback review app, default http://127.0.0.1:8770/
--summary       Discovery counts, no generated source files
--csv           Write target/icon-inventory.csv
--notes         Open artwork feedback
--resolve ID... Clear feedback after an intentional redraw; protect newer notes manually
--out FILE      Self-contained static review page (default target/icon-status.html)

Read assets/icons/ui/STYLE_GUIDE.md before editing artwork. SVGs are authoritative.
Every discovered non-final icon is Open; no inventory file controls discovery.
"""
import sys, os, re, glob, html, json, hashlib, collections, tomllib, uuid, datetime

HERE = os.path.dirname(os.path.abspath(__file__))
D = os.path.dirname(HERE)                       # .../assets/icons/ui/design
ROOT = os.path.abspath(os.path.join(D, '..', '..', '..', '..'))
REVIEW = D + '/review.json'
REQUESTS = os.path.join(ROOT, 'target', 'codex-icon-review', 'inbox')
args = sys.argv[1:]
SERVER_REV = uuid.uuid4().hex if '--serve' in args else ''
out_path = args[args.index('--out') + 1] if '--out' in args else os.path.join(ROOT, 'target', 'icon-status.html')

sys.path.insert(0, HERE)
import review_state
import icon_palette
load_review = review_state.load
master_hash = review_state.master_hash

def art_rev():
    h = hashlib.sha256()
    h.update(SERVER_REV.encode())  # A backend restart must refresh already-open review tabs too.
    paths = glob.glob(D + '/svg/*/*.svg') + glob.glob(HERE + '/review/*')
    paths += glob.glob(ROOT + '/crates/engine/src/**/*.rs', recursive=True)
    paths += glob.glob(ROOT + '/crates/ui-egui/src/**/*.rs', recursive=True)
    paths += glob.glob(HERE + '/*.py')
    paths += [D + '/palette.toml', D + '/aliases.toml', __file__]
    for path in sorted(paths):
        st = os.stat(path)
        h.update(('%s %d %d;' % (os.path.relpath(path, ROOT), st.st_mtime_ns, st.st_size)).encode())
    return h.hexdigest()[:16]


def ui_icon_names(source):
    """Literal icon arguments, including the pattern and line-type UI wrappers."""
    names = set()
    for call in re.finditer(r'icons::paint[a-z_]*\((.*?);', source, re.S):
        names.update(re.findall(r'"([a-z][a-z0-9_]*)"', call.group(1)))
    for call in re.finditer(r'\bpattern_toggle\([^;]*?\);', source, re.S):
        # The icon is the fourth argument; the first strings are accessible labels.
        icon = re.match(r'pattern_toggle\(\s*\w+\s*,\s*(?:"[^"]*"|\w+)\s*,\s*"[^"]*"\s*,\s*"([a-z][a-z0-9_]*)"', call.group())
        if icon: names.add(icon.group(1))
    for table in re.finditer(r'for\s*\(i,\s*\(label,\s*icon,\s*tip\)\)\s*in\s*\[(.*?)\]\s*\.iter\(', source, re.S):
        names.update(re.findall(r'\("[^"]*",\s*"([a-z][a-z0-9_]*)",\s*"[^"]*"\)', table.group(1)))
    return names

def scan():
    """Everything derived from the repo: rows, summary."""
    pal = tomllib.load(open(D + '/palette.toml', 'rb'))
    alias_raw = tomllib.load(open(D + '/aliases.toml', 'rb')).get('alias', {})
    global unassigned
    unassigned = []
    masters = {}
    for f in sorted(glob.glob(D + '/svg/*/*.svg')):
        fam = os.path.basename(os.path.dirname(f)); n = os.path.basename(f)[:-4]
        if fam != 'small': masters[n] = (fam, f)
    uses = collections.defaultdict(list)   # id -> command ids and "ui:<file>"
    where = collections.defaultdict(list)  # id -> [(tab, panel, command)]
    for f in glob.glob(ROOT + '/crates/engine/src/**/*.rs', recursive=True):
        t = open(f, encoding='utf-8').read()
        ms = list(re.finditer(r'(?:CommandSpec::new|planned!)\(\s*"([^"]+)"\s*,\s*"([^"]*)"', t))
        for i, m in enumerate(ms):
            end = ms[i + 1].start() if i + 1 < len(ms) else len(t)
            chunk = t[m.end():end]
            ic = re.search(r'\.icon\(\s*"([^"]+)"', chunk)
            if ic:
                uses[ic.group(1)].append(m.group(1))
                at = re.search(r'\.at\(\s*"([^"]+)"\s*,\s*"([^"]+)"', chunk)
                if at: where[ic.group(1)].append((at.group(1), at.group(2), m.group(1)))
            else: unassigned.append(m.group(1))
    for f in glob.glob(ROOT + '/crates/ui-egui/src/**/*.rs', recursive=True):
        t = open(f, encoding='utf-8').read()
        for name in ui_icon_names(t):
            uses[name].append('ui:' + os.path.relpath(f, ROOT + '/crates/ui-egui/src'))
    mk = ROOT + '/assets/icons/ui/source/markers.rs'
    if os.path.exists(mk):
        for name in re.findall(r'=> "(c_[a-z_]+)"', open(mk, encoding='utf-8').read()): uses[name].append('ui:sketch constraint glyph')
    def alias_of(n):
        return icon_palette.resolve(pal, alias_raw, n)
    targets = {alias_of(a)[0] for a in alias_raw}
    rows = []
    for n in sorted(set(uses) | set(masters) | set(alias_raw)):
        m, acc = alias_of(n)
        if m in masters and n != 'placeholder': status = 'done'
        else: status, m = 'placeholder', 'placeholder'
        cmds = [u for u in uses.get(n, []) if not u.startswith('ui:')]
        rows.append(dict(id=n, status=status, master=m, accent=acc, commands=len(cmds), example=cmds[0] if cmds else '',
                         ui=sorted({u[3:] for u in uses.get(n, []) if u.startswith('ui:')}), unused=not uses.get(n),
                         where=where.get(n, []), alias_target=n in targets))
    cnt = collections.Counter(r['status'] for r in rows if r['id'] != 'placeholder')
    cmd_cnt = collections.Counter()
    for r in rows:
        if r['id'] != 'placeholder': cmd_cnt[r['status']] += r['commands']
    summary = '%d discovered icon ids · %d with artwork · %d missing · %d assigned commands.' % (sum(cnt.values()), cnt['done'], cnt['placeholder'], sum(cmd_cnt.values()))
    if unassigned: summary += ' %d commands have no icon assignment.' % len(unassigned)
    return rows, masters, pal, summary

if '--resolve' in args:
    ids = [a for a in args[args.index('--resolve') + 1:] if not a.startswith('--')]
    cleared = review_state.resolve(ids)
    print('cleared notes: ' + (' '.join(cleared) or 'none'))
    sys.exit(0)

rows, masters, pal, summary = scan()
print(summary)
if '--check' in args: sys.exit(1 if unassigned or any(r['status'] == 'placeholder' and r['id'] != 'placeholder' for r in rows) else 0)
if '--summary' in args and '--csv' not in args: sys.exit(0)

if '--notes' in args:
    rv = load_review(); n = 0
    valid = review_state.valid_final(rv)
    for r in rows:
        note = rv['notes'].get(r['id'])
        if note and r['id'] not in valid:
            n += 1; print('\n%s  (master %s%s)\n  %s' % (r['id'], r['master'], ', ' + r['accent'] + ' accent' if r['accent'] else '', note.replace('\n', '\n  ')))
    print('\n%d icon(s) with notes to work on; %d final of %d.' % (n, len(rv['final']), len([r for r in rows if r['id'] != 'placeholder'])))
    sys.exit(0)

if '--csv' in args:
    csv_path = os.path.join(ROOT, 'target', 'icon-inventory.csv')
    os.makedirs(os.path.dirname(csv_path), exist_ok=True)
    with open(csv_path, 'w', encoding='utf-8') as f:
        f.write('icon,status,master,accent,commands,example_command,used_by_ui\n')
        for r in rows:
            if r['id'] == 'placeholder': continue
            f.write('%s,%s,%s,%s,%d,%s,"%s"\n' % (r['id'], r['status'], r['master'] if r['status'] != 'placeholder' else '', r['accent'], r['commands'], r['example'], ';'.join(r['ui'])))
    print('wrote', os.path.relpath(csv_path, ROOT))

# --- grouping: toolbar tab / panel, else where the app shows the icon -------------------------
TABS = ['SOLID', 'SURFACE', 'MESH', 'SHEET METAL', 'PLASTIC', 'RENDER', 'SKETCH']
PANELS = ['CREATE', 'MODIFY', 'ASSEMBLE', 'POSITION', 'CONFIGURE', 'CONSTRUCT', 'CONSTRAINTS', 'SETUP', 'SCENE', 'IMAGE', 'INSPECT', 'INSERT', 'SELECT', 'FINISH SKETCH']
UI_NAMES = {'browser.rs': 'Browser', 'timeline.rs': 'Timeline', 'timeline_left.rs': 'Timeline', 'context_menu.rs': 'Context menus', 'update.rs': 'Update banner',
            'sketch constraint glyph': 'Sketch constraint glyphs'}
def rank(l, v): return l.index(v) if v in l else len(l)
def group_of(r):
    if r['where']:
        tab, panel, _ = min(r['where'], key=lambda w: (rank(TABS, w[0]), rank(PANELS, w[1])))
        return (rank(TABS, tab), rank(PANELS, panel), tab, panel)
    if r['ui']:
        names = sorted({UI_NAMES.get(u, u.replace('.rs', '').replace('_', ' ').title()) for u in r['ui']})
        return (50, 0, 'IN THE APP', ' · '.join(names))
    if r['commands']: return (60, 0, 'MENUS & PALETTE', 'commands without a toolbar button')
    if r['alias_target']: return (65, 0, 'SHARED MASTERS', 'used through aliases')
    return (70, 0, 'UNUSED', 'no app use yet')

def themed(svg, theme, accent, pal, vision='off', live=False, namespace=''):
    return icon_palette.themed(svg, pal, theme, accent, vision, live, namespace)

def palette_state():
    path = D + '/palette.toml'
    return {'revision': icon_palette.revision(path), 'tokens': icon_palette.tokens(tomllib.load(open(path, 'rb')))}

def build_page(serving):
    rows, masters, pal, summary = scan()
    rv = load_review()
    valid = review_state.valid_final(rv)
    shown = [r for r in rows if r['id'] != 'placeholder']
    shown.sort(key=lambda r: (group_of(r), r['id']))
    cards = []; last = None; tabs_seen = []
    protected = collections.defaultdict(list)
    for name in valid: protected[review_state.master_of(name)].append(name)
    for r in shown:
        g = group_of(r)
        if last is None or g[:2] != last[:2]:
            first_of_tab = last is None or g[2] != last[2]
            if first_of_tab and g[2] not in tabs_seen: tabs_seen.append(g[2])
            txt = (g[2] + ' ▸ ' + g[3]) if first_of_tab else g[3]
            cards.append('<i class="gm%s" data-tab="%s" title="%s">%s</i>' % (' tab' if first_of_tab else '', html.escape(g[2]), html.escape(g[2] + ' ▸ ' + g[3]), html.escape(txt)))
        last = g
        missing = r['status'] == 'placeholder'
        svg_f = open(masters[r['master']][1], encoding='utf-8').read() if r['master'] in masters else '<svg viewBox="0 0 64 64"><title>Missing artwork</title><path d="M10 10L54 10L54 54L10 54Z" fill="none" stroke="currentColor" stroke-width="2.55" stroke-dasharray="3 5"/></svg>'
        small = glob.glob(D + '/svg/small/' + r['master'] + '.svg')
        small_svg = open(small[0], encoding='utf-8').read() if small else None
        inner = ''
        for vision in icon_palette.VISIONS:
            inner += '<span class="vision vision-' + vision + '">'
            for theme, cls in [('light', 'al'), ('dark', 'ad')]:
                scope = 'review-' + r['id'] + '-' + vision + '-' + theme
                inner += '<span class="' + cls + '">' + themed(svg_f, theme, r['accent'], pal, vision, live=True, namespace=scope + '-full') + '</span>'
                if small_svg:
                    inner += '<span class="' + ('sl' if theme == 'light' else 'sd') + '">' + themed(small_svg, theme, r['accent'], pal, vision, live=True, namespace=scope + '-small') + '</span>'
            inner += '</span>'
        src = ('= ' + r['master']) if r['master'] != r['id'] else ''
        tip = '; '.join('%s ▸ %s: %s' % w for w in r['where']) or ', '.join(r['ui']) or ('%d command(s)' % r['commands'])
        extra = ((' <i class="acc">%s accent</i>' % r['accent']) if r['accent'] else '') + (' <i class="st">%s</i>' % r['status'] if r['status'] != 'done' else '')
        if r['id'] not in valid and protected[r['master']]:
            extra += ' <span class="shared" title="Accepted as ' + html.escape(', '.join(protected[r['master']])) + '; reopen that alias before requesting a redraw">shared artwork accepted</span>'
        if r['id'] in rv['final'] and r['id'] not in valid: extra += ' <i class="st">changed since acceptance</i>'
        cards.append('<div class="card" data-id="%s" data-accent="%s" data-tab="%s" data-k="%s" title="%s" data-missing="%s"><div class="art%s">%s</div><div class="meta"><b>%s</b>%s%s<br><small>%s</small></div>'
                     '<button class="ck" title="Accept as final" aria-label="Accept %s as final" %s>✓</button><button class="nt" title="Notes">✎</button></div>' % (
            html.escape(r['id']), html.escape(r['accent']), html.escape(g[2]), html.escape(r['id'] + ' ' + r['example'] + ' ' + r['accent'] + ' ' + g[3].lower()), html.escape(tip), 'true' if missing else 'false',
            ' hs' if small_svg else '', inner, html.escape(r['id']), ('<span class="src"> ' + html.escape(src) + '</span>') if src else '', extra, html.escape(r['example'] or ', '.join(r['ui']) or ''), html.escape(r['id']), 'disabled' if missing else ''))
    tabchips = ''.join('<button class="chip" data-tab="%s">%s</button>' % (html.escape(t), html.escape(t.title())) for t in tabs_seen)
    state = json.dumps({'final': sorted(valid), 'notes': rv['notes'], 'serving': serving, 'rev': art_rev(), 'palette': palette_state(), 'hashes': {r['id']: master_hash(r['master']) for r in shown if r['status'] != 'placeholder'}}).replace('<', r'\u003c')
    return load_template().replace('@@PALETTE_CSS@@', icon_palette.palette_css(pal) + '\n' + icon_palette.review_css(pal, live=True)).replace('@@GUIDE@@', '/guide' if serving else os.path.basename(out_path) + '.guide.html').replace('@@WARNINGS@@', ('Unassigned commands: ' + html.escape(', '.join(unassigned))) if unassigned else '').replace('@@SUMMARY@@', html.escape(summary)).replace('@@CARDS@@', ''.join(cards)).replace('@@TABS@@', tabchips).replace('@@STATE@@', state)

WEB = os.path.join(HERE, 'review')
def load_template():
    template = open(os.path.join(WEB, 'index.html'), encoding='utf-8').read()
    return template.replace('@@CSS@@', open(os.path.join(WEB, 'review.css'), encoding='utf-8').read()).replace('@@JS@@', open(os.path.join(WEB, 'review.js'), encoding='utf-8').read())


def build_guide():
    sys.path.insert(0, os.path.join(HERE, 'author'))
    from bases import examples
    rows, masters, pal, _ = scan()
    by_id = {r['id']: r for r in rows}
    catalogue = tomllib.load(open(D + '/references.toml', 'rb'))
    sections = '<h2>Canonical bases</h2><div class="examples">'
    for label, svg in examples().items():
        sections += '<article><b>' + label + '</b>' + themed(svg, 'light', '', pal) + '</article>'
    sections += '</div>'
    for category in catalogue['category'].values():
        sections += '<h2>' + html.escape(category['label']) + '</h2><p>' + html.escape(category['rule']) + '</p><div class="examples">'
        for name in category['icons']:
            row = by_id[name]; svg = open(masters[row['master']][1], encoding='utf-8').read()
            sections += '<article><b>' + name + '</b><div class="light">' + themed(svg, 'light', row['accent'], pal, namespace='guide-' + category['label'].replace(' ', '-') + '-' + name + '-light') + '</div><div class="dark">' + themed(svg, 'dark', row['accent'], pal, namespace='guide-' + category['label'].replace(' ', '-') + '-' + name + '-dark') + '</div></article>'
        sections += '</div>'
    template = open(os.path.join(WEB, 'guide.html'), encoding='utf-8').read()
    return template.replace('@@EXAMPLES@@', sections)


if '--serve' in args:
    import http.server
    port = int(args[args.index('--serve') + 1]) if len(args) > args.index('--serve') + 1 and args[args.index('--serve') + 1].isdigit() else 8770
    _r = load_review(); review_state.save(_r['final'], _r['notes'])      # fills in missing hashes
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a): pass
        def _send(self, code, body, ctype):
            b = body.encode(); self.send_response(code); self.send_header('Content-Type', ctype); self.send_header('Content-Length', str(len(b))); self.send_header('Cache-Control', 'no-store'); self.end_headers(); self.wfile.write(b)
        def do_GET(self):
            if self.path.split('?')[0] in ('/', '/index.html'): self._send(200, build_page(True), 'text/html; charset=utf-8')
            elif self.path.split('?')[0] == '/api/review': self._send(200, json.dumps({'final': sorted(review_state.valid_final()), 'notes': load_review()['notes'], 'rev': art_rev()}), 'application/json')
            elif self.path.split('?')[0] == '/api/palette': self._send(200, json.dumps(palette_state()), 'application/json')
            elif self.path.split('?')[0] == '/guide': self._send(200, build_guide(), 'text/html; charset=utf-8')
            else: self._send(404, 'not found', 'text/plain')
        def do_POST(self):
            with review_state.LOCK: self._post()
        def _post(self):
            if self.path not in ('/api/review', '/api/export', '/api/palette'): return self._send(404, 'not found', 'text/plain')
            try:
                n = int(self.headers.get('Content-Length', '0'))
                if n <= 0 or n > 4_000_000: raise ValueError('invalid request size')
                inc = json.loads(self.rfile.read(n))
                if self.path == '/api/palette':
                    icon_palette.save_colours(D + '/palette.toml', inc.get('revision'), inc.get('changes'))
                    return self._send(200, json.dumps({'ok': True, **palette_state()}), 'application/json')
                base = inc.get('base')
                if not isinstance(base, dict): raise ValueError('stale page: reload it')
                cur = load_review(); old = cur['final']
                # three-way merge: apply what this page changed since the state it last saw (`base`) onto the file as it is now
                bf = set(base.get('final', [])); cf = set(inc.get('final', []))
                added = cf - bf
                if added:
                    available = {r['id'] for r in scan()[0] if r['status'] != 'placeholder'}
                    if not added <= available: raise ValueError('missing artwork cannot be accepted')
                    if inc.get('rev') != art_rev(): raise ValueError('artwork changed: reload before accepting it')
                final = (set(old) | added) - (bf - cf)
                notes = dict(cur['notes']); bn = base.get('notes', {}); cn = inc.get('notes', {})
                for k in set(bn) | set(cn):
                    if bn.get(k, '') != cn.get(k, ''):
                        if cn.get(k, '').strip(): notes[k] = cn[k]
                        else: notes.pop(k, None)
                review_state.save({i: '' if i in added else old.get(i, '') for i in final}, notes)
                res = {'ok': True}
                if self.path == '/api/export':
                    r = load_review(); valid = review_state.valid_final(r)
                    protected = {review_state.master_of(i) for i in valid}
                    todo = sorted(i for i in r['notes'] if i not in valid and review_state.master_of(i) not in protected)
                    res['sent'] = len(todo)
                    if todo:
                        os.makedirs(REQUESTS, exist_ok=True)
                        request_id = uuid.uuid4().hex
                        path = os.path.join(REQUESTS, request_id + '.json')
                        # Publish a complete snapshot atomically so the chat never reads half a request.
                        with open(path + '.tmp', 'w') as f:
                            json.dump({'version': 1, 'id': request_id,
                                       'created_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                       'notes': {i: r['notes'][i] for i in todo}}, f, indent=1)
                            f.write('\n')
                        os.replace(path + '.tmp', path)
                        res['request_id'] = request_id
                self._send(200, json.dumps(res), 'application/json')
            except Exception as e:
                self._send(400, json.dumps({'ok': False, 'error': str(e)}), 'application/json')
    srv = http.server.ThreadingHTTPServer(('127.0.0.1', port), H)
    print('review page: http://127.0.0.1:%d  (saves %s; Ctrl-C to stop)' % (port, os.path.relpath(REVIEW, ROOT)))
    try: srv.serve_forever()
    except KeyboardInterrupt: pass
    sys.exit(0)

page = build_page(False)
os.makedirs(os.path.dirname(out_path), exist_ok=True)
open(out_path, 'w', encoding='utf-8').write(page)
if os.path.exists(D + '/references.toml'):
    guide = build_guide().replace('href="/"', 'href="' + html.escape(os.path.basename(out_path)) + '"')
    open(out_path + '.guide.html', 'w', encoding='utf-8').write(guide)
print('wrote', os.path.relpath(out_path, ROOT), '(%d KB)' % (len(page) // 1024))
