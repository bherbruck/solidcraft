"""Shared semantic colour policy for repository icon tools; values live in palette.toml."""
import copy
import hashlib
import os
import re
import tomllib
import uuid
import xml.etree.ElementTree as ET

VISIONS = ('off', 'red-green', 'blue-yellow')


def validation_errors(palette, ids):
    errors, assigned = [], set()
    accents = palette.get('accents', {})
    for accent, icons in palette.get('icon-accents', {}).items():
        if accent not in accents:
            errors.append('unknown semantic accent ' + accent)
        for icon in icons:
            if icon not in ids or icon in assigned:
                errors.append('unknown or repeated semantic icon ' + icon)
            assigned.add(icon)
    groups = [('', accents)] + list(palette.get('variants', {}).items())
    for vision, group in groups:
        if vision and vision not in VISIONS:
            errors.append('unknown colour-vision group ' + vision)
        for accent, roles in group.items():
            if accent not in accents:
                errors.append('variant has unknown accent ' + accent)
            for role, value in roles.items():
                if role == 'guide':
                    errors.append('Guides use the shared guide role, never an accent override')
                if role not in palette['roles']:
                    errors.append('unknown colour role ' + role)
                if 'use' in value:
                    value = roles.get(value['use'], {})
                for theme in ('light', 'dark'):
                    if not re.fullmatch(r'#[0-9a-fA-F]{6}', value.get(theme, '')):
                        errors.append(f'{vision}/{accent}/{role}: invalid {theme} hex or role link')
    return errors


def default_accent(palette, icon):
    return next((accent for accent, ids in palette.get('icon-accents', {}).items() if icon in ids), '')


def resolve(palette, aliases, icon):
    """Explicit alias accents win; defaults belong to an id, not its shared master."""
    alias = aliases.get(icon, icon)
    if isinstance(alias, dict):
        return alias['master'], alias.get('accent') or default_accent(palette, icon)
    return alias, default_accent(palette, icon)


def role_values(palette, theme, accent='', vision='off'):
    values = {role: value[theme] for role, value in palette['roles'].items()}
    overrides = palette.get('accents', {}).get(accent, {})
    for role, value in overrides.items():
        if role == 'guide': continue
        if 'use' in value:
            value = overrides[value['use']]
        values[role] = value[theme]
    for role, value in palette.get('variants', {}).get(vision, {}).get(accent, {}).items():
        if role == 'guide': continue
        values[role] = value[theme]
    return values


def colour_ref(palette, theme, accent, role, vision='off'):
    """A live-review variable for the exact token, including links and overrides."""
    path = 'roles.' + role
    group = palette.get('accents', {}).get(accent, {})
    if role != 'guide' and role in group:
        target = group[role].get('use', role)
        path = 'accents.' + accent + '.' + target
    if role != 'guide' and role in palette.get('variants', {}).get(vision, {}).get(accent, {}):
        path = 'variants.' + vision + '.' + accent + '.' + role
    value = role_values(palette, theme, accent, vision)[role]
    return 'var(' + variable(path, theme) + ',' + value + ')'


def variable(path, theme):
    return '--icon-palette-' + path.replace('.', '-') + '-' + theme


def tokens(palette):
    """Editable display paints only; stable SVG source identifiers are never exposed."""
    purposes = {'ink': 'Outline', 'line': 'Line art', 'passive-1': 'Top face',
                'passive-2': 'Left face', 'passive-3': 'Right face', 'gap': 'Knockout',
                'guide': 'Guides', 'active-1': 'Pale', 'active-2': 'Mid', 'active-3': 'Stroke'}
    result = []
    groups = [('roles', '', palette['roles'])]
    groups += [('accents.' + name, 'Constr.' if name == 'construction' else name.title(), roles) for name, roles in palette.get('accents', {}).items()]
    groups += [('variants.' + vision + '.' + accent,
                accent.title() + (' R/G' if vision == 'red-green' else ' B/Y'), roles)
               for vision, accents in palette.get('variants', {}).items() for accent, roles in accents.items()]
    for prefix, group, roles in groups:
        for role, value in roles.items():
            linked = prefix + '.' + value['use'] if 'use' in value else ''
            paints = roles[value['use']] if linked else value
            purpose = purposes.get(role, role)
            if not group and role.startswith('active-'): purpose = 'Blue ' + purpose.lower()
            result.append({'id': prefix + '.' + role, 'name': (group + ' ' + purpose).strip(),
                           'light': paints['light'], 'dark': paints['dark'], 'linked': linked,
                           'knockout': prefix == 'roles' and role == 'gap'})
    return result


