#!/usr/bin/env python3
"""Create a new, unaccepted SVG from a canonical base; never overwrite artwork."""
import argparse
from pathlib import Path
import re
import sys
import tomllib
from lib import Icon
from bases import grey_cube, grey_slab


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('name', help='lower_snake_case app icon id')
    parser.add_argument('--base', choices=('cube', 'slab', 'empty'), default='cube')
    parser.add_argument('--family', choices=('model', 'flat', 'workspace'), default='model')
    args = parser.parse_args()
    design = Path(__file__).resolve().parents[2]
    if not re.fullmatch('[a-z][a-z0-9_]*', args.name): parser.error('use a lower_snake_case icon id')
    aliases = tomllib.loads((design / 'aliases.toml').read_text()).get('alias', {})
    if args.name in aliases or list((design / 'svg').glob('*/' + args.name + '.svg')):
        parser.error('id already exists; edit its master after checking review.json')
    if args.family != 'model' and args.base != 'empty': parser.error('cube/slab bases belong to the model family')
    icon = Icon(args.name)
    if args.base == 'cube': grey_cube(icon)
    if args.base == 'slab': grey_slab(icon)
    icon.write(args.family)
    root = design.parents[3]
    path = f'assets/icons/ui/design/svg/{args.family}/{args.name}.svg'
    attribution = root / 'ATTRIBUTION.md'
    row = f'| {path} | SolidCraft contributors | original icon master (64 grid) | MIT OR Apache-2.0 |\n'
    text = attribution.read_text()
    if '| ' + path + ' |' not in text:
        anchor = '| assets/icons/ui/design/svg/model/placeholder.svg'
        if anchor in text: text = text.replace(anchor, row + anchor, 1)
        else: text += '\n' + row
        attribution.write_text(text)
    print(path)
    print('Starter only: finish the operation cue using STYLE_GUIDE.md; review status is Open.')


if __name__ == '__main__': main()
