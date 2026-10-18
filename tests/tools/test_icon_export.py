"""Exercise the real HTTP export endpoint without touching the working review or queue."""
import json
import csv
import pathlib
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.error
import urllib.request


class IconExportTest(unittest.TestCase):
    def test_inventory_includes_nested_and_helper_icon_uses(self):
        source = pathlib.Path(__file__).resolve().parents[2] / 'assets/icons/ui/design/tools'
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            design = root / 'assets/icons/ui/design'
            tools = design / 'tools'
            tools.mkdir(parents=True)
            for name in ('icon_status.py', 'review_state.py', 'icon_palette.py'):
                shutil.copy(source / name, tools / name)
            shutil.copytree(source / 'review', tools / 'review')
            (design / 'palette.toml').write_text('[roles.line]\nlight="#000000"\ndark="#ffffff"\n')
            for name in ('aliases.toml',):
                (design / name).write_text('')
            svg = design / 'svg/model'
            svg.mkdir(parents=True)
            for name in ('agent_chat', 'pattern_flip', 'normal', 'infinite', 'trace'):
                (svg / (name + '.svg')).write_text('<svg viewBox="0 0 64 64"></svg>')
            ui = root / 'crates/ui-egui/src'
            (ui / 'agent_chat').mkdir(parents=True)
            (ui / 'agent_chat/panel.rs').write_text('icons::paint(p, r, "agent_chat", ink, fill, a);')
            (ui / 'dialogs.rs').write_text('pattern_toggle(ui, alignment_label, "Flip direction", "pattern_flip", reversed);')
            (ui / 'sketch_palette.rs').write_text('''
                for (i, (label, icon, tip)) in [
                    ("—", "normal", "Normal"),
                    ("↔", "infinite", "Infinite"),
                ].iter().enumerate() { icons::paint(p, r, icon, ink, fill, a); }
            ''')
            engine = root / 'crates/engine/src/cmd'
            engine.mkdir(parents=True)
            # A long builder must not lose its icon after the first fourteen lines.
            (engine / 'motion.rs').write_text('CommandSpec::new("motion.trace", "Trace", trace)\n'
                                             + '\n' * 16 + '.icon("trace").params("doc");')
            subprocess.run([sys.executable, '-B', str(tools / 'icon_status.py'), '--csv'],
                           check=True, capture_output=True, text=True)
            with open(root / 'target/icon-inventory.csv') as f:
                rows = {r['icon']: r for r in csv.DictReader(f)}
            self.assertEqual(set(rows), {'agent_chat', 'pattern_flip', 'normal', 'infinite', 'trace'})
            self.assertIn('agent_chat/panel.rs', rows['agent_chat']['used_by_ui'])
            self.assertIn('dialogs.rs', rows['pattern_flip']['used_by_ui'])
            for name in ('normal', 'infinite'):
                self.assertIn('sketch_palette.rs', rows[name]['used_by_ui'])
            self.assertEqual(rows['trace']['example_command'], 'motion.trace')

    def test_export_snapshots_only_pending_notes_for_codex(self):
        source = pathlib.Path(__file__).resolve().parents[2] / 'assets/icons/ui/design/tools'
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            design = root / 'assets/icons/ui/design'
            tools = design / 'tools'
            tools.mkdir(parents=True)
            for name in ('icon_status.py', 'review_state.py', 'icon_palette.py'):
                shutil.copy(source / name, tools / name)
            shutil.copytree(source / 'review', tools / 'review')
            for name in ('palette.toml', 'aliases.toml'):
                (design / name).write_text('')
            (design / 'palette.toml').write_text('[roles.line]\nlight="#000000"\ndark="#ffffff"\n')
            svg = design / 'svg/model'
            svg.mkdir(parents=True)
            for name in ('box', 'joint'):
                (svg / (name + '.svg')).write_text('<svg viewBox="0 0 64 64"></svg>')
            # An accepted icon's note must never be exported, even if it still has text.
            original = {'final': {'box': ''}, 'notes': {'box': 'Keep it', 'joint': 'Round the corners\nUse related icons'}}
            (design / 'review.json').write_text(json.dumps(original))
            with socket.socket() as port_socket:
                port_socket.bind(('127.0.0.1', 0))
                port = port_socket.getsockname()[1]
            proc = subprocess.Popen([sys.executable, str(tools / 'icon_status.py'), '--serve', str(port)],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                url = 'http://127.0.0.1:%d' % port
                # Up to a minute: CI machines can be busy enough that the server starts slowly.
                deadline = time.monotonic() + 60
                while time.monotonic() < deadline:
                    try:
                        with urllib.request.urlopen(url + '/api/review', timeout=1) as response:
                            base = json.load(response)
                        break
                    except urllib.error.URLError:
                        if proc.poll() is not None:
                            self.fail(proc.stderr.read().decode())
                        time.sleep(0.02)
                else:
                    self.fail('review server did not start')

                def post(endpoint, state):
                    data = json.dumps({'base': base, **state}).encode()
                    with urllib.request.urlopen(urllib.request.Request(url + endpoint, data=data), timeout=2) as response:
                        return json.load(response)

                state = {'final': base['final'], 'notes': base['notes']}
                inbox = root / 'target/codex-icon-review/inbox'
                post('/api/review', state)
                self.assertFalse(inbox.exists(), 'autosave must not request redraws')
                result = post('/api/export', state)
                self.assertTrue(result['ok'])
                self.assertEqual(result['sent'], 1)
                requests = list(inbox.glob('*.json'))
                self.assertEqual(len(requests), 1)
                request = json.loads(requests[0].read_text())
                self.assertEqual(request['id'], result['request_id'])
                self.assertEqual(request['notes'], {'joint': original['notes']['joint']})
                self.assertFalse(list(inbox.glob('*.tmp')))
                self.assertFalse((root / 'target/review-requests.log').exists())
                with urllib.request.urlopen(url, timeout=2) as response:
                    page = response.read().decode()
                self.assertIn('local review inbox', page)
                self.assertNotIn('Claude', page)
                self.assertIn('location.reload()', page)
                # Existing tabs also reload when the server's page implementation changes.
                with open(tools / 'icon_status.py', 'a') as f:
                    f.write('\n# page update\n')
                with urllib.request.urlopen(url + '/api/review', timeout=2) as response:
                    self.assertNotEqual(json.load(response)['rev'], base['rev'])
                # A note resolved on disk cannot be resurrected by an unchanged stale page.
                saved = json.loads((design / 'review.json').read_text())
                saved['notes'] = {'box': 'Keep it'}
                (design / 'review.json').write_text(json.dumps(saved))
                empty = post('/api/export', state)
                self.assertEqual(empty['sent'], 0)
                self.assertEqual(len(list(inbox.glob('*.json'))), 1)
            finally:
                proc.terminate()
                proc.wait(timeout=5)
                proc.stderr.close()


if __name__ == '__main__':
    unittest.main()
