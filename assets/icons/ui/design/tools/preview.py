#!/usr/bin/env python3
"""Contact sheet of svg/ in both themes at 16/20/26/64 px, resolving roles through palette.toml.
usage: preview.py <design dir> <out.html>   (serve the design dir and open the html)"""
import sys, glob, os, tomllib
import icon_palette
d, out = sys.argv[1], sys.argv[2]
palette = tomllib.load(open(d + '/palette.toml', 'rb'))
aliases = tomllib.load(open(d + '/aliases.toml', 'rb')).get('alias', {})
def themed(svg, theme, icon, family, size):
    _, accent = icon_palette.resolve(palette, aliases, icon)
    return icon_palette.themed(svg, palette, theme, accent, namespace='preview-' + family + '-' + icon + '-' + theme + '-' + str(size))
h = '<meta charset=utf-8><style>body{margin:0;font:10px sans-serif}.p{padding:8px}.light{background:#fff;color:#222}.dark{background:#1e2530;color:#ddd}.c{display:inline-block;width:92px;text-align:center;margin:3px 2px;vertical-align:top}.s{display:flex;gap:6px;justify-content:center;align-items:center;margin-top:2px}h3{margin:4px}</style>'
for theme in ('light', 'dark'):
    for fam in sorted(os.listdir(d + '/svg')):
        h += '<div class="p %s"><h3>%s - %s</h3>' % (theme, fam, theme)
        for f in sorted(glob.glob('%s/svg/%s/*.svg' % (d, fam))):
            svg, icon = open(f).read(), os.path.basename(f)[:-4]
            h += '<div class=c>'
            h += '<div style="width:64px;height:64px;margin:auto">%s</div>' % themed(svg, theme, icon, fam, 64).replace('<svg ', '<svg width=64 height=64 ', 1)
            h += '<div class=s>' + ''.join('<span style="width:%dpx;height:%dpx;display:inline-block">%s</span>' % (s, s, themed(svg, theme, icon, fam, s).replace('<svg ', '<svg width=%d height=%d ' % (s, s), 1)) for s in (16, 20, 26)) + '</div>' + icon + '</div>'
        h += '</div>'
open(out, 'w').write(h)
