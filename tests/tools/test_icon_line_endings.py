"""The icon tools read a checkout with CRLF line endings (Git for Windows' default) as the LF text committed (#1930)."""
from pathlib import Path
import sys
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[2] / 'assets/icons/ui/design/tools'
sys.path.insert(0, str(TOOLS))
import icon_palette  # noqa: E402
import review_state  # noqa: E402

SVG = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n<circle cx="32" cy="32" r="20"/>\n</svg>\n'


class LineEndingTest(unittest.TestCase):
    def test_an_accepted_icon_hashes_the_same_checked_out_with_crlf(self):
        with tempfile.TemporaryDirectory() as temporary:
            hashes = []
            for newline in (b'\n', b'\r\n'):
                design = Path(temporary) / ('crlf' if newline == b'\r\n' else 'lf')
                (design / 'svg/model').mkdir(parents=True)
                (design / 'svg/model/box.svg').write_bytes(SVG.encode().replace(b'\n', newline))
                hashes.append(review_state.master_hash('box', str(design)))
            self.assertEqual(hashes[0], hashes[1])

    def test_a_palette_checked_out_with_crlf_takes_a_save_at_its_own_revision(self):
        palette = '# roles\n[roles.ink]\nlight = "#111111"\ndark = "#eeeeee"\n'
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'palette.toml'
            path.write_bytes(palette.encode().replace(b'\n', b'\r\n'))
            lf = Path(temporary) / 'lf.toml'
            lf.write_bytes(palette.encode())
            self.assertEqual(icon_palette.revision(path), icon_palette.revision(lf))
            icon_palette.save_colours(path, icon_palette.revision(path), {'roles.ink': {'light': '#222222', 'dark': '#dddddd'}})
            self.assertIn('dark = "#dddddd"', path.read_text(encoding='utf-8'))


if __name__ == '__main__':
    unittest.main()