def palette_css(palette):
    return ':root{' + ''.join(variable(token['id'], theme) + ':' + token[theme] + ';'
                             for token in tokens(palette) if not token['linked'] and not token['knockout']
                             for theme in ('light', 'dark')) + '}'


def revision(path):
    # As save_colours reads it: text, so a CRLF checkout's line endings read as LF.
    with open(path, encoding='utf-8') as source: return hashlib.sha256(source.read().encode()).hexdigest()


def save_colours(path, base_revision, changes):
    """Apply validated display colours atomically, preserving comments and source ids."""
    with open(path, encoding='utf-8') as source: text = source.read()
    if hashlib.sha256(text.encode()).hexdigest() != base_revision:
        raise ValueError('Palette changed on disk. Reload before applying your colours.')
    palette = tomllib.loads(text)
    editable = {token['id'] for token in tokens(palette) if not token['linked'] and not token['knockout']}
    if not isinstance(changes, dict) or not changes or not set(changes) <= editable:
        raise ValueError('Unknown, linked or transparent palette token')
    expected = copy.deepcopy(palette)
    for key, paints in changes.items():
        if not isinstance(paints, dict) or set(paints) != {'light', 'dark'} or any(
                not isinstance(value, str) or not re.fullmatch(r'#[0-9a-fA-F]{6}', value) for value in paints.values()):
            raise ValueError('Use six-digit hex colours for both themes')
        target = expected
        for part in key.split('.'): target = target[part]
        target.update({theme: colour.lower() for theme, colour in paints.items()})
        section = re.compile(r'(?m)^\[' + re.escape(key) + r'\][^\n]*\n(?:(?!^\[)[\s\S])*')
        match = section.search(text)
        if not match: raise ValueError('Missing palette section ' + key)
        block = match[0]
        for theme, colour in paints.items():
            block, count = re.subn(r'(?m)^(\s*' + theme + r'\s*=\s*)["\']#[0-9a-fA-F]{6}["\']',
                                  lambda m: m[1] + '"' + colour.lower() + '"', block)
            if count != 1: raise ValueError('Missing colour in ' + key)
        text = text[:match.start()] + block + text[match.end():]
    if tomllib.loads(text) != expected: raise ValueError('Palette update changed unrelated data')
    temporary = str(path) + '.' + uuid.uuid4().hex + '.tmp'
    try:
        with open(temporary, 'w', encoding='utf-8') as output: output.write(text)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary): os.unlink(temporary)


def transparent_knockouts(svg, source):
    """GAP erases preceding artwork; later foreground paints remain untouched.

    Masks are generated only for display. The accepted SVG remains the editable
    authority, with GAP as its stable source identifier. Literal mask white/black
    are coverage values, not palette roles.
    """
    if source.lower() not in svg.lower(): return svg
    ns = 'http://www.w3.org/2000/svg'
    ET.register_namespace('', ns)
    root = ET.fromstring(svg)
    tag = lambda node: node.tag.rsplit('}', 1)[-1]
    element = lambda name, attrs: ET.Element('{' + ns + '}' + name, attrs)
    definitions, metadata, output = [], [], []
    used = {node.get('id') for node in root.iter() if node.get('id')}
    serial = 0

    def wrap(node, ancestors):
        for attrs in reversed(ancestors):
            group = element('g', attrs)
            group.append(node)
            node = group
        return node

    def erase(node, ancestors):
        nonlocal serial, output
        if not output: return
        serial += 1
        while 'sc-knockout-' + str(serial) in used: serial += 1
        name = 'sc-knockout-' + str(serial)
        used.add(name)
        mask = element('mask', {'id': name, 'maskUnits': 'userSpaceOnUse',
                               'maskContentUnits': 'userSpaceOnUse', 'x': '0', 'y': '0',
                               'width': '64', 'height': '64', 'mask-type': 'luminance'})
        mask.append(element('path', {'d': 'M0 0H64V64H0Z', 'fill': 'white'}))
        mask.append(wrap(node, ancestors))
        definitions.append(mask)
        group = element('g', {'mask': 'url(#' + name + ')'})
        group.extend(output)
        output = [group]

    def visit(node, ancestors=()):
        kind = tag(node)
        if kind == 'defs':
            definitions.extend(copy.deepcopy(list(node)))
        elif kind == 'title':
            metadata.append(copy.deepcopy(node))
        elif kind == 'g':
            attrs = {key: value for key, value in node.attrib.items() if key != 'id'}
            for child in node: visit(child, (*ancestors, attrs))
        else:
            fill_cut = node.get('fill', '').lower() == source.lower()
            stroke_cut = node.get('stroke', '').lower() == source.lower()
            if not fill_cut and not stroke_cut:
                output.append(wrap(copy.deepcopy(node), ancestors))
                return
            # SVG paints fill before stroke. Split only affected shapes so a
            # normal stroke is drawn after its cutout fill (and vice versa).
            for part, cut in [('fill', fill_cut), ('stroke', stroke_cut)]:
                paint = node.get(part, 'none')
                if paint == 'none': continue
                shape = copy.deepcopy(node)
                shape.attrib.pop('id', None)
                shape.set('stroke' if part == 'fill' else 'fill', 'none')
                if cut:
                    shape.set(part, 'black')
                    erase(shape, ancestors)
                else:
                    output.append(wrap(shape, ancestors))

    for child in root: visit(child)
    root[:] = []
    if definitions:
        defs = element('defs', {})
        defs.extend(definitions)
        root.append(defs)
    root.extend(metadata)
    root.extend(output)
    return ET.tostring(root, encoding='unicode')


