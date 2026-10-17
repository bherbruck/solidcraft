"""Long-term discovery and acceptance behavior of the repository-owned review app."""
import contextlib
import json
import re
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET

TOOLS = Path(__file__).resolve().parents[2] / 'assets/icons/ui/design/tools'
SVG = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><circle cx="32" cy="32" r="20" fill="currentColor"/></svg>'


@contextlib.contextmanager
def review_fixture():
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        design = root / 'assets/icons/ui/design'
        shutil.copytree(TOOLS, design / 'tools')
        (design / 'svg/model').mkdir(parents=True)
        (design / 'svg/model/box.svg').write_text(SVG)
        shutil.copy2(TOOLS.parent / 'palette.toml', design / 'palette.toml')
        (design / 'references.toml').write_text('[category.model]\nlabel="Model"\nrule="Standard projection"\nicons=["box"]\n')
        (design / 'aliases.toml').write_text('[alias]\nbox_alias="box"\n')
        (design / 'review.json').write_text(json.dumps({'final': {'box': ''}, 'notes': {}}))
        with socket.socket() as available:
            available.bind(('127.0.0.1', 0))
            port = available.getsockname()[1]
        process = subprocess.Popen([sys.executable, '-B', str(design / 'tools/icon_status.py'), '--serve', str(port)],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        url = f'http://127.0.0.1:{port}'
        def get(path):
            with urllib.request.urlopen(url + path, timeout=3) as response:
                data = response.read().decode()
                return json.loads(data) if path == '/api/review' else data
        def post(state, final=None, notes=None, endpoint='/api/review', rev=None):
            payload = state if endpoint == '/api/palette' else {
                'base': state, 'final': state['final'] if final is None else final,
                'notes': state['notes'] if notes is None else notes, 'rev': rev or state['rev']}
            try:
                with urllib.request.urlopen(urllib.request.Request(url + endpoint, data=json.dumps(payload).encode()), timeout=3) as response:
                    return json.load(response)
            except urllib.error.HTTPError as error:
                error.close()
                raise
        try:
            for attempt in range(100):
                try:
                    get('/api/review'); break
                except urllib.error.URLError:
                    if process.poll() is not None: raise RuntimeError(process.stderr.read().decode())
                    time.sleep(.02)
            else: raise RuntimeError('server did not start')
            yield root, design, get, post
        finally:
            process.terminate(); process.wait(timeout=5); process.stderr.close()


class IconReviewTest(unittest.TestCase):
    def test_inline_clips_and_masks_are_scoped_to_each_preview_and_dashes_use_guides(self):
        with review_fixture() as (root, design, get, post):
            for name in ('environment_map', 'new', 'draft', 'bounding_solid', 'extend'):
                shutil.copy2(TOOLS.parent / f'svg/model/{name}.svg', design / f'svg/model/{name}.svg')
            with (design / 'aliases.toml').open('a') as aliases:
                aliases.write('surface_env={master="environment_map",accent="surface"}\n')
            page = get('/')
            fragments = re.findall(r'<svg\b.*?</svg>', page, re.S)
            seen = set()
            for svg in fragments:
                ids = re.findall(r'\bid="([^"]+)"', svg)
                self.assertTrue(seen.isdisjoint(ids), 'all theme/vision/alias previews need unique ids')
                seen.update(ids)
                for ref in re.findall(r'url\(#([^)]+)\)', svg): self.assertIn(ref, ids, 'clip or mask must belong to this SVG')
            self.assertGreater(len(seen), 10)
            for svg in fragments:
                tree = ET.fromstring(svg)
                mask_nodes = {id(node) for mask in tree.iter('{http://www.w3.org/2000/svg}mask') for node in mask.iter()}
                for node in tree.iter():
                    if not node.get('stroke-dasharray'): continue
                    stroke = node.get('stroke', 'none')
                    if stroke == 'none': continue
                    if id(node) in mask_nodes and stroke == 'black': continue
                    self.assertTrue(any(role in stroke for role in ('roles-guide-', 'active-3-')), stroke)
                self.assertNotIn('var(--icon-palette-roles-gap-', svg)
            self.assertIn('review-environment_map-off-dark-full-disc-env', page)
            def card(icon): return page.split('data-id="' + icon + '"', 1)[1].split('<div class="meta">', 1)[0]
            self.assertIn('stroke="var(--icon-palette-roles-active-3-', card('bounding_solid'))
            self.assertIn('stroke="var(--icon-palette-accents-sketch-active-3-', card('extend'))
            self.assertIn('stroke="var(--icon-palette-roles-guide-', card('draft'))

    def test_live_palette_tokens_save_only_display_colours_and_reject_stale_or_invalid_changes(self):
        with review_fixture() as (root, design, get, post):
            palette_path = design / 'palette.toml'
            original = palette_path.read_text()
            palette = tomllib.loads(original)
            state = json.loads(get('/api/palette'))
            tokens = {token['id']: token for token in state['tokens']}
            self.assertIn('roles.ink', tokens)
            self.assertIn('accents.surface.active-2', tokens)
            self.assertIn('variants.red-green.sketch.active-3', tokens)
            self.assertNotIn('accents.construction.guide', tokens, 'guides have one shared colour')
            self.assertNotIn('roles.ink.source', tokens)
            self.assertTrue(tokens['roles.gap']['knockout'])
            accepted = (design / 'review.json').read_bytes()
            artwork = (design / 'svg/model/box.svg').read_bytes()
            def save(revision, changes):
                return post({'revision': revision, 'changes': changes}, endpoint='/api/palette')
            replacement = '#123456' if tokens['roles.ink']['dark'] != '#123456' else '#654321'
            paints = {'light': tokens['roles.ink']['light'], 'dark': replacement}
            changes = {'roles.ink': paints,
                       'accents.surface.active-2': {'light': tokens['accents.surface.active-2']['light'], 'dark': '#bc789a'},
                       'variants.red-green.sketch.active-3': {'light': tokens['variants.red-green.sketch.active-3']['light'], 'dark': '#778899'}}
            saved = save(state['revision'], changes)
            self.assertTrue(saved['ok'])
            expected = tomllib.loads(original)
            for key, colours in changes.items():
                target = expected
                for part in key.split('.'): target = target[part]
                target.update(colours)
            self.assertEqual(tomllib.loads(palette_path.read_text()), expected)
            self.assertIn('# Icon colour roles.', palette_path.read_text(), 'comments remain intact')
            self.assertEqual(tomllib.loads(palette_path.read_text())['roles']['ink']['source'], palette['roles']['ink']['source'])
            for revision, changes in [
                (state['revision'], {'roles.ink': paints}),
                (saved['revision'], {'roles.ink.source': paints}),
                (saved['revision'], {'accents.construction.guide': paints}),
                (saved['revision'], {'roles.gap': paints}),
                (saved['revision'], {'roles.ink': {'light': 'url(evil)', 'dark': replacement}}),
            ]:
                before = palette_path.read_bytes()
                with self.assertRaises(urllib.error.HTTPError): save(revision, changes)
                self.assertEqual(palette_path.read_bytes(), before)
            page = get('/')
            self.assertIn('var(--icon-palette-roles-line-light,', page)
            self.assertIn('--icon-palette-roles-ink-dark:' + replacement, page)
            self.assertIn('id="palette-tokens"', page)
            self.assertIn('input.oninput', page)
            self.assertIn('body.palette-preview .card.final .art{opacity:1}', page)
            self.assertEqual((design / 'review.json').read_bytes(), accepted)
            self.assertEqual((design / 'svg/model/box.svg').read_bytes(), artwork)
            self.assertFalse(list(design.glob('*.tmp')))

    def test_knockouts_mask_only_preceding_layers_and_preserve_later_foreground(self):
        with review_fixture() as (root, design, get, post):
            source = '''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><clipPath id="left"><path d="M0 0L32 0L32 64L0 64Z"/></clipPath></defs><circle id="base" cx="32" cy="32" r="28" fill="#8e9faf"/><g clip-path="url(#left)"><circle cx="32" cy="32" r="12" fill="#ffffff"/></g><circle id="foreground" cx="32" cy="32" r="3" fill="#236ead"/></svg>'''
            path = design / 'svg/model/cutout.svg'
            path.write_text(source)
            accepted = (design / 'review.json').read_bytes()
            page = get('/')
            fragments = [svg for svg in re.findall(r'<svg\b.*?</svg>', page, re.S) if '-base"' in svg]
            self.assertGreater(len(fragments), 3)
            ns = '{http://www.w3.org/2000/svg}'
            for svg in fragments:
                tree = ET.fromstring(svg)
                mask = tree.find(ns + 'defs/' + ns + 'mask')
                self.assertIsNotNone(mask)
                self.assertEqual(mask.get('maskUnits'), 'userSpaceOnUse')
                self.assertEqual(mask[0].get('fill'), 'white')
                self.assertIn('-left)', mask[1].get('clip-path'))
                self.assertEqual(mask[1][0].get('fill'), 'black')
                preceding, foreground = list(tree)[-2:]
                self.assertEqual(preceding.get('mask'), 'url(#' + mask.get('id') + ')')
                self.assertTrue(preceding[0].get('id').endswith('-base'))
                self.assertTrue(foreground.get('id').endswith('-foreground'))
                self.assertIsNone(foreground.get('mask'))
                self.assertNotIn('roles-gap-', svg)
            self.assertEqual(path.read_text(), source)
            self.assertEqual((design / 'review.json').read_bytes(), accepted)

    def test_semantic_colours_reload_from_one_palette_without_changing_acceptance(self):
        with review_fixture() as (root, design, get, post):
            palette = design / 'palette.toml'
            colours = tomllib.loads(palette.read_text())
            source = colours['roles']['active-3'].get('source', colours['roles']['active-3']['light'])
            art = SVG.replace('currentColor', source)
            for name in ('line', 'c_fix'):
                (design / f'svg/model/{name}.svg').write_text(art)
            with (design / 'aliases.toml').open('a') as aliases:
                aliases.write('ground="c_fix"\nsurface_line={master="line",accent="surface"}\n')
            state, accepted = get('/api/review'), (design / 'review.json').read_bytes()
            def card(page, icon):
                return page.split('data-id="' + icon + '"', 1)[1].split('<div class="meta">', 1)[0]
            page = get('/')
            self.assertIn(colours['accents']['sketch']['active-3']['light'], card(page, 'line'))
            self.assertIn(colours['variants']['red-green']['sketch']['active-3']['dark'], card(page, 'line'),
                          'red/green vision shares the application ramp')
            self.assertIn(colours['accents']['constraint']['active-3']['light'], card(page, 'c_fix'))
            self.assertIn(colours['roles']['active-3']['light'], card(page, 'ground'),
                          'a shared master does not transfer its meaning to an alias')
            self.assertIn(colours['accents']['surface']['active-3']['light'], card(page, 'surface_line'),
                          'explicit alias accent wins over the Sketch master')
            def set_light(section, colour):
                lines, current = palette.read_text().splitlines(), ''
                for index, line in enumerate(lines):
                    if line.strip().startswith('['):
                        current = line.strip().split(']', 1)[0] + ']'
                    elif current == '[' + section + ']' and line.split('=', 1)[0].strip() == 'light':
                        lines[index] = 'light = ' + json.dumps(colour)
                        palette.write_text('\n'.join(lines) + '\n')
                        return
                self.fail('missing palette section ' + section)
            replacement = '#123456' if colours['accents']['sketch']['active-3']['light'] != '#123456' else '#654321'
            set_light('accents.sketch.active-3', replacement)
            self.assertNotEqual(state['rev'], get('/api/review')['rev'])
            self.assertIn(replacement, card(get('/'), 'line'), 'a running server reads the updated palette')
            base = '#345678' if colours['roles']['active-3']['light'] != '#345678' else '#876543'
            set_light('roles.active-3', base)
            self.assertIn(base, card(get('/'), 'ground'), 'source identifiers survive changing the base display palette too')
            self.assertEqual((design / 'review.json').read_bytes(), accepted)
            self.assertEqual((design / 'svg/model/line.svg').read_text(), art)

    def test_new_unused_masters_and_alias_targets_are_never_hidden(self):
        with review_fixture() as (root, design, get, post):
            original = get('/api/review')
            page = get('/')
            self.assertIn('data-id="box"', page, 'unused alias target must still have a card')
            self.assertIn('data-id="box_alias"', page)
            (design / 'svg/model/brand_new.svg').write_text(SVG)
            current = get('/api/review')
            self.assertNotEqual(original['rev'], current['rev'])
            self.assertNotIn('brand_new', current['final'])
            page = get('/')
            self.assertIn('data-id="brand_new"', page)
            self.assertIn("let filter = 'open', tab = ''", page)
            self.assertNotIn("const VK='icon-view'", page)
            self.assertIn('cards.length - finals', page, 'notes remain a subset of Open')
            post(current, final=current['final'] + ['brand_new'])
            self.assertIn('brand_new', get('/api/review')['final'])
            # A new opening does not inherit a previous session's hidden tab/query.
            self.assertIn('data-f="open" class="on"', get('/'))

    def test_changed_art_reopens_and_requires_deliberate_current_acceptance(self):
        with review_fixture() as (root, design, get, post):
            initial = get('/api/review')
            self.assertIn('box', initial['final'])
            old_hash = json.loads((design / 'review.json').read_text())['final']['box']
            (design / 'svg/model/box.svg').write_text(SVG.replace('r="20"', 'r="18"'))
            changed = get('/api/review')
            self.assertNotIn('box', changed['final'])
            self.assertIn('changed since acceptance', get('/'))
            self.assertEqual(old_hash, json.loads((design / 'review.json').read_text())['final']['box'])
            (design / 'svg/model/box.svg').write_text(SVG.replace('r="20"', 'r="17"'))
            with self.assertRaises(urllib.error.HTTPError) as error:
                post(changed, final=['box'])
            self.assertEqual(error.exception.code, 400)
            post(get('/api/review'), final=['box'])
            self.assertIn('box', get('/api/review')['final'])
            self.assertNotEqual(old_hash, json.loads((design / 'review.json').read_text())['final']['box'])

    def test_missing_art_is_visible_but_cannot_be_accepted(self):
        with review_fixture() as (root, design, get, post):
            commands = root / 'crates/engine/src/cmd'
            commands.mkdir(parents=True)
            (commands / 'example.rs').write_text('CommandSpec::new("example.action", "Example", run).icon("missing");')
            page = get('/')
            self.assertIn('data-id="missing"', page)
            self.assertIn('aria-label="Accept missing as final" disabled', page)
            with self.assertRaises(urllib.error.HTTPError):
                post(get('/api/review'), final=['box', 'missing'])

    def test_final_alias_protects_shared_art_and_newer_notes_survive_stale_save(self):
        with review_fixture() as (root, design, get, post):
            state = get('/api/review')
            post(state, final=['box', 'box_alias'])
            state = get('/api/review')
            post(state, final=['box_alias'], notes={'box': 'Unreviewed master note'})
            state = get('/api/review')
            self.assertEqual(post(state, endpoint='/api/export')['sent'], 0)
            # A stale page changes another note; it must not resurrect replaced feedback.
            saved = json.loads((design / 'review.json').read_text())
            saved['notes']['box'] = 'Newer feedback'
            (design / 'review.json').write_text(json.dumps(saved))
            post(state, notes={**state['notes'], 'box_alias': 'Another note'})
            self.assertEqual(get('/api/review')['notes']['box'], 'Newer feedback')
            self.assertFalse(list(design.glob('*.tmp')))

    def test_export_resolution_preserves_notes_added_since_the_snapshot(self):
        with review_fixture() as (root, design, get, post):
            (design / 'svg/model/fresh.svg').write_text(SVG)
            state = get('/api/review')
            post(state, notes={'fresh': 'Newer feedback'})
            script = """
import sys
sys.path.insert(0,sys.argv[1])
import review_state
request={'notes':{'fresh':'Exported feedback'}}
assert review_state.resolve_export(request, {'fresh'}) == []
assert review_state.load()['notes']['fresh'] == 'Newer feedback'
request={'notes':{'fresh':'Newer feedback'}}
assert review_state.resolve_export(request, {'fresh'}) == ['fresh']
assert review_state.load()['notes'] == {}
"""
            subprocess.run([sys.executable, '-B', '-c', script, str(design / 'tools')], check=True)

    def test_style_reference_page_uses_repository_bases(self):
        with review_fixture() as (root, design, get, post):
            page = get('/guide')
            self.assertIn('Canonical grey cube', page)
            self.assertIn('Canonical grey slab', page)
            self.assertIn('Standard projection', page)
            self.assertIn('STYLE_GUIDE.md', page)

    def test_scaffold_refuses_overwrite_and_starts_unaccepted(self):
        with review_fixture() as (root, design, get, post):
            # The fixture's palette is intentionally minimal; the scaffold uses
            # the real helper constants and emits an editable starter SVG.
            (root / 'ATTRIBUTION.md').write_text('# Attribution\n')
            command = [sys.executable, '-B', str(design / 'tools/author/new_icon.py')]
            first = subprocess.run(command + ['fresh_shape', '--base', 'cube'], capture_output=True, text=True)
            self.assertEqual(first.returncode, 0, first.stderr)
            artwork = design / 'svg/model/fresh_shape.svg'
            self.assertTrue(artwork.exists())
            self.assertNotIn('fresh_shape', get('/api/review')['final'])
            self.assertIn('fresh_shape.svg', (root / 'ATTRIBUTION.md').read_text())
            before = artwork.read_bytes()
            again = subprocess.run(command + ['fresh_shape'], capture_output=True, text=True)
            self.assertNotEqual(again.returncode, 0)
            self.assertEqual(artwork.read_bytes(), before)
            alias = subprocess.run(command + ['box_alias'], capture_output=True, text=True)
            self.assertNotEqual(alias.returncode, 0)


if __name__ == '__main__': unittest.main()