def themed(svg, palette, theme, accent='', vision='off', live=False, namespace=''):
    gap = palette['roles'].get('gap')
    if gap: svg = transparent_knockouts(svg, gap.get('source', gap['light']))
    values = role_values(palette, theme, accent, vision)
    if live: values = {role: colour_ref(palette, theme, accent, role, vision) for role in values}
    # Only non-blue dashed references become guides. Original blue action paints
    # keep their semantic accent. Knockout mask coverage stays literal black.
    preserved = {value.get('source', value['light']).lower() for role, value in palette['roles'].items()
                 if role == 'gap' or role.startswith('active-')}
    # Resolve this paint after hex substitution, so a guide colour that happens
    # to equal another source hex cannot be remapped a second time.
    dashed = {}
    def protect_dash(match):
        tag = match[0]
        if 'stroke-dasharray=' not in tag: return tag
        def substitute(paint):
            if paint[2].lower() in preserved: return paint[0]
            key = '__GUIDE_PAINT_' + str(len(dashed)) + '__'; dashed[key] = values['guide']
            return 'stroke="' + key + '"'
        return re.sub(r'stroke=(["\'])(#[0-9a-fA-F]{6}|currentColor)\1', substitute, tag)
    svg = re.sub(r'<(?:path|circle)\b[^>]*>', protect_dash, svg)
    paints = {value.get('source', value['light']).lower(): values[role] for role, value in palette['roles'].items() if role != 'line'}
    svg = re.sub(r'#[0-9a-fA-F]{6}', lambda match: paints.get(match[0].lower(), match[0]), svg)
    svg = svg.replace('currentColor', values['line'])
    for key, value in dashed.items(): svg = svg.replace(key, value)
    if namespace:
        # Inline SVG ids share the page's global id namespace, including hidden
        # theme/vision/size variants and aliases of the same master.
        namespace = re.sub(r'[^a-zA-Z0-9_-]', '-', namespace)
        ids = set(re.findall(r'\bid=["\']([^"\']+)["\']', svg))
        for name in ids:
            scoped = namespace + '-' + name
            svg = re.sub(r'\bid=(["\'])' + re.escape(name) + r'\1', lambda m: 'id="' + scoped + '"', svg)
            svg = svg.replace('url(#' + name + ')', 'url(#' + scoped + ')')
    return svg


def review_css(palette, live=False):
    """Review controls follow the same shared colours; artwork is rendered by themed()."""
    rules = []
    for dark in (False, True):
        theme = 'dark' if dark else 'light'
        for vision in VISIONS:
            selector = 'body' + ('.dark' if dark else ':not(.dark)') + '[data-vision="' + vision + '"]'
            primary = lambda accent='': colour_ref(palette, theme, accent, 'active-3' if 'active-3' in palette['roles'] else 'line', vision) if live else role_values(palette, theme, accent, vision).get('active-3', palette['roles']['line'][theme])
            values = {'accent': primary(),
                      'accent-ink': colour_ref(palette, 'dark', '', 'line', vision) if live else palette['roles']['line']['dark'],
                      'success': primary('sketch'),
                      'warning': primary('mesh'),
                      'error': primary('constraint')}
            rules.append(selector + '{' + ''.join('--review-' + key + ':' + value + ';' for key, value in values.items()) + '}')
            for accent in palette.get('accents', {}):
                rules.append(selector + ' .card[data-accent="' + accent + '"]{--icon-accent:' + primary(accent) + '}')
    return '\n'.join(rules)
